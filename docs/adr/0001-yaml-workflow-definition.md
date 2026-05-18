# YAML Workflow Definition

Vik uses `workflow.yml` as the only runtime Workflow Definition source. We rejected `WORKFLOW.md` front matter plus Markdown body because one file mixed routing, runtime config, and large prompts, making stage-level prompt reuse and command-expanded prompt files harder to organize.

Prompt content now lives in explicit stage prompt sources referenced by the
Workflow Definition. Stages can use `prompt_file` for reusable files or
`prompt` for small inline prompts. Each stage owns exactly one prompt source.
