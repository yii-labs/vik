# Get Started

Vik is a small program that watches an issue tracker such as GitHub,
Linear, or Feishu Base, and lets agents (Codex or Claude Code) work
on issues for you. You describe how you want it to behave in a single
file called `workflow.yml`. Once that file is ready, you start Vik and
walk away.

This guide builds that file with you, one section at a time. Run
every command from the same folder you are working in.

> Never paste API keys into chat, commit them to git, or print them
> with `echo` / `cat`. Keep secrets in environment variables.

## Install Vik

Install the latest release binary:

```sh
curl -fsSL https://github.com/yii-labs/vik/releases/latest/download/install.sh | sh -
```

The installer supports Linux x64, Linux arm64, and macOS arm64. It installs to
`~/.local/bin` by default. Override that with `VIK_INSTALL_DIR`:

```sh
curl -fsSL https://github.com/yii-labs/vik/releases/latest/download/install.sh | VIK_INSTALL_DIR=/usr/local/bin sh -
```

You can also install from crates.io:

```sh
cargo install vik --locked
```

## What you need first

- A terminal you are comfortable using.
- The `vik` binary installed and on your `PATH`.
- One coding agent: Codex (`codex`) **or** Claude Code (`claude`).
- One tracker account: GitHub, Linear, or Feishu Base.
- The usual command-line tools: `git`, `jq`.

Quick sanity check:

```sh
pwd
vik --help
git --version
jq --version
```

If any of these fail, install the missing tool before continuing.

## 1. Create `workflow.yml`

Pick (or create) a folder where you want to work from, and start an
empty config file inside it:

```sh
mkdir -p hello-vik
cd hello-vik
touch workflow.yml
```

This is the file you will add to in every step below. Open it in
your editor of choice.

## 2. Tell Vik where to put files

Vik keeps logs, per-issue working folders, and session records under
one workflow-scoped workspace directory. If you omit the whole `workspace`
section, Vik uses `.vik` as the workspace home. With `workspace: {}` or
`workspace.root: null`, Vik uses non-empty `VIK_HOME` directly when it is set;
otherwise it uses your home `.vik` directory.

You can skip this section for the default workspace. Add this only if you want
the `workspace.root` fallback:

```yaml
workspace: {}
```

You can also set `workspace.root` to an absolute path like
`/Users/you/vik-workspaces`.
Relative paths resolve from the directory that contains `workflow.yml`.
After choosing the workspace home, Vik adds `workflows/<workflow-path-key>/`
under that root so different workflow files do not collide. `vik run` creates
that directory if it is missing.

## 3. Pick a coding agent

Vik can drive Codex or Claude Code. Pick one and follow that section.

### Option A: Codex

Check the CLI is installed and logged in:

```sh
codex --version
codex login status
```

Add an `agents` section to `workflow.yml`. The name (`coder` here)
is yours to choose — stages will reference it later.

```yaml
agents:
  coder:
    runtime: codex
    model: gpt-5.5
```

### Option B: Claude Code

Check the CLI is installed and logged in:

```sh
claude --version
claude auth status
```

Add an `agents` section to `workflow.yml`:

```yaml
agents:
  coder:
    runtime: claude_code
    model: claude-sonnet-4-6
```

## 4. Connect a tracker, then add the pull command

Vik does not own tracker access. You give it a shell command that
prints a list of issues as JSON; Vik runs that command on a loop.
Pick the tracker you use and follow its dedicated guide for full
setup, sample pull commands, and the prompt-side commands you will
need later (read details, leave comments, change state, etc.).

| Tracker     | Auth                            | Setup guide                                    |
| ----------- | ------------------------------- | ---------------------------------------------- |
| GitHub      | `gh auth login` (or `GH_TOKEN`) | [GitHub Issue Source](trackers/github.md)      |
| Linear      | `export LINEAR_API_KEY=...`     | [Linear Issue Source](trackers/linear.md)      |
| Feishu Base | `lark-cli auth login`           | [Feishu Base Issue Source](trackers/feishu.md) |

Whichever you pick, every issue your pull command emits must include
at least:

- `id` — a unique issue id string.
- `title` — the issue title.
- `state` — the state Vik will match on. Case-sensitive.

Run the command by hand once to confirm it prints a JSON array
before pasting it into `issues.pull.command`:

```sh
./your-pull-command | jq 'length'
```

Then add the `issues.pull` block to `workflow.yml` exactly as the
tracker guide shows. `idle_sec` controls how long Vik waits between
pull cycles. Start at `5` for GitHub, `10` for Linear or Feishu Base,
then tune.

## 5. Tell Vik what to do per state

For each tracker state, Vik runs a stage: one prompt source given to
your agent. A prompt source can be a file or inline text. If you use
files, create a prompts folder and a starter prompt:

```sh
mkdir -p ./prompts
```

Write `./prompts/plan.md` with whatever you want the agent to do
when an issue is in the `todo` state. For example:

```text
You are working on issue {{ issue.id }}: {{ issue.title }}.

Read the issue, write a short plan as a comment on the issue, and
move it to the `work` state.
```

Then add `issue.stages` to `workflow.yml`. The `when.state` value
must match exactly what your pull command returns. Each stage must
define exactly one of `prompt_file` or `prompt`.

```yaml
issue:
  stages:
    plan:
      when:
        state: todo
      agent: coder
      prompt_file: ./prompts/plan.md
```

For small prompts, use inline text instead:

```yaml
issue:
  stages:
    plan:
      when:
        state: todo
      agent: coder
      prompt: |
        You are working on issue {{ issue.id }}: {{ issue.title }}.

        Read the issue, write a short plan as a comment on the issue,
        and move it to the `work` state.
```

You can add more stages over time — one per state you want Vik to
react to. Give each stage exactly one prompt source.

> Vik never updates the tracker on its own. Your prompts must tell
> the agent how to leave comments, change labels, open PRs, etc.

## 6. (Optional) Run something on every new issue

If every issue should start with the same setup — cloning your repo,
creating a branch, etc. — add an `after_create` hook. It runs once
per issue, in the issue's working folder, before any stage starts.

```yaml
issue:
  hooks:
    after_create: |
      git clone --depth 1 <your-repo-url> .
  stages:
    # ... same stages as before
```

This pattern assumes `workflow.yml` lives at the repo root. It creates a local
issue branch from `origin/main`. Existing issue branches are reused.

Vik skips `after_create` when the issue folder already exists. If setup fails
halfway through, clean or repair that folder before relying on the hook again.

## 7. Validate before running

`vik doctor` reads your `workflow.yml` and reports problems without
running anything:

```sh
vik doctor ./workflow.yml
```

Fix anything it flags (missing fields, unknown agents, empty
strings, etc.) before moving on.

## 8. Run Vik

Two ways to start. Pick one for your first run.

**Foreground** — logs print in this terminal, Ctrl-C stops Vik:

```sh
vik run ./workflow.yml
```

**Detached** — Vik keeps running after you close the terminal:

```sh
vik run -d ./workflow.yml
```

Check it is alive:

```sh
vik status ./workflow.yml
```

Stop it when you are done:

```sh
vik stop ./workflow.yml
```

## 9. See what is happening

Everything Vik does ends up on disk under your workflow-scoped workspace root.
Run `vik status ./workflow.yml` to print the exact `log_dir` and
`sessions_dir`.

Tail the main log:

```sh
tail -f <log_dir>/vik.log.*
```

Tail errors only:

```sh
tail -f <log_dir>/vik-error.log.*
```

Browse one transcript per agent session:

```sh
find <sessions_dir> -type f -name '*.jsonl' -maxdepth 3
```

If nothing happens: confirm `vik status` reports the daemon as
running, and check that your tracker actually returns at least one
issue in a state one of your stages matches.

## What to read next

- [Configuration](configuration.md) — every field in `workflow.yml`.
- [Service Daemon](service-daemon.md) — running Vik long-term.
- [Observation](observation.md) — reading logs and session events.
- [Linear Issue Source](trackers/linear.md) — Linear-specific setup.
- [GitHub Issue Source](trackers/github.md) — GitHub-specific setup.
- [Feishu Base Issue Source](trackers/feishu.md) — Feishu-specific setup.
