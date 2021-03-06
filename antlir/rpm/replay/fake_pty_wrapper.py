#!/usr/bin/env python3
# Copyright (c) Facebook, Inc. and its affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

import os
from typing import ContextManager, List

from antlir.fs_utils import MehStr, Path


def fake_pty_resource() -> ContextManager[Path]:
    return Path.resource(__package__, "fake_pty_real.py", exe=False)


def fake_pty_cmd(os_root: MehStr, fake_pty_path: MehStr) -> List[MehStr]:
    # Try to find a usable Python in the container that will run fake-pty
    os_root = Path(os_root)
    for python in [
        "usr/bin/python3",
        "usr/libexec/platform-python",
        "usr/bin/python2",
    ]:
        if os.access(os_root / python, os.X_OK):
            break
    else:  # pragma: no cover
        raise RuntimeError(f"Could not find Python in {os_root}")
    # pyre-fixme[61]: `python` may not be initialized here.
    return ["/" + python, fake_pty_path]
