#!/usr/bin/env sh
# Remove a per-user LinAmp install. User data (library/config) is left intact.
set -eu

DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
PREFIX="$DATA/linamp"
BINDIR="$HOME/.local/bin"
APPDIR="$DATA/applications"
ICONS="$DATA/icons/hicolor"

echo "Removing LinAmp…"
rm -rf "$PREFIX"
rm -f "$BINDIR/linamp"
rm -f "$APPDIR/linamp.desktop"
rm -f "$ICONS/256x256/apps/linamp.png" "$ICONS/scalable/apps/linamp.svg"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPDIR" >/dev/null 2>&1 || true

echo "Done. Your library/config in $DATA/LinAmp was left intact — delete it manually to remove all data."
