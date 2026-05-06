# Observation

Vik exposes runtime state through JSON logs and an optional HTTP server.

## Vik Logs

Foreground runs write JSON logs to stdout and to `logging.dir`.

Default log directory:

```text
<workspace.root>/logs
```

Detached services also write early startup stderr to:

```text
<workspace.root>/logs/vik-service.log
```

Useful commands:

```sh
tail -f "$HOME/code/vik-workspaces/logs"/vik.log.*
tail -n 100 "$HOME/code/vik-workspaces/logs"/vik.log.*
```

Adjust the first command when `workspace.root` or `logging.dir` differs from
the checked-in `WORKFLOW.md`.

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

Persisted agent session logs require VIK-11. When the Codex runtime is in use,
builds that include VIK-11 append raw Codex app-server JSONL messages under:

```text
<workspace.root>/sessions/<issue-identifier>-<agent-session-id>.jsonl
```

The issue identifier is the human-facing key such as `VIK-16`. Filename
components are sanitized to ASCII letters, numbers, `.`, `_`, and `-`; other
characters become `_`.

Find logs for one issue:

```sh
find "$HOME/code/vik-workspaces/sessions" \
  -type f \
  -name 'VIK-16-*.jsonl' \
  -print
```

Inspect one session:

```sh
jq . "$HOME/code/vik-workspaces/sessions/<file>.jsonl" | less
```

With the Codex runtime, session files contain raw Codex app-server messages for
the session. They do not replace Vik daemon logs, HTTP snapshots, or tracker
workpad notes.

For builds before VIK-11, use `/api/v1/state`, `/api/v1/{issue_identifier}`,
and JSON daemon logs only. Durable per-session JSONL files are unavailable in
those builds.
