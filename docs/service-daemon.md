# Service Daemon

Use service commands when Vik should keep running after the terminal exits.
This is a local detached process manager, not a system-wide launchd or systemd
unit.

## Install And Start

Run from the directory that contains `WORKFLOW.md`:

```sh
cargo run --locked -p vik-cli -- service install --port 3000
```

Use an explicit workflow path when managing another workflow:

```sh
cargo run --locked -p vik-cli -- service install /path/to/WORKFLOW.md --port 3000
```

## Status

```sh
cargo run --locked -p vik-cli -- service status
```

Status values:

- `running`: stored pid is alive.
- `stopped`: service was stopped cleanly.
- `stale`: state exists but the pid is no longer alive.
- `not installed`: no service state file exists for this workflow.

## Logs

Service stdout and stderr are written to:

```text
<workflow-directory>/.vik/service/<workflow-name>.log
```

Read recent logs:

```sh
cargo run --locked -p vik-cli -- service logs --lines 100
```

Follow logs:

```sh
cargo run --locked -p vik-cli -- service logs --follow
```

Daemon JSON logs still use `logging.dir` from `WORKFLOW.md`.

## Restart And Stop

```sh
cargo run --locked -p vik-cli -- service restart --port 3000
cargo run --locked -p vik-cli -- service stop
```

Uninstall stops the process and removes service state:

```sh
cargo run --locked -p vik-cli -- service uninstall
```

## Environment

Service startup loads `.env` from the workflow working directory before config
dispatch validation. Existing shell environment values win over `.env` values.

Required credentials:

- `LINEAR_API_KEY`
- Codex auth in `CODEX_HOME` or `OPENAI_API_KEY`
- GitHub CLI auth, `GH_TOKEN`, `GITHUB_TOKEN`, or working SSH credentials

## State Files

Service state lives under:

```text
<workflow-directory>/.vik/service/
```

The state JSON records workflow path, cwd, pid, log path, port, and command.
Delete state only after confirming no matching Vik process is alive.
