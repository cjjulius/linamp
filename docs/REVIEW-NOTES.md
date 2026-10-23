# Review notes — feat/feature-set

Items needing a human eye or interaction to verify (not fully validatable by
code plus a working/not-working binary): visual and behavior changes.

## 2026-10-22

- #105 volume not remembered: fixed by adding volume and balance to the session
  change-detection hash in `persist_session_if_changed` (they were saved by
  `build_session` but never marked the session dirty, so a volume-only change was
  lost on the next launch). Verify interactively: change the volume, quit,
  relaunch, and confirm the level is restored. End-to-end persistence was
  confirmed by code inspection only — the automated test harness could not move
  the volume control via synthetic mouse or keyboard input.
