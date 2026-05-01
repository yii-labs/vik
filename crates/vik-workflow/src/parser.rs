use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_yaml::{Mapping, Value as YamlValue};
use vik_core::WorkflowDefinition;

use crate::{ServiceConfig, WorkflowError};

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedWorkflow {
    pub definition: WorkflowDefinition,
    pub config: ServiceConfig,
    pub modified_at: Option<SystemTime>,
}

pub fn default_workflow_path() -> PathBuf {
    PathBuf::from("WORKFLOW.md")
}

pub fn select_workflow_path(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(default_workflow_path)
}

pub fn load_workflow(explicit: Option<PathBuf>) -> Result<WorkflowDefinition, WorkflowError> {
    let path = select_workflow_path(explicit);
    parse_workflow_file(&path)
}

pub fn parse_workflow_file(path: &Path) -> Result<WorkflowDefinition, WorkflowError> {
    let content = fs::read_to_string(path).map_err(|_| WorkflowError::MissingWorkflowFile)?;
    parse_workflow_content(path.to_path_buf(), &content)
}

pub fn parse_workflow_content(
    path: PathBuf,
    content: &str,
) -> Result<WorkflowDefinition, WorkflowError> {
    let (config, body) = if let Some(rest) = content.strip_prefix("---") {
        let rest = rest.strip_prefix('\n').unwrap_or(rest);
        let mut front = Vec::new();
        let mut body = Vec::new();
        let mut found_end = false;
        for line in rest.lines() {
            if !found_end && line.trim() == "---" {
                found_end = true;
                continue;
            }
            if found_end {
                body.push(line);
            } else {
                front.push(line);
            }
        }
        if !found_end {
            return Err(WorkflowError::WorkflowParseError(
                "missing closing front matter delimiter".to_string(),
            ));
        }
        let yaml: YamlValue = serde_yaml::from_str(&front.join("\n"))
            .map_err(|err| WorkflowError::WorkflowParseError(err.to_string()))?;
        let map = yaml
            .as_mapping()
            .cloned()
            .ok_or(WorkflowError::WorkflowFrontMatterNotAMap)?;
        (map, body.join("\n"))
    } else {
        (Mapping::new(), content.to_string())
    };

    Ok(WorkflowDefinition {
        path,
        config,
        prompt_template: body.trim().to_string(),
    })
}

pub fn load_effective_workflow(explicit: Option<PathBuf>) -> Result<LoadedWorkflow, WorkflowError> {
    let definition = load_workflow(explicit)?;
    let modified_at = fs::metadata(&definition.path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    let config = ServiceConfig::from_definition(&definition)?;
    Ok(LoadedWorkflow {
        definition,
        config,
        modified_at,
    })
}
