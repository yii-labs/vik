//! Minimal HTTP server infrastructure.

mod routes;
mod services;

use axum::ServiceExt;
use lxy::routing::RouterService;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::config::ServerSchema;

pub(crate) use services::ServerAddress;

pub(crate) struct PreparedServer {
  listener: std::net::TcpListener,
  routes: RouterService,
  address: ServerAddress,
  shutdown: CancellationToken,
}

impl PreparedServer {
  pub(crate) fn bind(config: &ServerSchema, shutdown: CancellationToken) -> Result<Self, ServerError> {
    let listener =
      std::net::TcpListener::bind((config.host.as_str(), config.port)).map_err(|source| ServerError::Bind {
        host: config.host.clone(),
        port: config.port,
        source,
      })?;
    let bound_addr = listener.local_addr().map_err(ServerError::LocalAddr)?;
    listener.set_nonblocking(true).map_err(ServerError::SetNonblocking)?;
    let address = ServerAddress::new(config.https, config.domain.clone(), bound_addr);

    Ok(Self {
      listener,
      routes: routes::build(),
      address,
      shutdown,
    })
  }

  pub(crate) fn address(&self) -> &ServerAddress {
    &self.address
  }

  fn into_tokio_listener(
    self,
  ) -> Result<(tokio::net::TcpListener, RouterService, ServerAddress, CancellationToken), ServerError> {
    let listener = tokio::net::TcpListener::from_std(self.listener).map_err(ServerError::Serve)?;
    Ok((listener, self.routes, self.address, self.shutdown))
  }
}

pub(crate) async fn run(server: PreparedServer) -> Result<(), ServerError> {
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

  #[tokio::test]
  async fn bind_discovers_actual_port_for_random_port_config() {
    let config = ServerSchema::default();
    let server = PreparedServer::bind(&config, CancellationToken::new()).expect("server binds");

    assert_eq!(server.address().bind_address(), "127.0.0.1");
    assert_ne!(server.address().port(), 0);
  }

  #[tokio::test]
  async fn server_runs_health_route_and_stops_on_shutdown() {
    let shutdown = CancellationToken::new();
    let config = ServerSchema::default();
    let server = PreparedServer::bind(&config, shutdown.clone()).expect("server binds");
    let addr = server.address().bound_addr();
    let handle = tokio::spawn(run(server));

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
    let config = ServerSchema::default();
    let server = PreparedServer::bind(&config, shutdown.clone()).expect("server binds");
    let addr = server.address().bound_addr();
    let handle = tokio::spawn(run(server));

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
