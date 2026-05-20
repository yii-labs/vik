# Observation

Current Vik observation surfaces are files:

- foreground stdout logs
- rolling daemon logs
- daemon state JSON
- decoded session `AgentEvent` JSONL

`vik run --port ...` does not enable a full observation API. Current HTTP
behavior is limited to generic webhook intake:

- `POST /intake/issue`
- `POST /intake/issues`

Those routes exist only when `issues.webhook` is configured. Other observation
surfaces are still file based.

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
<workflow-workspace-root>/sessions/<issue.id>/<stage.name>-<uuid-v7>.jsonl
```

The file contains Vik `AgentEvent` records. It is not a byte-for-byte copy of
provider JSONL, but observation records keep the full parsed provider JSON under
`raw`.

Records include messages, token usage, rate-limit observations, tool calls,
subagent/delegation events, unknown valid provider events, completion, and
errors. Tool-call, subagent, and unknown records are JSONL-only evidence; they
do not update the session snapshot fields used for operator status.

The provider session id, when reported, appears inside events and snapshots. It
is not used as the filename.

## HTTP API

Current working endpoints are only the webhook intake endpoints listed above.
The intended observation/control surface is still design context, but it is not
served by current code:

- `GET /api/v1/state`
- `GET /api/v1/issues/{issue_id}`
- `POST /api/v1/refresh`
- `POST /api/v1/issues/{issue_id}/cancel`

Do not put `curl` calls to those planned endpoints in operator runbooks until
they exist in code.
