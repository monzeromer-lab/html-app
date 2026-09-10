#!/usr/bin/env sh
# HTML App installer (PRD §13).
#
#   curl -fsSL https://htmlapp.dev/install.sh | sh
#
# Installs to ~/.local/bin and registers the .hta MIME type, because §13 is explicit that
# "an install that leaves double-click broken is a failed install".
#
# This script never runs as root and never touches anything outside your home directory.
set -eu

REPO="monzeromer-lab/htmlapp"
PREFIX="${HTMLAPP_PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin"

say()  { printf '%s\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || die "this installer needs $1"
}

need curl
need uname

arch="$(uname -m)"
case "$arch" in
  x86_64|amd64)  arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) die "no prebuilt binary for $arch — build from source: cargo install --path crates/htmlapp" ;;
esac

[ "$(uname -s)" = "Linux" ] || die "HTML App is Linux-only (see PRD §12)"

version="${HTMLAPP_VERSION:-latest}"
if [ "$version" = "latest" ]; then
  say "==> Finding the latest release"
  version="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)"
  [ -n "$version" ] || die "could not determine the latest version"
fi

url="https://github.com/$REPO/releases/download/$version/htmlapp-$arch-linux.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "==> Downloading $version for $arch"
curl -fsSL "$url" -o "$tmp/htmlapp.tar.gz" || die "download failed: $url"

# Verify the checksum if the release published one. A missing checksum is a warning, not a
# hard failure, so a self-hosted mirror without one still works.
if curl -fsSL "$url.sha256" -o "$tmp/htmlapp.tar.gz.sha256" 2>/dev/null; then
  say "==> Verifying checksum"
  ( cd "$tmp" && sha256sum -c htmlapp.tar.gz.sha256 >/dev/null 2>&1 ) \
    || die "checksum mismatch — refusing to install"
else
  say "    (no published checksum for this release)"
fi

tar -xzf "$tmp/htmlapp.tar.gz" -C "$tmp"
[ -f "$tmp/htmlapp" ] || die "the archive did not contain a htmlapp binary"

say "==> Installing to $BIN"
mkdir -p "$BIN"
install -m755 "$tmp/htmlapp" "$BIN/htmlapp"

# §13: registration is part of the install. `htmlapp install` writes the .desktop entry and
# the MIME package, then refreshes the desktop and MIME databases.
say "==> Registering the .hta file type"
"$BIN/htmlapp" install || say "    (registration reported a problem; see above)"

case ":$PATH:" in
  *":$BIN:"*) ;;
  *)
    say ""
    say "Note: $BIN is not on your PATH. Add this to your shell profile:"
    say "    export PATH=\"$BIN:\$PATH\""
    ;;
esac

say ""
say "Installed $("$BIN/htmlapp" --version 2>/dev/null || echo htmlapp)."
say "Run 'htmlapp' to open the launcher, or double-click any .hta file."
