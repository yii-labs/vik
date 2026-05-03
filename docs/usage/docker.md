# Docker

Use Docker when the host should not install Rust, Codex, GitHub CLI, and system
packages directly. The image includes `vik`, `gh`, `codex`, `git`, and
`openssh-client`.

## Build

```sh
docker build -t vik:local .
```

## Workspace Mount

Mount one directory that contains `WORKFLOW.md`. Keep `workspace.root` inside
that mount so issue clones, `.vik` state, and logs survive container restarts.

```text
/vik-workspace
  .vik/
  WORKFLOW.md
  VIK-1/
  VIK-2/
```

For the standard mount, set:

```yaml
workspace:
  root: /vik-workspace
```

## Config Check

Pass required secrets as environment variables. Docker does not copy local
config files unless you mount them.

```sh
docker run --rm \
  --env LINEAR_API_KEY \
  --env GH_TOKEN \
  --env OPENAI_API_KEY \
  -v "$PWD:/vik-workspace" \
  vik:local vik check
```

## Run Daemon

```sh
docker run --rm \
  --env LINEAR_API_KEY \
  --env GH_TOKEN \
  --env OPENAI_API_KEY \
  -v "$PWD:/vik-workspace" \
  vik:local
```

The default command is:

```sh
vik start /vik-workspace/WORKFLOW.md
```

Set `VIK_WORKFLOW_PATH` when the workflow file is mounted somewhere else.

## Observation Port

The Vik HTTP server binds to `127.0.0.1` by default. Bind to all container
interfaces when publishing a Docker port:

```sh
docker run --rm \
  --env LINEAR_API_KEY \
  --env GH_TOKEN \
  --env OPENAI_API_KEY \
  -p 3000:3000 \
  -v "$PWD:/vik-workspace" \
  vik:local --bind-address 0.0.0.0 --port 3000
```

## GitHub Auth

Token auth is simplest in Docker:

```sh
docker run --rm \
  --env GH_TOKEN \
  --env LINEAR_API_KEY \
  --env OPENAI_API_KEY \
  -v "$PWD:/vik-workspace" \
  vik:local vik check
```

If `WORKFLOW.md` uses an SSH `repo.origin`, also mount SSH credentials or
change the origin to HTTPS:

```yaml
repo:
  origin: https://github.com/yii-labs/vik
  clone:
    depth: 1
```

## User And Permissions

The runtime uses the base image `node` user. If the host mount has a different
owner, pass a matching user:

```sh
docker run --rm --user "$(id -u):$(id -g)" \
  --env LINEAR_API_KEY \
  --env GH_TOKEN \
  --env OPENAI_API_KEY \
  -v "$PWD:/vik-workspace" \
  vik:local vik check
```

`/home/vik`, `CODEX_HOME`, `GH_CONFIG_DIR`, and `/vik-workspace` are writable in
the image.
