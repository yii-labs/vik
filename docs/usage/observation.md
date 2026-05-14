# Observation

Current Vik observation surfaces are files:

- foreground stdout logs
- rolling daemon logs
- daemon state JSON
- session `AgentEvent` JSONL with provider and semantic records

The HTTP API is planned but not implemented. `vik run --port ...` currently
parses the flag and then reaches the unimplemented server path.

## Logs

Foreground `vik run` prints compact tracing output to stdout and writes JSON log
files under:

```text
<workflow-workspace-root>/logs/
```

Detached `vik run -d` disables stdout logging and writes only file logs.

Files:

- `vik.log.YYYY-MM-DD`: INFO and above.
- `vik-error.log.YYYY-MM-DD`: ERROR only.

Examples:

```sh
tail -n 100 <log_dir>/vik.log.*
tail -f <log_dir>/vik-error.log.*
```

## Daemon State

The daemon state file is:

```text
<workflow-workspace-root>/service/state.json
```

It records:

- `workflow_path`
- `cwd`
- `pid`
- `port`
- `bind_address`
- `started_at`
- `log_dir`
- `sessions_dir`
- `command`

Prefer `vik status [WORKFLOW]` over reading this file by hand.

## Sessions

Session JSONL files live under:

```text
<workflow-workspace-root>/sessions/<issue.id>/<issue.state>-<uuid-v7>.jsonl
```

The file contains Vik `AgentEvent` records. Records include typed provider
records with raw parsed provider JSON values, messages, token usage, rate-limit
observations, completion, and errors when the provider adapter maps them.

The provider session id, when reported, appears inside events and snapshots. It
is not used as the filename.

## Planned HTTP API

The intended HTTP surface is still useful design context, but it is not served
by current code:

- `GET /api/v1/state`
- `GET /api/v1/issues/{identifier}`
- `POST /api/v1/refresh`
- `POST /api/v1/issues/{identifier}/cancel`

Do not put `curl` calls to those endpoints in operator runbooks until the
server module lands.
