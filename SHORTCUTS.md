# LinAmp keyboard shortcuts

Goal: match classic Winamp as closely as possible. ✅ = implemented today,
🔜 = planned (several imply features we still need to build).

## Main window — playback
| Keys | Action | Status |
|------|--------|--------|
| X | Play | ✅ |
| C | Pause / unpause | ✅ |
| V | Stop | ✅ |
| B | Next track | ✅ |
| Z | Previous track | ✅ |
| Space | Play/pause toggle | ✅ (LinAmp extra) |
| ← / → | Seek −5s / +5s | ✅ |
| ↑ / ↓ | Volume up / down | ✅ |
| Shift+V | Stop with fade-out | 🔜 |
| Ctrl+V | Stop after current track | 🔜 |
| R | Toggle repeat | ✅ |
| S | Toggle shuffle | ✅ |
| Ctrl+T | Toggle time elapsed / remaining | ✅ (also click the time display) |

## Files / navigation
| Keys | Action | Status |
|------|--------|--------|
| L | Open / play file(s) | ✅ |
| Shift+L | Open folder | 🔜 (map to add folder) |
| Ctrl+L | Open location (URL / stream) | 🔜 (needs streaming) |
| J | Jump to file (search) | 🔜 (needs jump-to-file dialog) |
| Ctrl+J | Jump to time | 🔜 (needs jump-to-time dialog) |

## Windows / view
| Keys | Action | Status |
|------|--------|--------|
| Alt+E | Toggle playlist editor | 🔜 |
| Alt+G | Toggle equalizer | 🔜 |
| Ctrl+D | Toggle double-size mode | 🔜 (ties into the 1x/2x scaling option) |
| Ctrl+P | Preferences | 🔜 (needs preferences window) |

## Playlist editor
| Keys | Action | Status |
|------|--------|--------|
| Alt+3 | View/edit file info (ID3 tag editor) | 🔜 (needs tag editor) |
| Enter | Play selected | ✅ (double-click today) |
| Delete | Remove selected | 🔜 (menu today) |
| Ctrl+A | Select all | 🔜 (menu today) |
| Insert | Add file | 🔜 |

## Features these shortcuts imply (to build)
- ID3 tag editor (Alt+3), jump-to-file (J) and jump-to-time (Ctrl+J) dialogs
- Open-location / streaming (Ctrl+L)
- Double-size / scaling toggle (Ctrl+D) — see preferences
- Stop-after-current and stop-with-fade (Ctrl+V / Shift+V)
