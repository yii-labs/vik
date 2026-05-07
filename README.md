# Vik

Vik is a local Rust service for unattended coding-agent orchestration. It reads
issues from commands, matches each returned `state` to stages in
`workflow.yml`, creates one issue workspace, and runs one-shot agent CLI
processes for stage work.

Vik does not own tracker state. Prompt files must say which commands read and
update issues, comments, pull requests, and links.

## Usage

- Start with [Get Started](docs/usage/get-started.md).
- Run as a detached local process through
  [Service Daemon](docs/usage/service-daemon.md).
- Tune workflow settings through [Configuration](docs/usage/configuration.md).
- Configure tracker commands through [Linear](docs/usage/trackers/linear.md) or
  [GitHub](docs/usage/trackers/github.md).
- Inspect logs, daemon state, and session JSONL through
  [Observation](docs/usage/observation.md).

## Development

Development docs live under [docs/development](docs/development/index.md).

## Credits

Vik is adapted from OpenAI's
[Symphony](https://github.com/openai/symphony) workflow and harness patterns.
Vik is also inspired by [Sandcastle](https://github.com/mattpocock/sandcastle) for prompt file design and commands output injection.
