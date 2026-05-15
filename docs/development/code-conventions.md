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
- When stable options would otherwise be passed to every call, introduce a
  reusable type that captures them once.
- Install dependencies with `cargo add <crate>` when changing dependencies.
- Use `<name>/mod.rs` for modules with submodules.
- Do not duplicate cross-cutting concerns across modules. If a primitive already
  has an owner, reuse the owner's public surface.
- Adapter modules own provider command assembly and event mapping. Session owns
  spawn, prompt rendering, event streaming, and JSONL writing.
- A spawned `Session` holds the runtime `IssueStage` it is executing. Stage
  hooks stay in the orchestrator launcher, outside session internals.
- Runtime path derivations for Vik-owned state go through `Workspace` in
  `src/workspace/`. Do not hand-join logs, sessions, service, or issue
  workspace paths in production code outside that module.
- Runtime layers should receive `Workflow` or `Arc<Workflow>` when they need
  schema, hooks, or workspace paths.
- The parsed YAML type is `WorkflowSchema`. The runtime supervisor is
  `Workflow`.
- Runtime issue behavior belongs on `IssueRun`; keep `Issue` as plain intake
  data.
- `IssueRun` may construct matching runtime `IssueStage` values from workflow
  stage order and `issue.state`; orchestrator-owned capacity checks stay in
  `RunningMap`.
- `IssueStage` may project canonical template context for hooks and prompts,
  but stage lifecycle stays outside `IssueStage`.
- Orchestrator should handle issue preparation failures through `IssueRunError`
  instead of importing hook internals for issue setup.

## Current Dependency Direction

- `main` calls `cli`.
- `cli` may compose every runtime layer.
- `workflow` owns schema plus resolved workspace and hook runner.
- `orchestrator` depends on `workflow`, `session`, `hooks`, `context`, and
  `logging`.
- `context` owns issue intake data plus issue-run runtime context and may depend
  on `workflow`, `hooks`, and `template`.
- `session` depends on `agent`, `template`, `shell`, `workflow`, `config`, and
  `context`.
- `agent` depends on `config` for profile and runtime tags.
- `hooks` depends on `template` and `shell`.
- `template` depends on `shell` for prompt command execution.
- `daemon` owns detach, signal, lifecycle, and state-file mechanics.

The important review rule is not "lower modules never import higher modules" in
the abstract. The rule is: do not skip the intended seam. Examples:

- Orchestrator should not call agent adapters directly.
- Agent adapters should not create process trees directly.
- Runtime code should not build workspace artifact paths outside `Workspace`.
- Provider-specific JSON mapping should not leak into orchestrator.

## SOLID And Abstractions

- **Single responsibility.** A type represents one concept. `Workflow` is the
  supervisor, `WorkflowSchema` is parsed YAML, `Workspace` is path layout,
  `SessionFactory` is the spawn seam.
- **Open extension through narrow traits.** `AgentAdapter` is the provider
  extension point. Keep it small.
- **Interface segregation.** Split traits when callers only need a subset.
- **Dependency inversion at seams.** High-level orchestration should depend on
  session-level behavior, not provider-specific behavior.
- **Composition over forwarding traits.** Prefer wrappers with clear ownership
  over traits that redeclare another trait's whole surface.

## Docs

- Use kebab-case file names.
- Keep usage topics under `docs/usage/`.
- Keep development docs under `docs/development/`.
- Include why, where, and exact command steps for connection docs.
- Keep docs aligned with current code. If a feature is planned but not
  implemented, say so.
- Do not commit non-English text.

## Updating

Agents are responsible for keeping these conventions current when a refactor
changes architecture or workflow behavior.
