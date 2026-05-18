//! One stage execution = one session.
//!
//! The session module owns prompt rendering, provider process spawn,
//! provider stdout decoding, JSONL writing, and the session snapshot.
//! Callers communicate through a small command channel and a state-change
//! channel. Snapshot reads are explicit commands; normal progress only
//! emits [`SessionState`] transitions.

mod factory;
mod jsonl_writer;
mod snapshot;

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

pub use crate::agent::AgentEvent;
pub use factory::*;
pub use snapshot::*;

use chrono::Utc;
use thiserror::Error;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::agent::{AgentAdapter, AgentCommand, AgentStdin, get_adapter};
use crate::config::{AgentProfileSchema, IssueStagePromptSource};
use crate::context::IssueStage;
use crate::shell::{Child, CommandExecError, CommandExt};
use crate::template::{PromptRenderer, TemplateError};

use self::jsonl_writer::JsonlWriter;

const SESSION_COMMAND_BUFFER: usize = 8;
const SESSION_STATE_BUFFER: usize = 8;

/// Errors surfaced when building or running a session.
#[derive(Debug, Error)]
pub enum SessionError {
  #[error("unknown agent profile `{profile}`")]
  ProfileNotFound { profile: String },
  #[error(transparent)]
  AgentSpawn(#[from] CommandExecError),
  #[error(transparent)]
  TemplateRender(#[from] TemplateError),
  #[error("prompt path `{0}` could not be resolved")]
  PromptPath(PathBuf),
  #[error(transparent)]
  WriteLog(#[from] std::io::Error),
}

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum SessionCommandError {
  #[error("session command channel closed")]
  Closed,
  #[error("session snapshot reply dropped")]
  SnapshotReplyDropped,
}

pub struct SessionCommandSender {
  sender: mpsc::Sender<SessionCommand>,
}

impl SessionCommandSender {
  fn new(sender: mpsc::Sender<SessionCommand>) -> Self {
    Self { sender }
  }

  pub async fn cancel(&self) -> Result<(), SessionCommandError> {
    self
      .sender
      .send(SessionCommand::Cancel)
      .await
      .map_err(|_| SessionCommandError::Closed)
  }

  #[allow(dead_code)]
  pub async fn snapshot(&self) -> Result<SessionSnapshot, SessionCommandError> {
    let (reply, result) = oneshot::channel();
    self
      .sender
      .send(SessionCommand::Snapshot { reply })
      .await
      .map_err(|_| SessionCommandError::Closed)?;
    result.await.map_err(|_| SessionCommandError::SnapshotReplyDropped)
  }
}

pub struct SessionStateReceiver {
  receiver: mpsc::Receiver<SessionState>,
}

impl SessionStateReceiver {
  fn new(receiver: mpsc::Receiver<SessionState>) -> Self {
    Self { receiver }
  }

  pub async fn recv(&mut self) -> Option<SessionState> {
    self.receiver.recv().await
  }
}

enum SessionCommand {
  Cancel,
  #[allow(dead_code)]
  Snapshot {
    reply: oneshot::Sender<SessionSnapshot>,
  },
}

pub struct Session {
  stage: IssueStage,
  profile: AgentProfileSchema,
  agent: Box<dyn AgentAdapter>,
  shutdown: CancellationToken,
  commands: mpsc::Receiver<SessionCommand>,
  states: mpsc::Sender<SessionState>,
  snapshot: SessionSnapshot,
  writer: Option<JsonlWriter>,
  child: Option<Child>,
}

impl Session {
  fn spawn(
    stage: IssueStage,
    profile: AgentProfileSchema,
    shutdown: CancellationToken,
  ) -> (SessionCommandSender, SessionStateReceiver) {
    let (command_tx, command_rx) = mpsc::channel(SESSION_COMMAND_BUFFER);
    let (state_tx, state_rx) = mpsc::channel(SESSION_STATE_BUFFER);
    let agent = get_adapter(profile.runtime);

    let task = Self {
      stage,
      profile,
      agent,
      shutdown,
      commands: command_rx,
      states: state_tx,
      snapshot: SessionSnapshot {
        started_at: Utc::now(),
        ..Default::default()
      },
      writer: None,
      child: None,
    };

    tokio::spawn(task.run().in_current_span());

    (
      SessionCommandSender::new(command_tx),
      SessionStateReceiver::new(state_rx),
    )
  }

  async fn run(mut self) {
    match self.prepare_and_spawn().await {
      Ok(true) => {},
      Ok(false) => {
        self.log_final_snapshot();
        return;
      },
      Err(error) => {
        tracing::error!(error = %error, "session start failed");
        self.set_state(SessionState::Failed).await;
        self.log_final_snapshot();
        return;
      },
    };

    self.run_started_child().await;
    self.log_final_snapshot();
  }

  async fn prepare_and_spawn(&mut self) -> Result<bool, SessionError> {
    self.set_state(SessionState::Preparing).await;
    tracing::info!("session preparing");

    if let Some(parent) = self.stage.log_file().parent() {
      fs::create_dir_all(parent).await?;
    }

    self.writer = Some(JsonlWriter::open(self.stage.log_file())?);

    let prompt = Self::render_prompt(self.stage.clone()).await?;
    let agent_command = self.agent.build_command(&self.profile, prompt);
    if self.shutdown.is_cancelled() {
      self.set_state(SessionState::Cancelled).await;
      return Ok(false);
    }

    self.spawn_child(agent_command).await?;
    if self.snapshot.state.is_terminated() {
      return Ok(false);
    }
    if self.shutdown.is_cancelled() {
      self.cancel_with_child(None).await;
      return Ok(false);
    }

    self.set_state(SessionState::Running).await;
    tracing::info!("session running");

    Ok(true)
  }

  async fn spawn_child(&mut self, agent_command: AgentCommand) -> Result<(), SessionError> {
    let mut command = Command::new(&agent_command.program);
    command
      .current_dir(self.stage.workdir())
      .args(agent_command.args)
      .stdout(Stdio::piped())
      .stderr(Stdio::null());

    match &agent_command.stdin {
      AgentStdin::None => {
        command.stdin(Stdio::null());
      },
      AgentStdin::Inherit => {},
      AgentStdin::Pipe(_) => {
        command.stdin(Stdio::piped());
      },
    }

    let mut child = command
      .timeout(/* TODO: we need timeout here */ Duration::from_hours(1))
      .spawn()?;

    child
      .stdout
      .as_ref()
      .ok_or_else(|| std::io::Error::other("Stdout was not bound to spawned agent process"))?;

    let stdin = child.stdin.take();
    self.child = Some(child);

    if let AgentStdin::Pipe(input) = agent_command.stdin
      && let Some(mut stdin) = stdin
    {
      let shutdown = self.shutdown.clone();
      tokio::select! {
        result = stdin.write_all(input.as_bytes()) => {
          result.map_err(|err| SessionError::AgentSpawn(CommandExecError::Spawn(err)))?;
        },
        _ = shutdown.cancelled() => {
          self.cancel_with_child(None).await;
        },
      }
    }

    Ok(())
  }

  async fn run_started_child(&mut self) {
    let stdout = self.child.as_mut().and_then(|child| child.stdout.take());
    let Some(stdout) = stdout else {
      self.set_state(SessionState::Failed).await;
      return;
    };

    self.stream_agent_events(stdout).await;
    self.wait_child().await;
  }

  async fn stream_agent_events(&mut self, stdout: ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();
    let shutdown = self.shutdown.clone();
    let mut commands_closed = false;

    loop {
      tokio::select! {
        _ = shutdown.cancelled() => {
          self.cancel_with_child(None).await;
          break;
        },
        command = self.commands.recv(), if !commands_closed => {
          match command {
            Some(command) => {
              let should_break = matches!(command, SessionCommand::Cancel);
              self.handle_command(command).await;
              if should_break {
                break;
              }
            },
            None => commands_closed = true,
          }
        },
        line = lines.next_line() => {
          match line {
            Ok(Some(line)) => {
              if self.snapshot.state.is_terminated() {
                continue;
              }

              let events = match serde_json::from_str(&line) {
                Ok(value) => self.agent.map_event(value),
                Err(err) => vec![AgentEvent::Error {
                  detail: err.to_string(),
                }],
              };

              for event in events {
                self.apply_event(event).await;
                if self.snapshot.state.is_terminated() {
                  break;
                }
              }
            },
            Ok(None) => break,
            Err(err) => {
              self.apply_event(AgentEvent::Error {
                detail: err.to_string(),
              }).await;
              break;
            },
          }
        },
      }
    }

    if matches!(self.snapshot.state, SessionState::Running) {
      self.set_state(SessionState::Failed).await;
    }
  }

  async fn wait_child(&mut self) {
    let Some(mut child) = self.child.take() else {
      return;
    };

    let mut commands_closed = false;
    let mut shutdown_seen = false;
    let shutdown = self.shutdown.clone();

    if shutdown.is_cancelled() {
      self.cancel_with_child(Some(&child)).await;
      shutdown_seen = true;
    }

    loop {
      tokio::select! {
        status = child.wait() => {
          match status {
            Ok(status) => {
              if !status.success() {
                tracing::warn!(status = %status, "session child exited unsuccessfully");
                if !self.snapshot.state.is_terminated() {
                  self.set_state(SessionState::Failed).await;
                }
              }
            },
            Err(error) => {
              tracing::warn!(error = %error, "session child wait failed");
              if !self.snapshot.state.is_terminated() {
                self.set_state(SessionState::Failed).await;
              }
            },
          }

          self.child = Some(child);
          return;
        },
        command = self.commands.recv(), if !commands_closed => {
          match command {
            Some(command) => self.handle_command_with_child(command, Some(&child)).await,
            None => commands_closed = true,
          }
        },
        _ = shutdown.cancelled(), if !shutdown_seen => {
          self.cancel_with_child(Some(&child)).await;
          shutdown_seen = true;
        },
      }
    }
  }

  async fn handle_command(&mut self, command: SessionCommand) {
    self.handle_command_with_child(command, None).await;
  }

  async fn handle_command_with_child(&mut self, command: SessionCommand, child: Option<&Child>) {
    match command {
      SessionCommand::Cancel => {
        self.cancel_with_child(child).await;
      },
      SessionCommand::Snapshot { reply } => {
        let _ = reply.send(self.snapshot.clone());
      },
    }
  }

  async fn cancel_with_child(&mut self, child: Option<&Child>) {
    tracing::info!("session cancelling");
    if let Some(child) = child.or(self.child.as_ref()) {
      child.cancel();
    }
    self.set_state(SessionState::Cancelled).await;
  }

  async fn apply_event(&mut self, event: AgentEvent) {
    if let Some(writer) = &mut self.writer
      && let Err(err) = writer.write(&event)
    {
      tracing::error!("session jsonl write failed: {err}");
    }

    self.snapshot.last_event_at = Some(Utc::now());

    match event {
      AgentEvent::SessionStarted { session_id } => {
        self.observe_agent_session_id(session_id);
      },
      AgentEvent::Message { text } => {
        self.snapshot.last_message = Some(text);
      },
      AgentEvent::TokenUsage {
        input,
        output,
        cache_read,
      } => {
        self.snapshot.tokens.input = self.snapshot.tokens.input.saturating_add(input);
        self.snapshot.tokens.output = self.snapshot.tokens.output.saturating_add(output);
        self.snapshot.tokens.cache_read = self.snapshot.tokens.cache_read.saturating_add(cache_read);
      },
      AgentEvent::RateLimit {
        scope,
        remaining,
        reset_at,
        observed_at,
      } => {
        let keep = match self.snapshot.rate_limits.get(&scope) {
          Some(existing) => observed_at >= existing.observed_at,
          None => true,
        };
        if keep {
          self.snapshot.rate_limits.insert(
            scope,
            RateLimitObservation {
              remaining,
              reset_at,
              observed_at,
            },
          );
        }
      },
      AgentEvent::Completed => {
        self.set_state(SessionState::Completed).await;
      },
      AgentEvent::Error { detail: _ } => {
        self.set_state(SessionState::Failed).await;
      },
    }
  }

  fn observe_agent_session_id(&mut self, session_id: String) {
    match self.snapshot.agent_session_id.as_deref() {
      Some(existing) if existing == session_id => {},
      Some(existing) => {
        tracing::warn!(
          existing_session_id = %existing,
          new_session_id = %session_id,
          "agent session id changed; keeping first value",
        );
      },
      None => {
        tracing::info!(session_id = %session_id, "agent session id observed");
        tracing::Span::current().record("session_id", session_id.as_str());
        self.snapshot.agent_session_id = Some(session_id);
      },
    }
  }

  async fn set_state(&mut self, state: SessionState) {
    if self.snapshot.state.is_terminated() || self.snapshot.state == state {
      return;
    }

    if state.is_terminated() {
      tracing::info!(state = ?state, "session terminal");
    }

    self.snapshot.state = state;
    let _ = self.states.send(state).await;
  }

  fn log_final_snapshot(&self) {
    tracing::info!(
      state = ?self.snapshot.state,
      tokens_input = self.snapshot.tokens.input,
      tokens_output = self.snapshot.tokens.output,
      tokens_cache_read = self.snapshot.tokens.cache_read,
      last_event_at = ?self.snapshot.last_event_at,
      "session finished",
    );
  }

  async fn render_prompt(stage: IssueStage) -> Result<String, SessionError> {
    let renderer = PromptRenderer::new();
    let template = match &stage.stage().prompt_source {
      IssueStagePromptSource::File(prompt_file) => {
        let prompt_file = stage
          .workflow()
          .resolve_path(prompt_file)
          .ok_or_else(|| SessionError::PromptPath(prompt_file.clone()))?;

        let mut file = File::open(&prompt_file)
          .await
          .map_err(|err| SessionError::TemplateRender(TemplateError::Io(err)))?;

        let mut template = String::new();
        file
          .read_to_string(&mut template)
          .await
          .map_err(|err| SessionError::TemplateRender(TemplateError::Io(err)))?;
        template
      },
      IssueStagePromptSource::Inline(prompt) => prompt.clone(),
    };

    Ok(renderer.render(&template, &stage).await?)
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use serde_json::Value;
  use tracing_subscriber::{Registry, layer::SubscriberExt};

  use super::*;
  use crate::agent::{AgentAdapter, AgentCommand};
  use crate::config::{AgentProfileSchema, AgentRuntime};
  use crate::context::{Issue, IssueRun};
  use crate::logging::{
    stage_span,
    tests::{CaptureLayer, captured_event, captured_message_exists},
  };
  use crate::workflow::Workflow;

  struct NoopAdapter;

  impl AgentAdapter for NoopAdapter {
    fn build_command(&self, _profile: &AgentProfileSchema, _prompt: String) -> AgentCommand {
      AgentCommand {
        program: "true".to_string(),
        args: Vec::new(),
        stdin: AgentStdin::None,
      }
    }

    fn map_event(&self, _value: Value) -> Vec<AgentEvent> {
      Vec::new()
    }
  }

  fn session_task() -> (Session, mpsc::Receiver<SessionState>) {
    let (command_tx, command_rx) = mpsc::channel(8);
    drop(command_tx);
    let (state_tx, state_rx) = mpsc::channel(8);
    let stage = issue_stage("ABC-1", "plan", "todo");

    (
      Session {
        stage,
        profile: AgentProfileSchema::new(AgentRuntime::Codex, "gpt-5.5".to_string()),
        agent: Box::new(NoopAdapter),
        shutdown: CancellationToken::new(),
        commands: command_rx,
        states: state_tx,
        snapshot: SessionSnapshot {
          started_at: Utc::now(),
          ..Default::default()
        },
        writer: None,
        child: None,
      },
      state_rx,
    )
  }

  fn matching_stage(workflow: Workflow, issue_id: &str) -> IssueStage {
    let workflow = Arc::new(workflow);
    let issue_run = Arc::new(IssueRun::new(Arc::clone(&workflow), issue(issue_id, "todo")));
    IssueRun::matching_stages(Arc::clone(&issue_run))
      .into_iter()
      .next()
      .expect("stage matches issue state")
  }

  #[tokio::test]
  async fn inline_prompt_renders_issue_variables_and_prompt_commands() {
    let prompt_command = "printf command";
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow = Workflow::builder()
      .workflow_path(temp.path().join("workflow.yml"))
      .workspace_root(temp.path().join("workspace"))
      .add_inline_stage(
        "plan",
        "todo",
        format!("plan {{{{ issue.id }}}} !`exec({prompt_command})`"),
      )
      .build();
    let stage = matching_stage(workflow, "ABC-1");

    let prompt = Session::render_prompt(stage).await.expect("prompt renders");

    assert_eq!(prompt, "plan ABC-1 command");
  }

  #[tokio::test]
  async fn prompt_file_renders_from_workflow_relative_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir(&prompts_dir).expect("prompts dir");
    std::fs::write(prompts_dir.join("plan.md"), "file {{ issue.id }}").expect("prompt file");
    let workflow = Workflow::builder()
      .workflow_path(temp.path().join("workflow.yml"))
      .workspace_root(temp.path().join("workspace"))
      .add_stage("plan", "todo", "./prompts/plan.md")
      .build();
    let stage = matching_stage(workflow, "ABC-1");

    let prompt = Session::render_prompt(stage).await.expect("prompt renders");

    assert_eq!(prompt, "file ABC-1");
  }

  #[tokio::test]
  async fn token_usage_events_accumulate_without_overflow() {
    let (mut task, _states) = session_task();

    task
      .apply_event(AgentEvent::TokenUsage {
        input: u64::MAX - 1,
        output: 10,
        cache_read: 20,
      })
      .await;
    task
      .apply_event(AgentEvent::TokenUsage {
        input: 10,
        output: u64::MAX,
        cache_read: 30,
      })
      .await;

    assert_eq!(task.snapshot.tokens.input, u64::MAX);
    assert_eq!(task.snapshot.tokens.output, u64::MAX);
    assert_eq!(task.snapshot.tokens.cache_read, 50);
  }

  #[tokio::test]
  async fn rate_limit_observation_keeps_latest_event_per_scope() {
    let (mut task, _states) = session_task();
    let reset_at = "2026-05-16T10:15:30Z".parse().expect("test timestamp parses");
    let stale = "2026-05-16T10:00:00Z".parse().expect("test timestamp parses");
    let fresh = "2026-05-16T10:05:00Z".parse().expect("test timestamp parses");

    task
      .apply_event(AgentEvent::RateLimit {
        scope: "codex:tokens_per_min".into(),
        remaining: 50,
        reset_at,
        observed_at: fresh,
      })
      .await;
    task
      .apply_event(AgentEvent::RateLimit {
        scope: "codex:tokens_per_min".into(),
        remaining: 10,
        reset_at,
        observed_at: stale,
      })
      .await;

    let observation = task
      .snapshot
      .rate_limits
      .get("codex:tokens_per_min")
      .expect("rate limit observation stored");
    assert_eq!(observation.remaining, 50);
    assert_eq!(observation.observed_at, fresh);
  }

  #[tokio::test]
  async fn set_state_emits_terminal_log_on_terminal_transition() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    let _default = tracing::subscriber::set_default(subscriber);
    let (mut task, _states) = session_task();
    task.set_state(SessionState::Running).await;
    task.set_state(SessionState::Completed).await;

    let events = events.lock().expect("events mutex");
    assert!(captured_message_exists(&events, "session terminal"));
    let event = captured_event(&events, "session terminal");
    assert_eq!(event["state"], "Completed");
  }

  #[tokio::test]
  async fn apply_event_emits_agent_session_id_log() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    let _default = tracing::subscriber::set_default(subscriber);
    let (mut task, _states) = session_task();
    task
      .apply_event(AgentEvent::SessionStarted {
        session_id: "sess-123".into(),
      })
      .await;

    let events = events.lock().expect("events mutex");
    let event = captured_event(&events, "agent session id observed");
    assert_eq!(event["session_id"], "sess-123");
  }

  #[tokio::test]
  async fn session_started_records_current_stage_span_once() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    let _default = tracing::subscriber::set_default(subscriber);
    let span = stage_span("86", "implement", "codex");
    let _entered = span.enter();
    let (mut task, _states) = session_task();

    task
      .apply_event(AgentEvent::SessionStarted {
        session_id: "019e35bf-2163-7c32-af3c-7728a92c94f7".into(),
      })
      .await;
    task
      .apply_event(AgentEvent::SessionStarted {
        session_id: "019e35bf-2163-7c32-af3c-7728a92c94f7".into(),
      })
      .await;
    tracing::info!("stage finished");

    assert_eq!(
      task.snapshot.agent_session_id.as_deref(),
      Some("019e35bf-2163-7c32-af3c-7728a92c94f7")
    );

    let events = events.lock().expect("events mutex");
    let observed_session_logs = events
      .iter()
      .filter(|event| event["message"] == "agent session id observed")
      .collect::<Vec<_>>();
    assert_eq!(observed_session_logs.len(), 1);
    assert_eq!(
      observed_session_logs[0]["session_id"],
      "019e35bf-2163-7c32-af3c-7728a92c94f7"
    );

    let stage_finished = captured_event(&events, "stage finished");
    assert_eq!(stage_finished["session_id"], "019e35bf-2163-7c32-af3c-7728a92c94f7");
  }

  #[tokio::test]
  async fn session_command_sender_hides_command_construction() {
    let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(8);
    let commands = SessionCommandSender::new(command_tx);
    let snapshot = SessionSnapshot {
      state: SessionState::Running,
      ..Default::default()
    };

    let snapshot_task = tokio::spawn(async move {
      match command_rx.recv().await.expect("snapshot command") {
        SessionCommand::Snapshot { reply } => reply.send(snapshot).expect("snapshot receiver waits"),
        SessionCommand::Cancel => panic!("expected snapshot command"),
      }

      match command_rx.recv().await.expect("cancel command") {
        SessionCommand::Cancel => {},
        SessionCommand::Snapshot { .. } => panic!("expected cancel command"),
      }
    });

    let returned = commands.snapshot().await.expect("snapshot succeeds");
    assert_eq!(returned.state, SessionState::Running);
    commands.cancel().await.expect("cancel succeeds");
    snapshot_task.await.expect("command task joins");
  }

  #[tokio::test]
  async fn session_state_receiver_receives_only_sent_state_changes() {
    let (state_tx, state_rx) = tokio::sync::mpsc::channel(8);
    let mut states = SessionStateReceiver::new(state_rx);

    state_tx.send(SessionState::Preparing).await.expect("send preparing");
    state_tx.send(SessionState::Running).await.expect("send running");
    drop(state_tx);

    assert_eq!(states.recv().await, Some(SessionState::Preparing));
    assert_eq!(states.recv().await, Some(SessionState::Running));
    assert_eq!(states.recv().await, None);
  }

  fn issue_stage(issue_id: &str, stage_name: &str, state: &str) -> IssueStage {
    let workflow = Arc::new(
      Workflow::builder()
        .add_stage(stage_name, state, format!("./{stage_name}.md"))
        .build(),
    );
    let issue_run = Arc::new(IssueRun::new(Arc::clone(&workflow), issue(issue_id, state)));
    let schema = workflow
      .stages()
      .find(|stage| stage.name == stage_name)
      .expect("stage fixture exists")
      .clone();

    IssueStage::new(issue_run, schema)
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
