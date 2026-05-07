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

use crate::hooks::HookError;
use crate::logging::{Phase, stage_span};
use crate::session::{SessionError, SessionFactory, SessionState};
use crate::template::StageContext;
use crate::workflow::Workflow;

use super::event::EventProducer;
use super::monitor::SessionMonitor;
use super::types::IssueStage;

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

  /// Spawn the launch task. `issue_workdir` is the path created by
  /// issue-level setup; we receive it instead of recomputing so the
  /// launcher and `prepare_issue` can never disagree on layout.
  pub(super) fn launch(&self, issue_stage: IssueStage, issue_workdir: std::path::PathBuf, shutdown: CancellationToken) {
    let span = stage_span(
      &issue_stage.issue().id,
      issue_stage.stage().name(),
      &issue_stage.stage().schema().agent,
    );
    let launcher = self.clone();

    tokio::spawn(async move { launcher.run(issue_stage, issue_workdir, shutdown).await }.instrument(span));
  }

  async fn run(self, issue_stage: IssueStage, issue_workdir: std::path::PathBuf, shutdown: CancellationToken) {
    if shutdown.is_cancelled() {
      return;
    }

    let key = issue_stage.key();
    let session = match self.start_session(&issue_stage, &issue_workdir).await {
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
      && let Err(error) = self.run_after_run(&issue_stage, &issue_workdir).await
    {
      tracing::error!(
        phase = %Phase::Hook,
        error = %error,
        "after_run hook failed",
      );
    }

    self.producer.stage_terminal(key, terminal).await;
  }

  async fn start_session(
    &self,
    issue_stage: &IssueStage,
    issue_workdir: &std::path::Path,
  ) -> Result<crate::session::Session, StageLaunchError> {
    let issue = issue_stage.issue().clone();
    // `before_run` failure aborts the stage before any agent process is
    // spawned, so a misconfigured setup hook cannot waste tokens.
    self.run_before_run(issue_stage, &issue, issue_workdir).await?;

    Ok(
      self
        .factory
        .spawn_named(
          issue,
          issue_stage.stage().name().to_string(),
          issue_stage.stage().schema().clone(),
          issue_workdir.to_path_buf(),
        )
        .await?,
    )
  }

  async fn run_before_run(
    &self,
    issue_stage: &IssueStage,
    issue: &crate::context::Issue,
    issue_workdir: &std::path::Path,
  ) -> Result<(), HookError> {
    let ctx = self.stage_context(issue_stage, issue, issue_workdir);
    self
      .workflow
      .hooks()
      .run_before_run(&issue_stage.stage().schema().hooks, ctx)
      .await
  }

  async fn run_after_run(&self, issue_stage: &IssueStage, issue_workdir: &std::path::Path) -> Result<(), HookError> {
    let issue = issue_stage.issue().clone();
    let ctx = self.stage_context(issue_stage, &issue, issue_workdir);
    self
      .workflow
      .hooks()
      .run_after_run(&issue_stage.stage().schema().hooks, ctx)
      .await
  }

  fn stage_context<'a>(
    &'a self,
    issue_stage: &'a IssueStage,
    issue: &'a crate::context::Issue,
    issue_workdir: &'a std::path::Path,
  ) -> StageContext<'a> {
    StageContext {
      issue,
      stage_name: issue_stage.stage().name(),
      agent_profile: &issue_stage.stage().schema().agent,
      stage_state: &issue_stage.stage().schema().when.state,
      issue_workdir,
      workspace_root: self.workflow.workspace().root(),
    }
  }
}

#[derive(Debug, Error)]
enum StageLaunchError {
  #[error(transparent)]
  Hook(#[from] HookError),
  #[error(transparent)]
  Session(#[from] SessionError),
}
