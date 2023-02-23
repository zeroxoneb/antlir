#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

import argparse
import json

parser = argparse.ArgumentParser()
parser.add_argument("action", choices=["install", "remove"])
parser.add_argument("package_specs", nargs="+")

args, unknown = parser.parse_known_args()

with open("/antlir2_dnf_log.jsonl", "a") as f:
    json.dump({"action": args.action, "package_specs": args.package_specs}, f)
    f.write("\n")
