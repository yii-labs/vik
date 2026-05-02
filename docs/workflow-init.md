# Workflow Initialization

Use this file as an executable checklist for a human operator or AI agent with
shell access. Follow it from top to bottom. Use browser automation only for login
or settings pages when a browser tool is available.

## Execution Rules

- Work only inside the current Vik repository.
- Never print, log, or commit secret values.
- Stop and record a blocker if required GitHub or Linear credentials are missing
  and no token or browser-based setup path is available.
- Do not start the daemon until workflow validation passes.

## Prerequisites

- A local clone of this repository.
- Rust and Cargo.
- GitHub CLI (`gh`).
- Codex CLI with `app-server` support.
- Linear access to the target workspace and project.

## 1. Confirm Repository

Run these checks from the repository root:

```sh
test -f WORKFLOW.md
test -f Cargo.toml
git rev-parse --show-toplevel
```

Continue only when the reported git root is the Vik repository.

## 2. Authenticate GitHub

Vik workflow hooks use GitHub for repository clone, branch, push, and PR
operations.

Official docs:

- GitHub CLI auth login: <https://cli.github.com/manual/gh_auth_login>
- GitHub CLI auth status: <https://cli.github.com/manual/gh_auth_status>
- GitHub CLI git credential setup: <https://cli.github.com/manual/gh_auth_setup-git>

Check current auth:

```sh
gh auth status --hostname github.com
```

If auth succeeds, continue.

If auth fails and `GH_TOKEN` or `GITHUB_TOKEN` is already exported, keep that
token in the environment for headless `gh` operations. The token must have
repository access for clone, push, PR creation, labels, comments, and review
reads.

If auth fails and a browser tool is available, run the browser login flow:

```sh
gh auth login --hostname github.com --git-protocol ssh --web
gh auth setup-git --hostname github.com
gh auth status --hostname github.com
```

If auth still fails and no usable token exists, stop with a GitHub auth blocker.

## 3. Configure Linear API Key

Vik uses Linear's GraphQL API. For local automation, use a Linear personal API
key.

Official docs:

- Linear GraphQL API getting started: <https://linear.app/developers/graphql>
- Linear API keys overview: <https://linear.app/docs/api-and-webhooks>

Check for an existing key:

```sh
test -n "${LINEAR_API_KEY:-}" || { test -f .env && grep -q '^LINEAR_API_KEY=' .env; }
```

If a key is already available, continue. Do not print the key.

If no key exists and a browser tool is available:

1. Open Linear.
2. Go to Settings, Account, Security & Access.
3. Create a personal API key.
4. Grant the smallest permissions that still allow Vik to read issues and update
   issue metadata for the target project.
5. Copy the key once.

Store the key in `.env`, replacing `lin_api_xxx` with the real key:

```sh
test -f .env || cp .env.example .env
if grep -q '^LINEAR_API_KEY=' .env; then
  sed -i.bak 's/^LINEAR_API_KEY=.*/LINEAR_API_KEY=lin_api_xxx/' .env
else
  printf '\nLINEAR_API_KEY=lin_api_xxx\n' >> .env
fi
rm -f .env.bak
```

Do not commit `.env`. If no key can be created or supplied, stop with a Linear
key blocker.

## 4. Review Workflow Configuration

Review the front matter in `WORKFLOW.md`:

- `tracker.project_slug` must match the Linear project slug.
- `tracker.active_states` controls which issue states Vik can claim.
- `tracker.terminal_states` controls cleanup and stop behavior.
- `workspace.root` must be a directory where Vik may create per-issue
  workspaces.
- `hooks.after_create` must clone the repository into the empty issue workspace.
- `codex.command` must launch `codex app-server`.

Update only values that do not match the target environment.

## 5. Validate Workflow

Run validation before dispatch:

```sh
cargo run --locked -p vik-cli -- ./WORKFLOW.md --check
```

Continue only when validation reports the workflow is valid.

## 6. Start Workflow

Start the daemon:

```sh
cargo run --locked -p vik-cli -- ./WORKFLOW.md
```

Enable the local HTTP status server when needed:

```sh
cargo run --locked -p vik-cli -- ./WORKFLOW.md --port 3000
```

## Completion Report

Report only:

- GitHub auth status, without token values.
- Linear key presence, without key values.
- Workflow validation result.
- Daemon start command used, if started.
- Any blocker and the exact missing credential or permission.
