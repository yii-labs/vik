# Session Factory Indirection

The orchestrator never calls provider adapters directly. A `SessionFactory` in
the `session` module owns the spawn boundary for intake and stage sessions.

Current shape:

- `SessionFactory` holds `Arc<Workflow>`.
- `spawn` looks up the selected agent profile from `Workflow`.
- `Session` selects a stateless adapter with `agent::get_adapter(runtime)`.
- The adapter builds the provider command and maps provider JSONL to
  provider-agnostic `AgentEvent` values. Valid unmodeled provider lines stay
  visible as retained observation events.

We rejected direct adapter calls from orchestrator because provider command
shape, JSONL event mapping, session logging, and cancellation belong below the
session boundary. Dependency direction is:

```text
orchestrator -> session -> agent
```

Adding a new provider should change agent adapter code and config parsing, not
orchestrator dispatch code.
