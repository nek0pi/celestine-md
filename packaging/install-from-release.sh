#!/usr/bin/env bash
# Download the latest Linux x86_64 GitHub release and install to ~/.local
#
# One-liner:
#   curl -fsSL https://raw.githubusercontent.com/nek0pi/celestine-md/main/packaging/install-from-release.sh | bash
#
# Or pin a version:
#   CELESTINE_VERSION=v0.2.0 bash install-from-release.sh
set -euo pipefail

REPO="nek0pi/celestine-md"
PREFIX="${PREFIX:-$HOME/.local}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: need '$1' on PATH" >&2
    exit 1
  }
}
need curl
need tar

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ARTIFACT="celestine-linux-x86_64" ;;
  *)
    echo "error: no prebuilt Linux binary for arch '$ARCH' yet." >&2
    echo "Build from source: https://github.com/${REPO}#from-source" >&2
    exit 1
    ;;
esac

if [[ -n "${CELESTINE_VERSION:-}" ]]; then
  TAG="$CELESTINE_VERSION"
  [[ "$TAG" == v* ]] || TAG="v$TAG"
else
  TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  [[ -n "$TAG" ]] || {
    echo "error: could not resolve latest release tag" >&2
    exit 1
  }
fi

VER="${TAG#v}"
ARCHIVE="${ARTIFACT}-v${VER}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ARCHIVE}"

echo "==> Downloading ${ARCHIVE}"
curl -fsSL "$URL" -o "$TMP/$ARCHIVE"
tar -xzf "$TMP/$ARCHIVE" -C "$TMP"

BUNDLE="$TMP/$ARTIFACT"
[[ -x "$BUNDLE/bin/celestine" ]] || {
  echo "error: unexpected archive layout (missing bin/celestine)" >&2
  exit 1
}

# Prefer the bundle's install.sh when present (future releases); else install here.
if [[ -x "$BUNDLE/install.sh" ]]; then
  PREFIX="$PREFIX" "$BUNDLE/install.sh"
else
  mkdir -p "$PREFIX/bin" "$PREFIX/share"
  install -m755 "$BUNDLE/bin/celestine" "$PREFIX/bin/celestine"
  if [[ -d "$BUNDLE/share" ]]; then
    cp -a "$BUNDLE/share/"* "$PREFIX/share/"
  fi
  DESKTOP="$PREFIX/share/applications/celestine.desktop"
  if [[ -f "$DESKTOP" ]]; then
    sed -i "s|^Exec=.*|Exec=$PREFIX/bin/celestine %F|" "$DESKTOP"
    sed -i "s|^TryExec=.*|TryExec=$PREFIX/bin/celestine|" "$DESKTOP"
  fi
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
  fi
  echo "Installed to $PREFIX/bin/celestine"
fi

echo
echo "Done. Ensure $PREFIX/bin is on your PATH, then:"
echo "  celestine ~/Notes"
