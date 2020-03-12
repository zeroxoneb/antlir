#!/usr/bin/env python3
# Copyright (c) Facebook, Inc. and its affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

import tempfile
import unittest

from fs_image.nspawn_in_subvol.run_test import (
    do_not_rewrite_cmd, forward_test_runner_env_vars,
    rewrite_testpilot_python_cmd,
)


class NspawnTestInSubvolTestCase(unittest.TestCase):

    def test_forward_env_vars(self):
        self.assertEqual([], list(forward_test_runner_env_vars({'a': 'b'})))
        self.assertEqual(
            ['--setenv=TEST_PILOT=xyz'],
            list(forward_test_runner_env_vars({'a': 'b', 'TEST_PILOT': 'xyz'})),
        )

    def test_do_not_rewrite_cmd(self):
        with do_not_rewrite_cmd(['a', 'b'], 3) as cmd_and_fds:
            self.assertEqual((['a', 'b'], []), cmd_and_fds)

    def test_rewrite_testpilot_python_cmd(self):
        bin = '/layer-test-binary'

        # Test no-op rewriting
        cmd = [bin, 'foo', '--bar', 'beep', '--baz', '-xack', '7', '9']
        with rewrite_testpilot_python_cmd(cmd, next_fd=1337) as cmd_and_fd:
            self.assertEqual((cmd, []), cmd_and_fd)

        for rewritten_opt in ('--output', '--list-tests'):
            with tempfile.NamedTemporaryFile(suffix='.json') as t:
                prefix = ['--zap=3', '--ou', 'boo', '--ou=3']
                suffix = ['garr', '-abc', '-gh', '-d', '--e"f']
                with rewrite_testpilot_python_cmd(
                    [bin, *prefix, f'{rewritten_opt}={t.name}', *suffix],
                    next_fd=37,
                ) as (new_cmd, fds_to_forward):
                    fd_to_forward, = fds_to_forward
                    self.assertIsInstance(fd_to_forward, int)
                    # The last argument deliberately requires shell quoting.
                    self.assertEqual([
                        '/bin/bash', '-c', ' '.join([
                            'exec',
                            bin, rewritten_opt, '>(cat >&37)', *prefix,
                            *suffix[:-1],
                            """'--e"f'""",
                        ])
                    ], new_cmd)
