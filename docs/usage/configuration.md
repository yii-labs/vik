# Configuration

Vik reads YAML front matter from `WORKFLOW.md`, then renders the markdown body
as the issue prompt. Config values are resolved before the daemon dispatches
work.

## Basic

Minimal shape:

```yaml
---
tracker:
  kind: linear
  project_slug: "vik-08c9cf588aa7"
workspace:
  root: ~/code/vik-workspaces
hooks:
  after_create: |
    git clone --depth 1 git@github.com:yii-labs/vik .
agent:
  default: codex
  max_concurrent_agents: 10
codex:
  command: codex --config shell_environment_policy.inherit=all app-server
---

You are working on {{ issue.identifier }}.
```

Validate:

```sh
vik check ./WORKFLOW.md
```

## Tracker

`tracker.kind` must be `linear`.

Fields:

- `endpoint`: defaults to `https://api.linear.app/graphql` for Linear.
- `api_key`: optional when `LINEAR_API_KEY` is set in the environment or `.env`.
- `project_slug`: Linear project slug Vik polls.
- `active_states`: states Vik may claim.
- `terminal_states`: states that stop tracking and may trigger cleanup.
- `filter`: optional delegable issue filter. Omitted filter values and empty
  lists match all issues.
  - `assignees`: Linear user IDs, names, display names, or email addresses.
  - `tags`: Linear label names.

`LINEAR_API_KEY` is loaded from `.env` before dispatch validation. Do not commit
real keys.

Limit delegation to issues assigned to specific users and tagged with specific
Linear labels:

```yaml
tracker:
  filter:
    assignees: [user-a, user-b]
    tags: [agent, codex]
```

## Polling

`polling.interval_ms` controls the main poll loop. Default: `30000`.

```yaml
polling:
  interval_ms: 5000
```

## Workspace

`workspace.root` is where Vik creates per-issue directories. Relative paths are
resolved from the workflow directory. `~` is expanded.

Vik sanitizes workspace names and prevents paths from escaping the root.

## Logging

`logging.dir` controls daemon JSON log files. Default:

```text
<workspace.root>/.vik/logs
```

Each run logs to stdout and to a daily file named `vik.log.<date>`.

## Hooks

Hooks are trusted shell snippets from `WORKFLOW.md`.

Fields:

- `after_create`: run once after a new issue workspace is created and after any
  workspace setup has completed.
- `before_run`: run before the selected coding agent starts.
- `after_run`: run after the selected coding agent exits.
- `before_remove`: run before terminal cleanup.
- `timeout_ms`: hook timeout. Default: `60000`.

Default clone hook:

```yaml
hooks:
  after_create: |
    git clone --depth 1 git@github.com:yii-labs/vik .
```

Use HTTPS when the runtime has token auth but no SSH key:

```yaml
hooks:
  after_create: |
    git clone --depth 1 https://github.com/yii-labs/vik .
```

## Agent

Fields:

- `default`: fallback coding agent for issues that do not match an agent label
  filter. Supported values: `codex`, `claude-code`. Default: `codex`.
- `max_concurrent_agents`: global concurrency. Default: `10`.
- `max_turns`: max coding-agent turns per issue attempt. Default: `20`.
- `max_retry_backoff_ms`: retry backoff cap. Default: `300000`.
- `max_concurrent_agents_by_state`: optional per-state concurrency limits.

Agent label filters route matching issues to a specific adapter:

```yaml
agent:
  default: codex
codex:
  filter:
    tags: [codex]
claude-code:
  filter:
    tags: [claude]
```

If multiple agent filters match, the default agent wins when it is one of the
matches. Otherwise Vik uses a deterministic supported-agent order. If no filter
matches, Vik uses `agent.default`.

## Codex

`codex.command` launches Codex app-server. Default: `codex app-server`.

Common fields:

- `model`
- `model_reasoning_effort`
- `filter.tags`
- `approval_policy`
- `approvals_reviewer`
- `thread_sandbox`
- `turn_sandbox_policy`
- `turn_timeout_ms`
- `read_timeout_ms`
- `stall_timeout_ms`

When `model` or `model_reasoning_effort` is set, `codex.command` must contain
the `app-server` token so Vik can inject model CLI config before it.

Official Codex config reference:

- <https://developers.openai.com/codex/cli/reference>
- <https://developers.openai.com/codex/config-basic>
- <https://developers.openai.com/codex/config-advanced>

See [Codex Agent](agents/codex.md) for setup and validation.

## Claude Code

`claude-code.command` launches Claude Code headless mode. Default:
`claude -p --output-format stream-json --input-format text --verbose`.

Common fields:

- `command`
- `filter.tags`
- `model`
- `permission_mode`
- `turn_timeout_ms`

Vik writes the rendered issue prompt to stdin. Vik appends `--max-turns 1` for
each headless process and repeats that process up to `agent.max_turns`, checking
issue state between turns.

See [Claude Code Agent](agents/claude-code.md) for setup and validation.

## Server

Set a default observation port in workflow config:

```yaml
server:
  port: 3000
```

CLI `--port` overrides `server.port`. CLI `--bind-address` controls the HTTP
bind host.

## References

- [Get Started](get-started.md)
- [Docker](docker.md)
- [Service Daemon](service-daemon.md)
- [Observation](observation.md)
- [Codex Agent](agents/codex.md)
- [Claude Code Agent](agents/claude-code.md)
- Linear GraphQL API: <https://linear.app/developers/graphql>
- GitHub CLI auth: <https://cli.github.com/manual/gh_auth>
- Codex CLI reference: <https://developers.openai.com/codex/cli/reference>
