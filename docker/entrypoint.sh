#!/usr/bin/env bash
set -euo pipefail

: "${VIK_WORKFLOW_PATH:=/vik-workspace/WORKFLOW.md}"
: "${VIK_SERVICE_DIR:=/vik-workspace/.vik/service}"
: "${CODEX_HOME:=$HOME/.codex}"
: "${GH_CONFIG_DIR:=$HOME/.config/gh}"

mkdir -p "$(dirname "$VIK_WORKFLOW_PATH")" "$VIK_SERVICE_DIR" "$CODEX_HOME" "$GH_CONFIG_DIR"

if [[ -n "${GH_TOKEN:-}${GITHUB_TOKEN:-}${GH_ENTERPRISE_TOKEN:-}${GITHUB_ENTERPRISE_TOKEN:-}" ]]; then
    gh auth setup-git >/dev/null 2>&1 || true
fi

uses_default_workflow=0

if [[ $# -eq 0 ]]; then
    set -- vik daemon --workflow "$VIK_WORKFLOW_PATH"
    uses_default_workflow=1
elif [[ "$1" =~ ^(--help|-h|--version|-V)$ ]]; then
    set -- vik "$@"
elif [[ "$1" == -* ]]; then
    set -- vik daemon --workflow "$VIK_WORKFLOW_PATH" "$@"
    uses_default_workflow=1
elif [[ "$1" == "start" ]]; then
    if [[ "${2:-}" =~ ^(--help|-h)$ ]]; then
        shift
        set -- vik daemon "$@"
    elif [[ $# -eq 1 || "${2:-}" == -* ]]; then
        shift
        set -- vik daemon --workflow "$VIK_WORKFLOW_PATH" "$@"
        uses_default_workflow=1
    else
        shift
        set -- vik daemon --workflow "$@"
    fi
elif [[ "$1" == "check" ]]; then
    if [[ $# -eq 1 ]]; then
        set -- vik check "$VIK_WORKFLOW_PATH"
        uses_default_workflow=1
    else
        set -- vik "$@"
    fi
elif [[ "$1" == "vik" && $# -eq 1 ]]; then
    set -- vik daemon --workflow "$VIK_WORKFLOW_PATH"
    uses_default_workflow=1
elif [[ "$1" == "vik" && "${2:-}" =~ ^(--help|-h|--version|-V)$ ]]; then
    :
elif [[ "$1" == "vik" && "${2:-}" == "start" && "${3:-}" =~ ^(--help|-h)$ ]]; then
    shift 2
    set -- vik daemon "$@"
elif [[ "$1" == "vik" && "${2:-}" == "start" && ( $# -eq 2 || "${3:-}" == -* ) ]]; then
    shift 2
    set -- vik daemon --workflow "$VIK_WORKFLOW_PATH" "$@"
    uses_default_workflow=1
elif [[ "$1" == "vik" && "${2:-}" == "check" && $# -eq 2 ]]; then
    set -- vik check "$VIK_WORKFLOW_PATH"
    uses_default_workflow=1
elif [[ "$1" == "vik" && "${2:-}" == -* ]]; then
    shift
    set -- vik daemon --workflow "$VIK_WORKFLOW_PATH" "$@"
    uses_default_workflow=1
fi

if [[ "$uses_default_workflow" -eq 1 && ! -f "$VIK_WORKFLOW_PATH" ]]; then
    echo "missing workflow file: $VIK_WORKFLOW_PATH" >&2
    echo "mount a workspace directory containing WORKFLOW.md or set VIK_WORKFLOW_PATH" >&2
    exit 64
fi

exec "$@"
