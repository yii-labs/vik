//! Two-stage parse: file → YAML → [`LoadedWorkflowSchema`].
//!
//! `LoadedWorkflowSchema` is the seam used by `vik doctor` (which just
//! wants the schema and path) and by [`super::builder`] (which builds the
//! full runtime [`super::Workflow`]). Both modes share the same parser so
//! diagnostics agree across CLI commands.

use std::fs;
use std::path::{Path, PathBuf};

use super::{Workflow, WorkflowError};
use crate::config::WorkflowSchema;
use crate::hooks::HookRunner;
use crate::workspace::Workspace;
use crate::{logging, utils};

#[derive(Debug)]
pub struct WorkflowSchemaLoader;

#[derive(Debug)]
pub struct LoadedWorkflowSchema {
  pub path: PathBuf,
  pub schema: WorkflowSchema,
}

impl WorkflowSchemaLoader {
  pub fn load(&self, path: &Path) -> Result<LoadedWorkflowSchema, super::WorkflowError> {
    let abs_path = self.canonicalize(path)?;
    tracing::debug!(phase=%logging::Phase::Startup, "Workflow path was canonicalized to {abs_path:?}.");
    self.ensure_valid_workflow_path(&abs_path)?;
    tracing::debug!(phase=%logging::Phase::Startup, "Workflow definition file exists and is readable.");

    let contents = fs::read_to_string(&abs_path).map_err(|err| WorkflowError::Read(abs_path.clone(), err))?;
    tracing::debug!(phase=%logging::Phase::Startup, "Workflow definition file was read successfully.");

    self.load_from_str(&contents, Some(abs_path))
  }

  pub fn load_from_str(
    &self,
    contents: &str,
    path: Option<PathBuf>,
  ) -> Result<LoadedWorkflowSchema, super::WorkflowError> {
    // Tests pass `None`; YAML errors still need *some* path to print, so
    // a virtual placeholder stands in. Never dereferenced.
    let abs_path = path.unwrap_or_else(|| "/virtual/path/workflow.yml".into());

    let value: WorkflowSchema =
      serde_yaml::from_str(contents).map_err(|err| WorkflowError::Yaml(abs_path.clone(), err))?;

    Ok(LoadedWorkflowSchema {
      path: abs_path,
      schema: value,
    })
  }

  fn ensure_valid_workflow_path(&self, path: &Path) -> Result<(), WorkflowError> {
    let metadata = path.metadata();

    match metadata {
      Ok(metadata) => {
        if metadata.is_dir() {
          Err(WorkflowError::IsDirectory(path.to_path_buf()))
        } else {
          Ok(())
        }
      },
      Err(err) => match err.kind() {
        std::io::ErrorKind::NotFound => Err(WorkflowError::NotFound(path.to_path_buf())),
        std::io::ErrorKind::PermissionDenied => Err(WorkflowError::PermissionDenied(path.to_path_buf())),
        _ => Err(WorkflowError::Read(path.to_path_buf(), err)),
      },
    }
  }

  /// `std::fs::canonicalize` requires the target to exist, which breaks
  /// test fixtures that never touch disk. This is a pure path
  /// transformation: relative paths get joined onto the cwd, absolute
  /// paths pass through unchanged.
  fn canonicalize(&self, path: &Path) -> Result<PathBuf, WorkflowError> {
    if path.is_absolute() {
      return Ok(path.to_path_buf());
    }

    let cwd = std::env::current_dir().map_err(|err| WorkflowError::PathResolution(path.to_path_buf(), err))?;

    Ok(utils::paths::canonicalize_from(&cwd, path))
  }
}

impl TryFrom<LoadedWorkflowSchema> for Workflow {
  type Error = WorkflowError;

  fn try_from(LoadedWorkflowSchema { path, schema }: LoadedWorkflowSchema) -> Result<Self, Self::Error> {
    // Diagnose runs here, not in the loader, so the doctor command can
    // surface warnings on a parsed-but-not-promoted schema.
    let diagnostics = schema.diagnose();

    if diagnostics.has_errors() {
      return Err(WorkflowError::Diagnose(diagnostics));
    }

    Workflow::from_schema_unchecked(path, schema)
  }
}

impl Workflow {
  pub(super) fn from_schema_unchecked(path: PathBuf, schema: WorkflowSchema) -> Result<Self, WorkflowError> {
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
