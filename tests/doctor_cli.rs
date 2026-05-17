//! Integration tests for the `vik doctor` CLI.
//!
//! These drive the compiled binary through `std::process::Command` so that
//! exit codes, argument parsing, and stdout shape are exercised end-to-end.

use std::path::PathBuf;
use std::process::Command;

fn vik_bin() -> PathBuf {
  PathBuf::from(env!("CARGO_BIN_EXE_vik"))
}

fn fixture_dir(name: &str) -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("tests")
    .join("fixtures")
    .join("workflows")
    .join(name)
}

fn missing_prompt_workflow() -> (tempfile::TempDir, PathBuf) {
  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");
  std::fs::write(
    &workflow,
    r#"
loop:
  max_issue_concurrency: 1
  wait_ms: 100
workspace:
  root: .vik
agents:
  codex:
    runtime: codex
    model: gpt-5.5
issues:
  pull:
    command: printf '[]'
issue:
  stages:
    plan:
      when:
        state: todo
      agent: codex
      prompt_file: ./prompts/missing.md
"#,
  )
  .expect("write workflow");
  (temp, workflow)
}

#[test]
fn valid_fixture_exits_zero() {
  let workflow = fixture_dir("valid").join("workflow.yml");
  let output = Command::new(vik_bin())
    .args(["doctor", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );
  let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
  assert!(
    stdout.contains("Vik doctor shows 0 error(s), 0 warning(s)"),
    "got: {stdout}"
  );
}

#[test]
fn valid_fixture_json_has_expected_shape() {
  let workflow = fixture_dir("valid").join("workflow.yml");
  let output = Command::new(vik_bin())
    .args(["doctor", "--json", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");
  assert!(output.status.success());
  let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
  let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
  assert!(json["errors"].is_array());
  assert!(json["warnings"].is_array());
}

#[test]
fn missing_prompt_file_does_not_fail_doctor() {
  let (_temp, workflow) = missing_prompt_workflow();
  let output = Command::new(vik_bin())
    .args(["doctor", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");

  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );
}
