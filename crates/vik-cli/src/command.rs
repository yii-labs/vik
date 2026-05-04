use std::error::Error;

use clap::{Parser, Subcommand};

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
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Register a workflow with the local Vik service.
    Work(crate::work::WorkArgs),
    /// Validate workflow and exit.
    Check(crate::check::CheckArgs),
    /// Manage Vik as a detached local service.
    Service(crate::service::ServiceArgs),
    #[command(hide = true)]
    Daemon(crate::service::DaemonArgs),
}

pub(crate) async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    match args.command {
        Command::Work(args) => crate::work::run(args),
        Command::Check(args) => {
            crate::env::load_dotenv()?;
            crate::check::run(args.workflow)
        }
        Command::Service(args) => crate::service::run(args).await,
        Command::Daemon(args) => crate::service::run_daemon(args).await,
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
            Command::Check(check_args) => {
                assert_eq!(check_args.workflow, Some(PathBuf::from("./custom.md")));
            }
            other => panic!("expected check subcommand, got {other:?}"),
        }
    }

    #[test]
    fn check_subcommand_allows_default_workflow_path() {
        let args = Args::try_parse_from(["vik", "check"]).unwrap();

        match args.command {
            Command::Check(check_args) => assert_eq!(check_args.workflow, None),
            other => panic!("expected check subcommand, got {other:?}"),
        }
    }

    #[test]
    fn legacy_check_flag_is_rejected() {
        let err = Args::try_parse_from(["vik", "./WORKFLOW.md", "--check"]).unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn root_help_shows_check_command_not_legacy_flag() {
        let help = Args::command().render_help().to_string();

        assert!(help.contains("work"));
        assert!(help.contains("check"));
        assert!(help.contains("Register a workflow with the local Vik service"));
        assert!(help.contains("Validate workflow and exit"));
        assert!(!help.contains("start"));
        assert!(!help.contains("daemon"));
        assert!(!help.contains("--check"));
    }

    #[test]
    fn work_command_accepts_workflow_option() {
        let args = Args::try_parse_from(["vik", "work", "--workflow", "WORKFLOW.md"]).unwrap();

        match args.command {
            Command::Work(args) => {
                assert_eq!(args.workflow, Some(PathBuf::from("WORKFLOW.md")));
            }
            other => panic!("expected work command, got {other:?}"),
        }
    }

    #[test]
    fn daemon_command_accepts_workflow_and_status_flags() {
        let args = Args::try_parse_from([
            "vik",
            "daemon",
            "--workflow",
            "WORKFLOW.md",
            "--port",
            "3000",
            "--bind-address",
            "0.0.0.0",
        ])
        .unwrap();

        match args.command {
            Command::Daemon(args) => {
                assert_eq!(args.workflows, vec![PathBuf::from("WORKFLOW.md")]);
                assert_eq!(args.port, Some(3000));
                assert_eq!(args.bind_address, Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
            }
            other => panic!("expected daemon command, got {other:?}"),
        }
    }

    #[test]
    fn implicit_workflow_arg_is_rejected() {
        let err = Args::try_parse_from(["vik", "WORKFLOW.md"]).unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn daemon_flags_are_scoped_to_daemon_command() {
        let err = Args::try_parse_from(["vik", "--port", "3000", "service", "status"])
            .unwrap_err()
            .kind();

        assert_eq!(err, clap::error::ErrorKind::UnknownArgument);
    }
}
