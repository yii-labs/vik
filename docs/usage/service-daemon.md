# Service Daemon

Use service commands when Vik should keep running after the terminal exits.
This is a local detached process manager, not a system-wide launchd or systemd
unit.

## Install And Start

Run from the directory that contains `WORKFLOW.md`:

```sh
vik service install --port 3000
```

Use an explicit workflow path when managing another workflow:

```sh
vik service install /path/to/WORKFLOW.md --port 3000
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

Service stdout and stderr are written to:

```text
<workflow-directory>/.vik/service/<workflow-stem>-<path-hash>.log
```

Set `logging.service_dir` in `WORKFLOW.md` to use a different directory:

```yaml
logging:
  service_dir: service
```

The service state file uses the same name with `.json`. The CLI derives the
name from the sanitized workflow file stem plus a stable hash of the full
workflow path. Use `service status` or `service logs` when possible; both
commands resolve the exact file path for the workflow.

Read recent logs:

```sh
vik service logs --lines 100
```

Follow logs:

```sh
vik service logs --follow
```

Daemon JSON logs still use `logging.dir` from `WORKFLOW.md`.

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

When `logging.service_dir` is set, state files live in that configured
directory instead.

Changing `logging.service_dir` does not migrate or read service state from the
previous directory. Stop or uninstall the existing service before changing the
service state directory.

Service management loads `.env` before reading `logging.service_dir` and before
full dispatch validation, so `status`, `logs`, and `stop` can still find
configured state when unrelated workflow fields are invalid.

The state JSON records workflow path, cwd, pid, log path, port, and command.
Delete state only after confirming no matching Vik process is alive.
