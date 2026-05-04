use std::fmt;
use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use vik_core::{Issue, IssueTracker, TrackerError};

pub mod github;
pub mod linear;

#[derive(Debug, Error)]
pub enum TrackerConfigError {
    #[error("unsupported_tracker_kind")]
    UnsupportedTrackerKind,
    #[error("missing_tracker_api_key")]
    MissingApiKey,
    #[error("missing_tracker_project_slug")]
    MissingProjectSlug,
    #[error("missing_tracker_repository")]
    MissingRepository,
    #[error("invalid_tracker_repository: {0}")]
    InvalidRepository(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerFilterConfig {
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommonTrackerConfig {
    pub active_states: Vec<String>,
    pub terminal_states: Vec<String>,
    #[serde(default)]
    pub filter: TrackerFilterConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackerKind {
    Linear(linear::LinearTrackerConfig),
    GitHub(github::GitHubTrackerConfig),
    Unsupported(String),
}

impl TrackerKind {
    pub fn name(&self) -> &str {
        match self {
            Self::Linear(_) => "linear",
            Self::GitHub(_) => "github",
            Self::Unsupported(kind) => kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerConfig {
    pub common: CommonTrackerConfig,
    pub kind: TrackerKind,
}

impl TrackerConfig {
    pub fn linear(common: CommonTrackerConfig, provider: linear::LinearTrackerConfig) -> Self {
        Self {
            common,
            kind: TrackerKind::Linear(provider),
        }
    }

    pub fn github(common: CommonTrackerConfig, provider: github::GitHubTrackerConfig) -> Self {
        Self {
            common,
            kind: TrackerKind::GitHub(provider),
        }
    }

    pub fn unsupported(common: CommonTrackerConfig, kind: impl Into<String>) -> Self {
        Self {
            common,
            kind: TrackerKind::Unsupported(kind.into()),
        }
    }

    pub fn kind_name(&self) -> &str {
        self.kind.name()
    }

    pub fn active_states(&self) -> &[String] {
        &self.common.active_states
    }

    pub fn terminal_states(&self) -> &[String] {
        &self.common.terminal_states
    }

    pub fn filter(&self) -> &TrackerFilterConfig {
        &self.common.filter
    }

    pub fn linear_provider(&self) -> Option<&linear::LinearTrackerConfig> {
        match &self.kind {
            TrackerKind::Linear(config) => Some(config),
            _ => None,
        }
    }

    pub fn github_provider(&self) -> Option<&github::GitHubTrackerConfig> {
        match &self.kind {
            TrackerKind::GitHub(config) => Some(config),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), TrackerConfigError> {
        match &self.kind {
            TrackerKind::Linear(config) => config.validate(),
            TrackerKind::GitHub(config) => config.validate(),
            TrackerKind::Unsupported(_) => Err(TrackerConfigError::UnsupportedTrackerKind),
        }
    }
}

impl fmt::Display for TrackerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueUpdate {
    pub state: Option<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueComment {
    pub id: String,
    pub body: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueAttachment {
    pub url: String,
    pub comment: Option<IssueComment>,
}

#[async_trait]
pub trait Tracker: Send + Sync + 'static {
    async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError>;
    async fn fetch_by_states(&self, state_names: &[String]) -> Result<Vec<Issue>, TrackerError>;
    async fn fetch_states_by_ids(&self, issue_ids: &[String]) -> Result<Vec<Issue>, TrackerError>;
    async fn get_issue(&self, issue_id: &str) -> Result<Issue, TrackerError>;
    async fn update_issue(
        &self,
        issue_id: &str,
        update: IssueUpdate,
    ) -> Result<Issue, TrackerError>;
    async fn create_comment(
        &self,
        issue_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError>;
    async fn update_comment(
        &self,
        comment_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError>;
    async fn upload_attachment(
        &self,
        issue_id: &str,
        path: &Path,
        content_type: &str,
    ) -> Result<IssueAttachment, TrackerError>;
    async fn link_pr(&self, issue_id: &str, title: &str, url: &str) -> Result<(), TrackerError>;
}

pub struct TrackerClient {
    inner: Box<dyn Tracker>,
}

impl TrackerClient {
    pub fn new(inner: Box<dyn Tracker>) -> Self {
        Self { inner }
    }
}

impl std::fmt::Debug for TrackerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackerClient").finish_non_exhaustive()
    }
}

#[async_trait]
impl Tracker for TrackerClient {
    async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_candidates().await
    }

    async fn fetch_by_states(&self, state_names: &[String]) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_by_states(state_names).await
    }

    async fn fetch_states_by_ids(&self, issue_ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_states_by_ids(issue_ids).await
    }

    async fn get_issue(&self, issue_id: &str) -> Result<Issue, TrackerError> {
        self.inner.get_issue(issue_id).await
    }

    async fn update_issue(
        &self,
        issue_id: &str,
        update: IssueUpdate,
    ) -> Result<Issue, TrackerError> {
        self.inner.update_issue(issue_id, update).await
    }

    async fn create_comment(
        &self,
        issue_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        self.inner.create_comment(issue_id, body).await
    }

    async fn update_comment(
        &self,
        comment_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        self.inner.update_comment(comment_id, body).await
    }

    async fn upload_attachment(
        &self,
        issue_id: &str,
        path: &Path,
        content_type: &str,
    ) -> Result<IssueAttachment, TrackerError> {
        self.inner
            .upload_attachment(issue_id, path, content_type)
            .await
    }

    async fn link_pr(&self, issue_id: &str, title: &str, url: &str) -> Result<(), TrackerError> {
        self.inner.link_pr(issue_id, title, url).await
    }
}

#[async_trait]
impl IssueTracker for TrackerClient {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_candidates().await
    }

    async fn fetch_issues_by_states(
        &self,
        state_names: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_by_states(state_names).await
    }

    async fn fetch_issue_states_by_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_states_by_ids(issue_ids).await
    }
}
