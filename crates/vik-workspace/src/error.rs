use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace_io: {0}")]
    Io(String),
    #[error("workspace_path_outside_root")]
    PathOutsideRoot,
    #[error("workspace_location_not_directory")]
    LocationNotDirectory,
    #[error("repo_clone_failed: status={status} stderr={stderr}")]
    RepoCloneFailed { status: i32, stderr: String },
    #[error("repo_clone_timeout")]
    RepoCloneTimeout,
    #[error("hook_failed: {hook} status={status}")]
    HookFailed { hook: &'static str, status: i32 },
    #[error("hook_timeout: {hook}")]
    HookTimeout { hook: &'static str },
}
