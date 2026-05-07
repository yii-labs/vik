//! Provider abstraction.
//!
//! Each runtime is one [`AgentAdapter`] implementation. The trait is
//! deliberately narrow — describe the CLI to spawn, decode one stdout
//! line — so adding a runtime is "two pure functions" rather than a
//! fresh subprocess scaffold. Spawning, stdin wiring, and event
//! streaming all live in [`crate::session`].
use std::sync::Arc;

mod adapters;
mod response;

use crate::config::AgentRuntime;
pub use adapters::*;
pub use response::*;

pub fn get_adapter(runtime: AgentRuntime) -> Arc<dyn AgentAdapter> {
  match runtime {
    AgentRuntime::Codex => Arc::new(CodexAdapter),
    AgentRuntime::ClaudeCode => Arc::new(ClaudeCodeAdapter),
  }
}
