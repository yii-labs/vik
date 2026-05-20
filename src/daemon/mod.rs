//! Background-service plumbing: run startup, detach, signal handling,
//! lifecycle verbs, state file.
//!
//! This layer sits between the CLI (`cli/`) and the runtime
//! (`orchestrator`, `session`, `agent`). The runner wires server and
//! orchestrator startup; lower daemon modules keep lifecycle and state
//! concerns local.
//!
//! Platform `cfg` gates live exclusively in `detach/` and `signals/`.
//! The module-surface types are platform-agnostic; Windows variants
//! return [`DetachError::PlatformUnsupported`] today.

pub mod detach;
pub mod lifecycle;
pub mod runner;
pub mod runtime;
pub mod signals;
pub mod state;

#[allow(unused_imports)]
pub use detach::{DetachError, detach};
#[allow(unused_imports)]
pub use lifecycle::{LifecycleError, RestartOutcome, StatusReport, StatusState};
#[allow(unused_imports)]
pub use runner::run;
#[allow(unused_imports)]
pub use signals::{ShutdownSignals, SignalError, install_shutdown_handler};
#[allow(unused_imports)]
pub use state::{State, StateError, StateManager};
