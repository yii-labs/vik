# YAML Workflow Definition

Vik uses `workflow.yml` as the only runtime Workflow Definition source. We rejected `WORKFLOW.md` front matter plus Markdown body because one file mixed routing, runtime config, and large prompts, making stage-level prompt reuse and command-expanded prompt sources harder to organize.

Prompt content now lives in explicit prompt sources referenced by the Workflow Definition. Most stages use prompt files, and small stages may use inline `prompt` text. This keeps workflow policy structured and lets each issue stage own a focused prompt source.
