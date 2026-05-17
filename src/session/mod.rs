//! One stage execution = one session.
//!
//! [`Session`] is a cloneable handle around a spawned agent subprocess.
//! It owns the prompt-render → spawn → stream-events → terminate
//! pipeline and exposes:
//!
//! - [`Session::snapshot`] — point-in-time copy of state, tokens, rate
//!   limits (cheap, lock-released before return).
//! - [`Session::subscribe_state`] / [`Session::wait`] — `watch::Receiver`
//!   for state changes; the watch is the only synchronization with the
//!   internal `Mutex`-guarded snapshot.
//! - [`Session::cancel`] — kill the child and force `Cancelled` (no-op
//!   if already terminal).
//!
//! Decoded provider events append to session JSONL as the durable
//! record. Semantic events also accumulate into the snapshot;
//! observation-only events stay JSONL-only.
#![allow(dead_code)]
mod factory;
mod jsonl_writer;
mod snapshot;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use crate::agent::AgentEvent;
pub use factory::*;
pub use snapshot::*;

use chrono::Utc;
use thiserror::Error;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdout, Command};
use tokio::sync::watch;
use tracing::Instrument;

use crate::agent::{AgentAdapter, AgentCommand, AgentStdin, get_adapter};
use crate::config::AgentProfileSchema;
use crate::context::{Issue, IssueStage};
use crate::shell::{Child, CommandExecError, CommandExt};
use crate::template::{PromptRenderer, TemplateError};

use self::jsonl_writer::JsonlWriter;

/// Errors surfaced when building a session.
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

#[derive(Clone)]
pub struct Session {
  stage: IssueStage,
  profile: AgentProfileSchema,
  agent: Arc<dyn AgentAdapter>,
  inner: Arc<Mutex<SessionInner>>,
  state_notifier: watch::Sender<SessionState>,
}

struct SessionInner {
  snapshot: SessionSnapshot,
  writer: Option<JsonlWriter>,
  child: Option<Child>,
}

impl Session {
  pub(super) async fn spawn(stage: IssueStage, profile: AgentProfileSchema) -> Result<Self, SessionError> {
    let now = Utc::now();

    let snapshot = SessionSnapshot {
      started_at: now,
      ..Default::default()
    };

    let agent = get_adapter(profile.runtime);

    let (state_notifier, _) = watch::channel(snapshot.state);

    let session = Self {
      stage,
      profile,
      agent,
      inner: Arc::new(Mutex::new(SessionInner {
        snapshot,
        writer: None,
        child: None,
      })),
      state_notifier,
    };

    let child = session.spawn_inner().await?;
    session.inner.lock().expect("session mutex never poisoned").child = Some(child);

    Ok(session)
  }

  pub fn id(&self) -> &str {
    &self.stage.issue().id
  }

  pub fn issue(&self) -> &Issue {
    self.stage.issue()
  }

  pub fn stage(&self) -> &IssueStage {
    &self.stage
  }

  pub fn snapshot(&self) -> SessionSnapshot {
    self.inner.lock().expect("session mutex never poisoned").snapshot.clone()
  }

  pub fn state(&self) -> SessionState {
    self.inner.lock().expect("session mutex never poisoned").snapshot.state
  }

  pub fn log_file(&self) -> &Path {
    self.stage.log_file()
  }

  pub fn terminated(&self) -> bool {
    self.state().is_terminated()
  }

  pub fn subscribe_state(&self) -> watch::Receiver<SessionState> {
    self.state_notifier.subscribe()
  }

  pub fn cancel(&self) {
    let mut inner = self.inner.lock().expect("session mutex never poisoned");
    if let Some(child) = &inner.child {
      tracing::info!("session cancelling");
      child.cancel();
      inner.set_state(SessionState::Cancelled, &self.state_notifier);
    }
  }

  pub async fn wait(&self) -> SessionSnapshot {
    let mut state_rx = self.state_notifier.subscribe();

    loop {
      if self.terminated() {
        return self.snapshot();
      }

      if state_rx.changed().await.is_err() {
        return self.snapshot();
      }
    }
  }

  async fn spawn_inner(&self) -> Result<Child, SessionError> {
    let agent_command = self.prepare().await?;

    self.set_state(SessionState::Running);

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

    tracing::info!("session running");

    // For `Pipe`, write the prompt and drop the handle so the child
    // sees EOF — Codex blocks on stdin until that happens.
    if let AgentStdin::Pipe(input) = agent_command.stdin
      && let Some(mut stdin) = child.stdin.take()
    {
      stdin
        .write_all(input.as_bytes())
        .await
        .map_err(|err| SessionError::AgentSpawn(CommandExecError::Spawn(err)))?;
    }

    let stdout = child
      .stdout
      .take()
      .ok_or_else(|| std::io::Error::other("Stdout was not bound to spawned agent process"))?;

    self.stream_agent_output(stdout)?;

    Ok(child)
  }

  async fn prepare(&self) -> Result<AgentCommand, SessionError> {
    self.set_state(SessionState::Preparing);
    tracing::info!("session preparing");

    if let Some(parent) = self.log_file().parent() {
      fs::create_dir_all(parent).await?;
    }

    let prompt = self.render_prompt().await?;

    Ok(self.agent.build_command(&self.profile, prompt))
  }

  async fn render_prompt(&self) -> Result<String, SessionError> {
    let renderer = PromptRenderer::new();
    let prompt_file = self
      .stage
      .workflow()
      .resolve_path(&self.stage().stage().prompt_file)
      .ok_or_else(|| SessionError::PromptPath(self.stage().stage().prompt_file.clone()))?;

    let mut file = File::open(&prompt_file)
      .await
      .map_err(|err| SessionError::TemplateRender(TemplateError::Io(err)))?;

    let mut template = String::new();
    file
      .read_to_string(&mut template)
      .await
      .map_err(|err| SessionError::TemplateRender(TemplateError::Io(err)))?;

    Ok(renderer.render(&template, &self.stage).await?)
  }

  fn set_state(&self, state: SessionState) {
    self
      .inner
      .lock()
      .expect("session mutex never poisoned")
      .set_state(state, &self.state_notifier);
  }

  fn stream_agent_output(&self, stdout: ChildStdout) -> Result<(), SessionError> {
    let writer = JsonlWriter::open(self.log_file())?;

    self.inner.lock().expect("session mutex never poisoned").writer = Some(writer);

    let agent = self.agent.clone();
    let inner = Arc::clone(&self.inner);
    let state_notifier = self.state_notifier.clone();

    // Inherit the stage span so SessionStarted / terminal logs flatten
    // `phase`, `issue_id`, and `stage_name` onto each event.
    tokio::spawn(stream_agent_events(stdout, agent, inner, state_notifier).in_current_span());

    Ok(())
  }
}

impl SessionInner {
  fn finish_output_stream(&mut self, state_notifier: &watch::Sender<SessionState>) {
    if matches!(self.snapshot.state, SessionState::Running) {
      self.set_state(SessionState::Failed, state_notifier);
    }
  }

  fn apply_event(&mut self, event: AgentEvent, state_notifier: &watch::Sender<SessionState>) {
    if let Some(writer) = &mut self.writer
      && let Err(err) = writer.write(&event)
    {
      tracing::error!("session jsonl write failed: {err}");
    }

    match event {
      AgentEvent::SessionStarted { session_id } => {
        self.snapshot.last_event_at = Some(Utc::now());
        tracing::info!(session_id = %session_id, "agent session id observed");
        self.snapshot.agent_session_id = Some(session_id);
      },
      AgentEvent::Message { text } => {
        self.snapshot.last_event_at = Some(Utc::now());
        self.snapshot.last_message = Some(text);
      },
      AgentEvent::TokenUsage {
        input,
        output,
        cache_read,
      } => {
        self.snapshot.last_event_at = Some(Utc::now());
        // saturating_add tolerates duplicated TokenUsage events
        // without panicking; providers occasionally report twice on
        // retry and the totals only grow.
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
        self.snapshot.last_event_at = Some(Utc::now());
        // Latest-wins by observation time: a provider retry can land a
        // stale observation after a newer one, and we want to keep the
        // most recent ground truth.
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
        self.snapshot.last_event_at = Some(Utc::now());
        self.set_state(SessionState::Completed, state_notifier);
      },
      AgentEvent::Error { detail: _ } => {
        self.snapshot.last_event_at = Some(Utc::now());
        self.set_state(SessionState::Failed, state_notifier);
      },
      AgentEvent::ToolCall { .. } | AgentEvent::Subagent { .. } | AgentEvent::Unknown { .. } => return,
    }

    state_notifier.send_replace(self.snapshot.state);
  }

  /// Terminal state is sticky: once Completed/Failed/Cancelled/Stalled
  /// is set, later transitions are ignored. Without this, the
  /// stdout-stream tail could overwrite a Completed with Failed when
  /// the child exits cleanly after its last event.
  fn set_state(&mut self, state: SessionState, state_notifier: &watch::Sender<SessionState>) {
    if self.snapshot.state.is_terminated() {
      return;
    }

    if state.is_terminated() {
      tracing::info!(state = ?state, "session terminal");
    }

    self.snapshot.state = state;
    state_notifier.send_replace(state);
  }
}

async fn stream_agent_events(
  stdout: ChildStdout,
  agent: Arc<dyn AgentAdapter>,
  inner: Arc<Mutex<SessionInner>>,
  state_notifier: watch::Sender<SessionState>,
) {
  let mut lines = BufReader::new(stdout).lines();

  loop {
    match lines.next_line().await {
      Ok(Some(line)) => {
        let events = match serde_json::from_str(&line) {
          Ok(value) => agent.map_event(value),
          Err(err) => vec![AgentEvent::Error {
            detail: err.to_string(),
          }],
        };

        let mut inner = inner.lock().expect("session mutex never poisoned");
        for event in events {
          inner.apply_event(event, &state_notifier);
        }
      },
      Ok(None) => break,
      Err(err) => {
        inner.lock().expect("session mutex never poisoned").apply_event(
          AgentEvent::Error {
            detail: err.to_string(),
          },
          &state_notifier,
        );
        break;
      },
    }
  }

  inner
    .lock()
    .expect("session mutex never poisoned")
    .finish_output_stream(&state_notifier);
}

#[cfg(test)]
mod tests {
  use serde_json::json;
  use tracing::subscriber::with_default;
  use tracing_subscriber::{Registry, layer::SubscriberExt};

  use super::*;
  use crate::agent::ToolCallPhase;
  use crate::agent::{AgentAdapter, AgentCommand};
  use crate::logging::tests::{CaptureLayer, captured_event, captured_message_exists};

  fn session_inner() -> (SessionInner, watch::Sender<SessionState>) {
    let snapshot = SessionSnapshot {
      started_at: Utc::now(),
      ..Default::default()
    };
    let (state_notifier, _) = watch::channel(snapshot.state);

    (
      SessionInner {
        snapshot,
        writer: None,
        child: None,
      },
      state_notifier,
    )
  }

  #[test]
  fn token_usage_events_accumulate_without_overflow() {
    let (mut inner, state_notifier) = session_inner();

    inner.apply_event(
      AgentEvent::TokenUsage {
        input: u64::MAX - 1,
        output: 10,
        cache_read: 20,
      },
      &state_notifier,
    );
    inner.apply_event(
      AgentEvent::TokenUsage {
        input: 10,
        output: u64::MAX,
        cache_read: 30,
      },
      &state_notifier,
    );

    assert_eq!(inner.snapshot.tokens.input, u64::MAX);
    assert_eq!(inner.snapshot.tokens.output, u64::MAX);
    assert_eq!(inner.snapshot.tokens.cache_read, 50);
  }

  #[test]
  fn rate_limit_observation_keeps_latest_event_per_scope() {
    let (mut inner, state_notifier) = session_inner();
    let reset_at = "2026-05-16T10:15:30Z".parse().expect("test timestamp parses");
    let stale = "2026-05-16T10:00:00Z".parse().expect("test timestamp parses");
    let fresh = "2026-05-16T10:05:00Z".parse().expect("test timestamp parses");

    inner.apply_event(
      AgentEvent::RateLimit {
        scope: "codex:tokens_per_min".into(),
        remaining: 50,
        reset_at,
        observed_at: fresh,
      },
      &state_notifier,
    );
    inner.apply_event(
      AgentEvent::RateLimit {
        scope: "codex:tokens_per_min".into(),
        remaining: 10,
        reset_at,
        observed_at: stale,
      },
      &state_notifier,
    );

    let observation = inner
      .snapshot
      .rate_limits
      .get("codex:tokens_per_min")
      .expect("rate limit observation stored");
    assert_eq!(observation.remaining, 50);
    assert_eq!(observation.observed_at, fresh);
  }

  #[test]
  fn observation_events_write_jsonl_without_snapshot_updates() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.jsonl");

    {
      let (mut inner, state_notifier) = session_inner();
      let rx = state_notifier.subscribe();
      inner.writer = Some(JsonlWriter::open(&path).expect("writer opens"));

      inner.apply_event(
        AgentEvent::ToolCall {
          call_id: Some("tool-1".into()),
          name: Some("Bash".into()),
          phase: ToolCallPhase::Request,
          input: Some(json!({"command": "ls"})),
          output: None,
          raw: json!({"type": "assistant"}),
        },
        &state_notifier,
      );
      inner.apply_event(
        AgentEvent::Subagent {
          call_id: Some("collab-1".into()),
          action: "spawnAgent".into(),
          status: Some("completed".into()),
          target_ids: vec!["thread-2".into()],
          raw: json!({"type": "collabAgentToolCall"}),
        },
        &state_notifier,
      );
      inner.apply_event(
        AgentEvent::Unknown {
          event_type: Some("future_event_kind".into()),
          raw: json!({"type": "future_event_kind"}),
        },
        &state_notifier,
      );

      assert!(matches!(inner.snapshot.state, SessionState::UnStarted));
      assert!(inner.snapshot.agent_session_id.is_none());
      assert!(inner.snapshot.last_event_at.is_none());
      assert!(inner.snapshot.last_message.is_none());
      assert_eq!(inner.snapshot.tokens.input, 0);
      assert_eq!(inner.snapshot.tokens.output, 0);
      assert_eq!(inner.snapshot.tokens.cache_read, 0);
      assert!(inner.snapshot.rate_limits.is_empty());
      assert!(!rx.has_changed().expect("watch sender alive"));
    }

    let lines = std::fs::read_to_string(&path)
      .expect("session JSONL reads")
      .lines()
      .map(|line| serde_json::from_str(line).expect("line is JSON"))
      .collect::<Vec<serde_json::Value>>();

    assert_eq!(
      lines,
      vec![
        json!({
          "kind": "tool_call",
          "call_id": "tool-1",
          "name": "Bash",
          "phase": "request",
          "input": {"command": "ls"},
          "output": null,
          "raw": {"type": "assistant"}
        }),
        json!({
          "kind": "subagent",
          "call_id": "collab-1",
          "action": "spawnAgent",
          "status": "completed",
          "target_ids": ["thread-2"],
          "raw": {"type": "collabAgentToolCall"}
        }),
        json!({
          "kind": "unknown",
          "event_type": "future_event_kind",
          "raw": {"type": "future_event_kind"}
        }),
      ]
    );
  }

  #[tokio::test]
  async fn malformed_jsonl_writes_error_event_and_fails_session() {
    #[derive(Debug)]
    struct NoopAdapter;

    impl AgentAdapter for NoopAdapter {
      fn build_command(&self, _profile: &AgentProfileSchema, _prompt: String) -> AgentCommand {
        unreachable!("stream parse failure does not call build_command")
      }

      fn map_event(&self, _value: serde_json::Value) -> Vec<AgentEvent> {
        Vec::new()
      }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.jsonl");
    let (mut inner, state_notifier) = session_inner();
    inner.writer = Some(JsonlWriter::open(&path).expect("writer opens"));
    inner.set_state(SessionState::Running, &state_notifier);
    let inner = Arc::new(Mutex::new(inner));

    let mut child = Command::new("sh")
      .arg("-c")
      .arg("printf '%s\n' '{bad-json'")
      .stdout(Stdio::piped())
      .spawn()
      .expect("test child spawns");
    let stdout = child.stdout.take().expect("stdout piped");

    stream_agent_events(stdout, Arc::new(NoopAdapter), Arc::clone(&inner), state_notifier).await;
    let status = child.wait().await.expect("test child exits");
    assert!(status.success());

    {
      let mut guard = inner.lock().expect("session mutex never poisoned");
      assert!(matches!(guard.snapshot.state, SessionState::Failed));
      guard.writer = None;
    }

    let lines = std::fs::read_to_string(&path)
      .expect("session JSONL reads")
      .lines()
      .map(|line| serde_json::from_str(line).expect("line is JSON"))
      .collect::<Vec<serde_json::Value>>();

    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["kind"], "error");
    assert!(
      lines[0]["detail"]
        .as_str()
        .expect("error detail is string")
        .contains("key must be a string")
    );
  }

  #[test]
  fn set_state_emits_terminal_log_on_terminal_transition() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    with_default(subscriber, || {
      let (mut inner, state_notifier) = session_inner();
      inner.set_state(SessionState::Running, &state_notifier);
      inner.set_state(SessionState::Completed, &state_notifier);
    });

    let events = events.lock().expect("events mutex");
    assert!(captured_message_exists(&events, "session terminal"));
    let event = captured_event(&events, "session terminal");
    assert_eq!(event["state"], "Completed");
  }

  #[test]
  fn apply_event_emits_agent_session_id_log() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    with_default(subscriber, || {
      let (mut inner, state_notifier) = session_inner();
      inner.apply_event(
        AgentEvent::SessionStarted {
          session_id: "sess-123".into(),
        },
        &state_notifier,
      );
    });

    let events = events.lock().expect("events mutex");
    let event = captured_event(&events, "agent session id observed");
    assert_eq!(event["session_id"], "sess-123");
  }
}
