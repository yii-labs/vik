use std::path::{Path, PathBuf};

use crate::error::WorkspaceError;

pub fn ensure_inside_root(root: &Path, workspace_path: &Path) -> Result<(), WorkspaceError> {
    let root = absolute_existing_or_join(root)?;
    let candidate = if workspace_path.is_absolute() {
        workspace_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| WorkspaceError::Io(err.to_string()))?
            .join(workspace_path)
    };
    if candidate.starts_with(&root) {
        Ok(())
    } else {
        Err(WorkspaceError::PathOutsideRoot)
    }
}

pub(crate) fn absolute_existing_or_join(path: &Path) -> Result<PathBuf, WorkspaceError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|err| WorkspaceError::Io(err.to_string()))
    }
}
