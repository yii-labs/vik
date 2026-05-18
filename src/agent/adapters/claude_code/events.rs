use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ClaudeEvent {
  System(SystemEvent),
  Assistant(MessageEvent),
  User(MessageEvent),
  Result(ResultEvent),
  #[serde(other)]
  Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub(super) enum SystemEvent {
  Init {
    session_id: String,
  },
  #[serde(other)]
  Unknown,
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageEvent {
  pub message: ClaudeMessage,
}

#[derive(Debug, Deserialize)]
pub(super) struct ClaudeMessage {
  #[serde(default)]
  pub content: MessageContent,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum MessageContent {
  Blocks(Vec<ContentBlock>),
  #[allow(dead_code)]
  Text(String),
}

impl Default for MessageContent {
  fn default() -> Self {
    Self::Text(String::new())
  }
}

impl MessageContent {
  pub(super) fn blocks(&self) -> &[ContentBlock] {
    match self {
      Self::Blocks(blocks) => blocks,
      Self::Text(_) => &[],
    }
  }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ContentBlock {
  Text {
    text: String,
  },
  ToolUse {
    id: String,
    name: ToolName,
    input: Value,
  },
  ToolResult {
    tool_use_id: String,
    content: Value,
    #[allow(dead_code)]
    is_error: Option<bool>,
  },
  #[serde(other)]
  Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum ToolName {
  Known(KnownToolName),
  Other(String),
}

impl ToolName {
  pub(super) fn as_str(&self) -> &str {
    match self {
      Self::Known(name) => name.as_str(),
      Self::Other(name) => name.as_str(),
    }
  }

  pub(super) fn is_subagent(&self) -> bool {
    matches!(self, Self::Known(KnownToolName::Agent | KnownToolName::Task))
  }
}

#[derive(Debug, Clone, Deserialize)]
pub(super) enum KnownToolName {
  Agent,
  Task,
}

impl KnownToolName {
  fn as_str(&self) -> &'static str {
    match self {
      Self::Agent => "Agent",
      Self::Task => "Task",
    }
  }
}

#[derive(Debug, Deserialize)]
pub(super) struct ResultEvent {
  pub usage: Option<TokenUsage>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TokenUsage {
  #[serde(default)]
  pub input_tokens: u64,
  #[serde(default)]
  pub output_tokens: u64,
  #[serde(default)]
  pub cache_read_input_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct EventType {
  #[serde(rename = "type")]
  kind: String,
}

pub(super) fn parse(value: &Value) -> Result<ClaudeEvent, serde_json::Error> {
  serde_json::from_value(value.clone())
}

pub(super) fn event_type(value: &Value) -> Option<String> {
  serde_json::from_value::<EventType>(value.clone()).ok().map(|event| event.kind)
}
