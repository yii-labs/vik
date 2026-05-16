use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use indexmap::IndexMap;

use super::WorkflowError;
use crate::config::issue::IssueStageSchema;
use crate::utils;

#[derive(Debug, Default)]
pub struct StagePromptSources {
  by_stage: IndexMap<String, StagePromptSource>,
  file_templates: IndexMap<PathBuf, Arc<str>>,
}

// Stage-keyed so future inline `stages.*.prompt` can become another
// source variant without changing session rendering.
#[derive(Debug)]
enum StagePromptSource {
  File(PathBuf),
}

impl StagePromptSources {
  pub(super) fn load<'a>(
    workflow_dir: &Path,
    stages: impl Iterator<Item = (&'a String, &'a IssueStageSchema)>,
  ) -> Result<Self, WorkflowError> {
    let mut by_stage = IndexMap::new();
    let mut file_templates = IndexMap::new();

    for (stage_name, stage) in stages {
      let prompt_path = resolve_prompt_path(workflow_dir, &stage.prompt_file)?;

      if !file_templates.contains_key(&prompt_path) {
        let template =
          fs::read_to_string(&prompt_path).map_err(|err| WorkflowError::PromptRead(prompt_path.clone(), err))?;
        file_templates.insert(prompt_path.clone(), Arc::from(template));
      }

      by_stage.insert(stage_name.clone(), StagePromptSource::File(prompt_path));
    }

    Ok(Self {
      by_stage,
      file_templates,
    })
  }

  pub(super) fn template_for_stage(&self, stage_name: &str) -> Result<&str, WorkflowError> {
    let source = self
      .by_stage
      .get(stage_name)
      .ok_or_else(|| WorkflowError::PromptNotLoaded(stage_name.to_string()))?;

    match source {
      StagePromptSource::File(prompt_path) => self
        .file_templates
        .get(prompt_path)
        .map(AsRef::as_ref)
        .ok_or_else(|| WorkflowError::PromptNotLoaded(stage_name.to_string())),
    }
  }

  #[cfg(test)]
  pub(super) fn stage_count(&self) -> usize {
    self.by_stage.len()
  }

  #[cfg(test)]
  pub(super) fn file_count(&self) -> usize {
    self.file_templates.len()
  }
}

fn resolve_prompt_path(workflow_dir: &Path, prompt_file: &Path) -> Result<PathBuf, WorkflowError> {
  utils::paths::resolve_from(workflow_dir, prompt_file)
    .ok_or_else(|| WorkflowError::PromptPath(prompt_file.to_path_buf()))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn stage(prompt_file: impl Into<PathBuf>) -> IssueStageSchema {
    IssueStageSchema::new("todo").with_prompt_file(prompt_file)
  }

  #[test]
  fn load_deduplicates_repeated_prompt_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompt_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompt_dir).expect("prompt dir");
    std::fs::write(prompt_dir.join("plan.md"), "plan prompt").expect("prompt file");
    let mut stages = IndexMap::new();
    stages.insert("plan".to_string(), stage("./prompts/plan.md"));
    stages.insert("implement".to_string(), stage("./prompts/plan.md"));

    let prompts = StagePromptSources::load(temp.path(), stages.iter()).expect("prompts load");

    assert_eq!(prompts.stage_count(), 2);
    assert_eq!(prompts.file_count(), 1);
    assert_eq!(
      prompts.template_for_stage("plan").expect("plan prompt exists"),
      "plan prompt"
    );
    assert_eq!(
      prompts.template_for_stage("implement").expect("implement prompt exists"),
      "plan prompt"
    );
  }

  #[test]
  fn load_fails_when_prompt_file_is_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut stages = IndexMap::new();
    stages.insert("plan".to_string(), stage("./prompts/missing.md"));
    let expected_prompt = temp.path().join("prompts/missing.md");

    let err = StagePromptSources::load(temp.path(), stages.iter()).expect_err("missing prompt fails");

    assert!(matches!(
      err,
      WorkflowError::PromptRead(path, _) if path == expected_prompt
    ));
  }

  #[test]
  fn get_reports_unloaded_prompt() {
    let err = StagePromptSources::default()
      .template_for_stage("plan")
      .expect_err("prompt is not loaded");

    assert!(matches!(
      err,
      WorkflowError::PromptNotLoaded(stage_name) if stage_name == "plan"
    ));
  }
}
