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
//! Codex emits flat JSONL events such as
//! `{"type": "thread.started" | "item.completed" | "turn.completed"}`.
//! Unknown shapes log at DEBUG and emit `AgentEvent::Unknown` with the
//! full parsed provider JSON so future Codex event kinds stay visible
//! in session JSONL.

mod events;

use serde_json::Value;

use super::{AgentAdapter, AgentCommand, AgentEvent, AgentStdin, ToolCallPhase, build_extra_args};
use crate::config::AgentProfileSchema;
use events::{CodexEvent, ThreadItem};

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
  let Ok(event) = events::parse(value) else {
    return vec![unknown_event(value)];
  };

  match event {
    CodexEvent::ThreadStarted { thread_id } => vec![AgentEvent::SessionStarted { session_id: thread_id }],
    CodexEvent::ItemStarted { item } => map_current_item(value, item, ToolCallPhase::Request),
    CodexEvent::ItemCompleted { item } => map_current_item(value, item, ToolCallPhase::Result),
    CodexEvent::TurnCompleted { usage } => map_current_turn_completed(usage),
    CodexEvent::TurnFailed { error } => vec![AgentEvent::Error { detail: error.message }],
    CodexEvent::Error { message } => vec![AgentEvent::Error { detail: message }],
    CodexEvent::TurnStarted | CodexEvent::ItemUpdated | CodexEvent::Unknown => {
      tracing::debug!(
        runtime = "codex",
        codex_event_type = events::event_type(value).as_deref().unwrap_or("unknown"),
        "codex event retained as unknown",
      );
      vec![unknown_event(value)]
    },
  }
}

fn map_current_item(value: &Value, item: ThreadItem, phase: ToolCallPhase) -> Vec<AgentEvent> {
  match item {
    ThreadItem::AgentMessage { text, .. } if phase == ToolCallPhase::Result => vec![AgentEvent::Message {
      text: text.unwrap_or_default(),
    }],
    ThreadItem::CommandExecution { id, fields } => {
      let raw_item = ThreadItem::command_execution_payload(&id, &fields);
      vec![AgentEvent::ToolCall {
        call_id: Some(id),
        name: Some("command_execution".into()),
        phase,
        input: (phase == ToolCallPhase::Request).then_some(raw_item.clone()),
        output: (phase == ToolCallPhase::Result).then_some(raw_item),
        raw: value.clone(),
      }]
    },
    ThreadItem::McpToolCall {
      id,
      tool,
      arguments,
      result,
      error,
    } => vec![AgentEvent::ToolCall {
      call_id: Some(id),
      name: tool,
      phase,
      input: (phase == ToolCallPhase::Request).then_some(arguments).flatten(),
      output: (phase == ToolCallPhase::Result).then(|| result.or(error)).flatten(),
      raw: value.clone(),
    }],
    ThreadItem::CollabToolCall {
      id,
      tool,
      status,
      receiver_thread_ids,
    } => vec![AgentEvent::Subagent {
      call_id: Some(id),
      action: tool.unwrap_or_else(|| "unknown".into()),
      status,
      target_ids: receiver_thread_ids,
      raw: value.clone(),
    }],
    _ => vec![unknown_event(value)],
  }
}

/// `turn.completed` carries both the per-turn usage and the stream
/// terminator — fan out into two events so the session sees both.
fn map_current_turn_completed(usage: Option<events::TokenUsage>) -> Vec<AgentEvent> {
  match usage {
    Some(usage) => vec![
      AgentEvent::TokenUsage {
        input: usage.input_tokens,
        output: usage.output_tokens,
        cache_read: usage.cached_input_tokens,
      },
      AgentEvent::Completed,
    ],
    None => vec![AgentEvent::Completed],
  }
}

fn unknown_event(value: &Value) -> AgentEvent {
  AgentEvent::Unknown {
    event_type: events::event_type(value),
    raw: value.clone(),
  }
}

#[cfg(test)]
mod tests {
  use crate::config::AgentRuntime;
  use serde_json::json;

  use super::*;

  fn parse_events(line: &str) -> Vec<AgentEvent> {
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");
    map_events(&value)
  }

  #[test]
  fn unsupported_msg_envelope_maps_to_unknown_event_with_raw_payload() {
    let line = r#"{"id":"evt-0","msg":{"type":"session_configured","session_id":"S-1"}}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::Unknown {
        event_type: None,
        raw: json!({"id": "evt-0", "msg": {"type": "session_configured", "session_id": "S-1"}}),
      }]
    );
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
  fn current_turn_completed_without_usage_still_completes() {
    let line = r#"{"type":"turn.completed"}"#;
    assert_eq!(parse_events(line), vec![AgentEvent::Completed]);
  }

  #[test]
  fn current_turn_failed_maps_to_provider_error() {
    let line = r#"{"type":"turn.failed","error":{"message":"boom"}}"#;
    assert_eq!(parse_events(line), vec![AgentEvent::Error { detail: "boom".into() }]);
  }

  #[test]
  fn current_error_event_maps_to_provider_error() {
    let line = r#"{"type":"error","message":"boom"}"#;
    assert_eq!(parse_events(line), vec![AgentEvent::Error { detail: "boom".into() }]);
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
