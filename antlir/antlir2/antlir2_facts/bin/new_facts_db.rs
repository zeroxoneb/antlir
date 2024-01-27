/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use antlir2_facts::fact::dir_entry::DirEntry;
use antlir2_facts::fact::dir_entry::FileCommon;
use antlir2_facts::fact::dir_entry::Symlink;
use antlir2_facts::fact::user::Group;
use antlir2_facts::fact::user::User;
use antlir2_facts::Database;
use antlir2_users::group::EtcGroup;
use antlir2_users::passwd::EtcPasswd;
use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use jwalk::WalkDir;
use tracing::trace;

#[derive(Parser)]
struct Args {
    #[clap(long)]
    root: PathBuf,
    #[clap(long)]
    db: PathBuf,
    #[clap(long)]
    rootless: bool,
}

fn populate(db: &mut Database, root: &Path) -> Result<()> {
    let root = root.canonicalize().context("while canonicalizing root")?;
    populate_files(db, &root)?;
    populate_usergroups(db, &root)?;
    Ok(())
}

fn populate_files(db: &mut Database, root: &Path) -> Result<()> {
    for entry in WalkDir::new(root) {
        let entry = entry?;
        let full_path = entry.path();
        let relpath = full_path
            .strip_prefix(root)
            .context("all paths must start with root dir")?;
        let path = Path::new("/").join(relpath);
        trace!("adding {path:?}");
        let meta = entry
            .metadata()
            .with_context(|| format!("while statting {}", full_path.display()))?;
        let common = FileCommon::new_with_metadata(path.clone(), &meta);
        if entry.file_type().is_dir() {
            db.insert(&DirEntry::Directory(common.into()))?;
        } else if entry.file_type().is_symlink() {
            let raw_target = std::fs::read_link(&full_path)
                .with_context(|| format!("while reading raw link {}", full_path.display()))?;
            db.insert(&DirEntry::Symlink(Symlink::new(common, raw_target)))?;
        } else if entry.file_type().is_file() {
            db.insert(&DirEntry::RegularFile(common.into()))?;
        } else {
            bail!(
                "{} was not a directory, symlink or file",
                full_path.display()
            );
        }
    }
    Ok(())
}

fn populate_usergroups(db: &mut Database, root: &Path) -> Result<()> {
    let user_db: EtcPasswd = match std::fs::read_to_string(root.join("etc/passwd")) {
        Ok(contents) => contents.parse().context("while parsing /etc/passwd"),
        Err(e) => match e.kind() {
            ErrorKind::NotFound => Ok(Default::default()),
            _ => Err(anyhow::Error::from(e).context("while reading /etc/passwd")),
        },
    }?;
    for user in user_db.into_records() {
        db.insert(&User::new(user.name.clone(), user.uid.into()))
            .with_context(|| format!("while inserting user '{}'", user.name))?;
    }
    let group_db: EtcGroup = match std::fs::read_to_string(root.join("etc/group")) {
        Ok(contents) => contents.parse().context("while parsing /etc/group"),
        Err(e) => match e.kind() {
            ErrorKind::NotFound => Ok(Default::default()),
            _ => Err(anyhow::Error::from(e).context("while reading /etc/group")),
        },
    }?;
    for group in group_db.into_records() {
        db.insert(&Group::new(
            group.name.clone(),
            group.gid.into(),
            group.users,
        ))
        .with_context(|| format!("while inserting group '{}'", group.name))?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt::init();

    let rootless = if args.rootless {
        antlir2_rootless::unshare_new_userns().context("while setting up userns")?;
        None
    } else {
        Some(antlir2_rootless::init().context("while dropping privileges")?)
    };

    if args.db.exists() {
        bail!(
            "{} already exists - populate currently only works with completely new dbs",
            args.db.display()
        );
    }
    let mut db =
        Database::open_readwrite(&args.db, rocksdb::Options::new().create_if_missing(true))
            .with_context(|| format!("while opening db {}", args.db.display()))?;

    let uid = nix::unistd::Uid::effective().as_raw();
    let gid = nix::unistd::Gid::effective().as_raw();

    let root_guard = rootless.map(|r| r.escalate()).transpose()?;
    populate(&mut db, &args.root)?;

    // make sure all the output files are owned by the unprivileged user
    for entry in jwalk::WalkDir::new(&args.db) {
        let entry = entry?;
        let path = entry.path();
        std::os::unix::fs::lchown(&path, Some(uid), Some(gid))
            .with_context(|| format!("while chowning {}", path.display()))?;
    }
    drop(root_guard);

    Ok(())
}
