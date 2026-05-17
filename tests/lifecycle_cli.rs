//! Integration tests for the `vik status` / `vik stop` / `vik uninstall`
//! lifecycle CLI entry points.
//!
//! These drive the compiled binary through `std::process::Command` so
//! the entire argument parser → workflow loader → lifecycle helper
//! chain is exercised end-to-end. Smoke-level coverage only — the
//! detailed behavior lives in the unit tests under
//! `src/daemon/lifecycle.rs` and `src/daemon/state.rs`.

use std::path::PathBuf;
use std::process::Command;

fn vik_bin() -> PathBuf {
  PathBuf::from(env!("CARGO_BIN_EXE_vik"))
}

fn fixture_workflow() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("tests")
    .join("fixtures")
    .join("workflows")
    .join("valid")
    .join("workflow.yml")
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
fn status_reports_not_installed_when_no_state_file() {
  // The valid fixture uses a `.vik/` workspace root relative to the
  // workflow file. Before this test runs there is no daemon running
  // for that workflow, so `vik status` should print "not installed".
  //
  // Remove any leftover state file from earlier runs so this is
  // deterministic. `ignore errors` because the tree may not exist
  // yet.
  let workflow = fixture_workflow();
  let state_file = workflow.parent().unwrap().join(".vik").join("service").join("state.json");
  let _ = std::fs::remove_file(&state_file);

  let output = Command::new(vik_bin())
    .args(["status", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );
  let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
  assert!(stdout.contains("not installed"), "got: {stdout}");
}

#[test]
fn stop_without_state_file_exits_nonzero() {
  // `vik stop` with no daemon record exits 1 per the
  // `LifecycleError::NotInstalled` mapping. Scripts rely on this to
  // distinguish "daemon was running and stopped" from "nothing to
  // do."
  let workflow = fixture_workflow();
  let state_file = workflow.parent().unwrap().join(".vik").join("service").join("state.json");
  let _ = std::fs::remove_file(&state_file);

  let output = Command::new(vik_bin())
    .args(["stop", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");
  assert!(
    !output.status.success(),
    "expected non-zero exit; stdout={} stderr={}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );
}

#[test]
fn uninstall_without_state_file_is_noop() {
  // Uninstall is idempotent — no state file means there is nothing
  // to clean up and the command still succeeds.
  let workflow = fixture_workflow();
  let state_file = workflow.parent().unwrap().join(".vik").join("service").join("state.json");
  let _ = std::fs::remove_file(&state_file);

  let output = Command::new(vik_bin())
    .args(["uninstall", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );
}

#[test]
fn status_does_not_require_prompt_file() {
  let (_temp, workflow) = missing_prompt_workflow();
  let output = Command::new(vik_bin())
    .args(["status", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");

  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );
  assert!(!String::from_utf8_lossy(&output.stderr).contains("prompt"));
}

#[test]
fn stop_does_not_require_prompt_file() {
  let (_temp, workflow) = missing_prompt_workflow();
  let output = Command::new(vik_bin())
    .args(["stop", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");

  assert!(!output.status.success());
  assert!(!String::from_utf8_lossy(&output.stderr).contains("prompt"));
}

#[test]
fn uninstall_does_not_require_prompt_file() {
  let (_temp, workflow) = missing_prompt_workflow();
  let output = Command::new(vik_bin())
    .args(["uninstall", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");

  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );
  assert!(!String::from_utf8_lossy(&output.stderr).contains("prompt"));
}

#[test]
fn run_detached_fails_before_daemon_start_when_prompt_file_is_missing() {
  let (_temp, workflow) = missing_prompt_workflow();
  let output = Command::new(vik_bin())
    .args(["run", "-d", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");

  assert!(!output.status.success());
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(stderr.contains("load workflow dynamic content"), "stderr: {stderr}");
  assert!(stderr.contains("missing.md"), "stderr: {stderr}");
  assert!(stderr.contains("stage `plan`"), "stderr: {stderr}");
}

#[test]
fn restart_fails_for_fresh_run_when_prompt_file_is_missing() {
  let (_temp, workflow) = missing_prompt_workflow();
  let output = Command::new(vik_bin())
    .args(["restart", workflow.to_str().expect("utf-8 path")])
    .output()
    .expect("spawn vik");

  assert!(!output.status.success());
  let stdout = String::from_utf8_lossy(&output.stdout);
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(stdout.contains("starting one"), "stdout: {stdout}");
  assert!(stderr.contains("load workflow dynamic content"), "stderr: {stderr}");
  assert!(stderr.contains("missing.md"), "stderr: {stderr}");
  assert!(stderr.contains("stage `plan`"), "stderr: {stderr}");
}
