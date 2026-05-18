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
pub(super) struct SystemEvent {
  pub subtype: String,
  pub session_id: String,
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
    name: String,
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

pub(super) fn parse(value: &Value) -> Option<ClaudeEvent> {
  serde_json::from_value(value.clone()).ok()
}
