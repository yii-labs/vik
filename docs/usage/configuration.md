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
  runtime: codex
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

`tracker.kind` must name a supported tracker provider.

Common fields:

- `endpoint`: optional provider API endpoint override.
- `api_key`: optional when the provider token is set in the environment or
  `.env`.
- `active_states`: states Vik may claim.
- `terminal_states`: states that stop tracking and may trigger cleanup.
- `filter`: optional delegable issue filter. Omitted filter values and empty
  lists match all issues.

Provider-specific fields:

- Linear requires `project_slug` and uses `LINEAR_API_KEY` by default.
- GitHub requires `repository` and uses `GH_TOKEN` or `GITHUB_TOKEN` by
  default.

Limit delegation to issues assigned to specific users and tagged with specific
labels:

```yaml
tracker:
  filter:
    assignees: [user-a, user-b]
    tags: [agent, codex]
```

Tracker reference:

- [Linear](trackers/linear.md)
- [GitHub](trackers/github.md)

During agent runs, Vik exposes one tracker-agnostic Codex app-server tool named
`vik_issue`. Its `action` field supports common issue operations including
`get_issue`, `list_comments`, `update_issue`, `create_comment`,
`update_comment`, `upload_attachment`, and `link_pr`. Each tool call is routed
to the configured tracker provider.

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
The direct-child support directory names `.vik`, `logs`, and `sessions` are
reserved and cannot be used as issue workspace names.

## Logging

`logging.dir` controls daemon JSON log files. Default:

```text
<workspace.root>/logs
```

Each run logs to stdout and to daily files under `logging.dir`:

- `service.log.<date>` contains service, manager, orchestrator, hook, tracker,
  HTTP, and lifecycle events.
- `session.log.<date>` contains worker-boundary agent run and return events
  with `agent`, `event`, structured `params`, and issue/session identity.

## Hooks

Hooks are trusted shell snippets from `WORKFLOW.md`.

Fields:

- `after_create`: run once after a new issue workspace is created.
- `before_run`: run before the agent runtime starts.
- `after_run`: run after the agent runtime exits.
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

- `runtime`: agent runtime adapter. Default: `codex`.
- `max_concurrent_agents`: global concurrency. Default: `10`.
- `max_turns`: max runtime turns per issue attempt. Default: `20`.
- `max_retry_backoff_ms`: retry backoff cap. Default: `300000`.
- `max_concurrent_agents_by_state`: optional per-state concurrency limits.

Supported runtime values:

- `codex`: run Codex app-server through the Vik Codex adapter.

## Codex

Used when `agent.runtime` is `codex`.

`codex.command` launches Codex app-server. Default: `codex app-server`.

Common fields:

- `model`
- `model_reasoning_effort`
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
- [Linear Tracker](trackers/linear.md)
- [GitHub Tracker](trackers/github.md)
- Linear GraphQL API: <https://linear.app/developers/graphql>
- GitHub CLI auth: <https://cli.github.com/manual/gh_auth>
- Codex CLI reference: <https://developers.openai.com/codex/cli/reference>
