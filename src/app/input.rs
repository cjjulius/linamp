//! Classic Winamp keyboard-shortcut handling.

use super::*;

/// Keys pressed this frame, read out of the input lock before we touch `self`.
#[derive(Default)]
struct KeyState {
    play: bool,
    pause: bool,
    stop: bool,
    next: bool,
    prev: bool,
    toggle: bool,
    open: bool,
    repeat: bool,
    shuffle: bool,
    vol_up: bool,
    vol_down: bool,
    seek_fwd: bool,
    seek_back: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
    key_d: bool,
    key_p: bool,
    key_t: bool,
    key_3: bool,
    key_e: bool,
    key_g: bool,
    key_a: bool,
    key_i: bool,
    key_j: bool,
    del: bool,
    insert: bool,
    enter: bool,
    home: bool,
    end: bool,
    // Numeric-keypad transport digits (3 is `key_3` above, shared with Alt+3).
    num0: bool,
    num1: bool,
    num2: bool,
    num4: bool,
    num5: bool,
    num6: bool,
    num7: bool,
    num8: bool,
    num9: bool,
}

impl super::LinAmpApp {
    /// Flip the always-on-top preference and report it on the status line (shared by
    /// the Options menu toggle and the Ctrl+A shortcut).
    fn toggle_always_on_top(&mut self) {
        self.prefs.always_on_top = !self.prefs.always_on_top;
        self.prefs.save();
        self.status = format!(
            "Always on top: {}",
            if self.prefs.always_on_top { "ON" } else { "off" }
        );
    }

    pub(super) fn handle_shortcuts(&mut self, ctx: &egui::Context, win: ClWin) {
        if ctx.wants_keyboard_input() {
            return; // a text field has focus
        }
        let mut k = KeyState::default();
        ctx.input(|i| {
            k.play = i.key_pressed(Key::X);
            k.pause = i.key_pressed(Key::C);
            k.stop = i.key_pressed(Key::V);
            k.next = i.key_pressed(Key::B);
            k.prev = i.key_pressed(Key::Z);
            k.toggle = i.key_pressed(Key::Space);
            k.open = i.key_pressed(Key::L);
            k.repeat = i.key_pressed(Key::R);
            k.shuffle = i.key_pressed(Key::S);
            k.vol_up = i.key_pressed(Key::ArrowUp);
            k.vol_down = i.key_pressed(Key::ArrowDown);
            k.seek_fwd = i.key_pressed(Key::ArrowRight);
            k.seek_back = i.key_pressed(Key::ArrowLeft);
            k.ctrl = i.modifiers.ctrl || i.modifiers.command;
            k.alt = i.modifiers.alt;
            k.shift = i.modifiers.shift;
            k.key_d = i.key_pressed(Key::D);
            k.key_p = i.key_pressed(Key::P);
            k.key_t = i.key_pressed(Key::T);
            k.key_3 = i.key_pressed(Key::Num3);
            k.key_e = i.key_pressed(Key::E);
            k.key_g = i.key_pressed(Key::G);
            k.key_a = i.key_pressed(Key::A);
            k.key_i = i.key_pressed(Key::I);
            k.key_j = i.key_pressed(Key::J);
            k.del = i.key_pressed(Key::Delete);
            k.insert = i.key_pressed(Key::Insert);
            k.enter = i.key_pressed(Key::Enter);
            k.home = i.key_pressed(Key::Home);
            k.end = i.key_pressed(Key::End);
            k.num0 = i.key_pressed(Key::Num0);
            k.num1 = i.key_pressed(Key::Num1);
            k.num2 = i.key_pressed(Key::Num2);
            k.num4 = i.key_pressed(Key::Num4);
            k.num5 = i.key_pressed(Key::Num5);
            k.num6 = i.key_pressed(Key::Num6);
            k.num7 = i.key_pressed(Key::Num7);
            k.num8 = i.key_pressed(Key::Num8);
            k.num9 = i.key_pressed(Key::Num9);
        });

        if k.play {
            self.play_pause();
        }
        if k.pause {
            // Fade out before pausing (when the pref is on), like the pause button
            // and the Play/Pause toggle, instead of cutting abruptly (#171). Still a
            // one-way pause: pause_playback only acts while playing.
            self.pause_playback();
        }
        if k.stop {
            if k.shift {
                // Stop with a fade-out.
                if self.player.state() != PlayState::Stopped {
                    self.fade = Some((self.prefs.fade_on_stop_secs, FadeEnd::Stop));
                    self.status = "Fading out…".into();
                }
            } else if k.ctrl {
                // Toggle stop-after-current.
                self.stop_after_current = !self.stop_after_current;
                self.status = format!(
                    "Stop after current track: {}",
                    if self.stop_after_current { "ON" } else { "off" }
                );
            } else {
                self.stop_playback();
            }
        }
        if k.next {
            self.next();
        }
        if k.prev {
            if k.ctrl {
                // Ctrl+Z, jump to the start of the list.
                if !self.playlist.is_empty() {
                    self.play_index(0);
                }
            } else {
                self.prev();
            }
        }
        if k.toggle {
            self.play_pause();
        }
        if k.open && !k.alt && !k.ctrl {
            if k.shift {
                self.open_folder();
            } else {
                self.open_files();
            }
        }
        if k.ctrl && k.open {
            // Ctrl+L, open a stream location (URL).
            self.open_location();
        }
        if k.repeat {
            if k.ctrl {
                // Ctrl+R, reverse the playlist order.
                self.playlist.reverse();
                self.plview.selection.clear();
            } else {
                self.repeat_all = !self.repeat_all;
            }
        }
        if k.shuffle {
            self.shuffle = !self.shuffle;
        }
        if k.vol_up {
            // Up/Down move the selection in the playlist window, else volume.
            // Ctrl adds individual rows; Shift extends the range from the anchor.
            if win == ClWin::Pl {
                self.pl_navigate(-1, k.ctrl, k.shift);
            } else {
                self.adjust_volume(0.05);
            }
        }
        if k.vol_down {
            if win == ClWin::Pl {
                self.pl_navigate(1, k.ctrl, k.shift);
            } else {
                self.adjust_volume(-0.05);
            }
        }
        // Home / End jump the playlist selection to the first / last track.
        if win == ClWin::Pl && (k.home || k.end) && !self.playlist.is_empty() {
            let target = if k.home { 0 } else { self.playlist.len() - 1 };
            self.pl_navigate_to(target, k.ctrl, k.shift);
        }
        if k.seek_fwd {
            self.seek_relative(5.0);
        }
        if k.seek_back {
            self.seek_relative(-5.0);
        }
        // Numeric-keypad transport (classic Winamp). Opt-in under Optional
        // Features, egui's logical Key can't tell the keypad apart from the top
        // number row, so these also fire for the top row; modifier combos (Alt+3
        // tag editor, Ctrl+digit) are excluded so they keep their own meanings.
        if self.prefs.numpad_transport && !k.ctrl && !k.alt {
            if k.num0 {
                self.open_files(); // keypad 0, open/play file(s)
            }
            if k.num5 {
                self.play_pause(); // keypad 5, play
            }
            if k.num4 {
                self.prev(); // keypad 4, previous track
            }
            if k.num6 {
                self.next(); // keypad 6, next track
            }
            if k.num8 {
                self.adjust_volume(0.05); // keypad 8, volume up
            }
            if k.num2 {
                self.adjust_volume(-0.05); // keypad 2, volume down
            }
            if k.num9 {
                self.seek_relative(5.0); // keypad 9, forward 5s
            }
            if k.num7 {
                self.seek_relative(-5.0); // keypad 7, back 5s
            }
            // keypad 3 / 1, skip forward / back 10 tracks (3 is `key_3`).
            let jump = if k.key_3 {
                Some(10)
            } else if k.num1 {
                Some(-10)
            } else {
                None
            };
            if let Some(delta) = jump {
                if let Some(target) = self.playlist.jump_target(delta) {
                    self.play_index(target);
                }
            }
        }
        if k.ctrl && k.key_d {
            let dbl = !self.prefs.double_size;
            self.set_double_size(ctx, dbl);
        }
        if k.ctrl && k.key_p {
            self.prefs_open = !self.prefs_open;
        }
        if k.ctrl && k.key_t {
            self.time_remaining = !self.time_remaining;
        }
        if k.alt && k.key_3 {
            self.open_tag_editor();
        }
        if k.alt && k.key_i {
            self.add_bookmark();
        }
        if k.key_j {
            if k.ctrl {
                self.jump_time = Some(String::new());
            } else {
                self.jump_file = Some(JumpFile::default());
            }
        }
        // Window toggles.
        if k.alt && k.key_g {
            self.eq_visible = !self.eq_visible;
            if self.eq_visible {
                self.restack();
            }
        }
        if k.alt && k.key_e {
            self.pl_visible = !self.pl_visible;
            if self.pl_visible {
                self.restack();
            }
        }
        if k.alt && k.open {
            self.toggle_library();
        }
        // Ctrl+A: select-all in the playlist window (the Winamp meaning), else
        // toggle always-on-top. Each classic window is its own viewport, so key
        // presses reach only the focused one, `win` here is effectively the window
        // with focus.
        if k.ctrl && k.key_a {
            if win == ClWin::Pl {
                self.pl_select_all();
            } else {
                self.toggle_always_on_top();
            }
        }
        if k.del {
            self.pl_remove(false);
        }
        if k.insert {
            self.open_files();
        }
        if k.enter {
            if let Some(&idx) = self.plview.selection.iter().min() {
                self.play_index(idx);
            }
        }
    }
}
