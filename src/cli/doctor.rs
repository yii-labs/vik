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

  let code = if diagnostics.has_errors() || (diagnostics.has_warnings() && args.strict) {
    ExitCode::FAILURE
  } else {
    ExitCode::SUCCESS
  };

  printer.print(diagnostics);
  code
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
