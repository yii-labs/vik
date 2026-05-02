use thiserror::Error;
use vik_core::TrackerError;
use vik_workflow::WorkflowError;
use vik_workspace::WorkspaceError;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("invalid_workspace_cwd")]
    InvalidWorkspaceCwd,
    #[error("codex_not_found: {0}")]
    CodexNotFound(String),
    #[error("invalid_codex_command: {0}")]
    InvalidCodexCommand(String),
    #[error("process_spawn_failed: {program}: {source}")]
    ProcessSpawn {
        program: String,
        source: std::io::Error,
    },
    #[error("response_timeout")]
    ResponseTimeout,
    #[error("turn_timeout")]
    TurnTimeout,
    #[error("port_exit")]
    PortExit,
    #[error("response_error: {0}")]
    ResponseError(String),
    #[error("turn_failed: {0}")]
    TurnFailed(String),
    #[error("turn_cancelled")]
    TurnCancelled,
    #[error("turn_input_required")]
    TurnInputRequired,
    #[error("workspace: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("workflow: {0}")]
    Workflow(#[from] WorkflowError),
    #[error("tracker: {0}")]
    Tracker(#[from] TrackerError),
}
