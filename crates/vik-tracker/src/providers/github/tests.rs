use std::path::Path;

use serde_json::json;
use vik_core::TrackerError;

use crate::providers::Tracker;

use super::{
    client::{
        DEFAULT_GITHUB_ENDPOINT, GitHubClient, GitHubClientConfig, GitHubIssueFilterConfig,
        GitHubPullRequest, GitHubRepository, append_closing_reference,
        body_contains_closing_reference, closing_reference, state_selectors,
        state_selectors_for_scan,
    },
    queries::{SEARCH_ISSUES_PATH, pull_path},
};

#[test]
fn repository_config_accepts_common_github_forms() {
    assert_eq!(
        GitHubRepository::parse(" yii-labs/vik ").unwrap(),
        GitHubRepository {
            owner: "yii-labs".to_string(),
            name: "vik".to_string(),
        }
    );
    assert_eq!(
        GitHubRepository::parse("https://github.com/yii-labs/vik.git").unwrap(),
        GitHubRepository {
            owner: "yii-labs".to_string(),
            name: "vik".to_string(),
        }
    );
    assert_eq!(
        GitHubRepository::parse("git@github.com:yii-labs/vik.git").unwrap(),
        GitHubRepository {
            owner: "yii-labs".to_string(),
            name: "vik".to_string(),
        }
    );
}

#[test]
fn repository_config_rejects_missing_repo() {
    assert!(matches!(
        GitHubRepository::parse("yii-labs"),
        Err(TrackerError::InvalidTrackerRepository(_))
    ));
}

#[test]
fn issue_identifier_is_url_safe() {
    let repo = GitHubRepository::parse("Yii-Labs/vik.service").unwrap();

    assert_eq!(repo.issue_identifier(42), "yii-labs-vik.service-42");
}

#[test]
fn pull_request_path_targets_pull_api() {
    assert_eq!(
        pull_path("yii-labs", "vik", 48),
        "/repos/yii-labs/vik/pulls/48"
    );
}

#[test]
fn pull_request_url_parses_owner_repo_and_number() {
    let pull_request =
        GitHubPullRequest::parse_url("https://github.com/yii-labs/vik/pull/48").unwrap();

    assert_eq!(
        pull_request.repository,
        GitHubRepository {
            owner: "yii-labs".to_string(),
            name: "vik".to_string(),
        }
    );
    assert_eq!(pull_request.number, 48);
}

#[test]
fn pull_request_url_rejects_non_pull_urls() {
    assert!(matches!(
        GitHubPullRequest::parse_url("https://github.com/yii-labs/vik/issues/48"),
        Err(TrackerError::UnsupportedTrackerOperation(_))
    ));
}

#[test]
fn closing_reference_uses_full_issue_repository_reference() {
    let repository = GitHubRepository::parse("yii-labs/vik").unwrap();

    assert_eq!(closing_reference(&repository, 42), "Closes yii-labs/vik#42");
}

#[test]
fn closing_reference_is_appended_to_pr_body() {
    assert_eq!(
        append_closing_reference("Existing body\n", "Closes yii-labs/vik#42"),
        "Existing body\n\nCloses yii-labs/vik#42"
    );
    assert_eq!(
        append_closing_reference("", "Closes yii-labs/vik#42"),
        "Closes yii-labs/vik#42"
    );
}

#[test]
fn closing_reference_detection_handles_full_and_same_repo_refs() {
    let issue_repository = GitHubRepository::parse("yii-labs/vik").unwrap();
    let same_pull_repository = GitHubRepository::parse("yii-labs/vik").unwrap();
    let fork_pull_repository = GitHubRepository::parse("forehalo/vik").unwrap();

    assert!(body_contains_closing_reference(
        "Fixes #42",
        &issue_repository,
        &same_pull_repository,
        42
    ));
    assert!(body_contains_closing_reference(
        "Resolves: yii-labs/vik#42",
        &issue_repository,
        &fork_pull_repository,
        42
    ));
    assert!(!body_contains_closing_reference(
        "Related #42",
        &issue_repository,
        &same_pull_repository,
        42
    ));
    assert!(!body_contains_closing_reference(
        "Fixes #420",
        &issue_repository,
        &same_pull_repository,
        42
    ));
    assert!(!body_contains_closing_reference(
        "discloses #42",
        &issue_repository,
        &same_pull_repository,
        42
    ));
    assert!(!body_contains_closing_reference(
        "Fixes #42",
        &issue_repository,
        &fork_pull_repository,
        42
    ));
}

#[test]
fn search_uses_issue_search_not_repository_issue_pagination() {
    assert_eq!(SEARCH_ISSUES_PATH, "/search/issues");
    let client = GitHubClient::new(
        GitHubClientConfig::new(
            DEFAULT_GITHUB_ENDPOINT,
            "gh_token",
            "yii-labs/vik",
            vec!["Todo".to_string()],
            vec!["Done".to_string()],
        )
        .with_filter(GitHubIssueFilterConfig::new(
            vec!["forehalo".to_string()],
            vec!["agent".to_string()],
        )),
    )
    .unwrap();
    let selector = state_selectors(&["In Progress".to_string()])
        .into_iter()
        .next()
        .unwrap();
    let query = client.search_queries(&selector).remove(0);

    assert!(query.contains("repo:yii-labs/vik"));
    assert!(query.contains("is:issue"));
    assert!(query.contains("state:open"));
    assert!(query.contains("label:\"In Progress\""));
    assert!(query.contains("assignee:\"forehalo\""));
    assert!(query.contains("label:\"agent\""));
}

#[test]
fn github_filter_or_semantics_use_separate_search_queries() {
    let client = GitHubClient::new(
        GitHubClientConfig::new(
            DEFAULT_GITHUB_ENDPOINT,
            "gh_token",
            "yii-labs/vik",
            vec!["Todo".to_string()],
            vec!["Done".to_string()],
        )
        .with_filter(GitHubIssueFilterConfig::new(
            vec!["one".to_string(), "two".to_string()],
            vec!["agent".to_string(), "codex".to_string()],
        )),
    )
    .unwrap();
    let selector = state_selectors(&["Todo".to_string()])
        .into_iter()
        .next()
        .unwrap();
    let queries = client.search_queries(&selector);

    assert_eq!(queries.len(), 4);
    assert!(
        queries
            .iter()
            .any(|query| query.contains("assignee:\"one\"") && query.contains("label:\"agent\""))
    );
    assert!(
        queries
            .iter()
            .any(|query| query.contains("assignee:\"two\"") && query.contains("label:\"codex\""))
    );
}

#[test]
fn state_scans_can_skip_github_candidate_filters() {
    let client = GitHubClient::new(
        GitHubClientConfig::new(
            DEFAULT_GITHUB_ENDPOINT,
            "gh_token",
            "yii-labs/vik",
            vec!["Todo".to_string()],
            vec!["Duplicate".to_string()],
        )
        .with_filter(GitHubIssueFilterConfig::new(
            vec!["forehalo".to_string()],
            vec!["agent".to_string()],
        )),
    )
    .unwrap();
    let selector = state_selectors(&["Duplicate".to_string()])
        .into_iter()
        .next()
        .unwrap();
    let queries = client.search_queries_for_selector(&selector, false);

    assert_eq!(queries.len(), 1);
    assert!(queries[0].contains("label:\"Duplicate\""));
    assert!(!queries[0].contains("assignee:"));
    assert!(!queries[0].contains("label:\"agent\""));
}

#[test]
fn terminal_label_state_scans_include_open_and_closed_issues() {
    let selectors = state_selectors_for_scan(&["Duplicate".to_string()], true);

    assert!(selectors.iter().any(|selector| {
        selector.github_state == "open" && selector.label.as_deref() == Some("Duplicate")
    }));
    assert!(selectors.iter().any(|selector| {
        selector.github_state == "closed" && selector.label.as_deref() == Some("Duplicate")
    }));
}

#[test]
fn state_selectors_map_github_state_aliases_and_label_states() {
    let selectors = state_selectors(&[
        "Todo".to_string(),
        "Done".to_string(),
        "Closed".to_string(),
        "open".to_string(),
    ]);

    assert!(selectors.iter().any(|selector| {
        selector.github_state == "open" && selector.label.as_deref() == Some("Todo")
    }));
    assert!(
        selectors
            .iter()
            .any(|selector| selector.github_state == "closed" && selector.label.is_none())
    );
    assert!(
        selectors
            .iter()
            .any(|selector| selector.github_state == "open" && selector.label.is_none())
    );
}

#[test]
fn github_filter_values_are_trimmed() {
    let filter = GitHubIssueFilterConfig::new(
        vec![" forehalo ".to_string(), "".to_string()],
        vec![" agent ".to_string()],
    );

    assert_eq!(filter.assignees, vec!["forehalo"]);
    assert_eq!(filter.tags, vec!["agent"]);
}

#[test]
fn github_issue_state_prefers_configured_label_state() {
    let client = GitHubClient::new(GitHubClientConfig::new(
        DEFAULT_GITHUB_ENDPOINT,
        "gh_token",
        "yii-labs/vik",
        vec!["Todo".to_string(), "In Progress".to_string()],
        vec!["Done".to_string()],
    ))
    .unwrap();
    let issue = client
        .normalize_issue(
            &json!({
                "number": 42,
                "title": "Work",
                "body": "Body",
                "state": "open",
                "html_url": "https://github.com/yii-labs/vik/issues/42",
                "labels": [{ "name": "In Progress" }],
                "assignees": [],
                "created_at": "2026-05-04T00:00:00Z",
                "updated_at": "2026-05-04T01:00:00Z"
            }),
            &["Todo".to_string(), "In Progress".to_string()],
        )
        .unwrap();

    assert_eq!(issue.id, "42");
    assert_eq!(issue.identifier, "yii-labs-vik-42");
    assert_eq!(issue.state, "In Progress");
    assert_eq!(issue.labels, vec!["in progress"]);
}

#[test]
fn github_issue_state_prefers_terminal_label_over_open_candidate_state() {
    let client = GitHubClient::new(GitHubClientConfig::new(
        DEFAULT_GITHUB_ENDPOINT,
        "gh_token",
        "yii-labs/vik",
        vec!["open".to_string()],
        vec!["Duplicate".to_string()],
    ))
    .unwrap();
    let issue = client
        .normalize_issue(
            &json!({
                "number": 42,
                "title": "Work",
                "body": "Body",
                "state": "open",
                "html_url": "https://github.com/yii-labs/vik/issues/42",
                "labels": [{ "name": "Duplicate" }],
                "assignees": [],
                "created_at": "2026-05-04T00:00:00Z",
                "updated_at": "2026-05-04T01:00:00Z"
            }),
            &["open".to_string()],
        )
        .unwrap();

    assert_eq!(issue.state, "Duplicate");
}

#[test]
fn github_issue_state_preserves_terminal_label_on_closed_issue() {
    let client = GitHubClient::new(GitHubClientConfig::new(
        DEFAULT_GITHUB_ENDPOINT,
        "gh_token",
        "yii-labs/vik",
        vec!["Todo".to_string()],
        vec!["Duplicate".to_string()],
    ))
    .unwrap();
    let issue = client
        .normalize_issue(
            &json!({
                "number": 42,
                "title": "Work",
                "body": "Body",
                "state": "closed",
                "html_url": "https://github.com/yii-labs/vik/issues/42",
                "labels": [{ "name": "Duplicate" }],
                "assignees": [],
                "created_at": "2026-05-04T00:00:00Z",
                "updated_at": "2026-05-04T01:00:00Z"
            }),
            &["Duplicate".to_string()],
        )
        .unwrap();

    assert_eq!(issue.state, "Duplicate");
}

#[tokio::test]
async fn github_upload_attachment_is_explicitly_unsupported() {
    let client = GitHubClient::new(GitHubClientConfig::new(
        DEFAULT_GITHUB_ENDPOINT,
        "gh_token",
        "yii-labs/vik",
        vec!["Todo".to_string()],
        vec!["Done".to_string()],
    ))
    .unwrap();
    let err = client
        .upload_attachment("1", Path::new("artifact.log"), "text/plain")
        .await
        .unwrap_err();

    assert!(matches!(
        err,
        TrackerError::UnsupportedTrackerOperation(message)
            if message.contains("does not support attachment upload")
    ));
}
