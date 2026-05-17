# Context

## Language

**Workflow Definition**:
A project workflow policy stored as `workflow.yml`. It declares loop settings,
workspace root, agent profiles, issue intake, issue stages, prompt files, and
hooks. Parsed type: `WorkflowSchema`.
_Avoid_: `WORKFLOW.md`, workflow prompt, hidden tracker config

**Workflow**:
The runtime supervisor built once after loading the workflow file. It wraps the
parsed schema, resolved workflow path, resolved workspace paths, and hook runner.
Runtime layers receive `Workflow` or `Arc<Workflow>` instead of separate path
or config arguments.
_Avoid_: loose runtime config, sibling workspace_root args

**Workspace**:
The single source of truth for Vik-owned paths under `workspace.root`. It
derives `.vik/logs`, `.vik/sessions`, `.vik/service/state.json`, and
`<workspace.root>/<issue.id>/`.
_Avoid_: hand-joined `.vik` paths outside `src/workspace/`

**Agent Profile**:
A named agent CLI config selected by issue intake or a stage. Fields are
`runtime`, `model`, and optional `args`.
_Avoid_: global agent config, `params`

**Agent Runtime**:
The provider adapter chosen by `agents.<name>.runtime`. Current runtimes are
`codex` and `claude_code`. Adapters build provider command lines and map JSONL
events into Vik `AgentEvent` values.
_Avoid_: app-server turn model, multi-turn resume

**Issue Stage**:
A named workflow step under `issue.stages`. It matches a user-owned issue state
by exact string equality and selects one agent profile plus one prompt file.
_Avoid_: normalized status, built-in state machine

**Workflow State**:
An opaque string returned by intake as `issue.state` and matched against
`issue.stages.<stage>.when.state`.
_Avoid_: status normalization, Linear-only state

**Prompt Source**:
A prompt file referenced by `workflow.yml`. Vik renders MiniJinja first, then
expands source-local prompt commands with ``!`exec(command)` `` or
```exec(command)` ``.
_Avoid_: inline workflow prompt, generated command injection

**Issue Management Command**:
A user-authored command or instruction in prompt files. Vik does not inject a
tracker tool. Prompts must say how to read or update state, comments,
attachments, branches, pull requests, and links.
_Avoid_: `vik_issue`, dynamic tracker tool, hidden issue API

**Issue Run**:
Runtime handling of one Issue inside one Workflow. It carries workflow-derived
context for the issue, prepares the issue workspace, and creates spawnable Issue
Stages. The tracker Issue remains plain intake data.
_Avoid_: mutating Issue into runtime state, loose parameter bag

**Issue Workspace**:
Directory at `<workspace.root>/issues/<issue_id>/`. Issue-level and stage-level
hooks run there. Current agent subprocesses inherit the Vik process cwd, so
prompts should use the `cwd` template value or explicit `cd` commands when
workspace-local execution matters.
_Avoid_: stage workspace, sandbox

**Session**:
One execution of one issue stage. It owns a session state snapshot, decoded
`AgentEvent` JSONL file, token counts, rate-limit observations, and a cancel
handle for the child process.
_Avoid_: tracker issue, durable run state

**Session Factory**:
The `session` module entry point used by the orchestrator. It looks up the
selected agent profile from `Workflow`, creates a `Session`, and keeps
orchestrator code provider-agnostic.
_Avoid_: direct adapter calls from orchestrator

## Relationships

- One **Workflow Definition** defines many **Agent Profiles**.
- One **Workflow Definition** defines many **Issue Stages**.
- The **Workflow Definition** is the parsed SSOT; **Workflow** is the runtime
  context carrier built from it.
- Intake runs `issues.pull.command` from the workflow file directory.
- Intake command stdout must be one JSON sequence of issues.
- Each issue item uses `identifier` or `id`, `title`, and `state`; `desc` and
  `status` are accepted aliases for `description` and `state`.
- An **Issue Run** wraps one plain **Issue** with runtime context before any
  Issue Stage can spawn.
- A runtime **Issue Stage** belongs to one **Issue Run** and extends the Issue
  Run context with `issue.stage.name` when rendering hooks, rendering prompts,
  and spawning a Session.
- If tracker payload already has `stage`, stage context preserves it at
  `issue.stage.value`.
- `issue.state == issue.stages.<name>.when.state` is the only dispatch rule.
- Stage iteration preserves workflow author order.
- Orchestrator reserves `(issue.id, stage.name)` before async setup so
  duplicate intake results do not launch the same stage twice.
- `loop.max_issue_concurrency` limits active issue ids, not stage count.
- Running stage state lives in memory. Restart loses it. Next intake cycle
  rebuilds work from tracker state.
- `issue.hooks.after_create` runs after the Issue Workspace is first created
  and before stage launch for that setup. Existing Issue Workspaces skip it.
- Stage `before_run` runs before session spawn. Failure aborts that stage.
- Stage `after_run` runs after terminal session state except cancellation.
  Failure is logged.
- Prompt files see `env.<VAR>` from the Vik process environment.
- `after_create` hook templates see `issue`, `workspace_root`,
  `workflow_path`, and `env`.
- Stage hook and prompt templates also see `issue.stage.name`.
- Stage hook and prompt templates do not expose root-level `stage`, `workflow`,
  `loop`, or `profile`.
- Vik-owned state lives under `<workspace.root>/.vik/`.
- Issue workspaces live as siblings of `.vik/`.
- Vik does not load `.env` files. Operators provide environment variables
  before starting Vik.
