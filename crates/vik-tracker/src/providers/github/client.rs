use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Method;
use serde_json::{Map, Value, json};
use vik_core::{Issue, IssueTracker, TrackerError, normalize_state};

use crate::providers::{IssueAttachment, IssueComment, IssueUpdate, Tracker};

use super::queries::{SEARCH_ISSUES_PATH, issue_comment_path, issue_comments_path, issue_path};

pub const DEFAULT_GITHUB_ENDPOINT: &str = "https://api.github.com";
const GITHUB_USER_AGENT: &str = "vik-tracker/0.1";
const DEFAULT_PAGE_SIZE: u64 = 100;
const STATE_REFRESH_CONCURRENCY: usize = 8;

#[derive(Debug, Clone)]
pub struct GitHubClientConfig {
    pub endpoint: String,
    pub api_key: String,
    pub repository: String,
    pub active_states: Vec<String>,
    pub terminal_states: Vec<String>,
    pub filter: GitHubIssueFilterConfig,
    pub page_size: u64,
}

impl GitHubClientConfig {
    pub fn new(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        repository: impl Into<String>,
        active_states: Vec<String>,
        terminal_states: Vec<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            repository: repository.into(),
            active_states,
            terminal_states,
            filter: GitHubIssueFilterConfig::default(),
            page_size: DEFAULT_PAGE_SIZE,
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
    pub tags: Vec<String>,
}

impl GitHubIssueFilterConfig {
    pub fn new(assignees: Vec<String>, tags: Vec<String>) -> Self {
        Self {
            assignees: clean_values(assignees),
            tags: clean_values(tags),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitHubRepository {
    pub(crate) owner: String,
    pub(crate) name: String,
}

impl GitHubRepository {
    pub(crate) fn parse(raw: &str) -> Result<Self, TrackerError> {
        let mut value = raw.trim().trim_end_matches('/').to_string();
        if value.is_empty() {
            return Err(TrackerError::MissingTrackerRepository);
        }
        for prefix in [
            "https://github.com/",
            "http://github.com/",
            "ssh://git@github.com/",
            "git@github.com:",
        ] {
            if let Some(stripped) = value.strip_prefix(prefix) {
                value = stripped.to_string();
                break;
            }
        }
        if let Some(stripped) = value.strip_suffix(".git") {
            value = stripped.to_string();
        }
        let parts: Vec<_> = value
            .split('/')
            .filter(|part| !part.trim().is_empty())
            .collect();
        if parts.len() != 2 {
            return Err(TrackerError::InvalidTrackerRepository(raw.to_string()));
        }
        Ok(Self {
            owner: parts[0].trim().to_string(),
            name: parts[1].trim().to_string(),
        })
    }

    fn name_with_owner(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    pub(crate) fn issue_identifier(&self, number: u64) -> String {
        format!(
            "{}-{}-{number}",
            identifier_part(&self.owner),
            identifier_part(&self.name)
        )
    }
}

#[derive(Debug, Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
    repository: GitHubRepository,
    active_states: Vec<String>,
    terminal_states: Vec<String>,
    filter: GitHubIssueFilterConfig,
    page_size: u64,
}

impl GitHubClient {
    pub fn new(config: GitHubClientConfig) -> Result<Self, TrackerError> {
        if config.api_key.trim().is_empty() {
            return Err(TrackerError::MissingTrackerApiKey);
        }
        let repository = GitHubRepository::parse(&config.repository)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(30_000))
            .build()
            .map_err(|err| TrackerError::GitHubApiRequest(err.to_string()))?;
        Ok(Self {
            http,
            endpoint: config.endpoint.trim_end_matches('/').to_string(),
            api_key: config.api_key,
            repository,
            active_states: config.active_states,
            terminal_states: config.terminal_states,
            filter: config.filter,
            page_size: config.page_size,
        })
    }

    async fn request_json(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value, TrackerError> {
        let url = append_query(format!("{}{}", self.endpoint, path), query);
        let mut request = self
            .http
            .request(method, &url)
            .bearer_auth(&self.api_key)
            .header("User-Agent", GITHUB_USER_AGENT)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|err| TrackerError::GitHubApiRequest(err.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| TrackerError::GitHubUnknownPayload(err.to_string()))?;
        let payload = if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text)
                .map_err(|err| TrackerError::GitHubUnknownPayload(err.to_string()))?
        };
        if !status.is_success() {
            return Err(TrackerError::GitHubApiStatus(status.as_u16()));
        }
        Ok(payload)
    }

    async fn search_issues(
        &self,
        state_names: &[String],
        apply_filter: bool,
    ) -> Result<Vec<Issue>, TrackerError> {
        let selectors = if apply_filter {
            state_selectors(state_names)
        } else {
            state_selectors_for_scan(state_names, true)
        };
        let mut issues = Vec::new();
        let mut seen = HashSet::new();
        for selector in selectors {
            let queries = if apply_filter {
                self.search_queries(&selector)
            } else {
                self.search_queries_for_selector(&selector, false)
            };
            for query_text in queries {
                let mut page = 1_u64;
                loop {
                    let payload = self
                        .request_json(
                            Method::GET,
                            SEARCH_ISSUES_PATH,
                            &[
                                ("q", query_text.clone()),
                                ("per_page", self.page_size.to_string()),
                                ("page", page.to_string()),
                            ],
                            None,
                        )
                        .await?;
                    let total_count = payload
                        .get("total_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        .min(1_000);
                    let items =
                        payload
                            .get("items")
                            .and_then(Value::as_array)
                            .ok_or_else(|| {
                                TrackerError::GitHubUnknownPayload(
                                    "missing search.items".to_string(),
                                )
                            })?;
                    for item in items {
                        let issue = self.normalize_issue(item, state_names)?;
                        if (!apply_filter || self.matches_filter(item, &issue))
                            && seen.insert(issue.id.clone())
                        {
                            issues.push(issue);
                        }
                    }
                    if items.len() < self.page_size as usize || page * self.page_size >= total_count
                    {
                        break;
                    }
                    page += 1;
                }
            }
        }
        Ok(issues)
    }

    pub(crate) fn search_queries(&self, selector: &StateSelector) -> Vec<String> {
        self.search_queries_for_selector(selector, true)
    }

    pub(crate) fn search_queries_for_selector(
        &self,
        selector: &StateSelector,
        apply_filter: bool,
    ) -> Vec<String> {
        let assignees: Vec<Option<&str>> = if !apply_filter || self.filter.assignees.is_empty() {
            vec![None]
        } else {
            self.filter
                .assignees
                .iter()
                .map(|assignee| Some(assignee.as_str()))
                .collect()
        };
        let tags: Vec<Option<&str>> = if !apply_filter || self.filter.tags.is_empty() {
            vec![None]
        } else {
            self.filter
                .tags
                .iter()
                .map(|tag| Some(tag.as_str()))
                .collect()
        };
        let mut queries = Vec::new();
        for assignee in &assignees {
            for tag in &tags {
                queries.push(self.search_query(selector, *assignee, *tag));
            }
        }
        queries
    }

    fn search_query(
        &self,
        selector: &StateSelector,
        assignee: Option<&str>,
        tag: Option<&str>,
    ) -> String {
        let mut parts = vec![
            format!("repo:{}", self.repository.name_with_owner()),
            "is:issue".to_string(),
            format!("state:{}", selector.github_state),
        ];
        if let Some(label) = &selector.label {
            push_label_qualifier(&mut parts, label);
        }
        if let Some(assignee) = assignee {
            parts.push(format!("assignee:{}", quote_search_value(assignee)));
        }
        if let Some(tag) = tag
            && selector.label.as_deref() != Some(tag)
        {
            push_label_qualifier(&mut parts, tag);
        }
        parts.join(" ")
    }

    async fn raw_issue(&self, issue_id: &str) -> Result<Value, TrackerError> {
        let number = parse_issue_number(issue_id)?;
        let path = issue_path(&self.repository.owner, &self.repository.name, number);
        let payload = self.request_json(Method::GET, &path, &[], None).await?;
        if payload.get("pull_request").is_some() {
            return Err(TrackerError::UnsupportedTrackerOperation(format!(
                "GitHub issue id {issue_id} is a pull request, not an issue"
            )));
        }
        Ok(payload)
    }

    pub(crate) fn normalize_issue(
        &self,
        node: &Value,
        preferred_states: &[String],
    ) -> Result<Issue, TrackerError> {
        if node.get("pull_request").is_some() {
            return Err(TrackerError::UnsupportedTrackerOperation(
                "GitHub pull requests are not tracker issues".to_string(),
            ));
        }
        let number = node.get("number").and_then(Value::as_u64).ok_or_else(|| {
            TrackerError::GitHubUnknownPayload("missing issue.number".to_string())
        })?;
        let title = string_field(node, "title").ok_or_else(|| {
            TrackerError::GitHubUnknownPayload(format!("missing title for issue {number}"))
        })?;
        let api_state = string_field(node, "state").ok_or_else(|| {
            TrackerError::GitHubUnknownPayload(format!("missing state for issue {number}"))
        })?;
        let labels = label_names(node);
        let state = self.display_state(&api_state, &labels, preferred_states);
        Ok(Issue {
            id: number.to_string(),
            identifier: self.repository.issue_identifier(number),
            title,
            description: string_field(node, "body"),
            priority: None,
            state,
            branch_name: None,
            url: string_field(node, "html_url"),
            labels: labels.iter().map(|label| normalize_state(label)).collect(),
            blocked_by: Vec::new(),
            created_at: datetime_field(node, "created_at"),
            updated_at: datetime_field(node, "updated_at"),
        })
    }

    fn display_state(
        &self,
        api_state: &str,
        labels: &[String],
        preferred_states: &[String],
    ) -> String {
        let states = if preferred_states.is_empty() {
            self.active_states
                .iter()
                .chain(self.terminal_states.iter())
                .cloned()
                .collect()
        } else {
            preferred_states.to_vec()
        };
        let normalized_api = normalize_state(api_state);
        if let Some(label_state) = matching_label_state(&self.terminal_states, labels) {
            return label_state;
        }
        if is_closed_state(&normalized_api) {
            return states
                .iter()
                .find(|state| is_closed_state(&normalize_state(state)))
                .cloned()
                .unwrap_or_else(|| "closed".to_string());
        }
        if let Some(label_state) = matching_label_state(&states, labels) {
            return label_state;
        }
        states
            .iter()
            .find(|state| normalize_state(state) == "open")
            .cloned()
            .unwrap_or_else(|| "open".to_string())
    }

    fn matches_filter(&self, node: &Value, issue: &Issue) -> bool {
        let assignee_match = self.filter.assignees.is_empty() || {
            let assignees = assignee_logins(node);
            self.filter.assignees.iter().any(|expected| {
                let expected = expected.to_ascii_lowercase();
                assignees
                    .iter()
                    .any(|login| login.eq_ignore_ascii_case(&expected))
            })
        };
        let tag_match = self.filter.tags.is_empty()
            || self.filter.tags.iter().any(|expected| {
                let expected = normalize_state(expected);
                issue.labels.iter().any(|label| label == &expected)
            });
        assignee_match && tag_match
    }

    fn configured_state_label_names(&self) -> HashSet<String> {
        self.active_states
            .iter()
            .chain(self.terminal_states.iter())
            .map(|state| normalize_state(state))
            .filter(|state| !is_github_api_state(state))
            .collect()
    }

    async fn comments_for_issue(&self, issue_id: &str) -> Result<Vec<Value>, TrackerError> {
        let number = parse_issue_number(issue_id)?;
        let path = issue_comments_path(&self.repository.owner, &self.repository.name, number);
        let mut comments = Vec::new();
        let mut page = 1_u64;
        loop {
            let payload = self
                .request_json(
                    Method::GET,
                    &path,
                    &[
                        ("per_page", self.page_size.to_string()),
                        ("page", page.to_string()),
                    ],
                    None,
                )
                .await?;
            let items = payload.as_array().cloned().ok_or_else(|| {
                TrackerError::GitHubUnknownPayload("comments was not an array".to_string())
            })?;
            let item_count = items.len();
            comments.extend(items);
            if item_count < self.page_size as usize {
                break;
            }
            page += 1;
        }
        Ok(comments)
    }
}

#[async_trait]
impl Tracker for GitHubClient {
    async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError> {
        self.search_issues(&self.active_states, true).await
    }

    async fn fetch_by_states(&self, state_names: &[String]) -> Result<Vec<Issue>, TrackerError> {
        if state_names.is_empty() {
            return Ok(Vec::new());
        }
        self.search_issues(state_names, false).await
    }

    async fn fetch_states_by_ids(&self, issue_ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        let mut issues = Vec::new();
        for chunk in issue_ids.chunks(STATE_REFRESH_CONCURRENCY) {
            let mut tasks = Vec::new();
            for issue_id in chunk {
                let client = self.clone();
                let issue_id = issue_id.clone();
                tasks.push(tokio::spawn(
                    async move { client.get_issue(&issue_id).await },
                ));
            }
            for task in tasks {
                let issue = task
                    .await
                    .map_err(|err| TrackerError::GitHubApiRequest(err.to_string()))??;
                issues.push(issue);
            }
        }
        Ok(issues)
    }

    async fn get_issue(&self, issue_id: &str) -> Result<Issue, TrackerError> {
        let payload = self.raw_issue(issue_id).await?;
        self.normalize_issue(&payload, &[])
    }

    async fn update_issue(
        &self,
        issue_id: &str,
        update: IssueUpdate,
    ) -> Result<Issue, TrackerError> {
        let current = self.raw_issue(issue_id).await?;
        let number = parse_issue_number(issue_id)?;
        let mut body = Map::new();
        let mut labels = label_names(&current);
        let mut labels_changed = false;
        if let Some(state) = update
            .state
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let normalized = normalize_state(state);
            if is_closed_state(&normalized) {
                body.insert("state".to_string(), json!("closed"));
                labels_changed |=
                    remove_state_labels(&mut labels, &self.configured_state_label_names());
            } else if normalized == "open" {
                body.insert("state".to_string(), json!("open"));
                labels_changed |=
                    remove_state_labels(&mut labels, &self.configured_state_label_names());
            } else {
                body.insert("state".to_string(), json!("open"));
                labels_changed |=
                    remove_state_labels(&mut labels, &self.configured_state_label_names());
                labels_changed |= add_label(&mut labels, state);
            }
        }
        for label in clean_values(update.labels) {
            labels_changed |= add_label(&mut labels, &label);
        }
        if labels_changed {
            body.insert("labels".to_string(), json!(labels));
        }
        if body.is_empty() {
            return self.normalize_issue(&current, &[]);
        }
        let path = issue_path(&self.repository.owner, &self.repository.name, number);
        let payload = self
            .request_json(Method::PATCH, &path, &[], Some(Value::Object(body)))
            .await?;
        self.normalize_issue(&payload, &[])
    }

    async fn create_comment(
        &self,
        issue_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        let number = parse_issue_number(issue_id)?;
        let path = issue_comments_path(&self.repository.owner, &self.repository.name, number);
        let payload = self
            .request_json(Method::POST, &path, &[], Some(json!({ "body": body })))
            .await?;
        normalize_comment(&payload)
    }

    async fn update_comment(
        &self,
        comment_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        let comment_id = comment_id.parse::<u64>().map_err(|_| {
            TrackerError::GitHubUnknownPayload(format!("invalid GitHub comment id: {comment_id}"))
        })?;
        let path = issue_comment_path(&self.repository.owner, &self.repository.name, comment_id);
        let payload = self
            .request_json(Method::PATCH, &path, &[], Some(json!({ "body": body })))
            .await?;
        normalize_comment(&payload)
    }

    async fn upload_attachment(
        &self,
        _issue_id: &str,
        _path: &Path,
        _content_type: &str,
    ) -> Result<IssueAttachment, TrackerError> {
        Err(TrackerError::UnsupportedTrackerOperation(
            "GitHub Issues API does not support attachment upload".to_string(),
        ))
    }

    async fn link_pr(&self, issue_id: &str, title: &str, url: &str) -> Result<(), TrackerError> {
        let comments = self.comments_for_issue(issue_id).await?;
        if comments.iter().any(|comment| {
            comment
                .get("body")
                .and_then(Value::as_str)
                .is_some_and(|body| body.contains(url))
        }) {
            return Ok(());
        }
        let body = if title.trim().is_empty() {
            format!("Linked pull request: {url}")
        } else {
            format!("Linked pull request: [{title}]({url})")
        };
        self.create_comment(issue_id, &body).await?;
        Ok(())
    }
}

#[async_trait]
impl IssueTracker for GitHubClient {
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct StateSelector {
    pub(crate) github_state: &'static str,
    pub(crate) label: Option<String>,
}

pub(crate) fn state_selectors(states: &[String]) -> Vec<StateSelector> {
    state_selectors_for_scan(states, false)
}

pub(crate) fn state_selectors_for_scan(
    states: &[String],
    include_closed_label_states: bool,
) -> Vec<StateSelector> {
    let mut selectors = Vec::new();
    let mut seen = HashSet::new();
    for state in states {
        let trimmed = state.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = normalize_state(trimmed);
        let selector = if is_closed_state(&normalized) {
            StateSelector {
                github_state: "closed",
                label: None,
            }
        } else if normalized == "open" {
            StateSelector {
                github_state: "open",
                label: None,
            }
        } else {
            StateSelector {
                github_state: "open",
                label: Some(trimmed.to_string()),
            }
        };
        if seen.insert(selector.clone()) {
            selectors.push(selector);
        }
        if include_closed_label_states && !is_github_api_state(&normalized) {
            let closed_label_selector = StateSelector {
                github_state: "closed",
                label: Some(trimmed.to_string()),
            };
            if seen.insert(closed_label_selector.clone()) {
                selectors.push(closed_label_selector);
            }
        }
    }
    selectors
}

fn matching_label_state(states: &[String], labels: &[String]) -> Option<String> {
    states
        .iter()
        .find(|state| {
            let normalized = normalize_state(state);
            !is_github_api_state(&normalized)
                && labels
                    .iter()
                    .any(|label| normalize_state(label) == normalized)
        })
        .cloned()
}

fn is_github_api_state(normalized: &str) -> bool {
    normalized == "open" || is_closed_state(normalized)
}

fn is_closed_state(normalized: &str) -> bool {
    matches!(normalized, "closed" | "close" | "done")
}

fn label_names(node: &Value) -> Vec<String> {
    node.get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| string_field(label, "name"))
                .collect()
        })
        .unwrap_or_default()
}

fn assignee_logins(node: &Value) -> Vec<String> {
    node.get("assignees")
        .and_then(Value::as_array)
        .map(|assignees| {
            assignees
                .iter()
                .filter_map(|assignee| string_field(assignee, "login"))
                .collect()
        })
        .unwrap_or_default()
}

fn string_field(node: &Value, key: &str) -> Option<String> {
    node.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn datetime_field(node: &Value, key: &str) -> Option<DateTime<Utc>> {
    node.get(key)
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn clean_values(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_issue_number(issue_id: &str) -> Result<u64, TrackerError> {
    issue_id.parse::<u64>().map_err(|_| {
        TrackerError::GitHubUnknownPayload(format!("invalid GitHub issue id: {issue_id}"))
    })
}

fn identifier_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn remove_state_labels(labels: &mut Vec<String>, state_labels: &HashSet<String>) -> bool {
    let original_len = labels.len();
    labels.retain(|label| !state_labels.contains(&normalize_state(label)));
    labels.len() != original_len
}

fn add_label(labels: &mut Vec<String>, label: &str) -> bool {
    let label = label.trim();
    if label.is_empty()
        || labels
            .iter()
            .any(|existing| normalize_state(existing) == normalize_state(label))
    {
        return false;
    }
    labels.push(label.to_string());
    true
}

fn normalize_comment(comment: &Value) -> Result<IssueComment, TrackerError> {
    let id = comment
        .get("id")
        .and_then(Value::as_u64)
        .map(|id| id.to_string())
        .or_else(|| string_field(comment, "id"))
        .ok_or_else(|| TrackerError::GitHubUnknownPayload("missing comment.id".to_string()))?;
    let body = string_field(comment, "body").unwrap_or_default();
    let url = string_field(comment, "html_url");
    Ok(IssueComment { id, body, url })
}

fn push_label_qualifier(parts: &mut Vec<String>, label: &str) {
    parts.push(format!("label:{}", quote_search_value(label)));
}

fn quote_search_value(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

fn append_query(mut url: String, query: &[(&str, String)]) -> String {
    if query.is_empty() {
        return url;
    }
    url.push('?');
    for (index, (key, value)) in query.iter().enumerate() {
        if index > 0 {
            url.push('&');
        }
        url.push_str(&percent_encode(key));
        url.push('=');
        url.push_str(&percent_encode(value));
    }
    url
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
