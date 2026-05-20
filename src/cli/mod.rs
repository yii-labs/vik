//! Command-line dispatch.
//!
//! `main.rs` calls [`run`] and nothing else. The workflow file path is
//! a global arg parsed once, loaded once, and threaded to whichever
//! subcommand wins — that way subcommand modules never re-parse.

pub mod doctor;
pub mod init;
pub mod lifecycle;
pub mod run;
pub mod shutdown;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::workflow::Workflow;
use crate::workflow::loader::WorkflowSchemaLoader;

#[derive(Debug, Parser)]
#[command(
  name = "vik",
  version,
  about = "Vik runs workflow-driven agents for issue tracker work.",
  long_about = None,
  override_usage = "vik <COMMAND> [WORKFLOW]",
)]
struct Cli {
  /// Path to the workflow file all subcommands act on.
  #[arg(value_name = "WORKFLOW", default_value = "./workflow.yml", global = true)]
  workflow: PathBuf,

  #[command(subcommand)]
  command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
  /// Generate a starter workflow setup.
  Init(init::InitArgs),
  /// Validate a workflow file without running any agents or mutating
  /// anything outside the process.
  Doctor(doctor::DoctorArgs),
  /// Run the workflow working loop.
  Run(run::RunArgs),
  /// Report the daemon status for a workflow.
  Status(lifecycle::LifecycleArgs),
  /// Ask the running daemon to shut down.
  Stop(lifecycle::LifecycleArgs),
  /// Stop the running daemon (if any) and start a fresh one.
  Restart(lifecycle::RestartArgs),
  /// Stop the running daemon (if any) and remove its state file.
  Uninstall(lifecycle::LifecycleArgs),
}

pub fn run() -> ExitCode {
  let cli = match Cli::try_parse() {
    Ok(cli) => cli,
    Err(err) => {
      // `clap::Error::print` already routes help/version to stdout and
      // real errors to stderr; we just mirror its decision in the exit
      // code. CLI parse errors return 2 (distinct from runtime fail=1).
      let _ = err.print();
      if err.use_stderr() {
        return ExitCode::from(2);
      }
      return ExitCode::SUCCESS;
    },
  };

  let Cli { workflow, command } = cli;

  let command = match command {
    Command::Init(args) => return init::execute(workflow, args),
    command => command,
  };

  let loaded = match WorkflowSchemaLoader.load(&workflow) {
    Ok(loaded) => loaded,
    Err(err) => {
      eprintln!("{err}");
      return ExitCode::from(1);
    },
  };

  match command {
    // Doctor takes the raw schema — it must never instantiate the
    // supervisor because part of its job is to report errors that
    // would prevent supervisor construction.
    Command::Doctor(args) => doctor::execute(loaded, args),
    command => {
      let workflow = match Workflow::try_from(loaded) {
        Ok(workflow) => workflow,
        Err(err) => {
          eprintln!("{err}");
          return ExitCode::from(1);
        },
      };

      match command {
        Command::Run(args) => run::execute(workflow, args),
        Command::Status(args) => lifecycle::status(workflow, args),
        Command::Stop(args) => lifecycle::stop(workflow, args),
        Command::Restart(args) => lifecycle::restart(workflow, args),
        Command::Uninstall(args) => lifecycle::uninstall(workflow, args),
        Command::Init(_) => unreachable!("init command already handled"),
        Command::Doctor(_) => unreachable!("doctor command already handled"),
      }
    },
  }
}
