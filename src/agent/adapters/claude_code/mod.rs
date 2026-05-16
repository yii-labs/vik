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

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::config::AgentProfileSchema;

use super::{AgentAdapter, AgentCommand, AgentEvent, AgentStdin, build_extra_args};

const CLAUDE_PROGRAM: &str = "claude";

#[derive(Debug, Clone)]
pub struct ClaudeCodeAdapter;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaudeCodeEvent {
  #[serde(flatten)]
  pub kind: ClaudeCodeEventKind,
  pub raw: Value,
}

impl<'de> Deserialize<'de> for ClaudeCodeEvent {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: Deserializer<'de>,
  {
    let value = Value::deserialize(deserializer)?;
    if value.get("raw").is_some() && value.get("kind").is_some() {
      #[derive(Deserialize)]
      struct StoredClaudeCodeEvent {
        #[serde(flatten)]
        kind: ClaudeCodeEventKind,
        raw: Value,
      }

      if let Ok(stored) = serde_json::from_value::<StoredClaudeCodeEvent>(value.clone()) {
        return Ok(Self {
          kind: stored.kind,
          raw: stored.raw,
        });
      }
    }

    Ok(Self::from_provider_value(value))
  }
}

impl ClaudeCodeEvent {
  fn from_provider_value(raw: Value) -> Self {
    let kind = claude_code_event_kind(&raw);
    Self { kind, raw }
  }
}

impl From<ClaudeCodeEvent> for AgentEvent {
  fn from(event: ClaudeCodeEvent) -> Self {
    AgentEvent::ClaudeCodeProviderEvent { event }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaudeCodeEventKind {
  System {
    subtype: Option<String>,
    session_id: Option<String>,
  },
  Assistant {
    content: Vec<ClaudeCodeContentBlock>,
  },
  User,
  Result {
    usage: Option<ClaudeCodeUsage>,
  },
  Unknown {
    event_type: Option<String>,
  },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeContentBlock {
  #[serde(rename = "type")]
  pub block_type: String,
  pub text: Option<String>,
  pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeUsage {
  pub input_tokens: u64,
  pub output_tokens: u64,
  pub cache_read_input_tokens: u64,
}

impl ClaudeCodeUsage {
  fn from_value(value: &Value) -> Self {
    Self {
      input_tokens: value.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
      output_tokens: value.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
      cache_read_input_tokens: value.get("cache_read_input_tokens").and_then(Value::as_u64).unwrap_or(0),
    }
  }
}

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

  fn provider_event(&self, line: &str) -> Result<AgentEvent, serde_json::Error> {
    let event: ClaudeCodeEvent = serde_json::from_str(line)?;
    Ok(event.into())
  }

  fn map_event(&self, event: &AgentEvent) -> Vec<AgentEvent> {
    match event {
      AgentEvent::ClaudeCodeProviderEvent { event } => map_value(event),
      _ => Vec::new(),
    }
  }
}

fn claude_code_event_kind(value: &Value) -> ClaudeCodeEventKind {
  let Some(ty) = value.get("type").and_then(Value::as_str) else {
    return ClaudeCodeEventKind::Unknown { event_type: None };
  };

  match ty {
    "system" => ClaudeCodeEventKind::System {
      subtype: value.get("subtype").and_then(Value::as_str).map(ToString::to_string),
      session_id: value.get("session_id").and_then(Value::as_str).map(ToString::to_string),
    },
    "assistant" => ClaudeCodeEventKind::Assistant {
      content: assistant_content(value),
    },
    "user" => ClaudeCodeEventKind::User,
    "result" => ClaudeCodeEventKind::Result {
      usage: value.get("usage").map(ClaudeCodeUsage::from_value),
    },
    other => ClaudeCodeEventKind::Unknown {
      event_type: Some(other.to_string()),
    },
  }
}

fn assistant_content(value: &Value) -> Vec<ClaudeCodeContentBlock> {
  value
    .get("message")
    .and_then(|message| message.get("content"))
    .and_then(Value::as_array)
    .map(|content| {
      content
        .iter()
        .filter_map(|block| {
          block
            .get("type")
            .and_then(Value::as_str)
            .map(|block_type| ClaudeCodeContentBlock {
              block_type: block_type.to_string(),
              text: block.get("text").and_then(Value::as_str).map(ToString::to_string),
              raw: block.clone(),
            })
        })
        .collect()
    })
    .unwrap_or_default()
}

pub(super) fn map_value(event: &ClaudeCodeEvent) -> Vec<AgentEvent> {
  match &event.kind {
    ClaudeCodeEventKind::System { subtype, session_id } => {
      // Only the `init` subtype reports session_id; other system
      // subtypes (config dumps, hook outputs) carry data we do not
      // surface as events.
      if subtype.as_deref() != Some("init") {
        return Vec::new();
      }
      let Some(session_id) = session_id else {
        return Vec::new();
      };
      vec![AgentEvent::SessionStarted {
        session_id: session_id.clone(),
      }]
    },
    ClaudeCodeEventKind::Assistant { content } => {
      let text = extract_assistant_text(content);
      // Tool-only turns have no semantic `Message`; the session still
      // writes the typed provider event before this mapper runs.
      if text.is_empty() {
        return Vec::new();
      }
      vec![AgentEvent::Message { text }]
    },
    ClaudeCodeEventKind::Result { usage } => {
      let mut out = Vec::new();
      if let Some(usage) = usage {
        out.push(AgentEvent::TokenUsage {
          input: usage.input_tokens,
          output: usage.output_tokens,
          cache_read: usage.cache_read_input_tokens,
        });
      }
      out.push(AgentEvent::Completed);
      out
    },
    ClaudeCodeEventKind::User => Vec::new(),
    ClaudeCodeEventKind::Unknown { event_type } => {
      tracing::debug!(
        runtime = "claude_code",
        claude_event_type = event_type.as_deref().unwrap_or("unknown"),
        "claude_code event ignored: unknown type",
      );
      Vec::new()
    },
  }
}

/// `message.content` is an array of blocks (text, tool_use, …). Only
/// `text` blocks are user-facing; concatenate them with newlines so a
/// multi-block reply still reads naturally in `last_message`.
fn extract_assistant_text(content: &[ClaudeCodeContentBlock]) -> String {
  let mut buf = String::new();
  for block in content {
    if block.block_type == "text"
      && let Some(text) = &block.text
    {
      if !buf.is_empty() {
        buf.push('\n');
      }
      buf.push_str(text.as_str());
    }
  }
  buf
}

#[cfg(test)]
mod tests {
  use crate::config::AgentRuntime;

  use super::*;
  fn parse(line: &str) -> Vec<AgentEvent> {
    let event: ClaudeCodeEvent = serde_json::from_str(line).expect("fixture is valid Claude Code event");
    map_value(&event)
  }

  #[test]
  fn provider_event_kind_recognizes_system_and_assistant_details() {
    let system: ClaudeCodeEvent =
      serde_json::from_str(r#"{"type":"system","subtype":"init","session_id":"S-42"}"#).expect("fixture");
    assert_eq!(
      system.kind,
      ClaudeCodeEventKind::System {
        subtype: Some("init".into()),
        session_id: Some("S-42".into()),
      }
    );

    let assistant: ClaudeCodeEvent =
      serde_json::from_str(r#"{"type":"assistant","message":{"content":[{"type":"tool_use"},{"type":"text"}]}}"#)
        .expect("fixture");
    assert_eq!(
      assistant.kind,
      ClaudeCodeEventKind::Assistant {
        content: vec![
          ClaudeCodeContentBlock {
            block_type: "tool_use".into(),
            text: None,
            raw: serde_json::json!({"type": "tool_use"}),
          },
          ClaudeCodeContentBlock {
            block_type: "text".into(),
            text: None,
            raw: serde_json::json!({"type": "text"}),
          },
        ],
      }
    );
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
