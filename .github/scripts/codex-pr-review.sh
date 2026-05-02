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
require_env GH_TOKEN

review_workspace="${REVIEW_WORKSPACE:-${GITHUB_WORKSPACE:-}}"
prompt_file="${CODEX_REVIEW_PROMPT:-.github/codex/prompts/pr-review.md}"

if [[ -z "${review_workspace}" ]]; then
  echo "Missing required environment variable: REVIEW_WORKSPACE or GITHUB_WORKSPACE" >&2
  exit 2
fi

require_cmd codex
require_cmd gh
require_cmd git
require_cmd base64

cd "${review_workspace}"

if [[ ! -f "${prompt_file}" ]]; then
  echo "Missing Codex review prompt: ${prompt_file}" >&2
  exit 2
fi

mkdir -p "$(dirname "${CODEX_REVIEW_OUTPUT}")"
: >"${CODEX_REVIEW_OUTPUT}"

gh auth status --active --hostname github.com >/dev/null

basic_auth="$(printf 'x-access-token:%s' "${GH_TOKEN}" | base64 | tr -d '\n')"
GIT_CONFIG_COUNT=1 \
  GIT_CONFIG_KEY_0=http.https://github.com/.extraheader \
  GIT_CONFIG_VALUE_0="AUTHORIZATION: basic ${basic_auth}" \
  git fetch --no-tags "https://github.com/${GITHUB_REPOSITORY}.git" \
    "+refs/heads/${BASE_REF}:refs/remotes/origin/${BASE_REF}"
unset basic_auth

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

pr_context="${tmp_dir}/pr-context.json"
review_prompt="${tmp_dir}/review-prompt.md"

gh pr view "${PR_NUMBER}" \
  --repo "${GITHUB_REPOSITORY}" \
  --json title,url,body,author,baseRefName,headRefName,files \
  >"${pr_context}"

{
  cat "${prompt_file}"
  printf '\n## Pull Request Context\n\n'
  printf '```json\n'
  cat "${pr_context}"
  printf '\n```\n'
} >"${review_prompt}"

set +e
env -u GH_TOKEN -u GITHUB_TOKEN codex exec --sandbox read-only review \
  --base "origin/${BASE_REF}" \
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
- sandbox: \`read-only\`
- exit code: ${codex_status}
EOF
  if [[ "${codex_status}" -eq 0 ]]; then
    codex_status=1
  fi
fi

exit "${codex_status}"
