# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

load("//antlir/antlir2/bzl/feature:defs.bzl?v2_only", antlir2 = "feature")
load("//antlir/bzl:build_defs.bzl", "is_buck2")
load("//antlir/bzl:target_tagger.bzl", "new_target_tagger", "target_tagger_to_feature")
load(":requires.shape.bzl", "requires_t")

def feature_requires(
        users = None,
        groups = None,
        files = None):
    """
`feature.requires(...)` adds macro-level requirements on image layers.

Currently this supports requiring users, groups and files to exist in the layer
being built. This feature doesn't materialize anything in the built image, but it
will cause a compiler error if any of the users/groups that are requested do not
exist in either the `parent_layer` or the layer being built.

An example of a reasonable use-case of this functionality is defining a macro
that generates systemd units that run as a specific user, where
`feature.requires` can be used for additional compile-time safety that the user,
groups or files do indeed exist.
"""
    req = requires_t(
        users = users,
        groups = groups,
        files = files,
    )
    return target_tagger_to_feature(
        new_target_tagger(),
        items = struct(requires = [req]),
        antlir2_feature = antlir2.requires(
            users = users or [],
            groups = groups or [],
            files = files or [],
        ) if is_buck2() else None,
    )
