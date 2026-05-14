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

use serde_json::Value;

use crate::config::AgentProfileSchema;

use super::{AgentAdapter, AgentCommand, AgentEvent, AgentStdin, build_extra_args};

const CLAUDE_PROGRAM: &str = "claude";

#[derive(Debug, Clone)]
pub struct ClaudeCodeAdapter;

impl AgentAdapter for ClaudeCodeAdapter {
  fn runtime_name(&self) -> &'static str {
    "claude_code"
  }

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
  let Some(ty) = value.get("type").and_then(Value::as_str) else {
    return Vec::new();
  };

  match ty {
    "system" => {
      // Only the `init` subtype reports session_id; other system
      // subtypes (config dumps, hook outputs) carry data we do not
      // surface as events.
      let subtype = value.get("subtype").and_then(Value::as_str);
      if subtype != Some("init") {
        return Vec::new();
      }
      let session_id = value.get("session_id").and_then(Value::as_str).unwrap_or("").to_string();
      if session_id.is_empty() {
        return Vec::new();
      }
      vec![AgentEvent::SessionStarted { session_id }]
    },
    "assistant" => {
      let text = extract_assistant_text(value);
      // Tool-only turns have no semantic `Message`; the session still
      // writes the raw provider event before this mapper runs.
      if text.is_empty() {
        return Vec::new();
      }
      vec![AgentEvent::Message { text }]
    },
    "result" => {
      let mut out = Vec::new();
      if let Some(usage) = value.get("usage") {
        let input = usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
        let output = usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
        // Note the field-name divergence from Codex: Claude reports
        // `cache_read_input_tokens`, Codex reports `cached_input_tokens`.
        let cache_read = usage.get("cache_read_input_tokens").and_then(Value::as_u64).unwrap_or(0);
        out.push(AgentEvent::TokenUsage {
          input,
          output,
          cache_read,
        });
      }
      out.push(AgentEvent::Completed);
      out
    },
    other => {
      tracing::debug!(
        runtime = "claude_code",
        claude_event_type = other,
        "claude_code event ignored: unknown type",
      );
      Vec::new()
    },
  }
}

/// `message.content` is an array of blocks (text, tool_use, …). Only
/// `text` blocks are user-facing; concatenate them with newlines so a
/// multi-block reply still reads naturally in `last_message`.
fn extract_assistant_text(value: &Value) -> String {
  let Some(content) = value.get("message").and_then(|m| m.get("content")).and_then(Value::as_array) else {
    return String::new();
  };
  let mut buf = String::new();
  for block in content {
    if block.get("type").and_then(Value::as_str) == Some("text")
      && let Some(text) = block.get("text").and_then(Value::as_str)
    {
      if !buf.is_empty() {
        buf.push('\n');
      }
      buf.push_str(text);
    }
  }
  buf
}

#[cfg(test)]
mod tests {
  use crate::config::AgentRuntime;

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
  fn assistant_tool_only_has_no_semantic_message() {
    let line = r#"{
          "type":"assistant",
          "message":{"content":[{"type":"tool_use","id":"t-1","name":"Bash","input":{}}]}
        }"#;
    assert!(parse(line).is_empty());
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
  fn user_event_has_no_semantic_mapping() {
    let line = r#"{"type":"user","message":{"content":[]}}"#;
    assert!(parse(line).is_empty());
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
