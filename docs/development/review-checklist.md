# Review Checklist

Use this for implementation review. Check acceptance criteria first, then these
gates.

## Module Boundaries

Run:

```sh
rg -n "crate::agent" src/orchestrator src/template src/workspace src/logging
rg -n "crate::shell" src/orchestrator src/config src/workflow src/logging src/workspace
rg -n "crate::orchestrator" src/agent src/session src/hooks src/template src/workflow src/workspace src/daemon
```

Expected result: no hits.

Allowed edges:

- `session -> agent`
- `session -> shell`
- `session -> template`
- `agent -> config`
- `template -> shell`
- `hooks -> shell`

## Workspace Paths

Run:

```sh
rg -n 'join\("(logs|sessions|service|issues|state\.json)"\)' src --glob '*.rs'
```

Production hits stay in `src/workspace/`. Test-only assertions elsewhere are
allowed.

## Provider Boundary

- Adapter builds provider commands.
- Adapter maps provider JSON events.
- Session spawns providers.
- Orchestrator does not know provider JSON or commands.

## Prompt And Hook Boundary

- Prompt files may use ``!`exec(command)` ``.
- Hooks do not run prompt-command expansion.
- Root template bindings stay `issue`, `workspace_root`, `workflow_path`, and
  `env`.
- Extra template objects are absent unless current code adds them.

## Error Surface

- Use typed errors at module boundaries.
- Use `anyhow` only in CLI glue.
- Return or log exact validation failures.

## Tests

- Keep fakes local and `#[cfg(test)]`.
- Use `Workflow::builder()` for runtime workflow tests.
- Use parser fixtures only when parser behavior is under test.
- Use `TempDir` only for filesystem behavior.
- Keep timing tests cheap and parameterized.

## CLI And Docs

- Current shape is `vik <command> [WORKFLOW]`.
- Reject stale `vik [WORKFLOW] <command>` examples.
- HTTP docs say planned unless server code exists.

## Language

- Use American English.
- Commit English text only.
- Grep changed files for non-English text.

## Report Shape

```text
## Architecture audit
- PASS/FAIL: module boundaries
- PASS/FAIL: workspace path ownership
- PASS/FAIL: provider boundaries
- PASS/FAIL: prompt and hook boundaries
- PASS/FAIL: error surface
- PASS/FAIL: tests
- PASS/FAIL: CLI and docs
- PASS/FAIL: language
```

If no old pattern came back, say: `No architectural regressions detected.`
