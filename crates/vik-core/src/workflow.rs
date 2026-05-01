use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub path: PathBuf,
    pub config: serde_yaml::Mapping,
    pub prompt_template: String,
}
