#!/usr/bin/env bash
set -euo pipefail

: "${VIK_WORKFLOW_PATH:=/workflow/WORKFLOW.md}"
: "${CODEX_HOME:=$HOME/.codex}"
: "${GH_CONFIG_DIR:=$HOME/.config/gh}"

mkdir -p "$(dirname "$VIK_WORKFLOW_PATH")" "$CODEX_HOME" "$GH_CONFIG_DIR"

if [[ -n "${GH_TOKEN:-}${GITHUB_TOKEN:-}${GH_ENTERPRISE_TOKEN:-}${GITHUB_ENTERPRISE_TOKEN:-}" ]]; then
    gh auth setup-git >/dev/null 2>&1 || true
fi

uses_default_workflow=0

if [[ $# -eq 0 ]]; then
    set -- vik "$VIK_WORKFLOW_PATH"
    uses_default_workflow=1
elif [[ "$1" == "--check" ]]; then
    shift
    set -- vik "$VIK_WORKFLOW_PATH" --check "$@"
    uses_default_workflow=1
elif [[ "$1" == "vik" && $# -eq 1 ]]; then
    set -- vik "$VIK_WORKFLOW_PATH"
    uses_default_workflow=1
elif [[ "$1" == "vik" && "${2:-}" == "--check" ]]; then
    shift 2
    set -- vik "$VIK_WORKFLOW_PATH" --check "$@"
    uses_default_workflow=1
fi

if [[ "$uses_default_workflow" -eq 1 && ! -f "$VIK_WORKFLOW_PATH" ]]; then
    echo "missing workflow file: $VIK_WORKFLOW_PATH" >&2
    echo "mount a workspace directory containing WORKFLOW.md or set VIK_WORKFLOW_PATH" >&2
    exit 64
fi

exec "$@"
