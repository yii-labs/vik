# Vik

Vik is a Rust implementation of [Symphony](https://github.com/openai/symphony),
adapted for the draft Vik service specification.

Vik is a long-running daemon that reads Linear issues, creates one isolated workspace per
issue, and runs Codex app-server sessions inside those workspaces.

## Crates

- `vik-core`: shared domain model and traits.
- `vik-workflow`: `WORKFLOW.md` parsing, typed config, dynamic reload, strict prompt rendering.
- `vik-tracker`: Linear GraphQL read adapter.
- `vik-workspace`: workspace path safety and hook execution.
- `vik-agent`: Codex app-server JSONL client and local agent worker.
- `vik-orchestrator`: polling, dispatch, retry, reconciliation, metrics.
- `vik-http`: optional observability HTTP API.
- `vik-cli`: `vik` binary.

## Run

If `.env` exists in the current directory or a parent directory, `vik` loads it before reading
`WORKFLOW.md`. Variables already set in the shell are preserved.

Create or edit `WORKFLOW.md`, then:

```sh
cargo run -p vik-cli -- ./WORKFLOW.md
```

Validate config only:

```sh
cargo run -p vik-cli -- ./WORKFLOW.md --check
```

Enable the optional HTTP status server:

```sh
cargo run -p vik-cli -- ./WORKFLOW.md --port 3000
```

Daemon logs are JSON lines on stdout and in a daily file under `logging.dir`. If
`logging.dir` is omitted, Vik writes to `<workspace.root>/.vik/logs/vik.log.<date>`.

## Workflow Templates

- `WORKFLOW.md` is the single default workflow. It keeps the upstream OpenAI Elixir workflow text
  and adds the Vik customized front matter configurations.

## Implementation-Defined Policy

This implementation targets trusted local automation environments.

- Codex app-server launches in the per-issue workspace from `codex.command`, with `codex.model`
  and `codex.model_reasoning_effort` inserted as CLI `--config` args before `app-server`.
  macOS/Linux keep POSIX shell execution; Windows uses direct executable plus args launch.
- The default workflow routes approval review to Codex `auto_review`, so connector write prompts do
  not wait for an interactive user.
- Command and file-change approvals are answered with session acceptance.
- Permission requests grant the requested permission subset for the current session.
- The default workflow uses Codex `externalSandbox` turns with network access enabled, so trusted
  local automation can write git metadata and publish branches.
- `workspaceWrite` turn sandbox policy still includes the per-issue workspace as a writable root
  and enables network access unless explicitly configured otherwise.
- The `linear_graphql` dynamic tool is exposed to Codex sessions when Linear tracker credentials
  are configured.
- User-input and elicitation requests return protocol errors, so runs do not wait forever.
- Unsupported dynamic tool calls return structured failure output and do not stall the session.
- Hooks are trusted `WORKFLOW.md` shell scripts and run inside the workspace through the host
  platform shell.

## Safety

The implementation enforces these filesystem invariants:

- Workspace names are sanitized to `[A-Za-z0-9._-]`, replacing all other characters with `_`.
- Workspace paths must stay under `workspace.root`.
- Codex app-server cwd must equal the per-issue workspace path.
- Terminal issue cleanup removes only the matching sanitized workspace directory.

Use stronger host-level isolation, narrower credentials, and stricter Codex approval/sandbox settings
for untrusted issue content or repositories.
