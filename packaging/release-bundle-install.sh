#!/usr/bin/env bash
# Copied into GitHub release archives as install.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
mkdir -p "$PREFIX/bin" "$PREFIX/share"
install -m755 "$HERE/bin/celestine" "$PREFIX/bin/celestine"
cp -a "$HERE/share/"* "$PREFIX/share/"
if [[ -f "$PREFIX/share/applications/celestine.desktop" ]]; then
  # macOS sed needs a backup suffix; Linux accepts -i with optional extension.
  sed -i.bak "s|^Exec=.*|Exec=$PREFIX/bin/celestine %F|" "$PREFIX/share/applications/celestine.desktop"
  sed -i.bak "s|^TryExec=.*|TryExec=$PREFIX/bin/celestine|" "$PREFIX/share/applications/celestine.desktop"
  rm -f "$PREFIX/share/applications/celestine.desktop.bak"
fi
echo "Installed to $PREFIX/bin/celestine"
