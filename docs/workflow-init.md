# Workflow Initialization

Use this guide to prepare a local Vik workspace for unattended Linear issue
orchestration.

## Prerequisites

- A local clone of this repository.
- Rust and Cargo.
- GitHub CLI (`gh`).
- Codex CLI with `app-server` support.
- Linear access to the target workspace and project.

## GitHub Authentication

Vik workflow hooks use GitHub for repository clone, branch, push, and PR operations.
Authenticate GitHub CLI before starting the daemon.

Official docs:

- GitHub CLI auth login: <https://cli.github.com/manual/gh_auth_login>
- GitHub CLI auth status: <https://cli.github.com/manual/gh_auth_status>
- GitHub CLI git credential setup: <https://cli.github.com/manual/gh_auth_setup-git>

Check current auth:

```sh
gh auth status --hostname github.com
```

Interactive browser flow:

```sh
gh auth login --hostname github.com --git-protocol ssh --web
gh auth setup-git --hostname github.com
gh auth status --hostname github.com
```

Headless automation can use `GH_TOKEN` or `GITHUB_TOKEN` instead. The token must have
repository access for clone, push, PR creation, labels, comments, and review reads.

## Linear API Key

Vik uses Linear's GraphQL API. For local automation, use a Linear personal API key.

Official docs:

- Linear GraphQL API getting started: <https://linear.app/developers/graphql>
- Linear API keys overview: <https://linear.app/docs/api-and-webhooks>

Create a key:

1. Open Linear.
2. Go to Settings, Account, Security & Access.
3. Create a personal API key.
4. Grant the smallest permissions that still allow Vik to read issues and update issue
   metadata for the target project.
5. Copy the key once and store it in `.env` or a secret manager.

Configure the local environment:

```sh
cp .env.example .env
printf 'LINEAR_API_KEY=lin_api_xxx\n' > .env
```

Do not commit `.env`. The workflow also accepts an already exported `LINEAR_API_KEY`.

## Workflow Configuration

Review the front matter in `WORKFLOW.md`:

- `tracker.project_slug` must match the Linear project slug.
- `tracker.active_states` controls which issue states Vik can claim.
- `tracker.terminal_states` controls cleanup and stop behavior.
- `workspace.root` must be a directory where Vik may create per-issue workspaces.
- `hooks.after_create` must clone the repository into the empty issue workspace.
- `codex.command` must launch `codex app-server`.

Validate the workflow before dispatch:

```sh
cargo run -p vik-cli -- ./WORKFLOW.md --check
```

Start the daemon:

```sh
cargo run -p vik-cli -- ./WORKFLOW.md
```

Enable the local HTTP status server when needed:

```sh
cargo run -p vik-cli -- ./WORKFLOW.md --port 3000
```

## Agent Bootstrap Instructions

The following prompt can be given to an AI agent that has shell access and, when
available, a browser tool:

```text
Initialize this Vik workspace for unattended issue orchestration.

Rules:
- Work only inside the current repository.
- Never print or commit secrets.
- Use browser automation only for login pages or settings pages when the browser tool is available.
- If required GitHub or Linear credentials are missing and no in-session auth path exists, record a blocker and stop.

Steps:
1. Confirm the current directory is the Vik repository.
2. Run `gh auth status --hostname github.com`.
3. If GitHub CLI is not authenticated and a browser tool is available, run `gh auth login --hostname github.com --git-protocol ssh --web`, complete the browser flow, then run `gh auth setup-git --hostname github.com`.
4. If GitHub CLI is not authenticated and no browser tool or token is available, stop with a GitHub auth blocker.
5. Ensure a Linear API key is available from `LINEAR_API_KEY` or `.env`.
6. If no key is available and a browser tool is available, open Linear settings, create a personal API key under Account > Security & Access, and store it as `LINEAR_API_KEY` in `.env`.
7. If no key is available and no browser tool can create one, stop with a Linear key blocker.
8. Review `WORKFLOW.md`; update `tracker.project_slug`, `workspace.root`, hooks, and `codex.command` only when they do not match the target environment.
9. Run `cargo run -p vik-cli -- ./WORKFLOW.md --check`.
10. Report the validation result and any blockers. Do not include secret values.
```
