# Observation

Vik exposes runtime state through JSON logs and an optional HTTP server.

## Vik Logs

The service daemon writes JSON logs to stdout and to its service log directory.

Default log directory:

```text
<workspace.root>/.vik/logs
```

Service runs also write detached stdout and stderr to:

```text
${VIK_SERVICE_DIR:-$HOME/.vik/service}/service.log
```

Useful commands:

```sh
tail -f "$HOME/code/vik-workspaces/.vik/logs"/vik.log.*
vik service logs --lines 100
vik service logs --follow
```

Adjust the first command when `workspace.root` or `logging.dir` differs from
the checked-in `WORKFLOW.md`.

## HTTP Server

Start the service with HTTP observation:

```sh
vik service start --port 3000
vik work --workflow ./WORKFLOW.md
```

Bind to another interface when needed:

```sh
vik service start \
  --bind-address 0.0.0.0 \
  --port 3000
vik work --workflow ./WORKFLOW.md
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
- aggregate Codex token totals
- rate-limit data when available

Running rows include issue ID, issue identifier, state, optional session ID,
turn count, last event, last message, workspace path, and token usage.

## Sessions

Persisted Codex app-server session logs require VIK-11. Builds that include
VIK-11 append raw Codex app-server JSONL messages under:

```text
<workspace.root>/.vik/sessions/<issue-identifier>-<codex-session-id>.jsonl
```

The issue identifier is the human-facing key such as `VIK-16`. Filename
components are sanitized to ASCII letters, numbers, `.`, `_`, and `-`; other
characters become `_`.

Find logs for one issue:

```sh
find "$HOME/code/vik-workspaces/.vik/sessions" \
  -type f \
  -name 'VIK-16-*.jsonl' \
  -print
```

Inspect one session:

```sh
jq . "$HOME/code/vik-workspaces/.vik/sessions/<file>.jsonl" | less
```

Session files contain raw Codex app-server messages for the session. They do
not replace Vik daemon logs, HTTP snapshots, or Linear workpad notes.

For builds before VIK-11, use `/api/v1/state`, `/api/v1/{issue_identifier}`,
and JSON daemon logs only. Durable per-session JSONL files are unavailable in
those builds.
