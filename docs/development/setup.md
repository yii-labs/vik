# Setup

## Requirements

- Rust stable toolchain with `cargo`, `rustfmt`, and `clippy`.
- Git.
- GitHub CLI for PR work.
- Codex CLI for local orchestration runs.
- Linear API key for real tracker runs.
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

Replace `LINEAR_API_KEY=lin_api_xxx` with a real key. Do not commit `.env`.

For config-only validation, a placeholder environment value is enough because
`--check` validates config shape and does not call Linear:

```sh
LINEAR_API_KEY=ci-placeholder cargo run --locked -p vik-cli -- ./WORKFLOW.md --check
```

## Local Smoke Run

Use an isolated Linear project and workspace root for real daemon testing. Do
not point a smoke run at the shared project unless that is the test target.

```sh
: "${VIK_LINEAR_PROJECT_SLUG:?set isolated Linear project slug}"
mkdir -p .tests "$HOME/code/vik-workspaces-local"
cp WORKFLOW.md .tests/WORKFLOW.local.md
perl -0pi -e "s/project_slug: \"[^\"]+\"/project_slug: \"$VIK_LINEAR_PROJECT_SLUG\"/" \
  .tests/WORKFLOW.local.md
perl -0pi -e "s|root: .*|root: ~/code/vik-workspaces-local|" \
  .tests/WORKFLOW.local.md
LINEAR_API_KEY=ci-placeholder cargo run --locked -p vik-cli -- \
  .tests/WORKFLOW.local.md --check
cargo run --locked -p vik-cli -- .tests/WORKFLOW.local.md --port 3000
```

Inspect:

```sh
curl -fsS http://127.0.0.1:3000/api/v1/state | jq .
```
