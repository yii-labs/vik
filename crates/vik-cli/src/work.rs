use std::error::Error;
use std::path::PathBuf;

use clap::Args;

#[derive(Debug, Args)]
pub(crate) struct WorkArgs {
    /// Path to WORKFLOW.md. Defaults to ./WORKFLOW.md.
    #[arg(short = 'w', long = "workflow", value_name = "WORKFLOW")]
    pub(crate) workflow: Option<PathBuf>,
}

pub(crate) fn run(args: WorkArgs) -> Result<(), Box<dyn Error>> {
    crate::service::register_workflow_and_start_service(args.workflow)
}
