mod dispatch;
mod engine;
mod error;
mod gate;
mod lifecycle;
mod reconcile;
mod retry;
mod session_log;
mod state;
mod state_events;
mod state_snapshot;

#[cfg(test)]
mod tests;

pub use dispatch::{
    available_global_slots, available_state_slots, failure_backoff_ms, should_dispatch,
    should_dispatch_retry, sort_for_dispatch,
};
pub use engine::Orchestrator;
pub use error::OrchestratorError;
pub use gate::{DispatchDecision, DispatchGate};
pub use state::{OrchestratorState, RunningEntry};

const CONTINUATION_RETRY_MS: u64 = 1_000;
const BASE_FAILURE_RETRY_MS: u64 = 10_000;
