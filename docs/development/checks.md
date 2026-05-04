# Checks

Run these before push unless the change is explicitly docs-only and the reviewer
accepts a narrower gate.

## Required CI Parity

```sh
LINEAR_API_KEY=ci-placeholder cargo run --locked -p vik-cli -- check ./WORKFLOW.md
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
git diff --check
```

## Docs Gate

Run for docs changes:

```sh
rg -n "[\\p{Han}\\p{Hiragana}\\p{Katakana}]" \
  README.md AGENTS.md docs .env.example WORKFLOW.md Dockerfile docker
```

No matches should be returned.

## Docker Gate

Run when `Dockerfile`, `docker/`, or Docker docs change:

```sh
docker build -t vik:local .
mkdir -p "$PWD/.vik/docker-workspace"
cp WORKFLOW.md "$PWD/.vik/docker-workspace/WORKFLOW.md"
docker run --rm \
  --env LINEAR_API_KEY=ci-placeholder \
  -v "$PWD/.vik/docker-workspace:/vik-workspace" \
  vik:local vik check
```

Pass real `GH_TOKEN`, `OPENAI_API_KEY`, and `LINEAR_API_KEY` only for an
end-to-end daemon run against an isolated project.

## Runtime Gate

Run when orchestration behavior changes:

```sh
VIK_SERVICE_DIR="$PWD/.tests/service" cargo run --locked -p vik-cli -- \
  service start --port 3000
VIK_SERVICE_DIR="$PWD/.tests/service" cargo run --locked -p vik-cli -- \
  work --workflow ./WORKFLOW.md
curl -fsS http://127.0.0.1:3000/api/v1/state | jq .
VIK_SERVICE_DIR="$PWD/.tests/service" cargo run --locked -p vik-cli -- service stop
```

Record the issue state, PR link, and final Linear state when using Vik itself to
drive a ticket.
