# Setup

## Requirements

- Rust stable toolchain with `cargo`, `rustfmt`, and `clippy`.
- Git.
- GitHub CLI for PR work.
- Codex CLI for local orchestration runs.
- Linear API key or GitHub token for real tracker runs.
- Docker for image validation when touching Docker files.

## Clone

```sh
git clone git@github.com:yii-labs/vik.git
cd vik
```

## Environment

Create `.env` only for local runs:

```sh
cp .env.example .env
```

Replace the tracker token placeholders with real keys. Do not commit `.env`.

For config-only validation, tracker credentials are not required because
`vik check` validates config shape and does not call the tracker. Missing
tracker credentials are reported as warnings:

```sh
cargo run --locked -p vik-cli -- check ./WORKFLOW.md
```

## Local Smoke Run

Use an isolated tracker project or repository and workspace root for real daemon
testing. Do not point a smoke run at the shared tracker unless that is the test
target.

For a Linear smoke run:

```sh
: "${VIK_LINEAR_PROJECT_SLUG:?set isolated Linear project slug}"
mkdir -p .tests "$HOME/code/vik-workspaces-local"
cp WORKFLOW.md .tests/WORKFLOW.local.md
perl -0pi -e "s/project_slug: \"[^\"]+\"/project_slug: \"$VIK_LINEAR_PROJECT_SLUG\"/" \
  .tests/WORKFLOW.local.md
perl -0pi -e "s|root: .*|root: ~/code/vik-workspaces-local|" \
  .tests/WORKFLOW.local.md
cargo run --locked -p vik-cli -- check .tests/WORKFLOW.local.md
cargo run --locked -p vik-cli -- start .tests/WORKFLOW.local.md --port 3000
```

Inspect:

```sh
curl -fsS http://127.0.0.1:3000/api/v1/state | jq .
```
