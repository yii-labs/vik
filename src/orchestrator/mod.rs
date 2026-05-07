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
//!    creates the workdir, runs `after_create`, emits `IssueReady`.
//! 4. [`launcher`] spawns one session per stage; [`monitor`] forwards
//!    snapshots and terminal state.
mod event;
mod intake;
mod launcher;
mod monitor;
mod running;
mod types;

use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::context::Issue;
use crate::hooks::HookError;
use crate::logging::{Phase, dispatch_span};
use crate::session::SessionFactory;
use crate::workflow::Workflow;

use self::event::{EventConsumer, EventProducer, IntakeEvent, OrchestratorEvent, StageEvent, event_channel};
use self::intake::IntakeLoop;
use self::launcher::StageLauncher;
use self::running::RunningMap;
use self::types::IssueStage;

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
        let _entered = dispatch_span().entered();
        let issue_stages = self.reserve_issue_stages(self.should_dispatch(issue));
        if !issue_stages.is_empty() {
          self.prepare_issue(issue_stages, shutdown.clone());
        }
        false
      },
      OrchestratorEvent::Intake(IntakeEvent::Failed(error)) => {
        tracing::error!(phase = %Phase::Intake, error = %error, "intake cycle failed");
        false
      },
      OrchestratorEvent::Intake(IntakeEvent::Stopped) => true,
      OrchestratorEvent::Stage(StageEvent::IssueReady {
        issue_stages,
        issue_workdir,
      }) => {
        for issue_stage in issue_stages {
          self.launcher.launch(issue_stage, issue_workdir.clone(), shutdown.clone());
        }
        false
      },
      OrchestratorEvent::Stage(StageEvent::Started { issue_stage, session }) => {
        let key = issue_stage.key();
        self.running.start(*issue_stage, session);
        tracing::debug!(phase = %Phase::Dispatch, issue_identifier = %key.issue_id, stage_name = %key.stage_name, "stage session started");
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
          issue_identifier = %key.issue_id,
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
  fn should_dispatch(&self, issue: Issue) -> Vec<IssueStage> {
    if !self.running.can_accept_issue(&issue.id) {
      tracing::debug!(
        phase = %Phase::Dispatch,
        issue_identifier = %issue.id,
        "issue concurrency full; skipping issue this cycle",
      );
      return Vec::new();
    }

    self
      .workflow
      .stages()
      .iter()
      .filter(|(_, stage)| stage.when.state == issue.state)
      .map(|(name, stage)| IssueStage::new(issue.clone(), name.clone(), stage.clone()))
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

  /// Spawn issue setup off the main loop. Setup runs `after_create` once
  /// for the whole matched issue (not per stage), then reports back via
  /// `IssueReady`. Spawning matters: hooks are user shell snippets that
  /// can stall, and the main loop must stay responsive.
  fn prepare_issue(&self, issue_stages: Vec<IssueStage>, shutdown: CancellationToken) {
    let workflow = Arc::clone(&self.workflow);
    let producer = self.producer.clone();
    let issue = issue_stages[0].issue().clone();
    let span = tracing::info_span!(
      "issue_setup",
      phase = Phase::Dispatch.as_str(),
      issue_identifier = %issue.id,
    );

    tokio::spawn(
      async move {
        let result = tokio::select! {
          result = prepare_issue_workdir(Arc::clone(&workflow), &issue) => result,
          _ = shutdown.cancelled() => return,
        };

        match result {
          Ok(issue_workdir) => producer.issue_ready(issue_stages, issue_workdir).await,
          Err(error) => {
            for issue_stage in issue_stages {
              producer.stage_failed(issue_stage.key(), error.to_string()).await;
            }
          },
        }
      }
      .instrument(span),
    );
  }
}

/// No completion marker by design. The tracker is the source of truth;
/// if setup fails partway, the next intake cycle will see the issue
/// still in scope and retry. A `.done` file would silently mask real
/// failures.
async fn prepare_issue_workdir(workflow: Arc<Workflow>, issue: &Issue) -> Result<PathBuf, IssueSetupError> {
  let issue_workdir = workflow.workspace().issue_workdir(&issue.id);

  match tokio::fs::metadata(&issue_workdir).await {
    Ok(metadata) if metadata.is_dir() => {
      tracing::debug!(path = %issue_workdir.display(), "issue workspace already exists; skipping creation");
      return Ok(issue_workdir);
    },
    Ok(_) => {
      return Err(IssueSetupError::CreateWorkspace {
        path: issue_workdir.clone(),
        source: std::io::Error::other("path exists but is not a directory"),
      });
    },
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
      // Expected case: workspace doesn't exist yet, will be created below.
    },
    Err(e) => {
      return Err(IssueSetupError::CreateWorkspace {
        path: issue_workdir.clone(),
        source: e,
      });
    },
  };

  tokio::fs::create_dir_all(&issue_workdir)
    .await
    .map_err(|source| IssueSetupError::CreateWorkspace {
      path: issue_workdir.clone(),
      source,
    })?;

  workflow
    .hooks()
    .run_after_create(&workflow.schema().issue.hooks, issue, &issue_workdir)
    .await?;

  Ok(issue_workdir)
}

#[derive(Debug, Error)]
enum IssueSetupError {
  #[error("failed to create issue workspace `{path}`: {source}")]
  CreateWorkspace {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
  #[error(transparent)]
  Hook(#[from] HookError),
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::time::Duration;

  use tokio::time::timeout;

  use super::*;
  use crate::workflow::Workflow;
  use crate::workflow::loader::WorkflowSchemaLoader;

  #[test]
  fn should_dispatch_preserves_author_order_and_respects_running_policy() {
    let workflow = workflow_fixture(1, None);
    let mut orchestrator = Orchestrator::new(workflow);

    let planned = orchestrator.should_dispatch(issue("ABC-1", "todo"));
    assert_eq!(
      planned.iter().map(|issue_stage| issue_stage.stage().name()).collect::<Vec<_>>(),
      ["plan", "implement"]
    );

    let reserved = orchestrator.reserve_issue_stages(planned);
    assert_eq!(reserved.len(), 2);

    assert!(
      orchestrator.should_dispatch(issue("ABC-1", "todo")).is_empty(),
      "same issue-stage keys are already reserved"
    );
    assert!(
      orchestrator.should_dispatch(issue("ABC-2", "todo")).is_empty(),
      "different issue is blocked by max_issue_concurrency"
    );
    assert!(
      orchestrator.should_dispatch(issue("ABC-1", "Todo")).is_empty(),
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

    let OrchestratorEvent::Stage(StageEvent::IssueReady {
      issue_stages,
      issue_workdir,
    }) = event
    else {
      panic!("expected issue-ready event");
    };

    assert_eq!(
      issue_stages
        .iter()
        .map(|issue_stage| issue_stage.stage().name())
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
