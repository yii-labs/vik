//! Stage-session manager.
//!
//! This is the bridge between tracker intake and session runtime. It owns
//! stage matching, issue preparation, hook execution, session command
//! senders, and drain signalling. The top-level orchestrator only passes
//! issues in and waits for the manager to become empty.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::context::{Issue, IssueRun, IssueStage, IssueStageKey};
use crate::hooks::HookError;
use crate::logging::Phase;
use crate::session::{SessionCommandSender, SessionError, SessionFactory, SessionState, SessionStateReceiver};
use crate::workflow::Workflow;

pub(super) struct StageSessionManager {
  workflow: Arc<Workflow>,
  factory: SessionFactory,
  sessions: HashMap<IssueStageKey, Option<SessionCommandSender>>,
  stages: HashMap<IssueStageKey, IssueStage>,
  session_events_channel: (mpsc::Sender<SessionEvent>, mpsc::Receiver<SessionEvent>),
  shutdown: CancellationToken,
}

impl StageSessionManager {
  pub(super) fn new(workflow: Arc<Workflow>) -> Self {
    Self {
      factory: SessionFactory::new(Arc::clone(&workflow)),
      workflow,
      sessions: HashMap::new(),
      stages: HashMap::new(),
      session_events_channel: mpsc::channel::<SessionEvent>(8),
      shutdown: CancellationToken::new(),
    }
  }

  pub(super) fn is_empty(&self) -> bool {
    self.sessions.is_empty()
  }

  pub(super) async fn try_run_issue(&mut self, issue: Issue) {
    let _span = tracing::info_span!(
      "issue",
      phase = %Phase::Dispatch,
      issue_id = &issue.id,
      issue_state = &issue.state
    )
    .entered();

    if self.shutdown.is_cancelled() {
      tracing::info!("stage session manager is shutting down; skipping issue this cycle");
      return;
    }

    let issue_run = Arc::new(IssueRun::new(Arc::clone(&self.workflow), issue));
    let dispatch = self.should_dispatch(Arc::clone(&issue_run));
    if let Some(reason) = dispatch.skip_reason {
      reason.trace();
      return;
    }

    let issue_stages = self.reserve_issue_stages(dispatch.issue_stages);
    if issue_stages.is_empty() {
      DispatchSkipReason::MatchingStagesAlreadyActive.trace();
      return;
    }

    let shutdown = self.shutdown.clone();

    let result = tokio::select! {
      biased;
      result = issue_run.prepare().in_current_span() => result,
      _ = shutdown.cancelled() => return,
    };

    match result {
      Ok(()) => {
        tracing::debug!("issue prepared successfully");
        self.launch_issue_stages(issue_stages);
      },
      Err(error) => {
        tracing::error!(error = %error, "issue preparation failed");

        for key in issue_stages.iter().map(IssueStage::key) {
          self.sessions.remove(&key);
          self.stages.remove(&key);
        }
      },
    }
  }

  pub(super) async fn recv(&mut self) -> Option<SessionManagerEvent> {
    let event = self.session_events_channel.1.recv().await?;
    let was_empty = self.is_empty();
    self.handle_event(event).await;

    if !was_empty && self.is_empty() {
      Some(SessionManagerEvent::Drained)
    } else {
      None
    }
  }

  pub(super) async fn cancel_all(&mut self) {
    self.shutdown.cancel();

    for (key, commands) in self
      .sessions
      .iter()
      .filter_map(|(key, commands)| commands.as_ref().map(|commands| (key, commands)))
    {
      if let Err(error) = commands.cancel().in_current_span().await {
        tracing::error!(issue_id = %key.issue_id, stage = %key.stage_name, error = %error, "failed to send cancel command to session");
      }
    }
  }

  async fn handle_event(&mut self, event: SessionEvent) {
    match event {
      SessionEvent::SessionStarted { key, commands } => {
        self.save_stage_session(key, commands);
      },
      SessionEvent::SessionStateUpdated { key, state } => {
        if state.is_terminated() {
          self.finish_stage(key, state).await;
        }
      },
      SessionEvent::SessionFinished { key } => {
        self.drain_key_state(&key);
      },
    }
  }

  fn save_stage_session(&mut self, key: IssueStageKey, commands: SessionCommandSender) {
    if let Some(slot) = self.sessions.get_mut(&key) {
      *slot = Some(commands);
    }
  }

  fn launch_issue_stages(&self, issue_stages: Vec<IssueStage>) {
    let stage_names: Vec<&str> = issue_stages.iter().map(|s| s.stage_name()).collect();
    tracing::info!(
      stage_names = ?stage_names,
      "issue ready; launching stages",
    );

    for issue_stage in issue_stages {
      self.launch_issue_stage(issue_stage);
    }
  }

  fn launch_issue_stage(&self, issue_stage: IssueStage) {
    let _span = tracing::info_span!(
      "stage",
      phase = %Phase::StageRun,
      stage_name = %issue_stage.stage().name,
      stage_profile = %issue_stage.stage().agent,
    )
    .entered();

    let workflow = Arc::clone(&self.workflow);
    let factory = self.factory.clone();
    let event_tx = self.session_events_channel.0.clone();
    let shutdown = self.shutdown.clone();

    let key = issue_stage.key();
    if shutdown.is_cancelled() {
      return;
    }

    tracing::debug!("launching issue stage");

    tokio::spawn(
      async move {
        if let Err(error) = Self::before_run(&workflow, &issue_stage).await {
          tracing::error!(error = %error, "issue stage before_run hook failed");
          Self::send_session_event(&event_tx, SessionEvent::SessionFinished { key }).await;
          return;
        }

        let (commands, states) = match factory.spawn_stage(issue_stage.clone(), shutdown.clone()) {
          Ok(session) => session,
          Err(error) => {
            tracing::error!(error = %error, "session spawn failed");
            Self::send_session_event(&event_tx, SessionEvent::SessionFinished { key }).await;
            return;
          },
        };

        Self::send_session_event(
          &event_tx,
          SessionEvent::SessionStarted {
            key: key.clone(),
            commands,
          },
        )
        .await;

        Self::proxy_session_state(key, states, event_tx).in_current_span().await;
      }
      .in_current_span(),
    );
  }

  async fn finish_stage(&mut self, key: IssueStageKey, state: SessionState) {
    let Some(issue_stage) = self.stages.get(&key).cloned() else {
      return;
    };

    let workflow = Arc::clone(&self.workflow);

    Self::after_run(workflow, issue_stage, state).await;
  }

  fn should_dispatch(&self, issue_run: Arc<IssueRun>) -> DispatchDecision {
    let issue_id = issue_run.issue().id.clone();
    let matching_stages = IssueRun::matching_stages(issue_run);
    if matching_stages.is_empty() {
      return DispatchDecision::skip(DispatchSkipReason::NoMatchingStage);
    }

    if !self.can_accept_issue(&issue_id) {
      return DispatchDecision::skip(DispatchSkipReason::IssueConcurrencyFull);
    }

    let issue_stages = matching_stages
      .into_iter()
      .filter(|issue_stage| !self.sessions.contains_key(&issue_stage.key()))
      .collect::<Vec<_>>();

    if issue_stages.is_empty() {
      return DispatchDecision::skip(DispatchSkipReason::MatchingStagesAlreadyActive);
    }

    DispatchDecision::run(issue_stages)
  }

  fn reserve_issue_stages(&mut self, issue_stages: Vec<IssueStage>) -> Vec<IssueStage> {
    issue_stages
      .into_iter()
      .filter(|issue_stage| {
        let key = issue_stage.key();
        if self.sessions.contains_key(&key) || !self.can_accept_issue(&key.issue_id) {
          return false;
        }

        self.sessions.insert(key.clone(), None);
        self.stages.insert(key, issue_stage.clone());
        true
      })
      .collect()
  }

  fn drain_key_state(&mut self, key: &IssueStageKey) {
    self.sessions.remove(key);
    self.stages.remove(key);
  }

  fn can_accept_issue(&self, issue_id: &str) -> bool {
    self.contains_issue(issue_id)
      || self.running_issue_count() < self.workflow.schema().loop_.max_issue_concurrency as usize
  }

  fn contains_issue(&self, issue_id: &str) -> bool {
    self.stages.values().any(|stage| stage.issue().id == issue_id)
  }

  fn running_issue_count(&self) -> usize {
    self
      .stages
      .values()
      .map(|stage| stage.issue().id.as_str())
      .collect::<HashSet<_>>()
      .len()
  }

  async fn before_run(workflow: &Workflow, issue_stage: &IssueStage) -> Result<(), StageLaunchError> {
    workflow
      .hooks()
      .before_issue_stage_run(issue_stage, &issue_stage.stage().hooks.before_run)
      .await?;

    Ok(())
  }

  async fn after_run(workflow: Arc<Workflow>, issue_stage: IssueStage, state: SessionState) {
    if !matches!(state, SessionState::Cancelled)
      && let Err(error) = workflow
        .hooks()
        .after_issue_stage_run(&issue_stage, &issue_stage.stage().hooks.after_run)
        .await
    {
      tracing::error!(
        error = %error,
        "issue stage after_run hook failed",
      );
    }
  }

  async fn proxy_session_state(
    key: IssueStageKey,
    mut states: SessionStateReceiver,
    event_tx: mpsc::Sender<SessionEvent>,
  ) {
    let mut terminal = false;

    while let Some(state) = states.recv().await {
      if state.is_terminated() {
        terminal = true;
      }

      Self::send_session_event(
        &event_tx,
        SessionEvent::SessionStateUpdated {
          key: key.clone(),
          state,
        },
      )
      .await;
    }

    if terminal {
      Self::send_session_event(&event_tx, SessionEvent::SessionFinished { key }).await;
      return;
    }

    // unreachable: the sender part of `SessionStateReceiver` is owned by [`StageSessionManager`], and is only dropped when the `SessionExited` event is sent.
    // But to make sure the state got released correctly, we fake it.
    Self::send_session_event(&event_tx, SessionEvent::SessionFinished { key }).await
  }

  async fn send_session_event(sender: &mpsc::Sender<SessionEvent>, event: SessionEvent) {
    if sender.send(event).await.is_err() {
      tracing::debug!("stage session manager event receiver dropped");
    }
  }
}

/// Events emitted by the stage-session manager to signal important state changes to the orchestrator.
pub(super) enum SessionManagerEvent {
  Drained,
}

/// Events emitted internally by stage sessions to signal state changes to the manager.
enum SessionEvent {
  SessionStarted {
    key: IssueStageKey,
    commands: SessionCommandSender,
  },
  SessionStateUpdated {
    key: IssueStageKey,
    state: SessionState,
  },
  SessionFinished {
    key: IssueStageKey,
  },
}

#[derive(Debug)]
struct DispatchDecision {
  issue_stages: Vec<IssueStage>,
  skip_reason: Option<DispatchSkipReason>,
}

impl DispatchDecision {
  fn run(issue_stages: Vec<IssueStage>) -> Self {
    Self {
      issue_stages,
      skip_reason: None,
    }
  }

  fn skip(reason: DispatchSkipReason) -> Self {
    Self {
      issue_stages: Vec::new(),
      skip_reason: Some(reason),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchSkipReason {
  NoMatchingStage,
  IssueConcurrencyFull,
  MatchingStagesAlreadyActive,
}

impl DispatchSkipReason {
  fn trace(self) {
    match self {
      Self::NoMatchingStage => {
        tracing::warn!("no workflow stage matched issue state; skipping issue this cycle",);
      },
      Self::IssueConcurrencyFull => {
        tracing::info!("issue concurrency is full; skipping issue this cycle",);
      },
      Self::MatchingStagesAlreadyActive => {
        tracing::info!("matching stages are already active; skipping issue this cycle",);
      },
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

#[cfg(test)]
mod tests {
  use tracing_subscriber::{Registry, layer::SubscriberExt};

  use super::*;
  use crate::context::Issue;
  use crate::logging::tests::{CaptureLayer, captured_event, captured_message_exists};

  #[test]
  fn should_dispatch_preserves_author_order_and_respects_running_policy() {
    let workflow = Arc::new(workflow_fixture(1, None));
    let mut manager = StageSessionManager::new(Arc::clone(&workflow));

    let planned = manager.should_dispatch(Arc::new(IssueRun::new(Arc::clone(&workflow), issue("ABC-1", "todo"))));
    assert_eq!(planned.skip_reason, None);
    assert_eq!(
      planned
        .issue_stages
        .iter()
        .map(|issue_stage| issue_stage.stage_name())
        .collect::<Vec<_>>(),
      ["plan", "implement"]
    );

    let reserved = manager.reserve_issue_stages(planned.issue_stages);
    assert_eq!(reserved.len(), 2);

    let same_issue = manager.should_dispatch(Arc::new(IssueRun::new(Arc::clone(&workflow), issue("ABC-1", "todo"))));
    assert_eq!(
      same_issue.skip_reason,
      Some(DispatchSkipReason::MatchingStagesAlreadyActive),
      "same issue-stage keys are already reserved"
    );
    assert!(same_issue.issue_stages.is_empty());

    let different_issue =
      manager.should_dispatch(Arc::new(IssueRun::new(Arc::clone(&workflow), issue("ABC-2", "todo"))));
    assert_eq!(
      different_issue.skip_reason,
      Some(DispatchSkipReason::IssueConcurrencyFull),
      "different issue is blocked by max_issue_concurrency"
    );
    assert!(different_issue.issue_stages.is_empty());

    let different_issue_state_mismatch =
      manager.should_dispatch(Arc::new(IssueRun::new(Arc::clone(&workflow), issue("ABC-2", "review"))));
    assert_eq!(
      different_issue_state_mismatch.skip_reason,
      Some(DispatchSkipReason::NoMatchingStage),
      "no-match issue states stay visible when concurrency is full"
    );
    assert!(different_issue_state_mismatch.issue_stages.is_empty());

    let case_mismatch = manager.should_dispatch(Arc::new(IssueRun::new(Arc::clone(&workflow), issue("ABC-1", "Todo"))));
    assert_eq!(
      case_mismatch.skip_reason,
      Some(DispatchSkipReason::NoMatchingStage),
      "state match is exact and case-sensitive"
    );
    assert!(case_mismatch.issue_stages.is_empty());
  }

  #[tokio::test]
  async fn dispatch_skip_reason_tracing_separates_no_match_concurrency_and_active_stage() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    let _default = tracing::subscriber::set_default(subscriber);
    let workflow = Arc::new(workflow_fixture(1, None));
    let mut no_match = StageSessionManager::new(Arc::clone(&workflow));
    no_match.try_run_issue(issue("ABC-3", "review")).await;

    let workflow = Arc::new(workflow_fixture(1, None));
    let mut busy = StageSessionManager::new(Arc::clone(&workflow));
    let planned = busy.should_dispatch(Arc::new(IssueRun::new(Arc::clone(&workflow), issue("ABC-1", "todo"))));
    assert_eq!(busy.reserve_issue_stages(planned.issue_stages).len(), 2);

    busy.try_run_issue(issue("ABC-2", "todo")).await;
    busy.try_run_issue(issue("ABC-1", "todo")).await;

    let events = events.lock().expect("events mutex");
    assert!(captured_message_exists(
      &events,
      "no workflow stage matched issue state; skipping issue this cycle"
    ));
    assert!(captured_message_exists(
      &events,
      "issue concurrency is full; skipping issue this cycle"
    ));
    assert!(captured_message_exists(
      &events,
      "matching stages are already active; skipping issue this cycle"
    ));

    let no_match = captured_event(
      &events,
      "no workflow stage matched issue state; skipping issue this cycle",
    );
    dbg!(&no_match);
    assert_eq!(no_match["phase"], Phase::Dispatch.to_string());
    assert_eq!(no_match["issue_id"], "ABC-3");
    assert_eq!(no_match["issue_state"], "review");
  }

  fn workflow_fixture(max_issue_concurrency: u32, after_create: Option<&str>) -> Workflow {
    let mut builder = Workflow::builder()
      .max_issue_concurrency(max_issue_concurrency)
      .add_stage("plan", "todo", "./plan.md")
      .add_stage("implement", "todo", "./implement.md")
      .workspace_root("workspace");

    if let Some(after_create) = after_create {
      builder = builder.after_issue_workdir_create_hook(after_create);
    }

    builder.build()
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
