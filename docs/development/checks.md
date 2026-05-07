# Checks

Run these before handoff unless the change is explicitly docs-only and the
reviewer accepts a narrower gate.

## Required CI Parity

```sh
cargo run --locked -- doctor --json ./workflow.yml
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
git diff --check
```

## Docs-Only Narrow Gate

For docs-only changes, run:

```sh
cargo run --locked -- --help
cargo run --locked -- doctor --json ./workflow.yml
git diff --check
```

Also grep changed docs for stale command shapes and non-English text before
handoff.
