/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use cap_std::fs::Dir;

pub(crate) fn open() -> Dir {
    Dir::open_ambient_dir("/package", cap_std::ambient_authority())
        .expect("could not open /package")
}