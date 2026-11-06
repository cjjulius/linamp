#!/usr/bin/env sh
# Build the per-user Linux tarball: dist/linamp-<version>-linux-x86_64.tar.gz
# Run on Linux (or in CI). Requires the usual build deps (libasound2-dev,
# libgtk-3-dev, pkg-config) plus the Rust toolchain.
set -eu

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"

# Optional Cargo features. Set FEATURES to build a variant, example:
#   FEATURES=ffmpeg ./packaging/linux/build-tarball.sh   # adds WMA (links libav*)
# The default (no FEATURES) is the pure-Rust build. A featured build gets a name
# suffix so it never overwrites the default tarball, and needs the matching system
# libraries present (for ffmpeg: libavcodec/libavformat/libavutil/libswresample).
FEATURES="${FEATURES:-}"
if [ -n "$FEATURES" ]; then
  SUFFIX="-$(echo "$FEATURES" | tr ', ' '--')"
  FEATURE_ARGS="--features $FEATURES"
else
  SUFFIX=""
  FEATURE_ARGS=""
fi
NAME="linamp-${VERSION}-linux-x86_64${SUFFIX}"
STAGE="dist/$NAME"

echo "==> cargo build --release $FEATURE_ARGS"
# shellcheck disable=SC2086
cargo build --release $FEATURE_ARGS

echo "==> Staging $STAGE"
rm -rf "$STAGE"; mkdir -p "$STAGE"
install -m 0755 target/release/linamp "$STAGE/linamp"
cp -r skins "$STAGE/skins"
rm -rf "$STAGE/skins/backup"   # dev-only skin backups, not for release
cp -r soundfont "$STAGE/soundfont"   # MIDI SoundFont, the app finds it next to the binary
[ -d music ] && cp -r music "$STAGE/music" || true
cp packaging/linux/install.sh packaging/linux/uninstall.sh packaging/linux/linamp.desktop "$STAGE/"
chmod +x "$STAGE/install.sh" "$STAGE/uninstall.sh"
cp assets/linamp_logo_256.png "$STAGE/linamp_256.png"
cp assets/linamp_logo.svg "$STAGE/linamp.svg"
for d in README.md FEATURES.md SHORTCUTS.md LINUX.md LICENSE; do [ -f "$d" ] && cp "$d" "$STAGE/" || true; done

echo "==> Creating dist/$NAME.tar.gz"
( cd dist && tar czf "$NAME.tar.gz" "$NAME" )
echo "==> Done: dist/$NAME.tar.gz"
