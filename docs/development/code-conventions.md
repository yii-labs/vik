# Code Conventions

## Change Scope

- Keep changes narrow and reviewable. Example: land docs cleanup separately from
  runtime refactors.
- Do not mix unrelated refactors into behavior changes. Example: fix parsing
  without renaming modules.

## Design Before Shape

- Before adding an API, struct, trait, or broad refactor, design the smallest
  shape first. Name owner, callers, invariants, lifecycle, and test seam.
- If the shape is hard to explain, stop and simplify before coding.

## Rust

### Conventions

- Keep public APIs narrow and specific.
- Prefer receiver methods in `impl` blocks over free functions that take the
  receiver as first argument. Example: prefer `manager.stop()` over
  `stop_manager(&manager)`, `session.start()` over `start_session(&session)`.
- Add helpers only when they remove repeated invariants. The added helpers also
  follow the previous rule about receiver methods.
- Use a reusable type when stable options would be passed everywhere. Example:
  keep workflow paths resolution logic in `Workspace` instead of threading path
  options through each call.
- Install dependencies with `cargo add <crate>`.
- Use `<name>/mod.rs` for modules with submodules. Example: use
  `session/mod.rs` plus `session/factory.rs`.

### Abstractions

- Design like Rust std: prefer ownership, borrowing, enums, generics, iterators,
  newtypes, and `Result` over hidden state or stringly runtime plumbing.
- Prefer zero-cost abstractions. Use concrete types, enums, and generics before
  `dyn`, `Box`, `Arc<Mutex<_>>`, or callback maps.
- Use traits for shared behavior contracts, not organization. A trait needs
  multiple real implementors, a generic bound, or an extension seam.
- Do not add a trait only for one implementation, tests, or a future guess.
  Prefer inherent methods on the owner type.
- If dynamic dispatch is required, keep the trait small, object-safe, and owned
  by the boundary that consumes it.
- Add runtime indirection only when ownership, async boundaries, or provider
  extension truly require it.
- One type means one concept. Follow single responsibility principle for modules,
  structs, and traits.
