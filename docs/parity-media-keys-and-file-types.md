# Linux parity: media keys, file associations, and "Open with"

This describes the Windows behavior shipped on branch
`feat/media-keys-and-associations` so the Linux side can match it. It covers the
two system-integration features split from #40, global media keys (#165) and
audio file-type associations (#166), plus the "Open with" / command-line open
behavior that both share.

Current state: the Windows behavior below is implemented and verified. The
cross-platform parts (the open logic and the single-instance forwarding) already
run on Linux. The two Linux-specific backends (media keys over MPRIS, and the
`.desktop` registration) are not implemented yet; both are described here with
where the code should go.

Shared plumbing to reuse rather than reimplement:
- `TrayAction` (src/tray.rs) is the common transport vocabulary. The tray and the
  media keys both emit it, and `LinAmpApp::handle_tray_action` dispatches it to
  `play_pause` / `stop_playback` / `prev` / `next`.
- `LinAmpApp::open_paths` (src/app.rs) is the single place that applies the "Open
  with" preference. Both the launch path and the forwarding path call it, so the
  Enqueue/Clear behavior is identical however files arrive.


## 1. Global media keys (#165)

### Windows behavior (done)
- The four keyboard transport keys, Play/Pause, Stop, Previous and Next, control
  LinAmp globally: they work while LinAmp is unfocused, minimized, or hidden to
  the tray.
- Implemented in src/mediakeys.rs: a dedicated thread registers the keys with
  `RegisterHotKey` on `VK_MEDIA_PLAY_PAUSE`, `VK_MEDIA_STOP`,
  `VK_MEDIA_PREV_TRACK` and `VK_MEDIA_NEXT_TRACK` (with `MOD_NOREPEAT`, so holding
  a key fires once). Each `WM_HOTKEY` maps to a `TrayAction` and is sent on the
  same channel the tray uses, so it routes through `handle_tray_action`.
- A key already grabbed by another application is skipped; the rest still
  register.
- Always on, in both window modes. There is no preference for it.

### What parity on Linux means
- The same four actions respond to the desktop's media keys while LinAmp is
  unfocused.
- Route: MPRIS over D-Bus. Serve `org.mpris.MediaPlayer2` and
  `org.mpris.MediaPlayer2.Player` under the well-known name
  `org.mpris.MediaPlayer2.linamp`. The desktop delivers the keyboard media keys
  (and the system media controls) to the Player interface. Map its `PlayPause`,
  `Stop`, `Next` and `Previous` methods onto the same `TrayAction`s and call
  `egui.request_repaint()` on each so the app drains the channel promptly.
- A natural bonus of MPRIS (no Windows equivalent, but expected on Linux): LinAmp
  appears in the GNOME/KDE system media controls (lock screen, quick settings)
  with at least play/pause and track skip. Reporting current-track metadata and
  playback status there is a reasonable follow-up, not required for key parity.
- Where: src/mediakeys.rs, `linux_impl`, behind `cfg(target_os = "linux")`. zbus 5
  is already in the tree via ksni, so share it rather than pulling a second zbus.
  `mpris-server` is the obvious crate but currently targets zbus 6 and is async;
  implementing the two interfaces directly on `zbus::blocking` keeps it consistent
  with the ksni tray backend and avoids a second zbus.


## 2. Audio file-type associations (#166)

### Windows behavior (done)
- On every launch, LinAmp registers itself as a candidate handler for the audio
  types it can decode. The list is `crate::playlist::SUPPORTED_EXTS`, so the
  associations always track what the player actually supports.
- Everything is per-user under `HKCU\Software\Classes`, with no admin rights,
  matching the installer's per-user model. Specifically (src/fileassoc.rs):
  - A ProgID `LinAmp.Audio` with a friendly name, the exe's icon, and an open
    command `"<exe>" "%1"` that passes the clicked file as the first argument.
  - An `Applications\linamp.exe` entry (FriendlyAppName plus the same open
    command) that backs the "Open with > LinAmp" menu item.
  - Per extension, an `OpenWithProgids` value (a candidate, so LinAmp shows under
    "Open with" without seizing the default), a `SupportedTypes` entry, and a
    `Capabilities\FileAssociations` mapping.
  - A `RegisteredApplications` capability block so LinAmp lists in Settings >
    Default apps.
  - `SHChangeNotify` so the shell picks up the change without a sign-out.
- LinAmp never force-sets itself as the default. Since Windows 8 the default is
  hash-protected, so an app can only offer itself and let the user pick it in
  Default apps.

### What parity on Linux means
- On launch, install or refresh a desktop entry so LinAmp appears in the file
  manager's "Open with" and the desktop's default-apps UI for its audio types.
- Write `~/.local/share/applications/linamp.desktop` with `Exec=linamp %F`, an
  icon, and a `MimeType=` list, then register it with `xdg-mime` and
  `update-desktop-database`. All under `$HOME`, no root.
- Map `SUPPORTED_EXTS` to MIME types, roughly: `audio/mpeg` (mp3, mp1, mp2, mpa),
  `audio/flac`, `audio/ogg` (ogg, oga), `audio/x-wav` (wav), `audio/mp4` (m4a,
  m4b, mp4), `audio/aac`, `audio/x-aiff` (aiff, aif), `audio/midi` (mid, midi).
  The tracker types (mod, xm, s3m, it) have less standard MIME types
  (`audio/x-mod`, `audio/x-xm`, `audio/x-s3m`, `audio/x-it`); include them if the
  desktop recognizes them, otherwise they can be left off without blocking parity.
- Do not force the default. `xdg-mime default` sets a default, so call it only on
  an explicit user action, matching Windows not seizing the default silently.
- Where: src/fileassoc.rs, behind `cfg(target_os = "linux")`. `register()` is
  already called once per launch; fill in the Linux body.


## 3. Opening files: enqueue / clear and single instance (already cross-platform)

This behavior is pure Rust and already runs on Linux. It is documented here so the
Linux side can verify it and so the `.desktop` `Exec` line in section 2 is wired to
match it.

### Behavior to match (identical on both systems)
- Launching LinAmp with file, folder, or playlist arguments:
  - Always restores the saved session first: playlist, volume, balance, and window
    layout. Volume, balance, and window layout are never changed by opening files.
  - Then applies the opened items per Preferences > Playlist > "Open with":
    - Enqueue (default): add the opened items to the current playlist and start
      playing the first opened item.
    - Clear queue: replace the playlist with just the opened items and start
      playing the first.
  - Folders are added recursively; `.m3u` / `.m3u8` / `.pls` are loaded as
    playlists; everything else is added as a file if its extension is supported.
- Single instance plus forwarding: LinAmp is single-instance. If it is already
  running, a second launch writes its paths to a small cross-instance queue
  (src/openq.rs, one atomically-published file in the data dir) and exits. The
  running instance polls the queue (throttled to 250ms) and opens the files
  through `open_paths`, then brings its window forward, restoring from the tray if
  it was hidden.

### Linux status and what to check
- The only missing Linux piece is the `.desktop` registration in section 2, so the
  file manager actually launches `linamp` with the selected file. Until then, test
  from a terminal: run `linamp <file>` to start it, then `linamp <other-file>`
  while it is running, and confirm the forwarding and the Enqueue/Clear pref behave
  as above.
- `openq` uses `dirs::data_local_dir()` (`~/.local/share/LinAmp` on Linux), shared
  by both instances alongside the single-instance lock. Verify the running
  instance picks up a forwarded request promptly on your desktop; it relies on the
  idle repaint (about 4fps) to poll, which should wake it within ~250 to 500ms.
- The window-raise on a forwarded open uses `ViewportCommand::Minimized(false)` and
  `Focus`. Confirm that actually raises and focuses the window under your window
  manager (X11 and Wayland differ on focus-stealing); if a WM ignores it, this may
  need an EWMH activate or a different nudge, similar to the taskbar-skip handling.


## Build and conventions
- Build on the Linux targets we QA (Pop!_OS and Mint) with `cargo build`.
- House style for any code, comments, and docs: no em dashes, no arrow glyphs, no
  emojis, plain active voice. Commits authored as cjjulius with no Claude or
  Anthropic attribution.
