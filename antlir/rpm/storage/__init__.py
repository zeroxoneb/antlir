#!/usr/bin/env python3
# Copyright (c) Facebook, Inc. and its affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

"""
IMPORTANT: The order of imports in this file is critical, and they must not be
re-ordered by a formatter.

isort:skip_file
"""
# fmt: off

from .storage import Storage, StorageInput, StorageOutput  # usort:skip
from .cli_object_storage import CLIObjectStorage  # usort:skip

__all__ = [Storage, StorageInput, StorageOutput, CLIObjectStorage]

# Register implementations with Storage
from . import filesystem_storage, s3_storage  # usort:skip # noqa: F401
try:
    # Import FB-specific implementations if available
    from . import facebook  # noqa: F401
except ImportError:  # pragma: no cover
    pass
