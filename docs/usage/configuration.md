# Configuration

Vik reads one YAML workflow file. Default path: `./workflow.yml`.

Relative paths resolve from the workflow file directory. `~` expands to the
home directory. Workflow string values do not expand `$VAR` or `${VAR}`.

## Basic

Minimal shape:

```yaml
agents:
  codex-medium:
    runtime: codex
    model: gpt-5.5
    args:
      --config:
        - model_reasoning_effort=medium
  claude-sonnet:
    runtime: claude_code
    model: claude-sonnet-4-6
    args:
      --permission-mode: acceptEdits

issues:
  pull:
    command: ./scripts/issues-json
    idle_sec: 5

issue:
  hooks:
    after_create: |
      git clone --depth 1 git@github.com:yii-labs/vik .
  stages:
    plan:
      when:
        state: plan
      agent: codex-medium
      prompt_file: ./.agents/prompts/plan.md
```

Validate:

```sh
vik doctor ./workflow.yml
vik doctor --strict ./workflow.yml
vik doctor --json ./workflow.yml
```

Current `doctor` checks YAML load and schema diagnostics. It rejects stages
with zero or multiple prompt sources. It does not check prompt file existence,
CLI binaries, auth, or external tracker access.

## Loop

`loop` is optional. If it is omitted, Vik uses the default loop values.
If `loop` is present, its fields are optional.

- `max_issue_concurrency`: maximum active issue ids. Default: `10`.
- `wait_ms`: parsed today, but current intake scheduling uses
  `issues.pull.idle_sec`. Default: `5000`.
- `max_iterations`: optional intake loop cap. Omitted means run until shutdown.

The orchestrator does not wait for all stages to finish before future intake
cycles. If issue capacity is available, later intake results can dispatch more
work.

## Workspace

`workspace` is optional. If it is omitted, Vik uses `.vik` as the workspace
home.

`workspace.root` names the workspace home. Relative values resolve from the
workflow file directory.

When the `workspace` section is present but empty (`workspace: {}`), or when
`workspace.root` is null, Vik chooses the workspace home from fallback state:
it uses a non-empty `VIK_HOME` directly when set; otherwise it uses the user
home `.vik` directory.

After choosing the workspace home, Vik creates a workflow-scoped workspace root
under that home:

```text
<workspace.root>/workflows/<workflow-path-key>/
```

`<workflow-path-key>` is the absolute workflow file path with `/` replaced by
`-`. This keeps workflows from colliding when they share one workspace home.
`vik run` creates the full workflow-scoped workspace root if it is missing.

All runtime paths below use the workflow-scoped workspace root:

```text
<workflow-workspace-root>/service/state.json
<workflow-workspace-root>/logs/
<workflow-workspace-root>/sessions/
<workflow-workspace-root>/issues/<issue.id>/
```

The issue id is used as a path segment. Pull commands must return safe issue
ids. Do not return issue ids that start with `.` or contain path separators.

## Agents

`agents` is a map of named profiles.

Required fields:

- `runtime`: `codex` or `claude_code`.
- `model`: model name passed to the provider CLI.

Optional field:

- `args`: runtime-specific CLI flag map.

`args` forwarding rules:

- String or number: `flag value`
- `true`: `flag`
- `false`: omitted
- Sequence of strings: `flag item1,item2`
- Other YAML value types are ignored by argument forwarding

Codex command shape:

```text
codex exec <args...> --json -m <model>
```

Codex receives the rendered prompt on stdin.

Claude Code command shape:

```text
claude --verbose --output-format stream-json --model <model> -p <prompt> <args...>
```

Current session code uses a hardcoded one-hour child timeout. Workflow profile
timeouts and stall-watchdog config are not implemented.

## Issues

`issues.pull.command` is a shell command that fetches and filters issues from an
external tracker. Vik runs it from the workflow file directory and reads stdout.
The command must output one raw JSON sequence:

```json
[
  {
    "id": "123",
    "title": "Add retry tests",
    "state": "plan"
  }
]
```

`issues.pull.idle_sec` controls the sleep after each pull cycle completes.
Default: `5`.

Required issue fields:

- `id` or `identifier`: non-empty safe path segment.
- `title`: string.
- `state`: string. Alias: `status`.

Optional issue fields are preserved under `issue` in the prompt and hook
context. Avoid extra field names that collide with Vik issue bindings such as
`id`, `title`, `description`, `state`, `workdir`, or `stage`. In stage prompt
and hook contexts, `issue.stage` is Vik-owned stage name.

Duplicate issue ids in one intake result are skipped after the first one.

## Stages

`issue.stages` is an ordered map. Stage keys are user-defined names. Vik keeps
that YAML shape and duplicates each key into the stage value as `stage.name`.

Each stage requires:

- `when.state`: exact issue state that triggers the stage.
- `agent`: agent profile name.
- exactly one prompt source:
  - `prompt_file`: prompt file for the stage.
  - `prompt`: inline prompt text for the stage.

Example with a prompt file:

```yaml
issue:
  stages:
    plan:
      when:
        state: plan
      agent: codex-medium
      prompt_file: ./.agents/prompts/plan.md
```

Example with inline text:

```yaml
issue:
  stages:
    plan:
      when:
        state: plan
      agent: codex-medium
      prompt: |
        Work on issue {{ issue.id }}: {{ issue.title }}.
```

`prompt_file` and `prompt` are mutually exclusive. A stage with both or neither
is invalid. `prompt_file` paths resolve from the workflow file directory.

Dispatch uses exact, case-sensitive state match:

```text
issue.state == issue.stages.<stage>.when.state
```

Multiple stages may match one issue state. Vik reserves and launches every
matching `(issue.id, stage.name)` while issue capacity allows it.

## Hooks

Issue hook:

- `issue.hooks.after_create`: runs after Vik first creates the issue workspace
  and before any matched stage launches for that setup. Existing issue
  workspaces skip it.

Stage hooks:

- `issue.stages.<stage>.hooks.before_run`: runs before the agent session.
- `issue.stages.<stage>.hooks.after_run`: runs after terminal session state,
  except cancelled sessions.

Hook shell bodies are MiniJinja-rendered and then executed with the shared shell
wrapper. Hooks do not support prompt command expansion.

Hook contexts:

- `after_create`: `issue`, `workspace_root`, `workflow_path`, and `env`.
- stage hooks: same context as `after_create`, plus `issue.stage`.

`issue` contains `id`, `title`, `description`, `state`, `workdir`, and optional
extra issue fields. Stage hook and prompt contexts also contain `issue.stage`
for the current stage. Vik does not add root-level `stage`.

Hooks run with current directory set to the issue workspace.

## Prompt Sources

Prompt sources are MiniJinja templates. Unknown variables fail rendering. For
`prompt_file`, Vik reads the file first. For `prompt`, Vik uses the inline text
directly. Both forms share the same context and command expansion.

Prompt render order:

1. MiniJinja render.
2. Prompt-command expansion.

Prompt command syntax:

```text
!`exec(gh issue view {{ issue.id }} --json title,body)`
```

The non-bang form also works:

```text
`exec(printf ready)`
```

Prompt commands run through the system shell with a 30-second timeout. They
inject stdout text and trim one trailing newline. Non-zero exit fails prompt
rendering.

Stage prompt context includes:

- `issue.id`, `issue.title`, `issue.description`, and `issue.state`.
- `issue.stage`: current stage name.
- `issue.workdir`: issue workspace path.
- optional extra issue fields returned by `issues.pull.command` as
  `issue.<field>`.
- `workspace_root`: workflow-scoped workspace root.
- `workflow_path`: absolute workflow file path.
- `env`

Current agent subprocesses run with current directory set to the issue
workspace. `issue.workdir` exists for prompts that need to print or pass the
path. Current code does not expose root-level `stage`, `workflow`, `loop`, or
`profile` template objects.

## Observation

Current observation surfaces are logs, daemon state, and session JSONL files.
The CLI parses `--port` and `--bind-address`, but the HTTP server is not
implemented yet.

## References

- [Get Started](get-started.md)
- [Service Daemon](service-daemon.md)
- [Observation](observation.md)
