# Copyright (c) Facebook, Inc. and its affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

load("@bazel_skylib//lib:paths.bzl", "paths")
load("//antlir/bzl/image/feature:defs.bzl", "feature")
load(":constants.bzl", "REPO_CFG")
load(":hoist.bzl", "hoist")
load(":image.bzl", "image")
load(":oss_shim.bzl", "buck_genrule", third_party_shim = "third_party")
load(":shape.bzl", "shape")

PREFIX = "/third_party_build"
SRC_TGZ = paths.join(PREFIX, "source.tar.gz")
SRC_DIR = paths.join(PREFIX, "src")
DEPS_DIR = paths.join(PREFIX, "deps")
STAGE_DIR = paths.join(PREFIX, "stage")
OUT_DIR = paths.join(PREFIX, "out")

def _prepare_layer(name, project, base_features, dependencies = []):
    source_target = third_party_shim.source(project)

    image.layer(
        name = "base-layer",
        features = [
            image.ensure_dirs_exist(SRC_DIR),
            image.ensure_dirs_exist(DEPS_DIR),
            image.ensure_dirs_exist(STAGE_DIR),
            image.ensure_dirs_exist(OUT_DIR),
            feature.install(source_target, SRC_TGZ),
            image.rpms_install(["tar", "fuse", "fuse-overlayfs"]),
        ] + base_features,
        flavor = REPO_CFG.antlir_linux_flavor,
    )

    image.layer(
        name = name,
        parent_layer = ":base-layer",
        features = [
            feature.install(dep.source, paths.join(DEPS_DIR, dep.name))
            for dep in dependencies
        ],
    )

def _cmd_prepare_dependency(dependency):
    """move the dependencies in the right places"""
    return "\n".join([
        "cp --reflink=auto -r {deps}/{name}/{path} {stage}".format(
            deps = DEPS_DIR,
            stage = STAGE_DIR,
            name = dependency.name,
            path = path,
        )
        for path in dependency.paths
    ])

def _native_build(base_features, script, dependencies = [], project = None):
    if not project:
        project = paths.basename(package_name())

    _prepare_layer(
        name = "setup-layer",
        base_features = base_features,
        dependencies = dependencies,
        project = project,
    )

    prepare_deps = "\n".join([
        _cmd_prepare_dependency(dep)
        for dep in dependencies
    ])

    image.genrule_layer(
        name = "build-layer",
        parent_layer = ":setup-layer",
        rule_type = "third_party_build",
        antlir_rule = "user-internal",
        user = "root",
        cmd = [
            "bash",
            "-uec",
            """
            set -eo pipefail

            # copy all specified dependencies
            {prepare_deps}

            # unpack the source in build dir
            cd "{src_dir}"
            tar xzf {src} --strip-components=1

            export STAGE="{stage_dir}"
            {prepare}
            {build}

            # trick the fs layer so that we can collect the installed files without
            # dependencies mixed in; while keeping correct paths in pkg-config
            mkdir {fswork_dir}
            mv {stage_dir} {stage_ro_dir}
            mkdir {stage_dir}
            fuse-overlayfs -o lowerdir="{stage_ro_dir}",upperdir="{out_dir}",workdir={fswork_dir} "{stage_dir}"

            {install}

            # unmount the overlay and remove whiteout files because we only want the
            # newly created ones by the install
            fusermount -u "{stage_dir}"
            find "{out_dir}" \\( -name ".wh.*" -o -type c \\) -delete
            """.format(
                src = SRC_TGZ,
                prepare_deps = prepare_deps,
                prepare = script.prepare,
                build = script.build,
                install = script.install,
                src_dir = SRC_DIR,
                stage_dir = STAGE_DIR,
                stage_ro_dir = paths.join(PREFIX, "stage_ro"),
                fswork_dir = paths.join(PREFIX, "fswork"),
                out_dir = OUT_DIR,
            ),
        ],
    )

    hoist(
        name = project,
        layer = "build-layer",
        path = OUT_DIR.lstrip("/"),
        selector = [
            "-mindepth 1",
            "-maxdepth 1",
        ],
        force_dir = True,
        visibility = [
            "//antlir/...",
            "//metalos/...",
        ],
    )

_script_t = shape.shape(
    prepare = str,
    build = str,
    install = str,
)

def _new_script(build, install, prepare = ""):
    return shape.new(
        _script_t,
        prepare = prepare,
        build = build,
        install = install,
    )

_dep_t = shape.shape(
    name = str,
    source = shape.target(),
    paths = shape.list(str),
)

def _library(name, *, include_path = "include", lib_path = "lib"):
    return shape.new(
        _dep_t,
        name = name,
        source = third_party_shim.library(name, name, "antlir"),
        paths = [include_path, lib_path],
    )

def _oss_build(*, project = None, name = None):
    if not project:
        project = paths.basename(package_name())

    if not name:
        name = project

    buck_genrule(
        name = project,
        out = "out",
        bash = """
            cp --reflink=auto -r $(location //antlir/third-party/{project}:{name}) "$OUT"
        """.format(
            project = project,
            name = name,
        ),
    )

third_party = struct(
    native_build = _native_build,
    script = _new_script,
    library = _library,
    oss_build = _oss_build,
)
