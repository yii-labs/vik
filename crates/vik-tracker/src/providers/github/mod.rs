mod client;
mod queries;

#[cfg(test)]
mod tests;

use std::env;

use serde::{Deserialize, Serialize};

use super::TrackerConfigError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubTrackerConfig {
    pub endpoint: String,
    pub api_key: String,
    pub repository: String,
}

impl GitHubTrackerConfig {
    pub fn new(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        repository: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            repository: repository.into(),
        }
    }

    pub fn default_endpoint() -> &'static str {
        client::DEFAULT_GITHUB_ENDPOINT
    }

    pub fn api_key_from_env() -> Option<String> {
        env::var("GH_TOKEN")
            .ok()
            .or_else(|| env::var("GITHUB_TOKEN").ok())
    }

    pub fn api_key_env_names() -> &'static [&'static str] {
        &["GH_TOKEN", "GITHUB_TOKEN"]
    }

    pub fn validate(&self) -> Result<(), TrackerConfigError> {
        if self.api_key.trim().is_empty() {
            return Err(TrackerConfigError::MissingApiKey);
        }
        self.validate_without_api_key()
    }

    pub fn validate_without_api_key(&self) -> Result<(), TrackerConfigError> {
        if self.repository.trim().is_empty() {
            return Err(TrackerConfigError::MissingRepository);
        }
        client::GitHubRepository::parse(&self.repository)
            .map(|_| ())
            .map_err(|_| TrackerConfigError::InvalidRepository(self.repository.clone()))
    }

    pub fn has_api_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

pub(super) use client::{GitHubClient, GitHubClientConfig, GitHubIssueFilterConfig};
