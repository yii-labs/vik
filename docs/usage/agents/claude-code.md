# Claude Code Agent

Use Claude Code when Vik should run Claude Code headless mode inside issue
workspaces.

## Config

Route issues with the `claude` label to Claude Code while Codex remains the
fallback agent:

```yaml
agent:
  default: codex
codex:
  filter:
    tags: [codex]
claude-code:
  filter:
    tags: [claude]
  command: claude -p --output-format stream-json --input-format text --verbose
  model: sonnet
  permission_mode: acceptEdits
```

Make Claude Code the fallback agent:

```yaml
agent:
  default: claude-code
claude-code:
  command: claude -p --output-format stream-json --input-format text --verbose
```

`claude-code.filter.tags` participates in agent selection only. Use
`tracker.filter` for coarse project delegation before agent selection.

## Fields

- `command`: Claude Code headless command. Default:
  `claude -p --output-format stream-json --input-format text --verbose`.
- `filter.tags`: Linear label names that route matching issues to Claude Code.
- `model`: optional model value appended as `--model`.
- `permission_mode`: optional value appended as `--permission-mode`.
- `turn_timeout_ms`: timeout for the headless process. Default: `3600000`.

Vik writes the rendered issue prompt to the command stdin. Vik also appends
`--max-turns` from `agent.max_turns` so the Claude Code process has the same
turn budget as other adapters.

If the agent needs Linear or GitHub access through MCP, include the required
Claude Code MCP flags in `command` and validate them before daemon startup.

## Setup

1. Check CLI availability:

   ```sh
   claude --version
   claude -p --help
   ```

2. Confirm headless mode can read stdin:

   ```sh
   printf 'Print OK only.\n' | \
     claude -p --output-format stream-json --input-format text --max-turns 1
   ```

3. Confirm unattended permissions match the workflow:

   ```sh
   printf 'Inspect this repository and stop.\n' | \
     claude -p --output-format stream-json --input-format text \
       --permission-mode acceptEdits \
       --max-turns 1
   ```

4. Stop with a Claude Code auth or permission blocker if headless mode cannot
   complete without an interactive prompt.

## Validate

```sh
LINEAR_API_KEY=ci-placeholder cargo run --locked -p vik-cli -- check ./WORKFLOW.md
```

Official references:

- <https://docs.claude.com/en/docs/claude-code/cli-reference>
- <https://docs.claude.com/en/docs/claude-code/sdk/sdk-headless>
