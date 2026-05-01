use serde_json::json;

use crate::{
    ATTACHMENT_CREATE_MUTATION, CANDIDATE_QUERY, ISSUE_BY_IDENTIFIER_QUERY,
    ISSUE_STATES_BY_IDS_QUERY, client::issue_has_attachment_url, normalize_issue,
};

#[test]
fn candidate_query_uses_project_slug_filter() {
    assert!(CANDIDATE_QUERY.contains("project: { slugId: { eq: $projectSlug } }"));
}

#[test]
fn state_refresh_uses_graphql_id_list_type() {
    assert!(ISSUE_STATES_BY_IDS_QUERY.contains("$ids: [ID!]!"));
}

#[test]
fn attachment_queries_support_pr_link_sync() {
    assert!(ISSUE_BY_IDENTIFIER_QUERY.contains("issue(id: $id)"));
    assert!(ISSUE_BY_IDENTIFIER_QUERY.contains("attachments(first: 50)"));
    assert!(ISSUE_BY_IDENTIFIER_QUERY.contains("url"));
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
