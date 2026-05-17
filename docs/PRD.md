# Vik Product Requirements

## Summary

Vik is a local service that orchestrates coding agents against issues from an
external tracker. A workflow file defines how to fetch issues, how issue states
map to stages, which agent profile each stage uses, and which prompt file each
run receives.

Vik does not own tracker state. Workflow pull commands read tracker state, and
prompt files own tracker updates. Vik observes state on each intake cycle and
dispatches matching stages.

## Problem

Unattended coding-agent runs need a supervisor. The supervisor decides when to
start an agent, which prompt to use, where Vik-owned files live, how many issues
may run at once, how to log events, and how to stop work.

Without a supervisor, teams repeat fragile shell scripts and lose visibility
when a run stalls, crashes, or restarts.

## Goals

- One YAML Workflow Definition at `workflow.yml`.
- Tracker-agnostic issue intake through workflow commands and state transitions
  through prompt-authored commands.
- One-shot agent CLI processes for stage sessions.
- Support Codex and Claude Code runtimes.
- One issue workspace per issue id.
- In-memory running state with cold-start recovery from tracker state.
- File-based observation through logs, state file, and session JSONL.

## Non-goals

- No built-in tracker client.
- No injected issue-management tool.
- No managed authentication.
- No resume of in-flight sessions across daemon restarts.
- No workflow hot reload.
- No multi-workflow daemon.
- No HTTP API in current implementation.

## Core Concepts

- **Workflow Definition**: YAML file with `loop`, `workspace`, `agents`,
  `issues`, and `issue`.
- **Workflow**: runtime supervisor wrapping parsed schema, resolved paths, and
  hook runner.
- **Agent Profile**: named `runtime`, `model`, and optional `args`.
- **Agent Runtime**: provider adapter selected by `runtime`.
- **Issue Stage**: one named stage matched by exact issue state.
- **Issue Workspace**:
  `<workflow-workspace-root>/issues/<issue.id>/`.
- **Session**: one stage execution with state snapshot and decoded `AgentEvent`
  JSONL.
- **Session Factory**: orchestrator spawn boundary for sessions.

## Workflow Config

- `workflow.yml` is the runtime config source.
- Workflow path defaults to `./workflow.yml`.
- Relative paths resolve from the workflow file directory.
- `~` expands to the home directory.
- `$VAR` and `${VAR}` are not expanded in workflow string values.
- `vik doctor [WORKFLOW]` loads YAML and runs schema diagnostics.
- `--strict` promotes warnings to a non-zero exit.
- `--json` emits machine-readable diagnostics.
- Current doctor does not check prompt file existence, external binaries, auth,
  or tracker access.

## Intake

- Intake runs `issues.pull.command` from the workflow file directory.
- `issues.pull.idle_sec` controls sleep after each intake cycle. Default: `5`.
- `loop.max_iterations` stops intake after the configured number of cycles.
- Pull command stdout must be one raw JSON issue sequence.
- Stdout parses as a JSON sequence of issue objects.
- Issue fields:
  - `identifier` or `id`
  - `title`
  - `state` or `status`
  - optional `description` or `desc`
- Extra issue fields are preserved under `issue` in hook and prompt rendering
  context.
- Duplicate issue ids in one intake batch: first wins.

## Dispatch

- Matching rule:

```text
issue.state == issue.stages.<stage>.when.state
```

- Matching is exact and case-sensitive.
- Workflow stage order is preserved.
- Multiple stages may match the same issue state.
- Orchestrator reserves `(issue.id, stage.name)` before async issue
  setup starts.
- `loop.max_issue_concurrency` limits active issue ids, not stage count.
- If capacity is full, new issue ids are skipped for that cycle.

## Reconciliation Model

Issue state lives in the tracker. Vik never writes tracker state on its own.
Stage prompts must run the commands that move issues between states. Later
intake cycles observe new tracker state.

Running state is in memory only. Restart loses active session tracking. The
next intake cycle rebuilds work from tracker state.

## Agent Runtime

Current adapter trait: `AgentAdapter`.

Adapters own:

- provider command construction
- provider JSONL event mapping

Current runtimes:

- `codex`: `codex exec <args...> --json -m <model>`, prompt on stdin.
- `claude_code`: `claude --verbose --output-format stream-json --model <model>
-p <prompt> <args...>`.

Current `args` forwarding supports strings, numbers, booleans, and string
sequences. Profile timeout fields and stall-watchdog config are not implemented
today.

## Session

`SessionFactory` creates sessions for stage runs. A session:

- renders the prompt file
- starts the selected provider process
- maps provider stdout JSONL to `AgentEvent`
- writes decoded events to JSONL
- tracks `SessionState`, last message, token usage, and rate-limit observations
- exposes cancellation through the child process wrapper

Session files:

```text
<workflow-workspace-root>/sessions/<issue.id>/<stage.name>-<uuid-v7>.jsonl
```

The filename is generated by Vik. Provider session ids, when present, are event
data, not filenames.

Agent subprocess cwd is the issue workspace.

## Workspace

- `workspace.root` is optional. It uses `VIK_HOME` when set; otherwise it uses
  the OS home directory.
- Relative `workspace.root` values resolve from the workflow file directory.
- Vik appends `workflows/<workflow-path-key>/` to build one
  workflow-scoped workspace root per workflow file.
- `<workflow-path-key>` is the absolute workflow file path with `/` replaced by
  `-`.
- The workflow-scoped workspace root is auto-created recursively when missing.
- Issue workspaces live at `<workflow-workspace-root>/issues/<issue.id>/`.
- Vik-owned state lives under the workflow-scoped workspace root.
- Workspaces persist across runs.
- Vik does not provide an issue-workspace cleanup command.

## Hooks

- `issue.hooks.after_create` runs after Vik first creates the issue workspace
  and before stage launch for that setup. Existing issue workspaces skip it.
- Stage `before_run` runs before session spawn. Failure aborts that stage.
- Stage `after_run` runs after terminal session state except cancellation.
  Failure is logged.
- Hooks run in the issue workspace.
- Hooks are MiniJinja-rendered shell snippets.
- Hooks do not expand prompt commands.
- Hook template context includes `issue`, `workspace_root`, `workflow_path`, and
  `env`. `issue` contains `id`, `title`, `description`, `state`, `workdir`, and
  optional extra issue fields. Stage hook and prompt context also includes
  `issue.stage.name`.

## Prompt Rendering

- MiniJinja strict rendering runs first.
- Prompt-command expansion runs second.
- Supported command syntax: ``!`exec(command)` `` and ```exec(command)` ``.
- Prompt commands run through the system shell with a 30-second timeout.
- stdout is injected as text.
- one trailing newline is trimmed.
- non-zero exit fails rendering.
- Prompt context is the same context used by hooks.
- Prompt context can read `env.<VAR>` from the Vik process environment.
- Current code does not expose root `stage`, `workflow`, `loop`, or `profile`
  template objects.

## CLI Surface

```text
vik doctor [--strict] [--json] [WORKFLOW]
vik run [-d|--detached] [WORKFLOW]
vik status [WORKFLOW]
vik stop [WORKFLOW]
vik restart [WORKFLOW]
vik uninstall [WORKFLOW]
vik --help
```

`run` and `restart` parse `--port` and `--bind-address`, but the HTTP server is
not implemented. Do not document HTTP endpoints as working behavior yet.

## Logging And Observation

- Foreground `vik run` logs to stdout and file appenders.
- Detached `vik run -d` logs to file appenders only.
- Log directory: `<workflow-workspace-root>/logs/`.
- `vik.log.YYYY-MM-DD`: INFO and above.
- `vik-error.log.YYYY-MM-DD`: ERROR only.
- Retention is hardcoded to 7 days on logging init.
- Daemon state: `<workflow-workspace-root>/service/state.json`.
- Session event logs: `<workflow-workspace-root>/sessions/.../*.jsonl`.

## Daemon

- `vik run -d [WORKFLOW]` detaches on Unix with double-fork and `setsid`.
- Windows detach is currently unsupported.
- State file records workflow path, cwd, pid, bind address, port, start time,
  log dir, sessions dir, and command.
- `stop` sends SIGTERM and waits up to 30 seconds for daemon exit.
- The daemon shutdown token cancels intake and running sessions.
- SIGHUP is ignored.

## Planned HTTP API

These endpoints remain planned design, not current behavior:

- `GET /api/v1/state`
- `GET /api/v1/issues/{issue_id}`
- `POST /api/v1/refresh`
- `POST /api/v1/issues/{issue_id}/cancel`

No current CLI command depends on these planned routes.

## Related Documents

- [CONTEXT.md](../CONTEXT.md)
- [ADR-0001 YAML workflow definition](adr/0001-yaml-workflow-definition.md)
- [ADR-0002 Explicit issue management commands](adr/0002-explicit-issue-management-commands.md)
- [ADR-0003 Stateless reconciliation model](adr/0003-stateless-reconciliation-model.md)
- [ADR-0004 In-memory running state](adr/0004-in-memory-running-state.md)
- [ADR-0005 Session factory indirection](adr/0005-session-factory-indirection.md)
- [ADR-0006 Single binary crate](adr/0006-single-binary-crate.md)
- [ADR-0007 Hooks stay outside session state](adr/0007-hooks-reads-session-status.md)
- [Configuration](usage/configuration.md)
- [Service Daemon](usage/service-daemon.md)
- [Observation](usage/observation.md)
