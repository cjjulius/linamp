#!/usr/bin/env sh
# Per-user install of LinAmp into ~/.local (no sudo). Run from the extracted
# tarball directory:  ./install.sh
set -eu

SRC="$(cd "$(dirname "$0")" && pwd)"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
PREFIX="$DATA/linamp"              # binary + skins live together here
BINDIR="$HOME/.local/bin"
APPDIR="$DATA/applications"
ICONS="$DATA/icons/hicolor"

echo "Installing LinAmp to $PREFIX (per-user, no sudo)…"
mkdir -p "$PREFIX" "$BINDIR" "$APPDIR" \
         "$ICONS/256x256/apps" "$ICONS/scalable/apps"

# Binary + skins must sit together: the app finds skins next to the binary.
install -m 0755 "$SRC/linamp" "$PREFIX/linamp"
rm -rf "$PREFIX/skins"; cp -r "$SRC/skins" "$PREFIX/skins"
if [ -d "$SRC/music" ]; then rm -rf "$PREFIX/music"; cp -r "$SRC/music" "$PREFIX/music"; fi
for d in README.md FEATURES.md SHORTCUTS.md LINUX.md LICENSE; do
  [ -f "$SRC/$d" ] && cp "$SRC/$d" "$PREFIX/" || true
done

# Icons.
[ -f "$SRC/linamp_256.png" ] && cp "$SRC/linamp_256.png" "$ICONS/256x256/apps/linamp.png" || true
[ -f "$SRC/linamp.svg" ] && cp "$SRC/linamp.svg" "$ICONS/scalable/apps/linamp.svg" || true

# PATH symlink + desktop entry (with the install prefix substituted in).
ln -sf "$PREFIX/linamp" "$BINDIR/linamp"
sed "s|@PREFIX@|$PREFIX|g" "$SRC/linamp.desktop" > "$APPDIR/linamp.desktop"
chmod 0644 "$APPDIR/linamp.desktop"

# Refresh desktop/icon caches (best-effort).
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPDIR" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f -t "$ICONS" >/dev/null 2>&1 || true

echo "Done. Launch 'LinAmp' from your application menu, or run: $BINDIR/linamp"
case ":$PATH:" in
  *":$BINDIR:"*) ;;
  *) echo "Note: $BINDIR is not on your PATH — add it to run 'linamp' from a terminal." ;;
esac
