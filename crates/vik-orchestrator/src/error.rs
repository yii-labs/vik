use thiserror::Error;
use vik_workflow::WorkflowError;

#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error("workflow: {0}")]
    Workflow(#[from] WorkflowError),
}
