# Checks

Run the full gate before handoff. Record any failing command exactly.

## Full Gate

```sh
cargo run --locked -- doctor --json ./workflow.yml
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
git diff --check
```
