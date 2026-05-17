//! Stage-keyed runtime prompt sources.
//!
//! Today each stage points to `prompt_file`. The enum shape keeps session
//! rendering keyed by stage name, so future `stages.*.prompt` inline content
//! can add a source variant without changing session ownership again.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use indexmap::IndexMap;
use indexmap::map::Entry;

use crate::config::issue::IssueStageSchema;
use crate::utils;

use super::WorkflowError;

#[derive(Debug, Default, Clone)]
pub struct StagePromptSources {
  sources: IndexMap<String, StagePromptSource>,
  #[cfg(test)]
  loaded_file_count: usize,
}

impl StagePromptSources {
  pub fn load(workflow_dir: &Path, stages: &IndexMap<String, IssueStageSchema>) -> Result<Self, WorkflowError> {
    let mut sources = IndexMap::new();
    let mut file_templates = IndexMap::new();

    for (stage, schema) in stages {
      let path =
        utils::paths::resolve_from(workflow_dir, &schema.prompt_file).ok_or_else(|| WorkflowError::PromptPath {
          stage: stage.clone(),
          path: schema.prompt_file.clone(),
        })?;

      let template = match file_templates.entry(path.clone()) {
        Entry::Occupied(entry) => Arc::clone(entry.get()),
        Entry::Vacant(entry) => {
          let template = fs::read_to_string(&path).map_err(|source| WorkflowError::PromptRead {
            stage: stage.clone(),
            path: path.clone(),
            source,
          })?;
          Arc::clone(entry.insert(Arc::<str>::from(template)))
        },
      };

      sources.insert(stage.clone(), StagePromptSource::file(template));
    }

    Ok(Self {
      sources,
      #[cfg(test)]
      loaded_file_count: file_templates.len(),
    })
  }

  pub fn get(&self, stage: &str) -> Option<&str> {
    self.sources.get(stage).map(StagePromptSource::template)
  }

  #[cfg(test)]
  pub fn is_empty(&self) -> bool {
    self.sources.is_empty()
  }

  #[cfg(test)]
  pub fn loaded_file_count(&self) -> usize {
    self.loaded_file_count
  }
}

#[derive(Debug, Clone)]
pub enum StagePromptSource {
  File(FilePromptSource),
}

impl StagePromptSource {
  fn file(template: Arc<str>) -> Self {
    Self::File(FilePromptSource { template })
  }

  fn template(&self) -> &str {
    match self {
      Self::File(source) => &source.template,
    }
  }
}

#[derive(Debug, Clone)]
pub struct FilePromptSource {
  template: Arc<str>,
}

#[cfg(test)]
mod tests {
  use std::path::Path;

  use super::super::*;

  fn workflow_at(workflow_path: &Path) -> Workflow {
    Workflow::builder()
      .workflow_path(workflow_path)
      .workspace_root(workflow_path.parent().expect("workflow has parent").join(".vik"))
      .add_stage("plan", "todo", "./prompts/shared.md")
      .add_stage("implement", "work", "./prompts/shared.md")
      .build()
  }

  #[test]
  fn explicit_runtime_load_reads_prompt_files_by_stage_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.yml");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("prompts dir");
    std::fs::write(prompts_dir.join("shared.md"), "Prompt for {{ issue.id }}").expect("prompt file");

    let workflow = workflow_at(&workflow_path).load().expect("runtime load");

    assert_eq!(
      workflow.prompt_for_stage("plan").expect("plan prompt"),
      "Prompt for {{ issue.id }}"
    );
    assert_eq!(
      workflow.prompt_for_stage("implement").expect("implement prompt"),
      "Prompt for {{ issue.id }}"
    );
  }

  #[test]
  fn explicit_runtime_load_dedupes_repeated_resolved_prompt_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.yml");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("prompts dir");
    std::fs::write(prompts_dir.join("shared.md"), "same prompt").expect("prompt file");

    let workflow = workflow_at(&workflow_path).load().expect("runtime load");

    assert_eq!(workflow.prompt_sources().loaded_file_count(), 1);
  }

  #[test]
  fn explicit_runtime_load_reports_missing_prompt_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.yml");
    let workflow = workflow_at(&workflow_path);

    let err = workflow.load().expect_err("missing prompt file should fail runtime load");

    assert!(matches!(err, WorkflowError::PromptRead { stage, .. } if stage == "plan"));
  }
}
