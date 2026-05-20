//! `vik init [WORKFLOW]` - generate a starter workflow setup.

use std::fmt::{self, Display};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use inquire::{Select, Text};
use thiserror::Error;

use crate::templates::{self, SkillNameBinding, SkillTemplate, TrackerTemplate, WorkflowTemplate};

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
  /// Work and review stages only.
  Simple,
  /// Plan, work, rework, review, and merge stages.
  Symphony,
  /// Grill, PRD, issues, work, review, and merge stages.
  MattPocock,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum InitTracker {
  /// GitHub Issue workflow using the GitHub CLI.
  Github,
  /// GitHub Projects workflow using Project Status.
  GithubProjects,
  /// Linear workflow using Linear API intake and explicit prompt operations.
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
  let interactive = args.template.is_none() || args.tracker.is_none();
  let generator = InitGenerator {
    workflow_path,
    template,
    tracker,
    force: args.force,
    interactive,
  };

  generator.generate()
}

fn choose_template(choice: Option<InitTemplate>) -> Result<InitTemplate, InitError> {
  match choice {
    Some(choice) => Ok(choice),
    None => prompt_choice(
      "Templates?",
      &[
        Choice::new("Simple: work -> review", InitTemplate::Simple),
        Choice::new(
          "Symphony: plan(rework) -> work -> review -> merge",
          InitTemplate::Symphony,
        ),
        Choice::new(
          "Matt Pocock: grill -> prd -> issues(ready/HITL) -> work -> review -> merge",
          InitTemplate::MattPocock,
        ),
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
        Choice::new("GitHub Projects", InitTracker::GithubProjects),
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

impl<T> Display for Choice<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.label)
  }
}

fn prompt_choice<T: Copy>(title: &str, choices: &[Choice<T>], flag: &'static str) -> Result<T, InitError> {
  Select::new(title, choices.to_vec())
    .without_filtering()
    .prompt()
    .map(|choice| choice.value)
    .map_err(|source| InitError::Prompt { flag, source })
}

struct InitGenerator {
  workflow_path: PathBuf,
  template: InitTemplate,
  tracker: InitTracker,
  force: bool,
  interactive: bool,
}

impl InitGenerator {
  fn generate(&self) -> Result<InitReport, InitError> {
    let workflow_dir = workflow_dir(&self.workflow_path);
    let template = self.template.definition();
    let skill_names = self.resolve_skill_names(&workflow_dir, template)?;
    let files = self.files(&workflow_dir, template, &skill_names);
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
    }

    Ok(InitReport {
      workflow_path: self.workflow_path.clone(),
      files: files.into_iter().map(|file| file.path).collect(),
      overwritten: self.force,
    })
  }

  fn files(
    &self,
    workflow_dir: &Path,
    template: WorkflowTemplate,
    skill_names: &[SkillNameBinding],
  ) -> Vec<GeneratedFile> {
    let prompt_dir = workflow_dir.join(".agents").join("prompts");
    let skill_dir = workflow_dir.join(".agents").join("skills");
    let scripts_dir = workflow_dir.join("scripts");
    let tracker = self.tracker.definition();

    let mut files = vec![
      GeneratedFile::plain(self.workflow_path.clone(), template.render_workflow(tracker)),
      GeneratedFile::plain(
        scripts_dir.join(tracker.script_name()),
        tracker.render_script(template.stages()),
      ),
    ];

    for stage in template.stages() {
      files.push(GeneratedFile::plain(
        prompt_dir.join(format!("{}.md", stage.name)),
        template.render_prompt(*stage, tracker, skill_names),
      ));
    }

    for (skill, binding) in template.skills().iter().zip(skill_names) {
      files.push(GeneratedFile::plain(
        skill_dir.join(&binding.name).join("SKILL.md"),
        skill.render_contents(&binding.name),
      ));
    }

    files
  }

  fn resolve_skill_names(
    &self,
    workflow_dir: &Path,
    template: WorkflowTemplate,
  ) -> Result<Vec<SkillNameBinding>, InitError> {
    let skill_dir = workflow_dir.join(".agents").join("skills");
    template
      .skills()
      .iter()
      .map(|skill| self.resolve_skill_name(&skill_dir, *skill))
      .collect()
  }

  fn resolve_skill_name(&self, skill_dir: &Path, skill: SkillTemplate) -> Result<SkillNameBinding, InitError> {
    if self.force || !skill_dir.join(skill.default_name).exists() {
      return Ok(SkillNameBinding {
        placeholder: skill.placeholder,
        name: skill.default_name.to_string(),
      });
    }

    if !self.interactive {
      return Err(InitError::SkillNameCollision {
        name: skill.default_name.to_string(),
      });
    }

    loop {
      let name = Text::new(&format!("Skill name for {}?", skill.display_name))
        .with_default(&format!("{}-local", skill.default_name))
        .prompt()
        .map_err(|source| InitError::Prompt {
          flag: "--template",
          source,
        })?;
      validate_skill_name(&name)?;

      if !skill_dir.join(&name).exists() {
        return Ok(SkillNameBinding {
          placeholder: skill.placeholder,
          name,
        });
      }

      let _ = writeln!(io::stderr(), "skill name already exists: {name}");
    }
  }
}

fn workflow_dir(path: &Path) -> PathBuf {
  path
    .parent()
    .filter(|parent| !parent.as_os_str().is_empty())
    .unwrap_or_else(|| Path::new("."))
    .to_path_buf()
}

impl InitTemplate {
  fn definition(self) -> WorkflowTemplate {
    match self {
      InitTemplate::Simple => templates::simple::template(),
      InitTemplate::Symphony => templates::symphony::template(),
      InitTemplate::MattPocock => templates::matt_pocock::template(),
    }
  }
}

impl InitTracker {
  fn definition(self) -> TrackerTemplate {
    match self {
      InitTracker::Github => templates::github_tracker(),
      InitTracker::GithubProjects => templates::github_projects_tracker(),
      InitTracker::Linear => templates::linear_tracker(),
    }
  }
}

struct GeneratedFile {
  path: PathBuf,
  contents: String,
}

impl GeneratedFile {
  fn plain(path: PathBuf, contents: String) -> Self {
    Self { path, contents }
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

#[derive(Debug, Error)]
enum InitError {
  #[error("{flag} is required when interactive prompt fails: {source}")]
  Prompt {
    flag: &'static str,
    #[source]
    source: inquire::InquireError,
  },

  #[error("refusing to overwrite existing file(s): {paths}", paths = display_paths(.paths))]
  WouldOverwrite { paths: Vec<PathBuf> },

  #[error(
    "bundled skill name already exists: {name}; rerun interactively to choose another name or pass --force to overwrite it"
  )]
  SkillNameCollision { name: String },

  #[error("invalid skill name `{name}`; use letters, numbers, dot, dash, or underscore")]
  InvalidSkillName { name: String },

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
}

fn display_paths(paths: &[PathBuf]) -> String {
  paths
    .iter()
    .map(|path| path.display().to_string())
    .collect::<Vec<_>>()
    .join(", ")
}

fn validate_skill_name(name: &str) -> Result<(), InitError> {
  if !name.is_empty()
    && name
      .chars()
      .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
  {
    return Ok(());
  }

  Err(InitError::InvalidSkillName { name: name.to_string() })
}
