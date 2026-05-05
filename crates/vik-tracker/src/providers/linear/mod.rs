mod client;
mod normalize;
mod queries;

#[cfg(test)]
mod tests;

use std::env;

use serde::{Deserialize, Serialize};

use super::TrackerConfigError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinearTrackerConfig {
    pub endpoint: String,
    pub api_key: String,
    pub project_slug: String,
}

impl LinearTrackerConfig {
    pub fn new(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        project_slug: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            project_slug: project_slug.into(),
        }
    }

    pub fn default_endpoint() -> &'static str {
        client::DEFAULT_LINEAR_ENDPOINT
    }

    pub fn api_key_from_env() -> Option<String> {
        env::var("LINEAR_API_KEY").ok()
    }

    pub fn validate(&self) -> Result<(), TrackerConfigError> {
        if self.api_key.trim().is_empty() {
            return Err(TrackerConfigError::MissingApiKey);
        }
        self.validate_without_api_key()
    }

    pub fn validate_without_api_key(&self) -> Result<(), TrackerConfigError> {
        if self.project_slug.trim().is_empty() {
            Err(TrackerConfigError::MissingProjectSlug)
        } else {
            Ok(())
        }
    }

    pub fn has_api_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

pub use client::{
    DEFAULT_LINEAR_ENDPOINT, DEFAULT_PAGE_SIZE, LinearClient, LinearClientConfig,
    LinearIssueFilterConfig,
};
