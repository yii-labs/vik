mod client;
mod normalize;
mod queries;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use super::TrackerConfigError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinearTrackerConfig {
    pub project_slug: String,
}

impl LinearTrackerConfig {
    pub fn new(project_slug: impl Into<String>) -> Self {
        Self {
            project_slug: project_slug.into(),
        }
    }

    pub fn validate(&self) -> Result<(), TrackerConfigError> {
        if self.project_slug.trim().is_empty() {
            Err(TrackerConfigError::MissingProjectSlug)
        } else {
            Ok(())
        }
    }
}

pub use client::{
    DEFAULT_LINEAR_ENDPOINT, DEFAULT_PAGE_SIZE, LinearClient, LinearClientConfig,
    LinearIssueFilterConfig,
};
