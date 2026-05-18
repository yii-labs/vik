//! Stage-session manager.
//!
//! This is the bridge between tracker intake and session runtime. It owns
//! stage matching, issue preparation, hook execution, session command
//! senders, and drain signalling. The top-level orchestrator only passes
//! issues in and waits for the manager to become empty.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::context::{Issue, IssueRun, IssueRunError, IssueStage, IssueStageKey};
use crate::hooks::HookError;
use crate::logging::{Phase, issue_span, stage_span};
use crate::session::{SessionCommandSender, SessionError, SessionFactory, SessionState, SessionStateReceiver};
use crate::workflow::Workflow;

const MANAGER_EVENT_BUFFER: usize = 256;

pub(super) struct StageSessionManager {
  workflow: Arc<Workflow>,
  factory: SessionFactory,
  sessions: HashMap<IssueStageKey, Option<SessionCommandSender>>,
  stages: HashMap<IssueStageKey, IssueStage>,
  finishing: HashSet<IssueStageKey>,
  events: mpsc::Receiver<ManagerEvent>,
  event_tx: mpsc::Sender<ManagerEvent>,
  pending_events: VecDeque<ManagerEvent>,
  shutdown: CancellationToken,
}

impl StageSessionManager {
  pub(super) fn new(workflow: Arc<Workflow>) -> Self {
    let (event_tx, events) = mpsc::channel(MANAGER_EVENT_BUFFER);

    Self {
      factory: SessionFactory::new(Arc::clone(&workflow)),
      workflow,
      sessions: HashMap::new(),
      stages: HashMap::new(),
      finishing: HashSet::new(),
      events,
      event_tx,
      pending_events: VecDeque::new(),
      shutdown: CancellationToken::new(),
    }
  }

  pub(super) fn is_empty(&self) -> bool {
    self.sessions.is_empty()
  }

  pub(super) async fn try_spawn(&mut self, issue: Issue) {
    let _span = issue_span(&issue.id).entered();

    if self.shutdown.is_cancelled() {
      tracing::info!("stage session manager is shutting down; skipping issue this cycle");
      return;
    }

    let issue_run = Arc::new(IssueRun::new(Arc::clone(&self.workflow), issue));
    let dispatch = self.should_dispatch(Arc::clone(&issue_run));
    if let Some(reason) = dispatch.skip_reason {
      reason.trace(issue_run.issue());
      return;
    }

    let issue_stages = self.reserve_issue_stages(dispatch.issue_stages);
    if issue_stages.is_empty() {
      DispatchSkipReason::MatchingStagesAlreadyActive.trace(issue_run.issue());
      return;
    }

    let event_tx = self.event_tx.clone();
    let shutdown = self.shutdown.clone();
    let keys = issue_stages.iter().map(IssueStage::key).collect::<Vec<_>>();

    tokio::spawn(
      async move {
        let result: Result<(), IssueRunError> = tokio::select! {
          result = issue_run.prepare() => result,
          _ = shutdown.cancelled() => return,
        };

        match result {
          Ok(()) => Self::send_manager_event(&event_tx, ManagerEvent::IssueReady { issue_stages }).await,
          Err(error) => {
            Self::send_manager_event(
              &event_tx,
              ManagerEvent::IssuePrepareFailed {
                keys,
                error: error.to_string(),
              },
            )
            .await;
          },
        }
      }
      .in_current_span(),
    );
  }

  pub(super) async fn recv(&mut self) -> Option<()> {
    let event = self.events.recv().await?;
    self.pending_events.push_back(event);
    Some(())
  }

  pub(super) async fn handle_received_event(&mut self) -> Option<StageSessionEvent> {
    let event = self.pending_events.pop_front()?;
    let was_empty = self.is_empty();
    self.handle_event(event).await;

    if !was_empty && self.is_empty() {
      Some(StageSessionEvent::Drained)
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
      if let Some(issue_stage) = self.stages.get(key) {
        let span = stage_span(
          &issue_stage.issue().id,
          issue_stage.stage_name(),
          &issue_stage.stage().agent,
        );
        if let Err(error) = commands.cancel().instrument(span).await {
          tracing::debug!(error = %error, "session cancel command failed");
        }
      }
    }
  }

  async fn handle_event(&mut self, event: ManagerEvent) {
    match event {
      ManagerEvent::IssueReady { issue_stages } => {
        self.launch_issue_stages(issue_stages);
      },
      ManagerEvent::IssuePrepareFailed { keys, error } => {
        for key in keys {
          self.sessions.remove(&key);
          self.stages.remove(&key);
          self.finishing.remove(&key);
        }
        tracing::error!(error = %error, "Failed to prepare for issue");
      },
      ManagerEvent::SessionStarted { key, commands } => {
        if let Some(session) = self.sessions.get_mut(&key) {
          *session = Some(commands);
          tracing::info!(
            phase = %Phase::Dispatch,
            issue_id = %key.issue_id,
            stage_name = %key.stage_name,
            "stage session started",
          );
        }
      },
      ManagerEvent::SessionState { key, state } => {
        if state.is_terminated() {
          self.finish_stage(key, state);
        }
      },
      ManagerEvent::SessionClosed { key } => {
        self.finish_stage(key, SessionState::Failed);
      },
      ManagerEvent::StageFinished { key } => {
        self.sessions.remove(&key);
        self.stages.remove(&key);
        self.finishing.remove(&key);
      },
      ManagerEvent::StageFailed { key, error } => {
        self.sessions.remove(&key);
        self.stages.remove(&key);
        self.finishing.remove(&key);
        tracing::error!(
          phase = %Phase::StageRun,
          issue_id = %key.issue_id,
          stage_name = %key.stage_name,
          error = %error,
          "stage launch failed",
        );
      },
    }
  }

  fn launch_issue_stages(&self, issue_stages: Vec<IssueStage>) {
    if let Some(first) = issue_stages.first() {
      let stage_names: Vec<&str> = issue_stages.iter().map(|s| s.stage_name()).collect();
      tracing::info!(
        phase = %Phase::Dispatch,
        issue_id = %first.issue().id,
        stage_names = ?stage_names,
        "issue ready; launching stages",
      );
    }

    for issue_stage in issue_stages {
      self.launch_issue_stage(issue_stage);
    }
  }

  fn launch_issue_stage(&self, issue_stage: IssueStage) {
    let span = stage_span(
      &issue_stage.issue().id,
      issue_stage.stage_name(),
      &issue_stage.stage().agent,
    );
    let workflow = Arc::clone(&self.workflow);
    let factory = self.factory.clone();
    let event_tx = self.event_tx.clone();
    let shutdown = self.shutdown.clone();

    tokio::spawn(
      async move {
        let key = issue_stage.key();
        if shutdown.is_cancelled() {
          return;
        }

        tracing::info!("stage launching");

        if let Err(error) = Self::before_run(&workflow, &issue_stage).await {
          Self::send_manager_event(
            &event_tx,
            ManagerEvent::StageFailed {
              key,
              error: error.to_string(),
            },
          )
          .await;
          return;
        }

        let (commands, states) = match factory.spawn_stage(issue_stage.clone(), shutdown.clone()) {
          Ok(session) => session,
          Err(error) => {
            Self::send_manager_event(
              &event_tx,
              ManagerEvent::StageFailed {
                key,
                error: error.to_string(),
              },
            )
            .await;
            return;
          },
        };

        Self::send_manager_event(
          &event_tx,
          ManagerEvent::SessionStarted {
            key: key.clone(),
            commands,
          },
        )
        .await;

        tokio::spawn(Self::watch_session_state_receiver(key, states, event_tx).in_current_span());
      }
      .instrument(span),
    );
  }

  fn finish_stage(&mut self, key: IssueStageKey, state: SessionState) {
    let Some(issue_stage) = self.stages.get(&key).cloned() else {
      return;
    };

    if !self.finishing.insert(key.clone()) {
      return;
    }

    let workflow = Arc::clone(&self.workflow);
    let event_tx = self.event_tx.clone();

    tokio::spawn(
      async move {
        Self::after_run_and_log(workflow, issue_stage, state).await;
        Self::send_manager_event(&event_tx, ManagerEvent::StageFinished { key }).await;
      }
      .in_current_span(),
    );
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

  async fn after_run_and_log(workflow: Arc<Workflow>, issue_stage: IssueStage, state: SessionState) {
    let span = stage_span(
      &issue_stage.issue().id,
      issue_stage.stage_name(),
      &issue_stage.stage().agent,
    );

    async move {
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

      tracing::info!("stage session exited");
    }
    .instrument(span)
    .await;
  }

  async fn watch_session_state_receiver(
    key: IssueStageKey,
    mut states: SessionStateReceiver,
    event_tx: mpsc::Sender<ManagerEvent>,
  ) {
    let mut terminal = false;

    while let Some(state) = states.recv().await {
      if state.is_terminated() {
        terminal = true;
      }

      Self::send_manager_event(
        &event_tx,
        ManagerEvent::SessionState {
          key: key.clone(),
          state,
        },
      )
      .await;

      if terminal {
        return;
      }
    }

    if !terminal {
      Self::send_manager_event(&event_tx, ManagerEvent::SessionClosed { key }).await;
    }
  }

  async fn send_manager_event(sender: &mpsc::Sender<ManagerEvent>, event: ManagerEvent) {
    if sender.send(event).await.is_err() {
      tracing::debug!(phase = %Phase::Dispatch, "stage session manager event receiver dropped");
    }
  }
}

pub(super) enum StageSessionEvent {
  Drained,
}

enum ManagerEvent {
  IssueReady {
    issue_stages: Vec<IssueStage>,
  },
  IssuePrepareFailed {
    keys: Vec<IssueStageKey>,
    error: String,
  },
  SessionStarted {
    key: IssueStageKey,
    commands: SessionCommandSender,
  },
  SessionState {
    key: IssueStageKey,
    state: SessionState,
  },
  SessionClosed {
    key: IssueStageKey,
  },
  StageFinished {
    key: IssueStageKey,
  },
  StageFailed {
    key: IssueStageKey,
    error: String,
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
  fn trace(self, issue: &Issue) {
    match self {
      Self::NoMatchingStage => {
        tracing::warn!(
          phase = %Phase::Dispatch,
          issue_id = %issue.id,
          issue_state = %issue.state,
          "no workflow stage matched issue state; skipping issue this cycle",
        );
      },
      Self::IssueConcurrencyFull => {
        tracing::info!(
          phase = %Phase::Dispatch,
          issue_id = %issue.id,
          "issue concurrency is full; skipping issue this cycle",
        );
      },
      Self::MatchingStagesAlreadyActive => {
        tracing::info!(
          phase = %Phase::Dispatch,
          issue_id = %issue.id,
          "matching stages are already active; skipping issue this cycle",
        );
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
  use std::fs;
  use std::time::Duration;

  use tokio::time::timeout;
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
  async fn try_spawn_runs_issue_setup_before_session_launch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let workflow_path = temp.path().join("workflow.yml");
    let workflow = Arc::new(workflow_fixture_with_path(
      10,
      Some("echo ok >> after_create.log"),
      &root,
      workflow_path,
    ));
    let mut manager = StageSessionManager::new(Arc::clone(&workflow));

    manager.try_spawn(issue("ABC-1", "todo")).await;

    timeout(Duration::from_secs(2), recv_until_drained(&mut manager))
      .await
      .expect("manager drains")
      .expect("drained event");

    let issue_workdir = workflow.workspace().issue_workdir("ABC-1");
    assert_eq!(
      fs::read_to_string(issue_workdir.join("after_create.log"))
        .expect("after_create hook wrote log")
        .trim(),
      "ok"
    );
  }

  #[tokio::test]
  async fn dispatch_skip_reason_tracing_separates_no_match_concurrency_and_active_stage() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    let _default = tracing::subscriber::set_default(subscriber);
    let workflow = Arc::new(workflow_fixture(1, None));
    let mut no_match = StageSessionManager::new(Arc::clone(&workflow));
    no_match.try_spawn(issue("ABC-3", "review")).await;

    let workflow = Arc::new(workflow_fixture(1, None));
    let mut busy = StageSessionManager::new(Arc::clone(&workflow));
    let planned = busy.should_dispatch(Arc::new(IssueRun::new(Arc::clone(&workflow), issue("ABC-1", "todo"))));
    assert_eq!(busy.reserve_issue_stages(planned.issue_stages).len(), 2);

    busy.try_spawn(issue("ABC-2", "todo")).await;
    busy.try_spawn(issue("ABC-1", "todo")).await;

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

  fn workflow_fixture_with_path(
    max_issue_concurrency: u32,
    after_create: Option<&str>,
    root: &std::path::Path,
    workflow_path: std::path::PathBuf,
  ) -> Workflow {
    let mut builder = Workflow::builder()
      .max_issue_concurrency(max_issue_concurrency)
      .add_stage("plan", "todo", "./plan.md")
      .add_stage("implement", "todo", "./implement.md")
      .workspace_root(root)
      .workflow_path(workflow_path);

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

  async fn recv_until_drained(manager: &mut StageSessionManager) -> Option<StageSessionEvent> {
    loop {
      manager.recv().await?;
      if let Some(event) = manager.handle_received_event().await {
        return Some(event);
      }
    }
  }
}
