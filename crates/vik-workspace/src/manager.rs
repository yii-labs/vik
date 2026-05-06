use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::fs;
use tokio::process::Command;
use tokio::time;
use vik_core::{PosixShell, ShellInvocation, Workspace, sanitize_workspace_key};
use vik_workflow::HooksConfig;

use crate::error::WorkspaceError;
use crate::path::{absolute_existing_or_join, ensure_inside_root};

const RESERVED_WORKSPACE_KEYS: &[&str] = &[".vik", "logs", "service", "sessions"];

#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    root: PathBuf,
    hooks: HooksConfig,
}

impl WorkspaceManager {
    pub fn new(root: impl Into<PathBuf>, hooks: HooksConfig) -> Self {
        Self {
            root: root.into(),
            hooks,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn create_for_issue(&self, identifier: &str) -> Result<Workspace, WorkspaceError> {
        let root = absolute_existing_or_join(&self.root)?;
        fs::create_dir_all(&root)
            .await
            .map_err(|err| WorkspaceError::Io(err.to_string()))?;
        let workspace_key = sanitize_workspace_key(identifier);
        ensure_not_reserved_workspace_key(&workspace_key)?;
        let path = root.join(&workspace_key);
        ensure_inside_root(&root, &path)?;
        let created_now = match fs::metadata(&path).await {
            Ok(metadata) if metadata.is_dir() => false,
            Ok(_) => return Err(WorkspaceError::LocationNotDirectory),
            Err(_) => {
                fs::create_dir_all(&path)
                    .await
                    .map_err(|err| WorkspaceError::Io(err.to_string()))?;
                true
            }
        };
        let workspace = Workspace {
            path: path.clone(),
            workspace_key,
            created_now,
        };
        if created_now {
            self.run_required_hook("after_create", self.hooks.after_create.as_deref(), &path)
                .await?;
        }
        Ok(workspace)
    }

    pub async fn before_run(&self, path: &Path) -> Result<(), WorkspaceError> {
        self.run_required_hook("before_run", self.hooks.before_run.as_deref(), path)
            .await
    }

    pub async fn after_run_best_effort(&self, path: &Path) {
        if let Err(err) = self
            .run_optional_hook("after_run", self.hooks.after_run.as_deref(), path)
            .await
        {
            tracing::warn!(error=%err, "hook name=after_run outcome=failed ignored=true");
        }
    }

    pub async fn remove_for_issue(&self, identifier: &str) -> Result<(), WorkspaceError> {
        let root = absolute_existing_or_join(&self.root)?;
        let workspace_key = sanitize_workspace_key(identifier);
        ensure_not_reserved_workspace_key(&workspace_key)?;
        let path = root.join(workspace_key);
        ensure_inside_root(&root, &path)?;
        if fs::metadata(&path).await.is_err() {
            return Ok(());
        }
        if let Err(err) = self
            .run_optional_hook("before_remove", self.hooks.before_remove.as_deref(), &path)
            .await
        {
            tracing::warn!(error=%err, "hook name=before_remove outcome=failed ignored=true");
        }
        fs::remove_dir_all(&path)
            .await
            .map_err(|err| WorkspaceError::Io(err.to_string()))?;
        Ok(())
    }

    pub fn validate_agent_cwd(
        &self,
        workspace_path: &Path,
        cwd: &Path,
    ) -> Result<(), WorkspaceError> {
        let root = absolute_existing_or_join(&self.root)?;
        ensure_inside_root(&root, workspace_path)?;
        if workspace_path != cwd {
            return Err(WorkspaceError::PathOutsideRoot);
        }
        Ok(())
    }

    async fn run_required_hook(
        &self,
        name: &'static str,
        script: Option<&str>,
        cwd: &Path,
    ) -> Result<(), WorkspaceError> {
        self.run_hook(name, script, cwd, true).await
    }

    async fn run_optional_hook(
        &self,
        name: &'static str,
        script: Option<&str>,
        cwd: &Path,
    ) -> Result<(), WorkspaceError> {
        self.run_hook(name, script, cwd, false).await
    }

    async fn run_hook(
        &self,
        name: &'static str,
        script: Option<&str>,
        cwd: &Path,
        required: bool,
    ) -> Result<(), WorkspaceError> {
        let Some(script) = script else {
            return Ok(());
        };
        tracing::info!(hook=name, cwd=%cwd.display(), "hook outcome=started");
        let shell = ShellInvocation::for_current_platform(script, PosixShell::Sh);
        let mut child = Command::new(shell.program());
        child
            .args(shell.args())
            .arg(shell.command())
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let fut = child.output();
        let output = time::timeout(Duration::from_millis(self.hooks.timeout_ms), fut)
            .await
            .map_err(|_| WorkspaceError::HookTimeout { hook: name })?
            .map_err(|err| WorkspaceError::Io(err.to_string()))?;
        if output.status.success() {
            tracing::info!(hook = name, "hook outcome=completed");
            return Ok(());
        }
        let status = output.status.code().unwrap_or(-1);
        let err = WorkspaceError::HookFailed { hook: name, status };
        if required {
            return Err(err);
        }
        tracing::warn!(hook = name, status, "hook outcome=failed ignored=true");
        Ok(())
    }
}

fn ensure_not_reserved_workspace_key(workspace_key: &str) -> Result<(), WorkspaceError> {
    if RESERVED_WORKSPACE_KEYS
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(workspace_key))
    {
        return Err(WorkspaceError::ReservedWorkspaceKey {
            key: workspace_key.to_string(),
        });
    }
    Ok(())
}
