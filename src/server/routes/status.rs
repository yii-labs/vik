use axum::Json;
use serde::Serialize;

pub(super) async fn get() -> Json<StatusResponse> {
  Json(StatusResponse {
    runtime: "vik",
    version: env!("CARGO_PKG_VERSION"),
  })
}

#[derive(Debug, Serialize)]
pub(super) struct StatusResponse {
  runtime: &'static str,
  version: &'static str,
}
