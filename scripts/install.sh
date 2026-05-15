#!/bin/sh
set -eu

repo="yii-labs/vik"
release_tag="${VIK_INSTALL_TAG:-__VIK_RELEASE_TAG__}"
install_dir="${VIK_INSTALL_DIR:-${HOME}/.local/bin}"

if [ "${release_tag}" = "__VIK_RELEASE_TAG__" ]; then
  release_tag="latest"
fi

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "vik installer: missing required command: $1" >&2
    exit 1
  fi
}

download() {
  url="$1"
  out="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$out"
    return
  fi

  if command -v wget >/dev/null 2>&1; then
    wget -qO "$out" "$url"
    return
  fi

  echo "vik installer: missing curl or wget" >&2
  exit 1
}

case "$(uname -s)" in
  Linux)
    case "$(uname -m)" in
      x86_64 | amd64) asset="vik-linux-x64" ;;
      aarch64 | arm64) asset="vik-linux-arm64" ;;
      *)
        echo "vik installer: unsupported Linux architecture: $(uname -m)" >&2
        exit 1
        ;;
    esac
    ;;
  Darwin)
    case "$(uname -m)" in
      arm64) asset="vik-macos-arm64" ;;
      *)
        echo "vik installer: unsupported macOS architecture: $(uname -m)" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    echo "vik installer: unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
esac

need chmod
need mkdir
need mktemp
need mv

if [ "${release_tag}" = "latest" ]; then
  url="https://github.com/${repo}/releases/latest/download/${asset}"
else
  url="https://github.com/${repo}/releases/download/${release_tag}/${asset}"
fi

tmp="$(mktemp "${TMPDIR:-/tmp}/vik.XXXXXX")"
trap 'rm -f "$tmp"' EXIT INT HUP TERM

echo "Downloading ${asset} from ${release_tag}..."
download "$url" "$tmp"

chmod 755 "$tmp"
mkdir -p "$install_dir"
mv "$tmp" "${install_dir}/vik"
trap - EXIT INT HUP TERM

echo "Installed vik to ${install_dir}/vik"

case ":$PATH:" in
  *":${install_dir}:"*) ;;
  *) echo "Add ${install_dir} to PATH to run vik from any shell." ;;
esac
