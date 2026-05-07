# Single Binary Crate

Vik ships as a single binary crate with internal modules under `src/`, not a Cargo workspace of per-module crates. Module boundaries are enforced by `pub(crate)` visibility and explicit `mod` declarations.

We rejected a workspace layout because publishing to crates.io would require publishing every internal crate (path deps don't resolve on crates.io), inflating the namespace and release process, while the user-visible artifact is one binary installable with `cargo install vik`. Workspace benefits (parallel compilation, enforced inter-crate DAG) do not outweigh the publishing friction for a service binary of this size.

If modules grow to the point where workspace separation adds real value (e.g. shared crates consumed by external tooling), splitting is a mechanical refactor.
