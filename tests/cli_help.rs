//! CLI help regressions.
//!
//! These tests keep operator-facing command shapes stable.

use std::path::PathBuf;
use std::process::Command;

fn vik_bin() -> PathBuf {
  PathBuf::from(env!("CARGO_BIN_EXE_vik"))
}

#[test]
fn top_level_help_prefers_subcommand_first_usage() {
  let output = Command::new(vik_bin()).args(["--help"]).output().expect("spawn vik");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );
  let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
  assert!(
    stdout.contains("Vik runs workflow-driven agents for issue tracker work."),
    "got: {stdout}"
  );
  assert!(stdout.contains("Usage: vik <COMMAND> [WORKFLOW]"), "got: {stdout}");
}

#[test]
fn top_level_help_lists_init_command() {
  let output = Command::new(vik_bin()).args(["--help"]).output().expect("spawn vik");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );
  let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
  assert!(stdout.contains("init"), "got: {stdout}");
  assert!(stdout.contains("Generate a starter workflow setup"), "got: {stdout}");
}

#[test]
fn subcommand_help_shows_workflow_argument() {
  let output = Command::new(vik_bin()).args(["run", "-h"]).output().expect("spawn vik");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );
  let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
  let usage = if cfg!(windows) {
    "Usage: vik.exe run [OPTIONS] [WORKFLOW]"
  } else {
    "Usage: vik run [OPTIONS] [WORKFLOW]"
  };
  assert!(stdout.contains(usage), "got: {stdout}");
  assert!(
    stdout.contains("[WORKFLOW]  Path to the workflow file all subcommands act on"),
    "got: {stdout}"
  );
}
