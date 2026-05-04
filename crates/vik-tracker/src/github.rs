use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::{StreamExt, TryStreamExt, stream};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use serde::Deserialize;
use vik_core::{BlockerRef, Issue, IssueTracker, TrackerError, normalize_state};

pub const DEFAULT_GITHUB_ENDPOINT: &str = "https://api.github.com";
pub const DEFAULT_GITHUB_PAGE_SIZE: usize = 100;

#[derive(Debug, Clone)]
pub struct GitHubClientConfig {
    pub endpoint: String,
    pub api_key: String,
    pub repository: String,
    pub active_states: Vec<String>,
    pub filter: GitHubIssueFilterConfig,
    pub page_size: usize,
}

impl GitHubClientConfig {
    pub fn new(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        repository: impl Into<String>,
        active_states: Vec<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            repository: repository.into(),
            active_states,
            filter: GitHubIssueFilterConfig::default(),
            page_size: DEFAULT_GITHUB_PAGE_SIZE,
        }
    }

    pub fn with_filter(mut self, filter: GitHubIssueFilterConfig) -> Self {
        self.filter = filter;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitHubIssueFilterConfig {
    pub assignees: Vec<String>,
    pub labels: Vec<String>,
}

impl GitHubIssueFilterConfig {
    pub fn new(assignees: Vec<String>, labels: Vec<String>) -> Self {
        Self {
            assignees: clean_filter_values(assignees),
            labels: clean_filter_values(labels),
        }
    }

    pub(crate) fn matches(&self, issue: &GitHubIssueNode) -> bool {
        let assignee_matches = self.assignees.is_empty()
            || issue.assignees.iter().any(|assignee| {
                self.assignees
                    .iter()
                    .any(|wanted| assignee.login.eq_ignore_ascii_case(wanted))
            });
        let label_matches = self.labels.is_empty()
            || issue.labels.iter().any(|label| {
                self.labels
                    .iter()
                    .any(|wanted| label.name.eq_ignore_ascii_case(wanted))
            });
        assignee_matches && label_matches
    }
}

fn clean_filter_values(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

#[derive(Debug, Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    config: GitHubClientConfig,
    owner: String,
    repo: String,
}

impl GitHubClient {
    pub fn new(config: GitHubClientConfig) -> Result<Self, TrackerError> {
        if config.api_key.trim().is_empty() {
            return Err(TrackerError::MissingTrackerApiKey);
        }
        let (owner, repo) = parse_repository(&config.repository)?;
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("vik"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            HeaderName::from_static("x-github-api-version"),
            HeaderValue::from_static("2022-11-28"),
        );
        let auth = HeaderValue::from_str(&format!("Bearer {}", config.api_key.trim()))
            .map_err(|err| TrackerError::GithubApiRequest(err.to_string()))?;
        headers.insert(AUTHORIZATION, auth);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(30_000))
            .default_headers(headers)
            .build()
            .map_err(|err| TrackerError::GithubApiRequest(err.to_string()))?;
        Ok(Self {
            http,
            config,
            owner,
            repo,
        })
    }

    async fn fetch_paginated(&self, state_names: &[String]) -> Result<Vec<Issue>, TrackerError> {
        let states = github_states(state_names);
        let mut issues = Vec::new();
        for state in states {
            let mut page = 1;
            loop {
                let nodes = self.fetch_page(&state, page).await?;
                let count = nodes.len();
                issues.extend(
                    nodes
                        .iter()
                        .filter(|node| node.pull_request.is_none())
                        .filter(|node| self.config.filter.matches(node))
                        .map(|node| normalize_github_issue(&self.config.repository, node)),
                );
                if count < self.config.page_size {
                    break;
                }
                page += 1;
            }
        }
        Ok(issues)
    }

    async fn fetch_page(
        &self,
        state: &str,
        page: usize,
    ) -> Result<Vec<GitHubIssueNode>, TrackerError> {
        let url = format!(
            "{}/repos/{}/{}/issues?state={state}&per_page={}&page={page}",
            self.config.endpoint.trim_end_matches('/'),
            self.owner,
            self.repo,
            self.config.page_size
        );
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|err| TrackerError::GithubApiRequest(err.to_string()))?;
        read_json(response).await
    }

    async fn fetch_issue_number(&self, number: &str) -> Result<Issue, TrackerError> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}",
            self.config.endpoint.trim_end_matches('/'),
            self.owner,
            self.repo,
            number
        );
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|err| TrackerError::GithubApiRequest(err.to_string()))?;
        let node = read_json(response).await?;
        Ok(normalize_github_issue(&self.config.repository, &node))
    }
}

#[async_trait]
impl IssueTracker for GitHubClient {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_paginated(&self.config.active_states).await
    }

    async fn fetch_issues_by_states(
        &self,
        state_names: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        if state_names.is_empty() {
            return Ok(Vec::new());
        }
        self.fetch_paginated(state_names).await
    }

    async fn fetch_issue_states_by_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        stream::iter(issue_ids.iter().cloned())
            .map(|id| async move {
                let number = github_issue_number(&id)?;
                self.fetch_issue_number(&number).await
            })
            .buffer_unordered(8)
            .try_collect()
            .await
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct GitHubIssueNode {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    html_url: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    #[serde(default)]
    labels: Vec<GitHubLabelNode>,
    #[serde(default)]
    assignees: Vec<GitHubUserNode>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GitHubLabelNode {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GitHubUserNode {
    login: String,
}

async fn read_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, TrackerError> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| TrackerError::GithubUnknownPayload(err.to_string()))?;
    if !status.is_success() {
        return Err(TrackerError::GithubApiStatus(status.as_u16(), text));
    }
    serde_json::from_str(&text).map_err(|err| TrackerError::GithubUnknownPayload(err.to_string()))
}

fn parse_repository(repository: &str) -> Result<(String, String), TrackerError> {
    let mut parts = repository.trim().split('/');
    let owner = parts.next().unwrap_or_default().trim();
    let repo = parts.next().unwrap_or_default().trim();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return Err(TrackerError::MissingTrackerRepository);
    }
    Ok((owner.to_string(), repo.to_string()))
}

fn github_states(state_names: &[String]) -> Vec<String> {
    state_names
        .iter()
        .map(|state| state.trim().to_lowercase())
        .filter(|state| !state.is_empty())
        .fold(Vec::<String>::new(), |mut states, state| {
            if !states.contains(&state) {
                states.push(state);
            }
            states
        })
}

pub(crate) fn github_issue_number(id: &str) -> Result<String, TrackerError> {
    let raw = id.trim();
    let after_hash = raw
        .rsplit_once('#')
        .map(|(_, number)| number)
        .unwrap_or_else(|| raw.trim_start_matches('#'));
    let candidate = after_hash
        .strip_prefix("GH-")
        .or_else(|| after_hash.strip_prefix("gh-"))
        .unwrap_or(after_hash);
    if candidate.parse::<u64>().is_ok() {
        Ok(candidate.to_string())
    } else {
        Err(TrackerError::GithubUnknownPayload(format!(
            "invalid github issue id: {id}"
        )))
    }
}

pub(crate) fn normalize_github_issue(_repository: &str, node: &GitHubIssueNode) -> Issue {
    Issue {
        id: node.number.to_string(),
        identifier: format!("GH-{}", node.number),
        title: node.title.clone(),
        description: node.body.clone(),
        priority: None,
        state: node.state.clone(),
        branch_name: None,
        url: node.html_url.clone(),
        labels: node
            .labels
            .iter()
            .map(|label| normalize_state(&label.name))
            .collect(),
        blocked_by: Vec::<BlockerRef>::new(),
        created_at: opt_datetime(node.created_at.as_deref()),
        updated_at: opt_datetime(node.updated_at.as_deref()),
    }
}

fn opt_datetime(raw: Option<&str>) -> Option<DateTime<Utc>> {
    raw.and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}
