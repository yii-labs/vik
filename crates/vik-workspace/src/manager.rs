use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time;
use vik_core::{PosixShell, ShellInvocation, Workspace, sanitize_workspace_key};
use vik_workflow::{HooksConfig, RepoConfig};

use crate::error::WorkspaceError;
use crate::path::{absolute_existing_or_join, ensure_inside_root};

#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    root: PathBuf,
    hooks: HooksConfig,
    repo: Option<RepoConfig>,
}

impl WorkspaceManager {
    pub fn new(root: impl Into<PathBuf>, hooks: HooksConfig) -> Self {
        Self::with_repo(root, hooks, None)
    }

    pub fn with_repo(
        root: impl Into<PathBuf>,
        hooks: HooksConfig,
        repo: Option<RepoConfig>,
    ) -> Self {
        Self {
            root: root.into(),
            hooks,
            repo,
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
            if let Some(repo) = &self.repo
                && let Err(err) = self.clone_repo(repo, &path).await
            {
                if let Err(cleanup_err) = fs::remove_dir_all(&path).await {
                    tracing::warn!(
                        error=%cleanup_err,
                        path=%path.display(),
                        "repo_clone_cleanup outcome=failed"
                    );
                }
                return Err(err);
            }
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
        let path = root.join(sanitize_workspace_key(identifier));
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

    async fn clone_repo(&self, repo: &RepoConfig, cwd: &Path) -> Result<(), WorkspaceError> {
        tracing::info!(cwd=%cwd.display(), "repo_clone outcome=started");
        let mut child = Command::new("git");
        child.arg("clone");
        if let Some(depth) = repo.clone.depth {
            child.arg("--depth").arg(depth.to_string());
        }
        child
            .arg("--")
            .arg(&repo.origin)
            .arg(".")
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        child.kill_on_drop(true);
        let mut child = child
            .spawn()
            .map_err(|err| WorkspaceError::Io(err.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| WorkspaceError::Io("repo clone stdout pipe unavailable".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| WorkspaceError::Io("repo clone stderr pipe unavailable".to_string()))?;
        let stdout_task = tokio::spawn(async move {
            let mut stdout = stdout;
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut stderr = stderr;
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let status =
            match time::timeout(Duration::from_millis(self.hooks.timeout_ms), child.wait()).await {
                Ok(result) => result.map_err(|err| WorkspaceError::Io(err.to_string()))?,
                Err(_) => {
                    if let Err(err) = child.start_kill() {
                        tracing::warn!(error=%err, "repo_clone_timeout_kill outcome=failed");
                    }
                    if let Err(err) = child.wait().await {
                        tracing::warn!(error=%err, "repo_clone_timeout_wait outcome=failed");
                    }
                    let _ = collect_child_output(stdout_task).await;
                    let _ = collect_child_output(stderr_task).await;
                    return Err(WorkspaceError::RepoCloneTimeout);
                }
            };
        let _stdout = collect_child_output(stdout_task).await?;
        let stderr = collect_child_output(stderr_task).await?;
        if status.success() {
            tracing::info!("repo_clone outcome=completed");
            return Ok(());
        }
        let status = status.code().unwrap_or(-1);
        let stderr = summarize_clone_stderr(&stderr);
        Err(WorkspaceError::RepoCloneFailed { status, stderr })
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

async fn collect_child_output(
    task: JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, WorkspaceError> {
    task.await
        .map_err(|err| WorkspaceError::Io(err.to_string()))?
        .map_err(|err| WorkspaceError::Io(err.to_string()))
}

fn summarize_clone_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let summary = lines
        .iter()
        .rev()
        .copied()
        .find(|line| line.contains("fatal:") || line.contains("error:"))
        .or_else(|| lines.last().copied())
        .unwrap_or("");
    redact_url_credentials(summary)
}

fn redact_url_credentials(raw: &str) -> String {
    let mut redacted = raw.to_string();
    for scheme in ["https://", "http://", "ssh://"] {
        let mut search_start = 0;
        while let Some(relative_start) = redacted[search_start..].find(scheme) {
            let credential_start = search_start + relative_start + scheme.len();
            let url_end = redacted[credential_start..]
                .find(char::is_whitespace)
                .map(|relative_end| credential_start + relative_end)
                .unwrap_or(redacted.len());
            let url = &redacted[credential_start..url_end];
            if let Some(relative_at) = url.find('@') {
                let at = credential_start + relative_at;
                let next_slash = url
                    .find('/')
                    .map(|relative_slash| credential_start + relative_slash);
                if next_slash.is_none_or(|slash| slash > at) {
                    redacted.replace_range(credential_start..at, "[redacted]");
                    search_start = credential_start + "[redacted]@".len();
                    continue;
                }
            }
            search_start = url_end;
        }
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::summarize_clone_stderr;

    #[test]
    fn clone_stderr_summary_prefers_fatal_line_and_redacts_credentials() {
        let stderr = b"Cloning into '.'...\nfatal: could not read Username for 'https://user:secret@example.com': terminal prompts disabled\n";

        let summary = summarize_clone_stderr(stderr);

        assert_eq!(
            summary,
            "fatal: could not read Username for 'https://[redacted]@example.com': terminal prompts disabled"
        );
    }

    #[test]
    fn clone_stderr_redaction_continues_after_url_without_credentials() {
        let stderr = b"fatal: failed after https://example.com/repo then https://user:secret@example.com/repo";

        let summary = summarize_clone_stderr(stderr);

        assert_eq!(
            summary,
            "fatal: failed after https://example.com/repo then https://[redacted]@example.com/repo"
        );
    }
}
