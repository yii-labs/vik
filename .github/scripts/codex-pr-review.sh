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
require_env BASE_REF
require_env CODEX_REVIEW_JSON
require_env CODEX_REVIEW_OUTPUT
require_env GH_TOKEN

review_workspace="${REVIEW_WORKSPACE:-${GITHUB_WORKSPACE:-}}"

if [[ -z "${review_workspace}" ]]; then
  echo "Missing required environment variable: REVIEW_WORKSPACE or GITHUB_WORKSPACE" >&2
  exit 2
fi

require_cmd codex
require_cmd git
require_cmd base64

cd "${review_workspace}"

mkdir -p "$(dirname "${CODEX_REVIEW_OUTPUT}")"
: >"${CODEX_REVIEW_OUTPUT}"
mkdir -p "$(dirname "${CODEX_REVIEW_JSON}")"
: >"${CODEX_REVIEW_JSON}"

basic_auth="$(printf 'x-access-token:%s' "${GH_TOKEN}" | base64 | tr -d '\n')"
git_auth_key='http.https://github.com/.extraheader'
git config --local "${git_auth_key}" "AUTHORIZATION: basic ${basic_auth}"
unset basic_auth

set +e
git fetch --no-tags "https://github.com/${GITHUB_REPOSITORY}.git" \
  "+refs/heads/${BASE_REF}:refs/remotes/origin/${BASE_REF}"
fetch_status=$?
set -e
git config --local --unset-all "${git_auth_key}" >/dev/null 2>&1 || true
unset git_auth_key

if [[ "${fetch_status}" -ne 0 ]]; then
  exit "${fetch_status}"
fi

set +e
# The review subcommand owns prompt and PR context loading. Do not pass stdin:
# current Codex CLI rejects combining `review --base` with a prompt argument.
env -u GH_TOKEN -u GITHUB_TOKEN codex exec --sandbox read-only review \
  --base "origin/${BASE_REF}" \
  --json \
  --ephemeral \
  --output-last-message "${CODEX_REVIEW_OUTPUT}" \
  >"${CODEX_REVIEW_JSON}"
codex_status=$?
set -e

if [[ ! -s "${CODEX_REVIEW_OUTPUT}" ]]; then
  cat >"${CODEX_REVIEW_OUTPUT}" <<EOF
## Codex Review

Codex review failed before producing a final review message.

- command: \`codex exec review --base origin/${BASE_REF} --json\`
- sandbox: \`read-only\`
- exit code: ${codex_status}
EOF
  if [[ "${codex_status}" -eq 0 ]]; then
    codex_status=1
  fi
fi

exit "${codex_status}"
