//! `vik run [-d] [WORKFLOW]`.

use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;

use crate::daemon;
use crate::workflow::Workflow;

#[derive(Debug, Parser)]
pub struct RunArgs {
  /// Detach into the background as a daemon.
  #[arg(short = 'd', long = "detached")]
  pub detached: bool,
}

pub fn execute(workflow: Workflow, args: RunArgs) -> ExitCode {
  match daemon::run(workflow, args.detached) {
    Ok(()) => ExitCode::SUCCESS,
    Err(err) => {
      let _ = writeln!(io::stderr(), "vik run failed: {err:#}");
      ExitCode::from(1)
    },
  }
}
