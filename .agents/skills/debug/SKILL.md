---
name: debug
description:
  Investigate Vik orchestrator stalls, retries, and Codex app-server failures by
  correlating Linear issues, runtime JSON logs, HTTP observation endpoints, and
  workspace evidence; use when runs hang, repeat, or fail unexpectedly.
---

# Debug

## Goals

- Identify why a Vik run is not making progress.
- Correlate Linear issue identity to Codex session and workspace state.
- Read observation sources in a repeatable order.
- Archive enough evidence for another agent to continue without rerunning the
  same discovery.

## Observation Sources

- Linear issue and workpad:
  - Current issue state decides whether the run should still be active.
  - `## Codex Workpad` is the durable progress and handoff record.
- Runtime JSON logs:
  - Vik logs through `tracing_subscriber::fmt().json()`.
  - The daemon writes logs to stdout and to daily files in `logging.dir`.
  - If `logging.dir` is omitted, search `<workspace.root>/.vik/logs`.
  - Daily file names use `vik.log.<date>`.
- Codex app-server session logs:
  - Builds with VIK-11 append raw Codex app-server JSONL messages under
    `<workspace.root>/.vik/sessions`.
  - Session file names use
    `<issue-identifier>-<session-id>.jsonl`.
  - Filename components are sanitized to ASCII letters, numbers, `.`, `_`, and
    `-`; other characters become `_`.
  - Use these logs for raw app-server protocol messages for one session. They
    do not replace daemon logs, HTTP snapshots, or Linear workpad notes.
- HTTP observation API when the daemon runs with `--port`:
  - `GET /api/v1/state` returns running and retrying rows plus token totals.
  - `GET /api/v1/{issue_identifier}` returns one issue debug snapshot.
  - `POST /api/v1/refresh` queues an immediate poll and reconcile pass.
- Workspace evidence:
  - Workspace root comes from `WORKFLOW.md`.
  - If `workspace.root` is omitted, the default is
    `std::env::temp_dir()/vik_workspaces`.
  - Per-issue workspace name is the sanitized Linear issue key.

## Correlation Keys

- `issue_identifier`: human ticket key, for example `VIK-9`.
- `issue_id`: Linear UUID.
- `session_id`: Codex thread-turn pair from `LiveSession`.
- `session_log_path`: session JSONL path derived from `workspace.root`,
  `issue_identifier`, and `session_id`.
- `codex_event`: app-server event name from the JSONL stream.
- `codex_app_server_pid`: local child process id when available.
- `workspace_path`: per-issue workspace path.
- `attempt`: retry attempt tracked by orchestrator state.

Always pair `session_id` with `issue_identifier` or `issue_id`. Concurrent
runs can emit similar Codex events.

## Quick Triage

1. Confirm issue state in Linear.
2. Query the HTTP issue debug endpoint if available.
3. Search runtime logs by `issue_identifier`, then by `issue_id`.
4. Extract `workspace.root`, `workspace_path`, `session_id`, and last
   `codex_event`.
5. Find the matching session log in `<workspace.root>/.vik/sessions`.
6. Trace that `session_id` from process start to terminal outcome across
   daemon logs and the session JSONL file.
7. Classify the failure.
8. Archive observations in the issue workpad.

## Commands

```bash
# Confirm workflow config is valid.
LINEAR_API_KEY=ci-placeholder cargo run --locked -p vik-cli -- check ./WORKFLOW.md

# Start daemon with HTTP observation enabled.
RUST_LOG=info cargo run --locked -p vik-cli -- start ./WORKFLOW.md --port 3000

# Inspect global runtime state.
curl -s http://127.0.0.1:3000/api/v1/state | jq .

# Inspect one issue.
curl -s http://127.0.0.1:3000/api/v1/VIK-9 | jq .

# Force one poll/reconcile pass.
curl -s -X POST http://127.0.0.1:3000/api/v1/refresh | jq .

# Search persisted JSON logs by issue key.
rg -n '"issue_identifier":"VIK-9"|issue_identifier=VIK-9|VIK-9' <log-dir>

# Search persisted JSON logs by Linear UUID.
rg -n '"issue_id":"<linear-uuid>"|issue_id=<linear-uuid>' <log-dir>

# Pull unique session IDs from persisted logs.
rg -o '"session_id":"[^"]+"|session_id=[^ ,}]+' <log-dir> | sort -u

# Find session logs for one issue.
find "<workspace.root>/.vik/sessions" -type f -name 'VIK-9-*.jsonl' -print

# Inspect one session log as JSONL.
jq . "<workspace.root>/.vik/sessions/<issue-identifier>-<session-id>.jsonl" | less

# Trace one session end to end.
rg -n '<session-id>' <log-dir>
rg -n '<session-id>|turn/|response|error|rateLimits' "<session-log-path>"

# Focus on retry, stall, and terminal signals.
rg -n 'stalled_run|retry_dispatch|worker_exit|turn_timeout|turn_failed|turn_cancelled|response_timeout|port_exit|codex_not_found|workflow_reload outcome=failed' <log-dir>
```

Use the configured `logging.dir` from `WORKFLOW.md`. If it is omitted, use
`<workspace.root>/.vik/logs`. Do not create committed log artifacts.

## Investigation Flow

1. Establish active state:
   - Linear state must be one of the configured active states.
   - If the issue is terminal, debug cleanup or stale state instead of worker
     progress.
2. Check HTTP snapshot:
   - In `/api/v1/state`, compare `running`, `retrying`, `codex_totals`, and
     `rate_limits`.
   - In `/api/v1/{issue_identifier}`, read `status`, `attempts`, `running`,
     `retry`, `recent_events`, and `last_error`.
3. Trace lifecycle:
   - `dispatch outcome=started`
   - `codex_process_starting`
   - `codex_process_started`
   - `codex_initialize_starting`
   - `codex_initialize_completed`
   - `codex_thread_starting`
   - `codex_thread_started`
   - `codex_turn_starting`
   - `session_started`
   - streamed `codex_update outcome=received` events
   - `worker_exit outcome=received`
4. Inspect session JSONL:
   - Derive session log directory from `workspace.root`:
     `<workspace.root>/.vik/sessions`.
   - Prefer the exact `session_id` from HTTP state or daemon logs.
   - If the exact ID is unavailable, list files matching
     `<issue_identifier>-*.jsonl` and sort by modified time.
   - Confirm the selected file belongs to the current attempt by matching the
     session id, nearby daemon timestamps, or lifecycle sequence.
   - Search the selected file for app-server method names, terminal statuses,
     errors, approvals, rate-limit messages, and the last streamed response.
   - Do not paste full prompts, tool payloads, credentials, or full session
     logs into Linear.
5. Inspect workspace:
   - Check the issue workspace path from HTTP or logs.
   - Read git branch, git status, workpad context, and recent file changes.
   - Do not modify the workspace until the failure target is explicit.
6. Classify and fix:
   - Pick one primary class from the list below.
   - Capture the exact evidence line or endpoint field that proves it.
   - Implement the smallest fix that addresses that class.

## Failure Classes

- Dispatch gate:
  - `dispatch_preflight outcome=failed`
  - `retry_dispatch outcome=gated`
  - no available orchestrator slots
- Workflow reload:
  - `workflow_reload outcome=failed`
  - invalid `WORKFLOW.md`
  - reload keeps last-good config
- Tracker or Linear:
  - `tracker_fetch_candidates outcome=failed`
  - `linear_graphql` tool failure
  - missing Linear credentials
- Workspace:
  - `hook_failed`
  - `hook_timeout`
  - `workspace_path_outside_root`
  - `invalid_workspace_cwd`
- Codex startup:
  - `codex_not_found`
  - `port_exit`
  - `response_timeout` before thread start
- Turn execution:
  - `turn_failed`
  - `turn_cancelled`
  - `turn_timeout`
  - unsupported user-input or elicitation request
- Stall and retry:
  - no event before `stall_timeout_ms`
  - `stalled_run outcome=retrying`
  - retry backoff grows without new progress
- Rate limit or token pressure:
  - `account/rateLimits/updated`
  - rising token totals with no useful final event

## Observation Archive

Archive findings in the existing `## Codex Workpad` comment. Prefer the
`### Notes` section for short evidence. Add this block when the investigation is
large enough to need handoff:

```md
### Observation Archive

- Timestamp:
- Issue:
- Environment:
- Evidence:
  - Linear:
  - HTTP:
  - Logs:
  - Session logs:
  - Workspace:
- Correlation:
  - issue_id:
  - issue_identifier:
  - session_id:
  - session_log_path:
  - codex_app_server_pid:
  - workspace_path:
- Classification:
- Probable root cause:
- Action taken:
- Validation:
```

Keep archive entries factual. Include commands, endpoint paths, output snippets,
and timestamps. Do not paste secrets, access tokens, full prompts, or full log
files.

## Notes

- Prefer `rg` over `grep` for local evidence search.
- Prefer HTTP observation endpoints before broad log scans when the daemon is
  still running.
- Treat missing required log fields as an observability bug.
- Keep debug artifacts out of commits unless they are intentional docs or tests.
