# Review Checklist

Every implementation review must run through this list in addition to the
issue's acceptance criteria.

## 1. Module Boundaries

Check current seams:

```sh
rg -n "crate::agent" src/orchestrator src/template src/workspace src/logging
rg -n "crate::shell" src/orchestrator src/config src/workflow src/logging src/workspace
rg -n "crate::orchestrator" src/agent src/session src/hooks src/template src/workflow src/workspace src/daemon
```

Expected result: no hits.

Allowed current edges:

- `session -> agent`
- `session -> shell`
- `session -> template`
- `agent -> config`
- `template -> shell`
- `hooks -> shell`

## 2. Workspace Path Ownership

Vik-owned state paths belong in `src/workspace/`.

```sh
rg -n 'join\("(logs|sessions|service|issues|state\.json)"\)' src --glob '*.rs'
```

Production hits should stay in `src/workspace/`. Test-only assertions in other
modules are acceptable when they verify public behavior.

## 3. Provider Boundaries

Agent adapters should only build commands and map JSON events.

Review for:

- no provider-specific parsing in orchestrator
- no provider-specific command construction in session
- no direct adapter calls from orchestrator

## 4. Prompt And Hook Boundaries

- Prompt files may use ``!`exec(command)` ``.
- Hooks do not run prompt-command expansion.
- Hook and prompt context keeps issue data under `issue`, with `issue.workdir`
  available for the issue workspace path.
- Root template bindings remain limited to `issue`, `workspace_root`,
  `workflow_path`, and `env`.
- Template objects such as `stage`, `workflow`, `loop`, `profile`, and
  `workspace` are not available unless current code adds them.

## 5. Error Surface

- Use typed errors with `thiserror` at module boundaries.
- Use `anyhow` only in CLI glue where context needs to stack for operator
  output.
- Do not hide validation failures. Return or log the exact error.

## 6. Test Scaffolding

- Keep fakes local and `#[cfg(test)]`.
- Use `tempfile::TempDir` for filesystem tests.
- Avoid process-id plus timestamp tempdir patterns.
- Timing-sensitive tests must stay cheap. Parameterize timeouts when possible.

## 7. CLI And Docs

Verify command examples use current workflow path shape:

```text
vik <command> [WORKFLOW]
```

Reject stale examples like:

```text
vik [WORKFLOW] <command>
```

HTTP endpoint docs must say planned unless the server implementation exists.

## 8. American English

Grep changed files for common British spellings:

```sh
rg -n "canonicali[s]ed|canonicali[s]e|seriali[s]ed|seriali[s]e|behavio[u]r|colo[u]r|defen[c]e|organi[s]ation|cent[r]e|synthesi[s]ing|authori[s]ed" <changed-files>
```

Zero hits required.

## 9. Non-English Text

Everything committed to git must be English. Check changed docs, source,
configs, prompts, commit text, and generated workflow text.

## 10. Architecture Audit Output

Review reports must include:

```text
## Architecture audit
- PASS/FAIL: module boundaries
- PASS/FAIL: workspace path ownership
- PASS/FAIL: provider boundaries
- PASS/FAIL: prompt and hook boundaries
- PASS/FAIL: error surface
- PASS/FAIL: test scaffolding
- PASS/FAIL: CLI and docs
- PASS/FAIL: American English
- PASS/FAIL: non-English text
```

If no old pattern came back, say: `No architectural regressions detected.`
