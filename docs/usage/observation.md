# Observation

Vik exposes runtime state through JSON logs and an optional HTTP server.

## Vik Logs

Foreground runs write service JSON logs to stdout. Service and session JSON
files are written under `logging.dir`.

Default log directory:

```text
<workspace.root>/logs
```

Useful commands:

```sh
tail -f "$HOME/code/vik-workspaces/logs"/service.log.*
tail -f "$HOME/code/vik-workspaces/logs"/session.log.*
```

Adjust the commands when `workspace.root` or `logging.dir` differs from the
checked-in `WORKFLOW.md`.

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

Agent session traffic is emitted through tracing to the session log from the
worker boundary:

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
- `event`: worker run boundary or agent event name.
- `params`: structured JSON for the run boundary, returned agent event, usage,
  rate-limit data, or error.
- `issue_id` and `issue_identifier`.
- `session_id`, `thread_id`, and `turn_id` when the returned agent event
  includes session identity.

Service events stay in `<logging.dir>/service.log.<date>` and do not include
agent session payloads.
