/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use std::path::PathBuf;
use std::process::Command;

use antlir2_btrfs::Subvolume;
use antlir2_cas_dir::CasDir;
use antlir2_working_volume::WorkingVolume;
use anyhow::anyhow;
use anyhow::ensure;
use anyhow::Context;
use anyhow::Result;
use buck_label::Label;
use clap::Parser;
use clap::ValueEnum;
use nix::sched::unshare;
use nix::sched::CloneFlags;
use nix::unistd::Uid;
use tracing::trace;
use tracing_subscriber::prelude::*;

#[derive(Parser, Debug)]
/// Receive a pre-built image package into the local working volume.
pub(crate) struct Receive {
    #[clap(long)]
    /// Label of the image being build
    label: Label,
    #[clap(long)]
    /// Path to the image file
    source: PathBuf,
    #[clap(long, value_enum)]
    /// Format of the image file
    format: Format,
    #[clap(long)]
    /// buck-out path to store the reference to this volume
    output: PathBuf,
    #[clap(flatten)]
    setup: SetupArgs,
    #[clap(long)]
    /// Use an unprivileged usernamespace
    rootless: bool,
}

#[derive(Debug, Copy, Clone, ValueEnum)]
pub enum Format {
    #[clap(name = "sendstream")]
    Sendstream,
    #[clap(name = "cas_dir")]
    CasDir,
}

#[derive(Parser, Debug)]
struct SetupArgs {
    #[clap(long)]
    /// Path to the working volume where images should be built
    working_dir: PathBuf,
}

impl Receive {
    /// Make sure that the working directory exists and clean up any existing
    /// version of the subvolume that we're receiving.
    #[tracing::instrument(skip(self), ret, err(Debug))]
    fn prepare_dst(&self, working_volume: &WorkingVolume) -> Result<PathBuf> {
        let dst = working_volume
            .allocate_new_path()
            .context("while allocating new path for subvol")?;
        trace!("WorkingVolume gave us new path {}", dst.display());
        Ok(dst)
    }

    #[tracing::instrument(name = "receive", skip(self))]
    pub(crate) fn run(self) -> Result<()> {
        trace!("setting up WorkingVolume");
        let is_real_root = Uid::effective().is_root();

        let rootless = antlir2_rootless::init().context("while setting up antlir2_rootless")?;

        let working_volume = WorkingVolume::ensure(self.setup.working_dir.clone())
            .context("while setting up WorkingVolume")?;

        if self.rootless {
            // It's actually surprisingly tricky to make the same code paths
            // work when both unprivileged and running as real root, so just
            // don't allow it.
            ensure!(
                !is_real_root,
                "cannot be real root if --rootless is being used"
            );

            antlir2_rootless::unshare_new_userns().context("while setting up userns")?;
            unshare(CloneFlags::CLONE_NEWNS).context("while unsharing mount")?;
        }

        let dst = self.prepare_dst(&working_volume)?;

        let root = rootless.escalate()?;

        match self.format {
            Format::Sendstream => {
                let recv_tmp = tempfile::tempdir_in(&self.setup.working_dir)?;
                let mut cmd = Command::new("btrfs");
                cmd.arg("--quiet")
                    .arg("receive")
                    .arg(recv_tmp.path())
                    .arg("-f")
                    .arg(&self.source);
                trace!("receiving sendstream: {cmd:?}");
                let res = cmd.spawn()?.wait()?;
                ensure!(res.success(), "btrfs-receive failed");
                let entries: Vec<_> = std::fs::read_dir(&recv_tmp)
                    .context("while reading tmp dir")?
                    .map(|r| {
                        r.map(|entry| entry.path())
                            .context("while iterating tmp dir")
                    })
                    .collect::<Result<_>>()?;
                if entries.len() != 1 {
                    return Err(anyhow!(
                        "did not get exactly one subvolume received: {entries:?}"
                    ));
                }

                trace!("opening received subvol: {}", entries[0].display());
                let mut subvol = Subvolume::open(&entries[0]).context("while opening subvol")?;
                subvol
                    .set_readonly(false)
                    .context("while making subvol rw")?;

                trace!(
                    "moving received subvol to right location {} -> {}",
                    subvol.path().display(),
                    dst.display()
                );
                std::fs::rename(subvol.path(), &dst).context("while renaming subvol")?;
            }
            Format::CasDir => {
                let subvol = Subvolume::create(&dst).context("while creating subvol")?;
                let cas_dir = CasDir::open(self.source).context("while opening CasDir")?;
                cas_dir
                    .hydrate_into(subvol.path())
                    .context("while materializing CasDir")?;
            }
        };
        let mut subvol = Subvolume::open(&dst).context("while opening subvol")?;

        subvol
            .set_readonly(true)
            .context("while making subvol ro")?;

        drop(root);

        std::os::unix::fs::symlink(subvol.path(), &self.output).context("while making symlink")?;

        rootless.as_root(|| {
            working_volume.keep_path_alive(&dst, &self.output)?;
            trace!(
                "marked path {} with keepalive {}",
                dst.display(),
                self.output.display()
            );
            working_volume
                .collect_garbage()
                .context("while garbage collecting old outputs")
        })??;
        Ok(())
    }
}

fn main() -> Result<()> {
    let args = Receive::parse();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::Layer::default()
                .event_format(
                    tracing_glog::Glog::default()
                        .with_span_context(true)
                        .with_timer(tracing_glog::LocalTime::default()),
                )
                .fmt_fields(tracing_glog::GlogFields::default()),
        )
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    args.run()
}
