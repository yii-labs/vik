//! Integration tests for `vik init`.

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::time::Duration;

fn vik_bin() -> PathBuf {
  PathBuf::from(env!("CARGO_BIN_EXE_vik"))
}

fn run_init(workflow: &Path, template: &str, tracker: &str) -> std::process::Output {
  Command::new(vik_bin())
    .args(["init", "--template", template, "--tracker", tracker])
    .arg(workflow)
    .output()
    .expect("spawn vik init")
}

fn run_init_force(workflow: &Path, template: &str, tracker: &str) -> std::process::Output {
  Command::new(vik_bin())
    .args(["init", "--template", template, "--tracker", tracker, "--force"])
    .arg(workflow)
    .output()
    .expect("spawn vik init")
}

fn run_doctor(workflow: &Path) -> std::process::Output {
  Command::new(vik_bin())
    .args(["doctor", "--json"])
    .arg(workflow)
    .output()
    .expect("spawn vik doctor")
}

#[test]
fn init_help_shows_non_interactive_flags() {
  let output = Command::new(vik_bin()).args(["init", "--help"]).output().expect("spawn vik");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );

  let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
  assert!(stdout.contains("--template"), "got: {stdout}");
  assert!(stdout.contains("--tracker"), "got: {stdout}");
  assert!(stdout.contains("--force"), "got: {stdout}");
}

#[test]
fn init_generates_symphony_github_setup_and_doctor_accepts_it() {
  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");

  let output = run_init(&workflow, "symphony", "github");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );

  let workflow_yaml = std::fs::read_to_string(&workflow).expect("read workflow");
  for stage in ["plan", "work", "rework", "review", "merge"] {
    assert!(
      workflow_yaml.contains(&format!("    {stage}:")),
      "missing stage {stage} in {workflow_yaml}"
    );
    assert!(
      temp.path().join(".agents").join("prompts").join(format!("{stage}.md")).exists(),
      "missing prompt for {stage}",
    );
  }
  assert!(workflow_yaml.contains("command: |"));
  assert!(workflow_yaml.contains("gh issue list --label \"vik\""));
  assert!(workflow_yaml.contains("label:plan,rework,work,review,merge -label:blocked"));
  assert!(!workflow_yaml.contains("command: ./scripts/github-issues-json"));
  assert!(!workflow_yaml.contains("label:plan,work,rework,review,merge -label:blocked"));

  let script = temp.path().join("scripts").join("github-issues-json");
  let script_body = std::fs::read_to_string(&script).expect("read script");
  assert!(script_body.starts_with("gh issue list"));
  assert!(script_body.contains("gh issue list"));
  assert!(script_body.contains("--label \"vik\""));
  assert!(script_body.contains("label:plan,rework,work,review,merge -label:blocked"));
  assert!(!script_body.contains("label:plan,label:work,label:rework,label:review,label:merge"));

  let prompt = std::fs::read_to_string(temp.path().join(".agents/prompts/work.md")).expect("read prompt");
  assert!(!prompt.contains("Template:"));
  assert!(prompt.contains("gh issue view {{ issue.id }}"));
  assert!(prompt.contains("gh issue comment {{ issue.id }}"));
  assert!(prompt.contains("gh issue edit {{ issue.id }}"));
  assert!(prompt.contains("Closes #{{ issue.id }}"));

  let doctor = run_doctor(&workflow);
  assert!(
    doctor.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&doctor.stdout),
    String::from_utf8_lossy(&doctor.stderr),
  );
}

#[test]
fn init_generates_simple_linear_setup_and_doctor_accepts_it() {
  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");

  let output = run_init(&workflow, "simple", "linear");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );

  let workflow_yaml = std::fs::read_to_string(&workflow).expect("read workflow");
  assert!(workflow_yaml.contains("    work:"), "got: {workflow_yaml}");
  assert!(workflow_yaml.contains("    review:"), "got: {workflow_yaml}");
  assert!(!workflow_yaml.contains("    plan:"), "got: {workflow_yaml}");
  assert!(workflow_yaml.contains("command: |"));
  assert!(workflow_yaml.contains("curl -sS https://api.linear.app/graphql"));
  assert!(!workflow_yaml.contains("command: ./scripts/linear-issues-json"));

  let script = temp.path().join("scripts").join("linear-issues-json");
  let script_body = std::fs::read_to_string(&script).expect("read script");
  assert!(script_body.contains("LINEAR_API_KEY"));
  assert!(script_body.contains("https://api.linear.app/graphql"));

  let prompt = std::fs::read_to_string(temp.path().join(".agents/prompts/review.md")).expect("read prompt");
  assert!(!prompt.contains("Template:"));
  assert!(prompt.contains("Linear MCP `get_issue"));
  assert!(prompt.contains("Linear MCP `create_comment"));
  assert!(prompt.contains("Linear MCP `update_issue"));
  assert!(prompt.contains("Linear MCP `create_attachment"));

  let doctor = run_doctor(&workflow);
  assert!(
    doctor.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&doctor.stdout),
    String::from_utf8_lossy(&doctor.stderr),
  );
}

#[test]
#[cfg(unix)]
fn init_prompts_for_missing_choices() {
  use expectrl::{Eof, Expect, Session};

  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");

  let mut command = Command::new(vik_bin());
  command.arg("init").arg(&workflow);

  let mut session = Session::spawn(command).expect("spawn vik init");
  session.set_expect_timeout(Some(Duration::from_secs(20)));
  session.expect("Templates?").expect("template prompt");
  session
    .expect("Symphony: plan(rework) -> work -> review -> merge")
    .expect("symphony choice label");
  session.send("\x1b[B\r").expect("select simple template");
  session.expect("Issue tracker?").expect("tracker prompt");
  session.send("\x1b[B\r").expect("select linear tracker");
  session.expect("Created Vik workflow setup").expect("created setup");
  session.expect(Eof).expect("vik init exits");

  let workflow_yaml = std::fs::read_to_string(&workflow).expect("read workflow");
  assert!(workflow_yaml.contains("curl -sS https://api.linear.app/graphql"));
  assert!(workflow_yaml.contains("    work:"), "got: {workflow_yaml}");
  assert!(workflow_yaml.contains("    review:"), "got: {workflow_yaml}");
  assert!(!workflow_yaml.contains("    plan:"), "got: {workflow_yaml}");
}

#[test]
fn init_refuses_to_overwrite_existing_workflow_without_force() {
  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");
  std::fs::write(&workflow, "keep me").expect("write workflow");

  let output = run_init(&workflow, "simple", "github");
  assert!(
    !output.status.success(),
    "expected non-zero; stdout={} stderr={}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );

  let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
  assert!(stderr.contains("refusing to overwrite"), "got: {stderr}");
  assert_eq!(std::fs::read_to_string(&workflow).expect("read workflow"), "keep me");
}

#[test]
fn init_force_overwrites_existing_generated_files() {
  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");
  let prompt = temp.path().join(".agents").join("prompts").join("work.md");
  std::fs::create_dir_all(prompt.parent().expect("prompt parent")).expect("create prompts");
  std::fs::write(&workflow, "old workflow").expect("write workflow");
  std::fs::write(&prompt, "old prompt").expect("write prompt");

  let output = run_init_force(&workflow, "simple", "github");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );

  let workflow_yaml = std::fs::read_to_string(&workflow).expect("read workflow");
  let prompt_body = std::fs::read_to_string(&prompt).expect("read prompt");
  assert!(workflow_yaml.contains("gh issue list --label \"vik\""));
  assert!(prompt_body.contains("# work Stage"));
  assert!(!workflow_yaml.contains("old workflow"));
  assert!(!prompt_body.contains("old prompt"));
}
