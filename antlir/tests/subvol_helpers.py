#!/usr/bin/env python3
# Copyright (c) Facebook, Inc. and its affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

import copy
import subprocess

from antlir.btrfs_diff.tests.render_subvols import render_sendstream

from ..subvol_utils import Subvol


# The easiest way to render a subvolume in a test.
def render_subvol(subvol: Subvol):
    # Determine the original ro/rw state of the subvol so we can put it back
    # the way it was after rendering.
    was_readonly = (
        subvol.run_as_root(
            ["btrfs", "property", "get", "-ts", subvol.path(), "ro"],
            text=True,
            stdout=subprocess.PIPE,
        ).stdout.strip()
        == "ro=true"
    )
    try:
        return render_sendstream(subvol.mark_readonly_and_get_sendstream())
    finally:
        subvol.set_readonly(was_readonly)


def pop_path(render, path):
    assert not isinstance(path, bytes), path  # Renderings are `str`
    parts = path.lstrip("/").split("/")
    for part in parts[:-1]:
        render = render[1][part]
    return render[1].pop(parts[-1])


# Future: this isn't really the right place for it, but for now we just have
# 2 places that need it, and it's annoying to create a whole new module just
# for this helper.
def check_common_rpm_render(test, rendered_subvol, yum_dnf: str):
    r = copy.deepcopy(rendered_subvol)

    # Ignore a bunch of yum / dnf / rpm spam

    if yum_dnf == "yum":
        (ino,) = pop_path(r, "var/log/yum.log")
        test.assertRegex(ino, r"^\(File m600 d[0-9]+\)$")
        ino, _ = pop_path(r, "var/lib/yum")
        test.assertEqual("(Dir)", ino)
    elif yum_dnf == "dnf":
        test.assertEqual(
            ["(Dir)", {"dnf": ["(Dir)", {"modules.d": ["(Dir)", {}]}]}],
            pop_path(r, "etc"),
        )
        for logname in [
            "dnf.log",
            "dnf.librepo.log",
            "dnf.rpm.log",
            "hawkey.log",
        ]:
            (ino,) = pop_path(r, f"var/log/{logname}")
            test.assertRegex(ino, r"^\(File d[0-9]+\)$", logname)
        ino, _ = pop_path(r, "var/lib/dnf")
        test.assertEqual("(Dir)", ino)
    else:
        raise AssertionError(yum_dnf)

    ino, _ = pop_path(r, "var/lib/rpm")
    test.assertEqual("(Dir)", ino)

    test.assertEqual(
        [
            "(Dir)",
            {
                "dev": ["(Dir)", {}],
                ".meta": [
                    "(Dir)",
                    {
                        "private": [
                            "(Dir)",
                            {
                                "opts": [
                                    "(Dir)",
                                    {
                                        "artifacts_may_require_repo": [
                                            "(File d2)"
                                        ]
                                    },
                                ]
                            },
                        ]
                    },
                ],
                "var": ["(Dir)", {"lib": ["(Dir)", {}], "log": ["(Dir)", {}]}],
            },
        ],
        r,
    )
