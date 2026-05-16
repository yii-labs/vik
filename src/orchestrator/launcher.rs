//! Per-stage launch task.
//!
//! Each call to [`StageLauncher::launch`] spawns an independent task that
//! takes a stage from `before_run` through session monitoring and
//! `after_run`. The launcher carries no running-state; every transition
//! is reported back through the event channel so the main loop is the
//! sole owner of [`super::running::RunningMap`].

use std::sync::Arc;
use std::time::Instant;

use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::context::IssueStage;
use crate::hooks::HookError;
use crate::logging::stage_span;
use crate::session::{SessionError, SessionFactory, SessionState};
use crate::workflow::Workflow;

use super::event::EventProducer;
use super::monitor::SessionMonitor;

#[derive(Clone)]
pub(super) struct StageLauncher {
  workflow: Arc<Workflow>,
  factory: SessionFactory,
  producer: EventProducer,
}

impl StageLauncher {
  pub(super) fn new(workflow: Arc<Workflow>, factory: SessionFactory, producer: EventProducer) -> Self {
    Self {
      workflow,
      factory,
      producer,
    }
  }

  /// Spawn the launch task. The runtime `IssueStage` already carries
  /// the issue run context needed for hooks and session spawn.
  pub(super) fn launch(&self, issue_stage: IssueStage, shutdown: CancellationToken) {
    let span = stage_span(
      &issue_stage.issue().id,
      issue_stage.stage_name(),
      &issue_stage.stage().agent,
    );
    let launcher = self.clone();

    tokio::spawn(async move { launcher.run(issue_stage, shutdown).await }.instrument(span));
  }

  async fn run(self, issue_stage: IssueStage, shutdown: CancellationToken) {
    if shutdown.is_cancelled() {
      return;
    }

    tracing::info!("stage launching");

    let key = issue_stage.key();
    let started_at = Instant::now();
    let session = match self.start_session(&issue_stage).await {
      Ok(session) => session,
      Err(error) => {
        tracing::error!(error = %error, "Failed to start session for issue stage");
        // No session ever existed; report under the reserved key so the
        // main loop can release the reservation.
        self.producer.stage_failed(key, error).await;
        return;
      },
    };

    tracing::info!("stage session started");
    self.producer.stage_started(issue_stage.clone(), session.clone()).await;

    let monitor = SessionMonitor::new(key.clone(), session.clone(), self.producer.clone());
    let terminal = tokio::select! {
      snapshot = monitor.watch() => snapshot,
      _ = shutdown.cancelled() => {
        // Cooperative shutdown: cancel the child then wait for the
        // session's own state machine to mark itself Cancelled. Without
        // the wait, `terminal` could observe a stale state.
        tracing::info!("stage cancellation requested");
        session.cancel();
        session.wait().await
      }
    };

    // `after_run` is skipped on cancellation only — failures still want
    // their cleanup to run. The hook's own failure is logged but not
    // propagated, so a flaky `after_run` cannot mask a stage result.
    if !matches!(terminal.state, SessionState::Cancelled)
      && let Err(error) = self
        .workflow
        .hooks()
        .after_issue_stage_run(&issue_stage, &issue_stage.stage().hooks.after_run)
        .await
    {
      tracing::error!(
        error = %error,
        "issue stage after_run hook failed",
      );
    }

    let duration_ms = started_at.elapsed().as_millis() as u64;
    tracing::info!(
      state = ?terminal.state,
      duration_ms,
      tokens_input = terminal.tokens.input,
      tokens_output = terminal.tokens.output,
      tokens_cache_read = terminal.tokens.cache_read,
      "stage terminal",
    );

    self.producer.stage_terminal(key, terminal).await;
  }

  async fn start_session(&self, issue_stage: &IssueStage) -> Result<crate::session::Session, StageLaunchError> {
    // `before_run` failure aborts the stage before any agent process is
    // spawned, so a misconfigured setup hook cannot waste tokens.
    if let Err(err) = self
      .workflow
      .hooks()
      .before_issue_stage_run(issue_stage, &issue_stage.stage().hooks.before_run)
      .await
    {
      tracing::error!(
        error = %err,
        "issue stage before_run hook failed",
      );
      return Err(StageLaunchError::Hook(err));
    }

    Ok(self.factory.spawn_stage(issue_stage.clone()).await?)
  }
}

#[derive(Debug, Error)]
enum StageLaunchError {
  #[error(transparent)]
  Hook(#[from] HookError),
  #[error(transparent)]
  Session(#[from] SessionError),
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use tokio_util::sync::CancellationToken;
  use tracing::subscriber::with_default;
  use tracing_subscriber::{Registry, layer::SubscriberExt};

  use super::*;
  use crate::context::{Issue, IssueRun};
  use crate::logging::tests::{CaptureLayer, captured_event, captured_message_exists};
  use crate::orchestrator::event::{OrchestratorEvent, StageEvent, event_channel};

  #[tokio::test]
  async fn run_reports_stage_failed_when_before_run_hook_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow = Arc::new(
      Workflow::builder()
        .workflow_path(temp.path().join("workflow.yml"))
        .workspace_root(temp.path().join("workspace"))
        .add_stage("plan", "todo", "./plan.md")
        .build(),
    );
    let issue_stage = issue_stage(Arc::clone(&workflow), "ABC-1", "plan", "exit 7");
    let key = issue_stage.key();
    std::fs::create_dir_all(issue_stage.workdir()).expect("issue workdir exists");
    let (producer, mut consumer) = event_channel();
    let launcher = StageLauncher::new(
      Arc::clone(&workflow),
      SessionFactory::new(Arc::clone(&workflow)),
      producer,
    );

    launcher.run(issue_stage, CancellationToken::new()).await;

    match consumer.recv().await.expect("stage failure event") {
      OrchestratorEvent::Stage(StageEvent::Failed { key: failed_key, error }) => {
        assert_eq!(failed_key, key);
        assert!(error.contains("before_issue_stage_run"));
        assert!(error.contains("7"));
      },
      _ => panic!("expected stage failure"),
    }
  }

  #[test]
  fn run_emits_stage_launching_lifecycle_log() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    with_default(subscriber, || {
      let temp = tempfile::tempdir().expect("tempdir");
      let workflow = Arc::new(
        Workflow::builder()
          .workflow_path(temp.path().join("workflow.yml"))
          .workspace_root(temp.path().join("workspace"))
          .add_stage("plan", "todo", "./plan.md")
          .build(),
      );
      let issue_stage = issue_stage(Arc::clone(&workflow), "ABC-1", "plan", "exit 7");
      std::fs::create_dir_all(issue_stage.workdir()).expect("issue workdir exists");
      let (producer, _consumer) = event_channel();
      let launcher = StageLauncher::new(
        Arc::clone(&workflow),
        SessionFactory::new(Arc::clone(&workflow)),
        producer,
      );

      let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
      let span = stage_span(
        &issue_stage.issue().id,
        issue_stage.stage_name(),
        &issue_stage.stage().agent,
      );
      runtime.block_on(async move {
        launcher.run(issue_stage, CancellationToken::new()).instrument(span).await;
      });
    });

    let events = events.lock().expect("events mutex");
    assert!(captured_message_exists(&events, "stage launching"));
    let event = captured_event(&events, "stage launching");
    assert_eq!(event["issue_id"], "ABC-1");
    assert_eq!(event["stage_name"], "plan");
    assert_eq!(event["phase"], crate::logging::Phase::StageRun.to_string());
  }

  fn issue_stage(workflow: Arc<Workflow>, issue_id: &str, stage_name: &str, before_run: &str) -> IssueStage {
    let mut schema = workflow.stages().get(stage_name).expect("stage fixture exists").clone();
    schema.hooks.before_run = Some(before_run.to_string());
    let issue_run = Arc::new(IssueRun::new(
      Arc::clone(&workflow),
      issue(issue_id, &schema.when.state),
    ));

    IssueStage::new(issue_run, stage_name.to_string(), schema)
  }

  fn issue(id: &str, state: &str) -> Issue {
    Issue {
      id: id.to_string(),
      title: "title".to_string(),
      description: String::new(),
      state: state.to_string(),
      extra_payload: serde_yaml::Mapping::new(),
    }
  }
}
