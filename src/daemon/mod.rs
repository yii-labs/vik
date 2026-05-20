//! Background-service plumbing: detach, signal handling, lifecycle
//! verbs, state file.
//!
//! This layer sits between the CLI (`cli/`) and the runtime
//! (`orchestrator`, `session`, `agent`). It depends on `workflow` and
//! `logging` only — never up the stack — so a daemon-side change does
//! not pull in agent or orchestrator concerns.
//!
//! Platform `cfg` gates live exclusively in `detach/` and `signals/`.
//! The module-surface types are platform-agnostic; Windows variants
//! return [`DetachError::PlatformUnsupported`] today.

pub mod detach;
pub mod lifecycle;
pub mod runtime;
pub mod signals;
pub mod state;

#[allow(unused_imports)]
pub use detach::{DetachError, detach};
#[allow(unused_imports)]
pub use lifecycle::{LifecycleError, RestartOutcome, StatusReport, StatusState};
#[allow(unused_imports)]
pub use signals::{ShutdownSignals, SignalError, install_shutdown_handler};
#[allow(unused_imports)]
pub use state::{State, StateError};
