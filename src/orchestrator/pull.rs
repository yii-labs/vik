//! Pull-command execution for issue intake.

use std::path::Path;
use std::process::Stdio;
use std::string::FromUtf8Error;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::context::Issues;
use crate::shell::{CommandExecError, CommandExt};
use crate::workflow::Workflow;

const PULL_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const STDERR_TAIL_BYTES: usize = 4096;

pub(super) async fn run_pull_command(
  workflow: &Workflow,
  command: &str,
  shutdown: &CancellationToken,
) -> Result<String, IssuePullError> {
  // Pull command runs in the workflow file directory so relative paths
  // (e.g. `./scripts/issues-json`) resolve as the author wrote them.
  let cwd = workflow.workflow_path().parent().unwrap_or_else(|| Path::new("."));
  let started = Instant::now();

  let mut cmd = shell_command(command);
  cmd.current_dir(cwd).stdout(Stdio::piped()).stderr(Stdio::piped());

  tracing::debug!(
    cwd = %cwd.display(),
    "issue pull command starting",
  );

  let mut child = cmd.timeout(PULL_COMMAND_TIMEOUT).spawn().map_err(IssuePullError::PullCommand)?;
  let output = tokio::select! {
    result = child.wait_with_output() => result?,
    _ = shutdown.cancelled() => {
      // Explicit cancel + wait so a SIGTERM does not leak a child
      // process into the operator's session.
      child.cancel();
      let _ = child.wait().await;
      return Err(IssuePullError::Cancelled);
    },
  };

  let duration_ms = started.elapsed().as_millis() as u64;
  if !output.status.success() {
    return Err(IssuePullError::PullCommandExit {
      code: output.status.code().unwrap_or(-1),
      stderr_tail: tail_utf8(&output.stderr, STDERR_TAIL_BYTES),
    });
  }

  tracing::info!(duration_ms, "issues pull command completed");

  String::from_utf8(output.stdout).map_err(IssuePullError::PullCommandStdout)
}

pub(super) fn parse_issues_output(output: &str) -> Result<Issues, IssuePullError> {
  serde_json::from_str(output).map_err(IssuePullError::ParseIssues)
}

/// Bound stderr surface in error messages; trackers occasionally dump
/// large traces and the operator only needs the tail.
fn tail_utf8(bytes: &[u8], limit: usize) -> String {
  if bytes.len() <= limit {
    return String::from_utf8_lossy(bytes).into_owned();
  }
  let start = bytes.len() - limit;
  String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
  let mut shell = Command::new("cmd");
  shell.args(["/C", command]);
  shell
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
  let mut shell = Command::new("sh");
  shell.args(["-c", command]);
  shell
}

#[derive(Debug, Error)]
pub(super) enum IssuePullError {
  #[error(transparent)]
  PullCommand(#[from] CommandExecError),
  #[error("issue pull command exited with code {code}: {stderr_tail}")]
  PullCommandExit { code: i32, stderr_tail: String },
  #[error("issue pull command stdout was not valid UTF-8: {0}")]
  PullCommandStdout(FromUtf8Error),
  #[error("failed to parse issue pull JSON: {0}")]
  ParseIssues(serde_json::Error),
  #[error("issue pull command cancelled")]
  Cancelled,
}

#[cfg(all(test, not(windows)))]
mod tests {
  use super::*;

  #[test]
  fn parses_pull_stdout_as_issue_json() {
    let issues = parse_issues_output(
      r#"[
        {"id":"123","title":"Add retry tests","state":"todo","labels":["vik"]},
        {"identifier":"LIN-1","title":"Ship state alias","status":"work"}
      ]"#,
    )
    .expect("issues parse");

    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].id, "123");
    assert_eq!(issues[0].state, "todo");
    assert_eq!(issues[1].id, "LIN-1");
    assert_eq!(issues[1].state, "work");
  }

  #[test]
  fn stderr_tail_is_limited() {
    let tail = tail_utf8(b"abcdef", 3);

    assert_eq!(tail, "def");
  }

  #[tokio::test]
  async fn runs_pull_command_from_workflow_directory() {
    let cwd = std::env::current_dir().expect("cwd");
    let workflow = Workflow::builder()
      .pull_command("pwd")
      .workflow_path(cwd.join("workflow.yml"))
      .build();

    let stdout = run_pull_command(&workflow, "pwd", &CancellationToken::new())
      .await
      .expect("pull command runs");

    let actual = std::fs::canonicalize(stdout.trim()).expect("canonical actual cwd");
    let expected = std::fs::canonicalize(cwd).expect("canonical workflow dir");

    assert_eq!(actual, expected);
  }

  #[tokio::test]
  async fn nonzero_pull_command_keeps_stderr_tail() {
    let cwd = std::env::current_dir().expect("cwd");
    let workflow = Workflow::builder()
      .pull_command("printf '%s' 'broken' >&2; exit 7")
      .workflow_path(cwd.join("workflow.yml"))
      .build();

    let err = run_pull_command(&workflow, "printf '%s' 'broken' >&2; exit 7", &CancellationToken::new())
      .await
      .expect_err("pull command must fail");

    assert!(matches!(
      err,
      IssuePullError::PullCommandExit {
        code: 7,
        ref stderr_tail
      } if stderr_tail == "broken"
    ));
  }
}
