#!/usr/bin/env bash
# install_roogle.sh — installs roogle-server v0.0.1
set -euo pipefail

REPO="alpaylan/roogle"
# Allow pinning a specific version by setting ROOGLE_VERSION=v0.0.1
if [ "${ROOGLE_VERSION-}" != "" ]; then
  BASE_URL="https://github.com/${REPO}/releases/download/${ROOGLE_VERSION}"
else
  BASE_URL="https://github.com/${REPO}/releases/latest/download"
fi
uname_s="$(uname -s || true)"
uname_m="$(uname -m || true)"

# OS triple
case "$uname_s" in
  Darwin)  os="apple-darwin" ;;
  Linux)   os="unknown-linux-gnu" ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT) os="pc-windows-msvc" ;;
  *) echo "Unsupported OS: $uname_s"; exit 1 ;;
esac

# Arch
case "$uname_m" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64)
    if [ "$os" = "apple-darwin" ]; then
      arch="x86_64"
      echo "Note: using x86_64 macOS build on Apple Silicon (requires Rosetta)." >&2
    else
      echo "No arm64 build for $os yet. Please open an issue."; exit 1
    fi
    ;;
  *) echo "Unsupported arch: $uname_m"; exit 1 ;;
esac

# Asset names from the v0.0.1 release
# - macOS:    roogle-server-x86_64-apple-darwin (and .tar.gz variant)
# - Linux:    roogle-server-x86_64-unknown-linux-gnu
# - Windows:  roogle-server-x86_64-pc-windows-msvc.exe  :contentReference[oaicite:0]{index=0}
if [ "$os" = "pc-windows-msvc" ]; then
  asset="roogle-server-${arch}-${os}.exe"
else
  asset="roogle-server-${arch}-${os}"
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

echo "Downloading $asset ..."
curl -fL "$BASE_URL/$asset" -o "$tmpdir/$asset"

# Verify sha256 if provided
if curl -fsI "$BASE_URL/$asset.sha256" >/dev/null 2>&1; then
  echo "Verifying checksum..."
  expected="$(curl -fsSL "$BASE_URL/$asset.sha256" | head -n1 | sed 's/ .*//; s/\r$//; s/^sha256://')"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmpdir/$asset" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "$tmpdir/$asset" | awk '{print $1}')"
  fi
  [ "$expected" = "$actual" ] || { echo "Checksum mismatch!"; exit 1; }
fi

# Install location
dest="${INSTALL_DIR:-/usr/local/bin}"
[ -w "$dest" ] || { dest="$HOME/.local/bin"; mkdir -p "$dest"; case ":$PATH:" in *":$dest:"*) ;; *) echo "Tip: add '$dest' to your PATH";; esac; }

binname="${BIN_NAME:-roogle-server}"
install_path="$dest/$binname"
[ "$os" = "pc-windows-msvc" ] && install_path="$dest/${binname}.exe"

install -m 0755 "$tmpdir/$asset" "$install_path"
echo "Installed to: $install_path"
"$install_path" --version || true
