# Service Daemon

Use service commands when Vik should keep running after the terminal exits.
This is a local detached process manager, not a system-wide launchd or systemd
unit.

## Start

Run from the directory that contains `WORKFLOW.md`:

```sh
vik service start --port 3000
```

Use an explicit workflow path when managing another workflow:

```sh
vik service start /path/to/WORKFLOW.md --port 3000
```

## Status

```sh
vik service status
```

Status values:

- `running`: stored pid is alive.
- `stopped`: service was stopped cleanly.
- `stale`: state exists but the pid is no longer alive.
- `not installed`: no service state file exists for this workflow.

## Logs

Daemon JSON logs are written to `logging.dir` from `WORKFLOW.md`, which
defaults to:

```text
<workspace.root>/logs/
```

The detached service also writes early startup stderr to:

```text
<workspace.root>/logs/vik-service.log
```

Use `service status` to print the configured log directory for the workflow.

Read recent daemon logs:

```sh
tail -n 100 <workspace.root>/logs/vik.log.*
```

Follow daemon logs:

```sh
tail -f <workspace.root>/logs/vik.log.*
```

## Restart And Stop

```sh
vik service restart --port 3000
vik service stop
```

If restart finds no running service for the workflow, it asks whether to start
one instead.

Uninstall stops the process and removes service state:

```sh
vik service uninstall
```

## Environment

Service startup loads `.env` from the workflow working directory before config
dispatch validation. Existing shell environment values win over `.env` values.

Required credentials:

- Tracker token: `LINEAR_API_KEY` for Linear, `GH_TOKEN`/`GITHUB_TOKEN` for
  GitHub, or authenticated `lark-cli` profile for Feishu
- Codex auth in `CODEX_HOME` or `OPENAI_API_KEY`
- GitHub CLI auth, `GH_TOKEN`, `GITHUB_TOKEN`, or working SSH credentials

## State Files

Service state lives under:

```text
<workspace.root>/service/
```

The state JSON records workflow path, cwd, pid, log dir, session dir, port, and command.
Delete state only after confirming no matching Vik process is alive.
