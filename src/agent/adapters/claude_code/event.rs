use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::agent::AgentEvent;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ClaudeCodeEvent {
  #[serde(flatten)]
  pub(super) kind: ClaudeCodeEventKind,
  pub(super) raw: Value,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ClaudeCodeEventKind {
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
pub(super) struct ClaudeCodeContentBlock {
  #[serde(rename = "type")]
  block_type: String,
  text: Option<String>,
  raw: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ClaudeCodeUsage {
  input_tokens: u64,
  output_tokens: u64,
  cache_read_input_tokens: u64,
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

pub(super) fn map_line(line: &str) -> Result<Vec<AgentEvent>, serde_json::Error> {
  let event: ClaudeCodeEvent = serde_json::from_str(line)?;
  Ok(map_value(&event))
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

fn map_value(event: &ClaudeCodeEvent) -> Vec<AgentEvent> {
  match &event.kind {
    ClaudeCodeEventKind::System { subtype, session_id } => {
      if subtype.as_deref() != Some("init") {
        return vec![unknown_event("system", event)];
      }
      let Some(session_id) = session_id else {
        return vec![unknown_event("system", event)];
      };
      vec![AgentEvent::SessionStarted {
        session_id: session_id.clone(),
        raw: Some(event.raw.clone()),
      }]
    },
    ClaudeCodeEventKind::Assistant { content } => {
      let mut out = Vec::new();
      for block in content {
        if block.block_type == "tool_use" {
          out.push(tool_call_event(block, Some(event.raw.clone())));
        }
      }

      let text = extract_assistant_text(content);
      if !text.is_empty() {
        out.push(AgentEvent::Message {
          text,
          raw: Some(event.raw.clone()),
        });
      }

      if out.is_empty() {
        out.push(unknown_event("assistant", event));
      }
      out
    },
    ClaudeCodeEventKind::Result { usage } => {
      let mut out = Vec::new();
      if let Some(usage) = usage {
        out.push(AgentEvent::TokenUsage {
          input: usage.input_tokens,
          output: usage.output_tokens,
          cache_read: usage.cache_read_input_tokens,
          raw: Some(event.raw.clone()),
        });
      }
      out.push(AgentEvent::Completed {
        raw: Some(event.raw.clone()),
      });
      out
    },
    ClaudeCodeEventKind::User => vec![unknown_event("user", event)],
    ClaudeCodeEventKind::Unknown { event_type } => {
      tracing::debug!(
        runtime = "claude_code",
        claude_event_type = event_type.as_deref().unwrap_or("unknown"),
        "claude_code event ignored: unknown type",
      );
      vec![AgentEvent::Unknown {
        event_type: event_type.clone(),
        raw: event.raw.clone(),
      }]
    },
  }
}

fn tool_call_event(block: &ClaudeCodeContentBlock, raw: Option<Value>) -> AgentEvent {
  let id = block.raw.get("id").and_then(Value::as_str).map(ToString::to_string);
  let name = block.raw.get("name").and_then(Value::as_str).map(ToString::to_string);
  let input = block.raw.get("input").cloned();

  AgentEvent::ToolCall { id, name, input, raw }
}

fn unknown_event(event_type: &str, event: &ClaudeCodeEvent) -> AgentEvent {
  AgentEvent::Unknown {
    event_type: Some(event_type.to_string()),
    raw: event.raw.clone(),
  }
}

/// `message.content` is an array of blocks (text, tool_use, ...). Only
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
  use super::*;

  fn parse(line: &str) -> Vec<AgentEvent> {
    map_line(line).expect("fixture is valid Claude Code event")
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
        raw: Some(serde_json::json!({
          "type": "system",
          "subtype": "init",
          "session_id": "S-42",
          "model": "claude-sonnet-4-6"
        })),
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
        raw: Some(serde_json::json!({
          "type":"assistant",
          "message":{"content":[
            {"type":"text","text":"hello"},
            {"type":"text","text":"world"}
          ]}
        })),
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
        id: Some("t-1".into()),
        name: Some("Bash".into()),
        input: Some(serde_json::json!({})),
        raw: Some(serde_json::json!({
          "type":"assistant",
          "message":{"content":[{"type":"tool_use","id":"t-1","name":"Bash","input":{}}]}
        })),
      }]
    );
  }

  #[test]
  fn result_emits_usage_then_completed() {
    let line = r#"{
          "type":"result",
          "usage":{"input_tokens":11,"output_tokens":22,"cache_read_input_tokens":3}
        }"#;
    let raw = serde_json::json!({
      "type":"result",
      "usage":{"input_tokens":11,"output_tokens":22,"cache_read_input_tokens":3}
    });
    assert_eq!(
      parse(line),
      vec![
        AgentEvent::TokenUsage {
          input: 11,
          output: 22,
          cache_read: 3,
          raw: Some(raw.clone()),
        },
        AgentEvent::Completed { raw: Some(raw) },
      ]
    );
  }

  #[test]
  fn result_without_usage_still_completes() {
    let line = r#"{"type":"result"}"#;
    assert_eq!(
      parse(line),
      vec![AgentEvent::Completed {
        raw: Some(serde_json::json!({"type":"result"})),
      }]
    );
  }

  #[test]
  fn user_event_maps_to_unknown_with_raw() {
    let line = r#"{"type":"user","message":{"content":[]}}"#;
    assert_eq!(
      parse(line),
      vec![AgentEvent::Unknown {
        event_type: Some("user".into()),
        raw: serde_json::json!({"type":"user","message":{"content":[]}}),
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
      matches!(events.last(), Some(AgentEvent::Completed { .. })),
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
      AgentEvent::Message { text, .. } => Some(text.clone()),
      _ => None,
    });
    assert_eq!(
      joined_message.as_deref(),
      Some("line one\nline two"),
      "multi text blocks concatenate with a newline separator"
    );
  }
}
