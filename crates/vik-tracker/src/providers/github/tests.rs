use std::path::Path;

use serde_json::json;
use vik_core::TrackerError;

use crate::providers::Tracker;

use super::{
    client::{
        DEFAULT_GITHUB_ENDPOINT, GitHubClient, GitHubClientConfig, GitHubIssueFilterConfig,
        GitHubRepository, state_selectors,
    },
    queries::SEARCH_ISSUES_PATH,
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
fn search_uses_issue_search_not_repository_issue_pagination() {
    assert_eq!(SEARCH_ISSUES_PATH, "/search/issues");
    let client = GitHubClient::new(GitHubClientConfig::new(
        DEFAULT_GITHUB_ENDPOINT,
        "gh_token",
        "yii-labs/vik",
        vec!["Todo".to_string()],
        vec!["Done".to_string()],
    ))
    .unwrap();
    let selector = state_selectors(&["In Progress".to_string()])
        .into_iter()
        .next()
        .unwrap();
    let query = client.search_query(&selector);

    assert!(query.contains("repo:yii-labs/vik"));
    assert!(query.contains("is:issue"));
    assert!(query.contains("state:open"));
    assert!(query.contains("label:\"In Progress\""));
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
