#!/usr/bin/env bash
# Install Forge3D Studio for the current user: a desktop entry, the app icon,
# and the binary on PATH. Nothing is copied out of the build tree without
# asking, and nothing is written outside ~/.local.
#
#   studio/packaging/install.sh            # install
#   studio/packaging/install.sh --uninstall
#
# The binary must be built first:  cargo build --release -p forge3d-studio
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
binary="$root/target/release/forge3d-studio"
icon_src="$root/assets/icons/mountain-snow.svg"

prefix="${XDG_DATA_HOME:-$HOME/.local/share}"
bindir="$HOME/.local/bin"
appdir="$prefix/applications"
icondir="$prefix/icons/hicolor/scalable/apps"

if [[ "${1:-}" == "--uninstall" ]]; then
  rm -f "$appdir/forge3d-studio.desktop" "$icondir/forge3d-studio.svg" "$bindir/forge3d-studio"
  echo "removed Forge3D Studio from $appdir, $icondir and $bindir"
  exit 0
fi

if [[ ! -x "$binary" ]]; then
  echo "no release binary at $binary" >&2
  echo "build it first: cargo build --release -p forge3d-studio" >&2
  exit 1
fi

mkdir -p "$appdir" "$icondir" "$bindir"
install -m 0755 "$binary" "$bindir/forge3d-studio"
install -m 0644 "$icon_src" "$icondir/forge3d-studio.svg"
install -m 0644 "$here/forge3d-studio.desktop" "$appdir/forge3d-studio.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$appdir" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$prefix/icons/hicolor" >/dev/null 2>&1 || true
fi

size="$(du -h "$bindir/forge3d-studio" | cut -f1)"
echo "installed Forge3D Studio ($size)"
echo "  binary:  $bindir/forge3d-studio"
echo "  entry:   $appdir/forge3d-studio.desktop"
echo "  icon:    $icondir/forge3d-studio.svg"
case ":$PATH:" in
  *":$bindir:"*) printf 'run it with: forge3d-studio [scene.tif]\n' ;;
  *) printf 'note: %s is not on PATH; run %s/forge3d-studio\n' "$bindir" "$bindir" ;;
esac
