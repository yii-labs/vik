use std::error::Error;
use std::path::PathBuf;

use clap::Args;
use vik_workflow::load_effective_workflow;

#[derive(Debug, Args)]
pub(crate) struct CheckArgs {
    /// Path to WORKFLOW.md. Defaults to ./WORKFLOW.md.
    pub(crate) workflow: Option<PathBuf>,
}

pub(crate) fn run(workflow: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let loaded = load_effective_workflow(workflow)?;
    loaded.config.validate_for_dispatch()?;
    println!("workflow valid: {}", loaded.definition.path.display());
    Ok(())
}
