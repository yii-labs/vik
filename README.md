# Vik

Vik is a local Rust service for unattended coding-agent orchestration. It reads
issues from commands, matches each returned `state` to stages in
`workflow.yml`, creates one issue workspace, and runs one-shot agent CLI
processes for stage work.

Vik does not own tracker state. Prompt files must say which commands read and
update issues, comments, pull requests, and links.

## Install

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

## Usage

- Start with [Get Started](docs/usage/get-started.md).
- Run as a detached local process through
  [Service Daemon](docs/usage/service-daemon.md).
- Tune workflow settings through [Configuration](docs/usage/configuration.md).
- Configure tracker commands through [Linear](docs/usage/trackers/linear.md),
  [GitHub](docs/usage/trackers/github.md), or
  [Feishu Base](docs/usage/trackers/feishu.md).
- Inspect logs, daemon state, and session JSONL through
  [Observation](docs/usage/observation.md).

## Development

Development docs live under [docs/development](docs/development/index.md).

## Credits

Vik is adapted from OpenAI's
[Symphony](https://github.com/openai/symphony) workflow and harness patterns.
Vik is also inspired by [Sandcastle](https://github.com/mattpocock/sandcastle) for prompt file design and commands output injection.
