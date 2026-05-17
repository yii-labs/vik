//! Claude Code provider adapter.
//!
//! Command shape:
//!
//! ```text
//! claude --verbose --output-format stream-json --model <model> -p [profile.args...]
//! ```
//!
//! The prompt is piped on stdin (the position of `-p` is just where
//! the rest of `profile.args` are appended). Claude's NDJSON uses a
//! flat `type` discriminant; one `result` line carries both usage
//! totals and stream completion, so `map_event` may fan out into two
//! [`AgentEvent`]s for that single line.

mod events;

use serde_json::Value;

use crate::config::AgentProfileSchema;

use super::{AgentAdapter, AgentCommand, AgentEvent, AgentStdin, ToolCallPhase, build_extra_args};
use events::{ClaudeEvent, ContentBlock, MessageEvent};

const CLAUDE_PROGRAM: &str = "claude";

#[derive(Debug, Clone)]
pub struct ClaudeCodeAdapter;

impl AgentAdapter for ClaudeCodeAdapter {
  fn build_command(&self, profile: &AgentProfileSchema, prompt: String) -> AgentCommand {
    let mut args: Vec<String> = vec![
      "--verbose".into(),
      "--output-format".into(),
      "stream-json".into(),
      "--model".into(),
      profile.model.clone(),
      "-p".into(),
    ];

    args.extend(build_extra_args(&profile.args));

    AgentCommand {
      program: CLAUDE_PROGRAM.into(),
      args,
      stdin: AgentStdin::Pipe(prompt),
    }
  }

  fn map_event(&self, value: Value) -> Vec<AgentEvent> {
    map_value(&value)
  }
}

pub(super) fn map_value(value: &Value) -> Vec<AgentEvent> {
  let Some(event) = events::parse(value) else {
    return vec![unknown_event(value)];
  };

  match event {
    ClaudeEvent::System(event) => {
      // Only the `init` subtype reports session_id; other system
      // subtypes (config dumps, hook outputs) carry data we do not
      // surface as events.
      if event.subtype.as_deref() != Some("init") {
        return vec![unknown_event(value)];
      }
      event
        .session_id
        .filter(|session_id| !session_id.is_empty())
        .map(|session_id| vec![AgentEvent::SessionStarted { session_id }])
        .unwrap_or_else(|| vec![unknown_event(value)])
    },
    ClaudeEvent::Assistant(event) => {
      let mut events = Vec::new();
      let text = extract_assistant_text(&event);
      if !text.is_empty() {
        events.push(AgentEvent::Message { text });
      }
      events.extend(extract_tool_uses(value, &event));
      if events.is_empty() {
        events.push(unknown_event(value));
      }
      events
    },
    ClaudeEvent::User(event) => {
      let events = extract_tool_results(value, &event);
      if events.is_empty() {
        vec![unknown_event(value)]
      } else {
        events
      }
    },
    ClaudeEvent::Result(event) => {
      let mut out = Vec::new();
      if let Some(usage) = event.usage {
        let input = usage.input_tokens.unwrap_or(0);
        let output = usage.output_tokens.unwrap_or(0);
        // Note the field-name divergence from Codex: Claude reports
        // `cache_read_input_tokens`, Codex reports `cached_input_tokens`.
        let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
        out.push(AgentEvent::TokenUsage {
          input,
          output,
          cache_read,
        });
      }
      out.push(AgentEvent::Completed);
      out
    },
    ClaudeEvent::Unknown => {
      tracing::debug!(
        runtime = "claude_code",
        claude_event_type = event_type(value).unwrap_or("unknown"),
        "claude_code event retained as unknown",
      );
      vec![unknown_event(value)]
    },
  }
}

/// `message.content` is an array of blocks (text, tool_use, …). Only
/// `text` blocks are user-facing; concatenate them with newlines so a
/// multi-block reply still reads naturally in `last_message`.
fn extract_assistant_text(event: &MessageEvent) -> String {
  let mut buf = String::new();
  for block in content_blocks(event) {
    if let ContentBlock::Text { text: Some(text) } = block {
      if !buf.is_empty() {
        buf.push('\n');
      }
      buf.push_str(text.as_str());
    }
  }
  buf
}

fn extract_tool_uses(value: &Value, event: &MessageEvent) -> Vec<AgentEvent> {
  content_blocks(event)
    .iter()
    .filter_map(|block| match block {
      ContentBlock::ToolUse { id, name, .. } if is_subagent_tool(name.as_deref()) => Some(AgentEvent::Subagent {
        call_id: id.clone(),
        action: name.clone().unwrap_or_else(|| "unknown".into()),
        status: None,
        target_ids: Vec::new(),
        raw: value.clone(),
      }),
      ContentBlock::ToolUse { id, name, input } => Some(AgentEvent::ToolCall {
        call_id: id.clone(),
        name: name.clone(),
        phase: ToolCallPhase::Request,
        input: input.clone(),
        output: None,
        raw: value.clone(),
      }),
      _ => None,
    })
    .collect()
}

fn extract_tool_results(value: &Value, event: &MessageEvent) -> Vec<AgentEvent> {
  content_blocks(event)
    .iter()
    .filter_map(|block| match block {
      ContentBlock::ToolResult {
        tool_use_id, content, ..
      } => Some(AgentEvent::ToolCall {
        call_id: tool_use_id.clone(),
        name: None,
        phase: ToolCallPhase::Result,
        input: None,
        output: content.clone(),
        raw: value.clone(),
      }),
      _ => None,
    })
    .collect()
}

fn unknown_event(value: &Value) -> AgentEvent {
  AgentEvent::Unknown {
    event_type: event_type(value).map(str::to_string),
    raw: value.clone(),
  }
}

fn content_blocks(event: &MessageEvent) -> &[ContentBlock] {
  event.message.as_ref().map(|message| message.content.blocks()).unwrap_or(&[])
}

fn is_subagent_tool(name: Option<&str>) -> bool {
  matches!(name, Some("Agent" | "Task"))
}

fn event_type(value: &Value) -> Option<&str> {
  value.get("type").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
  use crate::config::AgentRuntime;
  use serde_json::json;

  use super::*;
  fn parse(line: &str) -> Vec<AgentEvent> {
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");
    map_value(&value)
  }

  #[test]
  fn system_init_maps_to_session_started() {
    let line = r#"{"type":"system","subtype":"init","session_id":"S-42","model":"claude-sonnet-4-6"}"#;
    assert_eq!(
      parse(line),
      vec![AgentEvent::SessionStarted {
        session_id: "S-42".into(),
      }]
    );
  }

  #[test]
  fn assistant_text_blocks_concatenate() {
    let line = r#"{
          "type":"assistant",
          "message":{"content":[
            {"type":"text","text":"hello"},
            {"type":"text","text":"world"}
          ]}
        }"#;
    assert_eq!(
      parse(line),
      vec![AgentEvent::Message {
        text: "hello\nworld".into(),
      }]
    );
  }

  #[test]
  fn assistant_tool_only_maps_to_tool_call() {
    let line = r#"{
          "type":"assistant",
          "message":{"content":[{"type":"tool_use","id":"t-1","name":"Bash","input":{}}]}
        }"#;
    assert_eq!(
      parse(line),
      vec![AgentEvent::ToolCall {
        call_id: Some("t-1".into()),
        name: Some("Bash".into()),
        phase: ToolCallPhase::Request,
        input: Some(json!({})),
        output: None,
        raw: json!({
          "type": "assistant",
          "message": {
            "content": [
              {
                "type": "tool_use",
                "id": "t-1",
                "name": "Bash",
                "input": {}
              }
            ]
          }
        }),
      }]
    );
  }

  #[test]
  fn result_emits_usage_then_completed() {
    let line = r#"{
          "type":"result",
          "usage":{"input_tokens":11,"output_tokens":22,"cache_read_input_tokens":3}
        }"#;
    assert_eq!(
      parse(line),
      vec![
        AgentEvent::TokenUsage {
          input: 11,
          output: 22,
          cache_read: 3,
        },
        AgentEvent::Completed,
      ]
    );
  }

  #[test]
  fn result_without_usage_still_completes() {
    let line = r#"{"type":"result"}"#;
    assert_eq!(parse(line), vec![AgentEvent::Completed]);
  }

  #[test]
  fn user_tool_result_maps_to_tool_call_result() {
    let line =
      r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t-1","content":"file.txt"}]}}"#;
    assert_eq!(
      parse(line),
      vec![AgentEvent::ToolCall {
        call_id: Some("t-1".into()),
        name: None,
        phase: ToolCallPhase::Result,
        input: None,
        output: Some(json!("file.txt")),
        raw: json!({
          "type": "user",
          "message": {
            "content": [
              {
                "type": "tool_result",
                "tool_use_id": "t-1",
                "content": "file.txt"
              }
            ]
          }
        }),
      }]
    );
  }

  #[test]
  fn future_event_maps_to_unknown_event_with_raw_payload() {
    let line = r#"{"type":"future_event_kind","payload":{"ok":true}}"#;
    assert_eq!(
      parse(line),
      vec![AgentEvent::Unknown {
        event_type: Some("future_event_kind".into()),
        raw: json!({
          "type": "future_event_kind",
          "payload": {"ok": true}
        }),
      }]
    );
  }

  #[test]
  fn task_tool_use_maps_to_subagent_event() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"task-1","name":"Task","input":{"description":"scan docs","prompt":"Find docs drift","subagent_type":"general-purpose"}}]}}"#;
    assert_eq!(
      parse(line),
      vec![AgentEvent::Subagent {
        call_id: Some("task-1".into()),
        action: "Task".into(),
        status: None,
        target_ids: Vec::new(),
        raw: json!({
          "type": "assistant",
          "message": {
            "content": [
              {
                "type": "tool_use",
                "id": "task-1",
                "name": "Task",
                "input": {
                  "description": "scan docs",
                  "prompt": "Find docs drift",
                  "subagent_type": "general-purpose"
                }
              }
            ]
          }
        }),
      }]
    );
  }

  #[test]
  fn agent_tool_use_maps_to_subagent_event() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"agent-1","name":"Agent","input":{"description":"scan docs","prompt":"Find docs drift","agent_type":"general-purpose"}}]}}"#;
    assert_eq!(
      parse(line),
      vec![AgentEvent::Subagent {
        call_id: Some("agent-1".into()),
        action: "Agent".into(),
        status: None,
        target_ids: Vec::new(),
        raw: json!({
          "type": "assistant",
          "message": {
            "content": [
              {
                "type": "tool_use",
                "id": "agent-1",
                "name": "Agent",
                "input": {
                  "description": "scan docs",
                  "prompt": "Find docs drift",
                  "agent_type": "general-purpose"
                }
              }
            ]
          }
        }),
      }]
    );
  }

  #[test]
  fn happy_path_fixture_yields_full_session() {
    let path = concat!(
      env!("CARGO_MANIFEST_DIR"),
      "/tests/fixtures/agent_events/claude_code/happy_path.jsonl"
    );
    let body = std::fs::read_to_string(path).expect("fixture present");
    let mut events: Vec<AgentEvent> = Vec::new();
    for line in body.lines() {
      events.extend(parse(line));
    }
    assert!(
      matches!(events[0], AgentEvent::SessionStarted { .. }),
      "first event must be SessionStarted"
    );
    let messages = events.iter().filter(|e| matches!(e, AgentEvent::Message { .. })).count();
    assert_eq!(messages, 2, "two text-only assistant turns");
    assert!(
      events.iter().any(|e| matches!(e, AgentEvent::TokenUsage { .. })),
      "result event yields TokenUsage"
    );
    assert!(
      matches!(events.last(), Some(AgentEvent::Completed)),
      "stream terminates with Completed"
    );
  }

  #[test]
  fn multi_text_blocks_fixture_joins_text() {
    let path = concat!(
      env!("CARGO_MANIFEST_DIR"),
      "/tests/fixtures/agent_events/claude_code/multi_text_blocks.jsonl"
    );
    let body = std::fs::read_to_string(path).expect("fixture present");
    let mut events: Vec<AgentEvent> = Vec::new();
    for line in body.lines() {
      events.extend(parse(line));
    }
    let joined_message = events.iter().find_map(|e| match e {
      AgentEvent::Message { text } => Some(text.clone()),
      _ => None,
    });
    assert_eq!(
      joined_message.as_deref(),
      Some("line one\nline two"),
      "multi text blocks concatenate with a newline separator"
    );
  }

  #[test]
  fn command_contains_expected_flags_and_closed_stdin() {
    let adapter = ClaudeCodeAdapter;
    let profile =
      AgentProfileSchema::new(AgentRuntime::ClaudeCode, "opus".into()).with_args(serde_yaml::Mapping::from_iter([
        (
          serde_yaml::Value::String("--permission-mode".into()),
          serde_yaml::Value::String("plan".into()),
        ),
        (
          serde_yaml::Value::String("--allowed-tools".into()),
          serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::String("Edit".into()),
            serde_yaml::Value::String("Read".into()),
          ]),
        ),
        (
          serde_yaml::Value::String("--effort".into()),
          serde_yaml::Value::String("high".into()),
        ),
      ]));

    let cmd = adapter.build_command(&profile, "hello".into());
    assert_eq!(cmd.program, "claude");
    assert_eq!(
      cmd.args,
      vec![
        "--verbose",
        "--output-format",
        "stream-json",
        "--model",
        "opus",
        "-p",
        "--permission-mode",
        "plan",
        "--allowed-tools",
        "Edit,Read",
        "--effort",
        "high"
      ]
    );
    assert!(matches!(cmd.stdin, AgentStdin::Pipe(ref s) if s == "hello"));
  }
}
