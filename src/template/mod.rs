//! MiniJinja rendering and prompt-local command expansion.
//!
//! Two distinct surfaces share the renderer:
//!
//! - Hooks render Jinja only — no `!`exec(...)`` substitution.
//! - Prompts render Jinja first, then expand `!`exec(...)`` and ``exec(...)``
//!   markers as a second pass. The shell commands run with a 30s timeout.

mod context;
mod jinja;
mod prompt;

use thiserror::Error;

pub use context::{Context, StageContext, issue_value};
pub use jinja::*;
pub use prompt::*;

use crate::shell::CommandExecError;

#[derive(Debug, Error)]
pub enum TemplateError {
  #[error("template render failed: {0}")]
  Render(#[from] minijinja::Error),

  /// `stderr_tail` would belong here too, but the prompt expander is a
  /// thin wrapper and the lower layer captures stderr inside the
  /// `CommandExecError`. Bound any new tail field with a byte cap so
  /// log output stays readable.
  #[error("prompt injection command `{command}` failed: {source}")]
  PromptCommandFailed {
    command: String,
    #[source]
    source: CommandExecError,
  },

  #[error(transparent)]
  Io(#[from] std::io::Error),
}
