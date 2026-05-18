use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(super) struct LegacyEnvelope {
  pub id: Option<String>,
  pub msg: LegacyMessage,
}

#[derive(Debug, Deserialize)]
pub(super) struct LegacyMessage {
  #[serde(rename = "type")]
  pub kind: String,
  pub session_id: Option<String>,
  pub message: Option<String>,
  pub text: Option<String>,
  pub info: Option<LegacyTokenInfo>,
  pub scope: Option<String>,
  pub remaining: Option<u64>,
  pub reset_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LegacyTokenInfo {
  pub total_token_usage: Option<TokenUsage>,
  pub input_tokens: Option<u64>,
  pub output_tokens: Option<u64>,
  pub cached_input_tokens: Option<u64>,
}

impl LegacyTokenInfo {
  pub(super) fn usage(&self) -> TokenUsage {
    self.total_token_usage.clone().unwrap_or(TokenUsage {
      input_tokens: self.input_tokens.unwrap_or_default(),
      output_tokens: self.output_tokens.unwrap_or_default(),
      cached_input_tokens: self.cached_input_tokens.unwrap_or_default(),
      reasoning_output_tokens: 0,
    })
  }
}

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
pub(super) enum CurrentEvent {
  #[serde(rename = "thread.started")]
  ThreadStarted { thread_id: String },
  #[serde(rename = "turn.started")]
  TurnStarted,
  #[serde(rename = "turn.completed")]
  TurnCompleted { usage: TokenUsage },
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
pub(super) struct ThreadItem {
  pub id: String,
  #[serde(rename = "type")]
  pub kind: String,
  pub text: Option<String>,
  pub tool: Option<String>,
  pub arguments: Option<Value>,
  pub result: Option<Value>,
  pub error: Option<Value>,
  #[serde(default)]
  pub receiver_thread_ids: Vec<String>,
  pub status: Option<String>,
}

pub(super) fn parse_legacy(value: &Value) -> Option<LegacyEnvelope> {
  serde_json::from_value(value.clone()).ok()
}

pub(super) fn parse_current(value: &Value) -> Option<CurrentEvent> {
  serde_json::from_value(value.clone()).ok()
}
