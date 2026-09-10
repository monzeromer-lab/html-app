#!/usr/bin/env bash
# Build an AppImage of the HTML App runtime (PRD §13).
#
# The AppImage bundles WebKitGTK and its dependencies, which is the point: §16 R3 records that WPE
# packaging on Ubuntu is immature, and the same applies to keeping a WebKit version consistent
# across distributions. An AppImage sidesteps that by carrying its own.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
appdir="$root/target/HtmlApp.AppDir"
arch="$(uname -m)"
# Cargo features for the bundled binary. The release workflow passes the same set the tarball
# ships; empty — a plain local run — means default features.
features="${HTMLAPP_FEATURES:-}"

command -v appimagetool >/dev/null || {
  echo "appimagetool is not on PATH." >&2
  echo "Get it from https://github.com/AppImage/AppImageKit/releases" >&2
  exit 1
}

echo "==> Building the release binary${features:+ with $features}"
build=(--release --locked -p htmlapp --bin htmlapp --manifest-path "$root/Cargo.toml")
# `if` rather than `&&`: under `set -e` a false test on the last line of the script would exit.
if [ -n "$features" ]; then
  build+=(--features "$features")
fi
cargo build "${build[@]}"

echo "==> Assembling $appdir"
rm -rf "$appdir"
install -Dm755 "$root/target/release/htmlapp" "$appdir/usr/bin/htmlapp"
install -Dm644 "$root/packaging/htmlapp.desktop" "$appdir/htmlapp.desktop"
install -Dm644 "$root/packaging/htmlapp.desktop" \
  "$appdir/usr/share/applications/htmlapp.desktop"
install -Dm644 "$root/packaging/htmlapp.xml" "$appdir/usr/share/mime/packages/htmlapp.xml"
install -Dm644 "$root/packaging/htmlapp.svg" "$appdir/htmlapp.svg"
install -Dm644 "$root/packaging/htmlapp.svg" \
  "$appdir/usr/share/icons/hicolor/scalable/apps/htmlapp.svg"

# §7.2: the launcher's bundled examples travel with the AppImage.
install -dm755 "$appdir/usr/share/htmlapp/examples"
install -Dm644 "$root"/examples/*.hta -t "$appdir/usr/share/htmlapp/examples/"

echo "==> Bundling libraries"
mkdir -p "$appdir/usr/lib"
# Everything the binary links that is not part of the base system. glibc and the GL/Vulkan
# loaders are deliberately excluded: bundling those is how an AppImage breaks on a newer host.
ldd "$root/target/release/htmlapp" \
  | awk '/=> \//{print $3}' \
  | grep -vE '/(libc|libm|libdl|libpthread|librt|libstdc\+\+|libgcc_s|ld-linux)[.-]' \
  | grep -vE 'libGL|libEGL|libvulkan|libwayland|libX11|libxcb' \
  | sort -u \
  | while read -r lib; do
      cp -Lu "$lib" "$appdir/usr/lib/" 2>/dev/null || true
    done

cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="$HERE/usr/lib:$LD_LIBRARY_PATH"
# §7.2: the launcher looks for its examples relative to the binary when not installed.
export XDG_DATA_DIRS="$HERE/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
exec "$HERE/usr/bin/htmlapp" "$@"
APPRUN
chmod +x "$appdir/AppRun"

echo "==> Packing"
output="$root/target/HtmlApp-$arch.AppImage"
appimagetool "$appdir" "$output"

echo
echo "Built $output"
echo "Note: an AppImage does not register the .hta MIME type by itself."
echo "Run './HtmlApp-$arch.AppImage install' once to associate .hta files."
