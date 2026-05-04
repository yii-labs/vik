use serde_json::json;

use super::{
    client::{LinearIssueFilterConfig, issue_has_attachment_url},
    normalize::normalize_issue,
    queries::{
        ATTACHMENT_CREATE_MUTATION, CANDIDATE_QUERY, ISSUE_BY_ID_QUERY, ISSUE_STATES_BY_IDS_QUERY,
    },
};

#[test]
fn candidate_query_uses_project_slug_filter() {
    assert!(CANDIDATE_QUERY.contains("project: { slugId: { eq: $projectSlug } }"));
}

#[test]
fn candidate_query_uses_delegable_filter_variables() {
    assert!(CANDIDATE_QUERY.contains("$assigneeFilter: NullableUserFilter!"));
    assert!(CANDIDATE_QUERY.contains("$labelFilter: IssueLabelCollectionFilter!"));
    assert!(CANDIDATE_QUERY.contains("assignee: $assigneeFilter"));
    assert!(CANDIDATE_QUERY.contains("labels: $labelFilter"));
}

#[test]
fn empty_delegable_filters_are_noop_objects() {
    let filter = LinearIssueFilterConfig::new(vec![], vec![]);

    assert_eq!(filter.assignee_filter_value(), json!({}));
    assert_eq!(filter.label_filter_value(), json!({}));
}

#[test]
fn delegable_filters_match_assignees_and_tags_case_insensitively() {
    let filter = LinearIssueFilterConfig::new(
        vec![" user-a ".to_string()],
        vec!["agent".to_string(), "codex".to_string()],
    );

    assert_eq!(
        filter.assignee_filter_value(),
        json!({
            "or": [
                { "id": { "eq": "user-a" } },
                { "name": { "eqIgnoreCase": "user-a" } },
                { "displayName": { "eqIgnoreCase": "user-a" } },
                { "email": { "eqIgnoreCase": "user-a" } }
            ]
        })
    );
    assert_eq!(
        filter.label_filter_value(),
        json!({
            "some": {
                "or": [
                    { "name": { "eqIgnoreCase": "agent" } },
                    { "name": { "eqIgnoreCase": "codex" } }
                ]
            }
        })
    );
}

#[test]
fn state_refresh_uses_graphql_id_list_type() {
    assert!(ISSUE_STATES_BY_IDS_QUERY.contains("$ids: [ID!]!"));
}

#[test]
fn issue_update_metadata_query_includes_state_and_label_ids() {
    assert!(super::queries::ISSUE_STATES_FOR_ISSUE_QUERY.contains("states"));
    assert!(super::queries::ISSUE_STATES_FOR_ISSUE_QUERY.contains("labels"));
    assert!(super::queries::ISSUE_UPDATE_MUTATION.contains("IssueUpdateInput"));
}

#[test]
fn attachment_queries_support_pr_link_sync() {
    assert!(ISSUE_BY_ID_QUERY.contains("issue(id: $id)"));
    assert!(ISSUE_BY_ID_QUERY.contains("attachments(first: 50)"));
    assert!(ISSUE_BY_ID_QUERY.contains("url"));
    assert!(ATTACHMENT_CREATE_MUTATION.contains("attachmentCreate(input: $input)"));
}

#[test]
fn detects_existing_pr_attachment_url() {
    let issue = json!({
        "attachments": {
            "nodes": [
                { "url": "https://example.com/other" },
                { "url": "https://github.com/forehalo/repo/pull/1" }
            ]
        }
    });

    assert!(issue_has_attachment_url(
        &issue,
        "https://github.com/forehalo/repo/pull/1"
    ));
    assert!(!issue_has_attachment_url(
        &issue,
        "https://github.com/forehalo/repo/pull/2"
    ));
}

#[test]
fn normalizes_labels_and_blockers() {
    let issue = normalize_issue(&json!({
        "id": "id1",
        "identifier": "ABC-1",
        "title": "Title",
        "description": "Desc",
        "priority": 2,
        "branchName": "abc-1",
        "url": "https://linear.app/x",
        "createdAt": "2026-02-24T20:15:30Z",
        "updatedAt": "2026-02-25T20:15:30Z",
        "state": { "name": "Todo" },
        "labels": { "nodes": [{ "name": "Bug" }, { "name": "Backend" }] },
        "inverseRelations": {
            "nodes": [
                {
                    "type": "blocks",
                    "issue": { "id": "dep", "identifier": "ABC-0", "state": { "name": "Done" } }
                },
                {
                    "type": "related",
                    "issue": { "id": "other", "identifier": "ABC-2", "state": { "name": "Todo" } }
                }
            ]
        }
    }))
    .unwrap();
    assert_eq!(issue.labels, vec!["bug", "backend"]);
    assert_eq!(issue.blocked_by.len(), 1);
    assert_eq!(issue.blocked_by[0].identifier.as_deref(), Some("ABC-0"));
}
