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

mod events;

use serde_json::Value;

use super::{AgentAdapter, AgentCommand, AgentEvent, AgentStdin, ToolCallPhase, build_extra_args};
use crate::config::AgentProfileSchema;
use events::{CurrentEvent, ThreadItem};

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

  let Some(event) = events::parse_current(value) else {
    return vec![unknown_event(value)];
  };

  match event {
    CurrentEvent::ThreadStarted { thread_id } => vec![AgentEvent::SessionStarted { session_id: thread_id }],
    CurrentEvent::ItemStarted { item } => map_current_item(value, item, ToolCallPhase::Request),
    CurrentEvent::ItemCompleted { item } => map_current_item(value, item, ToolCallPhase::Result),
    CurrentEvent::TurnCompleted { usage } => map_current_turn_completed(usage),
    CurrentEvent::TurnFailed { error } => vec![AgentEvent::Error { detail: error.message }],
    CurrentEvent::Error { message } => vec![AgentEvent::Error { detail: message }],
    CurrentEvent::TurnStarted | CurrentEvent::ItemUpdated | CurrentEvent::Unknown => {
      tracing::debug!(
        runtime = "codex",
        codex_event_type = event_type(value).unwrap_or("unknown"),
        "codex event retained as unknown",
      );
      vec![unknown_event(value)]
    },
  }
}

fn map_current_item(value: &Value, item: ThreadItem, phase: ToolCallPhase) -> Vec<AgentEvent> {
  let item_type = item.kind.clone();
  let Some(raw_item) = value.get("item").cloned() else {
    return vec![unknown_event(value)];
  };

  match item_type.as_str() {
    "agent_message" if phase == ToolCallPhase::Result => vec![AgentEvent::Message {
      text: item.text.unwrap_or_default(),
    }],
    "command_execution" => vec![AgentEvent::ToolCall {
      call_id: Some(item.id),
      name: Some(item_type.to_string()),
      phase,
      input: (phase == ToolCallPhase::Request).then_some(raw_item.clone()),
      output: (phase == ToolCallPhase::Result).then_some(raw_item),
      raw: value.clone(),
    }],
    "mcp_tool_call" => vec![AgentEvent::ToolCall {
      call_id: Some(item.id),
      name: item.tool,
      phase,
      input: (phase == ToolCallPhase::Request).then(|| item.arguments.clone()).flatten(),
      output: (phase == ToolCallPhase::Result).then(|| item.result.or(item.error)).flatten(),
      raw: value.clone(),
    }],
    "collab_tool_call" => vec![AgentEvent::Subagent {
      call_id: Some(item.id),
      action: item.tool.unwrap_or_else(|| "unknown".into()),
      status: item.status,
      target_ids: item.receiver_thread_ids,
      raw: value.clone(),
    }],
    _ => vec![unknown_event(value)],
  }
}

/// `turn.completed` carries both the per-turn usage and the stream
/// terminator — fan out into two events so the session sees both.
fn map_current_turn_completed(usage: events::TokenUsage) -> Vec<AgentEvent> {
  vec![
    AgentEvent::TokenUsage {
      input: usage.input_tokens,
      output: usage.output_tokens,
      cache_read: usage.cached_input_tokens,
    },
    AgentEvent::Completed,
  ]
}

/// `pub(super)` so the in-module tests below can drive it directly
/// without round-tripping through the adapter.
pub(super) fn map_value(value: &Value) -> Option<AgentEvent> {
  let envelope = events::parse_legacy(value)?;
  let msg = envelope.msg;

  match msg.kind.as_str() {
    "session_configured" => {
      // Newer codex versions report session id under `msg.session_id`,
      // older ones under the outer envelope's `id`. Try both.
      let session_id = msg.session_id.or(envelope.id)?;
      Some(AgentEvent::SessionStarted { session_id })
    },
    "agent_message" => {
      let text = msg.message.or(msg.text).unwrap_or_default();
      Some(AgentEvent::Message { text })
    },
    "token_count" => {
      let info = msg.info?;
      // Prefer the `total_token_usage` subtree when present — it
      // already represents the cumulative count, so we don't need to
      // accumulate per-turn deltas ourselves.
      let totals = info.usage();
      Some(AgentEvent::TokenUsage {
        input: totals.input_tokens,
        output: totals.output_tokens,
        cache_read: totals.cached_input_tokens,
      })
    },
    "rate_limit_warning" | "rate_limit_reset" => {
      let scope_raw = msg.scope.as_deref().unwrap_or("unknown");
      // Provider prefix keeps Codex limits from colliding with Claude
      // limits in the session's per-scope map.
      let scope = format!("codex:{scope_raw}");
      let remaining = msg.remaining.unwrap_or(0);
      let reset_at = msg
        .reset_at
        .as_deref()
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
      let detail = msg.message.unwrap_or_default();
      Some(AgentEvent::Error { detail })
    },
    other => {
      tracing::debug!(
        runtime = "codex",
        codex_event_type = other,
        "codex legacy event retained as unknown",
      );
      None
    },
  }
}

fn unknown_event(value: &Value) -> AgentEvent {
  AgentEvent::Unknown {
    event_type: event_type(value).map(str::to_string),
    raw: value.clone(),
  }
}

fn event_type(value: &Value) -> Option<&str> {
  value
    .get("type")
    .and_then(Value::as_str)
    .or_else(|| value.pointer("/msg/type").and_then(Value::as_str))
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
  fn current_thread_started_without_thread_id_maps_to_unknown() {
    let line = r#"{"type":"thread.started"}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::Unknown {
        event_type: Some("thread.started".into()),
        raw: json!({"type": "thread.started"}),
      }]
    );
  }

  #[test]
  fn current_item_completed_agent_message_maps_to_message() {
    let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hello"}}"#;
    assert_eq!(parse_events(line), vec![AgentEvent::Message { text: "hello".into() }]);
  }

  #[test]
  fn current_agent_message_started_maps_to_unknown_event() {
    let line = r#"{"type":"item.started","item":{"id":"item_0","type":"agent_message","text":""}}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::Unknown {
        event_type: Some("item.started".into()),
        raw: json!({
          "type": "item.started",
          "item": {
            "id": "item_0",
            "type": "agent_message",
            "text": ""
          }
        }),
      }]
    );
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
  fn current_mcp_tool_call_completed_maps_to_tool_call_result() {
    let line = r#"{"type":"item.completed","item":{"id":"item_2","type":"mcp_tool_call","server":"github","tool":"pulls.get","arguments":{"number":96},"result":{"state":"OPEN"},"status":"completed"}}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::ToolCall {
        call_id: Some("item_2".into()),
        name: Some("pulls.get".into()),
        phase: ToolCallPhase::Result,
        input: None,
        output: Some(json!({"state": "OPEN"})),
        raw: json!({
          "type": "item.completed",
          "item": {
            "id": "item_2",
            "type": "mcp_tool_call",
            "server": "github",
            "tool": "pulls.get",
            "arguments": {"number": 96},
            "result": {"state": "OPEN"},
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
  fn current_collab_tool_call_maps_to_subagent_event() {
    let line = r#"{"type":"item.started","item":{"id":"call_2","type":"collab_tool_call","tool":"spawn_agent","status":"in_progress","sender_thread_id":"thread-1","receiver_thread_ids":["thread-3"],"prompt":"scan docs"}}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::Subagent {
        call_id: Some("call_2".into()),
        action: "spawn_agent".into(),
        status: Some("in_progress".into()),
        target_ids: vec!["thread-3".into()],
        raw: json!({
          "type": "item.started",
          "item": {
            "id": "call_2",
            "type": "collab_tool_call",
            "tool": "spawn_agent",
            "status": "in_progress",
            "sender_thread_id": "thread-1",
            "receiver_thread_ids": ["thread-3"],
            "prompt": "scan docs"
          }
        }),
      }]
    );
  }

  #[test]
  fn collab_agent_tool_call_retains_unknown_raw_event() {
    let line = r#"{"type":"collabAgentToolCall","id":"call_1","tool":"spawnAgent","status":"completed","senderThreadId":"thread-1","receiverThreadIds":["thread-2"],"agentsStates":{},"model":"gpt-5.5","reasoningEffort":"medium","prompt":"scan docs"}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::Unknown {
        event_type: Some("collabAgentToolCall".into()),
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
