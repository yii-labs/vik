# Observation

Vik exposes runtime state through JSON logs and an optional HTTP server.

## Vik Logs

Foreground runs write JSON logs to stdout and to `logging.dir`.

Default log directory:

```text
<workspace.root>/logs
```

Service runs also write detached stdout and stderr to:

```text
<workflow-directory>/.vik/service/<workflow-stem>-<path-hash>.log
```

Useful commands:

```sh
tail -f "$HOME/code/vik-workspaces/logs"/service.log.*
tail -f "$HOME/code/vik-workspaces/logs"/session.log.*
vik service logs --lines 100
vik service logs --follow
```

Adjust the first two commands when `workspace.root` or `logging.dir` differs
from the checked-in `WORKFLOW.md`.

## HTTP Server

Start the daemon with HTTP observation:

```sh
vik start ./WORKFLOW.md --port 3000
```

Bind to another interface when needed:

```sh
vik start ./WORKFLOW.md \
  --bind-address 0.0.0.0 \
  --port 3000
```

Endpoints:

- `GET /`: small HTML dashboard.
- `GET /api/v1/state`: runtime snapshot.
- `GET /api/v1/{issue_identifier}`: issue debug snapshot.
- `POST /api/v1/refresh`: request poll and reconcile.

Examples:

```sh
curl -fsS http://127.0.0.1:3000/api/v1/state | jq .
curl -fsS http://127.0.0.1:3000/api/v1/VIK-16 | jq .
curl -fsS -X POST http://127.0.0.1:3000/api/v1/refresh | jq .
```

## State Snapshot

`/api/v1/state` includes:

- generated timestamp
- counts by runtime bucket
- running issue rows
- retry rows
- aggregate agent token totals
- rate-limit data when available

Running rows include issue ID, issue identifier, state, optional session ID,
turn count, last event, last message, workspace path, and token usage.

## Sessions

Codex app-server session traffic is emitted through tracing to the session log:

```text
<logging.dir>/session.log.<date>
```

The default `logging.dir` is `<workspace.root>/logs`, but workflows can set a
different directory.

Inspect session events:

```sh
jq . "$HOME/code/vik-workspaces/logs"/session.log.* | less
```

Session records include:

- `agent`: currently `codex`.
- `event`: Codex method name, or the correlated request method for RPC
  responses.
- `params`: structured JSON from request params, response result, or error.
- `issue_id` and `issue_identifier`.
- `session_id`, `thread_id`, and `turn_id` when known or derivable from the
  message.
- `rpc_id` for JSON-RPC request/response correlation.

Service events stay in `<logging.dir>/service.log.<date>` and do not include
Codex message payloads.
