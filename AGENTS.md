# Agent Instructions

## Development Docs

Read the relevant docs before changing this repo:

- [Development index](docs/development/index.md)
- [Checks](docs/development/checks.md)
- [Pull requests](docs/development/pull-requests.md)
- [Code conventions](docs/development/code-conventions.md)

## Agent Workflow

1. Sync with `origin/main` before implementation and before handoff.
2. Prefer narrow, reviewable changes.
3. Run the checks listed in [Checks](docs/development/checks.md).
4. Keep PR title, body, labels current.

## Repo Rules

- Use `rg` for search.
- Use `cargo fmt`, `cargo clippy`, and `cargo test` before handoff.
- Keep docs concrete enough for another agent to execute.
- Never commit secrets, local logs, target artifacts, or generated temp files.
- Do not hide failing validation. Record the exact command and result.
