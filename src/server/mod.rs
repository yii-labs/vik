//! HTTP intake endpoints.
//!
//! This server only accepts generic issue intake webhooks today. It
//! parses normalized issue JSON and hands it to the orchestrator ingress;
//! stage matching still belongs to the orchestrator.

use std::net::SocketAddr;

use anyhow::Context;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::de::DeserializeOwned;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;

use crate::config::IssueWebhookSchema;
use crate::context::{Issue, Issues};
use crate::orchestrator::IssueIngress;

const SIGNATURE_HEADER: &str = "x-event-signature";

#[derive(Clone)]
struct IntakeState {
  webhook: Option<IssueWebhookSchema>,
  ingress: Option<IssueIngress>,
}

pub async fn serve(
  addr: SocketAddr,
  webhook: Option<IssueWebhookSchema>,
  ingress: Option<IssueIngress>,
  shutdown: CancellationToken,
) -> anyhow::Result<()> {
  let listener = TcpListener::bind(addr)
    .await
    .with_context(|| format!("bind HTTP API to {addr}"))?;
  let local_addr = listener.local_addr().context("read HTTP API listener address")?;
  tracing::info_span!("server").in_scope(|| {
    tracing::info!(
      bind_address = %local_addr,
      "HTTP API listening",
    );
  });

  axum::serve(listener, router(webhook, ingress))
    .with_graceful_shutdown(async move {
      shutdown.cancelled().await;
    })
    .await
    .context("serve HTTP API")?;

  Ok(())
}

pub fn router(webhook: Option<IssueWebhookSchema>, ingress: Option<IssueIngress>) -> Router {
  let has_webhook = webhook.is_some() && ingress.is_some();
  let state = IntakeState { webhook, ingress };
  let router = Router::new();
  let router = if has_webhook {
    router
      .route("/intake/issue", post(post_issue))
      .route("/intake/issues", post(post_issues))
  } else {
    router
  };

  router.with_state(state).layer(TraceLayer::new_for_http())
}

async fn post_issue(State(state): State<IntakeState>, headers: HeaderMap, body: Bytes) -> Response {
  let issue = match parse_signed_json::<Issue>(&state, &headers, &body) {
    Ok(issue) => issue,
    Err(status) => return status.into_response(),
  };
  if let Err(status) = validate_issue_id(&issue) {
    return status.into_response();
  }
  let Some(ingress) = &state.ingress else {
    return StatusCode::NOT_FOUND.into_response();
  };

  match ingress.enqueue_issue(issue).await {
    Ok(()) => StatusCode::ACCEPTED.into_response(),
    Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
  }
}

async fn post_issues(State(state): State<IntakeState>, headers: HeaderMap, body: Bytes) -> Response {
  let issues = match parse_signed_json::<Issues>(&state, &headers, &body) {
    Ok(issues) => issues,
    Err(status) => return status.into_response(),
  };
  if let Err(status) = validate_issue_ids(&issues) {
    return status.into_response();
  }
  let Some(ingress) = &state.ingress else {
    return StatusCode::NOT_FOUND.into_response();
  };

  match ingress.enqueue_issues(issues).await {
    Ok(()) => StatusCode::ACCEPTED.into_response(),
    Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
  }
}

fn parse_signed_json<T>(state: &IntakeState, headers: &HeaderMap, body: &[u8]) -> Result<T, StatusCode>
where
  T: DeserializeOwned,
{
  validate_signature(state, headers)?;
  serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)
}

fn validate_signature(state: &IntakeState, headers: &HeaderMap) -> Result<(), StatusCode> {
  let Some(webhook) = &state.webhook else {
    return Err(StatusCode::NOT_FOUND);
  };
  let Some(expected) = &webhook.x_event_signature else {
    return Ok(());
  };

  let Some(actual) = headers.get(SIGNATURE_HEADER).and_then(|value| value.to_str().ok()) else {
    return Err(StatusCode::UNAUTHORIZED);
  };

  if signature_matches(actual, expected) {
    Ok(())
  } else {
    Err(StatusCode::UNAUTHORIZED)
  }
}

fn signature_matches(actual: &str, expected: &str) -> bool {
  let actual = actual.as_bytes();
  let expected = expected.as_bytes();
  let mut diff = actual.len() ^ expected.len();

  for (idx, expected_byte) in expected.iter().enumerate() {
    diff |= actual.get(idx).copied().unwrap_or_default() as usize ^ *expected_byte as usize;
  }

  diff == 0
}

fn validate_issue_ids(issues: &Issues) -> Result<(), StatusCode> {
  for issue in issues.iter() {
    validate_issue_id(issue)?;
  }
  Ok(())
}

fn validate_issue_id(issue: &Issue) -> Result<(), StatusCode> {
  if issue.id.is_empty() || issue.id.starts_with('.') || issue.id.contains('/') || issue.id.contains('\\') {
    return Err(StatusCode::BAD_REQUEST);
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use axum::body::Body;
  use axum::http::{Request, header};
  use tower::ServiceExt;

  use super::*;
  use crate::orchestrator::Orchestrator;
  use crate::workflow::Workflow;

  #[tokio::test]
  async fn webhook_intake_issue_route_enqueues_one_issue() {
    let mut orchestrator = Orchestrator::new(Workflow::builder().workspace_root("workspace").build());
    let app = router(Some(IssueWebhookSchema::default()), Some(orchestrator.issue_ingress()));

    let response = app
      .oneshot(json_request(
        "/intake/issue",
        r#"{"id":"WEB-1","title":"Webhook issue","state":"todo"}"#,
      ))
      .await
      .expect("request succeeds");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let issue = orchestrator.recv_issue_for_test().await.expect("issue event");
    assert_eq!(issue.id, "WEB-1");
    assert_eq!(issue.title, "Webhook issue");
    assert_eq!(issue.state, "todo");
  }

  #[tokio::test]
  async fn webhook_intake_issues_route_enqueues_batch_in_order() {
    let mut orchestrator = Orchestrator::new(Workflow::builder().workspace_root("workspace").build());
    let app = router(Some(IssueWebhookSchema::default()), Some(orchestrator.issue_ingress()));

    let response = app
      .oneshot(json_request(
        "/intake/issues",
        r#"[{"id":"WEB-1","title":"First","state":"todo"},{"identifier":"WEB-2","title":"Second","status":"work"}]"#,
      ))
      .await
      .expect("request succeeds");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let first = orchestrator.recv_issue_for_test().await.expect("first issue event");
    let second = orchestrator.recv_issue_for_test().await.expect("second issue event");
    assert_eq!(first.id, "WEB-1");
    assert_eq!(second.id, "WEB-2");
    assert_eq!(second.state, "work");
  }

  #[tokio::test]
  async fn webhook_intake_routes_are_absent_when_webhook_is_not_configured() {
    let app = router(None, None);

    let response = app
      .oneshot(json_request(
        "/intake/issue",
        r#"{"id":"WEB-1","title":"Webhook issue","state":"todo"}"#,
      ))
      .await
      .expect("request succeeds");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
  }

  #[tokio::test]
  async fn webhook_configured_signature_must_match_before_parsing_body() {
    let mut webhook = IssueWebhookSchema::default();
    webhook.x_event_signature = Some("shared-secret".to_string());
    let mut orchestrator = Orchestrator::new(Workflow::builder().workspace_root("workspace").build());
    let app = router(Some(webhook), Some(orchestrator.issue_ingress()));

    let response = app
      .oneshot(json_request("/intake/issue", "not json"))
      .await
      .expect("request succeeds");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(orchestrator.recv_issue_now_for_test().await.is_none());
  }

  #[tokio::test]
  async fn webhook_matching_signature_allows_json_parse_and_enqueue() {
    let mut webhook = IssueWebhookSchema::default();
    webhook.x_event_signature = Some("shared-secret".to_string());
    let mut orchestrator = Orchestrator::new(Workflow::builder().workspace_root("workspace").build());
    let app = router(Some(webhook), Some(orchestrator.issue_ingress()));

    let response = app
      .oneshot(signed_json_request(
        "/intake/issue",
        r#"{"id":"WEB-1","title":"Webhook issue","state":"todo"}"#,
        "shared-secret",
      ))
      .await
      .expect("request succeeds");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let issue = orchestrator.recv_issue_for_test().await.expect("issue event");
    assert_eq!(issue.id, "WEB-1");
  }

  #[tokio::test]
  async fn webhook_mismatched_signature_does_not_enqueue() {
    let mut webhook = IssueWebhookSchema::default();
    webhook.x_event_signature = Some("shared-secret".to_string());
    let mut orchestrator = Orchestrator::new(Workflow::builder().workspace_root("workspace").build());
    let app = router(Some(webhook), Some(orchestrator.issue_ingress()));

    let response = app
      .oneshot(signed_json_request(
        "/intake/issue",
        r#"{"id":"WEB-1","title":"Webhook issue","state":"todo"}"#,
        "wrong-secret",
      ))
      .await
      .expect("request succeeds");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(orchestrator.recv_issue_now_for_test().await.is_none());
  }

  #[test]
  fn webhook_signature_match_checks_full_value_and_length() {
    assert!(signature_matches("shared-secret", "shared-secret"));
    assert!(!signature_matches("shared-secret", "shared-secreu"));
    assert!(!signature_matches("shared-secret", "shared-secret-longer"));
    assert!(!signature_matches("shared-secret-longer", "shared-secret"));
  }

  #[tokio::test]
  async fn webhook_rejects_unsafe_issue_id_without_enqueueing() {
    let mut orchestrator = Orchestrator::new(Workflow::builder().workspace_root("workspace").build());
    let app = router(Some(IssueWebhookSchema::default()), Some(orchestrator.issue_ingress()));

    let response = app
      .oneshot(json_request(
        "/intake/issue",
        r#"{"id":"../../outside","title":"Unsafe","state":"todo"}"#,
      ))
      .await
      .expect("request succeeds");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(orchestrator.recv_issue_now_for_test().await.is_none());
  }

  #[tokio::test]
  async fn webhook_rejects_unsafe_issue_batch_without_partial_enqueueing() {
    let mut orchestrator = Orchestrator::new(Workflow::builder().workspace_root("workspace").build());
    let app = router(Some(IssueWebhookSchema::default()), Some(orchestrator.issue_ingress()));

    let response = app
      .oneshot(json_request(
        "/intake/issues",
        r#"[{"id":"WEB-1","title":"First","state":"todo"},{"id":".hidden","title":"Unsafe","state":"todo"}]"#,
      ))
      .await
      .expect("request succeeds");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(orchestrator.recv_issue_now_for_test().await.is_none());
  }

  fn json_request(path: &str, body: &'static str) -> Request<Body> {
    Request::builder()
      .method("POST")
      .uri(path)
      .header(header::CONTENT_TYPE, "application/json")
      .body(Body::from(body))
      .expect("request builds")
  }

  fn signed_json_request(path: &str, body: &'static str, signature: &str) -> Request<Body> {
    Request::builder()
      .method("POST")
      .uri(path)
      .header(header::CONTENT_TYPE, "application/json")
      .header(SIGNATURE_HEADER, signature)
      .body(Body::from(body))
      .expect("request builds")
  }
}
