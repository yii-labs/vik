# Configuration

Vik reads one YAML workflow file. Default path: `./workflow.yml`.

Relative paths resolve from the workflow file directory. `~` expands to the
home directory. Workflow string values do not expand `$VAR` or `${VAR}`.

## Basic

Minimal shape:

```yaml
loop: {}

workspace:
  root: ~/code/vik-workspaces

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

Current `doctor` checks YAML load and schema diagnostics. It does not check
prompt file existence, CLI binaries, auth, or external tracker access.

## Loop

`loop` is required. Its fields are optional.

- `max_issue_concurrency`: maximum active issue identifiers. Default: `10`.
- `wait_ms`: parsed today, but current intake scheduling uses
  `issues.pull.idle_sec`.
- `max_iterations`: optional intake loop cap. Omitted means run until shutdown.

The orchestrator does not wait for all stages to finish before future intake
cycles. If issue capacity is available, later intake results can dispatch more
work.

## Workspace

`workspace.root` is optional. It names the workspace home. Relative values
resolve from the workflow file directory. If omitted or null, Vik uses
`VIK_HOME` when set; otherwise it uses the OS home directory.

Vik creates a workflow-scoped workspace root under that home:

```text
<workspace.root>/workflows/<workflow-path-key>/
```

`<workflow-path-key>` is the absolute workflow file path with `/` replaced by
`-`. This keeps workflows from colliding when they share one workspace home.
Before `vik run`, create the parent directory `<workspace.root>/workflows`.
Vik creates only the final workflow-scoped workspace root.

All runtime paths below use the workflow-scoped workspace root:

```text
<workflow-workspace-root>/service/state.json
<workflow-workspace-root>/logs/
<workflow-workspace-root>/sessions/
<workflow-workspace-root>/issues/<issue.id>/
```

The issue identifier is used as a path segment. Pull commands must return safe
identifiers. Do not return identifiers that start with `.` or contain path
separators.

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

Optional fields are preserved in `issue.extra_payload` and flattened into stage
prompt context before canonical stage bindings are applied.

Duplicate identifiers in one intake result are skipped after the first one.

## Stages

`issue.stages` is an ordered map. Stage keys are user-defined names.

Each stage requires:

- `when.state`: exact issue state that triggers the stage.
- `agent`: agent profile name.
- `prompt_file`: prompt file for the stage.

Dispatch uses exact, case-sensitive state match:

```text
issue.state == issue.stages.<stage>.when.state
```

Multiple stages may match one issue state. Vik reserves and launches every
matching `(issue.id, stage.name)` while issue capacity allows it.

## Hooks

Issue hook:

- `issue.hooks.after_create`: runs after Vik creates or verifies the issue
  workspace and before any matched stage launches. It runs every matched cycle,
  even when the workspace already exists. Make it idempotent.

Stage hooks:

- `issue.stages.<stage>.hooks.before_run`: runs before the agent session.
- `issue.stages.<stage>.hooks.after_run`: runs after terminal session state,
  except cancelled sessions.

Hook shell bodies are MiniJinja-rendered and then executed with the shared shell
wrapper. Hooks do not support prompt command expansion.

Hook contexts:

- `after_create`: `issue`, `env`
- stage hooks: `cwd`, `workspace`, `issue`, `stage`, `env`

Hooks run with current directory set to the issue workspace.

## Prompt Files

Prompt files are MiniJinja templates. Unknown variables fail rendering.

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

- `cwd`: issue workspace path.
- `workspace.root`: workflow-scoped workspace root.
- `issue`
- `stage`
- `workflow`
- `loop`
- `profile`
- `env`

Current agent subprocesses inherit the Vik process cwd. If an agent must run
commands in the issue workspace, say so in the prompt, for example
`cd {{ cwd }}`.

## Observation

Current observation surfaces are logs, daemon state, and session JSONL files.
The CLI parses `--port` and `--bind-address`, but the HTTP server is not
implemented yet.

## References

- [Get Started](get-started.md)
- [Service Daemon](service-daemon.md)
- [Observation](observation.md)
