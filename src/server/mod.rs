//! Minimal HTTP server infrastructure.

mod routes;
mod services;

use std::future::Future;

use axum::ServiceExt;
use lxy::routing::RouterService;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::workflow::Workflow;

pub(crate) use services::ServerConfig;

struct Server {
  listener: std::net::TcpListener,
  routes: RouterService,
  address: ServerConfig,
  shutdown: CancellationToken,
}

impl Server {
  fn bind(config: ServerConfig, shutdown: CancellationToken) -> Result<Self, ServerError> {
    let listener = std::net::TcpListener::bind(config.bind_addr()).map_err(|source| ServerError::Bind {
      host: config.bind_address(),
      port: config.port(),
      source,
    })?;
    let bound_addr = listener.local_addr().map_err(ServerError::LocalAddr)?;
    listener.set_nonblocking(true).map_err(ServerError::SetNonblocking)?;
    let address = config.with_bound_addr(bound_addr);

    Ok(Self {
      listener,
      routes: routes::build(),
      address,
      shutdown,
    })
  }

  fn address(&self) -> &ServerConfig {
    &self.address
  }

  fn into_tokio_listener(
    self,
  ) -> Result<(tokio::net::TcpListener, RouterService, ServerConfig, CancellationToken), ServerError> {
    let listener = tokio::net::TcpListener::from_std(self.listener).map_err(ServerError::Serve)?;
    Ok((listener, self.routes, self.address, self.shutdown))
  }
}

pub(crate) fn run(
  workflow: &Workflow,
  shutdown: CancellationToken,
) -> Result<(ServerConfig, impl Future<Output = Result<(), ServerError>> + use<>), ServerError> {
  let schema = workflow.schema().server.clone().unwrap_or_default();
  let config = ServerConfig::try_from(schema)?;
  let server = Server::bind(config, shutdown)?;
  let address = server.address().clone();
  Ok((address, serve(server)))
}

async fn serve(server: Server) -> Result<(), ServerError> {
  let (listener, routes, address, shutdown) = server.into_tokio_listener()?;
  let addr = address.bound_addr();
  tracing::info_span!("server").in_scope(|| {
    tracing::info!(
      bind_address = %addr,
      base_url = %address.url().build("/"),
      "HTTP server listening",
    );
  });

  axum::serve(listener, routes.into_make_service())
    .with_graceful_shutdown(shutdown.cancelled_owned())
    .await
    .map_err(ServerError::Serve)
}

#[derive(Debug, Error)]
pub(crate) enum ServerError {
  #[error(transparent)]
  Config(#[from] services::ServerConfigError),

  #[error("failed to bind HTTP server to {host}:{port}: {source}")]
  Bind {
    host: String,
    port: u16,
    #[source]
    source: std::io::Error,
  },

  #[error("failed to read bound HTTP address: {0}")]
  LocalAddr(#[source] std::io::Error),

  #[error("failed to set HTTP listener nonblocking: {0}")]
  SetNonblocking(#[source] std::io::Error),

  #[error("HTTP server failed: {0}")]
  Serve(#[source] std::io::Error),
}

#[cfg(test)]
mod tests {
  use tokio::io::{AsyncReadExt, AsyncWriteExt};

  use super::*;
  use crate::config::ServerSchema;

  #[tokio::test]
  async fn bind_discovers_actual_port_for_random_port_config() {
    let config = ServerConfig::try_from(ServerSchema::default()).expect("server config");
    let server = Server::bind(config, CancellationToken::new()).expect("server binds");

    assert_eq!(server.address().bind_address(), "127.0.0.1");
    assert_ne!(server.address().port(), 0);
  }

  #[test]
  fn run_uses_default_config_when_server_is_missing() {
    let workflow = Workflow::builder().build();
    let shutdown = CancellationToken::new();

    let (address, _server) = run(&workflow, shutdown.clone()).expect("server enabled");

    assert_eq!(address.bind_address(), "127.0.0.1");
    assert_ne!(address.port(), 0);
    shutdown.cancel();
  }

  #[test]
  fn run_uses_workflow_server() {
    let mut config = ServerSchema::default();
    config.https = true;
    config.domain = Some("example.local".into());
    let workflow = Workflow::builder().server(config).build();
    let shutdown = CancellationToken::new();

    let (address, _server) = run(&workflow, shutdown.clone()).expect("server enabled");

    assert_eq!(address.url().build("/status"), "https://example.local/status");
    shutdown.cancel();
  }

  #[test]
  fn run_reports_bind_error_before_runtime_start() {
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("occupy port");
    let mut server = ServerSchema::default();
    server.port = occupied.local_addr().expect("occupied addr").port();
    let workflow = Workflow::builder().server(server).build();

    let err = match run(&workflow, CancellationToken::new()) {
      Ok(_) => panic!("bind must fail"),
      Err(err) => err,
    };

    assert!(format!("{err:#}").contains("failed to bind HTTP server"));
  }

  #[tokio::test]
  async fn server_runs_health_route_and_stops_on_shutdown() {
    let shutdown = CancellationToken::new();
    let config = ServerConfig::try_from(ServerSchema::default()).expect("server config");
    let server = Server::bind(config, shutdown.clone()).expect("server binds");
    let addr = server.address().bound_addr();
    let handle = tokio::spawn(serve(server));

    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    stream
      .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
      .await
      .expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).await.expect("read response");
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");

    shutdown.cancel();
    handle.await.expect("server task joins").expect("server stops");
  }

  #[tokio::test]
  async fn server_runs_status_route() {
    let shutdown = CancellationToken::new();
    let config = ServerConfig::try_from(ServerSchema::default()).expect("server config");
    let server = Server::bind(config, shutdown.clone()).expect("server binds");
    let addr = server.address().bound_addr();
    let handle = tokio::spawn(serve(server));

    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    stream
      .write_all(b"GET /status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
      .await
      .expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).await.expect("read response");

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains(r#""runtime":"vik""#), "{response}");
    assert!(
      response.contains(&format!(r#""version":"{}""#, env!("CARGO_PKG_VERSION"))),
      "{response}"
    );

    shutdown.cancel();
    handle.await.expect("server task joins").expect("server stops");
  }
}
