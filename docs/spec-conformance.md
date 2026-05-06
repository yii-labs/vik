# Specification Conformance

This document maps Vik draft v1 required behavior to this workspace.

## Core

- Workflow path selection: `vik-workflow::select_workflow_path`.
- `WORKFLOW.md` YAML front matter parser: `vik-workflow::parse_workflow_content`.
- Typed config defaults and `$VAR` resolution: `vik-workflow::ServiceConfig`.
- Dynamic reload: `vik-workflow::WorkflowReloader`.
- Invalid workflow reload handling: reconciliation keeps last-good config, new dispatch/retry launch is
  blocked until reload succeeds.
- Polling orchestrator state authority: `vik-orchestrator::OrchestratorState`.
- Tracker candidate, terminal, state refresh, issue update, comment, attachment,
  and PR-link operations: `vik_core::IssueTracker` with
  `vik-tracker::TrackerClient`.
- Sanitized per-issue workspaces: `vik-workspace::WorkspaceManager`.
- Workspace hooks and timeout: `vik-workspace::WorkspaceManager`.
- Agent runtime seam: `vik-agent::AgentRuntime`.
- Agent runtime selection: `agent.runtime` in `vik-workflow::AgentConfig`.
- Codex runtime adapter: internal `vik-agent` Codex module.
- Tracker-agnostic Codex app-server `vik_issue` dynamic tool routes through the
  configured `vik_core::IssueTracker`.
- Strict prompt rendering: `vik-workflow::render_prompt`.
- Retry queue and backoff: `vik-orchestrator::failure_backoff_ms`.
- Terminal/non-active reconciliation: `vik-orchestrator::Orchestrator`.
- Structured logs: `tracing` events include issue/session fields where available.
- Operator observability: JSON logs plus optional HTTP API in `vik-http`.

Implemented tracker providers:

- Linear.
- GitHub.

Not implemented:

- SSH worker extension.
- Durable retry/session persistence.
- Non-Codex runtimes.
- Trackers beyond Linear and GitHub.

## Production Validation

Run with real tracker and Codex credentials before production use:

```sh
cargo run -p vik-cli -- check ./WORKFLOW.md
cargo test --workspace
```

Then start the daemon against an isolated tracker target and workspace root.
