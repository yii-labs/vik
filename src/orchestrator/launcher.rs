//! Per-stage launch task.
//!
//! Each call to [`StageLauncher::launch`] spawns an independent task that
//! takes a stage from `before_run` through session monitoring and
//! `after_run`. The launcher carries no running-state; every transition
//! is reported back through the event channel so the main loop is the
//! sole owner of [`super::running::RunningMap`].

use std::sync::Arc;

use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::context::IssueStage;
use crate::hooks::HookError;
use crate::logging::{Phase, stage_span};
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

    let key = issue_stage.key();
    let session = match self.start_session(&issue_stage).await {
      Ok(session) => session,
      Err(error) => {
        // No session ever existed; report under the reserved key so the
        // main loop can release the reservation.
        self.producer.stage_failed(key, error).await;
        return;
      },
    };

    let runtime = format!("{:?}", session.profile().runtime);
    tracing::Span::current().record("runtime", runtime.as_str());
    self.producer.stage_started(issue_stage.clone(), session.clone()).await;

    let monitor = SessionMonitor::new(key.clone(), session.clone(), self.producer.clone());
    let terminal = tokio::select! {
      snapshot = monitor.watch() => snapshot,
      _ = shutdown.cancelled() => {
        // Cooperative shutdown: cancel the child then wait for the
        // session's own state machine to mark itself Cancelled. Without
        // the wait, `terminal` could observe a stale state.
        session.cancel();
        session.wait().await
      }
    };

    // `after_run` is skipped on cancellation only — failures still want
    // their cleanup to run. The hook's own failure is logged but not
    // propagated, so a flaky `after_run` cannot mask a stage result.
    if !matches!(terminal.state, SessionState::Cancelled)
      && let Err(error) = self.run_after_run(&issue_stage).await
    {
      tracing::error!(
        phase = %Phase::Hook,
        error = %error,
        "after_run hook failed",
      );
    }

    self.producer.stage_terminal(key, terminal).await;
  }

  async fn start_session(&self, issue_stage: &IssueStage) -> Result<crate::session::Session, StageLaunchError> {
    // `before_run` failure aborts the stage before any agent process is
    // spawned, so a misconfigured setup hook cannot waste tokens.
    self.run_before_run(issue_stage).await?;

    Ok(self.factory.spawn_stage(issue_stage.clone()).await?)
  }

  async fn run_before_run(&self, issue_stage: &IssueStage) -> Result<(), HookError> {
    let ctx = issue_stage.template_context();
    self.workflow.hooks().run_before_run(&issue_stage.stage().hooks, ctx).await
  }

  async fn run_after_run(&self, issue_stage: &IssueStage) -> Result<(), HookError> {
    let ctx = issue_stage.template_context();
    self.workflow.hooks().run_after_run(&issue_stage.stage().hooks, ctx).await
  }
}

#[derive(Debug, Error)]
enum StageLaunchError {
  #[error(transparent)]
  Hook(#[from] HookError),
  #[error(transparent)]
  Session(#[from] SessionError),
}
