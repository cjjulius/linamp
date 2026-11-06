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
# suffix so it never overwrites the default tarball. A build that links FFmpeg (the
# ffmpeg feature) also bundles the FFmpeg shared libraries below, so the install is
# self-contained and needs no system FFmpeg at runtime.
FEATURES="${FEATURES:-}"
if [ -n "$FEATURES" ]; then
  SUFFIX="-$(echo "$FEATURES" | tr ', ' '--')"
  FEATURE_ARGS="--features $FEATURES"
else
  SUFFIX=""
  FEATURE_ARGS=""
fi
case ",$FEATURES," in
  *,ffmpeg,*) BUNDLE_FFMPEG=1 ;;
  *) BUNDLE_FFMPEG=0 ;;
esac
NAME="linamp-${VERSION}-linux-x86_64${SUFFIX}"
STAGE="dist/$NAME"

# A bundled-FFmpeg build links with an $ORIGIN/lib rpath so the binary loads the
# bundled libav* from next to itself under lib/; --disable-new-dtags makes it a
# DT_RPATH, so the search also covers the bundled libraries' own dependencies (the
# codec libs libavcodec pulls), not just the binary's direct NEEDEDs.
if [ "$BUNDLE_FFMPEG" = 1 ]; then
  export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,\$ORIGIN/lib -C link-arg=-Wl,--disable-new-dtags"
fi

echo "==> cargo build --release $FEATURE_ARGS"
# shellcheck disable=SC2086
cargo build --release $FEATURE_ARGS

echo "==> Staging $STAGE"
rm -rf "$STAGE"; mkdir -p "$STAGE"
install -m 0755 target/release/linamp "$STAGE/linamp"

if [ "$BUNDLE_FFMPEG" = 1 ]; then
  echo "==> Bundling FFmpeg shared libraries into $STAGE/lib (self-contained WMA build)"
  mkdir -p "$STAGE/lib"
  # Copy the FFmpeg family and the codec libraries it pulls, resolved from the built
  # binary, so no system FFmpeg is needed at runtime. The C runtime and the host
  # desktop stack (X11/Wayland/GTK/GLib/ALSA/PulseAudio/OpenGL/fontconfig/openssl/
  # compression) are provided by the user's system and are deliberately NOT bundled,
  # to avoid ABI clashes. NOTE: validate the resulting library set on a real Linux
  # install; this deny-list is a heuristic and may need per-distro tuning.
  deny='ld-linux|/libc\.so|/libm\.so|/libdl\.so|/librt\.so|/libpthread\.so|/libresolv\.so|/libgcc_s\.so|/libstdc..\.so|/libX11|/libXau|/libXdmcp|/libXext|/libXcursor|/libXrender|/libXfixes|/libXi\.|/libXrandr|/libXinerama|/libxcb|/libwayland|/libxkbcommon|/libGLX|/libGLdispatch|/libGL\.so|/libEGL|/libOpenGL|/libgbm|/libdrm|/libgtk-3|/libgdk-3|/libglib-2|/libgobject-2|/libgio-2|/libgmodule-2|/libgthread-2|/libpango|/libcairo|/libatk|/libharfbuzz|/libfreetype|/libfontconfig|/libpng|/libjpeg|/libpixman|/libgdk_pixbuf|/libepoxy|/libasound|/libpulse|/libdbus-1|/libsystemd|/libudev|/libselinux|/libmount|/libblkid|/libffi|/libpcre|/libexpat|/libz\.so|/liblzma\.so|/libzstd\.so|/libbz2\.so|/libssl|/libcrypto'
  ldd "$STAGE/linamp" | awk '/=> \// {print $3}' | sort -u | grep -Ev "$deny" | while read -r so; do
    [ -f "$so" ] && cp -L "$so" "$STAGE/lib/" && echo "    + $(basename "$so")"
  done
fi
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
