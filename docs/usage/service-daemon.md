# Service Daemon

Use daemon commands when Vik should keep running after the terminal exits.
This is a local detached process manager, not a launchd or systemd unit.

The workflow path is accepted by every workflow subcommand:

```text
vik <command> [WORKFLOW]
```

## Start

Run from the directory that contains `workflow.yml`:

```sh
vik run -d
```

Use an explicit workflow path when managing another workflow:

```sh
vik run -d /path/to/workflow.yml
```

`--port` and `--bind-address` are parsed today, but the HTTP server is not
implemented. Do not use them for normal daemon runs yet.

## Status

```sh
vik status
vik status /path/to/workflow.yml
```

Status values:

- `running`: state file exists and stored pid is alive.
- `stale`: state file exists but pid is dead.
- `not installed`: no daemon state file exists for this workflow.

When a state file exists, status also prints pid, bind address, start time, log
dir, sessions dir, workflow path, and recorded command.

## Logs

Daemon logs live under:

```text
<workflow-workspace-root>/logs/
```

Files:

- `vik.log.YYYY-MM-DD`: INFO and above.
- `vik-error.log.YYYY-MM-DD`: ERROR only.

Use `vik status` to print the configured log directory.

```sh
tail -n 100 <log_dir>/vik.log.*
tail -f <log_dir>/vik-error.log.*
```

## Restart, Stop, Uninstall

```sh
vik restart
vik stop
vik uninstall
```

Explicit workflow path:

```sh
vik restart /path/to/workflow.yml
vik stop /path/to/workflow.yml
vik uninstall /path/to/workflow.yml
```

`restart` stops a running daemon if present, then starts a detached daemon.
If no daemon is running, it starts one.

`uninstall` stops a running daemon if present and removes stale state files.

## Shutdown

`vik stop`, `vik restart`, and SIGTERM trigger daemon shutdown.

Current code:

1. Sends SIGTERM to the daemon pid.
2. The daemon shutdown token cancels intake and running sessions.
3. The orchestrator returns after cancellation.
4. The daemon removes its state file and exits.
5. `vik stop` waits up to 30 seconds for the daemon process to exit.

No workspace cleanup runs on shutdown. The next start re-queries the tracker
through `issues.pull.command`.

## Environment

Daemon startup inherits the shell environment. Vik does not load `.env` files
and does not manage tracker credentials.

Required credentials depend on pull, prompt, and hook commands:

- Codex auth for `runtime: codex`
- Claude Code auth or `ANTHROPIC_API_KEY` for `runtime: claude_code`
- GitHub CLI auth, `GH_TOKEN`, `GITHUB_TOKEN`, or SSH credentials when hooks or
  prompts use GitHub
- `LINEAR_API_KEY` when prompts use Linear
- `lark-cli` config and auth for the same OS user when pull commands or
  prompts use Feishu Base

Templates can read environment values through `{{ env.VAR }}`. Missing values
fail template render under MiniJinja strict mode.

## State Files

`workspace.root` in YAML names a workspace home. Vik appends
`workflows/<workflow-path-key>/` to build the workflow-scoped workspace root.
`<workflow-path-key>` is the absolute workflow file path with `/` replaced by
`-`. Create `<workspace.root>/workflows` before the first run; Vik creates only
the final workflow-scoped root.

All Vik-owned state for one workflow lives under that workflow-scoped root:

```text
<workflow-workspace-root>/service/state.json
<workflow-workspace-root>/logs/
<workflow-workspace-root>/sessions/<issue.id>/<issue.state>-<uuid-v7>.jsonl
```

Issue workspaces live under `issues/`:

```text
<workflow-workspace-root>/issues/<issue.id>/
```

Vik does not provide an issue-workspace cleanup command. Stop active sessions
before removing issue workspace directories yourself.
