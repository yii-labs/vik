use std::error::Error;

use clap::{Parser, Subcommand};

use crate::check;
use crate::service;

#[derive(Debug, Parser)]
#[command(
    name = "vik",
    version,
    about = "Run Vik coding-agent orchestrator",
    subcommand_required = true,
    arg_required_else_help = true
)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Command,

    /// Path to WORKFLOW.md. Defaults to ./WORKFLOW.md.
    #[arg(global = true)]
    workflow: Option<std::path::PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate workflow and exit.
    Check,
    /// Start Vik coding-agent orchestration.
    Start(service::StartArgs),
    /// Manage Vik as a detached local service.
    Service(service::ServiceArgs),
}

pub(crate) async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    match args.command {
        Command::Check => check::run(args.workflow),
        Command::Start(start_args) => service::start(args.workflow, start_args).await,
        Command::Service(service_args) => service::run(args.workflow, service_args).await,
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;

    use clap::CommandFactory;

    use super::*;

    #[test]
    fn check_subcommand_accepts_workflow_path() {
        let args = Args::try_parse_from(["vik", "check", "./custom.md"]).unwrap();

        match args.command {
            Command::Check => {
                assert_eq!(args.workflow, Some(PathBuf::from("./custom.md")));
            }
            other => panic!("expected check subcommand, got {other:?}"),
        }
    }

    #[test]
    fn check_subcommand_allows_default_workflow_path() {
        let args = Args::try_parse_from(["vik", "check"]).unwrap();

        match args.command {
            Command::Check => assert_eq!(args.workflow, None),
            other => panic!("expected check subcommand, got {other:?}"),
        }
    }

    #[test]
    fn legacy_check_flag_is_rejected() {
        let err = Args::try_parse_from(["vik", "./WORKFLOW.md", "--check"]).unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn root_help_shows_check_command_not_legacy_flag() {
        let help = Args::command().render_help().to_string();

        assert!(help.contains("start"));
        assert!(help.contains("check"));
        assert!(help.contains("Start Vik coding-agent orchestration"));
        assert!(help.contains("Validate workflow and exit"));
        assert!(!help.contains("--check"));
    }

    #[test]
    fn start_command_accepts_workflow_and_daemon_flags() {
        let args = Args::try_parse_from([
            "vik",
            "start",
            "WORKFLOW.md",
            "--port",
            "3000",
            "--bind-address",
            "0.0.0.0",
        ])
        .unwrap();

        match args.command {
            Command::Start(args) => {
                assert_eq!(args.run_args.port, Some(3000));
                assert_eq!(args.run_args.host, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
            }
            other => panic!("expected start command, got {other:?}"),
        }
    }

    #[test]
    fn start_command_keeps_port_optional() {
        let args = Args::try_parse_from(["vik", "start", "WORKFLOW.md"]).unwrap();

        match args.command {
            Command::Start(args) => assert_eq!(args.run_args.port, None),
            other => panic!("expected start command, got {other:?}"),
        }
    }

    #[test]
    fn implicit_workflow_arg_is_rejected() {
        let err = Args::try_parse_from(["vik", "WORKFLOW.md"]).unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingSubcommand);
    }

    #[test]
    fn daemon_flags_are_scoped_to_start_command() {
        let err = Args::try_parse_from(["vik", "--port", "3000", "service", "status"])
            .unwrap_err()
            .kind();

        assert_eq!(err, clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn service_install_command_is_rejected() {
        let err = Args::try_parse_from(["vik", "service", "install"]).unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingSubcommand);
    }

    #[test]
    fn service_start_accepts_workflow_path() {
        let args =
            Args::try_parse_from(["vik", "service", "start", "WORKFLOW.md", "--port", "3000"])
                .unwrap();

        assert_eq!(args.workflow, Some(PathBuf::from("WORKFLOW.md")));
    }

    #[test]
    fn service_status_accepts_workflow_path() {
        let args = Args::try_parse_from(["vik", "service", "status", "WORKFLOW.md"]).unwrap();

        assert_eq!(args.workflow, Some(PathBuf::from("WORKFLOW.md")));
    }
}
