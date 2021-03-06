# Copyright (c) Facebook, Inc. and its affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

load("//antlir/bzl:shape.bzl", "shape")

kernel_artifacts_t = shape.shape(
    vmlinuz = shape.target(),
    # devel and modules may not exist, such as in the case of a vmlinuz with
    # all necessary features compiled with =y
    devel = shape.target(optional = True),
    modules = shape.target(optional = True),
)

kernel_t = shape.shape(
    uname = str,
    artifacts = shape.field(kernel_artifacts_t),
)

def normalize_kernel(kernel):
    # Convert from a struct kernel struct format
    # into a kernel shape instance.  Note, if the provided `kernel` attr
    # is already a shape instance, this just makes another one. Wasteful, yes
    # but we don't have an `is_shape` mechanism yet to avoid something like
    # this.
    return shape.new(
        kernel_t,
        uname = kernel.uname,
        artifacts = shape.new(
            kernel_artifacts_t,
            devel = kernel.artifacts.devel,
            modules = kernel.artifacts.modules,
            vmlinuz = kernel.artifacts.vmlinuz,
        ),
    )
