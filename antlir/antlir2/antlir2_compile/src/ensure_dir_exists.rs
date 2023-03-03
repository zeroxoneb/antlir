/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use std::fs::Permissions;
use std::os::unix::fs::chown;
use std::os::unix::fs::PermissionsExt;

use antlir2_features::ensure_dir_exists::EnsureDirExists;

use crate::CompileFeature;
use crate::CompilerContext;
use crate::Result;

impl<'a> CompileFeature for EnsureDirExists<'a> {
    #[tracing::instrument(name = "ensure_dir_exists", skip(ctx), ret, err)]
    fn compile(&self, ctx: &CompilerContext) -> Result<()> {
        let dst = ctx.dst_path(&self.dir);
        tracing::trace!("creating {}", dst.display());
        match std::fs::create_dir(&dst) {
            Ok(_) => {
                let uid = ctx.uid(self.user.name())?;
                let gid = ctx.gid(self.group.name())?;
                chown(&dst, Some(uid.into()), Some(gid.into())).map_err(std::io::Error::from)?;
                std::fs::set_permissions(&dst, Permissions::from_mode(self.mode.0))?;
            }
            Err(e) => match e.kind() {
                // The directory may have already been created by a concurrent [EnsureDirsExist]
                // This is safe to ignore because the depgraph will already
                // have validated that the ownership and modes are identical
                std::io::ErrorKind::AlreadyExists => {
                    tracing::debug!(dst = dst.display().to_string(), "dir already existed");
                }
                _ => return Err(e.into()),
            },
        }
        Ok(())
    }
}
