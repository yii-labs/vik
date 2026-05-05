use std::error::Error;
use std::path::PathBuf;

use vik_workflow::load_effective_workflow;

use crate::env;

pub(crate) fn run(workflow: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    env::load_dotenv()?;
    let loaded = load_effective_workflow(workflow)?;
    for warning in loaded.config.validate_for_check()? {
        eprintln!("warning: {warning}");
    }
    println!("workflow valid: {}", loaded.definition.path.display());
    Ok(())
}
