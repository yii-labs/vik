//! `server:` section of the Workflow Definition.

use serde::{Deserialize, Serialize};

use super::WorkflowSchema;
use super::diagnose::*;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ServerSchema {
  #[serde(default = "default_host")]
  pub host: String,
  #[serde(default)]
  pub port: u16,
  #[serde(default)]
  pub https: bool,
  #[serde(default)]
  pub domain: Option<String>,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

fn default_host() -> String {
  "127.0.0.1".into()
}

impl Default for ServerSchema {
  fn default() -> Self {
    Self {
      host: default_host(),
      port: 0,
      https: false,
      domain: None,
      unknown_fields: serde_yaml::Mapping::new(),
    }
  }
}

impl Diagnose for ServerSchema {
  fn diagnose(&self, _: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnostics.error_if_empty_str("host", &self.host);
    if let Some(domain) = &self.domain {
      diagnostics.error_if_empty_str("domain", domain);
    }
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

#[cfg(test)]
mod tests {
  use crate::config::diagnose::DiagnosticCode;

  use super::*;

  const MINIMAL_WORKFLOW: &str = r#"
agents:
  codex:
    runtime: codex
    model: gpt-5.5
issues:
  pull:
    command: ./scripts/issues-json
issue:
  stages:
    plan:
      when:
        state: todo
      agent: codex
      prompt_file: ./prompts/plan.md
"#;

  #[test]
  fn missing_server_keeps_http_disabled() {
    let schema = parse_workflow(MINIMAL_WORKFLOW);

    assert!(schema.server.is_none());
  }

  #[test]
  fn empty_server_uses_documented_defaults() {
    let schema = parse_workflow(&format!("{MINIMAL_WORKFLOW}\nserver: {{}}\n"));
    let server = schema.server.expect("server config");

    assert_eq!(server.host, "127.0.0.1");
    assert_eq!(server.port, 0);
    assert!(!server.https);
    assert_eq!(server.domain, None);
  }

  #[test]
  fn explicit_server_fields_parse() {
    let schema = parse_workflow(&format!(
      "{MINIMAL_WORKFLOW}\nserver:\n  host: 0.0.0.0\n  port: 8080\n  https: true\n  domain: example.local\n"
    ));
    let server = schema.server.expect("server config");

    assert_eq!(server.host, "0.0.0.0");
    assert_eq!(server.port, 8080);
    assert!(server.https);
    assert_eq!(server.domain.as_deref(), Some("example.local"));
  }

  #[test]
  fn nullable_domain_parses_as_absent_domain() {
    let schema = parse_workflow(&format!("{MINIMAL_WORKFLOW}\nserver:\n  domain: null\n"));
    let server = schema.server.expect("server config");

    assert_eq!(server.domain, None);
  }

  #[test]
  fn unknown_server_fields_surface_as_warnings() {
    let schema = parse_workflow(&format!("{MINIMAL_WORKFLOW}\nserver:\n  typo: true\n"));

    let diagnostics = schema.diagnose();

    assert!(!diagnostics.has_errors());
    assert!(
      diagnostics
        .warnings
        .iter()
        .any(|diag| diag.pointer == "server.typo" && matches!(diag.code, DiagnosticCode::UnknownField))
    );
  }

  #[test]
  fn server_diagnoses_empty_host_and_domain() {
    let schema = parse_workflow(&format!("{MINIMAL_WORKFLOW}\nserver:\n  host: ''\n  domain: ''\n"));

    let diagnostics = schema.diagnose();

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| diag.pointer == "server.host" && matches!(diag.code, DiagnosticCode::EmptyStr))
    );
    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| diag.pointer == "server.domain" && matches!(diag.code, DiagnosticCode::EmptyStr))
    );
  }

  fn parse_workflow(contents: &str) -> WorkflowSchema {
    serde_yaml::from_str(contents).expect("workflow schema parses")
  }
}
