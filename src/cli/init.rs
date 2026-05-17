//! `vik init [WORKFLOW]` — generate a starter workflow setup.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use thiserror::Error;

#[derive(Debug, Parser)]
pub struct InitArgs {
  /// Workflow template to generate.
  #[arg(long, value_enum)]
  pub template: Option<InitTemplate>,

  /// Issue tracker integration placeholders to generate.
  #[arg(long, value_enum)]
  pub tracker: Option<InitTracker>,

  /// Overwrite generated files when they already exist.
  #[arg(long)]
  pub force: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum InitTemplate {
  /// Plan, work, rework, review, and merge stages.
  Symphony,
  /// Work and review stages only.
  Simple,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum InitTracker {
  /// GitHub Issue workflow using the GitHub CLI.
  Github,
  /// Linear workflow using Linear API intake and MCP prompt operations.
  Linear,
}

pub fn execute(workflow_path: PathBuf, args: InitArgs) -> ExitCode {
  match execute_inner(workflow_path, args) {
    Ok(report) => {
      report.print();
      ExitCode::SUCCESS
    },
    Err(err) => {
      let _ = writeln!(io::stderr(), "vik init failed: {err}");
      ExitCode::from(1)
    },
  }
}

fn execute_inner(workflow_path: PathBuf, args: InitArgs) -> Result<InitReport, InitError> {
  let template = choose_template(args.template)?;
  let tracker = choose_tracker(args.tracker)?;
  let generator = InitGenerator {
    workflow_path,
    template,
    tracker,
    force: args.force,
  };

  generator.generate()
}

fn choose_template(choice: Option<InitTemplate>) -> Result<InitTemplate, InitError> {
  match choice {
    Some(choice) => Ok(choice),
    None => prompt_choice(
      "Templates?",
      &[
        Choice::new(
          "Symphony Like: plan -> work(rework) -> review -> merge",
          InitTemplate::Symphony,
        ),
        Choice::new("Simple(oneshot): work -> review", InitTemplate::Simple),
      ],
      "--template",
    ),
  }
}

fn choose_tracker(choice: Option<InitTracker>) -> Result<InitTracker, InitError> {
  match choice {
    Some(choice) => Ok(choice),
    None => prompt_choice(
      "Issue tracker?",
      &[
        Choice::new("GitHub Issue", InitTracker::Github),
        Choice::new("Linear", InitTracker::Linear),
      ],
      "--tracker",
    ),
  }
}

#[derive(Clone, Copy)]
struct Choice<T> {
  label: &'static str,
  value: T,
}

impl<T> Choice<T> {
  const fn new(label: &'static str, value: T) -> Self {
    Self { label, value }
  }
}

fn prompt_choice<T: Copy>(title: &str, choices: &[Choice<T>], flag: &'static str) -> Result<T, InitError> {
  let mut stdout = io::stdout();
  writeln!(stdout, "{title}")?;
  for (index, choice) in choices.iter().enumerate() {
    writeln!(stdout, "  {}) {}", index + 1, choice.label)?;
  }

  loop {
    write!(stdout, "Select 1-{}: ", choices.len())?;
    stdout.flush()?;

    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
      return Err(InitError::MissingChoice { flag });
    }
    if let Ok(index) = input.trim().parse::<usize>()
      && (1..=choices.len()).contains(&index)
    {
      return Ok(choices[index - 1].value);
    }
    writeln!(stdout, "Invalid choice.")?;
  }
}

struct InitGenerator {
  workflow_path: PathBuf,
  template: InitTemplate,
  tracker: InitTracker,
  force: bool,
}

impl InitGenerator {
  fn generate(&self) -> Result<InitReport, InitError> {
    let workflow_dir = workflow_dir(&self.workflow_path);
    let files = self.files(&workflow_dir);
    let existing = files
      .iter()
      .filter(|file| file.path.exists())
      .map(|file| file.path.clone())
      .collect::<Vec<_>>();

    if !existing.is_empty() && !self.force {
      return Err(InitError::WouldOverwrite { paths: existing });
    }

    if let Some(parent) = self.workflow_path.parent().filter(|path| !path.as_os_str().is_empty()) {
      fs::create_dir_all(parent).map_err(|source| InitError::CreateDir {
        path: parent.to_path_buf(),
        source,
      })?;
    }

    for file in &files {
      if let Some(parent) = file.path.parent() {
        fs::create_dir_all(parent).map_err(|source| InitError::CreateDir {
          path: parent.to_path_buf(),
          source,
        })?;
      }
      fs::write(&file.path, file.contents.as_bytes()).map_err(|source| InitError::Write {
        path: file.path.clone(),
        source,
      })?;
      make_executable(file)?;
    }

    Ok(InitReport {
      workflow_path: self.workflow_path.clone(),
      files: files.into_iter().map(|file| file.path).collect(),
      overwritten: self.force,
    })
  }

  fn files(&self, workflow_dir: &Path) -> Vec<GeneratedFile> {
    let prompt_dir = workflow_dir.join(".agents").join("prompts");
    let scripts_dir = workflow_dir.join("scripts");
    let stages = self.template.stages();

    let mut files = vec![
      GeneratedFile::plain(self.workflow_path.clone(), self.workflow_yaml()),
      GeneratedFile::executable(
        scripts_dir.join(self.tracker.script_name()),
        self.tracker.script(stages),
      ),
    ];

    for stage in stages {
      files.push(GeneratedFile::plain(
        prompt_dir.join(format!("{}.md", stage.name)),
        prompt(*stage, self.template, self.tracker),
      ));
    }

    files
  }

  fn workflow_yaml(&self) -> String {
    let mut yaml = format!(
      "\
loop:
  max_issue_concurrency: 2
  wait_ms: 5000

workspace:
  root: .vik

agents:
  coder:
    runtime: codex
    model: gpt-5.5

issues:
  pull:
    command: ./scripts/{script_name}
    idle_sec: {idle_sec}

issue:
  stages:
",
      script_name = self.tracker.script_name(),
      idle_sec = self.tracker.idle_sec(),
    );

    for stage in self.template.stages() {
      yaml.push_str(&format!(
        "    {name}:\n      when:\n        state: {state}\n      agent: coder\n      prompt_file: ./.agents/prompts/{name}.md\n",
        name = stage.name,
        state = stage.state,
      ));
    }

    yaml
  }
}

#[cfg(unix)]
fn make_executable(file: &GeneratedFile) -> Result<(), InitError> {
  use std::os::unix::fs::PermissionsExt;

  if !file.executable {
    return Ok(());
  }

  let mut permissions = fs::metadata(&file.path)
    .map_err(|source| InitError::Metadata {
      path: file.path.clone(),
      source,
    })?
    .permissions();
  permissions.set_mode(0o755);
  fs::set_permissions(&file.path, permissions).map_err(|source| InitError::Permissions {
    path: file.path.clone(),
    source,
  })
}

#[cfg(not(unix))]
fn make_executable(file: &GeneratedFile) -> Result<(), InitError> {
  let _ = file.executable;
  Ok(())
}

fn workflow_dir(path: &Path) -> PathBuf {
  path
    .parent()
    .filter(|parent| !parent.as_os_str().is_empty())
    .unwrap_or_else(|| Path::new("."))
    .to_path_buf()
}

#[derive(Clone, Copy)]
struct Stage {
  name: &'static str,
  state: &'static str,
}

const SYMPHONY_STAGES: &[Stage] = &[
  Stage {
    name: "plan",
    state: "plan",
  },
  Stage {
    name: "work",
    state: "work",
  },
  Stage {
    name: "rework",
    state: "rework",
  },
  Stage {
    name: "review",
    state: "review",
  },
  Stage {
    name: "merge",
    state: "merge",
  },
];

const SIMPLE_STAGES: &[Stage] = &[
  Stage {
    name: "work",
    state: "work",
  },
  Stage {
    name: "review",
    state: "review",
  },
];

impl InitTemplate {
  fn stages(self) -> &'static [Stage] {
    match self {
      InitTemplate::Symphony => SYMPHONY_STAGES,
      InitTemplate::Simple => SIMPLE_STAGES,
    }
  }

  fn name(self) -> &'static str {
    match self {
      InitTemplate::Symphony => "Symphony Like",
      InitTemplate::Simple => "Simple",
    }
  }
}

impl InitTracker {
  fn script_name(self) -> &'static str {
    match self {
      InitTracker::Github => "github-issues-json",
      InitTracker::Linear => "linear-issues-json",
    }
  }

  fn idle_sec(self) -> u64 {
    match self {
      InitTracker::Github => 5,
      InitTracker::Linear => 10,
    }
  }

  fn script(self, stages: &[Stage]) -> String {
    match self {
      InitTracker::Github => github_script(stages),
      InitTracker::Linear => linear_script(),
    }
  }
}

struct GeneratedFile {
  path: PathBuf,
  contents: String,
  executable: bool,
}

impl GeneratedFile {
  fn plain(path: PathBuf, contents: String) -> Self {
    Self {
      path,
      contents,
      executable: false,
    }
  }

  fn executable(path: PathBuf, contents: String) -> Self {
    Self {
      path,
      contents,
      executable: true,
    }
  }
}

struct InitReport {
  workflow_path: PathBuf,
  files: Vec<PathBuf>,
  overwritten: bool,
}

impl InitReport {
  fn print(&self) {
    if self.overwritten {
      println!("Overwrote Vik workflow setup at {}", self.workflow_path.display());
    } else {
      println!("Created Vik workflow setup at {}", self.workflow_path.display());
    }
    for file in &self.files {
      println!("- {}", file.display());
    }
    println!("Next: vik doctor {}", self.workflow_path.display());
  }
}

fn github_script(stages: &[Stage]) -> String {
  let labels = stages.iter().map(|stage| stage.state).collect::<Vec<_>>();
  let search_labels = labels
    .iter()
    .map(|label| format!("label:{label}"))
    .collect::<Vec<_>>()
    .join(",");
  let jq_states = labels
    .iter()
    .map(|label| format!(". == \"{label}\""))
    .collect::<Vec<_>>()
    .join(" or ");

  format!(
    r#"#!/usr/bin/env bash
set -euo pipefail

gh issue list --label "vik" --state "open" --limit 50 \
  --search '{search_labels} -label:blocked sort:created-asc' \
  --json number,title,labels \
  --jq '
    [
      .[]
      | ([.labels[].name] | map(select({jq_states}))) as $states
      | select($states | length == 1)
      | {{ id: (.number | tostring), title: .title, state: $states[0] }}
    ]
  '
"#,
  )
}

fn linear_script() -> String {
  r#"#!/usr/bin/env bash
set -euo pipefail

: "${LINEAR_API_KEY:?LINEAR_API_KEY is required}"
TEAM_KEY="${LINEAR_TEAM_KEY:-ENG}"

QUERY='
query ($teamKey: String!) {
  issues(
    filter: { team: { key: { eq: $teamKey } } }
    first: 50
    orderBy: createdAt
  ) {
    nodes {
      identifier
      title
      state { name }
    }
  }
}'

curl -sS https://api.linear.app/graphql \
  -H "Authorization: $LINEAR_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$(jq -n --arg q "$QUERY" --arg teamKey "$TEAM_KEY" '{query: $q, variables: {teamKey: $teamKey}}')" \
| jq '
    [
      .data.issues.nodes[]
      | { id: .identifier, title: .title, state: .state.name }
    ]
  '
"#
  .to_string()
}

fn prompt(stage: Stage, template: InitTemplate, tracker: InitTracker) -> String {
  format!(
    r#"# {stage_name} Stage

Template: {template_name}
Issue: `{{{{ issue.id }}}}`: `{{{{ issue.title }}}}`
State: `{{{{ issue.state }}}}`
Workdir: `{{{{ issue.workdir }}}}`

## Start

{tracker_read}

## Work

- Do only the work for this stage.
- Keep tracker comments current.
- Move tracker state only after this stage is complete.

## Tracker Operations

{tracker_operations}

## Finish

- Record what changed.
- Record validation commands and results.
- Move state to the next workflow state when complete.
"#,
    stage_name = stage.name,
    template_name = template.name(),
    tracker_read = tracker_read(tracker),
    tracker_operations = tracker_operations(tracker),
  )
}

fn tracker_read(tracker: InitTracker) -> &'static str {
  match tracker {
    InitTracker::Github => {
      r#"Fetch current GitHub issue detail:

!`exec(gh issue view {{ issue.id }} --json number,title,body,state,labels,comments,url,updatedAt)`
"#
    },
    InitTracker::Linear => {
      r#"Use the Linear MCP `get_issue` tool with `id: "{{ issue.id }}"`.
Fetch description, state, labels, comments, and attachments.
"#
    },
  }
}

fn tracker_operations(tracker: InitTracker) -> &'static str {
  match tracker {
    InitTracker::Github => {
      r#"- View issue: `gh issue view {{ issue.id }} --json number,title,body,labels,comments,url`
- Comment: `gh issue comment {{ issue.id }} --body "..."`
- Move label state: `gh issue edit {{ issue.id }} --remove-label <old-state> --add-label <new-state>`
- Link PR: include `Closes #{{ issue.id }}` in the PR body or run `gh pr edit <pr> --body-file <file>`.
"#
    },
    InitTracker::Linear => {
      r#"- Read issue: Linear MCP `get_issue { id: "{{ issue.id }}" }`.
- Comment: Linear MCP `create_comment { issueId: "{{ issue.id }}", body: "..." }`.
- Move state: Linear MCP `update_issue`; first find the target state id with `get_workflow_states`.
- Attach PR: Linear MCP `create_attachment { issueId: "{{ issue.id }}", url: "<pr-url>", title: "<pr-title>" }`.
"#
    },
  }
}

#[derive(Debug, Error)]
enum InitError {
  #[error("{flag} is required when no prompt answer is available")]
  MissingChoice { flag: &'static str },

  #[error("refusing to overwrite existing file(s): {paths}", paths = display_paths(.paths))]
  WouldOverwrite { paths: Vec<PathBuf> },

  #[error("failed to create directory {path}: {source}")]
  CreateDir {
    path: PathBuf,
    #[source]
    source: io::Error,
  },

  #[error("failed to write {path}: {source}")]
  Write {
    path: PathBuf,
    #[source]
    source: io::Error,
  },

  #[cfg(unix)]
  #[error("failed to inspect {path}: {source}")]
  Metadata {
    path: PathBuf,
    #[source]
    source: io::Error,
  },

  #[cfg(unix)]
  #[error("failed to set executable bit on {path}: {source}")]
  Permissions {
    path: PathBuf,
    #[source]
    source: io::Error,
  },

  #[error(transparent)]
  Io(#[from] io::Error),
}

fn display_paths(paths: &[PathBuf]) -> String {
  paths
    .iter()
    .map(|path| path.display().to_string())
    .collect::<Vec<_>>()
    .join(", ")
}
