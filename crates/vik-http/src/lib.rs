use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use axum::routing::{get, post};
use serde_json::json;
use thiserror::Error;
use tokio::sync::mpsc;
use vik_core::{IssueDebugSnapshot, RuntimeSnapshot};

type SnapshotFuture = Pin<Box<dyn Future<Output = RuntimeSnapshot> + Send>>;
type IssueFuture = Pin<Box<dyn Future<Output = Option<IssueDebugSnapshot>> + Send>>;

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("bind_failed: {0}")]
    Bind(String),
    #[error("serve_failed: {0}")]
    Serve(String),
}

#[derive(Clone)]
pub struct HttpState {
    pub snapshot: Arc<dyn Fn() -> SnapshotFuture + Send + Sync>,
    pub issue: Arc<dyn Fn(String) -> IssueFuture + Send + Sync>,
    pub refresh_tx: mpsc::UnboundedSender<()>,
}

pub async fn serve(addr: SocketAddr, state: HttpState) -> Result<SocketAddr, HttpError> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|err| HttpError::Bind(err.to_string()))?;
    let bound_addr = listener
        .local_addr()
        .map_err(|err| HttpError::Bind(err.to_string()))?;
    let app = router(state);
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            tracing::error!(error=%err, "http_server outcome=failed");
        }
    });
    Ok(bound_addr)
}

pub fn router(state: HttpState) -> Router {
    Router::new()
        .route("/", get(dashboard))
        .route("/api/v1/state", get(state_json))
        .route("/api/v1/refresh", post(refresh))
        .route("/api/v1/{issue_identifier}", get(issue_json))
        .with_state(state)
}

async fn dashboard(State(state): State<HttpState>) -> Html<String> {
    let snapshot = (state.snapshot)().await;
    let rows = snapshot
        .running
        .iter()
        .map(|row| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                html_escape(&row.issue_identifier),
                html_escape(&row.state),
                row.turn_count,
                html_escape(row.last_event.as_deref().unwrap_or(""))
            )
        })
        .collect::<Vec<_>>()
        .join("");
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Vik</title></head>
<body>
<h1>Vik</h1>
<p>running={} retrying={} input_tokens={} output_tokens={} total_tokens={}</p>
<table><thead><tr><th>Issue</th><th>State</th><th>Turns</th><th>Last event</th></tr></thead><tbody>{}</tbody></table>
</body>
</html>"#,
        snapshot.counts.get("running").copied().unwrap_or(0),
        snapshot.counts.get("retrying").copied().unwrap_or(0),
        snapshot.token_totals.input_tokens,
        snapshot.token_totals.output_tokens,
        snapshot.token_totals.total_tokens,
        rows
    ))
}

async fn state_json(State(state): State<HttpState>) -> Json<RuntimeSnapshot> {
    Json((state.snapshot)().await)
}

async fn issue_json(
    State(state): State<HttpState>,
    Path(issue_identifier): Path<String>,
) -> impl IntoResponse {
    match (state.issue)(issue_identifier.clone()).await {
        Some(snapshot) => (StatusCode::OK, Json(json!(snapshot))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "code": "issue_not_found",
                    "message": format!("issue not found: {issue_identifier}")
                }
            })),
        )
            .into_response(),
    }
}

async fn refresh(State(state): State<HttpState>) -> impl IntoResponse {
    let coalesced = state.refresh_tx.send(()).is_err();
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "queued": !coalesced,
            "coalesced": coalesced,
            "requested_at": chrono::Utc::now(),
            "operations": ["poll", "reconcile"]
        })),
    )
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
