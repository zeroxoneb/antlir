#!/usr/bin/env python3
# Copyright (c) Facebook, Inc. and its affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

import asyncio
import io
import os.path
import sys
import time
from typing import List, Optional

from antlir.artifacts_dir import find_buck_cell_root
from antlir.common import get_logger
from antlir.fs_utils import Path
from antlir.vm.share import BtrfsDisk, Plan9Export
from antlir.vm.vm import vm, VMExecOpts
from antlir.vm.vm_opts_t import vm_opts_t


logger = get_logger()


MINUTE = 60


def blocking_print(*args, file: io.IOBase = sys.stdout, **kwargs):
    blocking = os.get_blocking(file.fileno())
    os.set_blocking(file.fileno(), True)
    print(*args, file=file, **kwargs)
    # reset to the old blocking mode
    os.set_blocking(file.fileno(), blocking)


class VMTestExecOpts(VMExecOpts):
    """
    Custom execution options for this VM entry point.
    """

    devel_layer: bool = False
    interactive: bool = False
    timeout: int = 0
    setenv: List[str] = []
    sync_file: List[Path] = []
    test_binary: Path
    test_binary_image: Path
    gtest_list_tests: bool
    list_tests: Optional[str]

    @classmethod
    def setup_cli(cls, parser):
        super(VMTestExecOpts, cls).setup_cli(parser)

        parser.add_argument(
            "--devel-layer",
            action="store_true",
            default=False,
            help="Provide the kernel devel layer as a mount to the booted VM",
        )
        parser.add_argument(
            "--interactive",
            action="store_true",
            default=False,
            help="Boot into the VM for manual interaction. This will setup the "
            "test but will not execute it.",
        )
        parser.add_argument(
            "--timeout",
            type=int,
            # TestPilot sets this environment variable
            default=os.environ.get("TIMEOUT", 5 * MINUTE),
            help="how many seconds to wait for the test to finish",
        )
        parser.add_argument(
            "--setenv",
            action="append",
            default=[],
            help="Specify an environment variable to pass to the test "
            "in the form NAME=VALUE",
        )

        parser.add_argument(
            "--sync-file",
            type=Path,
            action="append",
            default=[],
            help="Sync this file for tpx from the vm to the host.",
        )
        parser.add_argument(
            "--test-binary",
            type=Path,
            help="Path to the actual test binary that will be invoked.  This "
            "is used to discover tests before they are executed inside the VM",
            required=True,
        )
        parser.add_argument(
            "--test-binary-image",
            type=Path,
            help="Path to a btrfs loopback image that contains the test binary "
            "to run",
            required=True,
        )
        parser.add_argument(
            "--gtest_list_tests",
            action="store_true",
        )  # For c++ gtest
        parser.add_argument(
            "--list-tests",
        )  # Python pyunit with the new TestPilot adapter


async def run(
    # common args from VMExecOpts
    bind_repo_ro: bool,
    debug: bool,
    extra: List[str],
    opts: vm_opts_t,
    # antlir.vm.vmtest specific args
    devel_layer: bool,
    gtest_list_tests: bool,
    interactive: bool,
    list_tests: Optional[str],
    setenv: List[str],
    sync_file: List[str],
    test_binary: Path,
    test_binary_image: Path,
    timeout: int,
) -> None:

    # Start the test binary directly to list out test cases instead of
    # starting an entire VM.  This is faster, but it's also a potential
    # security hazard since the test code may expect that it always runs
    # sandboxed, and may run untrusted code as part of listing tests.
    # TODO(vmagro): the long-term goal should be to make vm boots as
    # fast as possible to avoid unintuitive tricks like this
    if gtest_list_tests or list_tests:
        assert not (
            gtest_list_tests and list_tests
        ), "Cannot provide both --gtest_list_tests and --list-tests"
        proc = await asyncio.create_subprocess_exec(
            str(test_binary),
            *(
                ["--gtest_list_tests"]
                if gtest_list_tests
                # NB: Unlike for the VM, we don't explicitly have to
                # pass the magic `TEST_PILOT` environment var to allow
                # triggering the new TestPilotAdapter. The environment
                # is inherited.
                else ["--list-tests", list_tests]
            ),
        )
        await proc.wait()
        sys.exit(proc.returncode)

    # If we've made it this far we are executing the actual test, not just
    # listing tests
    returncode = -1
    start_time = time.time()
    test_env = dict(s.split("=", maxsplit=1) for s in setenv)

    # Build shares to provide to the vm
    shares = [BtrfsDisk(test_binary_image, "/vmtest")]
    if devel_layer and opts.kernel.artifacts.devel is None:
        raise Exception(
            "--devel-layer requires kernel.artifacts.devel set in vm_opts"
        )
    if devel_layer:
        shares += [
            Plan9Export(
                path=opts.kernel.artifacts.devel.subvol.path(),
                mountpoint="/usr/src/kernels/{}".format(opts.kernel.uname),
                mount_tag="kernel-devel-src",
                generator=True,
            ),
            Plan9Export(
                path=opts.kernel.artifacts.devel.subvol.path(),
                mountpoint="/usr/lib/modules/{}/build".format(
                    opts.kernel.uname
                ),
                mount_tag="kernel-devel-build",
                generator=True,
            ),
        ]

    async with vm(
        bind_repo_ro=bind_repo_ro,
        opts=opts,
        verbose=debug,
        interactive=interactive,
        shares=shares,
    ) as instance:

        boot_time_elapsed = time.time() - start_time
        logger.debug(f"VM took {boot_time_elapsed} seconds to boot")
        if not interactive:

            # Sync the file which tpx needs from the vm to the host.
            file_arguments = list(sync_file)
            for arg in extra:
                # for any args that look like files make sure that the
                # directory exists so that the test binary can write to
                # files that it expects to exist (that would normally be
                # created by TestPilot)
                dirname = os.path.dirname(arg)
                # TestPilot will already create the directories on the
                # host, so as another sanity check only create the
                # directories in the VM that already exist on the host
                if dirname and os.path.exists(dirname):
                    await instance.run(("mkdir", "-p", dirname))
                    file_arguments.append(arg)

            # The behavior of the FB-internal Python test main changes
            # completely depending on whether this environment var is set.
            # We must forward it so that the new TP adapter can work.
            test_pilot_env = os.environ.get("TEST_PILOT")
            if test_pilot_env:
                test_env["TEST_PILOT"] = test_pilot_env

            cmd = ["/vmtest/test"] + list(extra)
            logger.debug(f"executing {cmd} inside guest")
            returncode, stdout, stderr = await instance.run(
                cmd=cmd,
                # a certain amount of the total timeout is allocated for
                # the host to boot, subtract the amount of time it actually
                # took, so that vmtest times out internally before choking
                # to TestPilot, which gives the same end result but should
                # allow for some slightly better logging opportunities
                # Give at least 10s (sometimes this can even be negative)
                timeout=max(timeout - boot_time_elapsed - 1, 10),
                env=test_env,
                # TODO(lsalis):  This is currently needed due to how some
                # cpp_unittest targets depend on artifacts in the code
                # repo.  Once we have proper support for `runtime_files`
                # this can be removed.  See here for more details:
                # https://fburl.com/xt322rks
                cwd=find_buck_cell_root(path_in_repo=os.getcwd()),
            )
            if returncode != 0:
                logger.error(f"{cmd} failed with returncode {returncode}")
            else:
                logger.debug(f"{cmd} succeeded")

            # Some tests have incredibly large amounts of output, which
            # results in a BlockingIOError when stdout/err are in
            # non-blocking mode. Just force it to print the output in
            # blocking mode to avoid that - we don't really care how long
            # it ends up blocked as long as it eventually gets written.
            if stdout:
                blocking_print(stdout.decode("utf-8"), end="")
            else:
                logger.warning("Test stdout was empty")
            if stderr:
                logger.debug("Test stderr:")
                blocking_print(stderr.decode("utf-8"), file=sys.stderr, end="")
            else:
                logger.warning("Test stderr was empty")

            for path in file_arguments:
                logger.debug(f"copying {path} back to the host")
                # copy any files that were written in the guest back to the
                # host so that TestPilot can read from where it expects
                # outputs to end up
                try:
                    outfile_contents = await instance.cat_file(str(path))
                    with open(path, "wb") as out:
                        out.write(outfile_contents)
                except Exception as e:
                    logger.error(f"Failed to copy {path} to host: {str(e)}")

    sys.exit(returncode)


if __name__ == "__main__":
    asyncio.run(run(**dict(VMTestExecOpts.parse_cli(sys.argv[1:]))))
