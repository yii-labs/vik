# Code Conventions

## Rust

- Keep public APIs narrow.
- Prefer typed config and structured errors over stringly typed plumbing.
- Keep async boundaries explicit.
- Do not block inside async tasks unless the code already isolates that work.
- Keep validation close to config parsing when invalid input should fail at
  startup.
- Use small helpers when they remove repeated invariants.
- Prefer receiver methods in `impl` blocks over free functions whose first
  argument is the receiver.

## Orchestration

- Treat the configured issue tracker as the tracker authority.
- Treat `WORKFLOW.md` as trusted local automation code.
- Keep workspace path safety intact.
- Do not add behavior that can delete paths outside `workspace.root`.
- Keep retry and terminal-state logic deterministic.
- Keep issue state transitions observable in logs or snapshots.

## Codex Integration

- Preserve `codex app-server` protocol compatibility.
- Do not log prompts, tool payloads, or credentials unless the surrounding code
  already treats the data as safe.
- Keep timeouts configurable.
- Prefer explicit lifecycle events for startup, session, turn, and failure
  boundaries.

## Docs

- Use kebab-case file names.
- Keep top-level usage topics as standalone files under `docs/usage/`.
- Keep development docs under `docs/development/`.
- Include why, where, and exact command steps for connection docs.
- Do not commit non-English text.
