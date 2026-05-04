# Specification Conformance

This document maps Vik draft v1 required behavior to this workspace.

## Core

- Workflow path selection: `vik-workflow::select_workflow_path`.
- `WORKFLOW.md` YAML front matter parser: `vik-workflow::parse_workflow_content`.
- Typed config defaults and `$VAR` resolution: `vik-workflow::ServiceConfig`.
- Top-level repo clone config: `vik-workflow::RepoConfig`.
- Dynamic reload: `vik-workflow::WorkflowReloader`.
- Invalid workflow reload handling: reconciliation keeps last-good config, new dispatch/retry launch is
  blocked until reload succeeds.
- Polling orchestrator state authority: `vik-orchestrator::OrchestratorState`.
- Linear candidate, terminal, and state refresh reads: `vik-tracker::LinearClient`.
- Sanitized per-issue workspaces: `vik-workspace::WorkspaceManager`.
- Configured per-issue repository clones before `after_create` hooks:
  `vik-workspace::WorkspaceManager`.
- Workspace hooks and timeout: `vik-workspace::WorkspaceManager`.
- Codex JSONL app-server client: `vik-agent::CodexAppServerClient`.
- `linear_graphql` client-side dynamic tool extension.
- Strict prompt rendering: `vik-workflow::render_prompt`.
- Retry queue and backoff: `vik-orchestrator::failure_backoff_ms`.
- Terminal/non-active reconciliation: `vik-orchestrator::Orchestrator`.
- Structured logs: `tracing` events include issue/session fields where available.
- Operator observability: JSON logs plus optional HTTP API in `vik-http`.

Not implemented:

- SSH worker extension.
- Durable retry/session persistence.
- Pluggable non-Linear trackers.

## Production Validation

Run with real Linear and Codex credentials before production use:

```sh
cargo run -p vik-cli -- check ./WORKFLOW.md
cargo test --workspace
```

Then start the daemon against an isolated Linear project and workspace root.
