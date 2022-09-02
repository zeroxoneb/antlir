/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use std::path::PathBuf;

use structopt::StructOpt;

#[derive(Debug, Clone, StructOpt)]
#[structopt(about = "Command to upgrade a btrfs send stream")]
pub struct SendStreamUpgradeOptions {
    /// Avoid crcing input
    /// This will implicitly trust the given commands and end up skipping the
    /// CRC32C checks on for commands that were populated from storage
    /// false is the default value (default_value isn't set because of structopt
    /// weirdness)
    #[structopt(short, long, parse(from_flag))]
    pub avoid_crcing_input: bool,

    /// Command Bytes to Dump to Event Log
    /// This represents the maximum number of command bytes dumped as a part of
    /// serde checks
    /// 0 is the default value
    #[structopt(short, long, default_value = "0")]
    pub bytes_to_log: usize,

    /// Compression level
    /// This represents the zstd compression level to apply as a part of the
    /// ugprade process
    /// 0 will disable compression
    /// 3 is the default compression value
    /// 22 is the maximum value that can be used
    #[structopt(short, long, default_value = "3")]
    pub compression_level: i32,

    /// Path to input file representing a send stream to upgrade
    /// Optional argument; stdin is used if this is not specified
    #[structopt(short, long)]
    pub input: Option<PathBuf>,

    /// Maximum Batched Extent Size
    /// This represents the maximum number of contiguous blocks (measured in
    /// bytes)
    /// to stich together across multiple commands to form a single extent
    /// Note that this value should be a multiple of 4096 (the underlying block
    /// size)
    /// 0 will disable batching
    /// 131072 is the default
    /// 131072 is the maximum value that can be used
    #[structopt(short, long, default_value = "131072")]
    pub maximum_batched_extent_size: usize,

    /// Path to output file representing an upgraded send stream
    /// Optional argument; stdout is used if this is not specified
    #[structopt(short, long)]
    pub output: Option<PathBuf>,

    /// Pad data payload offset with dummy commands
    /// This will instruct the send stream upgrade tool to add a dummy command
    /// to align the data payload to a 4KiB boundary for all writes
    /// false is the default value (default_value isn't set because of structopt
    /// weirdness)
    #[structopt(short, long, parse(from_flag))]
    pub pad_with_dummy_commands: bool,

    /// Quiet
    /// This supresses all output including logging and summary statistics
    /// false is the default value (default_value isn't set because of structopt
    /// weirdness)
    #[structopt(short, long, parse(from_flag))]
    pub quiet: bool,

    /// Read buffer size
    /// This controls the maximum size of the read buffer
    /// The default value is 8KiB
    #[structopt(short, long, default_value = "8192")]
    pub read_buffer_size: usize,

    /// Serialize-Deserialize Checks
    /// This will serialize and deserialize a command at every step of its
    /// lifecycle to verify its contents
    /// false is the default value (default_value isn't set because of structopt
    /// weirdness)
    #[structopt(short, long, parse(from_flag))]
    pub serde_checks: bool,

    /// Verbosity
    /// This represents the log level for the event log that is directed to
    /// stderr
    #[structopt(short, long, parse(from_occurrences))]
    pub verbose: usize,

    /// Write buffer size
    /// This controls the maximum size of the write buffer
    /// The default value is 8KiB
    #[structopt(short, long, default_value = "8192")]
    pub write_buffer_size: usize,
}