# Context

## Agent runtime

An Agent runtime runs one tracker issue in one issue workspace. It owns the
details needed to prepare the run, stream runtime events, decide whether to
continue, and shut down runtime resources.

## Runtime selection

Runtime selection is workflow config under `agent.runtime`. The selected runtime
adapter is built inside `vik-agent`, so CLI and orchestration code depend on the
runtime seam instead of a concrete adapter.

## Codex

Codex is the first Agent runtime adapter. It runs Codex app-server inside the
issue workspace and keeps Codex-specific process, JSONL, dynamic tool, and
session-log details inside the `vik-agent` Codex module.
