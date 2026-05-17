# Testing

Prefer TDD for behavior changes and bug fixes.

## TDD Loop

- One failing test for one behavior.
- Smallest production change to pass it.
- Refactor only after green.
- Repeat. No batch tests first.
- Use `tdd` skill if it's available

## Test Shape

- Test public behavior, not private implementation.
- Name tests after behavior.
- Each test sets only fields it needs.
- Avoid broad default fixtures that hide intent.
- Mock only slow, external, or nondeterministic boundaries.

## Workflow Fixtures

- `Workflow::builder()` for runtime tests that need a `Workflow`.
- Direct `WorkflowSchema` construction for schema-level behavior.
- `WorkflowSchemaLoader::load_from_str` only when parser or validation behavior
  is the behavior under test.
- Do not hand-write workflow YAML strings for runtime unit tests.
- Parser tests use smallest YAML input that proves parser behavior.

## Filesystem Fixtures

- Do not create temp files just to satisfy an API.
- Use synthetic `PathBuf` when code only needs a path.
- Use in-memory data when code only needs bytes.
- Use `TempDir` only when filesystem behavior is under test.

## Validation

Run [Checks](checks.md) before handoff. Use docs-only gate for docs-only changes.
