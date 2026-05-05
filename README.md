# Vik

Vik is a Rust service for unattended coding-agent orchestration. It reads issue
tracker issues, creates one isolated workspace per issue, and runs Codex
app-server sessions inside those workspaces.

Vik is adapted from OpenAI's
[Symphony](https://github.com/openai/symphony) workflow and harness patterns.

## Usage

- Start with [Get Started](docs/usage/get-started.md).
- Run with Docker through [Docker](docs/usage/docker.md).
- Run as a detached local process through
  [Service Daemon](docs/usage/service-daemon.md).
- Tune workflow settings through [Configuration](docs/usage/configuration.md).
- Configure trackers through [Linear](docs/usage/trackers/linear.md),
  [GitHub](docs/usage/trackers/github.md), or
  [Feishu](docs/usage/trackers/feishu.md).
- Inspect runtime state through [Observation](docs/usage/observation.md).

## Development

Development docs live under [docs/development](docs/development/index.md).
