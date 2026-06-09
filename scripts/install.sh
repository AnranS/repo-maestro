#!/bin/sh
# maestro installer — downloads a prebuilt binary (closed-source distribution:
# only the compiled binary is shipped, never the source).
#
#   curl -fsSL https://github.com/AnranS/maestro-dist/releases/latest/download/install.sh | sh
#
# Configuration (environment variables):
#   MAESTRO_INSTALL_BASE  Base URL for release assets.
#                         Default: https://github.com/$MAESTRO_REPO/releases/latest/download
#   MAESTRO_REPO          owner/name of the release repo (default: AnranS/maestro)
#   MAESTRO_VERSION       Tag to install, e.g. v0.1.0 (default: latest)
#   MAESTRO_BIN_DIR       Install directory (default: $HOME/.local/bin)
#
# Private repo? Point MAESTRO_INSTALL_BASE at an authenticated mirror, or host
# the binaries in a public "dist" repo (the source repo can stay private).
set -eu

REPO="${MAESTRO_REPO:-AnranS/maestro-dist}"
VERSION="${MAESTRO_VERSION:-latest}"
BIN_DIR="${MAESTRO_BIN_DIR:-$HOME/.local/bin}"
if [ "$VERSION" = "latest" ]; then
  BASE="${MAESTRO_INSTALL_BASE:-https://github.com/$REPO/releases/latest/download}"
else
  BASE="${MAESTRO_INSTALL_BASE:-https://github.com/$REPO/releases/download/$VERSION}"
fi

say() { printf '\033[1;36m→\033[0m %s\n' "$1"; }
err() { printf '\033[1;31m✗\033[0m %s\n' "$1" >&2; exit 1; }

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin)
    case "$arch" in
      arm64|aarch64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) err "unsupported macOS arch: $arch" ;;
    esac ;;
  Linux)
    case "$arch" in
      x86_64) target="x86_64-unknown-linux-gnu" ;;
      *) err "unsupported Linux arch: $arch (prebuilt binaries: x86_64 only for now)" ;;
    esac ;;
  *) err "unsupported OS: $os" ;;
esac

asset="maestro-${target}.tar.gz"
url="$BASE/$asset"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "downloading $asset"
if ! curl -fsSL "$url" -o "$tmp/$asset"; then
  err "download failed: $url
  (if the repo is private, the release assets aren't publicly downloadable —
   set MAESTRO_INSTALL_BASE to an authenticated mirror, or publish the binaries
   to a public dist repo.)"
fi

say "extracting"
tar xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$BIN_DIR"
for b in maestro mst; do
  src="$(find "$tmp" -type f -name "$b" -perm -u+x 2>/dev/null | head -1)"
  [ -n "$src" ] || src="$(find "$tmp" -type f -name "$b" | head -1)"
  [ -n "$src" ] || err "binary '$b' not found in archive"
  install -m 0755 "$src" "$BIN_DIR/$b"
done

say "installed maestro + mst to $BIN_DIR"
case ":$PATH:" in
  *":$BIN_DIR:"*) "$BIN_DIR/maestro" --version 2>/dev/null || true ;;
  *) printf '\033[1;33m!\033[0m add it to your PATH:  export PATH="%s:$PATH"\n' "$BIN_DIR" ;;
esac
say "run 'maestro' (or 'mst') to start"
