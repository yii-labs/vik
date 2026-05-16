//! Channel-driven runtime orchestrator.
//!
//! Single rule: only the main loop mutates running-stage state. Every
//! background task — intake, issue setup, stage launch, session monitor,
//! cancellation — reports through [`event`] channels and never touches
//! [`running::RunningMap`] directly. That keeps the state machine
//! single-threaded without forcing the slow paths to be.
//!
//! Flow per issue:
//!
//! 1. [`intake`] pulls the tracker, sends one `Issue` per result.
//! 2. [`Orchestrator::should_dispatch`] matches the issue against
//!    `issue.stages`, then [`reserve_issue_stages`] locks each
//!    `(issue, stage)` key in `RunningMap` *before* spawning background
//!    work — without this, duplicate intake events could re-launch a
//!    stage while its setup is still pending.
//! 3. [`prepare_issue`] runs once per matched issue (not per stage):
//!    prepares the issue run, then emits `IssueReady`.
//! 4. [`launcher`] spawns one session per stage; [`monitor`] forwards
//!    snapshots and terminal state.
mod event;
mod intake;
mod launcher;
mod monitor;
mod running;

use std::sync::Arc;

use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::context::{Issue, IssueRun, IssueRunError, IssueStage};
use crate::logging::{Phase, issue_span};
use crate::session::SessionFactory;
use crate::workflow::Workflow;

use self::event::{EventConsumer, EventProducer, IntakeEvent, OrchestratorEvent, StageEvent, event_channel};
use self::intake::IntakeLoop;
use self::launcher::StageLauncher;
use self::running::RunningMap;

#[derive(Debug, Error)]
pub enum OrchestratorError {
  #[error("orchestrator event channel closed while work was still running")]
  EventChannelClosed,
}

pub struct Orchestrator {
  workflow: Arc<Workflow>,
  launcher: StageLauncher,
  running: RunningMap,
  producer: EventProducer,
  consumer: EventConsumer,
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
          issue_state = %issue.state,
          "no workflow stage matched issue state; skipping issue this cycle",
        );
      },
      Self::IssueConcurrencyFull => {
        tracing::info!("issue concurrency is full; skipping issue this cycle");
      },
      Self::MatchingStagesAlreadyActive => {
        tracing::info!("matching stages are already active; skipping issue this cycle");
      },
    }
  }
}

impl Orchestrator {
  pub fn new(workflow: Workflow) -> Self {
    let workflow = Arc::new(workflow);
    let factory = SessionFactory::new(Arc::clone(&workflow));
    let (producer, consumer) = event_channel();

    Self {
      workflow: Arc::clone(&workflow),
      launcher: StageLauncher::new(Arc::clone(&workflow), factory.clone(), producer.clone()),
      running: RunningMap::new(workflow.schema().loop_.max_issue_concurrency as usize),
      producer,
      consumer,
    }
  }

  /// Drive intake and stage events until shutdown or natural drain.
  ///
  /// Two exit conditions, both required: intake stopped *and*
  /// `RunningMap` empty. Intake may stop first (max iterations); stages
  /// may finish first (idle tracker). Hard cancel via the shutdown token
  /// skips the drain entirely — sessions are cancelled in place and
  /// intake is aborted without waiting for the channel to drain.
  pub async fn run(&mut self, shutdown: CancellationToken) -> Result<(), OrchestratorError> {
    let intake = IntakeLoop::new(Arc::clone(&self.workflow), self.producer.clone());
    let intake_handle = intake.start(shutdown.clone());
    let mut intake_stopped = false;

    loop {
      if intake_stopped && self.running.is_empty() {
        return Ok(());
      }

      tokio::select! {
        // `biased` so shutdown wins over a queued event when both fire
        // in the same tick — otherwise a flood of snapshot events could
        // delay cancellation indefinitely.
        biased;

        _ = shutdown.cancelled() => {
          self.running.cancel_all();
          intake_handle.abort();
          return Ok(());
        }

        event = self.consumer.recv() => {
          match event {
            Some(event) => {
              if self.handle_event(event, &shutdown) {
                intake_stopped = true;
              }
            }
            None if intake_stopped || self.running.is_empty() => return Ok(()),
            None => return Err(OrchestratorError::EventChannelClosed),
          }
        }
      }
    }
  }

  /// Returns `true` when intake has stopped. The caller still waits for
  /// `RunningMap` to drain before exiting.
  fn handle_event(&mut self, event: OrchestratorEvent, shutdown: &CancellationToken) -> bool {
    match event {
      OrchestratorEvent::Intake(IntakeEvent::Issue(issue)) => {
        self.prepare_issue(issue, shutdown.clone());
        false
      },
      OrchestratorEvent::Intake(IntakeEvent::Failed(error)) => {
        tracing::error!(phase = %Phase::Intake, error = %error, "intake cycle failed");
        false
      },
      OrchestratorEvent::Intake(IntakeEvent::Stopped) => true,
      OrchestratorEvent::Stage(StageEvent::IssueReady { issue_stages }) => {
        for issue_stage in issue_stages {
          self.launcher.launch(issue_stage, shutdown.clone());
        }
        false
      },
      OrchestratorEvent::Stage(StageEvent::Started { issue_stage, session }) => {
        let key = issue_stage.key();
        self.running.start(*issue_stage, session);
        tracing::debug!(phase = %Phase::Dispatch, issue_id = %key.issue_id, stage_name = %key.stage_name, "stage session started");
        false
      },
      OrchestratorEvent::Stage(StageEvent::Snapshot { key, snapshot }) => {
        self.running.update(&key, snapshot);
        false
      },
      OrchestratorEvent::Stage(StageEvent::Terminal { key, snapshot }) => {
        self.running.finish(&key, snapshot);
        false
      },
      OrchestratorEvent::Stage(StageEvent::Failed { key, error }) => {
        self.running.fail(&key);
        tracing::error!(
          phase = %Phase::StageRun,
          issue_id = %key.issue_id,
          stage_name = %key.stage_name,
          error = %error,
          "stage launch failed",
        );
        false
      },
    }
  }

  /// Match one issue against workflow stages and current capacity.
  ///
  /// Matching is exact string equality. `Workflow::stages` is an
  /// `IndexMap`, so iteration order matches the YAML so stage launch
  /// order is deterministic. Concurrency and running-key filters happen
  /// here, before any background work — the central loop is the only
  /// place these decisions are made.
  fn should_dispatch(&self, issue_run: Arc<IssueRun>) -> DispatchDecision {
    let issue = issue_run.issue();
    if !self.running.can_accept_issue(&issue.id) {
      return DispatchDecision::skip(DispatchSkipReason::IssueConcurrencyFull);
    }

    let matching_stages = IssueRun::matching_stages(issue_run);
    if matching_stages.is_empty() {
      return DispatchDecision::skip(DispatchSkipReason::NoMatchingStage);
    }

    let issue_stages = matching_stages
      .into_iter()
      .filter(|issue_stage| !self.running.contains_key(&issue_stage.key()))
      .collect::<Vec<_>>();

    if issue_stages.is_empty() {
      return DispatchDecision::skip(DispatchSkipReason::MatchingStagesAlreadyActive);
    }

    DispatchDecision::run(issue_stages)
  }

  /// Reservation closes the race window between dispatch and session
  /// spawn: an intake cycle that returns the same issue twice cannot
  /// re-launch a stage that is already in setup.
  fn reserve_issue_stages(&mut self, issue_stages: Vec<IssueStage>) -> Vec<IssueStage> {
    issue_stages
      .into_iter()
      .filter(|issue_stage| self.running.reserve(issue_stage.clone()))
      .collect()
  }

  /// Spawn issue setup off the main loop. Setup prepares the whole matched
  /// issue (not each stage), then reports back via `IssueReady`. Spawning
  /// matters: hooks are user shell snippets that can stall, and the main
  /// loop must stay responsive.
  fn prepare_issue(&mut self, issue: Issue, shutdown: CancellationToken) {
    let _span = issue_span(&issue.id).entered();

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

    let producer = self.producer.clone();

    tokio::spawn(
      async move {
        let result: Result<(), IssueRunError> = tokio::select! {
          result = issue_run.prepare() => result,
          _ = shutdown.cancelled() => return,
        };

        match result {
          Ok(()) => producer.issue_ready(issue_stages).await,
          Err(error) => {
            tracing::error!(error = %error, "Failed to prepare for issue");
          },
        }
      }
      .in_current_span(),
    );
  }
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::time::Duration;

  use tokio::time::timeout;
  use tracing::subscriber::with_default;
  use tracing_subscriber::{Registry, layer::SubscriberExt};

  use super::*;
  use crate::context::Issue;
  use crate::logging::tests::CaptureLayer;
  use crate::workflow::Workflow;

  #[test]
  fn should_dispatch_preserves_author_order_and_respects_running_policy() {
    let workflow = workflow_fixture(1, None);
    let mut orchestrator = Orchestrator::new(workflow);

    let planned = orchestrator.should_dispatch(Arc::new(IssueRun::new(
      Arc::clone(&orchestrator.workflow),
      issue("ABC-1", "todo"),
    )));
    assert_eq!(planned.skip_reason, None);
    assert_eq!(
      planned
        .issue_stages
        .iter()
        .map(|issue_stage| issue_stage.stage_name())
        .collect::<Vec<_>>(),
      ["plan", "implement"]
    );

    let reserved = orchestrator.reserve_issue_stages(planned.issue_stages);
    assert_eq!(reserved.len(), 2);

    let same_issue = orchestrator.should_dispatch(Arc::new(IssueRun::new(
      Arc::clone(&orchestrator.workflow),
      issue("ABC-1", "todo"),
    )));
    assert_eq!(
      same_issue.skip_reason,
      Some(DispatchSkipReason::MatchingStagesAlreadyActive),
      "same issue-stage keys are already reserved"
    );
    assert!(same_issue.issue_stages.is_empty());

    let different_issue = orchestrator.should_dispatch(Arc::new(IssueRun::new(
      Arc::clone(&orchestrator.workflow),
      issue("ABC-2", "todo"),
    )));
    assert_eq!(
      different_issue.skip_reason,
      Some(DispatchSkipReason::IssueConcurrencyFull),
      "different issue is blocked by max_issue_concurrency"
    );
    assert!(different_issue.issue_stages.is_empty());

    let case_mismatch = orchestrator.should_dispatch(Arc::new(IssueRun::new(
      Arc::clone(&orchestrator.workflow),
      issue("ABC-1", "Todo"),
    )));
    assert_eq!(
      case_mismatch.skip_reason,
      Some(DispatchSkipReason::NoMatchingStage),
      "state match is exact and case-sensitive"
    );
    assert!(case_mismatch.issue_stages.is_empty());
  }

  #[test]
  fn dispatch_skip_reason_tracing_separates_no_match_concurrency_and_active_stage() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    with_default(subscriber, || {
      let mut no_match = Orchestrator::new(workflow_fixture(1, None));
      no_match.prepare_issue(issue("ABC-3", "review"), CancellationToken::new());

      let mut busy = Orchestrator::new(workflow_fixture(1, None));
      let planned = busy.should_dispatch(Arc::new(IssueRun::new(
        Arc::clone(&busy.workflow),
        issue("ABC-1", "todo"),
      )));
      assert_eq!(busy.reserve_issue_stages(planned.issue_stages).len(), 2);

      busy.prepare_issue(issue("ABC-2", "todo"), CancellationToken::new());
      busy.prepare_issue(issue("ABC-1", "todo"), CancellationToken::new());
    });

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
    assert!(!captured_message_exists(
      &events,
      "issue fetch but no stage matched to run"
    ));

    let no_match = captured_event(
      &events,
      "no workflow stage matched issue state; skipping issue this cycle",
    );
    assert_eq!(no_match["phase"], Phase::Dispatch.to_string());
    assert_eq!(no_match["issue_id"], "ABC-3");
    assert_eq!(no_match["issue_state"], "review");
  }

  #[tokio::test]
  async fn intake_issue_event_runs_issue_setup_once_before_stage_launch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let workflow_path = temp.path().join("workflow.yml");
    let workflow = workflow_fixture_with_path(10, Some("echo ok >> after_create.log"), &root, workflow_path);
    let mut orchestrator = Orchestrator::new(workflow);
    let shutdown = CancellationToken::new();

    let intake_stopped = orchestrator.handle_event(
      OrchestratorEvent::Intake(IntakeEvent::Issue(issue("ABC-1", "todo"))),
      &shutdown,
    );
    assert!(!intake_stopped);

    let event = timeout(Duration::from_secs(2), orchestrator.consumer.recv())
      .await
      .expect("issue setup event")
      .expect("event channel open");

    let OrchestratorEvent::Stage(StageEvent::IssueReady { issue_stages }) = event else {
      panic!("expected issue-ready event");
    };
    let issue_workdir = issue_stages[0].workdir().to_path_buf();

    assert_eq!(
      issue_stages
        .iter()
        .map(|issue_stage| issue_stage.stage_name())
        .collect::<Vec<_>>(),
      ["plan", "implement"]
    );
    assert_eq!(issue_workdir, orchestrator.workflow.workspace().issue_workdir("ABC-1"));
    assert_eq!(
      fs::read_to_string(issue_workdir.join("after_create.log"))
        .expect("after_create hook wrote log")
        .trim(),
      "ok"
    );
    assert!(
      !orchestrator
        .workflow
        .workspace()
        .issue_sessions_dir("ABC-1")
        .join("after-create.done")
        .exists(),
      "issue setup must not create stage/session marker files"
    );
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

  fn captured_event<'event>(events: &'event [serde_json::Value], message: &str) -> &'event serde_json::Value {
    events
      .iter()
      .find(|event| event["message"] == message)
      .unwrap_or_else(|| panic!("missing captured message: {message}"))
  }

  fn captured_message_exists(events: &[serde_json::Value], message: &str) -> bool {
    events.iter().any(|event| event["message"] == message)
  }
}
