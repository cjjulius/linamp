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
    key_j: bool,
    del: bool,
    insert: bool,
    enter: bool,
    home: bool,
    end: bool,
}

impl super::LinAmpApp {
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
            k.key_j = i.key_pressed(Key::J);
            k.del = i.key_pressed(Key::Delete);
            k.insert = i.key_pressed(Key::Insert);
            k.enter = i.key_pressed(Key::Enter);
            k.home = i.key_pressed(Key::Home);
            k.end = i.key_pressed(Key::End);
        });

        if k.play {
            self.play_pause();
        }
        if k.pause {
            self.player.pause();
        }
        if k.stop {
            if k.shift {
                // Stop with a fade-out.
                if self.player.state() != PlayState::Stopped {
                    self.fade = Some(FADE_SECS);
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
            self.prev();
        }
        if k.toggle {
            self.play_pause();
        }
        if k.open && !k.alt {
            if k.shift {
                self.open_folder();
            } else {
                self.open_files();
            }
        }
        if k.repeat {
            self.repeat_all = !self.repeat_all;
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
        // Playlist edit.
        if k.ctrl && k.key_a {
            self.pl_select_all();
        }
        if k.del {
            self.pl_remove(false);
        }
        if k.insert {
            self.open_files();
        }
        if k.enter {
            if let Some(&idx) = self.pl_selection.iter().min() {
                self.play_index(idx);
            }
        }
    }
}
