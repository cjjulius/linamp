# FFmpeg in LinAmp: why it is here, whether it is the best option, and what else we could do

A research note on how LinAmp decodes audio, the one place FFmpeg is used, and how
to reach the goal of "play as many formats as possible" while keeping a pure-Rust
core, permissive licensing, and a single installer with everything inside it.

## TL;DR recommendation

1. FFmpeg is used in exactly one place: the optional `ffmpeg` feature (`src/ffdec.rs`),
   for Windows Media Audio (.wma) only. Everything else decodes without it.
2. It is the only dependency that links dynamically (system `libav*` DLLs). That one
   fact is what forces a separate WMA build and the DLL bundling, and it is the whole
   of the "extra DLLs / second installer" pain.
3. We already ship C libraries statically linked into the single binary with no DLLs:
   libopus (BSD) for Opus, and LAME (LGPL) via `mp3lame-encoder` for MP3 encoding. So
   a statically linked, LGPL, C audio library in one binary is already an accepted,
   shipping pattern here. FFmpeg is the odd one out only because it links dynamically.
4. Phased plan:
   - Now: ship one installer that includes WMA, by making the FFmpeg-enabled build the
     release and either bundling the DLLs (already wired in `linamp.iss`) or, better,
     static-linking FFmpeg so there is one binary and no DLLs.
   - Next: move WMA onto the pure-Rust `oxideav-wma` decoder when it can decode a real
     file end to end (it is close but not there yet), then drop FFmpeg for WMA.
   - Later: add the other "wall" formats (DTS, RealAudio, ATRAC) through the same
     pure-Rust side-decoder pattern as they mature, at which point FFmpeg can be
     dropped entirely.

## How decoding is wired today

The core is `symphonia` 0.6 (MPL-2.0), pulled with only the codecs we use. Everything
`symphonia` cannot do is handled by a pure-Rust "side decoder" that produces PCM and
is wrapped as a seekable `rodio::Source`, or by a pure-Rust demuxer that pulls the
audio stream out of a container and feeds it back through the existing decoders. Two C
libraries are vendored and compiled from source into the binary (no DLLs). FFmpeg is
the single optional, dynamically linked exception.

| Area | Formats | Decoder / demuxer | Crate | License | Linking |
| --- | --- | --- | --- | --- | --- |
| Core | mp1, mp2, mp3, aac, m4a/mp4, mkv/webm, caf, ogg/vorbis, flac, wav, aiff, pcm, alac, adpcm | symphonia | `symphonia` | MPL-2.0 | pure Rust |
| Opus | opus (in ogg/mp4/webm) | libopus via bindings | `opus` | BSD-3 (lib) / MIT (crate) | C, static (built from source) |
| APE | ape | pure Rust | `ape-decoder` | MIT/Apache | pure Rust |
| AC-3 / E-AC-3 | ac3, eac3 | clean-room pure Rust | `filmcraft-ac3` | MIT/Apache | pure Rust |
| WavPack | wv | pure Rust | `wavicle` | MIT/Apache | pure Rust |
| True Audio | tta | oxideav | `oxideav-tta` | MIT | pure Rust |
| Musepack | mpc, mp+, mpp | oxideav | `oxideav-musepack` | MIT | pure Rust |
| MIDI | mid, midi | rustysynth + SoundFont | `rustysynth` | MIT | pure Rust |
| Trackers | mod, xm, s3m, it | xmrs | `xmrs`/`xmrsplayer` | MIT/Apache | pure Rust |
| MPEG-TS | ts, m2ts, mts | demux, reuse decoders | `mpeg2ts-reader` | MIT/Apache | pure Rust |
| AVI | avi | demux, reuse decoders | `filmcraft-avi` | MIT/Apache | pure Rust |
| FLV | flv | demux, reuse decoders | `scuffle-flv` | MIT/Apache | pure Rust |
| MPEG-PS | mpg, mpeg, vob, m2p | demux, reuse decoders | `mpeg-ps` + our MPEG-1 fix | MIT/Apache | pure Rust |
| MP3/Vorbis/FLAC/Opus encode (rip/convert) | - | LAME / vorbis / flac / opus | `mp3lame-encoder`, `vorbis_rs`, `flacenc`, `opus` | LGPL (LAME) + others | C static (LAME, libopus); rest pure Rust |
| **FFmpeg (optional)** | **wma** (and any other libav codec) | **libavcodec/format/util + swresample** | **`ffmpeg-next`** | **LGPL** | **C, dynamic (system DLLs)** |

Two things stand out. First, the architecture is already a decoder-per-format mosaic,
not a monolith, so adding or swapping one format touches one module. Second, the only
non-pure-Rust, non-static piece is FFmpeg, and it exists for one format.

## Why FFmpeg is wired the way it is

WMA has no mature permissive pure-Rust decoder (yet, see below), and `symphonia` does
not decode it. WMA is a real ask on Windows, where people have .wma libraries. The
honest options at the time were: ship nothing for WMA, or lean on `libavcodec`, which
decodes WMA (and DTS, RealAudio, ATRAC, TAK, and more) well. The choice was to put it
behind an off-by-default `ffmpeg` feature so:

- the normal build stays pure Rust and ships nothing under FFmpeg's license, and
- a packager who wants WMA opts in and takes on the `libav*` dependency.

`ffdec.rs` is deliberately small and format-agnostic: it demuxes with FFmpeg, decodes
the best audio stream, resamples to interleaved stereo f32 with swresample, and wraps
the result as a seekable source, the same shape as the APE/WavPack/TTA side decoders.
It is vetted against an ffmpeg-CLI decode of the same file (RMS under 1e-4). The design
is sound. The friction is entirely in how FFmpeg is obtained and linked, not in the
module.

## The problem this creates

`ffmpeg-next` links the system `libav*` libraries dynamically. On Windows that means
avcodec/avformat/avutil/swresample DLLs must sit next to `linamp.exe` or on PATH. On
Linux it means the FFmpeg runtime must be installed. Consequences:

- WMA is a second, opt-in build, not the default.
- A WMA-enabled Windows installer has to carry the DLLs (now wired in `linamp.iss`
  behind `/DWithFfmpeg`), and a non-WMA installer does not.
- The build needs the FFmpeg dev libraries plus LLVM/libclang (bindgen) to compile.

This is the "get other DLLs or use a different installer" overhead. None of it is
inherent to supporting WMA. It is inherent to linking FFmpeg dynamically.

## The key precedent we already rely on

LinAmp already compiles C audio libraries from source and links them statically into
the one binary, with no DLLs and no second installer:

- `opus` builds libopus (BSD-3) from source (needs cmake at build) and static-links it.
- `mp3lame-encoder` builds LAME (LGPL) and static-links it.

LAME is LGPL and is already static-linked and shipped in the single binary. LGPL static
linking is satisfied here because LinAmp is open source: the LGPL's requirement that a
user be able to relink against a modified version of the library is met by the project
source being available to rebuild. The same reasoning applies to FFmpeg's LGPL `libav*`.
So "static LGPL C audio code in one binary" is not a new risk for this project; it is
the status quo for LAME. FFmpeg is linked dynamically today only by default of how
`ffmpeg-next` is set up, not by any licensing necessity.

## Options for the single-installer goal (ranked)

### A. Ship the FFmpeg build as the release, DLLs bundled in the one installer

Make the `--features ffmpeg` build the shipped build and bundle the `libav*` DLLs
inside the single installer (already supported in `linamp.iss` via `/DWithFfmpeg`
`/DFfmpegBin=...`). The user runs one installer and gets WMA; no separate downloads.

- Pros: zero new code; available today; maximal format coverage immediately (FFmpeg
  also gives DTS, RealAudio, ATRAC, TAK the moment it is in). Satisfies "one installer,
  everything inside."
- Cons: the binary is accompanied by DLLs in the install dir (not a single file);
  larger install; LGPL applies (trivially satisfied by dynamic linking plus shipping
  the unmodified `libav*` and their license). Still a C/LGPL dependency, counter to the
  pure-Rust preference.
- Licensing: fine. Dynamic LGPL with the libraries shipped alongside and their license
  included is the textbook-compliant case.

### B. Static-link FFmpeg into one binary (no DLLs)

Build `libavcodec/avformat/avutil/swresample` as static libraries (configured LGPL,
no GPL components such as x264/libass) and link them into `linamp.exe`, exactly as we
already do for LAME and libopus. On Windows the vcpkg `x64-windows-static-md` triplet
is the usual route; point `FFMPEG_DIR` at the static build. Verify with `dumpbin
/dependents` that no `avcodec-*.dll` remains.

- Pros: one self-contained binary, no DLLs, consistent with how LAME/libopus already
  ship; one installer with nothing extra. Keeps full FFmpeg format coverage.
- Cons: more Windows build plumbing (static FFmpeg is fiddly; the `ffmpeg-sys-next`
  static path has known rough edges on Windows, for example the final link needing
  extra system libs); larger binary; still C/LGPL. Must avoid `ffmpeg-next`'s `build`
  feature path that pulls GPL components, which would make the binary GPL.
- Licensing: LGPL static, satisfied the same way LAME already is (open-source relink).
  Configure the FFmpeg build as LGPL-only to stay out of GPL.

### C. Move WMA to pure-Rust `oxideav-wma`, then drop FFmpeg for WMA

`oxideav-wma` (MIT, same `OxideAV` family as the `oxideav-tta` and `oxideav-musepack`
decoders we already ship) is a clean-room WMA decoder. When it can decode a real WMA
file end to end, wrap it as a side decoder like TTA/Musepack and remove the FFmpeg path
for WMA. Result: pure Rust, MIT, single static binary, no DLLs, one installer.

- Pros: the ideal end state on every axis (pure Rust, permissive, single binary). Fits
  the existing architecture exactly, and we already trust two crates from this family.
- Cons: not ready today (see the maturity note below). Covers WMA v1/v2 first; WMA Pro
  and WMA Lossless come later, so FFmpeg may need to linger (optional) for those until
  the crate catches up, or we accept a Pro/Lossless gap.
- Effort: integration is small (mirror `crate::tta`); the gating work is the crate
  maturing, which is outside our tree but trackable, and which we can accelerate by
  contributing and by vetting against the oracle we already have in `ffdec.rs` tests.

### D. Build our own WMA decoder (the "use an existing decoder as a template" idea)

Two shapes:

- Clean-room, from the format description and a reference for understanding only,
  writing fresh code. This is exactly what `filmcraft-ac3`, `oxideav-tta`,
  `oxideav-musepack`, and `oxideav-wma` do. Output can be MIT. This is the
  highest-quality route and, for WMA specifically, is already underway as
  `oxideav-wma`, so the pragmatic version of "build our own" is "back/track
  `oxideav-wma`," not start from zero.
- Direct port of FFmpeg's `wmadec.c` to Rust. A line-by-line port of LGPL code is a
  derivative work and stays LGPL, so the crate would be LGPL, acceptable as a static
  LGPL dependency (same footing as LAME) but less clean than the MIT clean-room route.
  Faster to get working than clean-room, but it carries FFmpeg's license into our tree.

Note on "using an encoder as a template": an encoder does not give you a decoder for
free; the decoder is driven by the bitstream format plus the inverse transforms. The
real template for a decoder is the format specification plus a reference decoder, which
is what the clean-room crates use.

### E. Vendor a small standalone C decoder, static-linked (the libopus model)

If a compact, standalone WMA decoder in C existed under a license we accept, we could
vendor and static-link it exactly like libopus. In practice no such clean standalone
WMA decoder exists; the real C implementation is `libavcodec`, which is option A/B. So
this reduces to A/B and is not a separate path for WMA. It remains the model to keep in
mind for any future format where a small permissive C decoder does exist.

## The "walls" and the maximize-coverage goal

Formats with no mature permissive pure-Rust decoder, and the state of play. The pattern
is that `OxideAV` is systematically clean-rooming these, which is the same source we
already depend on for TTA and Musepack.

| Format | Pure-Rust status | FFmpeg covers it | Notes |
| --- | --- | --- | --- |
| WMA v1/v2 | `oxideav-wma` 0.0.4, in progress (header + DSP/entropy primitives done; end-to-end not proven) | yes | The one with real user demand. |
| WMA Pro / Lossless | planned in `oxideav-wma`, not started | yes | Later. |
| DTS (Core) | `oxideav-dts` 0.0.2, early | yes | Niche for a desktop player. |
| RealAudio (Cook) | `oxideav-cook` 0.0.0, placeholder | yes | Very niche. |
| ATRAC3+ | `atrac3p-decoder` / `atrac3p-decoder2`, independent, exists | yes | MiniDisc-era; niche. |
| TAK | none | yes | Niche lossless. |
| OptimFROG | none (closed format) | yes | Proprietary; realistically out of scope. |

Takeaway for goal #1 ("as many as possible"): FFmpeg, if shipped, instantly maximizes
coverage (WMA, DTS, RealAudio, ATRAC, TAK). The pure-Rust route reaches the same place
incrementally and more cleanly, led by `oxideav-*`. These are not in conflict: shipping
FFmpeg short-term and migrating to pure-Rust decoders as they mature is a coherent path
that keeps coverage high the whole way and lets us retire FFmpeg format by format.

## `oxideav-wma` maturity (why it is not a drop-in yet)

From the crate's own README (repo `OxideAV/oxideav-wma`, MIT, last updated 2026-09):
it is a "clean-room rebuild in progress," assembled from a multimedia.cx orientation
snapshot, a patents-only structural trace, and numeric tables extracted from the vendor
decoder's PE data sections. What works today: the WMA v1/v2 header parser
(`WAVEFORMATEX` extradata, the flags, the MDCT block-size decision tree) and a set of
individually tested DSP and entropy primitives (inverse MLT, windowing, overlap-add,
inverse quantization, run-level, codebooks). What is not yet claimed: a proven
end-to-end decode of a real WMA file to correct PCM. So the trajectory is right and the
license and framework are exactly what we want, but it cannot replace FFmpeg for actual
WMA playback right now. The test to graduate it is the one we already have: decode
`hypno-this.wma` and compare to the ffmpeg oracle (`ffdec.rs` tests) within RMS 1e-4.

## Recommendation (phased)

- Phase 0, now, to kill the single-installer pain: decide between A (bundle DLLs in the
  one installer, zero work, available today) and B (static-link FFmpeg, one binary, no
  DLLs, modest Windows build work and consistent with LAME/libopus). B is the better
  end state for packaging; A is the zero-risk stopgap. Either one gives the user a single
  installer that includes WMA, which is the stated goal. Recommend A immediately, with B
  as the packaging target once the static FFmpeg build is proven on Windows CI.
- Phase 1, medium term, to honor pure-Rust: track `oxideav-wma`; when it passes our
  oracle test for v1/v2, wrap it as a side decoder and drop FFmpeg for WMA v1/v2. Keep
  FFmpeg optional only for WMA Pro/Lossless (and the other walls) until covered.
- Phase 2, long term, to maximize coverage cleanly: adopt `oxideav-dts`,
  `oxideav-cook`, and `atrac3p-decoder` as they mature, via the same pattern. When the
  formats we care about are all pure Rust, remove FFmpeg entirely and the project is
  back to a single pure-Rust static binary with the widest coverage.
- Keep doing what already works: every side decoder is vetted bit-exact or RMS-close
  against an ffmpeg oracle before it ships. That method is why the pure-Rust mosaic is
  trustworthy, and it is exactly how to qualify each replacement.

## Licensing quick reference

- symphonia: MPL-2.0, file-level copyleft, fine to link.
- Pure-Rust side decoders and demuxers: MIT/Apache, no obligations beyond attribution.
- libopus: BSD-3; LAME: LGPL. Both already static-linked and shipped; LGPL static is
  satisfied by the project being open source (relinkable).
- FFmpeg `libav*`: LGPL when built without GPL components (no x264/libass/etc.). Dynamic
  linking is trivially compliant; static linking is compliant on the same open-source
  relink basis as LAME. Avoid the GPL build configuration, which would make the whole
  binary GPL.
- No proprietary code anywhere in any of these paths, which satisfies the hard "no
  proprietary" rule.
