mod client;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use super::TrackerConfigError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuFieldsMap {
    pub identifier: String,
    pub title: String,
    pub description: String,
    pub state: String,
    pub delegated: String,
    pub labels: String,
    pub comments: String,
    pub pr_links: String,
}

impl Default for FeishuFieldsMap {
    fn default() -> Self {
        Self {
            identifier: String::new(),
            title: "Title".to_string(),
            description: String::new(),
            state: "State".to_string(),
            delegated: String::new(),
            labels: "Labels".to_string(),
            comments: "Workpad".to_string(),
            pr_links: "PR Links".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeishuTrackerConfig {
    pub cli_path: String,
    pub base_token: String,
    pub table_id: String,
    pub view_id: String,
    /// `lark-cli --as` identity type. Supported values are `user` and `bot`.
    pub identity: String,
    #[serde(rename = "fieldsMap")]
    pub fields_map: FeishuFieldsMap,
}

impl FeishuTrackerConfig {
    pub fn new(base_token: impl Into<String>, table_id: impl Into<String>) -> Self {
        Self {
            cli_path: "lark-cli".to_string(),
            base_token: base_token.into(),
            table_id: table_id.into(),
            view_id: String::new(),
            identity: "user".to_string(),
            fields_map: FeishuFieldsMap::default(),
        }
    }

    pub fn validate(&self) -> Result<(), TrackerConfigError> {
        if self.cli_path.trim().is_empty() {
            return Err(TrackerConfigError::MissingCliPath);
        }
        if self.base_token.trim().is_empty() {
            return Err(TrackerConfigError::MissingBaseToken);
        }
        if self.table_id.trim().is_empty() {
            return Err(TrackerConfigError::MissingTableId);
        }
        if !matches!(self.identity.as_str(), "user" | "bot") {
            return Err(TrackerConfigError::InvalidCliIdentity(
                self.identity.clone(),
            ));
        }
        Ok(())
    }
}

pub use client::{FeishuClient, FeishuClientConfig, FeishuIssueFields};
