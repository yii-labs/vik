---
name: debug
description: Debug this Vik repo after the single-crate refactor. Use when Codex needs to diagnose Vik runtime, workflow, daemon, orchestration, hook, prompt-rendering, provider-adapter, session JSONL, or workspace/log/state-file problems in /Users/yii/code/vik.
---

# Debug Vik

Use this for evidence-first debugging in this repo. Trust current source and command output over older docs when they disagree.

## First Pass

Run these from repo root:

```sh
git status --short --branch
cargo run -- doctor --json ./workflow.yml
cargo run -- status ./workflow.yml
```

Do not start `vik run` until you know which workflow and tracker commands it will execute. A run can spawn Codex or Claude Code and execute workflow hooks.

For repo changes, read required docs first:

```text
docs/development/index.md
docs/development/checks.md
docs/development/pull-requests.md
docs/development/code-conventions.md
```

## Current Runtime Shape

Vik is one Rust binary crate.

```text
src/main.rs              binary entry
src/cli/                 CLI parser and command dispatch
src/config/              workflow.yml schema and doctor diagnostics
src/workflow/            runtime supervisor built from parsed schema
src/workspace/           canonical path layout under resolved workflow workspace root
src/logging/             JSON tracing setup and phase/span helpers
src/daemon/              detach, signals, state.json, lifecycle commands
src/orchestrator/        intake loop, dispatch, running-stage registry
src/session/             one agent run, prompt render, JSONL writer, snapshot
src/agent/               Codex and Claude Code provider adapters
src/template/            MiniJinja and prompt !`exec(...)` expansion
src/hooks/               workflow hook rendering and shell execution
src/shell/               subprocess primitive and timeout/cancel handling
```

Debug layer direction with `docs/development/architecture.md` and `docs/development/code-conventions.md`.

## Workflow Basics

Default CLI shape:

```sh
cargo run -- <command> [WORKFLOW]
cargo run -- doctor ./workflow.yml
cargo run -- run ./workflow.yml
cargo run -- status ./workflow.yml
```

Important workflow facts:

- `workflow.yml` is source of truth.
- YAML `workspace.root` is resolved relative to workflow file directory.
- Vik appends `workflows/<workflow-path-key>` to make the runtime workspace root.
- `<workflow-path-key>` is the absolute workflow file path with `/` replaced by `-`.
- `<workspace.root>/workflows` must exist before `vik run`; Vik creates only the final workflow root.
- `issue.state` matches `issue.stages.<stage>.when.state` by exact string equality.
- `issues.pull.command` must print one JSON issue sequence.
- Issue identifiers become path segments under `<workflow-workspace-root>/issues`; unsafe names break dispatch.
- Prompt files are MiniJinja first, then prompt-command expansion.
- Hooks are MiniJinja only; hooks do not run prompt-command expansion.

Checked-in `workflow.yml` has `workspace.root: .vik`, so current Vik-owned state lives under:

```text
/Users/yii/code/vik/.vik/workflows/-Users-yii-code-vik-workflow.yml/
```

Always use `vik status` or `Workflow::workspace().*` path helpers to find current state. Ignore older leftover paths unless the status output points there.

## State And Logs

Canonical path layout comes from `src/workspace/mod.rs`:

```text
<workflow-workspace-root>/service/state.json
<workflow-workspace-root>/logs/vik.log.YYYY-MM-DD
<workflow-workspace-root>/logs/vik-error.log.YYYY-MM-DD
<workflow-workspace-root>/sessions/<issue.id>/<issue.state>-<uuid-v7>.jsonl
<workflow-workspace-root>/issues/<issue.id>/
```

`vik status` prints:

```text
status
state_file
pid
bind_address
started_at
log_dir
sessions_dir
workflow_path
command
```

Log files are newline-delimited JSON. Useful fields:

- `timestamp`, `level`, `message`, `target`
- `phase`: `startup`, `intake`, `dispatch`, `stage_run`, `hook`, `server`, `daemon`
- `issue_identifier`, `stage_name`, `agent_profile`, `runtime`, `session_id`
- `error`, `duration_ms`, `state_file`, `workflow_path`, `workspace_root`

Fast log commands:

```sh
cargo run -- status ./workflow.yml
tail -n 100 .vik/workflows/-Users-yii-code-vik-workflow.yml/logs/vik.log.*
tail -n 100 .vik/workflows/-Users-yii-code-vik-workflow.yml/logs/vik-error.log.*
jq -c 'select(.level=="ERROR" or .phase=="intake" or .phase=="stage_run" or .phase=="hook")' .vik/workflows/-Users-yii-code-vik-workflow.yml/logs/vik.log.*
```

If paths differ, replace the example `logs` directory with `log_dir` from `vik status`.

## Session JSONL

Session JSONL files store normalized `AgentEvent` records, not raw provider stdout.

Event shapes:

```json
{"kind":"session_started","session_id":"..."}
{"kind":"message","text":"..."}
{"kind":"token_usage","input":120,"output":45,"cache_read":12}
{"kind":"rate_limit","scope":"codex:tokens_per_min","remaining":100,"reset_at":"...","observed_at":"..."}
{"kind":"completed"}
{"kind":"error","detail":"..."}
```

Inspect session logs:

```sh
find .vik/workflows/-Users-yii-code-vik-workflow.yml/sessions -type f -name '*.jsonl' -print
jq -c . .vik/workflows/-Users-yii-code-vik-workflow.yml/sessions/<issue.id>/*.jsonl
jq -r 'select(.kind=="message") | .text' .vik/workflows/-Users-yii-code-vik-workflow.yml/sessions/<issue.id>/*.jsonl
```

Provider fixture input lives under:

```text
tests/fixtures/agent_events/codex/*.jsonl
tests/fixtures/agent_events/claude_code/*.jsonl
```

Use adapter tests when event mapping is suspect:

```sh
cargo test agent::adapters
```

## Common Failure Routes

Workflow parse or schema error:

- Start in `src/workflow/loader.rs`, `src/workflow/builder.rs`, `src/config/*`.
- Run `cargo run -- doctor --json ./workflow.yml`.
- Unknown old top-level fields should not survive review.

Wrong path or missing logs:

- Start in `src/workspace/mod.rs`.
- Then inspect `src/cli/run.rs`, `src/daemon/state.rs`, `src/logging/mod.rs`.
- Use `vik status`; do not hand-derive workspace artifact paths outside `Workspace`.

Intake returns no work or fails:

- Start in `src/orchestrator/intake.rs`.
- Check `workflow.yml -> issues.pull.command`.
- Command stdout must be one JSON issue sequence.
- Returned issue fields are `identifier`, `title`, `state`; aliases `desc` and `status` also parse.

Stage does not run:

- Start in `src/orchestrator/mod.rs::should_dispatch`.
- Check exact state match.
- Check `max_issue_concurrency`.
- Check existing running reservations in logs.

Hook failure:

- Start in `src/hooks/mod.rs`.
- Hooks run through `sh -c` on Unix, `cmd /C` on Windows.
- Timeout is 30 seconds.
- `after_create` context has `issue`, `workspace_root`, `workflow_path`, and `env`.
- `before_run` and `after_run` context has `issue`, `issue.stage.name`,
  `workspace_root`, `workflow_path`, and `env`.
- If tracker payload includes `stage`, stage hooks see that tracker value at
  `issue.stage.value`.
- Nonzero stderr tail is capped at 2048 bytes.

Prompt render failure:

- Start in `src/session/mod.rs::render_prompt` and `src/template/*`.
- MiniJinja is strict; missing variables fail.
- Prompt commands use syntax ``!`exec(command)` `` and run after Jinja render.
- Prompt command timeout is 30 seconds.

Agent spawn or event mapping failure:

- Start in `src/session/mod.rs::spawn_inner`, `src/agent/adapters/*`, `src/shell/command_ext.rs`.
- Codex command shape is `codex exec ... --json -m <model>` with prompt on stdin.
- Claude Code command shape is in `src/agent/adapters/claude_code/mod.rs`; verify actual args before assuming docs are right.
- Missing binary or auth usually appears as spawn error, provider error event, or session JSONL error.

Daemon lifecycle failure:

- Start in `src/daemon/state.rs`, `src/daemon/lifecycle.rs`, `src/daemon/signals/*`, `src/daemon/detach/*`.
- `status` missing state exits 0.
- `stop` missing state exits nonzero.
- `uninstall` missing state is a no-op.
- `restart` stop phase treats missing or stale state as not running, then starts a detached daemon.

Issue workspace cleanup:

- No cleanup CLI exists in current source.
- Stop active sessions before deleting `<workflow-workspace-root>/issues/<issue.id>` manually.

## Known Current Gaps

- HTTP API is not implemented in current source. `vik run --port` reaches `todo!` in `src/cli/run.rs`; no `src/server` module exists.
- Some usage docs and README still mention HTTP endpoints or Codex app-server. Treat those as stale unless source confirms them.

## Verification

For skill-only or docs-only changes, at minimum run:

```sh
cargo run -- doctor --json ./workflow.yml
cargo fmt --all -- --check
git diff --check
```

For Rust behavior changes, run full gate:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
git diff --check
```

Before handoff, sync per repo rule:

```sh
git fetch origin main
git pull --ff-only origin main
```
