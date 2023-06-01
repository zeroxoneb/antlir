/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;

use antlir2_features::rpms::Item as RpmItem;
use antlir2_features::rpms::Rpm;
use anyhow::Context;
use anyhow::Error;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Deserializer;
use tracing::trace;

use crate::plan::DnfTransaction;
use crate::plan::Item;
use crate::Arch;
use crate::CompileFeature;
use crate::CompilerContext;
use crate::Result;

#[derive(Debug, Serialize)]
struct DriverSpec<'a> {
    install_root: &'a Path,
    repos: &'a Path,
    items: &'a [RpmItem<'a>],
    mode: DriverMode,
    arch: Arch,
    versionlock: Option<&'a BTreeMap<String, String>>,
    excluded_rpms: &'a BTreeSet<String>,
}

#[derive(Debug, Copy, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
enum DriverMode {
    ResolveOnly,
    Run,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransactionOperation {
    /// Package cleanup is being performed.
    Cleanup,
    /// Package is being installed as a downgrade
    Downgrade,
    /// Installed package is being downgraded
    Downgraded,
    /// Package is being installed
    Install,
    /// Package is obsoleting another package
    Obsolete,
    /// Installed package is being obsoleted
    Obsoleted,
    /// Package is installed as a reinstall
    Reinstall,
    /// Installed package is being reinstalled
    Reinstalled,
    /// Package is being removed
    Remove,
    /// Package is installed as an upgrade
    Upgrade,
    /// Installed package is being upgraded
    Upgraded,
    /// Package is being verified
    Verify,
    /// Package scriptlet is being performed
    Scriptlet,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
struct Package {
    name: String,
    epoch: i32,
    version: String,
    release: String,
    arch: String,
}

impl Package {
    fn nevra(&self) -> String {
        format!(
            "{}-{}:{}-{}.{}",
            self.name, self.epoch, self.version, self.release, self.arch
        )
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
struct InstallPackage {
    package: Package,
    repo: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // I want to log structured data
enum DriverEvent {
    TransactionResolved {
        install: BTreeSet<InstallPackage>,
        remove: BTreeSet<Package>,
    },
    TxItem {
        package: Package,
        operation: TransactionOperation,
    },
    TxError(String),
    GpgError {
        package: Package,
        error: String,
    },
    ScriptletOutput(String),
}

/// Relatively simple implementation of rpm features. This does not yet respect
/// version locks.
impl<'a> CompileFeature for Rpm<'a> {
    #[tracing::instrument(name = "rpms", skip(self, ctx), ret, err)]
    fn compile(&self, ctx: &CompilerContext) -> Result<()> {
        run_dnf_driver(ctx, &self.items, DriverMode::Run).map(|_| ())
    }

    fn plan(&self, ctx: &CompilerContext) -> Result<Item> {
        let events = run_dnf_driver(ctx, &self.items, DriverMode::ResolveOnly)?;
        if events.len() != 1 {
            return Err(Error::msg("expected exactly one event in resolve-only mode").into());
        }
        match &events[0] {
            DriverEvent::TransactionResolved { install, remove } => {
                Ok(Item::DnfTransaction(DnfTransaction {
                    install: install
                        .iter()
                        .map(|ip| crate::plan::InstallPackage {
                            nevra: ip.package.nevra(),
                            repo: ip.repo.clone(),
                        })
                        .collect(),
                    remove: remove.iter().map(|p| p.nevra()).collect(),
                }))
            }
            _ => Err(Error::msg("resolve-only event should have been TransactionResolved").into()),
        }
    }
}

fn run_dnf_driver(
    ctx: &CompilerContext,
    items: &[RpmItem<'_>],
    mode: DriverMode,
) -> Result<Vec<DriverEvent>> {
    let input = serde_json::to_string(&DriverSpec {
        install_root: ctx.root(),
        repos: ctx.dnf().repos(),
        items,
        mode,
        arch: ctx.target_arch(),
        versionlock: ctx.dnf.versionlock(),
        excluded_rpms: ctx.dnf.excluded_rpms(),
    })
    .context("while serializing dnf-driver input")?;

    let mut child = Command::new("/__antlir__/dnf/driver")
        .arg(&input)
        .stdout(Stdio::piped())
        .spawn()
        .context("while spawning dnf-driver")?;

    let deser = Deserializer::from_reader(child.stdout.take().expect("this is a pipe"));
    let mut events = Vec::new();
    for event in deser.into_iter::<DriverEvent>() {
        let event = event.context("while deserializing event from dnf-driver")?;
        trace!("dnf-driver: {event:?}");
        events.push(event);
    }
    let result = child.wait().context("while waiting for dnf-driver")?;
    if !result.success() {
        Err(Error::msg("dnf-driver failed").into())
    } else {
        // make sure there weren't any error events, if there was -> fail
        let errors: Vec<_> = events
            .iter()
            .filter_map(|ev| match ev {
                DriverEvent::TxError(error) => Some(error.as_str()),
                _ => None,
            })
            .collect();
        if !errors.is_empty() {
            return Err(
                anyhow::anyhow!("there were one or more transaction errors: {errors:?}").into(),
            );
        }
        Ok(events)
    }
}
