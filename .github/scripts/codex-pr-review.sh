#!/usr/bin/env bash
set -euo pipefail

require_env() {
  local name="$1"

  if [[ -z "${!name:-}" ]]; then
    echo "Missing required environment variable: ${name}" >&2
    exit 2
  fi
}

require_cmd() {
  local name="$1"

  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 2
  fi
}

require_env GITHUB_REPOSITORY
require_env GITHUB_WORKSPACE
require_env PR_NUMBER
require_env BASE_REF
require_env CODEX_REVIEW_OUTPUT

prompt_file="${CODEX_REVIEW_PROMPT:-.github/codex/prompts/pr-review.md}"

require_cmd codex
require_cmd gh
require_cmd git

cd "${GITHUB_WORKSPACE}"

if [[ ! -f "${prompt_file}" ]]; then
  echo "Missing Codex review prompt: ${prompt_file}" >&2
  exit 2
fi

mkdir -p "$(dirname "${CODEX_REVIEW_OUTPUT}")"
: >"${CODEX_REVIEW_OUTPUT}"

gh auth status --hostname github.com >/dev/null

git fetch --no-tags origin "+refs/heads/${BASE_REF}:refs/remotes/origin/${BASE_REF}"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

pr_context="${tmp_dir}/pr-context.json"
review_prompt="${tmp_dir}/review-prompt.md"

gh pr view "${PR_NUMBER}" \
  --repo "${GITHUB_REPOSITORY}" \
  --json title,url,body,author,baseRefName,headRefName,files \
  >"${pr_context}"

pr_title="$(gh pr view "${PR_NUMBER}" --repo "${GITHUB_REPOSITORY}" --json title --jq '.title')"

{
  cat "${prompt_file}"
  printf '\n## Pull Request Context\n\n'
  printf '```json\n'
  cat "${pr_context}"
  printf '\n```\n'
} >"${review_prompt}"

set +e
codex exec review \
  --base "origin/${BASE_REF}" \
  --title "${pr_title}" \
  --ephemeral \
  --output-last-message "${CODEX_REVIEW_OUTPUT}" \
  - <"${review_prompt}"
codex_status=$?
set -e

if [[ ! -s "${CODEX_REVIEW_OUTPUT}" ]]; then
  cat >"${CODEX_REVIEW_OUTPUT}" <<EOF
## Codex Review

Codex review failed before producing a final review message.

- command: \`codex exec review --base origin/${BASE_REF}\`
- exit code: ${codex_status}
EOF
fi

exit "${codex_status}"
