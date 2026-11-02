# Winamp 2.666 parity

LinAmp's classic-mode target is Winamp 2.666. As of the v1.0 line the 2.666
functional feature set is complete: transport (play/pause/stop, stop-after-current,
stop-with-fade, next/previous, seek), the playlist (add/remove/crop/remove-missing/
reverse/sort, jump-to-file and jump-to-time, m3u/m3u8/pls), shuffle and repeat, the
10-band equalizer with presets, Auto gain and `.EQF` import/export, classic `.wsz`
skins including skinned cursors, the main / equalizer / playlist windows with
window-shade, double-size and always-on-top (positions and state persisted), the
visualizer (spectrum / oscilloscope / off), the clutterbar, volume and stereo
balance, drag-and-drop, bookmarks, Open-location streaming with the live ICY title,
tag reading and writing, the SQLite media library, audio CD playback and ripping,
and system integration (global media keys, a system-tray mode, and "Open with"
file associations). Audio CD is verified on Windows and Linux hardware.

Conversion and CD ripping (to MP3 / Ogg Vorbis / Opus / WAV / AIFF) go a little
past stock 2.666; they are shipped and noted here for completeness.

For how each change was verified see [REVIEW-NOTES.md](REVIEW-NOTES.md); the
keyboard and mouse reference is [SHORTCUTS.md](SHORTCUTS.md).

## Not yet matching 2.666

These are the only in-scope gaps left:

- #146, the minibrowser's SHOUTcast **directory** pane. The minibrowser window and
  "Open in browser" already work; the live directory listing is the remaining piece
  (and SHOUTcast's directory is largely defunct, so this is cosmetic parity).
- #209 / #210, window-shade close-button polish: a collapsed window should show the
  pressed close-X and act on release, matching the full windows.

## Audio formats

Local playback flows through one symphonia path (`src/decode.rs`), with
special-cased MIDI and tracker renderers:

- MPEG audio (`.mp3`, `.mp1` / `.mp2` / `.mpa`), FLAC, Ogg Vorbis, Opus, WAV, AIFF,
  CAF and the AAC / ISO-MP4 family (`.m4a` / `.m4b` / `.mp4` / `.aac`).
- The audio track of video containers: `.mp4` / `.m4v` / `.3gp` / `.mov`, `.mkv`,
  `.webm`, `.ogv`.
- MIDI (`.mid` / `.midi`) via a bundled SoundFont; tracker modules
  (`.mod` / `.xm` / `.s3m` / `.it`).

Converts / rips to MP3, Ogg Vorbis, Opus, WAV and AIFF. Not offered, by design:
AC-3 (#161) and WMA (#162) are won't-fix (proprietary / licensing); FLAC as a
convert *output* is deferred (#190); WavPack and Monkey's Audio (#182) have no
viable decoder in the current engine.

## Beyond 2.666 (tracked, out of scope)

Later-Winamp-line features and internal work, kept in the backlog until the above
is signed off:

- Media Library buildout (#78) and its pieces: saved playlists (#150), genre / year
  columns (#151), drag-to-editor (#152), album-art pane (#153), audio-CD cover art
  (#212).
- Tag-editor embedded album art (#174).
- Classic EQ drag-to-draw the response curve (#139).
- A true modern (Winamp 3/5 `.wal`) skin engine (#20).
- Gapless playback (#42) and ReplayGain (#44).
- Internal, no user-facing parity effect: collapse per-window state into arrays
  (#111); a native-Wayland single composite window for docking (#69).

The full backlog of record is
[GitHub Issues](https://github.com/cjjulius/linamp/issues).
