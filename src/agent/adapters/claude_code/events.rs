use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(super) enum ClaudeEvent {
  #[serde(rename = "system")]
  System(SystemEvent),
  #[serde(rename = "assistant")]
  Assistant(MessageEvent),
  #[serde(rename = "user")]
  User(MessageEvent),
  #[serde(rename = "result")]
  Result(ResultEvent),
  #[serde(other)]
  Unknown,
}

#[derive(Debug, Deserialize)]
pub(super) struct SystemEvent {
  pub subtype: Option<String>,
  pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageEvent {
  pub message: Option<ClaudeMessage>,
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
#[serde(tag = "type")]
pub(super) enum ContentBlock {
  #[serde(rename = "text")]
  Text { text: Option<String> },
  #[serde(rename = "tool_use")]
  ToolUse {
    id: Option<String>,
    name: Option<String>,
    input: Option<Value>,
  },
  #[serde(rename = "tool_result")]
  ToolResult {
    tool_use_id: Option<String>,
    content: Option<Value>,
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
  pub input_tokens: Option<u64>,
  pub output_tokens: Option<u64>,
  pub cache_read_input_tokens: Option<u64>,
}

pub(super) fn parse(value: &Value) -> Option<ClaudeEvent> {
  serde_json::from_value(value.clone()).ok()
}
