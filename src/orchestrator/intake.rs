//! Background issue intake loop.
//!
//! Intake is its own task because the user-supplied pull command can
//! take arbitrary time. The loop runs the command, parses stdout as a
//! JSON array of issues, and emits one event per parsed issue. Intake
//! never decides what to do with an issue — that is dispatch's job.

use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::string::FromUtf8Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::context::{Issues, RenderContext};
use crate::shell::{CommandExecError, CommandExt};
use crate::template::{JinjaRenderer, TemplateError};
use crate::workflow::Workflow;

use super::event::EventProducer;

const PULL_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const STDERR_TAIL_BYTES: usize = 4096;

#[derive(Clone)]
pub(super) struct IntakeLoop {
  workflow: Arc<Workflow>,
  producer: EventProducer,
  renderer: JinjaRenderer,
}

impl IntakeLoop {
  pub(super) fn new(workflow: Arc<Workflow>, producer: EventProducer) -> Self {
    Self {
      workflow,
      producer,
      renderer: JinjaRenderer::new(),
    }
  }

  pub(super) fn start(self, shutdown: CancellationToken) -> JoinHandle<()> {
    let span = tracing::info_span!("intake");
    tokio::spawn(async move { self.run(shutdown).await }.instrument(span))
  }

  async fn run(self, shutdown: CancellationToken) {
    let max_iterations = self.workflow.schema().loop_.max_iterations;
    let mut iterations = 0_u64;
    let Some(pull) = self.workflow.schema().issues.pull.clone() else {
      self.producer.intake_stopped().await;
      return;
    };

    let command = match self.renderer.render(&pull.command, self.workflow.as_render_context()) {
      Ok(command) => command,
      Err(error) => {
        self.producer.intake_failed(IntakeError::PullCommandTemplate(error)).await;
        return;
      },
    };

    loop {
      if shutdown.is_cancelled() || max_iterations.is_some_and(|max| iterations >= max) {
        break;
      }

      iterations = iterations.saturating_add(1);
      match self.run_once(&command, &shutdown).await {
        Ok(()) => {},
        // A cancelled pull is not a failure — shutdown raced the
        // command and we will exit on the next iteration check.
        Err(IntakeError::Cancelled) if shutdown.is_cancelled() => break,
        Err(error) => self.producer.intake_failed(error).await,
      }

      let wait = Duration::from_secs(pull.idle_sec);
      tokio::select! {
        _ = shutdown.cancelled() => break,
        _ = tokio::time::sleep(wait) => {},
      }
    }

    // Always emit Stopped on clean exit so the orchestrator knows the
    // intake side has drained and can decide whether to terminate.
    self.producer.intake_stopped().await;
  }

  async fn run_once(&self, command: &str, shutdown: &CancellationToken) -> Result<(), IntakeError> {
    let stdout = run_pull_command(self.workflow.as_ref(), command, shutdown).await?;
    let issues = parse_issues_output(&stdout)?;
    tracing::info!(candidates = issues.len(), "issues pulled");
    let mut seen = HashSet::new();

    for issue in issues.iter().cloned() {
      // First-wins dedup inside one cycle; trackers occasionally repeat
      // the same issue id across queries and we do not want to launch
      // duplicate dispatch attempts inside a single batch.
      if seen.insert(issue.id.clone()) {
        self.producer.intake_issue(issue).await;
      } else {
        tracing::warn!(
          issue_id = %issue.id,
          "duplicate issue id from intake; keeping first issue",
        );
      }
    }

    Ok(())
  }
}

async fn run_pull_command(
  workflow: &Workflow,
  command: &str,
  shutdown: &CancellationToken,
) -> Result<String, IntakeError> {
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

  let mut child = cmd.timeout(PULL_COMMAND_TIMEOUT).spawn().map_err(IntakeError::PullCommand)?;
  let output = tokio::select! {
    result = child.wait_with_output() => result?,
    _ = shutdown.cancelled() => {
      // Explicit cancel + wait so a SIGTERM does not leak a child
      // process into the operator's session.
      child.cancel();
      let _ = child.wait().await;
      return Err(IntakeError::Cancelled);
    },
  };

  let duration_ms = started.elapsed().as_millis() as u64;
  if !output.status.success() {
    return Err(IntakeError::PullCommandExit {
      code: output.status.code().unwrap_or(-1),
      stderr_tail: tail_utf8(&output.stderr, STDERR_TAIL_BYTES),
    });
  }

  tracing::info!(duration_ms, "issues pull command completed");

  String::from_utf8(output.stdout).map_err(IntakeError::PullCommandStdout)
}

fn parse_issues_output(output: &str) -> Result<Issues, IntakeError> {
  serde_json::from_str(output).map_err(IntakeError::ParseIssues)
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
enum IntakeError {
  #[error(transparent)]
  PullCommandTemplate(#[from] TemplateError),
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
  use std::sync::Arc;

  use super::super::event::{IntakeEvent, OrchestratorEvent, event_channel};
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
      IntakeError::PullCommandExit {
        code: 7,
        ref stderr_tail
      } if stderr_tail == "broken"
    ));
  }

  #[tokio::test]
  async fn run_once_emits_issues_from_command_json() {
    let cwd = std::env::current_dir().expect("cwd");
    let workflow = Workflow::builder().workflow_path(cwd.join("workflow.yml")).build();
    let (producer, mut consumer) = event_channel();
    let intake = IntakeLoop::new(Arc::new(workflow), producer);

    intake
      .run_once(
        r#"printf '%s' '[{"id":"ABC-1","title":"Pulled","state":"todo"}]'"#,
        &CancellationToken::new(),
      )
      .await
      .expect("intake runs");

    match consumer.recv().await.expect("event") {
      OrchestratorEvent::Intake(IntakeEvent::Issue(issue)) => {
        assert_eq!(issue.id, "ABC-1");
        assert_eq!(issue.title, "Pulled");
        assert_eq!(issue.state, "todo");
      },
      _ => panic!("expected intake issue"),
    }
  }
}
