<!-- Maintainers: this is the user-facing format list. Keep it current as codecs are added. -->

# What LinAmp Plays

LinAmp plays a lot: your everyday music, the audio hiding inside a video file, and a deep back
catalog of retro game and computer music. The rule of thumb is simple. If you can find your
file's extension in one of the tables below, LinAmp can open it and play it.

Formats are grouped by what they actually are, not by the machinery behind them:

- [Everyday audio](#everyday-audio) - MP3, FLAC, and the usual suspects
- [Audio from video](#audio-from-video) - pull the soundtrack out of a video file
- [Retro audio](#retro-audio) - console and computer music, sorted by system
- [Other](#other) - MIDI files and audio CDs

One thing about the retro formats: they are not "decoded" the way an MP3 is. LinAmp emulates the
original sound chip and renders it fresh at 44.1 kHz stereo, so there is no bitrate to quote. A
lot of these files also hold several tunes in one file, and LinAmp plays the first one.

---

## Everyday audio

The files you probably have the most of.

### Lossy (compressed)

| Extension | Format | Good to know |
|---|---|---|
| `.mp3` `.mp1` `.mp2` `.mpa` | MPEG audio (Layer I/II/III) | Every bitrate, CBR or VBR |
| `.aac` | AAC | The audio in most streaming and Apple files |
| `.m4a` | AAC or ALAC | Lossy AAC, or lossless ALAC (listed again below) |
| `.ogg` `.oga` | Ogg Vorbis | |
| `.opus` | Opus | |
| `.wma` | Windows Media Audio | Covers WMA 1, 2, Pro, and Lossless |
| `.mpc` `.mp+` `.mpp` | Musepack | |
| `.dts` | DTS | Including the DTS-HD extensions |
| `.ra` `.rm` | RealAudio / RealMedia | The old RealPlayer formats |
| `.oma` `.omg` `.aa3` `.at3` `.aea` | Sony ATRAC | MiniDisc and OpenMG (ATRAC1 / 3 / 3+) |
| `.amr` `.awb` | AMR | Voice recordings (narrowband and wideband) |
| `.spx` | Speex | Another speech codec |
| `.qoa` | QOA (Quite OK Audio) | A simple, modern lossy format, decoded in pure Rust |

### Lossless (nothing thrown away)

| Extension | Format | Good to know |
|---|---|---|
| `.flac` | FLAC | The popular lossless choice, up to 24-bit |
| `.wav` | WAV | Uncompressed PCM |
| `.aiff` `.aif` | AIFF | Uncompressed PCM, common on Mac |
| `.ape` | Monkey's Audio | |
| `.wv` | WavPack | |
| `.tta` | True Audio | |
| `.tak` | TAK | Up to 24-bit |
| `.shn` | Shorten | Common in live-concert archives |
| `.w64` | Sony Wave64 | A WAV variant for very long recordings |
| `.thd` `.mlp` | Dolby TrueHD / MLP | Multichannel, mixed down to stereo |
| `.dsf` `.dff` | DSD / SACD | The 1-bit Super Audio CD format |
| `.au` | Sun / NeXT audio | |
| `.caf` | Apple Core Audio | Plays whatever audio the file holds |

---

## Audio from video

Have a video but only want the sound? Drop it in. LinAmp reads the audio track straight out of
these containers and plays it like any other song.

| Extension | Container | Good to know |
|---|---|---|
| `.mp4` `.m4v` `.mov` `.m4b` `.3gp` | MPEG-4 / QuickTime | Plays the main audio track |
| `.mkv` `.webm` | Matroska / WebM | |
| `.ogv` | Ogg video | |
| `.avi` | AVI | |
| `.flv` | Flash Video | |
| `.mpg` `.mpeg` `.vob` `.m2p` | MPEG Program Stream | DVD-style discs and rips |
| `.ts` `.m2ts` `.mts` | MPEG Transport Stream | Blu-ray and broadcast recordings |
| `.ac3` `.eac3` `.ec3` | Dolby Digital / E-AC-3 | On its own, or lifted from a video |

---

## Retro audio

This is where LinAmp goes deep: chip music and tracker tunes from decades of consoles, home
computers, and PC games.

### Game consoles and home computers

Sound-chip music, grouped by the machine it came from.

#### Nintendo

| Extension | System | Good to know |
|---|---|---|
| `.nsf` `.nsfe` | NES / Famicom | `.nsfe` just adds extra track info |
| `.gbs` | Game Boy | |
| `.spc` | SNES / Super Famicom | |

#### Sega

| Extension | System | Good to know |
|---|---|---|
| `.vgm` `.vgz` | Genesis / Mega Drive, Master System, arcade | `.vgz` is the zipped version of `.vgm` |
| `.gym` | Genesis / Mega Drive | |

#### NEC

| Extension | System |
|---|---|
| `.hes` | PC Engine / TurboGrafx-16 |

#### Atari

| Extension | System |
|---|---|
| `.sap` | Atari 8-bit |

#### Sinclair / Amstrad

| Extension | System |
|---|---|
| `.ay` | ZX Spectrum, Amstrad CPC |

#### MSX

| Extension | System |
|---|---|
| `.kss` | MSX |

#### Commodore 64

| Extension | System | Good to know |
|---|---|---|
| `.sid` `.psid` `.rsid` | Commodore 64 | The SID chip, emulated. Plays without the original C64 ROMs, which covers most tunes |

### PC: AdLib and Sound Blaster (OPL FM)

The bleepy FM-synth sound of early-90s DOS games, from the AdLib and Sound Blaster cards. Nearly
40 formats in all, from raw DOSBox captures to the music drivers of individual games. The common
ones:

| Extension | Format | Good to know |
|---|---|---|
| `.dro` | DOSBox Raw OPL | A recording of what a game sent to the OPL chip |
| `.cmf` | Creative Music File | Shipped with early Sound Blaster cards |
| `.rad` | Reality AdLib Tracker | |
| `.hsc` | HSC AdLib Composer | |
| `.laa` | LucasArts AdLib | LucasArts adventure games |
| `.d00` | EdLib | |
| `.a2m` | AdLib Tracker 2 | |
| `.adl` | Westwood ADL | Westwood Studios games |
| `.mus` | Doom DMX | id Software's OPL music |
| `.wlf` | Wolfenstein 3-D IMF | |
| `.lds` | Loudness | |
| `.amd` | AMUSIC | |
| `.cff` | BoomTracker | |
| `.dmo` | Twin TrackPlayer | |
| `.xad` | XAD | A wrapper around several AdLib sub-formats |
| `.bam` | Bob's AdLib Music | |
| `.bmf` | Easy AdLib | |
| `.mdi` | AdLib MIDI | |

Plus a long tail of game and tracker formats that work the same way:
`.got` `.agd` `.ha2` `.hsp` `.hsq` `.ims` `.jbm` `.mad` `.mkj` `.msc` `.rix` `.sa2` `.sat`
`.sdb` `.sng` `.sop` `.sqx` `.xsm` `.dfm`.

### Tracker modules

Pattern-based "module" music, born on the Amiga and everywhere in the demoscene since. These pack
their own instrument samples and a sequence of notes into one file.

The big four:

| Extension | Format | Good to know |
|---|---|---|
| `.mod` | ProTracker / SoundTracker | The original Amiga module |
| `.xm` | FastTracker 2 | |
| `.s3m` | ScreamTracker 3 | |
| `.it` | Impulse Tracker | |

And a whole zoo of about 28 more, for when you dig into the archives:

| Extension | Format |
|---|---|
| `.mptm` | OpenMPT |
| `.med` | OctaMED |
| `.far` | Farandole Composer |
| `.okt` | Oktalyzer |
| `.669` | Composer 669 |
| `.mtm` | MultiTracker |
| `.dbm` `.digi` | DigiBooster |

Plus: `.amf` `.ams` `.c67` `.dmf` `.dsm` `.dtm` `.gdm` `.imf` `.j2b` `.mdl` `.mt2` `.plm`
`.psm` `.ptm` `.stm` `.stp` `.stx` `.symmod` `.ult` `.umx` (Velvet Studio, X-Tracker, DSIK,
Imago Orpheus, MadTracker, Epic MASI, Symphonie, UltraTracker, Unreal, and more).

### Vintage sampled audio

A few older formats that store real recorded samples rather than chip instructions, so unlike
everything else in this section they are genuine recorded sound.

| Extension | Format | Good to know |
|---|---|---|
| `.8svx` | Amiga IFF 8SVX | 8-bit Amiga samples |
| `.voc` | Creative Voice | The Sound Blaster sample format |
| `.adx` | CRI ADX | Game audio (Dreamcast and arcade) |

### Game-engine streams

The streamed music and sound inside modern console and PC games, straight out of the formats the
game engines ship them in. Nothing to convert first, drop the file in and it plays.

| Extension | Format | Good to know |
|---|---|---|
| `.dsp` `.idsp` | GameCube / Wii ADPCM | Nintendo's console audio |
| `.brstm` `.bcstm` `.bfstm` `.bfwav` `.bwav` | Nintendo stream / wave | Wii, 3DS, Switch |
| `.vag` `.svag` | Sony PS-ADPCM | PlayStation audio |
| `.xvag` `.at9` | Sony XVAG / ATRAC9 | PS3 / PS4 / Vita |
| `.hca` | CRI HCA | A very common middleware format |
| `.fsb` | FMOD Sound Bank | The FMOD engine's audio bank |
| `.xwb` | Microsoft XACT | Xbox and PC games |
| `.hps` | HAL stream | GameCube (Smash Bros. and friends) |
| `.ktss` | Koei Tecmo stream | |
| `.wem` | Wwise encoded media | The Audiokinetic Wwise middleware, common in modern games (usually Vorbis) |
| `.xma` | Microsoft XMA | Xbox 360 audio (XMA1/2) |
| `.genh` | Generic header | A manual header for raw/headerless audio |

Many of these containers hold their audio in a codec that needs an extra decoder: Vorbis (Wwise `.wem`
and FMOD `.fsb`), or the FFmpeg-backed codecs (Xbox 360 XMA, PlayStation ATRAC3, WMA). LinAmp bundles
those decoders (libvorbis, and the same LGPL FFmpeg it uses elsewhere), so these now play rather than
being skipped. A file in a codec still not bundled is skipped rather than played wrong.

### Console program music (the *SF family)

Instead of a recording, these files carry the game's little music program (and its instrument data)
and LinAmp emulates the console's sound hardware to play it, just like the chip formats above. All are
played without any console BIOS or ROM. A `.mini*` variant wants its shared library file (`.psflib`,
`.ssflib`/`.dsflib`, `.qsflib`, `.ncsflib`, `.usflib`, `.2sflib`, `.gsflib`) sitting beside it.

| Extension | System | Good to know |
|---|---|---|
| `.psf` `.minipsf` | PlayStation | The SPU, emulated |
| `.ssf` `.minissf` | Sega Saturn | The SCSP sound chip, emulated |
| `.dsf` `.minidsf` `.dcsf` `.minidcsf` | Sega Dreamcast | The AICA sound chip, emulated (a `.dsf` is told apart from a DSD audio file by its contents) |
| `.qsf` `.miniqsf` | Capcom CPS-2 / ZN (QSound) | The QSound arcade chip, emulated |
| `.ncsf` `.minincsf` | Nintendo DS | The DS Nitro sound sequence, played back |
| `.usf` `.miniusf` | Nintendo 64 | The N64 RSP audio, emulated (High-Level Emulation, no boot ROM) |
| `.2sf` `.mini2sf` | Nintendo DS | The DS hardware, emulated - runs the game's own sound driver, for titles NCSF cannot cover |
| `.gsf` `.minigsf` | Game Boy Advance | The GBA hardware, emulated (mGBA with its HLE BIOS, no boot ROM) |

---

## Other

| Source | What it does |
|---|---|
| MIDI (`.mid` `.midi`) | Played through a built-in SoundFont, so you do not need any external synth |
| Audio CD | Pops in an audio CD and plays it straight from the drive |
| CUE sheet (`.cue`) | Splits a single-file album (one big `.flac`/`.ape`/`.wv`/`.wav` plus a `.cue`) into its real tracks |
| Playlists (`.m3u` `.m3u8` `.pls` `.xspf` `.asx` `.ram`) | Load a playlist file and its tracks are added to yours |
| Internet radio | Open a stream URL (Ctrl+L) and play a SHOUTcast/Icecast station, with live track titles |

---

## On the roadmap

- More game-engine codecs for the containers vgmstream routes (as upstream support lands).
- PSF2 is not planned: the only ROM-less engine renders it silent, and a working one would need the
  copyrighted PS2 BIOS. (The rest of the `*SF` program-music family is now supported.)
- An in-app internet-radio station directory (the streaming engine itself already works).
- More AdLib MIDI-wrapper formats (XMI, HMI, HMP) may follow.
- Wider tag editing for more of the formats above.
