//! Background issue intake loop.
//!
//! Intake is its own task because the user-supplied pull command can
//! take arbitrary time. The loop runs the command, parses stdout as a
//! JSON array of issues, and emits one event per parsed issue. Intake
//! never decides what to do with an issue — that is dispatch's job.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::context::RenderContext;
use crate::template::{JinjaRenderer, TemplateError};
use crate::workflow::Workflow;

use super::event::EventProducer;
use super::pull::{IssuePullError, parse_issues_output, run_pull_command};

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

    let command = match self.renderer.render(
      &self.workflow.schema().issues.pull.command,
      self.workflow.as_render_context(),
    ) {
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
        Err(IntakeError::PullCommand(IssuePullError::Cancelled)) if shutdown.is_cancelled() => break,
        Err(error) => self.producer.intake_failed(error).await,
      }

      let wait = Duration::from_secs(self.workflow.schema().issues.pull.idle_sec);
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

#[derive(Debug, Error)]
enum IntakeError {
  #[error(transparent)]
  PullCommandTemplate(#[from] TemplateError),
  #[error(transparent)]
  PullCommand(#[from] IssuePullError),
}

#[cfg(all(test, not(windows)))]
mod tests {
  use std::sync::Arc;

  use super::super::event::{IntakeEvent, OrchestratorEvent, event_channel};
  use super::*;

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
