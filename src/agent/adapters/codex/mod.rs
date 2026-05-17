//! Codex provider adapter.
//!
//! Command shape:
//!
//! ```text
//! codex exec [profile.args...] --json -m <model>
//! ```
//!
//! Prompt is written to stdin; Codex blocks until EOF.
//!
//! Codex emits two event shapes today and we accept both: the legacy
//! `{"msg": {"type": ...}}` envelope and the newer flat
//! `{"type": "thread.started" | "item.completed" | "turn.completed"}`.
//! Unknown shapes log at DEBUG and emit `AgentEvent::Unknown` with the
//! full parsed provider JSON so future Codex event kinds stay visible
//! in session JSONL.

use serde_json::Value;

use super::{AgentAdapter, AgentCommand, AgentEvent, AgentStdin, ToolCallPhase, build_extra_args};
use crate::config::AgentProfileSchema;

const CODEX_PROGRAM: &str = "codex";

#[derive(Debug, Clone)]
pub struct CodexAdapter;

impl AgentAdapter for CodexAdapter {
  fn build_command(&self, profile: &AgentProfileSchema, prompt: String) -> AgentCommand {
    let mut args: Vec<String> = vec!["exec".into()];

    args.extend(build_extra_args(&profile.args));

    args.extend(["--json".into(), "-m".into(), profile.model.clone()]);

    AgentCommand {
      program: CODEX_PROGRAM.into(),
      args,
      stdin: AgentStdin::Pipe(prompt),
    }
  }

  fn map_event(&self, value: Value) -> Vec<AgentEvent> {
    map_events(&value)
  }
}

fn map_events(value: &Value) -> Vec<AgentEvent> {
  // Try the legacy nested-`msg` form first; if that misses, fall through
  // to the flat `type` form. Trying both lets us cope with mixed
  // streams from different Codex CLI versions in one run.
  if let Some(event) = map_value(value) {
    return vec![event];
  }

  let Some(ty) = value.get("type").and_then(Value::as_str) else {
    return vec![unknown_event(value)];
  };

  match ty {
    "thread.started" => value
      .get("thread_id")
      .and_then(Value::as_str)
      .map(|session_id| {
        vec![AgentEvent::SessionStarted {
          session_id: session_id.to_string(),
        }]
      })
      .unwrap_or_default(),
    "item.started" => map_current_item_started(value),
    "item.completed" => map_current_item_completed(value),
    "turn.completed" => map_current_turn_completed(value),
    "collabAgentToolCall" => map_collab_agent_tool_call(value),
    "error" => vec![AgentEvent::Error {
      detail: value.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
    }],
    _ => {
      tracing::debug!(
        runtime = "codex",
        codex_event_type = ty,
        "codex event ignored: unknown type",
      );
      vec![unknown_event(value)]
    },
  }
}

fn map_collab_agent_tool_call(value: &Value) -> Vec<AgentEvent> {
  let target_ids = value
    .get("receiverThreadIds")
    .and_then(Value::as_array)
    .map(|ids| {
      ids
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<String>>()
    })
    .unwrap_or_default();

  vec![AgentEvent::Subagent {
    call_id: value.get("id").and_then(Value::as_str).map(str::to_string),
    action: value.get("tool").and_then(Value::as_str).unwrap_or("unknown").to_string(),
    status: value.get("status").and_then(Value::as_str).map(str::to_string),
    target_ids,
    raw: value.clone(),
  }]
}

fn map_current_item_started(value: &Value) -> Vec<AgentEvent> {
  let Some(item) = value.get("item") else {
    return vec![unknown_event(value)];
  };
  let Some(item_type) = item.get("type").and_then(Value::as_str) else {
    return vec![unknown_event(value)];
  };

  match item_type {
    "command_execution" => vec![AgentEvent::ToolCall {
      call_id: item.get("id").and_then(Value::as_str).map(str::to_string),
      name: Some(item_type.to_string()),
      phase: ToolCallPhase::Request,
      input: Some(item.clone()),
      output: None,
      raw: value.clone(),
    }],
    _ => vec![unknown_event(value)],
  }
}

fn map_current_item_completed(value: &Value) -> Vec<AgentEvent> {
  let Some(item) = value.get("item") else {
    return vec![unknown_event(value)];
  };
  let Some(item_type) = item.get("type").and_then(Value::as_str) else {
    return vec![unknown_event(value)];
  };

  match item_type {
    "agent_message" => vec![AgentEvent::Message {
      text: item.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
    }],
    "command_execution" => vec![AgentEvent::ToolCall {
      call_id: item.get("id").and_then(Value::as_str).map(str::to_string),
      name: Some(item_type.to_string()),
      phase: ToolCallPhase::Result,
      input: None,
      output: Some(item.clone()),
      raw: value.clone(),
    }],
    _ => vec![unknown_event(value)],
  }
}

/// `turn.completed` carries both the per-turn usage and the stream
/// terminator — fan out into two events so the session sees both.
fn map_current_turn_completed(value: &Value) -> Vec<AgentEvent> {
  let mut events = Vec::new();
  if let Some(usage) = value.get("usage") {
    events.push(AgentEvent::TokenUsage {
      input: usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
      output: usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
      cache_read: usage.get("cached_input_tokens").and_then(Value::as_u64).unwrap_or(0),
    });
  }
  events.push(AgentEvent::Completed);
  events
}

/// `pub(super)` so the in-module tests below can drive it directly
/// without round-tripping through the adapter.
pub(super) fn map_value(value: &Value) -> Option<AgentEvent> {
  let msg = value.get("msg")?;
  let ty = msg.get("type")?.as_str()?;

  match ty {
    "session_configured" => {
      // Newer codex versions report session id under `msg.session_id`,
      // older ones under the outer envelope's `id`. Try both.
      let session_id = msg
        .get("session_id")
        .and_then(Value::as_str)
        .or_else(|| value.get("id").and_then(Value::as_str))?
        .to_string();
      Some(AgentEvent::SessionStarted { session_id })
    },
    "agent_message" => {
      let text = msg
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| msg.get("text").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
      Some(AgentEvent::Message { text })
    },
    "token_count" => {
      let info = msg.get("info")?;
      // Prefer the `total_token_usage` subtree when present — it
      // already represents the cumulative count, so we don't need to
      // accumulate per-turn deltas ourselves.
      let totals = info.get("total_token_usage").unwrap_or(info);
      let input = totals.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
      let output = totals.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
      let cache_read = totals.get("cached_input_tokens").and_then(Value::as_u64).unwrap_or(0);
      Some(AgentEvent::TokenUsage {
        input,
        output,
        cache_read,
      })
    },
    "rate_limit_warning" | "rate_limit_reset" => {
      let scope_raw = msg.get("scope").and_then(Value::as_str).unwrap_or("unknown");
      // Provider prefix keeps Codex limits from colliding with Claude
      // limits in the session's per-scope map.
      let scope = format!("codex:{scope_raw}");
      let remaining = msg.get("remaining").and_then(Value::as_u64).unwrap_or(0);
      let reset_at = msg
        .get("reset_at")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
      Some(AgentEvent::RateLimit {
        scope,
        remaining,
        reset_at,
        observed_at: chrono::Utc::now(),
      })
    },
    "turn_complete" | "shutdown_complete" => Some(AgentEvent::Completed),
    "error" => {
      let detail = msg.get("message").and_then(Value::as_str).unwrap_or("").to_string();
      Some(AgentEvent::Error { detail })
    },
    other => {
      tracing::debug!(
        runtime = "codex",
        codex_event_type = other,
        "codex event ignored: unknown type",
      );
      None
    },
  }
}

fn unknown_event(value: &Value) -> AgentEvent {
  AgentEvent::Unknown {
    event_type: value
      .get("type")
      .and_then(Value::as_str)
      .or_else(|| value.pointer("/msg/type").and_then(Value::as_str))
      .map(str::to_string),
    raw: value.clone(),
  }
}

#[cfg(test)]
mod tests {
  use crate::config::AgentRuntime;
  use serde_json::json;

  use super::*;

  fn parse(line: &str) -> Option<AgentEvent> {
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");
    map_value(&value)
  }

  fn parse_events(line: &str) -> Vec<AgentEvent> {
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");
    map_events(&value)
  }

  #[test]
  fn session_configured_maps_to_session_started() {
    let line = r#"{"id":"evt-0","msg":{"type":"session_configured","session_id":"S-1"}}"#;
    assert_eq!(
      parse(line),
      Some(AgentEvent::SessionStarted {
        session_id: "S-1".into(),
      })
    );
  }

  #[test]
  fn agent_message_maps_to_message() {
    let line = r#"{"id":"evt-1","msg":{"type":"agent_message","message":"hi"}}"#;
    assert_eq!(parse(line), Some(AgentEvent::Message { text: "hi".into() }));
  }

  #[test]
  fn token_count_reads_total_usage() {
    let line = r#"{"id":"evt-2","msg":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":20,"cached_input_tokens":5}}}}"#;
    assert_eq!(
      parse(line),
      Some(AgentEvent::TokenUsage {
        input: 10,
        output: 20,
        cache_read: 5,
      })
    );
  }

  #[test]
  fn turn_complete_maps_to_completed() {
    let line = r#"{"id":"evt-3","msg":{"type":"turn_complete"}}"#;
    assert_eq!(parse(line), Some(AgentEvent::Completed));
  }

  #[test]
  fn unknown_msg_event_maps_to_unknown_event_with_raw_payload() {
    let line = r#"{"id":"evt-4","msg":{"type":"future_event_kind"}}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::Unknown {
        event_type: Some("future_event_kind".into()),
        raw: json!({"id": "evt-4", "msg": {"type": "future_event_kind"}}),
      }]
    );
  }

  #[test]
  fn rate_limit_emits_scope_prefix() {
    let line = r#"{"id":"evt-5","msg":{"type":"rate_limit_warning","scope":"tokens_per_min","remaining":100}}"#;
    match parse(line) {
      Some(AgentEvent::RateLimit { scope, remaining, .. }) => {
        assert_eq!(scope, "codex:tokens_per_min");
        assert_eq!(remaining, 100);
      },
      other => panic!("expected RateLimit, got {other:?}"),
    }
  }

  #[test]
  fn error_event_maps_to_provider_error() {
    let line = r#"{"id":"evt-6","msg":{"type":"error","message":"boom"}}"#;
    match parse(line) {
      Some(AgentEvent::Error { detail }) => {
        assert_eq!(detail, "boom");
      },
      other => panic!("expected Error, got {other:?}"),
    }
  }

  #[test]
  fn current_thread_started_maps_to_session_started() {
    let line = r#"{"type":"thread.started","thread_id":"T-1"}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::SessionStarted {
        session_id: "T-1".into(),
      }]
    );
  }

  #[test]
  fn current_item_completed_agent_message_maps_to_message() {
    let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hello"}}"#;
    assert_eq!(parse_events(line), vec![AgentEvent::Message { text: "hello".into() }]);
  }

  #[test]
  fn current_turn_completed_maps_usage_and_completed() {
    let line = r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":2}}"#;
    assert_eq!(
      parse_events(line),
      vec![
        AgentEvent::TokenUsage {
          input: 10,
          output: 2,
          cache_read: 4,
        },
        AgentEvent::Completed,
      ]
    );
  }

  #[test]
  fn current_command_execution_started_maps_to_tool_call_request() {
    let line = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc pwd","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::ToolCall {
        call_id: Some("item_1".into()),
        name: Some("command_execution".into()),
        phase: ToolCallPhase::Request,
        input: Some(json!({
          "id": "item_1",
          "type": "command_execution",
          "command": "/bin/zsh -lc pwd",
          "aggregated_output": "",
          "exit_code": null,
          "status": "in_progress"
        })),
        output: None,
        raw: json!({
          "type": "item.started",
          "item": {
            "id": "item_1",
            "type": "command_execution",
            "command": "/bin/zsh -lc pwd",
            "aggregated_output": "",
            "exit_code": null,
            "status": "in_progress"
          }
        }),
      }]
    );
  }

  #[test]
  fn current_command_execution_completed_maps_to_tool_call_result() {
    let line = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc pwd","aggregated_output":"/tmp\n","exit_code":0,"status":"completed"}}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::ToolCall {
        call_id: Some("item_1".into()),
        name: Some("command_execution".into()),
        phase: ToolCallPhase::Result,
        input: None,
        output: Some(json!({
          "id": "item_1",
          "type": "command_execution",
          "command": "/bin/zsh -lc pwd",
          "aggregated_output": "/tmp\n",
          "exit_code": 0,
          "status": "completed"
        })),
        raw: json!({
          "type": "item.completed",
          "item": {
            "id": "item_1",
            "type": "command_execution",
            "command": "/bin/zsh -lc pwd",
            "aggregated_output": "/tmp\n",
            "exit_code": 0,
            "status": "completed"
          }
        }),
      }]
    );
  }

  #[test]
  fn current_turn_started_maps_to_unknown_event_with_raw_payload() {
    let line = r#"{"type":"turn.started"}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::Unknown {
        event_type: Some("turn.started".into()),
        raw: json!({"type": "turn.started"}),
      }]
    );
  }

  #[test]
  fn collab_agent_tool_call_maps_to_subagent_event() {
    let line = r#"{"type":"collabAgentToolCall","id":"call_1","tool":"spawnAgent","status":"completed","senderThreadId":"thread-1","receiverThreadIds":["thread-2"],"agentsStates":{},"model":"gpt-5.5","reasoningEffort":"medium","prompt":"scan docs"}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::Subagent {
        call_id: Some("call_1".into()),
        action: "spawnAgent".into(),
        status: Some("completed".into()),
        target_ids: vec!["thread-2".into()],
        raw: json!({
          "type": "collabAgentToolCall",
          "id": "call_1",
          "tool": "spawnAgent",
          "status": "completed",
          "senderThreadId": "thread-1",
          "receiverThreadIds": ["thread-2"],
          "agentsStates": {},
          "model": "gpt-5.5",
          "reasoningEffort": "medium",
          "prompt": "scan docs"
        }),
      }]
    );
  }

  #[test]
  fn happy_path_fixture_maps_to_expected_sequence() {
    let path = concat!(
      env!("CARGO_MANIFEST_DIR"),
      "/tests/fixtures/agent_events/codex/happy_path.jsonl"
    );
    let body = std::fs::read_to_string(path).expect("fixture present");
    let mut events: Vec<AgentEvent> = Vec::new();
    for line in body.lines() {
      events.extend(parse_events(line));
    }
    assert!(
      matches!(events[0], AgentEvent::SessionStarted { .. }),
      "first event must be SessionStarted, got {:?}",
      events.first()
    );
    assert_eq!(
      events.iter().filter(|e| matches!(e, AgentEvent::Message { .. })).count(),
      2,
      "fixture must contribute two assistant messages"
    );
    assert!(
      events.iter().any(|e| matches!(e, AgentEvent::TokenUsage { .. })),
      "fixture must contribute a TokenUsage"
    );
    assert!(
      events.iter().filter(|e| matches!(e, AgentEvent::Completed)).count() == 1,
      "turn.completed yields one Completed"
    );
  }

  #[test]
  fn rate_limit_fixture_yields_rate_limit_and_error() {
    let path = concat!(
      env!("CARGO_MANIFEST_DIR"),
      "/tests/fixtures/agent_events/codex/rate_limit_and_error.jsonl"
    );
    let body = std::fs::read_to_string(path).expect("fixture present");
    let mut rl_count = 0usize;
    let mut saw_error = false;
    for line in body.lines() {
      match parse(line) {
        Some(AgentEvent::RateLimit { scope, .. }) => {
          assert!(scope.starts_with("codex:"), "scope must be prefixed");
          rl_count += 1;
        },
        Some(AgentEvent::Error { .. }) => saw_error = true,
        _ => {},
      }
    }
    assert_eq!(rl_count, 2, "warning + reset each yield RateLimit");
    assert!(saw_error, "fixture includes one provider error line");
  }

  #[test]
  fn command_contains_expected_flags_and_stdin_pipe() {
    let adapter = CodexAdapter;
    let req =
      AgentProfileSchema::new(AgentRuntime::Codex, "gpt-5.5".into()).with_args(serde_yaml::Mapping::from_iter([
        (
          serde_yaml::Value::String("--config".into()),
          serde_yaml::Value::Sequence(serde_yaml::Sequence::from_iter(["model_reasoning_effort=high".into()])),
        ),
        (
          serde_yaml::Value::String("--ephemeral".into()),
          serde_yaml::Value::Bool(true),
        ),
        (
          serde_yaml::Value::String("--ignore-rules".into()),
          serde_yaml::Value::Bool(false),
        ),
      ]));

    let cmd = adapter.build_command(&req, "hello".into());

    assert_eq!(cmd.program, "codex");
    assert!(cmd.args.contains(&"--json".to_string()));
    assert!(cmd.args.contains(&"-m".to_string()));
    assert!(cmd.args.contains(&"gpt-5.5".to_string()));
    assert!(
      cmd.args.iter().any(|a| a == "model_reasoning_effort=high"),
      "typed param forwarded as --config override"
    );
    assert!(
      cmd.args.iter().any(|a| a == "--ephemeral"),
      "true boolean args are forwarded as no-value flags"
    );
    assert!(
      !cmd.args.iter().any(|a| a == "--ignore-rules"),
      "false boolean args are omitted"
    );
    match cmd.stdin {
      AgentStdin::Pipe(payload) => assert_eq!(payload, "hello"),
      other => panic!("expected Pipe(prompt), got {other:?}"),
    }
  }
}
