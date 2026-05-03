# Observation

Vik exposes runtime state through JSON logs and an optional HTTP server.

## Vik Logs

Foreground runs write JSON logs to stdout and to `logging.dir`.

Default log directory:

```text
<workspace.root>/.vik/logs
```

Service runs also write detached stdout and stderr to:

```text
<workflow-directory>/.vik/service/<workflow-name>.log
```

Useful commands:

```sh
tail -f .vik/logs/vik.log.*
cargo run --locked -p vik-cli -- service logs --lines 100
cargo run --locked -p vik-cli -- service logs --follow
```

## HTTP Server

Start the daemon with HTTP observation:

```sh
cargo run --locked -p vik-cli -- ./WORKFLOW.md --port 3000
```

Bind to another interface when needed:

```sh
cargo run --locked -p vik-cli -- ./WORKFLOW.md \
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
- aggregate Codex token totals
- rate-limit data when available

Running rows include issue ID, issue identifier, state, optional session ID,
turn count, last event, last message, workspace path, and token usage.

## Sessions

Session observation is live only until VIK-11 lands. VIK-11 is required for
persisted Codex app-server session logs.

Before VIK-11 lands:

- Use `/api/v1/state` for live `session_id`, turn count, and last event.
- Use `/api/v1/{issue_identifier}` for current issue debug state.
- Use JSON logs for lifecycle events.
- Do not rely on durable per-session history across process restarts.

After VIK-11 lands, update this section with the persisted session-log location,
retention behavior, and lookup commands.
