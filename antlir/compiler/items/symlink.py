#!/usr/bin/env python3
# Copyright (c) Facebook, Inc. and its affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

import os
import pwd

from antlir.compiler.requires_provides import (
    ProvidesSymlink,
    RequireDirectory,
    RequireFile,
)
from antlir.fs_utils import Path, generate_work_dir
from antlir.nspawn_in_subvol.args import PopenArgs, new_nspawn_opts
from antlir.nspawn_in_subvol.nspawn import run_nspawn
from antlir.subvol_utils import Subvol
from pydantic import root_validator

from .common import (
    ImageItem,
    LayerOpts,
    make_path_normal_relative,
    validate_path_field_normal_relative,
)
from .symlink_t import symlink_t


def _make_rsync_style_dest_path(dest: str, source: str) -> str:
    """
    rsync convention for a destination: "ends/in/slash/" means "write
    into this directory", "does/not/end/with/slash" means "write with
    the specified filename".
    """

    # Normalize after applying the rsync convention, since this would
    # remove any trailing / in 'dest'.
    return make_path_normal_relative(
        os.path.join(dest, os.path.basename(source))
        if dest.endswith("/")
        else dest
    )


class SymlinkBase(symlink_t, ImageItem):
    _normalize_source = validate_path_field_normal_relative("source")

    @root_validator(pre=True)
    def dest_is_rsync_style(cls, values):  # noqa B902
        # Validators are classmethods but flake8 doesn't catch that.
        values["dest"] = _make_rsync_style_dest_path(  # def provides(self):
            #     yield ProvidesFile(path=self.dest)
            values["dest"],
            values["source"],
        )
        return values

    def provides(self):
        yield ProvidesSymlink(path=self.dest, target=self.source)

    def build(self, subvol: Subvol, layer_opts: LayerOpts):
        dest = subvol.path(self.dest)
        # Best-practice would tell us to do `subvol.path(self.source)`.
        # However, this will trigger the paranoid check in the `path()`
        # implementation if any component of `source` inside the image is an
        # absolute symlink.  We are not writing to `source`, so that
        # safeguard isn't useful here.
        #
        # We DO check below that the relative symlink we made does not point
        # outside the image.  However, a non-chrooted process resolving our
        # well-formed relative link might still traverse pre-existing
        # absolute symlinks on the filesystem, and go outside of the image
        # root.
        abs_source = subvol.path() / self.source
        # Make all symlinks relative because this makes it easy to inspect
        # the subvolums from outside the container.  We can add an
        # `absolute` option if needed.
        rel_source = abs_source.relpath(dest.dirname())
        assert os.path.normpath(dest / rel_source).startswith(
            subvol.path()
        ), f"{self}: A symlink to {rel_source} would point outside the image"
        if os.path.exists(dest):
            if not os.path.islink(dest):
                raise RuntimeError(f"{self}: dest already exists")
            # Should we check abs_source.relpath(os.path.realpath(dest))?
            # If so, we may also need to check that os.readlink(dest) does
            # not point outside subvol.path(). This currently errors if an
            # existing symlink does not matches exactly this item would've
            # created.
            current_link = os.readlink(dest)
            if current_link == rel_source:
                return
            raise RuntimeError(
                f"{self}: {self.dest} -> {self.source} exists to {current_link}"
            )
        if layer_opts.build_appliance:
            build_appliance = layer_opts.build_appliance
            work_dir = generate_work_dir()
            rel_dest = work_dir / self.dest
            opts = new_nspawn_opts(
                cmd=[
                    "ln",
                    "--symbolic",
                    "--no-dereference",
                    rel_source,
                    rel_dest,
                ],
                layer=build_appliance,
                bindmount_rw=[(subvol.path(), work_dir)],
                user=pwd.getpwnam("root"),
            )
            run_nspawn(opts, PopenArgs())
        else:
            subvol.run_as_root(
                ["ln", "--symbolic", "--no-dereference", rel_source, dest]
            )


class SymlinkToDirItem(SymlinkBase):
    def requires(self):
        yield RequireDirectory(path=self.source)
        yield RequireDirectory(path=self.dest.dirname())


# We should allow symlinks to certain files that will be in the image
# at runtime but may not be at build time.
def _allowlisted_symlink_source(source: Path) -> bool:
    return source in [b"dev/null"]


class SymlinkToFileItem(SymlinkBase):
    def requires(self):
        if not _allowlisted_symlink_source(self.source):
            yield RequireFile(path=self.source)
        yield RequireDirectory(path=self.dest.dirname())
