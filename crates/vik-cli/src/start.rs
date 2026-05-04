use std::error::Error;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use vik_agent::LocalAgentWorker;
use vik_http::{HttpState, serve};
use vik_orchestrator::Orchestrator;
use vik_tracker::{
    DEFAULT_GITHUB_ENDPOINT, DEFAULT_LINEAR_ENDPOINT, GitHubClient, GitHubClientConfig,
    GitHubIssueFilterConfig, LinearClient, LinearClientConfig, LinearIssueFilterConfig,
    TrackerClient,
};
use vik_workflow::{TrackerConfig, WorkflowReloader};

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
    let _log_guard = init_logging(&log_dir)?;
    tracing::info!(log_dir=%log_dir.display(), "logging outcome=started");

    let tracker = Arc::new(build_tracker(&loaded.config.tracker)?);
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

fn init_logging(log_dir: &Path) -> Result<WorkerGuard, Box<dyn Error>> {
    fs::create_dir_all(log_dir)?;
    let file_appender = tracing_appender::rolling::daily(log_dir, "vik.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stdout_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_writer)
        .json()
        .with_current_span(false)
        .with_span_list(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();
    Ok(guard)
}

fn build_tracker(config: &TrackerConfig) -> Result<TrackerClient, Box<dyn Error>> {
    match config.kind.as_str() {
        "linear" => {
            let tracker_config = LinearClientConfig::new(
                if config.endpoint.is_empty() {
                    DEFAULT_LINEAR_ENDPOINT
                } else {
                    &config.endpoint
                },
                &config.api_key,
                &config.project_slug,
                config.active_states.clone(),
            )
            .with_filter(LinearIssueFilterConfig::new(
                config.filter.assignees.clone(),
                config.filter.tags.clone(),
            ));
            Ok(TrackerClient::new(LinearClient::new(tracker_config)?))
        }
        "github" => {
            let tracker_config = GitHubClientConfig::new(
                if config.endpoint.is_empty() {
                    DEFAULT_GITHUB_ENDPOINT
                } else {
                    &config.endpoint
                },
                &config.api_key,
                &config.repository,
                config.active_states.clone(),
            )
            .with_filter(GitHubIssueFilterConfig::new(
                config.filter.assignees.clone(),
                config.filter.tags.clone(),
            ));
            Ok(TrackerClient::new(GitHubClient::new(tracker_config)?))
        }
        _ => Err(Box::new(vik_core::TrackerError::UnsupportedTrackerKind)),
    }
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
    fn build_tracker_selects_github_client() {
        let tracker = build_tracker(&TrackerConfig {
            kind: "github".to_string(),
            endpoint: "https://api.github.com".to_string(),
            api_key: "token".to_string(),
            project_slug: String::new(),
            repository: "yii-labs/vik".to_string(),
            active_states: vec!["open".to_string()],
            terminal_states: vec!["closed".to_string()],
            filter: Default::default(),
        })
        .unwrap();

        drop(tracker);
    }
}
