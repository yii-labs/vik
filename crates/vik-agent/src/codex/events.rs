use chrono::Utc;
use serde_json::Value;
use vik_core::{AgentEvent, AgentSession, TokenUsage};

pub(crate) fn agent_event(
    issue_id: String,
    event: impl Into<String>,
    session: Option<AgentSession>,
    usage: Option<TokenUsage>,
    rate_limits: Option<Value>,
    raw: Value,
) -> AgentEvent {
    let message = summarize_message(&raw);
    AgentEvent {
        issue_id,
        event: event.into(),
        timestamp: Utc::now(),
        process_id: session.as_ref().and_then(|s| s.process_id.clone()),
        session,
        usage,
        rate_limits,
        message,
        raw,
    }
}

pub(crate) fn summarize_message(message: &Value) -> Option<String> {
    message
        .pointer("/params/message")
        .or_else(|| message.pointer("/params/text"))
        .or_else(|| message.pointer("/params/turn/error/message"))
        .and_then(Value::as_str)
        .map(truncate)
}

pub(crate) fn truncate(value: &str) -> String {
    const MAX: usize = 512;
    if value.len() > MAX {
        format!("{}...", &value[..MAX])
    } else {
        value.to_string()
    }
}

pub(crate) fn extract_rate_limits(method: &str, message: &Value) -> Option<Value> {
    if method == "account/rateLimits/updated" {
        Some(message.get("params").cloned().unwrap_or(Value::Null))
    } else {
        None
    }
}

pub(crate) fn extract_usage(method: &str, message: &Value) -> Option<TokenUsage> {
    if method != "thread/tokenUsage/updated" {
        return None;
    }
    let params = message.get("params")?;
    let input = first_u64(params, &["input_tokens", "inputTokens", "input"]);
    let output = first_u64(params, &["output_tokens", "outputTokens", "output"]);
    let total = first_u64(params, &["total_tokens", "totalTokens", "total"]);
    Some(TokenUsage {
        input_tokens: input.unwrap_or(0),
        output_tokens: output.unwrap_or(0),
        total_tokens: total.unwrap_or_else(|| input.unwrap_or(0) + output.unwrap_or(0)),
    })
}

fn first_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(value) = value.get(*key).and_then(Value::as_u64) {
            return Some(value);
        }
    }
    if let Some(obj) = value.as_object() {
        for child in obj.values() {
            if let Some(value) = first_u64(child, keys) {
                return Some(value);
            }
        }
    }
    None
}
