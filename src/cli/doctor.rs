//! `vik doctor [WORKFLOW]` — schema validation only.
//!
//! Operates on the parsed schema, not the runtime supervisor: doctor's
//! job is to surface the same errors that would block supervisor
//! construction, so it must not itself trigger that construction.

use std::process::ExitCode;

use clap::Parser;

use crate::config::diagnose::Diagnostics;
use crate::workflow::loader::LoadedWorkflowSchema;

#[derive(Debug, Parser)]
pub struct DoctorArgs {
  /// Emit machine-readable JSON instead of human-readable lines.
  #[arg(long)]
  pub json: bool,

  /// Treat warnings as errors and exit non-zero.
  #[arg(long)]
  pub strict: bool,
}

pub fn execute(loaded: LoadedWorkflowSchema, args: DoctorArgs) -> ExitCode {
  let printer = Printer { json: args.json };
  let diagnostics = loaded.schema.diagnose();

  let code = doctor_exit_code(&diagnostics, args.strict);

  printer.print(diagnostics);
  code
}

fn doctor_exit_code(diagnostics: &Diagnostics, strict: bool) -> ExitCode {
  if diagnostics.has_errors() || (diagnostics.has_warnings() && strict) {
    ExitCode::FAILURE
  } else {
    ExitCode::SUCCESS
  }
}

struct Printer {
  json: bool,
}

impl Printer {
  fn print(&self, diagnostics: Diagnostics) {
    if self.json {
      println!("{}", serde_json::to_string_pretty(&diagnostics).unwrap());
    } else {
      for line in diagnostics.errors.iter().chain(diagnostics.warnings.iter()) {
        println!("[{}] {}", line.severity, line);
      }

      println!(
        "Vik doctor shows {} error(s), {} warning(s)",
        diagnostics.errors.len(),
        diagnostics.warnings.len()
      );
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::diagnose::{Diagnostic, DiagnosticCode};

  #[test]
  fn strict_mode_fails_when_diagnostics_only_have_warnings() {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::warning("extra_field", DiagnosticCode::UnknownField));

    assert_eq!(doctor_exit_code(&diagnostics, false), ExitCode::SUCCESS);
    assert_eq!(doctor_exit_code(&diagnostics, true), ExitCode::FAILURE);
  }

  #[test]
  fn errors_fail_even_when_strict_mode_is_disabled() {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error("agents", DiagnosticCode::EmptyMap));

    assert_eq!(doctor_exit_code(&diagnostics, false), ExitCode::FAILURE);
  }
}
