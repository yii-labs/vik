//! Minimal HTTP server infrastructure.
//!
//! Vik owns binding, shutdown, and URL construction. The route table is
//! built with the published `lxy` router so later endpoint work can grow
//! from the same seam.

use std::net::{IpAddr, SocketAddr};

use axum::ServiceExt;
use lxy::Router;
use lxy::routing::RouterService;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::ServerSchema;

pub struct PreparedServer {
  listener: TcpListener,
  routes: RouterService,
  address: ServerAddress,
  shutdown: CancellationToken,
}

impl PreparedServer {
  pub async fn bind(config: &ServerSchema, shutdown: CancellationToken) -> Result<Self, ServerError> {
    let listener =
      TcpListener::bind((config.host.as_str(), config.port))
        .await
        .map_err(|source| ServerError::Bind {
          host: config.host.clone(),
          port: config.port,
          source,
        })?;
    let bound_addr = listener.local_addr().map_err(ServerError::LocalAddr)?;
    let address = ServerAddress::new(config.https, config.domain.clone(), bound_addr);

    Ok(Self {
      listener,
      routes: build_routes(),
      address,
      shutdown,
    })
  }

  pub fn address(&self) -> &ServerAddress {
    &self.address
  }

  pub async fn run(self) -> Result<(), ServerError> {
    let addr = self.address.bound_addr();
    tracing::info_span!("server").in_scope(|| {
      tracing::info!(
        bind_address = %addr,
        base_url = %self.address.url().build("/"),
        "HTTP server listening",
      );
    });

    axum::serve(self.listener, self.routes.into_make_service())
      .with_graceful_shutdown(self.shutdown.cancelled_owned())
      .await
      .map_err(ServerError::Serve)
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddress {
  https: bool,
  domain: Option<String>,
  bound_addr: SocketAddr,
}

impl ServerAddress {
  pub fn new(https: bool, domain: Option<String>, bound_addr: SocketAddr) -> Self {
    Self {
      https,
      domain,
      bound_addr,
    }
  }

  pub fn bound_addr(&self) -> SocketAddr {
    self.bound_addr
  }

  pub fn bind_address(&self) -> String {
    self.bound_addr.ip().to_string()
  }

  pub fn port(&self) -> u16 {
    self.bound_addr.port()
  }

  pub fn url(&self) -> UrlService {
    UrlService::new(self.https, self.domain.clone(), self.bound_addr)
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlService {
  https: bool,
  domain: Option<String>,
  bound_addr: SocketAddr,
}

impl UrlService {
  pub fn new(https: bool, domain: Option<String>, bound_addr: SocketAddr) -> Self {
    Self {
      https,
      domain,
      bound_addr,
    }
  }

  pub fn build(&self, path: impl AsRef<str>) -> String {
    let scheme = if self.https { "https" } else { "http" };
    let authority = self.authority();
    let path = normalize_path(path.as_ref());

    format!("{scheme}://{authority}{path}")
  }

  fn authority(&self) -> String {
    if let Some(domain) = self.domain.as_deref() {
      return match domain.parse::<IpAddr>() {
        Ok(ip) => SocketAddr::new(ip, self.bound_addr.port()).to_string(),
        Err(_) => domain.to_string(),
      };
    }

    self.bound_addr.to_string()
  }
}

#[derive(Debug, Error)]
pub enum ServerError {
  #[error("failed to bind HTTP server to {host}:{port}: {source}")]
  Bind {
    host: String,
    port: u16,
    #[source]
    source: std::io::Error,
  },

  #[error("failed to read bound HTTP address: {0}")]
  LocalAddr(#[source] std::io::Error),

  #[error("HTTP server failed: {0}")]
  Serve(#[source] std::io::Error),
}

fn build_routes() -> RouterService {
  let mut router = Router::new();
  router.get("/health", || async { "ok" });
  router.build()
}

fn normalize_path(path: &str) -> String {
  let trimmed = path.trim_start_matches('/');
  if trimmed.is_empty() {
    "/".to_string()
  } else {
    format!("/{trimmed}")
  }
}

#[cfg(test)]
mod tests {
  use tokio::io::{AsyncReadExt, AsyncWriteExt};

  use super::*;

  #[test]
  fn url_uses_bound_ip_and_port_without_domain() {
    let url = UrlService::new(false, None, "127.0.0.1:4321".parse().expect("socket"));

    assert_eq!(url.build("api/v1/state"), "http://127.0.0.1:4321/api/v1/state");
  }

  #[test]
  fn url_uses_domain_without_port_for_named_domain() {
    let url = UrlService::new(
      true,
      Some("example.local".into()),
      "127.0.0.1:4321".parse().expect("socket"),
    );

    assert_eq!(url.build("/api/v1/state"), "https://example.local/api/v1/state");
  }

  #[test]
  fn url_includes_port_for_ip_domain() {
    let url = UrlService::new(
      false,
      Some("127.0.0.1".into()),
      "127.0.0.1:4321".parse().expect("socket"),
    );

    assert_eq!(url.build("/api/v1/state"), "http://127.0.0.1:4321/api/v1/state");
  }

  #[test]
  fn url_brackets_ipv6_authority() {
    let url = UrlService::new(false, None, "[::1]:4321".parse().expect("socket"));

    assert_eq!(url.build("/health"), "http://[::1]:4321/health");
  }

  #[test]
  fn url_normalizes_empty_path_to_root() {
    let url = UrlService::new(false, None, "127.0.0.1:4321".parse().expect("socket"));

    assert_eq!(url.build(""), "http://127.0.0.1:4321/");
  }

  #[tokio::test]
  async fn bind_discovers_actual_port_for_random_port_config() {
    let config = ServerSchema::default();
    let server = PreparedServer::bind(&config, CancellationToken::new())
      .await
      .expect("server binds");

    assert_eq!(server.address().bind_address(), "127.0.0.1");
    assert_ne!(server.address().port(), 0);
  }

  #[tokio::test]
  async fn server_runs_health_route_and_stops_on_shutdown() {
    let shutdown = CancellationToken::new();
    let config = ServerSchema::default();
    let server = PreparedServer::bind(&config, shutdown.clone()).await.expect("server binds");
    let addr = server.address().bound_addr();
    let handle = tokio::spawn(server.run());

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
}
