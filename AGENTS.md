# Agent Instructions

All committed text must be English.

## Development Docs

Read the relevant docs before changing this repo:

- [Development index](docs/development/index.md)
- [Setup](docs/development/setup.md)
- [Checks](docs/development/checks.md)
- [Pull requests](docs/development/pull-requests.md)
- [Code conventions](docs/development/code-conventions.md)

Usage and operator docs:

- [Get Started](docs/get-started.md)
- [Docker](docs/docker.md)
- [Service Daemon](docs/service-daemon.md)
- [Configuration](docs/configuration.md)
- [Observation](docs/observation.md)
- [Specification Conformance](docs/spec-conformance.md)

## Agent Workflow

1. Start from ticket scope. Do not expand scope without a separate issue.
2. Reproduce the current signal before edits.
3. Keep the worktree clean between milestones.
4. Sync with `origin/main` before implementation and before handoff.
5. Prefer narrow, reviewable changes.
6. Run the checks listed in [Checks](docs/development/checks.md).
7. Keep PR title, body, labels, and Linear links current.

## Repo Rules

- Use `rg` for search.
- Use `cargo fmt`, `cargo clippy`, and `cargo test` before handoff.
- Keep docs concrete enough for another agent to execute.
- Never commit secrets, local logs, target artifacts, or generated temp files.
- Do not hide failing validation. Record the exact command and result.
