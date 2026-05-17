# Agent Instructions

## Doc Routing

Read this file first, and understand the task type you were given. Then read only the rows that match the task type.
Skip unlisted docs unless scope changes.

| Task type | Must read |
| --- | --- |
| Plan, research, or grill | [Development index](docs/development/index.md), [Architecture](docs/development/architecture.md) |
| Code implementation, refactor, or bug fix | [Development index](docs/development/index.md), [Architecture](docs/development/architecture.md), [Code conventions](docs/development/code-conventions.md), [Testing](docs/development/testing.md), [Checks](docs/development/checks.md) |
| Test-only change | [Development index](docs/development/index.md), [Testing](docs/development/testing.md), [Code conventions](docs/development/code-conventions.md), [Checks](docs/development/checks.md) |
| Docs-only change | [Development index](docs/development/index.md), [Checks](docs/development/checks.md), touched docs |
| Review or audit | [Architecture](docs/development/architecture.md), [Code conventions](docs/development/code-conventions.md), [Testing](docs/development/testing.md), [Review checklist](docs/development/review-checklist.md), [Checks](docs/development/checks.md) |
| PR, push, or handoff | [Checks](docs/development/checks.md), [Pull requests](docs/development/pull-requests.md), [Review checklist](docs/development/review-checklist.md) |

## Repo Rules

- Use `rg` for search.
- Keep docs concrete enough for another agent to execute.
- Everything committed to git must be English, except non-English text is
  by designed to be used in tests or demonstrations.
- Do not hide failing validation. Record the exact command and result.
