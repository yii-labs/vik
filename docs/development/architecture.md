# Vik Architecture

This document describes current code. It is not a target design.

## Overview

Vik is one Rust binary crate. CLI startup loads `workflow.yml` into
`WorkflowSchema`, builds a `Workflow`, prepares the workflow-scoped workspace
root, installs logging, writes daemon state, and runs the event-driven
orchestrator.

The orchestrator owns running-stage state. Intake, issue setup, stage launch,
session monitoring, and hook execution run in background tasks and report back
through typed channels.

There is no `src/server/` module today. HTTP API docs describe planned work, not
current runtime behavior.

## Folder Structure

```text
src/
|-- main.rs          binary entry
|-- cli/             clap parsing and subcommand execution
|-- config/          workflow.yml serde types and diagnostics
|-- workflow/        runtime supervisor built from loaded schema
|-- workspace/       workflow-scoped workspace path layout
|-- logging/         tracing subscriber, phases, spans, retention
|-- shell/           CommandExt wrapper for timeout and cancellation
|-- template/        MiniJinja context plus prompt command expansion
|-- agent/           AgentAdapter trait, Codex and Claude Code adapters
|-- session/         session spawn, event stream, snapshots, JSONL writer
|-- hooks/           after_create, before_run, after_run shell hooks
|-- orchestrator/    intake loop, dispatch, running map, launch, monitor
|-- daemon/          detach, signals, lifecycle, state file
|-- context/         issue intake data model
`-- utils/           shared path helpers
```

Layout rules:

- Multi-file modules use `<name>/mod.rs`.
- Single-file leaves may stay as `<name>.rs`.
- Vik-owned path derivation belongs in `src/workspace/`.
- Platform detach and signal code belongs under `src/daemon/{detach,signals}/`.

## Layer Map

```mermaid
graph TD
    main[main.rs] --> cli[cli]
    cli --> workflow[workflow]
    cli --> daemon[daemon]
    cli --> logging[logging]
    cli --> orchestrator[orchestrator]
    cli --> workspace[workspace]

    workflow --> config[config]
    workflow --> hooks[hooks]
    workflow --> workspace
    workflow --> utils[utils]

    orchestrator --> workflow
    orchestrator --> session[session]
    orchestrator --> hooks
    orchestrator --> context[context]
    orchestrator --> logging

    session --> agent[agent]
    session --> config
    session --> context
    session --> shell[shell]
    session --> template[template]
    session --> workflow

    hooks --> config
    hooks --> context
    hooks --> logging
    hooks --> shell
    hooks --> template

    agent --> config
    template --> shell
    daemon --> logging
```

Important current boundaries:

- `orchestrator` does not import `agent` or `shell`.
- `agent` adapters do not spawn subprocesses directly.
- `SessionFactory` is the orchestrator-to-session spawn seam.
- `Workflow` is the path/config carrier passed into runtime layers.
- `Workspace` accessors produce logs, sessions, service, and issue paths.

## Startup

```mermaid
sequenceDiagram
    participant Op as Operator
    participant CLI as cli::run
    participant Loader as WorkflowSchemaLoader
    participant Wf as Workflow
    participant Log as logging::init
    participant D as daemon
    participant O as Orchestrator

    Op->>CLI: vik run [-d] [workflow.yml]
    CLI->>Loader: load(workflow path)
    Loader-->>CLI: LoadedWorkflowSchema
    CLI->>Wf: Workflow::try_from(loaded)
    CLI->>Wf: workspace.ensure_root()
    opt detached
      CLI->>D: detach(log_dir)
      D-->>CLI: parent exits; child continues
    end
    CLI->>Log: init(workspace.logs_dir)
    CLI->>D: install_shutdown_handler()
    CLI->>D: write state.json
    CLI->>O: Orchestrator::new(workflow).run(shutdown)
```

`--port` resolves a socket address, but the server path is `todo!` today.

## Orchestrator Runtime

`Orchestrator::run` starts one `IntakeLoop` task and then selects over:

- shutdown token
- orchestrator event channel

Main loop owns `RunningMap`. Other tasks send events:

- `IntakeEvent::Issue`
- `IntakeEvent::Failed`
- `IntakeEvent::Stopped`
- `StageEvent::IssueReady`
- `StageEvent::Started`
- `StageEvent::Snapshot`
- `StageEvent::Terminal`
- `StageEvent::Failed`

Dispatch flow:

1. Intake emits an issue.
2. Orchestrator matches stages by exact `state`.
3. Orchestrator reserves `(issue_id, stage_name)`.
4. Issue setup task creates
   `<workflow-workspace-root>/issues/<issue_id>/`.
5. Issue setup runs `after_create`.
6. Launcher task runs `before_run`.
7. Launcher spawns a `Session`.
8. Monitor task sends snapshots and terminal event.
9. Launcher runs `after_run` after terminal state, except cancellation.

## Intake

`IntakeLoop` runs `issues.pull.command` from the workflow file directory, waits
for command completion, and parses stdout as `Issues(Vec<Issue>)` JSON.

The sleep between intake cycles is `issues.pull.idle_sec`.

## Session

`SessionFactory` holds `Arc<Workflow>`.

Session spawn:

1. Resolve the stage prompt path through `Workflow::resolve_path`.
2. Build prompt context.
3. Render MiniJinja.
4. Expand prompt commands with ``!`exec(command)` ``.
5. Pick adapter with `agent::get_adapter(profile.runtime)`.
6. Build provider command.
7. Spawn the child process.
8. Stream stdout lines into adapter event mapping.
9. Write decoded `AgentEvent` JSONL.
10. Update `SessionSnapshot`.

Session logs live at:

```text
<workflow-workspace-root>/sessions/<issue.id>/<issue.state>-<uuid-v7>.jsonl
```

Current code uses a hardcoded one-hour child timeout. There is no stall
watchdog config in workflow schema.

## Agents

`AgentAdapter` has two methods:

- `build_command(&self, profile, prompt) -> AgentCommand`
- `map_event(&self, value) -> Vec<AgentEvent>`

`get_adapter(runtime)` returns a stateless adapter:

- `CodexAdapter`
- `ClaudeCodeAdapter`

`AgentProfileSchema.args` is forwarded into provider CLI flags before fixed
provider flags.

## Template Context

`Context::new()` captures process env under `env`.

Stage context applies:

- `cwd`
- `workspace.root`
- `issue`
- `stage`

Stage prompt rendering then adds:

- `workflow`
- `loop`
- `profile`
- extra issue payload fields

`after_create` hook context is intentionally smaller: `issue` and `env`.

## Hooks

`HookRunner` owns:

- `schedule_after_create`
- `schedule_before_run`
- `schedule_after_run`
- fire-and-wait wrappers for each hook

Hook execution:

1. Render with strict MiniJinja.
2. Wait for the one-shot trigger.
3. Run `sh -c` or `cmd /C` in the issue workspace.
4. Apply a hardcoded 30-second timeout.
5. Return `HookOutcome`.

Hooks do not run prompt-command expansion.

## Workspace And State

The YAML `workspace.root` names a workspace home. `Workflow` resolves it against
the workflow file directory when relative, then appends
`workflows/<workflow-path-key>/`. `<workflow-path-key>` is the absolute workflow
file path with `/` replaced by `-`. `Workspace::ensure_root()` creates only
that final workflow-scoped root, so its parent must already exist.

`Workspace` memoizes these path helpers:

- `root()`
- `logs_dir()`
- `sessions_dir()`
- `service_dir()`
- `service_state_file()`
- `issues_dir()`
- `issue_workdir(identifier)`
- `issue_sessions_dir(identifier)`

Runtime artifacts:

| path                                                       | owner    | purpose                   |
| ---------------------------------------------------------- | -------- | ------------------------- |
| `<root>/service/state.json`                                | daemon   | pid and lifecycle state   |
| `<root>/logs/vik.log.YYYY-MM-DD`                           | logging  | INFO+ log events          |
| `<root>/logs/vik-error.log.YYYY-MM-DD`                     | logging  | ERROR-only log events     |
| `<root>/sessions/<identifier>/<state>-<uuid-v7>.jsonl`     | session  | decoded AgentEvent stream |
| `<root>/issues/<identifier>/`                              | operator | issue workspace           |

## Daemon

Daemon modules:

- `detach/`: Unix detach and Windows unsupported stub.
- `signals/`: SIGINT/SIGTERM/SIGHUP handling and pid liveness helpers.
- `state.rs`: atomic state JSON read/write/remove.
- `lifecycle.rs`: status, stop, restart stop phase, uninstall.

`stop` sends SIGTERM and waits up to 30 seconds for the daemon pid to exit.
The daemon itself cancels intake and running sessions when the shutdown token
trips.

## Where New Behavior Lands

| change                          | destination                                      |
| ------------------------------- | ------------------------------------------------ |
| new workflow field              | `src/config/`, then `Workflow` accessor if used  |
| new CLI subcommand              | `src/cli/<name>.rs` and `src/cli/mod.rs`         |
| new Vik-owned path              | `src/workspace/mod.rs`                           |
| new agent provider              | `src/agent/adapters/<provider>/` and `get_adapter` |
| new prompt binding              | `src/template/context.rs` or session render code |
| new hook trigger point          | `src/hooks/` plus orchestrator or launcher call site |
| HTTP API implementation         | new server module plus CLI `drive_runtime`       |

## Related Documents

- [`CONTEXT.md`](../../CONTEXT.md)
- [`PRD.md`](../PRD.md)
- [`code-conventions.md`](./code-conventions.md)
- [`review-checklist.md`](./review-checklist.md)
- [ADRs](../adr/)
