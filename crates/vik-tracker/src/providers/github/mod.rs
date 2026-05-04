mod client;
mod queries;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use super::TrackerConfigError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubTrackerConfig {
    pub repository: String,
}

impl GitHubTrackerConfig {
    pub fn new(repository: impl Into<String>) -> Self {
        Self {
            repository: repository.into(),
        }
    }

    pub fn validate(&self) -> Result<(), TrackerConfigError> {
        if self.repository.trim().is_empty() {
            return Err(TrackerConfigError::MissingRepository);
        }
        client::GitHubRepository::parse(&self.repository)
            .map(|_| ())
            .map_err(|_| TrackerConfigError::InvalidRepository(self.repository.clone()))
    }
}

pub use client::{
    DEFAULT_GITHUB_ENDPOINT, GitHubClient, GitHubClientConfig, GitHubIssueFilterConfig,
};
