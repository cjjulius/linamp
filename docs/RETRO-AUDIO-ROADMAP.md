# Retro Audio Roadmap

Goal: make LinAmp a one-stop destination for audio. Drop in anything, past or present,
and get sound out. This document is the plan for widening format coverage, with special
attention to the retro and chiptune world, and to how we get or generate a test file for
every new format so both the Linux and Windows machines can QA it.

## Guiding principles (the mantra)

1. Support as many audio decoding and tagging options as possible.
2. Rust is the ideal implementation.
3. Permissive licenses (MIT/BSD/Apache/MPL/ISC/Zlib) are ideal.
4. Failing 2, a low-level language (C, and by extension C++) is acceptable.
5. Failing 3, a copyleft or attribution license (GPL/LGPL, CC-BY) is acceptable.
6. Failing all the above: never a proprietary license, and never a high-level language
   such as Python.

Decision order for any new format: pure-Rust permissive crate first; then a C/C++ library
(permissive, else GPL/LGPL); never a proprietary SDK. If the only decoder is proprietary,
it is a wall: file a wontfix issue and move on (see Walls).

## How a decoder plugs in

Every format reaches playback through one of two shapes, both ending as a seekable
`rodio::Source`:

- Side-decoder crate: a module in `src/` (see `mpc.rs`, `dsd.rs`, `amr.rs`, `tracker.rs`,
  `tta.rs`, `wavpack.rs`, `ape.rs`) that wraps a Rust crate or an FFI binding and exposes an
  `is_<fmt>` predicate plus an `open()` returning a `Source`.
- FFmpeg path: `src/ffdec.rs`, behind the `ffmpeg` feature, for anything FFmpeg already
  decodes. Adding a format here is usually just extending the `is_ffmpeg_audio` extension
  list.

Wiring, in all cases:
- `src/<mod>.rs` or `src/ffdec.rs`: the predicate and decoder.
- `src/playlist.rs`: add the extension(s) to `SUPPORTED_EXTS` and to the router table; the
  `supported_exts_match_the_router` guard test keeps the predicate, the list and the router
  in agreement.
- `src/player.rs`: route the file to the decoder (the ffmpeg path is already wired; a new
  side-crate needs a branch here).
- `src/fileassoc.rs`: a MIME type and a `mime_package_xml` entry so the desktop offers
  LinAmp for the new extension.
- A decode test plus a QA sample and oracle (see QA strategy below).

Native C/C++ libraries follow the minimal-FFmpeg precedent (`packaging/linux/build-ffmpeg-min.sh`):
build a trimmed shared library from source, bundle it next to the binary with an
`$ORIGIN/lib` rpath, so the single installer stays self-contained. Prefer a maintained Rust
binding crate over hand-rolled FFI where one exists.

## Phase 1 - FFmpeg freebies (route-only, zero new dependency)

Decoders already compiled into the FFmpeg we bundle, not yet routed. Each is an
extension-list change plus a sample and oracle. Confirmed present in the bundled
`libavcodec`/`libavformat`:

- Shorten (`.shn`) - classic lossless, large in the live-recording trading scene.
- MPEG-4 ALS (`.als`) - lossless.
- TwinVQ (`.vqf`) - Yamaha/NTT late-90s lossy.
- CRI ADX (`.adx`) - very common game audio.
- Amiga 8SVX / IFF (`.8svx`, `.iff`).
- Creative Voice (`.voc`), Sun/NeXT (`.au`, `.snd`), Sony Wave64 (`.w64`), RF64.
- QuickTime QDesign (`.qdm2` content inside `.mov`, already reachable), Nellymoser (Flash;
  also enables demuxing `.swf` for embedded audio).
- Classic Mac MACE (`.mace`), TrueSpeech, MetaSound.
- Telephony/speech: GSM (`.gsm`), G.729, QCELP, EVRC, iLBC.
- Game FMV audio: Smacker, Interplay, RoQ, PAF, and the broad ADPCM/DPCM family.

Approach: these are lossy or lossless codecs FFmpeg already decodes, so they route through
`ffdec`. Group them into one branch. Skip anything we cannot get a sample for (same rule we
used to hold back ATRAC9).

## Phase 2 - Retro multipliers (the marquee)

Each library below adds a whole family of formats. This is what turns LinAmp into a
retro audio museum. All are C/C++ (principle 4) with acceptable licenses (principle 3 or 5).
Integrate each as a new side-crate wrapping a Rust binding (preferred) or thin FFI, bundled
like the minimal FFmpeg.

### 2a. Game Music Emu (console chiptunes)
- Adds: NSF/NSFE (NES), SPC (SNES), GBS (Game Boy), VGM/VGZ (Genesis, Master System,
  arcade), AY (ZX Spectrum, Amstrad CPC), GYM (Genesis), HES (PC Engine), KSS (MSX),
  SAP (Atari 8-bit).
- Language/License: C++ / LGPL-2.1 (principle 5). Rust bindings exist; evaluate the `gme`
  crate family, else FFI to `libgme`.
- Notes: these are emulated sound chips driven by a register/music log. Many have multiple
  subsongs per file, which the player UI will eventually need to expose (track index). Start
  with first-subsong playback.

### 2b. libopenmpt (the tracker zoo)
- Adds: roughly 30 module formats beyond our current xmrs set (mod/xm/s3m/it). Includes
  `.mptm`, `.med` (OctaMED), `.669`, `.far`, `.okt`, `.dbm`, `.psm`, `.mt2`, `.ult`,
  `.ams`, `.dmf`, `.dsm`, `.gdm`, `.imf`, `.j2b`, `.mdl`, `.mtm`, `.ptm`, `.stm`, and more.
- Language/License: C++ / BSD-3-Clause (principle 3, ideal among the native options).
- Notes: libopenmpt would supersede the pure-Rust `xmrs` path for breadth. Decide whether to
  replace xmrs or route only the extra formats to libopenmpt and keep xmrs for the four it
  already handles. Replacing is cleaner long-term.

### 2c. AdPlug / libADLMIDI (AdLib / OPL FM synthesis)
- Adds: DRO (DOSBox OPL capture), IMF (id Software), RAD, and game MIDI wrappers HMI/HMP/XMI,
  plus Doom-style MUS. The AdLib/OPL FM sound of early-90s PC games.
- Language/License: C++ / LGPL (AdPlug), mixed MIT/LGPL (libADLMIDI). Verify bank licensing.
- Notes: AdPlug alone supports dozens of AdLib container formats. libADLMIDI adds MIDI-to-OPL
  synthesis. Consider AdPlug first for file coverage.

### 2d. libsidplayfp (Commodore 64 SID)
- Adds: `.sid` (PSID/RSID) - the entire C64 music catalog (HVSC).
- Language/License: C++ / GPL-2 (principle 5).
- Notes: cycle-exact 6502 + SID emulation. Subsongs as in GME. GPL is acceptable but note it
  is the most restrictive license we would take; keep it isolated behind a feature so the
  license surface is explicit.

### Phase 2 stretch: emulated-console song formats
The `*SF` family (PSF/PSF2 PlayStation, GSF Game Boy Advance, 2SF DS, USF N64, SSF Saturn)
and vgmstream (hundreds of game-engine audio formats: HCA, FSB, Wwise WEM, GameCube DSP,
PS VAG, Nintendo BRSTM/BCSTM). High coverage, but heavier and with murkier licensing
(vgmstream bundles several libraries; audit before adopting). Defer until 2a-2d land.

## Phase 3 - Modern and ideal-fit

- QOA (Quite OK Audio, `.qoa`): a 2023 format, dead simple, MIT/public-domain, with a
  pure-Rust decoder already available (`qoaudio`). This is the only candidate that satisfies
  every principle at once. Low effort, good signal that we track new formats.
- Tagging: `lofty` 0.21 is already a dependency. Audit which of the new formats it can tag
  and widen read/write coverage as formats land (principle 1 covers tagging, not just decode).

## Phase 4 - Non-codec audio sources (the "drop anything" glue)

- CUE sheets (`.cue`): split a single-file album (with `.flac`/`.ape`/`.wv`) into real
  tracks. Large UX win for ripped albums; no new codec, just index + seek.
- Playlists: `.m3u`/`.m3u8`, `.pls`, `.xspf`, `.asx`, and `.ram` (RealAudio metafile).
- Internet radio: SHOUTcast/Icecast streaming (existing issue #146, the minibrowser pane).

## Walls (proprietary - do not implement)

File a short wontfix issue and move on. Known: AC-4 (#232), OptimFROG (#233), plus LA
(Lossless Audio), RKAU, MQA, LDAC, and the encoder side of Sony 360RA / MPEG-H.

## QA sample strategy (read this before implementing any format)

Every format needs a test file in `qa-music/format-samples/` (gitignored, per-box) and,
where possible, a decode oracle in `oracles/`. The decode test loads our output and compares
it against the oracle by correlation, exactly as the ffmpeg-backed tests already do. Keep
samples short (tens of seconds) so the debug test suite stays fast.

Two ways to get a sample:

- Generate it with an open-source tool (preferred: reproducible on both machines).
- Procure it from a known archive when no encoder exists (document the exact URL).

Oracle: decode the sample with the reference library's own CLI to raw f32, store that, and
compare. If no CLI oracle is practical, fall back to a smoke test (decodes to non-silent
audio of plausible length), as we do for the short DSD tone.

### Per-family acquisition and generation

Phase 1 (FFmpeg freebies):
- Oracle for all: `ffmpeg -i sample.<ext> -t 20 -f f32le -ac 2 -ar <rate> oracles/<fmt>-oracle.f32`.
- Shorten, ALS, TwinVQ, ADX, 8SVX, VOC, AU, Wave64: no encoder in our ffmpeg for most, so
  procure from `https://samples.ffmpeg.org/` (A-codecs and the format-named directories).
  A few (ADPCM, 8SVX, some PCM wrappers) can be minted with ffmpeg; check `ffmpeg -encoders`.

Phase 2a (Game Music Emu):
- Generate: Furnace (open source, cross-platform tracker) exports VGM and many chip formats
  directly, so we can author our own short test tunes. FamiStudio (open source) exports NSF
  (NES). These give us clean, freely-licensed, reproducible samples.
- Procure: freely/CC-licensed chiptunes and homebrew rips where generation is impractical
  (SPC in particular needs an SNES driver; prefer a known CC-licensed SPC or a smoke test).
- Oracle: Game Music Emu ships a CLI; decode to wav and convert to f32. Where subsongs exist,
  pin subsong 0.

Phase 2b (libopenmpt):
- Generate/procure: the OpenMPT project and ModArchive host many Creative-Commons and
  public-domain modules across the exotic formats. Pick small CC0/CC-BY files per format.
- Oracle: `openmpt123 --render sample.<ext>` (writes a wav), then convert to f32. openmpt123
  ships with libopenmpt on both platforms.

Phase 2c (AdPlug / libADLMIDI):
- Generate: DRO can be captured from DOSBox OPL output; IMF/RAD can be authored with open
  trackers. Simpler: use AdPlug's own test corpus.
- Oracle: AdPlug ships `adplay`; render to wav, convert to f32.

Phase 2d (libsidplayfp):
- Generate/procure: tiny public-domain `.sid` tunes, or author a minimal one. HVSC is not
  freely licensed, so prefer a known public-domain composer or a smoke test.
- Oracle: `sidplayfp` can write a wav; convert to f32.

Phase 3:
- QOA: generate directly - the reference `qoaconv` tool converts a wav to `.qoa`. Oracle:
  decode back with `qoaconv` or the reference decoder.

### Briefing the Windows box

The Windows machine keeps its own copy of `qa-music/format-samples/` (gitignored, so it is
never in git and must be built locally on each box). After we implement a format here, post
to the coordination Slack (the self-DM, messages marked as from the Linux box) with:

- The format, the extension(s), and the commit.
- The exact sample recipe: the generator command or the procurement URL, verbatim.
- The oracle command.
- Any platform caveat.

All the generator tools named above (ffmpeg, openmpt123, Furnace, FamiStudio, Game Music Emu
CLI, sidplayfp, qoaconv) run on Windows as well, so the recipes are cross-platform; note any
path or packaging differences when they come up. The canonical, always-current copy of each
recipe lives in `qa-music/format-samples/README.md`; this roadmap is the plan, the README is
the cookbook.

## Status tracker

Phase 1 - FFmpeg freebies
- [ ] Shorten (.shn)
- [ ] MPEG-4 ALS (.als)
- [ ] TwinVQ (.vqf)
- [ ] CRI ADX (.adx)
- [ ] Amiga 8SVX (.8svx/.iff)
- [ ] Creative VOC (.voc), Sun AU (.au/.snd), Wave64 (.w64)
- [ ] Flash Nellymoser / .swf audio, QuickTime QDM2
- [ ] Speech: GSM (.gsm), G.729, QCELP, EVRC, iLBC (as useful)

Phase 2 - Retro multipliers
- [x] Game Music Emu (NSF/SPC/GBS/VGM/AY/GYM/HES/KSS/SAP) - merged to main.
- [x] libopenmpt (the tracker zoo) - `openmpt` feature, src/openmpt.rs (streaming FFI). ~28 of the
  ~30 formats sampled and passing the visualizer test (see cookbook); `.stx`/`.j2b`/`.umx` are
  supported but have no free QA sample yet. xmrs keeps mod/xm/s3m/it. Windows libopenmpt build and
  DLL bundling still pending on the Windows box.
- [ ] AdPlug / libADLMIDI (OPL FM, DRO/IMF/RAD, Doom MUS)
- [ ] libsidplayfp (.sid)

Phase 3 - Modern/ideal
- [ ] QOA (.qoa)
- [ ] Tagging coverage audit (lofty)

Phase 4 - Non-codec sources
- [ ] CUE sheets
- [ ] Playlists (m3u/pls/xspf/asx/ram)
- [ ] Internet radio (#146)

Done this cycle (context)
- [x] RealAudio/RealMedia, TAK, Sony ATRAC (ffdec), plus the corrupt-input decode-hang fix.
- [x] Walls documented: AC-4 (#232), OptimFROG (#233).

## Appendix: QA sample cookbook (every format)

The canonical per-format recipe for building the local QA corpus in `qa-music/format-samples/`.
That folder is gitignored (samples are large/per-box/often copyrighted), so each machine builds
its own set from these recipes - this appendix is how both the Linux and Windows boxes stay in
sync. Preference order per format: a real song transcoded from the CC0 clip "Hypno This" by Kevin
MacLeod (public domain); else generate; else procure (exact URL given). The `hypno-this.wav`
source clip is the shared base; `hypno-mono.wav` / `hypno-48k.wav` are variants.

Vet any new sample before relying on it with the all-codecs test (ignored/slow), which renders a
sample per extension and flags any that error or render silence:
`cargo test --features "ffmpeg gme" -- --ignored every_supported_codec_feeds_the_visualizer`
(Linux: `LD_LIBRARY_PATH=build/gme-min/lib`; Windows: libgme DLL on PATH).

### Core audio and containers (symphonia and pure-Rust side-decoders)
- Transcode from hypno-this.wav: `.mp3` (libmp3lame 192k), `.mp2`/`.mpa` (mp2), `.flac`, `.ogg`
  (Vorbis), `.opus`, `.m4a`/`.aac`, `.wav` (pcm_s16le), `.aiff` (pcm_s16be), `.eac3`/`.ec3` (E-AC-3),
  `.wv` (WavPack: stereo/mono/48k/24-bit/untagged variants), `.tta` (True Audio).
- Musepack `.mpc` (SV8): `mpcenc` (musepack-tools; ffmpeg has no MPC encoder). `.mp+`/`.mpp` are copies.
- Video-for-audio (a 320x240 test-pattern video + the audio track): `.mp4`/`.mov`/`.mkv` (H.264/AAC),
  `.webm` (VP8/Opus), `.ogv` (Theora/Vorbis), `.mov`. `.m4b`/`.m4v`/`.3gp` are MP4-family copies.
- Container-extract: MPEG-TS `.ts`/`.m2ts`/`.mts` (AAC/MP2/AC-3), AVI `.avi` (MP3/PCM/AC-3),
  FLV `.flv` (MP3/AAC), MPEG-PS `.mpg`/`.mpeg`/`.vob`/`.m2p` (MP2/AC-3).
- MP1 `.mp1`: ffmpeg has no Layer I encoder; make with mjpegtools `mp2enc -l 1` fed a real WAV on
  stdin (`apt-get download mjpegtools`, `dpkg -x`), or procure samples.ffmpeg.org/A-codecs/mp1-sample.mp1.
- MIDI `.mid`/`.midi`: a format-1 General MIDI song (exercises rustysynth + SoundFont).
- Trackers: `.mod`/`.s3m`/`.it` small valid modules (TagLib test data), `.xm` = xmrs examples/note.xm.
  Use real modules with audible notes, not empty templates (an empty module renders silence).

### FFmpeg-backed formats (crate::ffdec, `--features ffmpeg`)
Oracle for the decode tests: `ffmpeg -i <sample> -f f32le -ac 2 -ar <rate> oracles/<name>-oracle.f32`.
- Mint from hypno-this.wav: `.wma` (`-c:a wmav2`), `.dts` (`-c:a dca -strict -2`), `.thd`/`.mlp`
  (`-c:a truehd`/`mlp -strict -2`), `.ra` (`-ar 8000 -ac 1 -c:a real_144`, RA 1.0), `.adx`
  (`-c:a adpcm_adx`), `.voc`/`.w64` (`-c:a pcm_s16le`), `.au` (`-c:a pcm_s16be`).
- Procure (no encoder) from samples.ffmpeg.org:
  - Shorten `.shn`: `/A-codecs/lossless/luckynight.shn`
  - 8SVX `.8svx`: `/8svx/conect.8svx`
  - RealAudio Cook `.rm`: `/real/audiosig.rm` (audio-only), `/real/RLG2_19.rm` (cook + rv20 video)
  - TAK `.tak`: `/A-codecs/tak/lostchord-tak_2_2_0.tak` (2.x works; the 1.0 file is a negative
    fixture - our FFmpeg 4.4 demuxes but cannot decode it, and playlist::playable must reject it)
  - ATRAC: `.oma`/`.omg`/`.aa3` `/A-codecs/ATRAC3+/sonateno14op27-2.aa3` (one OpenMG/ATRAC3+
    container plays under all three extensions; use this audible sonata for every one. The
    other file there, at3p_sample1.oma, decodes correctly but is a very quiet test tone that
    sounds like wind on listening QA, so do not use it as the shown sample). `.at3`
    `/A-codecs/ATRAC3/SND0.AT3`, `.aea` `/A-codecs/ATRAC1/Christmas1970.aea`.
- AMR `.amr` samples.ffmpeg.org/A-codecs/amr/sample.amr; `.awb` = an AMR-WB file renamed (crate::amr
  dispatches by magic). DSD `.dsf` generated via sigma-delta; `.dff` tone procured.
- Not advertised (no sample): `.at9` (ATRAC9). Deferred: `.als` (only ALS-in-mp4), `.vqf` (TwinVQ).

### Chiptunes / console music (crate::gme, `--features gme`)
libgme emulates the sound hardware; no encoder, so procure from Modland
(modland.com/pub/modules/<Format>/<author>/...). One libgme build plays all. Pick a small file;
LinAmp plays subsong 0, so choose one whose first track has audio.
- NES `.nsf`: "Nintendo Sound Format" | Game Boy `.gbs`: "Gameboy Sound System" | PC Engine `.hes`:
  "HES" | Atari `.sap`: "Slight Atari Player" | SNES `.spc`: "Nintendo SPC" | Sega/arcade `.vgz`
  (gzipped VGM): "Video Game Music".
- The four formats Modland does not serve directly (all now sampled; recipes below):
  - `.ay` (ZX Spectrum / Amstrad CPC): Modland's AY dirs hold `.emul`/`.amad`/`.strc`, not raw `.ay`.
    Pull a raw `.ay` (magic `ZXAYEMUL`) from the chris-y/AY mirror of Project AY. The `games`
    tree is split into one subdirectory per leading letter, so the path carries that letter:
    `https://raw.githubusercontent.com/chris-y/AY/main/music/ay/games/<letter>/<name>.ay`
    (URL-encode spaces and `[]`). `a/ATF(AdvancedTacticalFighter).ay` is single-track and loud
    (subsong 0 audible).
  - `.gym` (Sega Genesis): Modland's "Megadrive GYM" files are PACKED (magic `GYMX`), which libgme
    0.6.3 rejects with "Packed GYM file not supported". Unpack to a raw register dump libgme accepts:
    strip the 428-byte GYMX header, then zlib-decompress the body when the uint32 at header offset
    424 (compressed size) is nonzero. Write out just the decompressed stream (no GYMX header):
    `python3 -c 'import struct,zlib,sys; d=open(sys.argv[1],"rb").read(); assert d[:4]==b"GYMX";
    n=struct.unpack_from("<I",d,424)[0]; b=d[428:]; open(sys.argv[2],"wb").write(zlib.decompress(b) if n else b)'
    packed.gym gym-sample.gym`
    Pick an actual game soundtrack, not a utility: the sample in use is
    `Megadrive GYM/_unknown/Sonic 2 Time Zone/sonic 2 time zone - title screen theme.gym` (a clear
    ~61s melody that is easy to QA). The earlier "Art Alive" file was a drawing program's sparse,
    noise-like audio and hard to judge by ear.
  - `.kss` (MSX): most KSS rips have a silent subsong 0 and LinAmp plays subsong 0. Of the whole
    Modland "KSS" collection (17 authors), only `KSS/Airborne/nemesis gametune.kss` has an audible
    first track; use that until the subsong-selection feature lands.
  - `.nsfe` (NES extended): no sample located, so derive one from any `.nsf`. NSFe is a chunked
    (RIFF-style) container: `NSFE` magic then `<uint32 size><4cc><body>` chunks. Emit `INFO`
    (load/init/play addrs + region + chip + track count + 0-based start), a `BANK` chunk with the
    NSF header's 8 bankswitch bytes when any is nonzero (ours needs it), the `DATA` chunk (NSF bytes
    from 0x80), then an empty `NEND`. The result renders byte-identical audio to the source `.nsf`
    through libgme. Converter (`python3 nsf2nsfe.py nsf-sample.nsf nsfe-sample.nsfe`):

    ```python
    import struct, sys
    nsf = open(sys.argv[1], "rb").read()
    assert nsf[:5] == b"NESM\x1a"
    def chunk(cc, body): return struct.pack("<I", len(body)) + cc + body
    info = nsf[8:14] + bytes([nsf[0x7A], nsf[0x7B], nsf[6], max(0, nsf[7] - 1)])
    out = b"NSFE" + chunk(b"INFO", info)
    if any(nsf[0x70:0x78]): out += chunk(b"BANK", nsf[0x70:0x78])
    out += chunk(b"DATA", nsf[0x80:]) + chunk(b"NEND", b"")
    open(sys.argv[2], "wb").write(out)
    ```

### Tracker zoo (crate::openmpt, `--features openmpt`)
libopenmpt plays ~30 module formats beyond xmrs's mod/xm/s3m/it; one build plays all. There is no
encoder, so procure small modules per format from Modland (modland.com/pub/modules/<Format>/...),
one file per extension named `<ext>-sample.<ext>`. libopenmpt detects format by content, so the
on-disk extension only has to route the file to the decoder; a module stored under a different name
on Modland can be saved under the target extension. No oracle: libopenmpt is the only decoder, so
the test is a render smoke test (non-silent audio), same as the chiptunes. Prefer short tunes
(most Modland modules are full songs; the source streams, so length does not bloat the tests, but a
shorter file is nicer for manual QA). Format -> Modland directory used:
- `669` Composer 669 | `amf` Asylum | `ams` Velvet Studio | `c67` Composer 670 (CDFM) |
  `dbm` Digibooster Pro | `digi` Digibooster | `dmf` X-Tracker | `dtm` Digital Tracker DTM |
  `far` Farandole Composer | `gdm` General DigiMusic | `imf` Imago Orpheus | `mdl` Digitrakker |
  `med` OctaMED MMD0 (files are `.mmd0`; save as `.med`) | `mptm` OpenMPT MPTM |
  `mt2` Mad Tracker 2 | `mtm` Multitracker | `okt` Oktalyzer (files are `.okta`; save as `.okt`) |
  `plm` Disorder Tracker 2 | `psm` Epic Megagames MASI | `ptm` Polytracker |
  `stm` Screamtracker 2 | `stp` Soundtracker Pro II | `symmod` Symphonie | `ult` Ultratracker.
- `dsm`: use "Digital Sound Interface Kit RIFF" (the RIFF DSM), not the plain "Digital Sound
  Interface Kit" dir - the older DSIK `.dsm` files fail to load in libopenmpt 0.6.1.
- `c67`: the "Composer 670 (CDFM)" dir holds both `.670` and `.c67` files; take a `.c67` one
  (save as `c67-sample.c67`). The `.670` variant does not load in libopenmpt (tested 0.8.9).
- No free sample located yet (supported, untested here): `.stx` (STMIK), `.j2b` (Galaxy / Jazz
  Jackrabbit 2), `.umx` (Unreal/UT package). Procure from game data when available.

### Oracles
The `oracles/*.f32` and `*.s16` files are headerless raw-PCM decode references for the correlation
tests. They are not a playable format and are deliberately absent from SUPPORTED_EXTS.
