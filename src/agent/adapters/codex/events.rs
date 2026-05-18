use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Deserialize)]
pub(super) struct TokenUsage {
  #[serde(default)]
  pub input_tokens: u64,
  #[serde(default)]
  pub output_tokens: u64,
  #[serde(default)]
  pub cached_input_tokens: u64,
  #[allow(dead_code)]
  #[serde(default)]
  pub reasoning_output_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(super) enum CodexEvent {
  #[serde(rename = "thread.started")]
  ThreadStarted { thread_id: String },
  #[serde(rename = "turn.started")]
  TurnStarted,
  #[serde(rename = "turn.completed")]
  TurnCompleted { usage: Option<TokenUsage> },
  #[serde(rename = "turn.failed")]
  TurnFailed { error: ThreadError },
  #[serde(rename = "item.started")]
  ItemStarted { item: ThreadItem },
  #[serde(rename = "item.updated")]
  ItemUpdated,
  #[serde(rename = "item.completed")]
  ItemCompleted { item: ThreadItem },
  #[serde(rename = "error")]
  Error { message: String },
  #[serde(other)]
  Unknown,
}

#[derive(Debug, Deserialize)]
pub(super) struct ThreadError {
  #[serde(default)]
  pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ThreadItem {
  AgentMessage {
    #[allow(dead_code)]
    id: String,
    text: Option<String>,
  },
  CommandExecution {
    id: String,
    #[serde(flatten)]
    fields: Map<String, Value>,
  },
  McpToolCall {
    id: String,
    tool: Option<String>,
    arguments: Option<Value>,
    result: Option<Value>,
    error: Option<Value>,
  },
  CollabToolCall {
    id: String,
    tool: Option<String>,
    status: Option<String>,
    #[serde(default)]
    receiver_thread_ids: Vec<String>,
  },
  #[serde(other)]
  Unknown,
}

impl ThreadItem {
  pub(super) fn command_execution_payload(id: &str, fields: &Map<String, Value>) -> Value {
    let mut payload = fields.clone();
    payload.insert("id".into(), Value::String(id.into()));
    payload.insert("type".into(), Value::String("command_execution".into()));
    Value::Object(payload)
  }
}

#[derive(Debug, Deserialize)]
struct EventType {
  #[serde(rename = "type")]
  kind: String,
}

pub(super) fn parse(value: &Value) -> Result<CodexEvent, serde_json::Error> {
  serde_json::from_value(value.clone())
}

pub(super) fn event_type(value: &Value) -> Option<String> {
  serde_json::from_value::<EventType>(value.clone()).ok().map(|event| event.kind)
}
