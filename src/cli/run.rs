//! `vik run [-d] [WORKFLOW]`.

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::anyhow;
use clap::Parser;
use tokio_util::sync::CancellationToken;

use crate::daemon;
use crate::orchestrator::Orchestrator;
use crate::workflow::Workflow;

#[derive(Debug, Parser)]
pub struct RunArgs {
  /// Detach into the background as a daemon.
  #[arg(short = 'd', long = "detached")]
  pub detached: bool,
}

pub fn execute(workflow: Workflow, args: RunArgs) -> ExitCode {
  match daemon::run(workflow, args.detached, start_orchestrator) {
    Ok(()) => ExitCode::SUCCESS,
    Err(err) => {
      let _ = writeln!(io::stderr(), "vik run failed: {err:#}");
      ExitCode::from(1)
    },
  }
}

async fn start_orchestrator(workflow: Workflow, shutdown: CancellationToken) -> anyhow::Result<()> {
  let mut orchestrator = Orchestrator::new(workflow);
  orchestrator
    .run(shutdown)
    .await
    .map_err(|err| anyhow!("orchestrator loop: {err:#}"))
}
