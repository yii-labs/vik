# Codex Agent

Use Codex when Vik should run Codex app-server sessions inside issue
workspaces.

## Config

Minimal shape:

```yaml
agent:
  default: codex
codex:
  command: codex app-server
```

Route issues tagged for Codex while keeping Codex as fallback for untagged
issues:

```yaml
agent:
  default: codex
codex:
  filter:
    tags: [codex]
  command: codex --config shell_environment_policy.inherit=all app-server
  model: gpt-5.5
  model_reasoning_effort: xhigh
```

`codex.filter.tags` participates in agent selection only. Use
`tracker.filter` for coarse project delegation before agent selection.

## Fields

- `command`: Codex app-server command. Default: `codex app-server`.
- `filter.tags`: Linear label names that route matching issues to Codex.
- `model`: optional Codex model injected into the CLI command.
- `model_reasoning_effort`: optional reasoning effort injected into the CLI
  command.
- `approval_policy`, `approvals_reviewer`, `thread_sandbox`,
  `turn_sandbox_policy`: app-server protocol settings.
- `turn_timeout_ms`, `read_timeout_ms`, `stall_timeout_ms`: Codex runtime
  timeout settings.

When `model` or `model_reasoning_effort` is set, `command` must contain the
`app-server` token so Vik can inject CLI config before it.

## Setup

1. Check CLI availability:

   ```sh
   codex --version
   codex app-server --help
   ```

2. Check auth:

   ```sh
   codex login status
   ```

3. If auth is missing and a browser is available, run:

   ```sh
   codex login
   codex login status
   ```

4. If browser auth is unavailable and `OPENAI_API_KEY` is already exported,
   authenticate without printing the key:

   ```sh
   printenv OPENAI_API_KEY | codex login --with-api-key
   codex login status
   ```

5. Stop with a Codex auth blocker if neither browser auth nor an API key is
   available.

## Validate

```sh
LINEAR_API_KEY=ci-placeholder cargo run --locked -p vik-cli -- check ./WORKFLOW.md
```

Official references:

- <https://developers.openai.com/codex/cli/reference>
- <https://developers.openai.com/codex/config-basic>
- <https://developers.openai.com/codex/config-advanced>
