/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

mod disk;
mod isolation;
mod net;
mod runtime;
mod share;
mod ssh;
mod types;
mod utils;
mod vm;

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::anyhow;
use anyhow::Context;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use image_test_lib::KvPair;
use image_test_lib::Test;
use json_arg::JsonFile;
use tracing::debug;

use crate::isolation::default_passthrough_envs;
use crate::isolation::is_isolated;
use crate::isolation::isolated;
use crate::isolation::Platform;
use crate::runtime::set_runtime;
use crate::types::MachineOpts;
use crate::types::RuntimeOpts;
use crate::types::VMArgs;
use crate::utils::log_command;
use crate::vm::VM;

type Result<T> = std::result::Result<T, anyhow::Error>;

#[derive(Debug, Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the VM. Must be executed inside container.
    Run(RunCmdArgs),
    /// Respawn inside isolated image and execute `Run` command.
    Isolate(IsolateCmdArgs),
    /// Run VM tests inside container.
    Test(IsolateCmdArgs),
}

/// Execute the VM
#[derive(Debug, Args)]
struct RunCmdArgs {
    /// Json-encoded file for VM machine configuration
    #[arg(long)]
    machine_spec: JsonFile<MachineOpts>,
    /// Json-encoded file describing paths of binaries required by VM
    #[arg(long)]
    runtime_spec: JsonFile<RuntimeOpts>,
    #[clap(flatten)]
    vm_args: VMArgs,
}

/// Spawn a container and execute the VM inside.
#[derive(Debug, Args)]
struct IsolateCmdArgs {
    /// Path to container image.
    #[arg(long)]
    image: PathBuf,
    /// Set these env vars in the container. If VM executes a command, these
    /// env vars will also be prepended to the command.
    #[arg(long)]
    setenv: Vec<KvPair>,
    /// Args for run command
    #[clap(flatten)]
    run_cmd_args: RunCmdArgs,
}

/// Actually starting the VM. This needs to be inside an ephemeral container as
/// lots of resources relies on container for clean up.
fn run(args: &RunCmdArgs) -> Result<()> {
    if !is_isolated()? {
        return Err(anyhow!("run must be called from inside container"));
    }
    debug!("RuntimeOpts: {:?}", args.runtime_spec);
    debug!("MachineOpts: {:?}", args.machine_spec);

    set_runtime(args.runtime_spec.clone().into_inner())
        .map_err(|_| anyhow!("Failed to set runtime"))?;
    Ok(VM::new(args.machine_spec.clone().into_inner(), args.vm_args.clone())?.run()?)
}

/// Enter isolated container and then respawn itself inside it with `run`
/// command and its parameters.
fn respawn(args: &IsolateCmdArgs) -> Result<()> {
    let mut envs = default_passthrough_envs();
    envs.extend(args.setenv.clone());
    let mut vm_args = args.run_cmd_args.vm_args.clone();
    vm_args.command_envs = envs.clone();

    let isolated = isolated(&args.image, envs, &vm_args.output_dirs)?;
    let exe = env::current_exe().context("while getting argv[0]")?;
    let mut command = isolated.into_command();
    command
        .arg(&exe)
        .arg("run")
        .arg("--machine-spec")
        .arg(args.run_cmd_args.machine_spec.path())
        .arg("--runtime-spec")
        .arg(args.run_cmd_args.runtime_spec.path());
    command.args(vm_args.to_args());

    log_command(&mut command).status()?;
    Ok(())
}

/// Merge all sources of our envs into final list of env vars we should use
/// everywhere for tests. Dedup is handled by functions that use the result.
fn get_test_envs(from_cli: &[KvPair]) -> Vec<KvPair> {
    // This handles common envs like RUST_LOG
    let mut envs = default_passthrough_envs();
    envs.extend_from_slice(from_cli);
    // forward test runner env vars to the inner test
    for (key, val) in std::env::vars() {
        if key.starts_with("TEST_PILOT") {
            envs.push((key, OsString::from(val)).into());
        }
    }
    envs
}

/// Further validate `VMArgs` parsed by clap and generate a new `VMArgs` with
/// content specific to test execution.
fn get_test_vm_args(orig_args: &VMArgs, cli_envs: &[KvPair]) -> Result<(VMArgs, bool)> {
    if orig_args.timeout_s.is_none() {
        return Err(anyhow!("Test command must specify --timeout-s."));
    }
    if !orig_args.output_dirs.is_empty() {
        return Err(anyhow!(
            "Test command must not specify --output-dirs. \
            This will be parsed from env and test command parameters instead."
        ));
    }
    let envs = get_test_envs(cli_envs);

    #[derive(Debug, Parser)]
    struct TestArgsParser {
        #[clap(subcommand)]
        test: Test,
    }
    let mut orig_command = vec![OsString::from("bogus_exec")];
    orig_command.extend_from_slice(
        &orig_args
            .command
            .clone()
            .ok_or(anyhow!("Test command must not be empty"))?,
    );
    let test_args = TestArgsParser::try_parse_from(orig_command)
        .context("Test command does not match expected format of `<type> <command>`")?;
    let is_list = test_args.test.is_list_tests();
    let mut vm_args = orig_args.clone();
    vm_args.output_dirs = test_args.test.output_dirs().into_iter().collect();
    vm_args.command = Some(test_args.test.into_inner_cmd());
    vm_args.command_envs = envs;
    Ok((vm_args, is_list))
}

/// This function is similar to `respawn`, except that we assume control for
/// some inputs instead of allowing caller to pass them in. Some inputs are
/// parsed from test command.
fn test(args: &IsolateCmdArgs) -> Result<()> {
    let (vm_args, is_list) = get_test_vm_args(&args.run_cmd_args.vm_args, &args.setenv)?;
    let isolated = isolated(
        &args.image,
        vm_args.command_envs.clone(),
        &vm_args.output_dirs,
    )?;

    let mut command = isolated.into_command();
    if is_list {
        // If this is a list test command, we run it directly inside container
        command.args(vm_args.command.as_ref().expect("command must exist here"));
    } else {
        // Otherwise, it's a real test we run inside a VM
        let exe = env::current_exe().context("while getting argv[0]")?;
        command
            .arg(&exe)
            .arg("run")
            .arg("--machine-spec")
            .arg(args.run_cmd_args.machine_spec.path())
            .arg("--runtime-spec")
            .arg(args.run_cmd_args.runtime_spec.path());
        command.args(vm_args.to_args());
    }

    log_command(&mut command).status()?;
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    Platform::set()?;

    debug!("Args: {:?}", env::args());

    let cli = Cli::parse();
    match &cli.command {
        Commands::Isolate(args) => respawn(args),
        Commands::Run(args) => run(args),
        Commands::Test(args) => test(args),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_get_test_envs() {
        env::set_var("RUST_LOG", "hello");
        env::set_var("TEST_PILOT_A", "A");
        let from_cli = vec![KvPair::from(("foo", "bar"))];
        assert_eq!(
            get_test_envs(&from_cli),
            vec![
                KvPair::from(("RUST_LOG", "hello")),
                KvPair::from(("foo", "bar")),
                KvPair::from(("TEST_PILOT_A", "A")),
            ],
        )
    }

    #[test]
    fn test_get_test_vm_args() {
        let valid = VMArgs {
            timeout_s: Some(1),
            console: false,
            output_dirs: vec![],
            command_envs: vec![],
            command: Some(["custom", "whatever"].iter().map(OsString::from).collect()),
        };
        let empty_env = Vec::<KvPair>::new();
        let mut expected = valid.clone();
        expected.command = Some(vec![OsString::from("whatever")]);
        let (parsed, is_list) =
            get_test_vm_args(&valid, &empty_env).expect("Parsing should succeed");
        assert_eq!(parsed, expected);
        assert!(!is_list);

        let mut timeout = valid.clone();
        timeout.timeout_s = None;
        assert!(get_test_vm_args(&timeout, &empty_env).is_err());

        let mut output_dirs = valid.clone();
        output_dirs.output_dirs = vec![PathBuf::from("/some")];
        assert!(get_test_vm_args(&output_dirs, &empty_env).is_err());

        let mut command = valid;
        command.command = None;
        assert!(get_test_vm_args(&command, &empty_env).is_err());
        command.command = Some(vec![OsString::from("invalid")]);
        assert!(get_test_vm_args(&command, &empty_env).is_err());
    }
}
