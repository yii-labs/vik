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
  assert!(stdout.contains("matt-pocock"), "got: {stdout}");
  assert!(stdout.contains("github-projects"), "got: {stdout}");
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
  assert!(workflow_yaml.contains("command: sh ./scripts/github-issues-json"));
  assert!(!workflow_yaml.contains("label:plan,rework,work,review,merge -label:blocked"));
  assert!(!workflow_yaml.contains("command: ./scripts/github-issues-json"));
  assert!(!workflow_yaml.contains("gh issue list --label \"vik\""));
  assert!(!workflow_yaml.contains("label:plan,work,rework,review,merge -label:blocked"));

  let script = temp.path().join("scripts").join("github-issues-json");
  let script_body = std::fs::read_to_string(&script).expect("read script");
  assert!(script_body.starts_with("gh issue list"));
  assert!(script_body.contains("gh issue list"));
  assert!(script_body.contains("--label \"vik\""));
  assert!(script_body.contains("--search '-label:blocked sort:created-asc'"));
  assert!(script_body.contains(". == \"plan\" or . == \"rework\" or . == \"work\""));
  assert!(!script_body.contains("label:plan,rework,work,review,merge"));
  assert!(!script_body.contains("label:plan,label:work,label:rework,label:review,label:merge"));

  let prompt = std::fs::read_to_string(temp.path().join(".agents/prompts/work.md")).expect("read prompt");
  assert!(!prompt.contains("Template:"));
  assert!(prompt.contains("$github-issues"), "got: {prompt}");
  assert!(prompt.contains("$symphony-workflow"), "got: {prompt}");
  let tracker_skill =
    std::fs::read_to_string(temp.path().join(".agents/skills/github-issues/SKILL.md")).expect("read skill");
  assert!(tracker_skill.contains("ISSUE_ID"), "got: {tracker_skill}");
  assert!(
    tracker_skill.contains("gh issue view \"$ISSUE_ID\""),
    "got: {tracker_skill}"
  );
  assert!(
    tracker_skill.contains("gh issue comment \"$ISSUE_ID\""),
    "got: {tracker_skill}"
  );
  assert!(
    tracker_skill.contains("gh issue edit \"$ISSUE_ID\""),
    "got: {tracker_skill}"
  );
  assert!(tracker_skill.contains("Closes #$ISSUE_ID"), "got: {tracker_skill}");
  assert!(!tracker_skill.contains("!`exec("), "got: {tracker_skill}");
  assert!(!tracker_skill.contains("{{"), "got: {tracker_skill}");

  let doctor = run_doctor(&workflow);
  assert!(
    doctor.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&doctor.stdout),
    String::from_utf8_lossy(&doctor.stderr),
  );
}

#[test]
fn init_generates_github_issue_management_skill_and_prompt_reference() {
  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");

  let output = run_init(&workflow, "simple", "github");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );

  let prompt = std::fs::read_to_string(temp.path().join(".agents/prompts/work.md")).expect("read prompt");
  assert!(prompt.contains("$github-issues"), "got: {prompt}");
  assert!(!prompt.contains("gh issue view {{ issue.id }}"), "got: {prompt}");
  assert!(!prompt.contains("__TRACKER_OPERATIONS__"), "got: {prompt}");

  let skill = std::fs::read_to_string(temp.path().join(".agents/skills/github-issues/SKILL.md")).expect("read skill");
  assert!(skill.contains("gh issue view \"$ISSUE_ID\""), "got: {skill}");
  assert!(skill.contains("gh issue comment \"$ISSUE_ID\""), "got: {skill}");
  assert!(skill.contains("gh issue edit \"$ISSUE_ID\""), "got: {skill}");
  assert!(skill.contains("vik init --force"), "got: {skill}");
  assert!(!skill.contains("!`exec("), "got: {skill}");
  assert!(!skill.contains("{{"), "got: {skill}");
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
  assert!(workflow_yaml.contains("command: sh ./scripts/linear-issues-json"));
  assert!(!workflow_yaml.contains("curl -sS https://api.linear.app/graphql"));
  assert!(!workflow_yaml.contains("command: ./scripts/linear-issues-json"));

  let script = temp.path().join("scripts").join("linear-issues-json");
  let script_body = std::fs::read_to_string(&script).expect("read script");
  assert!(script_body.contains("LINEAR_API_KEY"));
  assert!(script_body.contains("https://api.linear.app/graphql"));
  assert!(script_body.contains("STATES='[\"work\",\"review\"]'"));
  assert!(script_body.contains("state: { name: { in: $states } }"));
  assert!(script_body.contains("variables: {teamKey: $teamKey, states: $states}"));
  assert!(
    script_body.contains("response=$(curl -sS https://api.linear.app/graphql"),
    "got: {script_body}"
  );
  assert!(script_body.contains(") || exit $?"), "got: {script_body}");
  assert!(
    script_body.contains("printf '%s\\n' \"$response\""),
    "got: {script_body}"
  );

  let prompt = std::fs::read_to_string(temp.path().join(".agents/prompts/review.md")).expect("read prompt");
  assert!(!prompt.contains("Template:"));
  assert!(prompt.contains("$linear-issues"), "got: {prompt}");
  let tracker_skill =
    std::fs::read_to_string(temp.path().join(".agents/skills/linear-issues/SKILL.md")).expect("read skill");
  assert!(tracker_skill.contains("LINEAR_ISSUE_ID"), "got: {tracker_skill}");
  assert!(tracker_skill.contains("Linear MCP `get_issue"), "got: {tracker_skill}");
  assert!(
    tracker_skill.contains("Linear MCP `create_comment"),
    "got: {tracker_skill}"
  );
  assert!(
    tracker_skill.contains("Linear MCP `update_issue"),
    "got: {tracker_skill}"
  );
  assert!(
    tracker_skill.contains("Linear MCP `create_attachment"),
    "got: {tracker_skill}"
  );
  assert!(!tracker_skill.contains("{{"), "got: {tracker_skill}");

  let doctor = run_doctor(&workflow);
  assert!(
    doctor.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&doctor.stdout),
    String::from_utf8_lossy(&doctor.stderr),
  );
}

#[test]
fn init_generates_all_template_tracker_pairs_and_doctor_accepts_them() {
  for template in ["simple", "symphony", "matt-pocock"] {
    for tracker in ["github", "github-projects", "linear"] {
      let temp = tempfile::tempdir().expect("tempdir");
      let workflow = temp.path().join("workflow.yml");

      let output = run_init(&workflow, template, tracker);
      assert!(
        output.status.success(),
        "template={template} tracker={tracker}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
      );

      let doctor = run_doctor(&workflow);
      assert!(
        doctor.status.success(),
        "template={template} tracker={tracker}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&doctor.stdout),
        String::from_utf8_lossy(&doctor.stderr),
      );
    }
  }
}

#[test]
fn init_generates_matt_pocock_setup_with_skills_and_ready_hitl_issue_prompt() {
  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");

  let output = run_init(&workflow, "matt-pocock", "github");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );

  let workflow_yaml = std::fs::read_to_string(&workflow).expect("read workflow");
  for stage in ["grill", "prd", "issues", "work", "review", "merge"] {
    assert!(
      workflow_yaml.contains(&format!("    {stage}:")),
      "missing stage {stage} in {workflow_yaml}"
    );
    assert!(
      temp.path().join(".agents").join("prompts").join(format!("{stage}.md")).exists(),
      "missing prompt for {stage}",
    );
  }

  for skill in ["grill-me", "grill-with-docs", "to-prd", "to-issues"] {
    let skill_body = std::fs::read_to_string(temp.path().join(".agents").join("skills").join(skill).join("SKILL.md"))
      .expect("read skill");
    assert!(
      skill_body.contains("vik init --force"),
      "missing refresh path in {skill}: {skill_body}",
    );
    assert!(
      temp.path().join(".agents").join("skills").join(skill).join("SKILL.md").exists(),
      "missing skill {skill}",
    );
  }

  let grill_prompt = std::fs::read_to_string(temp.path().join(".agents/prompts/grill.md")).expect("read grill prompt");
  assert!(grill_prompt.contains("$grill-me"), "got: {grill_prompt}");
  assert!(grill_prompt.contains("$grill-with-docs"), "got: {grill_prompt}");
  let prd_prompt = std::fs::read_to_string(temp.path().join(".agents/prompts/prd.md")).expect("read prd prompt");
  assert!(prd_prompt.contains("$to-prd"), "got: {prd_prompt}");
  let issues_prompt =
    std::fs::read_to_string(temp.path().join(".agents/prompts/issues.md")).expect("read issues prompt");
  assert!(issues_prompt.contains("$to-issues"), "got: {issues_prompt}");
  assert!(issues_prompt.contains("ready"), "got: {issues_prompt}");
  assert!(issues_prompt.contains("HITL"), "got: {issues_prompt}");
}

#[test]
fn init_generates_github_projects_script_and_status_operations() {
  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");

  let output = run_init(&workflow, "simple", "github-projects");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );

  let workflow_yaml = std::fs::read_to_string(&workflow).expect("read workflow");
  assert!(workflow_yaml.contains("command: sh ./scripts/github-project-items-json"));

  let script = temp.path().join("scripts").join("github-project-items-json");
  let script_body = std::fs::read_to_string(&script).expect("read script");
  assert!(script_body.contains("gh project item-list"));
  assert!(script_body.contains("GITHUB_PROJECT_OWNER"));
  assert!(script_body.contains("GITHUB_PROJECT_NUMBER"));
  assert!(!script_body.contains("--query"));
  assert!(script_body.contains("state: .status"));
  assert!(script_body.contains("project_item_id: .id"));
  assert!(script_body.contains(". == \"work\" or . == \"review\""));

  let prompt = std::fs::read_to_string(temp.path().join(".agents/prompts/work.md")).expect("read prompt");
  assert!(prompt.contains("$github-projects"), "got: {prompt}");
  let tracker_skill =
    std::fs::read_to_string(temp.path().join(".agents/skills/github-projects/SKILL.md")).expect("read skill");
  assert!(tracker_skill.contains("gh project item-edit"), "got: {tracker_skill}");
  assert!(tracker_skill.contains("PROJECT_ITEM_ID"), "got: {tracker_skill}");
  assert!(
    tracker_skill.contains("--id \"$PROJECT_ITEM_ID\""),
    "got: {tracker_skill}"
  );
  assert!(!tracker_skill.contains("!`exec("), "got: {tracker_skill}");
  assert!(!tracker_skill.contains("{{"), "got: {tracker_skill}");
}

#[test]
fn init_fails_on_non_interactive_skill_name_collision_without_force() {
  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");
  let skill = temp.path().join(".agents/skills/grill-me/SKILL.md");
  std::fs::create_dir_all(skill.parent().expect("skill parent")).expect("create skill dir");
  std::fs::write(&skill, "existing skill").expect("write skill");

  let output = run_init(&workflow, "matt-pocock", "github");
  assert!(
    !output.status.success(),
    "expected non-zero; stdout={} stderr={}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );

  let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
  assert!(stderr.contains("bundled skill name already exists"), "got: {stderr}");
  assert!(stderr.contains("grill-me"), "got: {stderr}");
  assert!(
    !workflow.exists(),
    "workflow must not be generated after skill collision"
  );
}

#[test]
fn init_force_overwrites_existing_skill_files() {
  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");
  let skill = temp.path().join(".agents/skills/grill-me/SKILL.md");
  std::fs::create_dir_all(skill.parent().expect("skill parent")).expect("create skill dir");
  std::fs::write(&skill, "old skill").expect("write skill");

  let output = run_init_force(&workflow, "matt-pocock", "github");
  assert!(
    output.status.success(),
    "stdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr),
  );

  let skill_body = std::fs::read_to_string(&skill).expect("read skill");
  let prompt_body = std::fs::read_to_string(temp.path().join(".agents/prompts/grill.md")).expect("read prompt");
  assert!(skill_body.contains("Grill the plan"), "got: {skill_body}");
  assert!(prompt_body.contains("$grill-me"), "got: {prompt_body}");
  assert!(!skill_body.contains("old skill"));
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
  session.expect("Simple: work -> review").expect("simple choice label");
  session
    .expect("Symphony: plan(rework) -> work -> review -> merge")
    .expect("symphony choice label");
  session
    .expect("Matt Pocock: grill -> prd -> issues(ready/HITL) -> work -> review -> merge")
    .expect("matt pocock choice label");
  session.send("\x1b[B\x1b[B\r").expect("select matt pocock template");
  session.expect("Issue tracker?").expect("tracker prompt");
  session.expect("GitHub Issue").expect("github issue choice label");
  session.expect("GitHub Projects").expect("github projects choice label");
  session.expect("Linear").expect("linear choice label");
  session.send("\x1b[B\r").expect("select github projects tracker");
  session.expect("Created Vik workflow setup").expect("created setup");
  session.expect(Eof).expect("vik init exits");

  let workflow_yaml = std::fs::read_to_string(&workflow).expect("read workflow");
  assert!(workflow_yaml.contains("command: sh ./scripts/github-project-items-json"));
  assert!(workflow_yaml.contains("    grill:"), "got: {workflow_yaml}");
  assert!(workflow_yaml.contains("    issues:"), "got: {workflow_yaml}");
  assert!(workflow_yaml.contains("    merge:"), "got: {workflow_yaml}");
}

#[test]
#[cfg(unix)]
fn init_prompts_for_alternate_skill_name_when_default_skill_exists() {
  use expectrl::{Eof, Expect, Session};

  let temp = tempfile::tempdir().expect("tempdir");
  let workflow = temp.path().join("workflow.yml");
  let existing_skill = temp.path().join(".agents/skills/symphony-workflow/SKILL.md");
  std::fs::create_dir_all(existing_skill.parent().expect("skill parent")).expect("create skill dir");
  std::fs::write(&existing_skill, "keep existing skill").expect("write skill");

  let mut command = Command::new(vik_bin());
  command.arg("init").arg(&workflow);

  let mut session = Session::spawn(command).expect("spawn vik init");
  session.set_expect_timeout(Some(Duration::from_secs(20)));
  session.expect("Templates?").expect("template prompt");
  session.send("\x1b[B\r").expect("select symphony template");
  session.expect("Issue tracker?").expect("tracker prompt");
  session.send("\r").expect("select github tracker");
  session
    .expect("Skill name for Symphony workflow?")
    .expect("skill rename prompt");
  session.send("symphony-local\r").expect("enter alternate skill name");
  session.expect("Created Vik workflow setup").expect("created setup");
  session.expect(Eof).expect("vik init exits");

  let existing_skill_body = std::fs::read_to_string(&existing_skill).expect("read existing skill");
  let prompt_body = std::fs::read_to_string(temp.path().join(".agents/prompts/work.md")).expect("read prompt");
  assert_eq!(existing_skill_body, "keep existing skill");
  assert!(
    temp.path().join(".agents/skills/symphony-local/SKILL.md").exists(),
    "missing renamed skill",
  );
  assert!(prompt_body.contains("$symphony-local"), "got: {prompt_body}");
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
  assert!(workflow_yaml.contains("command: sh ./scripts/github-issues-json"));
  assert!(prompt_body.contains("# Stage `work`"));
  assert!(!workflow_yaml.contains("old workflow"));
  assert!(!prompt_body.contains("old prompt"));
}
