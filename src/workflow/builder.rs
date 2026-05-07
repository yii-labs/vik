use crate::{hooks::HookRunner, utils, workspace::Workspace};

use super::{Workflow, WorkflowError, loader::LoadedWorkflowSchema};

impl TryFrom<LoadedWorkflowSchema> for Workflow {
  type Error = WorkflowError;

  fn try_from(LoadedWorkflowSchema { path, schema }: LoadedWorkflowSchema) -> Result<Self, Self::Error> {
    // Diagnose runs here, not in the loader, so the doctor command can
    // surface warnings on a parsed-but-not-promoted schema.
    let diagnostics = schema.diagnose();

    if diagnostics.has_errors() {
      return Err(WorkflowError::Diagnose(diagnostics));
    }

    let workflow_dir = path
      .parent()
      .expect("path to workflow.yml must be valid because we've already read from it.")
      .to_path_buf();

    // Workspace root in YAML is optional; the supervisor needs an
    // absolute path so resolution happens here, anchored at the
    // workflow file's directory rather than process cwd.
    let unresolved_workspace_home_dir = schema.workspace.root.clone().unwrap_or_else(utils::paths::default_home);

    let workspace_root_dir = utils::paths::resolve_from(&workflow_dir, &unresolved_workspace_home_dir)
      .ok_or(WorkflowError::WorkspaceRoot(unresolved_workspace_home_dir))?
      .join("workflows")
      .join(path.to_string_lossy().replace('/', "-"));

    Ok(Workflow {
      workflow_dir: workflow_dir.clone(),
      workflow_path: path,
      schema,
      workspace: Workspace::new(workspace_root_dir),
      hooks: HookRunner::new(),
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::workflow::loader::WorkflowSchemaLoader;

  #[test]
  fn workflow_root_resolution() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.yml");
    let loaded = WorkflowSchemaLoader
      .load_from_str(&workflow_yaml("workspace: { root: .vik }"), Some(workflow_path.clone()))
      .expect("workflow schema parses");

    let workflow = Workflow::try_from(loaded).expect("workflow builds");

    assert_eq!(
      workflow.workspace().root(),
      temp
        .path()
        .join(".vik")
        .join("workflows")
        .join(workflow_path.to_string_lossy().replace('/', "-"))
    );
  }

  #[test]
  fn missing_workspace_root_defaults_to_home_vik_and_workflow_namespace() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.yml");
    let loaded = WorkflowSchemaLoader
      .load_from_str(&workflow_yaml("workspace: {}"), Some(workflow_path.clone()))
      .expect("workflow schema parses");

    let workflow = Workflow::try_from(loaded).expect("workflow builds");

    assert_eq!(
      workflow.workspace().root(),
      utils::paths::default_home()
        .join("workflows")
        .join(workflow_path.to_string_lossy().replace('/', "-"))
    );
  }

  #[test]
  fn null_workspace_root_defaults_to_home_vik_and_workflow_namespace() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.yml");
    let loaded = WorkflowSchemaLoader
      .load_from_str(
        &workflow_yaml(
          r#"
workspace:
  root:
"#,
        ),
        Some(workflow_path.clone()),
      )
      .expect("workflow schema parses");

    let workflow = Workflow::try_from(loaded).expect("workflow builds");

    assert_eq!(
      workflow.workspace().root(),
      utils::paths::default_home()
        .join("workflows")
        .join(workflow_path.to_string_lossy().replace('/', "-"))
    );
  }

  fn workflow_yaml(workspace_section: &str) -> String {
    format!(
      r#"
loop:
  max_issue_concurrency: 1
  wait_ms: 100
{workspace_section}
agents:
  codex:
    runtime: codex
    model: gpt-5.5
issues:
  pull:
    command: ./scripts/issues-json
    idle_sec: 5
issue:
  stages:
    plan:
      when:
        state: todo
      agent: codex
      prompt_file: ./prompts/plan.md
"#
    )
  }
}
