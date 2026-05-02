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

## Docker

Build the worker image:

```sh
docker build -t vik:local .
```

Run a config check with the workspace directory mounted:

```sh
docker run --rm \
  --env LINEAR_API_KEY \
  --env GH_TOKEN \
  --env OPENAI_API_KEY \
  -v "$PWD:/vik-workspace" \
  vik:local --check
```

Run the daemon:

```sh
docker run --rm \
  --env LINEAR_API_KEY \
  --env GH_TOKEN \
  --env OPENAI_API_KEY \
  -v "$PWD:/vik-workspace" \
  vik:local
```

The image includes `vik`, `gh`, `codex`, `git`, and `openssh-client`. The default command is
`vik /vik-workspace/WORKFLOW.md`. The mounted directory must contain `WORKFLOW.md`. For the
standard container layout, keep workflow state in that same mounted directory:

```text
/vik-workspace
  .vik/
  WORKFLOW.md
  <issue-clone-1>/
  <issue-clone-2>/
```

Set `workspace.root` to `.` or `/vik-workspace` in `WORKFLOW.md` so `.vik` state and issue clones
stay under the mount. Set `VIK_WORKFLOW_PATH` when mounting the workflow file elsewhere. Pass Vik
flags after the image name.

To expose the optional HTTP status server from Docker, publish the port and bind the server to the
container interface:

```sh
docker run --rm \
  --env LINEAR_API_KEY \
  --env GH_TOKEN \
  --env OPENAI_API_KEY \
  -p 3000:3000 \
  -v "$PWD:/vik-workspace" \
  vik:local --bind-address 0.0.0.0 --port 3000
```

The runtime uses the base image `node` user, which has UID/GID 1000 for common Linux bind mounts.
If the host workspace uses a different owner, pass a matching Docker `--user` value. The image keeps
`/home/vik` writable so GitHub CLI and Codex config directories still work with that override.

Pass environment variables explicitly with Docker `--env NAME` or `--env NAME=value`. Add every
GitHub CLI or Codex variable the workflow needs, including any `GH_*`, `GITHUB_*`, `CODEX_*`,
`OPENAI_*`, provider, proxy, or certificate variables. Docker then passes only those variables into
the container without copying local config files. `gh` and `codex` inherit the container
environment when Vik starts them. `LINEAR_API_KEY` must be passed this way unless the workflow file
provides `tracker.api_key`.

If workflow hooks use SSH remotes, pass SSH credentials separately or switch hooks to HTTPS with a
GitHub token. The minimal Docker path above only mounts one workspace directory containing
`WORKFLOW.md`; it does not copy local config files.

## Workflow Templates

- `WORKFLOW.md` is the single default workflow. It keeps the upstream OpenAI Elixir workflow text
  and adds the Vik customized front matter configurations.

## Implementation-Defined Policy

This implementation targets trusted local automation environments.

- Codex app-server launches in the per-issue workspace using a command derived from
  `codex.command`, with `codex.model` and `codex.model_reasoning_effort` converted into CLI
  `--config` args before `app-server`.
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
- Hooks are trusted `WORKFLOW.md` shell scripts and run inside the workspace.

## Safety

The implementation enforces these filesystem invariants:

- Workspace names are sanitized to `[A-Za-z0-9._-]`, replacing all other characters with `_`.
- Workspace paths must stay under `workspace.root`.
- Codex app-server cwd must equal the per-issue workspace path.
- Terminal issue cleanup removes only the matching sanitized workspace directory.

Use stronger host-level isolation, narrower credentials, and stricter Codex approval/sandbox settings
for untrusted issue content or repositories.
