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

# A bundled-FFmpeg build links WMA/DTS against a self-contained FFmpeg built from
# source (packaging/linux/build-ffmpeg-min.sh), not the distro's libavcodec: the
# distro build drags ~120 MB of external codec/network/crypto libraries, whereas the
# from-source one keeps the native codecs but links no external libraries, so its
# libav* depend only on libc/libm and the bundle is ~15 MB. FFMPEG_DIR points
# ffmpeg-sys-next at it. The $ORIGIN/lib rpath (DT_RPATH via --disable-new-dtags, so
# it also covers the libraries' own siblings) makes the binary load the bundled
# libav* from next to itself at runtime, so no system FFmpeg is needed.
if [ "$BUNDLE_FFMPEG" = 1 ]; then
  FFMPEG_MIN="$ROOT/build/ffmpeg-min"
  sh "$ROOT/packaging/linux/build-ffmpeg-min.sh" "$FFMPEG_MIN"
  export FFMPEG_DIR="$FFMPEG_MIN"
  export PKG_CONFIG_PATH="$FFMPEG_MIN/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,\$ORIGIN/lib -C link-arg=-Wl,--disable-new-dtags"
fi

echo "==> cargo build --release $FEATURE_ARGS"
# shellcheck disable=SC2086
cargo build --release $FEATURE_ARGS

echo "==> Staging $STAGE"
rm -rf "$STAGE"; mkdir -p "$STAGE"
install -m 0755 target/release/linamp "$STAGE/linamp"

if [ "$BUNDLE_FFMPEG" = 1 ]; then
  echo "==> Bundling the from-source FFmpeg libraries into $STAGE/lib (self-contained)"
  mkdir -p "$STAGE/lib"
  # The from-source FFmpeg (build-ffmpeg-min.sh) links only libc/libm, so the whole
  # runtime set is just its own libav*/libsw* libraries. Copy them straight from the
  # build prefix we linked against (not by ldd-walking the binary, which would resolve
  # against the host's full FFmpeg and drag its ~120 MB of external deps). The soname
  # symlinks are preserved; the binary's $ORIGIN/lib rpath loads them at runtime, and
  # the C runtime comes from the user's system.
  cp -a "$FFMPEG_DIR"/lib/libav*.so.* "$FFMPEG_DIR"/lib/libsw*.so.* "$STAGE/lib/" 2>/dev/null || true
  for f in "$STAGE"/lib/*.so.*; do [ -e "$f" ] && echo "    + $(basename "$f")"; done
  # Sanity check: the installed binary must resolve every libav* from the bundle.
  if ldd "$STAGE/linamp" 2>/dev/null | grep -E 'libav|libswresample' | grep -vq "$STAGE/lib"; then
    echo "!!  WARNING: a libav* still resolves outside $STAGE/lib; check the rpath" >&2
  fi
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
