use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use serde::Deserialize;
use serde_json::{Value, json};
use vik_core::{BlockerRef, Issue, IssueTracker, TrackerError, normalize_state};

pub const DEFAULT_GITHUB_ENDPOINT: &str = "https://api.github.com";
pub const DEFAULT_GITHUB_PAGE_SIZE: usize = 100;
pub(crate) const GITHUB_ISSUES_BY_IDS_QUERY: &str = r#"
query VikGitHubIssuesByIds($ids: [ID!]!) {
  nodes(ids: $ids) {
    ... on Issue {
      id
      number
      title
      body
      state
      url
      createdAt
      updatedAt
      labels(first: 100) {
        nodes {
          name
        }
      }
    }
  }
}
"#;
pub(crate) const GITHUB_ISSUES_BY_STATES_QUERY: &str = r#"
query VikGitHubIssuesByStates(
  $owner: String!
  $repo: String!
  $states: [IssueState!]
  $first: Int!
  $after: String
) {
  repository(owner: $owner, name: $repo) {
    issues(
      first: $first
      after: $after
      states: $states
      orderBy: { field: UPDATED_AT, direction: DESC }
    ) {
      nodes {
        id
        number
        title
        body
        state
        url
        createdAt
        updatedAt
        labels(first: 100) {
          nodes {
            name
          }
        }
        assignees(first: 100) {
          nodes {
            login
          }
        }
      }
      pageInfo {
        hasNextPage
        endCursor
      }
    }
  }
}
"#;

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

    #[cfg(test)]
    pub(crate) fn matches(&self, issue: &GitHubIssueNode) -> bool {
        self.matches_values(&issue.assignees, &issue.labels)
    }

    fn matches_graphql(&self, issue: &GitHubGraphqlIssueNode) -> bool {
        self.matches_values(&issue.assignees.nodes, &issue.labels.nodes)
    }

    fn matches_values(&self, assignees: &[GitHubUserNode], labels: &[GitHubLabelNode]) -> bool {
        let assignee_matches = self.assignees.is_empty()
            || assignees.iter().any(|assignee| {
                self.assignees
                    .iter()
                    .any(|wanted| assignee.login.eq_ignore_ascii_case(wanted))
            });
        let label_matches = self.labels.is_empty()
            || labels.iter().any(|label| {
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

    async fn fetch_paginated(
        &self,
        state_names: &[String],
        apply_filter: bool,
    ) -> Result<Vec<Issue>, TrackerError> {
        let states = github_graphql_states(state_names);
        let mut issues = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = self
                .fetch_graphql_issue_page(&states, after.as_deref())
                .await?;
            issues.extend(
                page.nodes
                    .into_iter()
                    .flatten()
                    .filter(|node| !apply_filter || self.config.filter.matches_graphql(node))
                    .map(|node| normalize_github_graphql_issue(&node)),
            );
            if !page.page_info.has_next_page {
                break;
            }
            after = page.page_info.end_cursor;
        }
        Ok(issues)
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

    async fn fetch_issues_by_graphql_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        let body = json!({
            "query": GITHUB_ISSUES_BY_IDS_QUERY,
            "variables": {
                "ids": issue_ids,
            },
        });
        let response = self
            .http
            .post(github_graphql_endpoint(&self.config.endpoint))
            .json(&body)
            .send()
            .await
            .map_err(|err| TrackerError::GithubApiRequest(err.to_string()))?;
        let payload: GitHubGraphqlResponse = read_json(response).await?;
        if let Some(errors) = payload.errors {
            return Err(TrackerError::GithubUnknownPayload(format!(
                "github_graphql_errors: {}",
                compact_json(&errors)
            )));
        }
        let data = payload.data.ok_or_else(|| {
            TrackerError::GithubUnknownPayload("missing github graphql data".to_string())
        })?;
        Ok(data
            .nodes
            .into_iter()
            .flatten()
            .map(|node| normalize_github_graphql_issue(&node))
            .collect())
    }

    async fn fetch_graphql_issue_page(
        &self,
        states: &[String],
        after: Option<&str>,
    ) -> Result<GitHubGraphqlIssueConnection, TrackerError> {
        let body = json!({
            "query": GITHUB_ISSUES_BY_STATES_QUERY,
            "variables": {
                "owner": self.owner,
                "repo": self.repo,
                "states": states,
                "first": self.config.page_size,
                "after": after,
            },
        });
        let response = self
            .http
            .post(github_graphql_endpoint(&self.config.endpoint))
            .json(&body)
            .send()
            .await
            .map_err(|err| TrackerError::GithubApiRequest(err.to_string()))?;
        let payload: GitHubIssuesByStatesResponse = read_json(response).await?;
        if let Some(errors) = payload.errors {
            return Err(TrackerError::GithubUnknownPayload(format!(
                "github_graphql_errors: {}",
                compact_json(&errors)
            )));
        }
        let data = payload.data.ok_or_else(|| {
            TrackerError::GithubUnknownPayload("missing github graphql data".to_string())
        })?;
        let repository = data.repository.ok_or_else(|| {
            TrackerError::GithubUnknownPayload("missing github graphql repository".to_string())
        })?;
        Ok(repository.issues)
    }
}

#[async_trait]
impl IssueTracker for GitHubClient {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_paginated(&self.config.active_states, true).await
    }

    async fn fetch_issues_by_states(
        &self,
        state_names: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        if state_names.is_empty() {
            return Ok(Vec::new());
        }
        self.fetch_paginated(state_names, false).await
    }

    async fn fetch_issue_states_by_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        if issue_ids.is_empty() {
            return Ok(Vec::new());
        }
        let (graphql_ids, numeric_ids): (Vec<_>, Vec<_>) = issue_ids
            .iter()
            .cloned()
            .partition(|id| !is_numeric_github_issue_ref(id));

        let mut issues = if graphql_ids.is_empty() {
            Vec::new()
        } else {
            self.fetch_issues_by_graphql_ids(&graphql_ids).await?
        };
        for id in numeric_ids {
            let number = github_issue_number(&id)?;
            let mut issue = self.fetch_issue_number(&number).await?;
            issue.id = id;
            issues.push(issue);
        }
        Ok(issues)
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct GitHubIssueNode {
    node_id: Option<String>,
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    html_url: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    #[serde(default)]
    labels: Vec<GitHubLabelNode>,
    #[cfg(test)]
    #[serde(default)]
    assignees: Vec<GitHubUserNode>,
}

#[derive(Debug, Deserialize)]
struct GitHubGraphqlResponse {
    data: Option<GitHubGraphqlData>,
    errors: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct GitHubGraphqlData {
    nodes: Vec<Option<GitHubGraphqlIssueNode>>,
}

#[derive(Debug, Deserialize)]
struct GitHubIssuesByStatesResponse {
    data: Option<GitHubIssuesByStatesData>,
    errors: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct GitHubIssuesByStatesData {
    repository: Option<GitHubGraphqlRepositoryNode>,
}

#[derive(Debug, Deserialize)]
struct GitHubGraphqlRepositoryNode {
    issues: GitHubGraphqlIssueConnection,
}

#[derive(Debug, Deserialize)]
struct GitHubGraphqlIssueConnection {
    nodes: Vec<Option<GitHubGraphqlIssueNode>>,
    #[serde(rename = "pageInfo")]
    page_info: GitHubGraphqlPageInfo,
}

#[derive(Debug, Deserialize)]
struct GitHubGraphqlPageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubGraphqlIssueNode {
    id: String,
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    url: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    #[serde(default)]
    labels: GitHubGraphqlLabelConnection,
    #[serde(default)]
    assignees: GitHubGraphqlUserConnection,
}

#[derive(Debug, Default, Deserialize)]
struct GitHubGraphqlLabelConnection {
    #[serde(default)]
    nodes: Vec<GitHubLabelNode>,
}

#[derive(Debug, Default, Deserialize)]
struct GitHubGraphqlUserConnection {
    #[serde(default)]
    nodes: Vec<GitHubUserNode>,
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

pub(crate) fn github_graphql_endpoint(rest_endpoint: &str) -> String {
    let endpoint = rest_endpoint.trim_end_matches('/');
    if let Some(base) = endpoint.strip_suffix("/api/v3") {
        format!("{base}/api/graphql")
    } else {
        format!("{endpoint}/graphql")
    }
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

fn github_graphql_states(state_names: &[String]) -> Vec<String> {
    github_states(state_names)
        .into_iter()
        .filter_map(|state| match state.as_str() {
            "open" => Some("OPEN".to_string()),
            "closed" => Some("CLOSED".to_string()),
            _ => None,
        })
        .collect()
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

fn is_numeric_github_issue_ref(id: &str) -> bool {
    github_issue_number(id).is_ok()
}

pub(crate) fn normalize_github_issue(_repository: &str, node: &GitHubIssueNode) -> Issue {
    Issue {
        id: node
            .node_id
            .clone()
            .unwrap_or_else(|| node.number.to_string()),
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

fn normalize_github_graphql_issue(node: &GitHubGraphqlIssueNode) -> Issue {
    Issue {
        id: node.id.clone(),
        identifier: format!("GH-{}", node.number),
        title: node.title.clone(),
        description: node.body.clone(),
        priority: None,
        state: node.state.to_lowercase(),
        branch_name: None,
        url: node.url.clone(),
        labels: node
            .labels
            .nodes
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

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}
