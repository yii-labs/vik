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
  fn should_dispatch(&self, issue_run: Arc<IssueRun>) -> Vec<IssueStage> {
    let issue = issue_run.issue();
    if !self.running.can_accept_issue(&issue.id) {
      tracing::debug!(
        phase = %Phase::Dispatch,
        issue_id = %issue.id,
        "issue concurrency full; skipping issue this cycle",
      );
      return Vec::new();
    }

    IssueRun::matching_stages(issue_run)
      .into_iter()
      .filter(|issue_stage| !self.running.contains_key(&issue_stage.key()))
      .collect()
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
    let issue_stages = self.reserve_issue_stages(self.should_dispatch(Arc::clone(&issue_run)));
    if issue_stages.is_empty() {
      tracing::warn!("issue fetch but no stage matched to run");
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

  use super::*;
  use crate::context::Issue;
  use crate::workflow::Workflow;
  use crate::workflow::loader::WorkflowSchemaLoader;

  #[test]
  fn should_dispatch_preserves_author_order_and_respects_running_policy() {
    let workflow = workflow_fixture(1, None);
    let mut orchestrator = Orchestrator::new(workflow);

    let planned = orchestrator.should_dispatch(Arc::new(IssueRun::new(
      Arc::clone(&orchestrator.workflow),
      issue("ABC-1", "todo"),
    )));
    assert_eq!(
      planned.iter().map(|issue_stage| issue_stage.stage_name()).collect::<Vec<_>>(),
      ["plan", "implement"]
    );

    let reserved = orchestrator.reserve_issue_stages(planned);
    assert_eq!(reserved.len(), 2);

    assert!(
      orchestrator
        .should_dispatch(Arc::new(IssueRun::new(
          Arc::clone(&orchestrator.workflow),
          issue("ABC-1", "todo"),
        )))
        .is_empty(),
      "same issue-stage keys are already reserved"
    );
    assert!(
      orchestrator
        .should_dispatch(Arc::new(IssueRun::new(
          Arc::clone(&orchestrator.workflow),
          issue("ABC-2", "todo"),
        )))
        .is_empty(),
      "different issue is blocked by max_issue_concurrency"
    );
    assert!(
      orchestrator
        .should_dispatch(Arc::new(IssueRun::new(
          Arc::clone(&orchestrator.workflow),
          issue("ABC-1", "Todo"),
        )))
        .is_empty(),
      "state match is exact and case-sensitive"
    );
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
    Workflow::load_from_str(&workflow_yaml(max_issue_concurrency, after_create, "workspace")).expect("load workflow")
  }

  fn workflow_fixture_with_path(
    max_issue_concurrency: u32,
    after_create: Option<&str>,
    root: &std::path::Path,
    workflow_path: std::path::PathBuf,
  ) -> Workflow {
    let root_yaml = root.to_string_lossy();
    let loaded = WorkflowSchemaLoader
      .load_from_str(
        &workflow_yaml(max_issue_concurrency, after_create, root_yaml.as_ref()),
        Some(workflow_path),
      )
      .expect("workflow schema parses");

    Workflow::try_from(loaded).expect("load workflow")
  }

  fn workflow_yaml(max_issue_concurrency: u32, after_create: Option<&str>, root_yaml: &str) -> String {
    let hook = after_create
      .map(|body| format!("  hooks:\n    after_create: {body}\n"))
      .unwrap_or_else(|| "  hooks: {}\n".to_string());

    format!(
      r#"
loop:
  max_issue_concurrency: {max_issue_concurrency}
  wait_ms: 10
workspace:
  root: '{root_yaml}'
agents:
  codex:
    runtime: codex
    model: gpt-5.5
issues:
  pull:
    command: ./issues-json
    idle_sec: 1
issue:
{hook}  stages:
    plan:
      when:
        state: todo
      agent: codex
      prompt_file: ./plan.md
    implement:
      when:
        state: todo
      agent: codex
      prompt_file: ./implement.md
"#
    )
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
