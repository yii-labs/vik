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
//! Provider events append to JSONL as the durable record. Decoded
//! semantic events also accumulate into the snapshot.
#![allow(dead_code)]
mod factory;
mod jsonl_writer;
mod snapshot;

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use crate::agent::AgentEvent;
pub use factory::*;
pub use snapshot::*;

use chrono::Utc;
use serde_json::Value;
use thiserror::Error;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdout, Command};
use tokio::sync::watch;
use uuid::Uuid;

use crate::agent::{AgentAdapter, AgentCommand, AgentStdin, get_adapter};
use crate::config::{AgentProfileSchema, IssueStage};
use crate::context::Issue;
use crate::shell::{Child, CommandExecError, CommandExt};
use crate::template::{Context as TemplateContext, PromptRenderer, StageContext, TemplateError};
use crate::workflow::Workflow;

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

struct SessionOptions {
  workflow: Arc<Workflow>,
  log_file: PathBuf,
  issue_workdir: PathBuf,
  issue: Issue,
  stage_name: String,
  stage: IssueStage,
  profile: AgentProfileSchema,
}

#[derive(Clone)]
pub struct Session {
  agent: Arc<dyn AgentAdapter>,
  opts: Arc<SessionOptions>,
  inner: Arc<Mutex<SessionInner>>,
  state_notifier: watch::Sender<SessionState>,
}

struct SessionInner {
  snapshot: SessionSnapshot,
  writer: Option<JsonlWriter>,
  child: Option<Child>,
}

impl Session {
  pub(super) async fn spawn(
    workflow: Arc<Workflow>,
    issue: Issue,
    stage_name: String,
    stage: IssueStage,
    issue_workdir: PathBuf,
    profile: AgentProfileSchema,
  ) -> Result<Self, SessionError> {
    let now = Utc::now();
    // State prefix is for human eyeballing; UUIDv7 keeps names unique
    // and sorts runs within one state. Provider session ids land inside
    // the JSONL, not in the filename, because they are not always known
    // when the file must be created.
    let log_file = workflow
      .workspace()
      .issue_sessions_dir(&issue.id)
      .join(session_log_file_name(&issue.state, Uuid::now_v7()));

    let opts = SessionOptions {
      workflow,
      log_file,
      issue_workdir,
      issue,
      stage_name,
      stage,
      profile,
    };

    let snapshot = SessionSnapshot {
      started_at: now,
      ..Default::default()
    };

    let agent = get_adapter(opts.profile.runtime);

    let (state_notifier, _) = watch::channel(snapshot.state);

    let session = Self {
      agent,
      opts: Arc::new(opts),
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
    &self.opts.issue.id
  }

  pub fn issue(&self) -> &Issue {
    &self.opts.issue
  }

  pub fn stage(&self) -> &IssueStage {
    &self.opts.stage
  }

  pub fn profile(&self) -> &AgentProfileSchema {
    &self.opts.profile
  }

  pub fn snapshot(&self) -> SessionSnapshot {
    self.inner.lock().expect("session mutex never poisoned").snapshot.clone()
  }

  pub fn state(&self) -> SessionState {
    self.inner.lock().expect("session mutex never poisoned").snapshot.state
  }

  pub fn log_file(&self) -> &PathBuf {
    &self.opts.log_file
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
      .current_dir(&self.opts.issue_workdir)
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

    if let Some(parent) = self.log_file().parent() {
      fs::create_dir_all(parent).await?;
    }

    // render the prompt template
    let prompt = self.render_prompt().await?;

    Ok(self.agent.build_command(self.profile(), prompt))
  }

  async fn render_prompt(&self) -> Result<String, SessionError> {
    let renderer = PromptRenderer::new();
    let prompt_file = self
      .opts
      .workflow
      .resolve_path(&self.stage().prompt_file)
      .ok_or_else(|| SessionError::PromptPath(self.stage().prompt_file.clone()))?;

    let mut file = File::open(&prompt_file)
      .await
      .map_err(|err| SessionError::TemplateRender(TemplateError::Io(err)))?;

    let mut template = String::new();
    file
      .read_to_string(&mut template)
      .await
      .map_err(|err| SessionError::TemplateRender(TemplateError::Io(err)))?;

    let context = render_context(self);
    Ok(renderer.render(&template, &context).await?)
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

    tokio::spawn(stream_agent_events(stdout, agent, inner, state_notifier));

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
    let notify_state_watch = !matches!(&event, AgentEvent::ProviderEvent { .. });

    if let Some(writer) = &mut self.writer
      && let Err(err) = writer.write(&event)
    {
      tracing::error!("session jsonl write failed: {err}");
    }

    if notify_state_watch {
      self.snapshot.last_event_at = Some(Utc::now());
    }

    match event {
      AgentEvent::SessionStarted { session_id } => {
        self.snapshot.agent_session_id = Some(session_id);
      },
      AgentEvent::Message { text } => {
        self.snapshot.last_message = Some(text);
      },
      AgentEvent::TokenUsage {
        input,
        output,
        cache_read,
      } => {
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
        self.set_state(SessionState::Completed, state_notifier);
      },
      AgentEvent::Error { detail: _ } => {
        self.set_state(SessionState::Failed, state_notifier);
      },
      AgentEvent::ProviderEvent { runtime: _, event: _ } => {},
    }

    if notify_state_watch {
      state_notifier.send_replace(self.snapshot.state);
    }
  }

  /// Terminal state is sticky: once Completed/Failed/Cancelled/Stalled
  /// is set, later transitions are ignored. Without this, the
  /// stdout-stream tail could overwrite a Completed with Failed when
  /// the child exits cleanly after its last event.
  fn set_state(&mut self, state: SessionState, state_notifier: &watch::Sender<SessionState>) {
    if self.snapshot.state.is_terminated() {
      return;
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
        let events = map_agent_stdout_line(agent.as_ref(), &line);

        for event in events {
          apply_agent_event(&inner, &state_notifier, event);
        }
      },
      Ok(None) => break,
      Err(err) => {
        apply_agent_event(
          &inner,
          &state_notifier,
          AgentEvent::Error {
            detail: err.to_string(),
          },
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

fn map_agent_stdout_line(agent: &dyn AgentAdapter, line: &str) -> Vec<AgentEvent> {
  match serde_json::from_str(line) {
    Ok(value) => map_provider_value(agent, value),
    Err(err) => vec![AgentEvent::Error {
      detail: err.to_string(),
    }],
  }
}

fn map_provider_value(agent: &dyn AgentAdapter, value: Value) -> Vec<AgentEvent> {
  let semantic_events = agent.map_event(&value);
  let mut events = Vec::with_capacity(semantic_events.len() + 1);
  events.push(AgentEvent::ProviderEvent {
    runtime: agent.runtime_name().to_string(),
    event: value,
  });
  events.extend(semantic_events);
  events
}

fn apply_agent_event(
  inner: &Arc<Mutex<SessionInner>>,
  state_notifier: &watch::Sender<SessionState>,
  event: AgentEvent,
) {
  inner
    .lock()
    .expect("session mutex never poisoned")
    .apply_event(event, state_notifier);
}

fn render_context(session: &Session) -> TemplateContext {
  let session = &session.opts;
  let mut context = TemplateContext::new();

  // Payload first, stage bindings second: a tracker that happens to
  // include keys named `cwd`, `stage`, etc. cannot shadow the bindings
  // the prompt author depends on.
  for (key, value) in &session.issue.extra_payload {
    let Some(key) = key.as_str() else {
      continue;
    };
    let value = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    context.with(key, value);
  }

  let workspace_root = session.workflow.workspace().root().to_path_buf();

  // Hook renderer and prompt renderer share `StageContext` so the two
  // template surfaces cannot disagree on what `cwd` / `issue` / `stage`
  // mean for one dispatch.
  let stage_ctx = StageContext {
    issue: &session.issue,
    stage_name: &session.stage_name,
    agent_profile: &session.stage.agent,
    stage_state: &session.stage.when.state,
    issue_workdir: &session.issue_workdir,
    workspace_root: &workspace_root,
  };
  stage_ctx.apply(&mut context);

  // Prompt-only extras the hook context does not need. The richer
  // `stage` value here merges the schema with the orchestrator's
  // stage name so prompts can address either form.
  context.with("workflow", session.workflow.schema());
  context.with("loop", &session.workflow.schema().loop_);
  context.with("stage", stage_context_value(&session.stage, &session.stage_name));
  context.with("profile", &session.profile);

  context
}

fn stage_context_value(stage: &IssueStage, stage_name: &str) -> serde_json::Value {
  let mut value = serde_json::to_value(stage).unwrap_or(serde_json::Value::Null);
  if let serde_json::Value::Object(stage_value) = &mut value {
    stage_value.insert("name".to_string(), serde_json::Value::String(stage_name.to_string()));
    stage_value.insert("state".to_string(), serde_json::Value::String(stage.when.state.clone()));
  }
  value
}

fn session_log_file_name(issue_state: &str, session_id: Uuid) -> String {
  format!("{issue_state}-{session_id}.jsonl")
}

#[cfg(test)]
mod tests {
  use crate::config::AgentRuntime;

  use super::*;

  #[test]
  fn session_log_file_name_puts_issue_state_before_uuid() {
    let session_id = Uuid::nil();

    assert_eq!(
      session_log_file_name("todo", session_id),
      "todo-00000000-0000-0000-0000-000000000000.jsonl"
    );
  }

  #[test]
  fn codex_provider_event_precedes_semantic_events() {
    let agent = get_adapter(AgentRuntime::Codex);
    let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hello"}}"#;
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");

    assert_eq!(
      map_agent_stdout_line(agent.as_ref(), line),
      vec![
        AgentEvent::ProviderEvent {
          runtime: "codex".into(),
          event: value,
        },
        AgentEvent::Message { text: "hello".into() },
      ]
    );
  }

  #[test]
  fn codex_tool_call_provider_event_is_retained_without_semantic_events() {
    let agent = get_adapter(AgentRuntime::Codex);
    let line = r#"{"type":"item.completed","item":{"id":"tool_0","type":"tool_call","name":"shell","arguments":"{}"}}"#;
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");

    assert_eq!(
      map_agent_stdout_line(agent.as_ref(), line),
      vec![AgentEvent::ProviderEvent {
        runtime: "codex".into(),
        event: value,
      }]
    );
  }

  #[test]
  fn codex_unknown_provider_event_is_retained_without_semantic_events() {
    let agent = get_adapter(AgentRuntime::Codex);
    let line = r#"{"type":"future.event","payload":{"ok":true}}"#;
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");

    assert_eq!(
      map_agent_stdout_line(agent.as_ref(), line),
      vec![AgentEvent::ProviderEvent {
        runtime: "codex".into(),
        event: value,
      }]
    );
  }

  #[test]
  fn claude_tool_only_provider_event_is_retained_without_semantic_message() {
    let agent = get_adapter(AgentRuntime::ClaudeCode);
    let line =
      r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t-1","name":"Bash","input":{}}]}}"#;
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");

    assert_eq!(
      map_agent_stdout_line(agent.as_ref(), line),
      vec![AgentEvent::ProviderEvent {
        runtime: "claude_code".into(),
        event: value,
      }]
    );
  }

  #[test]
  fn claude_user_provider_event_is_retained_without_semantic_events() {
    let agent = get_adapter(AgentRuntime::ClaudeCode);
    let line = r#"{"type":"user","message":{"content":[]}}"#;
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");

    assert_eq!(
      map_agent_stdout_line(agent.as_ref(), line),
      vec![AgentEvent::ProviderEvent {
        runtime: "claude_code".into(),
        event: value,
      }]
    );
  }

  #[test]
  fn invalid_jsonl_maps_to_error_without_provider_event() {
    let agent = get_adapter(AgentRuntime::Codex);
    let events = map_agent_stdout_line(agent.as_ref(), "{not-json");

    assert_eq!(events.len(), 1);
    assert!(matches!(events.first(), Some(AgentEvent::Error { .. })));
  }

  #[test]
  fn provider_event_does_not_change_snapshot_semantics() {
    let (state_notifier, state_rx) = watch::channel(SessionState::Running);
    let mut inner = SessionInner {
      snapshot: SessionSnapshot {
        state: SessionState::Running,
        ..Default::default()
      },
      writer: None,
      child: None,
    };

    inner.apply_event(
      AgentEvent::ProviderEvent {
        runtime: "codex".into(),
        event: serde_json::json!({"type":"future.event"}),
      },
      &state_notifier,
    );

    assert!(matches!(inner.snapshot.state, SessionState::Running));
    assert!(inner.snapshot.last_message.is_none());
    assert_eq!(inner.snapshot.tokens.input, 0);
    assert_eq!(inner.snapshot.tokens.output, 0);
    assert_eq!(inner.snapshot.tokens.cache_read, 0);
    assert!(inner.snapshot.last_event_at.is_none());
    assert!(!state_rx.has_changed().expect("state channel open"));
  }

  #[test]
  fn jsonl_writer_persists_provider_and_semantic_events_in_order() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let log_file = tempdir.path().join("session.jsonl");
    let provider_event = AgentEvent::ProviderEvent {
      runtime: "codex".into(),
      event: serde_json::json!({
        "type": "item.completed",
        "item": {
          "id": "tool_0",
          "type": "tool_call",
          "name": "shell"
        }
      }),
    };
    let message_event = AgentEvent::Message { text: "hello".into() };

    {
      let mut writer = JsonlWriter::open(&log_file).expect("open JSONL writer");
      writer.write(&provider_event).expect("write provider event");
      writer.write(&message_event).expect("write message event");
    }

    let contents = std::fs::read_to_string(&log_file).expect("read JSONL");
    let events: Vec<AgentEvent> = contents
      .lines()
      .map(|line| serde_json::from_str(line).expect("event JSON"))
      .collect();

    assert_eq!(events, vec![provider_event, message_event]);
  }
}
