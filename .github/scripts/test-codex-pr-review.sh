#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
review_script="${repo_root}/.github/scripts/codex-pr-review.sh"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_file_contains() {
  local file="$1"
  local needle="$2"

  if ! grep -Fq -- "${needle}" "${file}"; then
    echo "Missing expected text in ${file}: ${needle}" >&2
    echo "--- ${file}" >&2
    [[ -f "${file}" ]] && cat "${file}" >&2
    exit 1
  fi
}

assert_file_missing_or_empty() {
  local file="$1"

  if [[ -s "${file}" ]]; then
    echo "Expected missing or empty file: ${file}" >&2
    echo "--- ${file}" >&2
    cat "${file}" >&2
    exit 1
  fi
}

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

bin_dir="${tmp_dir}/bin"
mkdir -p "${bin_dir}"

cat >"${bin_dir}/git" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >>"${STUB_LOG_DIR}/git.log"

case "${1:-}" in
  config)
    exit 0
    ;;
  fetch)
    exit "${GIT_STUB_FETCH_STATUS:-0}"
    ;;
esac

echo "unexpected git command: $*" >&2
exit 99
STUB

cat >"${bin_dir}/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >>"${STUB_LOG_DIR}/gh.log"

if [[ "${1:-}" != "pr" || "${2:-}" != "view" ]]; then
  echo "unexpected gh command: $*" >&2
  exit 99
fi

cat <<'JSON'
{"title":"Review test","url":"https://github.com/yii-labs/vik/pull/123","body":"","author":{"login":"forehalo"},"baseRefName":"main","headRefName":"test","files":[]}
JSON
STUB

cat >"${bin_dir}/codex" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >>"${STUB_LOG_DIR}/codex.log"

for arg in "$@"; do
  if [[ "${arg}" == "-" ]]; then
    echo "stdin prompt marker cannot be used with codex review --base" >&2
    exit 48
  fi
done

if [[ -n "${GH_TOKEN+x}" ]]; then
  echo "GH_TOKEN=set" >>"${STUB_LOG_DIR}/codex-env.log"
  echo "GH_TOKEN leaked into codex" >&2
  exit 44
else
  echo "GH_TOKEN=unset" >>"${STUB_LOG_DIR}/codex-env.log"
fi

if [[ -n "${GITHUB_TOKEN+x}" ]]; then
  echo "GITHUB_TOKEN=set" >>"${STUB_LOG_DIR}/codex-env.log"
  echo "GITHUB_TOKEN leaked into codex" >&2
  exit 45
else
  echo "GITHUB_TOKEN=unset" >>"${STUB_LOG_DIR}/codex-env.log"
fi

output_path=""
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --output-last-message)
      output_path="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

case "${CODEX_STUB_MODE:-write}" in
  write)
    [[ -n "${output_path}" ]] || {
      echo "missing --output-last-message" >&2
      exit 46
    }
    cat >"${output_path}" <<'MARKDOWN'
## Codex Review

No findings.
MARKDOWN
    exit 0
    ;;
  empty-success)
    exit 0
    ;;
esac

echo "unknown CODEX_STUB_MODE: ${CODEX_STUB_MODE:-}" >&2
exit 47
STUB

chmod +x "${bin_dir}/git" "${bin_dir}/gh" "${bin_dir}/codex"

run_case() {
  local name="$1"
  local case_dir="${tmp_dir}/${name}"
  local log_dir="${case_dir}/logs"

  rm -rf "${case_dir}"
  mkdir -p "${case_dir}/github" "${case_dir}/review" "${case_dir}/out" "${log_dir}"

  (
    export PATH="${bin_dir}:${PATH}"
    export STUB_LOG_DIR="${log_dir}"
    export GITHUB_REPOSITORY="yii-labs/vik"
    export GITHUB_WORKSPACE="${case_dir}/github"
    export BASE_REF="main"
    export CODEX_REVIEW_OUTPUT="${case_dir}/out/codex-review.md"
    export GH_TOKEN="test-token"
    export GITHUB_TOKEN="ambient-token"
    export REVIEW_WORKSPACE="${case_dir}/review"
    "${review_script}"
  )
}

run_case happy
assert_file_contains "${tmp_dir}/happy/out/codex-review.md" "No findings."
assert_file_contains "${tmp_dir}/happy/logs/git.log" "config --local http.https://github.com/.extraheader AUTHORIZATION: basic"
assert_file_contains "${tmp_dir}/happy/logs/git.log" "fetch --no-tags https://github.com/yii-labs/vik.git +refs/heads/main:refs/remotes/origin/main"
assert_file_contains "${tmp_dir}/happy/logs/git.log" "config --local --unset-all http.https://github.com/.extraheader"
assert_file_missing_or_empty "${tmp_dir}/happy/logs/gh.log"
assert_file_contains "${tmp_dir}/happy/logs/codex.log" "exec --sandbox read-only review --base origin/main --ephemeral --output-last-message"
assert_file_contains "${tmp_dir}/happy/logs/codex-env.log" "GH_TOKEN=unset"
assert_file_contains "${tmp_dir}/happy/logs/codex-env.log" "GITHUB_TOKEN=unset"

set +e
CODEX_STUB_MODE=empty-success run_case empty_success
empty_status=$?
set -e
if [[ "${empty_status}" -ne 1 ]]; then
  fail "empty-success case should exit 1, got ${empty_status}"
fi
assert_file_contains "${tmp_dir}/empty_success/out/codex-review.md" "Codex review failed before producing a final review message."
assert_file_contains "${tmp_dir}/empty_success/out/codex-review.md" "exit code: 0"

set +e
GIT_STUB_FETCH_STATUS=42 run_case fetch_failure
fetch_status=$?
set -e
if [[ "${fetch_status}" -ne 42 ]]; then
  fail "fetch failure should exit 42, got ${fetch_status}"
fi
assert_file_contains "${tmp_dir}/fetch_failure/logs/git.log" "config --local --unset-all http.https://github.com/.extraheader"
assert_file_missing_or_empty "${tmp_dir}/fetch_failure/logs/codex.log"

echo "codex-pr-review tests passed"
