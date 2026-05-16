//! Provider-adapter seam.
//!
//! Adapters describe what CLI to invoke and how to decode its JSONL
//! output. Subprocess spawn, stdin wiring, stdout streaming, and
//! cancellation all live in [`crate::session`]. Adapter files therefore
//! carry no `cfg(unix)`/`cfg(windows)` gates, no `libc` calls, and no
//! `tokio::process` references — adding a new provider is two pure
//! functions, not a fresh copy of the spawn scaffolding.
mod claude_code;
mod codex;

use crate::config::AgentProfileSchema;

use super::response::AgentEvent;

pub(super) use claude_code::ClaudeCodeAdapter;
pub(super) use codex::CodexAdapter;

#[derive(Debug, Clone)]
pub struct AgentCommand {
  pub program: String,
  pub args: Vec<String>,
  pub stdin: AgentStdin,
}

/// Stdin wiring requested by the adapter. Adapters never call
/// `Stdio::piped()` themselves; the runner inspects this and chooses.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum AgentStdin {
  /// Closed immediately — for providers that take the prompt on argv.
  None,
  /// Reserved for symmetry; not used by any current adapter.
  Inherit,
  /// Write the string and close the handle so the child sees EOF.
  Pipe(String),
}

/// Implementations are stateless per spawn: `build_command` is called
/// once per run and `map_line` once per provider stdout line. Adapters
/// keep provider-specific parsing local, then return provider-neutral
/// session events. Unknown future shapes should map to
/// `AgentEvent::Unknown` with the original JSON instead of failing the
/// stream.
pub trait AgentAdapter: Send + Sync {
  fn build_command(&self, profile: &AgentProfileSchema, prompt: String) -> AgentCommand;

  /// Decode one provider JSONL line into provider-neutral session
  /// events. Returning multiple events fans one line out (e.g. Claude's
  /// `result` line yields both `TokenUsage` and `Completed`).
  fn map_line(&self, line: &str) -> Result<Vec<AgentEvent>, serde_json::Error>;
}

/// Flatten the YAML `args` map into a flat CLI token list. Booleans
/// follow GNU conventions: `true` becomes a no-value flag, `false`
/// drops the flag entirely. Sequences are joined with `,` because every
/// supported provider accepts comma lists in one argument and that
/// keeps logs readable.
fn build_extra_args(map: &serde_yaml::Mapping) -> Vec<String> {
  map
    .iter()
    .flat_map(|(k, v)| {
      let k = k.as_str()?;

      let kv = match v {
        serde_yaml::Value::String(s) => Some((k, s.clone())),
        serde_yaml::Value::Number(n) => Some((k, n.to_string())),
        serde_yaml::Value::Bool(true) => return Some(vec![k.to_string()]),
        serde_yaml::Value::Bool(false) => return Some(Vec::new()),
        serde_yaml::Value::Sequence(seq) => Some((
          k,
          seq
            .iter()
            .filter_map(|item| item.as_str().map(|s| s.to_string()))
            .collect::<Vec<String>>()
            .join(","),
        )),

        _ => return None,
      };

      if let Some((k, v)) = kv {
        Some(vec![k.to_string(), v])
      } else {
        None
      }
    })
    .flatten()
    .collect()
}
