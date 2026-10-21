#!/usr/bin/env sh
# Build the per-user Linux tarball: dist/linamp-<version>-linux-x86_64.tar.gz
# Run on Linux (or in CI). Requires the usual build deps (libasound2-dev,
# libgtk-3-dev, pkg-config) plus the Rust toolchain.
set -eu

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
NAME="linamp-${VERSION}-linux-x86_64"
STAGE="dist/$NAME"

echo "==> cargo build --release"
cargo build --release

echo "==> Staging $STAGE"
rm -rf "$STAGE"; mkdir -p "$STAGE"
install -m 0755 target/release/linamp "$STAGE/linamp"
cp -r skins "$STAGE/skins"
[ -d music ] && cp -r music "$STAGE/music" || true
cp packaging/linux/install.sh packaging/linux/uninstall.sh packaging/linux/linamp.desktop "$STAGE/"
chmod +x "$STAGE/install.sh" "$STAGE/uninstall.sh"
cp assets/linamp_logo_256.png "$STAGE/linamp_256.png"
cp assets/linamp_logo.svg "$STAGE/linamp.svg"
for d in README.md FEATURES.md SHORTCUTS.md LINUX.md LICENSE; do [ -f "$d" ] && cp "$d" "$STAGE/" || true; done

echo "==> Creating dist/$NAME.tar.gz"
( cd dist && tar czf "$NAME.tar.gz" "$NAME" )
echo "==> Done: dist/$NAME.tar.gz"
