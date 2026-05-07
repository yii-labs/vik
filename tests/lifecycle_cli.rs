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
