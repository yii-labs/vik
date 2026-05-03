# Vik

Vik is a Rust service for unattended coding-agent orchestration. It reads Linear
issues, creates one isolated workspace per issue, and runs Codex app-server
sessions inside those workspaces.

Vik is adapted from OpenAI's
[Symphony](https://github.com/openai/symphony) workflow and harness patterns.

## Usage

- Start with [Get Started](docs/get-started.md).
- Run with Docker through [Docker](docs/docker.md).
- Run as a detached local process through
  [Service Daemon](docs/service-daemon.md).
- Tune workflow settings through [Configuration](docs/configuration.md).
- Inspect runtime state through [Observation](docs/observation.md).

## Development

Development docs live under [docs/development](docs/development/index.md).
