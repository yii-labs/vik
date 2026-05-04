# Service Daemon

Use service commands when Vik should keep running after the terminal exits.
This is a local detached process manager, not a system-wide launchd or systemd
unit.

## Start And Register

Run from the directory that contains `WORKFLOW.md`:

```sh
vik service start --port 3000
```

`service start` starts one local Vik service center. When the current directory
contains `WORKFLOW.md`, it also registers that workflow for convenience.
When you plan to register a different workflow path with `vik work --workflow`,
start the service from a directory that does not contain `WORKFLOW.md`.

Register another workflow with an explicit path:

```sh
vik work --workflow /path/to/WORKFLOW.md
```

## Status

```sh
vik service status
```

Status values:

- `running`: stored pid is alive.
- `stopped`: service was stopped cleanly.
- `stale`: state exists but the pid is no longer alive.
- `not installed`: no service state file exists.

Status output also lists registered workflow paths.

## Logs

Service stdout and stderr are written to:

```text
<service-dir>/service.log
```

The default service directory is `$HOME/.vik/service`. Set `VIK_SERVICE_DIR`
to place service state somewhere else, for example in a test workspace.

Read recent logs:

```sh
vik service logs --lines 100
```

Follow logs:

```sh
vik service logs --follow
```

Daemon JSON logs are also written under `<service-dir>/logs/`.

## Restart And Stop

```sh
vik service restart --port 3000
vik service stop
```

Uninstall stops the process and removes service state:

```sh
vik service uninstall
```

## Environment

Workflow registration loads `.env` from the workflow working directory before
config dispatch validation. Existing shell environment values win over `.env`
values.

Required credentials:

- `LINEAR_API_KEY`
- Codex auth in `CODEX_HOME` or `OPENAI_API_KEY`
- GitHub CLI auth, `GH_TOKEN`, `GITHUB_TOKEN`, or working SSH credentials

## State Files

Service state lives under:

```text
${VIK_SERVICE_DIR:-$HOME/.vik/service}/
```

`service.json` records the service pid, cwd, log path, port, and command.
`workflows.json` records every registered workflow path and working directory.
Delete state only after confirming no matching Vik process is alive.
