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
      input_tokens: self.input_tokens,
      output_tokens: self.output_tokens,
      cached_input_tokens: self.cached_input_tokens,
      reasoning_output_tokens: None,
    })
  }
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct TokenUsage {
  pub input_tokens: Option<u64>,
  pub output_tokens: Option<u64>,
  pub cached_input_tokens: Option<u64>,
  #[allow(dead_code)]
  pub reasoning_output_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(super) enum CurrentEvent {
  #[serde(rename = "thread.started")]
  ThreadStarted { thread_id: Option<String> },
  #[serde(rename = "turn.started")]
  TurnStarted,
  #[serde(rename = "turn.completed")]
  TurnCompleted { usage: Option<TokenUsage> },
  #[serde(rename = "turn.failed")]
  TurnFailed { error: Option<ThreadError> },
  #[serde(rename = "item.started")]
  ItemStarted { item: Option<ThreadItem> },
  #[serde(rename = "item.updated")]
  ItemUpdated,
  #[serde(rename = "item.completed")]
  ItemCompleted { item: Option<ThreadItem> },
  #[serde(rename = "collabAgentToolCall")]
  LegacyCollabAgentToolCall(LegacyCollabAgentToolCall),
  #[serde(rename = "error")]
  Error { message: Option<String> },
  #[serde(other)]
  Unknown,
}

#[derive(Debug, Deserialize)]
pub(super) struct ThreadError {
  pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ThreadItem {
  pub id: Option<String>,
  #[serde(rename = "type")]
  pub kind: Option<String>,
  pub text: Option<String>,
  pub tool: Option<String>,
  pub arguments: Option<Value>,
  pub result: Option<Value>,
  pub error: Option<Value>,
  pub receiver_thread_ids: Option<Vec<String>>,
  pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LegacyCollabAgentToolCall {
  pub id: Option<String>,
  pub tool: Option<String>,
  pub status: Option<String>,
  pub receiver_thread_ids: Option<Vec<String>>,
}

pub(super) fn parse_legacy(value: &Value) -> Option<LegacyEnvelope> {
  serde_json::from_value(value.clone()).ok()
}

pub(super) fn parse_current(value: &Value) -> Option<CurrentEvent> {
  serde_json::from_value(value.clone()).ok()
}
