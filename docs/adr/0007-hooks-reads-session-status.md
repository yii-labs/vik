# Hooks Stay Outside Session State

This ADR replaces the older `SessionStatus` wording. Current code no longer has
`SessionStatus` or a hook helper that imports it.

Decision: hooks stay outside session internals. `StageSessionManager` owns the
sequence:

1. Run stage `before_run`.
2. Spawn the session task, which holds the runtime Issue Stage context.
3. Run stage `after_run` when the session reaches terminal state, except when
   the session was cancelled.

The session module owns the runtime Issue Stage context for prompt rendering and
exposes state changes plus one-time snapshot commands. It does not know hook
types. The hooks module renders shell bodies and reports hook outcomes. It does
not own session lifecycle.

This keeps the coupling direction clear:

```text
orchestrator -> hooks
orchestrator -> session
```

There is no reverse edge from `session` to `hooks`.
