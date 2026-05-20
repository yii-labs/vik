use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerAddress {
  config: ServerConfig,
}

impl ServerAddress {
  pub(crate) fn new(https: bool, domain: Option<String>, bound_addr: SocketAddr) -> Self {
    Self {
      config: ServerConfig::new(https, domain, bound_addr),
    }
  }

  pub(crate) fn bound_addr(&self) -> SocketAddr {
    self.config.bound_addr
  }

  pub(crate) fn bind_address(&self) -> String {
    self.config.bound_addr.ip().to_string()
  }

  pub(crate) fn port(&self) -> u16 {
    self.config.bound_addr.port()
  }

  pub(crate) fn url(&self) -> UrlService {
    UrlService::new(self.config.clone())
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerConfig {
  https: bool,
  domain: Option<String>,
  bound_addr: SocketAddr,
}

impl ServerConfig {
  pub(crate) fn new(https: bool, domain: Option<String>, bound_addr: SocketAddr) -> Self {
    Self {
      https,
      domain,
      bound_addr,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UrlService {
  config: ServerConfig,
}

impl UrlService {
  pub(crate) fn new(config: ServerConfig) -> Self {
    Self { config }
  }

  pub(crate) fn build(&self, path: impl AsRef<str>) -> String {
    let scheme = if self.config.https { "https" } else { "http" };
    let authority = self.authority();
    let path = normalize_path(path.as_ref());

    format!("{scheme}://{authority}{path}")
  }

  fn authority(&self) -> String {
    if let Some(domain) = self.config.domain.as_deref() {
      return match domain.parse::<IpAddr>() {
        Ok(ip) => SocketAddr::new(ip, self.config.bound_addr.port()).to_string(),
        Err(_) => domain.to_string(),
      };
    }

    client_addr(self.config.bound_addr).to_string()
  }
}

fn client_addr(addr: SocketAddr) -> SocketAddr {
  match addr.ip() {
    IpAddr::V4(ip) if ip == Ipv4Addr::UNSPECIFIED => SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port()),
    IpAddr::V6(ip) if ip == Ipv6Addr::UNSPECIFIED => SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), addr.port()),
    _ => addr,
  }
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
  use super::*;

  fn url(https: bool, domain: Option<&str>, bound_addr: &str) -> UrlService {
    UrlService::new(ServerConfig::new(
      https,
      domain.map(str::to_string),
      bound_addr.parse().expect("socket"),
    ))
  }

  #[test]
  fn url_uses_bound_ip_and_port_without_domain() {
    let url = url(false, None, "127.0.0.1:4321");

    assert_eq!(url.build("api/v1/state"), "http://127.0.0.1:4321/api/v1/state");
  }

  #[test]
  fn url_uses_loopback_for_unspecified_ipv4_bind() {
    let url = url(false, None, "0.0.0.0:4321");

    assert_eq!(url.build("/status"), "http://127.0.0.1:4321/status");
  }

  #[test]
  fn url_uses_loopback_for_unspecified_ipv6_bind() {
    let url = url(false, None, "[::]:4321");

    assert_eq!(url.build("/status"), "http://[::1]:4321/status");
  }

  #[test]
  fn url_uses_domain_without_port_for_named_domain() {
    let url = url(true, Some("example.local"), "127.0.0.1:4321");

    assert_eq!(url.build("/api/v1/state"), "https://example.local/api/v1/state");
  }

  #[test]
  fn url_includes_port_for_ip_domain() {
    let url = url(false, Some("127.0.0.1"), "127.0.0.1:4321");

    assert_eq!(url.build("/api/v1/state"), "http://127.0.0.1:4321/api/v1/state");
  }

  #[test]
  fn url_brackets_ipv6_authority() {
    let url = url(false, None, "[::1]:4321");

    assert_eq!(url.build("/health"), "http://[::1]:4321/health");
  }

  #[test]
  fn url_normalizes_empty_path_to_root() {
    let url = url(false, None, "127.0.0.1:4321");

    assert_eq!(url.build(""), "http://127.0.0.1:4321/");
  }
}
