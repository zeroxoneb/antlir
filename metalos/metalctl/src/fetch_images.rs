/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use anyhow::{Context, Result};
use slog::Logger;
use structopt::StructOpt;
use url::Url;

use image::download::HttpsDownloader;
use image::Package;

use get_host_config::get_host_config;

#[derive(StructOpt)]
pub struct Opts {
    host_config_uri: Url,
}

/// Fetch all the immediately-necessary images from the host config. If in the
/// initrd, this is just the rootfs and kernel.
pub async fn fetch_images(log: Logger, opts: Opts) -> Result<()> {
    let host = get_host_config(&opts.host_config_uri)
        .await
        .with_context(|| format!("while loading host config from {} ", opts.host_config_uri))?;

    // TODO: use fbpkg.proxy when in the rootfs
    let dl = HttpsDownloader::new().context("while creating downloader")?;

    let (root_subvol, kernel_subvol) = tokio::join!(
        host.boot_config.rootfs.download(log.clone(), &dl),
        host.boot_config.kernel.download(log, &dl),
    );
    let root_subvol = root_subvol.context("while downloading rootfs")?;
    let kernel_subvol = kernel_subvol.context("while downloading kernel")?;
    // TODO: download service images as well

    let kernel_modules_subdir = kernel_subvol.path().join("modules");
    let kernel_modules_path = match kernel_modules_subdir.exists() {
        true => kernel_modules_subdir,
        false => kernel_subvol.path().into(),
    };

    // TODO: onboard this to systemd_generator_lib if there is a lot more that
    // needs to be included here
    std::fs::write(
        "/run/metalos/image_paths_environment",
        format!(
            "METALOS_OS_VOLUME={}\nMETALOS_KERNEL_MODULES_PATH={}\nMETALOS_KERNEL_SUBVOLID={}\n",
            root_subvol.path().display(),
            kernel_modules_path.display(),
            kernel_subvol.id(),
        ),
    )
    .context("while writing /run/metalos/image_paths_environment")?;

    Ok(())
}
