# Copyright (c) Facebook, Inc. and its affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

import os
import unittest

from antlir.artifacts_dir import (
    find_repo_root,
    ensure_per_repo_artifacts_dir_exists,
    find_buck_cell_root,
)
from antlir.fs_utils import Path, temp_dir


class ArtifactsDirTests(unittest.TestCase):
    def test_git_repo_root(self):
        with temp_dir() as td:
            # Make the td the repo root
            os.makedirs(td / b".git")

            # A subdir to start from, a very good place to start.
            repo_subdir = td / "i/am/a/subdir/of/the/repo"
            os.makedirs(repo_subdir)

            # Git has submodules, make one of those like git does
            repo_submodule_subdir = td / "i/am/a/submodule/subdir"
            os.makedirs(repo_submodule_subdir)
            Path(repo_submodule_subdir.dirname() / ".git").touch()

            # Check all the possible variations
            self.assertEqual(find_repo_root(path_in_repo=td), td)
            self.assertEqual(find_repo_root(path_in_repo=repo_subdir), td)
            self.assertEqual(
                find_repo_root(path_in_repo=repo_submodule_subdir), td
            )

    def test_hg_repo_root(self):
        with temp_dir() as td:
            # Make the td the repo root
            os.makedirs(td / b".hg")

            repo_subdir = td / "i/am/a/subdir/of/the/repo"
            os.makedirs(repo_subdir)

            # Check all the possible variations
            self.assertEqual(find_repo_root(path_in_repo=td), td)
            self.assertEqual(find_repo_root(path_in_repo=repo_subdir), td)

    def test_ensure_per_repo_artifacts_dir_exists(self):
        with temp_dir() as td:
            # Make the td the buck cell root
            (td / b".buckconfig").touch()

            repo_subdir = td / "i/am/a/subdir/of/the/repo"
            os.makedirs(repo_subdir)

            artifacts_dir = ensure_per_repo_artifacts_dir_exists(repo_subdir)
            self.assertEqual(td / "buck-image-out", artifacts_dir)
            self.assertTrue(artifacts_dir.exists())
            self.assertTrue((artifacts_dir / "clean.sh").exists())

    def test_find_buck_cell_root(self):
        with temp_dir() as td:
            # Make the td the buck cell root
            (td / b".buckconfig").touch()

            repo_subdir = td / "i/am/a/subdir/of/the/repo"
            os.makedirs(repo_subdir)
            have = find_buck_cell_root(repo_subdir)
            self.assertEqual(td, have)

    def test_find_buck_cell_root_missing(self):
        with temp_dir() as td:
            try:
                find_buck_cell_root(td)
            except RuntimeError:
                return

            self.fail("Expected RuntimeError when missing .buckconfig")
