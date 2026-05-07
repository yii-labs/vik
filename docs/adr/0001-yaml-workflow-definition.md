# YAML Workflow Definition

Vik uses `workflow.yml` as the only runtime Workflow Definition source. We rejected `WORKFLOW.md` front matter plus Markdown body because one file mixed routing, runtime config, and large prompts, making stage-level prompt reuse and command-expanded prompt files harder to organize.

Prompt content now lives in explicit prompt files referenced by the Workflow Definition. This creates more files, but keeps workflow policy structured and lets each issue stage own a focused prompt source.
