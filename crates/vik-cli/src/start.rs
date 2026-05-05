use std::error::Error;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};
use vik_agent::LocalAgentWorker;
use vik_http::{HttpState, serve};
use vik_orchestrator::Orchestrator;
use vik_tracker::{
    DEFAULT_LINEAR_ENDPOINT, LinearClient, LinearClientConfig, LinearIssueFilterConfig,
};
use vik_workflow::WorkflowReloader;

#[derive(Debug, Args)]
pub(crate) struct StartArgs {
    /// Path to WORKFLOW.md. Defaults to ./WORKFLOW.md.
    pub(crate) workflow: Option<PathBuf>,

    /// Enable HTTP status server. Overrides server.port from WORKFLOW.md.
    #[arg(long)]
    pub(crate) port: Option<u16>,

    /// HTTP status server bind address. Defaults to 127.0.0.1.
    #[arg(long, alias = "host", value_name = "ADDR")]
    pub(crate) bind_address: Option<IpAddr>,
}

pub(crate) async fn run(args: StartArgs) -> Result<(), Box<dyn Error>> {
    let reloader = WorkflowReloader::start(args.workflow)?;
    let loaded = reloader.current().clone();
    loaded.config.validate_for_dispatch()?;

    let log_dir = loaded.config.logging.dir.clone();
    let _log_guards = init_logging(&log_dir)?;
    tracing::info!(log_dir=%log_dir.display(), "logging outcome=started");

    let tracker_config = LinearClientConfig::new(
        if loaded.config.tracker.endpoint.is_empty() {
            DEFAULT_LINEAR_ENDPOINT
        } else {
            &loaded.config.tracker.endpoint
        },
        &loaded.config.tracker.api_key,
        &loaded.config.tracker.project_slug,
        loaded.config.tracker.active_states.clone(),
    )
    .with_filter(LinearIssueFilterConfig::new(
        loaded.config.tracker.filter.assignees.clone(),
        loaded.config.tracker.filter.tags.clone(),
    ));
    let tracker = Arc::new(LinearClient::new(tracker_config)?);
    let worker = Arc::new(LocalAgentWorker::new(Arc::clone(&tracker)));
    let orchestrator = Arc::new(Orchestrator::new(Arc::clone(&tracker), worker, reloader));

    let port = args
        .port
        .or(loaded.config.server.as_ref().map(|server| server.port));
    if let Some(port) = port {
        let orch_for_state = Arc::clone(&orchestrator);
        let orch_for_issue = Arc::clone(&orchestrator);
        let addr = http_addr(args.bind_address, port);
        let bound = serve(
            addr,
            HttpState {
                snapshot: Arc::new(move || {
                    let orch = Arc::clone(&orch_for_state);
                    Box::pin(async move { orch.snapshot().await })
                }),
                issue: Arc::new(move |identifier| {
                    let orch = Arc::clone(&orch_for_issue);
                    Box::pin(async move { orch.issue_debug(&identifier).await })
                }),
                refresh_tx: orchestrator.refresh_sender(),
            },
        )
        .await?;
        tracing::info!(addr=%bound, "http_server outcome=started");
    }

    orchestrator.run_forever().await?;
    Ok(())
}

fn init_logging(log_dir: &Path) -> Result<Vec<WorkerGuard>, Box<dyn Error>> {
    fs::create_dir_all(log_dir)?;
    let service_appender = tracing_appender::rolling::daily(log_dir, "service.log");
    let session_appender = tracing_appender::rolling::daily(log_dir, "session.log");
    let (service_writer, service_guard) = tracing_appender::non_blocking(service_appender);
    let (session_writer, session_guard) = tracing_appender::non_blocking(session_appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stdout_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_filter(filter_fn(is_service_log));
    let service_layer = tracing_subscriber::fmt::layer()
        .with_writer(service_writer)
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_filter(filter_fn(is_service_log));
    let session_layer = tracing_subscriber::fmt::layer()
        .with_writer(session_writer)
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_filter(filter_fn(is_session_log));

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(service_layer)
        .with(session_layer)
        .init();
    Ok(vec![service_guard, session_guard])
}

fn is_service_log(metadata: &tracing::Metadata<'_>) -> bool {
    !is_session_target(metadata.target())
}

fn is_session_log(metadata: &tracing::Metadata<'_>) -> bool {
    is_session_target(metadata.target())
}

fn is_session_target(target: &str) -> bool {
    target == vik_agent::SESSION_LOG_TARGET
}

fn http_addr(host: Option<IpAddr>, port: u16) -> SocketAddr {
    SocketAddr::new(host.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)), port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_addr_defaults_to_localhost() {
        assert_eq!(
            http_addr(None, 3000),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3000)
        );
    }

    #[test]
    fn http_addr_uses_explicit_host() {
        assert_eq!(
            http_addr(Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)), 3000),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 3000)
        );
    }

    #[test]
    fn log_filters_split_service_and_session_targets() {
        assert!(is_session_target(vik_agent::SESSION_LOG_TARGET));
        assert!(!is_session_target("vik_orchestrator::engine"));
    }
}
