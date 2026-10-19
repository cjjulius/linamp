use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, Id, Key, RichText, Stroke};
use rustfft::{Fft, FftPlanner};

use crate::classic::{self, Btn, ClassicSkin, MainState, PlayUi};
use crate::config::Prefs;
use crate::library::Library;
use crate::player::{PlayState, Player};
use crate::playlist::{save_playlist_file, Playlist};
use crate::skin::{self, Theme};
use crate::viz::{self, VizShared};

const FFT_SIZE: usize = 2048;
const VIZ_BANDS: usize = 28;
/// Magnetic docking snap distance in points.
const SNAP: f32 = 16.0;

fn win_idx(w: ClWin) -> usize {
    match w {
        ClWin::Main => 0,
        ClWin::Eq => 1,
        ClWin::Pl => 2,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Classic,
    Modern,
}

/// Which classic window a pointer gesture is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClWin {
    Main,
    Eq,
    Pl,
}

/// An in-progress title-bar drag of a classic window.
///
/// On Windows (where we can read the global cursor) we drive the move ourselves
/// each frame so docked windows move in tandem, live. Elsewhere we fall back to
/// the OS window-drag (ViewportCommand::StartDrag), which snaps the group on
/// release. `win_pos` stays authoritative and is snapped on release either way.
struct Dragging {
    win: ClWin,
    /// true = manual per-frame move (live tandem); false = OS drag fallback.
    manual: bool,
    /// Manual: offset of the global cursor from the window origin (points).
    grab: egui::Vec2,
    /// Window position at press (to tell a click from a drag).
    press_pos: egui::Pos2,
    /// OS fallback: screen pos at press (drag threshold) + whether started.
    press_screen: egui::Pos2,
    started_os: bool,
    moved: bool,
    /// Docked windows that move with this one, by offset from it.
    group: Vec<(usize, egui::Vec2)>,
}

/// Global cursor position in egui points, if the platform exposes it (Windows).
fn global_cursor_points(ctx: &egui::Context) -> Option<egui::Pos2> {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct Point {
            x: i32,
            y: i32,
        }
        extern "system" {
            fn GetCursorPos(p: *mut Point) -> i32;
        }
        let mut pt = Point { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt) } != 0 {
            let ppp = ctx.pixels_per_point().max(0.1);
            return Some(egui::pos2(pt.x as f32 / ppp, pt.y as f32 / ppp));
        }
        None
    }
    #[cfg(not(windows))]
    {
        let _ = ctx;
        None
    }
}

/// Popup menus in classic mode (options + the playlist bottom-bar fly-outs).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Popup {
    Options,
    PlAdd,
    PlRem,
    PlSel,
    PlMisc,
    PlList,
}

const EQ_LABELS: [&str; 10] = [
    "60", "170", "310", "600", "1K", "3K", "6K", "12K", "14K", "16K",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Playlist,
    Library,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Custom,
    Title,
    Artist,
    Album,
    Duration,
}

impl SortKey {
    fn label(self) -> &'static str {
        match self {
            SortKey::Custom => "Manual order",
            SortKey::Title => "Title",
            SortKey::Artist => "Artist",
            SortKey::Album => "Album",
            SortKey::Duration => "Duration",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VizMode {
    Spectrum,
    Oscilloscope,
    Off,
}

impl VizMode {
    fn next(self) -> Self {
        match self {
            VizMode::Spectrum => VizMode::Oscilloscope,
            VizMode::Oscilloscope => VizMode::Off,
            VizMode::Off => VizMode::Spectrum,
        }
    }
}

pub struct LinAmpApp {
    player: Player,
    playlist: Playlist,
    library: Option<Library>,

    shuffle: bool,
    repeat_all: bool,
    repeat_one: bool,

    // seek bar drag state
    seeking: bool,
    seek_preview: f32,

    // equalizer
    eq_enabled: bool,
    eq_preamp_db: f32,
    eq_bands: [f32; 10],

    // playlist / library views
    view: View,
    pl_search: String,
    sort_key: SortKey,
    lib_query: String,
    lib_results: Vec<crate::playlist::Track>,

    // visualization
    viz: Arc<VizShared>,
    viz_mode: VizMode,
    viz_bars: Vec<f32>,
    viz_samples: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,

    // album art
    album_art: Option<egui::TextureHandle>,
    art_bytes: Option<Vec<u8>>,
    art_dirty: bool,

    // theming / skins
    theme: Theme,
    skins: Vec<PathBuf>,

    // classic skin renderer
    ui_mode: UiMode,
    classic: Option<ClassicSkin>,
    classic_tex: Option<egui::TextureHandle>,
    eq_tex: Option<egui::TextureHandle>,
    pl_tex: Option<egui::TextureHandle>,
    active_region: Option<Btn>,
    eq_active: Option<classic::EqRegion>,
    pressed_btn: Option<Btn>,
    classic_seek_preview: Option<f32>,
    balance: f32,
    mode_init: bool,
    eq_visible: bool,
    pl_visible: bool,
    eq_auto: bool,
    pl_first: usize,
    pl_selection: HashSet<usize>,
    pl_last_click: Option<(usize, Instant)>,
    popup: Option<(Popup, ClWin, egui::Pos2)>,
    popup_fresh: bool,
    main_shade: bool,
    eq_shade: bool,
    pl_shade: bool,
    tb_last_click: Option<(ClWin, Instant)>,
    // multi-window geometry (points)
    win_pos: [egui::Pos2; 3],
    pl_px_h: f32,
    positions_init: bool,
    drag: Option<Dragging>,
    resize_pl: Option<f32>,
    root_last: Option<(egui::Pos2, egui::Vec2)>,
    pl_menu_press: Option<classic::PlRegion>,
    prefs: Prefs,
    scale: f32,
    prefs_open: bool,

    marquee_offset: f32,
    last_frame: Instant,
    last_device_check: Instant,
    status: String,
}

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
    key_d: bool,
    key_p: bool,
}

impl LinAmpApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let player = Player::new().expect("failed to open audio output device");
        let viz = player.viz();
        let library = Library::open().ok();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let prefs = Prefs::load();
        let scale = prefs.scale();

        let mut app = Self {
            player,
            playlist: Playlist::default(),
            library,
            shuffle: false,
            repeat_all: true,
            repeat_one: false,
            seeking: false,
            seek_preview: 0.0,
            eq_enabled: false,
            eq_preamp_db: 0.0,
            eq_bands: [0.0; 10],
            view: View::Playlist,
            pl_search: String::new(),
            sort_key: SortKey::Custom,
            lib_query: String::new(),
            lib_results: Vec::new(),
            viz,
            viz_mode: VizMode::Spectrum,
            viz_bars: vec![0.0; VIZ_BANDS],
            viz_samples: Vec::new(),
            fft,
            album_art: None,
            art_bytes: None,
            art_dirty: false,
            theme: Theme::default(),
            skins: Vec::new(),
            ui_mode: UiMode::Modern,
            classic: None,
            classic_tex: None,
            eq_tex: None,
            pl_tex: None,
            active_region: None,
            eq_active: None,
            pressed_btn: None,
            classic_seek_preview: None,
            balance: 0.0,
            mode_init: true,
            eq_visible: false,
            pl_visible: false,
            eq_auto: false,
            pl_first: 0,
            pl_selection: HashSet::new(),
            pl_last_click: None,
            popup: None,
            popup_fresh: false,
            main_shade: false,
            eq_shade: false,
            pl_shade: false,
            tb_last_click: None,
            win_pos: [egui::pos2(100.0, 100.0); 3],
            pl_px_h: classic::PL_H as f32,
            positions_init: false,
            drag: None,
            resize_pl: None,
            root_last: None,
            pl_menu_press: None,
            prefs,
            scale,
            prefs_open: false,
            marquee_offset: 0.0,
            last_frame: Instant::now(),
            last_device_check: Instant::now(),
            status: "Ready. Press L to open files.".to_string(),
        };

        // Accept files/folders/playlists passed on the command line.
        for arg in std::env::args().skip(1) {
            let p = PathBuf::from(arg);
            if p.is_dir() {
                app.playlist.add_folder(&p);
            } else if matches!(
                p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
                Some("m3u") | Some("m3u8") | Some("pls")
            ) {
                app.playlist.load_file(&p);
            } else {
                app.playlist.add_file(p);
            }
        }
        if !app.playlist.is_empty() {
            app.play_index(0);
        }
        app.refresh_library();
        app.discover_skins();

        // Open with the LinAmp-branded default skin (fall back to any skin found).
        let stem = |p: &std::path::Path| {
            p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
        };
        let startup = app
            .skins
            .iter()
            .find(|p| stem(p) == "linamp_base_skin")
            .or_else(|| app.skins.iter().find(|p| stem(p).contains("base")))
            .cloned()
            .or_else(|| app.skins.first().cloned());
        if let Some(p) = startup {
            if let Ok(cs) = ClassicSkin::load(&p) {
                app.classic = Some(cs);
                if let Ok(t) = skin::load_wsz(&p) {
                    app.theme = t;
                }
                app.ui_mode = UiMode::Classic;
            }
        }

        apply_theme(&cc.egui_ctx, &app.theme);
        app
    }

    // ---- transport helpers -------------------------------------------------

    fn play_index(&mut self, idx: usize) {
        self.playlist.ensure_loaded(idx);
        if let Some(track) = self.playlist.tracks.get(idx).cloned() {
            match self.player.play_file(&track.path) {
                Ok(()) => {
                    self.playlist.current = Some(idx);
                    self.seek_preview = 0.0;
                    self.marquee_offset = 0.0;
                    self.apply_eq();
                    self.art_bytes = crate::playlist::read_cover(&track.path);
                    self.art_dirty = true;
                    self.status = format!("Playing: {}", track.display_name());
                }
                Err(e) => self.status = format!("Error: {e}"),
            }
        }
    }

    fn play_pause(&mut self) {
        match self.player.state() {
            PlayState::Stopped => {
                if !self.playlist.is_empty() {
                    let start = self.playlist.current.unwrap_or(0);
                    self.play_index(start);
                }
            }
            _ => self.player.toggle_pause(),
        }
    }

    fn next(&mut self) {
        if let Some(i) = self.playlist.next_index(self.shuffle, self.repeat_all) {
            self.play_index(i);
        } else {
            self.player.stop();
            self.status = "End of playlist.".to_string();
        }
    }

    fn prev(&mut self) {
        if let Some(i) = self.playlist.prev_index(self.shuffle, self.repeat_all) {
            self.play_index(i);
        }
    }

    fn adjust_volume(&mut self, delta: f32) {
        let v = (self.player.volume() + delta).clamp(0.0, 1.0);
        self.player.set_volume(v);
    }

    fn seek_relative(&mut self, secs: f32) {
        let pos = self.player.position().as_secs_f32() + secs;
        self.player.seek(Duration::from_secs_f32(pos.max(0.0)));
    }

    /// Keep `current` pointing at the same track after a reorder/sort.
    fn restore_current(&mut self, path: Option<PathBuf>) {
        if let Some(p) = path {
            self.playlist.current = self.playlist.tracks.iter().position(|t| t.path == p);
        }
    }

    fn move_track(&mut self, from: usize, mut to: usize) {
        if from >= self.playlist.tracks.len() {
            return;
        }
        let cur = self.playlist.current_track().map(|t| t.path.clone());
        let track = self.playlist.tracks.remove(from);
        if from < to {
            to -= 1;
        }
        let to = to.min(self.playlist.tracks.len());
        self.playlist.tracks.insert(to, track);
        self.sort_key = SortKey::Custom;
        self.restore_current(cur);
    }

    fn sort_playlist(&mut self) {
        let cur = self.playlist.current_track().map(|t| t.path.clone());
        let t = &mut self.playlist.tracks;
        match self.sort_key {
            SortKey::Custom => {}
            SortKey::Title => t.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            SortKey::Artist => t.sort_by(|a, b| {
                (a.artist.to_lowercase(), a.album.to_lowercase(), a.title.to_lowercase()).cmp(&(
                    b.artist.to_lowercase(),
                    b.album.to_lowercase(),
                    b.title.to_lowercase(),
                ))
            }),
            SortKey::Album => t.sort_by(|a, b| {
                (a.album.to_lowercase(), a.title.to_lowercase())
                    .cmp(&(b.album.to_lowercase(), b.title.to_lowercase()))
            }),
            SortKey::Duration => t.sort_by(|a, b| a.duration.cmp(&b.duration)),
        }
        self.restore_current(cur);
    }

    // ---- file dialogs ------------------------------------------------------

    fn open_files(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "Audio",
                &["mp3", "flac", "ogg", "oga", "wav", "m4a", "aac", "opus", "wv", "aiff", "aif"],
            )
            .pick_files()
        {
            let was_empty = self.playlist.is_empty();
            for p in paths {
                self.playlist.add_file(p);
            }
            self.maybe_scan_tags();
            if was_empty && !self.playlist.is_empty() {
                self.play_index(0);
            }
            self.status = format!("{} track(s) in playlist.", self.playlist.len());
        }
    }

    fn maybe_scan_tags(&mut self) {
        if self.prefs.tag_on_load {
            self.playlist.load_all();
        }
    }

    /// Append any files/folders/playlists dropped onto this viewport's window.
    fn handle_dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect()
        });
        if dropped.is_empty() {
            return;
        }
        let was_empty = self.playlist.is_empty();
        for p in dropped {
            let is_pl = matches!(
                p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
                Some("m3u") | Some("m3u8") | Some("pls")
            );
            if p.is_dir() {
                self.playlist.add_folder(&p);
            } else if is_pl {
                self.playlist.load_file(&p);
            } else {
                self.playlist.add_file(p);
            }
        }
        self.maybe_scan_tags();
        if was_empty && !self.playlist.is_empty() {
            self.play_index(0);
        }
        self.status = format!("{} track(s) in playlist.", self.playlist.len());
    }

    fn open_folder(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let was_empty = self.playlist.is_empty();
            self.playlist.add_folder(&dir);
            self.maybe_scan_tags();
            if was_empty && !self.playlist.is_empty() {
                self.play_index(0);
            }
            self.status = format!("{} track(s) in playlist.", self.playlist.len());
        }
    }

    fn save_playlist_dialog(&mut self) {
        if self.playlist.is_empty() {
            self.status = "Playlist is empty.".to_string();
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("M3U playlist", &["m3u"])
            .add_filter("PLS playlist", &["pls"])
            .set_file_name("playlist.m3u")
            .save_file()
        {
            match save_playlist_file(&path, &self.playlist.tracks) {
                Ok(()) => self.status = format!("Saved {}", path.display()),
                Err(e) => self.status = format!("Save error: {e}"),
            }
        }
    }

    fn load_playlist_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Playlists", &["m3u", "m3u8", "pls"])
            .pick_file()
        {
            let was_empty = self.playlist.is_empty();
            let n = self.playlist.load_file(&path);
            self.maybe_scan_tags();
            if was_empty && !self.playlist.is_empty() {
                self.play_index(0);
            }
            self.status = format!("Loaded {n} track(s).");
        }
    }

    // ---- library -----------------------------------------------------------

    fn refresh_library(&mut self) {
        if let Some(lib) = &self.library {
            let q = self.lib_query.trim();
            self.lib_results = if q.is_empty() {
                lib.all(1000)
            } else {
                lib.search(q, 1000)
            };
        }
    }

    fn scan_into_library(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            if let Some(lib) = &self.library {
                match lib.scan_folder(&dir) {
                    Ok(n) => self.status = format!("Scanned {n} file(s) into library."),
                    Err(e) => self.status = format!("Scan error: {e}"),
                }
            }
            self.refresh_library();
        }
    }

    // ---- skins / theming ---------------------------------------------------

    fn discover_skins(&mut self) {
        self.skins.clear();
        let mut dirs = vec![PathBuf::from("skins")];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                dirs.push(d.join("skins"));
            }
        }
        for d in dirs {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let p = e.path();
                    let is_skin = p
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| matches!(x.to_lowercase().as_str(), "wsz" | "zip"))
                        .unwrap_or(false);
                    if is_skin && !self.skins.contains(&p) {
                        self.skins.push(p);
                    }
                }
            }
        }
        self.skins.sort();
    }

    fn apply_skin_path(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        match skin::load_wsz(path) {
            Ok(t) => {
                self.theme = t;
                apply_theme(ctx, &self.theme);
                self.status = format!("Loaded skin: {}", self.theme.name);
            }
            Err(e) => self.status = format!("Skin error: {e}"),
        }
        // Load the classic sprite sheets for the same skin.
        self.classic = ClassicSkin::load(path).ok();
        self.classic_tex = None;
    }

    fn set_mode(&mut self, ctx: &egui::Context, mode: UiMode) {
        self.ui_mode = mode;
        match mode {
            UiMode::Classic => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                    self.win_inner_size(ClWin::Main),
                ));
                self.root_last = None;
            }
            UiMode::Modern => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(420.0, 640.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(true));
            }
        }
    }

    // ---- classic skin view -------------------------------------------------

    fn make_main_state(&self) -> MainState {
        let play = match self.player.state() {
            PlayState::Playing => PlayUi::Playing,
            PlayState::Paused => PlayUi::Paused,
            PlayState::Stopped => PlayUi::Stopped,
        };
        let (mut title, mut kbps, mut khz, mut stereo, mut dur) =
            (String::from("linamp"), String::new(), String::new(), true, 0.0);
        if let Some(t) = self.playlist.current_track() {
            title = t.display_name();
            kbps = t.bitrate.map(|b| b.to_string()).unwrap_or_default();
            khz = t.sample_rate.map(|s| (s / 1000).to_string()).unwrap_or_default();
            stereo = t.channels.map(|c| c >= 2).unwrap_or(true);
            dur = t.duration.map(|d| d.as_secs_f32()).unwrap_or(0.0);
        }
        let pos = self
            .classic_seek_preview
            .unwrap_or_else(|| self.player.position().as_secs_f32());

        MainState {
            focused: true,
            play,
            pressed: self.pressed_btn,
            shuffle: self.shuffle,
            repeat: self.repeat_all,
            eq_on: self.eq_visible,
            pl_on: self.pl_visible,
            pos,
            duration: dur,
            volume: self.player.volume(),
            balance: self.balance,
            stereo,
            title: scroll_text(&title, self.marquee_offset, 30),
            kbps,
            khz,
            fg: [self.theme.lcd.r(), self.theme.lcd.g(), self.theme.lcd.b()],
        }
    }

    /// Height in native px of each classic window given shade state.
    fn main_h(&self) -> f32 {
        if self.main_shade { 14.0 } else { classic::MAIN_H as f32 }
    }
    fn eq_h(&self) -> f32 {
        if self.eq_shade { 14.0 } else { classic::EQ_H as f32 }
    }
    fn apply_classic_slider(&mut self, region: Btn, lx: f32) {
        match region {
            Btn::Seek => {
                let dur = self
                    .playlist
                    .current_track()
                    .and_then(|t| t.duration)
                    .map(|d| d.as_secs_f32())
                    .unwrap_or(0.0);
                if dur > 0.0 {
                    let frac = ((lx - 16.0) / (248.0 - 29.0)).clamp(0.0, 1.0);
                    self.classic_seek_preview = Some(frac * dur);
                }
            }
            Btn::Volume => {
                let frac = ((lx - 107.0) / (68.0 - 14.0)).clamp(0.0, 1.0);
                self.player.set_volume(frac);
            }
            Btn::Balance => {
                let t = ((lx - 177.0) / (38.0 - 14.0)).clamp(0.0, 1.0);
                self.balance = t * 2.0 - 1.0;
            }
            _ => {}
        }
    }

    fn trigger_classic(&mut self, ctx: &egui::Context, region: Btn) {
        match region {
            Btn::Prev => self.prev(),
            Btn::Play => self.play_pause(),
            Btn::Pause => self.player.pause(),
            Btn::Stop => self.player.stop(),
            Btn::Next => self.next(),
            Btn::Eject => self.open_files(),
            Btn::Shuffle => self.shuffle = !self.shuffle,
            Btn::Repeat => self.repeat_all = !self.repeat_all,
            Btn::Eq => {
                self.eq_visible = !self.eq_visible;
                if self.eq_visible {
                    self.restack();
                }
            }
            Btn::Pl => {
                self.pl_visible = !self.pl_visible;
                if self.pl_visible {
                    self.restack();
                }
            }
            Btn::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Btn::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            Btn::Seek | Btn::Volume | Btn::Balance | Btn::TitleBar | Btn::Options => {}
        }
    }

    fn make_eq_state(&self) -> classic::EqState {
        classic::EqState {
            focused: true,
            on: self.eq_enabled,
            auto: self.eq_auto,
            pressed: self.eq_active,
            preamp: (self.eq_preamp_db / 12.0).clamp(-1.0, 1.0),
            bands: self.eq_bands.map(|d| (d / 12.0).clamp(-1.0, 1.0)),
        }
    }

    fn build_pl_rows(&self) -> Vec<classic::PlRow> {
        let cur = self.playlist.current;
        self.playlist
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                (
                    format!("{}. {}", i + 1, t.display_name()),
                    Some(i) == cur,
                    self.pl_selection.contains(&i),
                )
            })
            .collect()
    }

    fn pl_select_all(&mut self) {
        self.pl_selection = (0..self.playlist.len()).collect();
    }

    fn pl_invert_selection(&mut self) {
        self.pl_selection = (0..self.playlist.len())
            .filter(|i| !self.pl_selection.contains(i))
            .collect();
    }

    /// Remove tracks: when `keep` is true keep only selected (crop), else remove
    /// the selected ones. Preserves the current track by path where possible.
    fn pl_remove(&mut self, keep: bool) {
        if self.pl_selection.is_empty() {
            return;
        }
        let cur_path = self.playlist.current_track().map(|t| t.path.clone());
        let sel = self.pl_selection.clone();
        let mut i = 0usize;
        self.playlist.tracks.retain(|_| {
            let selected = sel.contains(&i);
            i += 1;
            if keep {
                selected
            } else {
                !selected
            }
        });
        self.pl_selection.clear();
        self.playlist.current =
            cur_path.and_then(|p| self.playlist.tracks.iter().position(|t| t.path == p));
        self.pl_first = self.pl_first.min(self.playlist.len().saturating_sub(1));
        self.status = format!("{} track(s) in playlist.", self.playlist.len());
    }

    fn pl_colors(&self) -> classic::PlColors {
        let c = |x: Color32| [x.r(), x.g(), x.b()];
        classic::PlColors {
            bg: c(self.theme.list_bg),
            normal: c(self.theme.list_normal),
            current: c(self.theme.list_current),
            selbg: c(self.theme.list_sel_bg),
        }
    }

    fn trigger_eq(&mut self, _ctx: &egui::Context, region: classic::EqRegion) {
        use classic::EqRegion as R;
        match region {
            R::On => {
                self.eq_enabled = !self.eq_enabled;
                self.apply_eq();
            }
            R::Auto => self.eq_auto = !self.eq_auto,
            R::Close => {
                self.eq_visible = false;
            }
            R::Presets | R::TitleBar | R::Preamp | R::Band(_) => {}
        }
    }

    fn draw_popup_in(&mut self, ctx: &egui::Context, win: ClWin) {
        let Some((kind, owner, pos)) = self.popup else {
            return;
        };
        if owner != win {
            return;
        }
        let mut close = false;
        let sort = |app: &mut Self, k: SortKey| {
            app.sort_key = k;
            app.sort_playlist();
        };
        let inner = egui::Area::new(egui::Id::new("classic_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(140.0);
                    let item = |ui: &mut egui::Ui, label: &str| ui.button(label).clicked();
                    match kind {
                        Popup::Options => {
                            if item(ui, "Open file(s)…") {
                                self.open_files();
                                close = true;
                            }
                            if item(ui, "Open folder…") {
                                self.open_folder();
                                close = true;
                            }
                            ui.separator();
                            let eql = if self.eq_visible { "Hide Equalizer" } else { "Show Equalizer" };
                            if item(ui, eql) {
                                self.eq_visible = !self.eq_visible;
                                if self.eq_visible {
                                    self.restack();
                                }
                                close = true;
                            }
                            let pll = if self.pl_visible { "Hide Playlist" } else { "Show Playlist" };
                            if item(ui, pll) {
                                self.pl_visible = !self.pl_visible;
                                if self.pl_visible {
                                    self.restack();
                                }
                                close = true;
                            }
                            ui.separator();
                            if item(ui, "Load skin…") {
                                self.load_skin_dialog(ctx);
                                close = true;
                            }
                            if item(ui, "Preferences…") {
                                self.prefs_open = true;
                                close = true;
                            }
                            ui.separator();
                            if item(ui, "Exit") {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                        Popup::PlAdd => {
                            if item(ui, "Add file(s)…") {
                                self.open_files();
                                close = true;
                            }
                            if item(ui, "Add folder…") {
                                self.open_folder();
                                close = true;
                            }
                        }
                        Popup::PlRem => {
                            if item(ui, "Remove selected") {
                                self.pl_remove(false);
                                close = true;
                            }
                            if item(ui, "Crop (keep selected)") {
                                self.pl_remove(true);
                                close = true;
                            }
                            if item(ui, "Remove all") {
                                self.playlist.clear();
                                self.pl_selection.clear();
                                self.player.stop();
                                close = true;
                            }
                        }
                        Popup::PlSel => {
                            if item(ui, "Select all") {
                                self.pl_select_all();
                                close = true;
                            }
                            if item(ui, "Select none") {
                                self.pl_selection.clear();
                                close = true;
                            }
                            if item(ui, "Invert selection") {
                                self.pl_invert_selection();
                                close = true;
                            }
                        }
                        Popup::PlMisc => {
                            if item(ui, "Sort by title") {
                                sort(self, SortKey::Title);
                                close = true;
                            }
                            if item(ui, "Sort by artist") {
                                sort(self, SortKey::Artist);
                                close = true;
                            }
                            if item(ui, "Sort by album") {
                                sort(self, SortKey::Album);
                                close = true;
                            }
                            if item(ui, "Sort by duration") {
                                sort(self, SortKey::Duration);
                                close = true;
                            }
                        }
                        Popup::PlList => {
                            if item(ui, "New list") {
                                self.playlist.clear();
                                self.pl_selection.clear();
                                self.player.stop();
                                close = true;
                            }
                            if item(ui, "Save list…") {
                                self.save_playlist_dialog();
                                close = true;
                            }
                            if item(ui, "Load list…") {
                                self.load_playlist_dialog();
                                close = true;
                            }
                        }
                    }
                });
            });

        if self.popup_fresh {
            self.popup_fresh = false;
        } else if inner.response.clicked_elsewhere()
            || ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            close = true;
        }
        if close {
            self.popup = None;
        }
    }

    fn open_popup_in(&mut self, kind: Popup, win: ClWin, local: egui::Pos2) {
        self.popup = Some((kind, win, local));
        self.popup_fresh = true;
    }

    /// Per-window input in local (viewport) coordinates.
    fn handle_window_input(&mut self, ctx: &egui::Context, win: ClWin) {
        let outer = ctx.input(|i| i.viewport().outer_rect);
        let monitor = ctx.input(|i| i.viewport().monitor_size);
        let local = ctx.pointer_latest_pos();
        let screen_cursor = match (outer, local) {
            (Some(o), Some(l)) => Some(o.min + l.to_vec2()),
            _ => None,
        };
        let (pressed, released, down, secondary, scroll) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.primary_down(),
                i.pointer.secondary_clicked(),
                i.raw_scroll_delta.y,
            )
        });
        let shaded = match win {
            ClWin::Main => self.main_shade,
            ClWin::Eq => self.eq_shade,
            ClWin::Pl => self.pl_shade,
        };

        if secondary && win == ClWin::Main {
            if let Some(l) = local {
                self.open_popup_in(Popup::Options, win, l);
            }
        }

        if win == ClWin::Pl && !shaded && scroll != 0.0 {
            let max = self.playlist.len().saturating_sub(1) as i32;
            let step = if scroll > 0.0 { -3 } else { 3 };
            self.pl_first = (self.pl_first as i32 + step).clamp(0, max.max(0)) as usize;
        }

        if pressed {
            self.active_region = None;
            self.eq_active = None;
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if shaded {
                    match classic::shade_region_at(nx, ny) {
                        Some(classic::ShadeHit::Close) => self.close_window(ctx, win),
                        Some(classic::ShadeHit::TitleBar) => self.start_drag(ctx, win, screen_cursor),
                        None => {}
                    }
                } else {
                    match win {
                        ClWin::Main => {
                            let r = classic::region_at(nx, ny);
                            self.active_region = r;
                            match r {
                                Some(Btn::TitleBar) => {
                                    self.active_region = None;
                                    self.start_drag(ctx, win, screen_cursor);
                                }
                                Some(b @ (Btn::Seek | Btn::Volume | Btn::Balance)) => {
                                    self.apply_classic_slider(b, nx)
                                }
                                _ => {}
                            }
                        }
                        ClWin::Eq => {
                            let r = classic::eq_region_at(nx, ny);
                            self.eq_active = r;
                            match r {
                                Some(classic::EqRegion::TitleBar) => {
                                    self.eq_active = None;
                                    self.start_drag(ctx, win, screen_cursor);
                                }
                                Some(er @ (classic::EqRegion::Preamp | classic::EqRegion::Band(_))) => {
                                    self.apply_eq_slider(er, ny)
                                }
                                _ => {}
                            }
                        }
                        ClWin::Pl => {
                            use classic::PlRegion as P;
                            match classic::pl_region_at(nx, ny, self.pl_px_h) {
                                Some(P::Close) => self.close_window(ctx, ClWin::Pl),
                                Some(P::TitleBar) => self.start_drag(ctx, win, screen_cursor),
                                Some(P::Resize) => self.resize_pl = Some(self.pl_px_h - ny),
                                Some(P::List) => {
                                    let row = classic::pl_row_at(ny, self.pl_first);
                                    if row < self.playlist.len() {
                                        let now = Instant::now();
                                        let dbl = self
                                            .pl_last_click
                                            .map(|(r, t)| r == row && now.duration_since(t).as_millis() < 350)
                                            .unwrap_or(false);
                                        if dbl {
                                            self.play_index(row);
                                        } else {
                                            self.pl_selection.clear();
                                            self.pl_selection.insert(row);
                                        }
                                        self.pl_last_click = Some((row, now));
                                    }
                                }
                                Some(menu @ (P::AddMenu | P::RemMenu | P::SelMenu | P::MiscMenu | P::ListMenu)) => {
                                    // Open on release (below) so this click can't dismiss it.
                                    self.pl_menu_press = Some(menu);
                                }
                                None => {}
                            }
                        }
                    }
                }
            }
        }

        if down {
            let idx = win_idx(win);
            let is_my_drag = self.drag.as_ref().map(|d| d.win == win).unwrap_or(false);
            if is_my_drag {
                let manual = self.drag.as_ref().unwrap().manual;
                if manual {
                    // Live move: drive position from the global cursor (feedback-
                    // free), moving the docked group in tandem each frame.
                    if let Some(g) = global_cursor_points(ctx) {
                        let d = self.drag.as_ref().unwrap();
                        let target = g - d.grab;
                        let group = d.group.clone();
                        let press_pos = d.press_pos;
                        self.win_pos[idx] = target;
                        for (mi, off) in group {
                            self.win_pos[mi] = target + off;
                        }
                        if (target - press_pos).length() > 3.0 {
                            if let Some(dm) = self.drag.as_mut() {
                                dm.moved = true;
                            }
                        }
                        if idx == 0 {
                            self.root_last = None; // keep root in sync while moving
                        }
                    }
                    ctx.request_repaint();
                } else if !self.drag.as_ref().unwrap().started_os {
                    // OS fallback: hand off to the OS window-drag past a threshold.
                    if let Some(sc) = screen_cursor {
                        if (sc - self.drag.as_ref().unwrap().press_screen).length() > 4.0 {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                            if let Some(dm) = self.drag.as_mut() {
                                dm.started_os = true;
                            }
                        }
                    }
                }
            }
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if let Some(r) = self.active_region {
                    if matches!(r, Btn::Seek | Btn::Volume | Btn::Balance) {
                        self.apply_classic_slider(r, nx);
                    }
                }
                if let Some(er) = self.eq_active {
                    self.apply_eq_slider(er, ny);
                }
                if let Some(grab) = self.resize_pl {
                    let step = 29.0;
                    let raw = (ny + grab).max(87.0);
                    self.pl_px_h = ((raw / step).round() * step).max(87.0);
                }
            }
        }

        // Finalize a drag once the button is up (covers release after the modal
        // OS drag, which may swallow the release event).
        if !down {
            if let Some(d) = self.drag.take() {
                if d.win == win {
                    self.finalize_drag(d, outer, monitor);
                } else {
                    self.drag = Some(d);
                }
            }
        }

        if released {
            self.resize_pl = None;
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if win == ClWin::Main {
                    if let Some(r) = self.active_region {
                        if classic::region_at(nx, ny) == Some(r) {
                            if r == Btn::Options {
                                self.open_popup_in(Popup::Options, win, l);
                            } else {
                                self.trigger_classic(ctx, r);
                            }
                        }
                    }
                    if self.active_region == Some(Btn::Seek) {
                        if let Some(p) = self.classic_seek_preview.take() {
                            self.player.seek(Duration::from_secs_f32(p));
                        }
                    }
                }
                if win == ClWin::Eq {
                    if let Some(er) = self.eq_active {
                        if classic::eq_region_at(nx, ny) == Some(er) {
                            self.trigger_eq(ctx, er);
                        }
                    }
                }
                if win == ClWin::Pl {
                    if let Some(r) = self.pl_menu_press.take() {
                        if classic::pl_region_at(nx, ny, self.pl_px_h) == Some(r) {
                            use classic::PlRegion as P;
                            let kind = match r {
                                P::AddMenu => Popup::PlAdd,
                                P::RemMenu => Popup::PlRem,
                                P::SelMenu => Popup::PlSel,
                                P::MiscMenu => Popup::PlMisc,
                                _ => Popup::PlList,
                            };
                            self.open_popup_in(kind, ClWin::Pl, l);
                        }
                    }
                }
            }
            self.active_region = None;
            self.eq_active = None;
            self.pl_menu_press = None;
        }

        if win == ClWin::Main {
            self.pressed_btn = if down {
                match self.active_region {
                    Some(b @ (Btn::Prev | Btn::Play | Btn::Pause | Btn::Stop | Btn::Next | Btn::Eject
                    | Btn::Shuffle | Btn::Repeat | Btn::Seek | Btn::Volume | Btn::Balance)) => Some(b),
                    _ => None,
                }
            } else {
                None
            };
        }
    }

    fn finalize_drag(&mut self, d: Dragging, outer: Option<egui::Rect>, monitor: Option<egui::Vec2>) {
        let idx = win_idx(d.win);
        let did_move = if d.manual { d.moved } else { d.started_os };
        if did_move {
            // OS fallback moved the window out from under win_pos; re-sync it.
            if !d.manual {
                if let Some(r) = outer {
                    self.win_pos[idx] = r.min;
                }
                for (mi, off) in &d.group {
                    self.win_pos[*mi] = self.win_pos[idx] + *off;
                }
            }
            // Magnetic snap to screen edges / other windows; move the group together.
            let before = self.win_pos[idx];
            let size = self.win_inner_size(d.win);
            let snapped = self.snap_pos(idx, before, size, monitor);
            let delta = snapped - before;
            self.win_pos[idx] = snapped;
            for (mi, _) in &d.group {
                self.win_pos[*mi] += delta;
            }
            if d.win == ClWin::Main {
                self.root_last = None;
            }
        } else {
            // A click with no move: double-click toggles the window shade.
            let now = Instant::now();
            let dbl = self
                .tb_last_click
                .map(|(w, t)| w == d.win && now.duration_since(t).as_millis() < 350)
                .unwrap_or(false);
            if dbl {
                self.toggle_shade(d.win);
                self.tb_last_click = None;
            } else {
                self.tb_last_click = Some((d.win, now));
            }
        }
    }

    fn win_native_h(&self, win: ClWin) -> f32 {
        match win {
            ClWin::Main => self.main_h(),
            ClWin::Eq => self.eq_h(),
            ClWin::Pl => {
                if self.pl_shade {
                    14.0
                } else {
                    self.pl_px_h
                }
            }
        }
    }

    fn win_inner_size(&self, win: ClWin) -> egui::Vec2 {
        egui::vec2(classic::MAIN_W as f32 * self.scale, self.win_native_h(win) * self.scale)
    }

    fn start_drag(&mut self, ctx: &egui::Context, win: ClWin, screen_cursor: Option<egui::Pos2>) {
        if let Some(sc) = screen_cursor {
            // Only the main window carries a group: the set of windows actually
            // docked (edge-flush) to it. EQ/playlist always move on their own,
            // even when docked to each other.
            let group = if win == ClWin::Main {
                self.main_dock_group()
            } else {
                Vec::new()
            };
            let idx = win_idx(win);
            let (manual, grab) = match global_cursor_points(ctx) {
                Some(g) => (true, g - self.win_pos[idx]),
                None => (false, egui::Vec2::ZERO),
            };
            self.drag = Some(Dragging {
                win,
                manual,
                grab,
                press_pos: self.win_pos[idx],
                press_screen: sc,
                started_os: false,
                moved: false,
                group,
            });
        }
    }

    fn win_rect_pts(&self, idx: usize) -> egui::Rect {
        let w = match idx {
            0 => ClWin::Main,
            1 => ClWin::Eq,
            _ => ClWin::Pl,
        };
        egui::Rect::from_min_size(self.win_pos[idx], self.win_inner_size(w))
    }

    fn win_visible(&self, idx: usize) -> bool {
        match idx {
            0 => true,
            1 => self.eq_visible,
            2 => self.pl_visible,
            _ => false,
        }
    }

    /// Are windows `a` and `b` docked — edge-flush and overlapping on the shared axis?
    fn is_docked(&self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.win_rect_pts(a), self.win_rect_pts(b));
        let tol = 3.0;
        let x_overlap = ra.left() < rb.right() - 1.0 && ra.right() > rb.left() + 1.0;
        let y_overlap = ra.top() < rb.bottom() - 1.0 && ra.bottom() > rb.top() + 1.0;
        let v_adj = x_overlap
            && ((ra.bottom() - rb.top()).abs() <= tol || (ra.top() - rb.bottom()).abs() <= tol);
        let h_adj = y_overlap
            && ((ra.right() - rb.left()).abs() <= tol || (ra.left() - rb.right()).abs() <= tol);
        v_adj || h_adj
    }

    /// Windows transitively docked to the main window (by offset from main).
    fn main_dock_group(&self) -> Vec<(usize, egui::Vec2)> {
        let mut in_group = [true, false, false];
        let mut changed = true;
        while changed {
            changed = false;
            for j in [1usize, 2usize] {
                if in_group[j] || !self.win_visible(j) {
                    continue;
                }
                if (0..3).any(|k| in_group[k] && self.is_docked(j, k)) {
                    in_group[j] = true;
                    changed = true;
                }
            }
        }
        [1usize, 2usize]
            .into_iter()
            .filter(|&j| in_group[j])
            .map(|j| (j, self.win_pos[j] - self.win_pos[0]))
            .collect()
    }

    /// Canonical stacking: EQ then playlist dock directly below the main window,
    /// in order, regardless of which was opened first.
    fn restack(&mut self) {
        let base = self.win_pos[0];
        let mut y = base.y + self.main_h() * self.scale;
        if self.eq_visible {
            self.win_pos[1] = egui::pos2(base.x, y);
            y += self.eq_h() * self.scale;
        }
        if self.pl_visible {
            self.win_pos[2] = egui::pos2(base.x, y);
        }
    }

    /// Magnetic snap of `pos` (rect of `size`) to screen edges and other windows.
    fn snap_pos(
        &self,
        idx: usize,
        pos: egui::Pos2,
        size: egui::Vec2,
        monitor: Option<egui::Vec2>,
    ) -> egui::Pos2 {
        use egui::{vec2, Rect};
        let s = SNAP;
        let mut r = Rect::from_min_size(pos, size);
        if let Some(m) = monitor {
            if r.left().abs() < s {
                r = r.translate(vec2(-r.left(), 0.0));
            }
            if (m.x - r.right()).abs() < s {
                r = r.translate(vec2(m.x - r.right(), 0.0));
            }
            if r.top().abs() < s {
                r = r.translate(vec2(0.0, -r.top()));
            }
            if (m.y - r.bottom()).abs() < s {
                r = r.translate(vec2(0.0, m.y - r.bottom()));
            }
        }
        for j in 0..3 {
            if j == idx {
                continue;
            }
            let vis = match j {
                0 => true,
                1 => self.eq_visible,
                2 => self.pl_visible,
                _ => false,
            };
            if !vis {
                continue;
            }
            {
                let jwin = match j {
                    0 => ClWin::Main,
                    1 => ClWin::Eq,
                    _ => ClWin::Pl,
                };
                let o = Rect::from_min_size(self.win_pos[j], self.win_inner_size(jwin));
                let xover = r.left() < o.right() + s && r.right() > o.left() - s;
                if xover {
                    if (r.top() - o.bottom()).abs() < s {
                        r = r.translate(vec2(0.0, o.bottom() - r.top()));
                    }
                    if (r.bottom() - o.top()).abs() < s {
                        r = r.translate(vec2(0.0, o.top() - r.bottom()));
                    }
                    if (r.left() - o.left()).abs() < s {
                        r = r.translate(vec2(o.left() - r.left(), 0.0));
                    }
                }
                let yover = r.top() < o.bottom() + s && r.bottom() > o.top() - s;
                if yover {
                    if (r.left() - o.right()).abs() < s {
                        r = r.translate(vec2(o.right() - r.left(), 0.0));
                    }
                    if (r.right() - o.left()).abs() < s {
                        r = r.translate(vec2(o.left() - r.right(), 0.0));
                    }
                    if (r.top() - o.top()).abs() < s {
                        r = r.translate(vec2(0.0, o.top() - r.top()));
                    }
                }
            }
        }
        r.min
    }

    fn close_window(&mut self, ctx: &egui::Context, win: ClWin) {
        match win {
            ClWin::Main => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            ClWin::Eq => self.eq_visible = false,
            ClWin::Pl => self.pl_visible = false,
        }
    }

    fn toggle_shade(&mut self, win: ClWin) {
        match win {
            ClWin::Main => self.main_shade = !self.main_shade,
            ClWin::Eq => self.eq_shade = !self.eq_shade,
            ClWin::Pl => self.pl_shade = !self.pl_shade,
        }
        // Keep the docked stack consistent and resize the root window.
        self.restack();
        self.root_last = None;
    }

    fn set_double_size(&mut self, _ctx: &egui::Context, dbl: bool) {
        if self.prefs.double_size == dbl {
            return;
        }
        self.prefs.double_size = dbl;
        self.scale = self.prefs.scale();
        self.prefs.save();
        self.restack();
        self.root_last = None; // force the root window to resize next frame
    }

    fn ui_prefs(&mut self, ctx: &egui::Context) {
        if !self.prefs_open {
            return;
        }
        let mut open = true;
        egui::Window::new("LinAmp Preferences")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.strong("Display size");
                let mut dbl = self.prefs.double_size;
                ui.radio_value(&mut dbl, false, "1x — pixel-for-pixel");
                ui.radio_value(&mut dbl, true, "2x — scaled for modern screens (Ctrl+D)");
                if dbl != self.prefs.double_size {
                    self.set_double_size(ctx, dbl);
                }
                ui.add_space(8.0);
                ui.strong("ID3 tag loading");
                let mut onload = self.prefs.tag_on_load;
                ui.radio_value(&mut onload, true, "On load — scan when files are added");
                ui.radio_value(&mut onload, false, "On view — lazily as rows scroll in");
                if onload != self.prefs.tag_on_load {
                    self.prefs.tag_on_load = onload;
                    self.prefs.save();
                    if onload {
                        self.playlist.load_all();
                    }
                }
            });
        self.prefs_open = open;
    }

    fn apply_eq_slider(&mut self, region: classic::EqRegion, ey: f32) {
        let v = classic::eq_value_at_y(ey);
        match region {
            classic::EqRegion::Preamp => {
                self.eq_preamp_db = v * 12.0;
                self.apply_eq();
            }
            classic::EqRegion::Band(i) if i < 10 => self.eq_bands[i] = v * 12.0,
            _ => {}
        }
    }

    fn render_classic(&mut self, ctx: &egui::Context) {
        if self.classic.is_none() {
            return;
        }

        // Root viewport = main window.
        self.render_window(ctx, ClWin::Main);
        self.ui_prefs(ctx);

        // Initialise window positions from the real main window, once.
        if !self.positions_init {
            if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
                self.win_pos[0] = r.min;
                self.win_pos[1] = egui::pos2(r.min.x, r.min.y + self.main_h() * self.scale);
                self.win_pos[2] =
                    egui::pos2(r.min.x, r.min.y + (self.main_h() + self.eq_h()) * self.scale);
                self.positions_init = true;
            }
        }

        // Apply root position/size only when we changed them (avoids fighting OS).
        let size = self.win_inner_size(ClWin::Main);
        let pos = self.win_pos[0];
        if self.root_last != Some((pos, size)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            if self.positions_init {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            }
            self.root_last = Some((pos, size));
        }

        // Equalizer child window.
        if self.eq_visible {
            let builder = egui::ViewportBuilder::default()
                .with_title("LinAmp Equalizer")
                .with_inner_size(self.win_inner_size(ClWin::Eq))
                .with_position(self.win_pos[1])
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false);
            let id = egui::ViewportId::from_hash_of("linamp_eq");
            ctx.show_viewport_immediate(id, builder, |ctx, _| {
                self.render_window(ctx, ClWin::Eq);
            });
        }

        // Playlist child window.
        if self.pl_visible {
            let builder = egui::ViewportBuilder::default()
                .with_title("LinAmp Playlist")
                .with_inner_size(self.win_inner_size(ClWin::Pl))
                .with_position(self.win_pos[2])
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false);
            let id = egui::ViewportId::from_hash_of("linamp_pl");
            ctx.show_viewport_immediate(id, builder, |ctx, _| {
                self.render_window(ctx, ClWin::Pl);
            });
        }
    }

    fn render_window(&mut self, ctx: &egui::Context, win: ClWin) {
        self.handle_window_input(ctx, win);
        self.handle_dropped(ctx);

        // Lazily read tags for the playlist rows about to be shown (on-view mode).
        if win == ClWin::Pl && !self.pl_shade {
            let vis = classic::pl_visible_rows(self.pl_px_h as u32);
            let end = (self.pl_first + vis).min(self.playlist.len());
            for i in self.pl_first..end {
                self.playlist.ensure_loaded(i);
            }
        }

        let img = self.compose_window(win);
        let name = match win {
            ClWin::Main => "cl_main",
            ClWin::Eq => "cl_eq",
            ClWin::Pl => "cl_pl",
        };
        let slot = match win {
            ClWin::Main => &mut self.classic_tex,
            ClWin::Eq => &mut self.eq_tex,
            ClWin::Pl => &mut self.pl_tex,
        };
        let tex = upload_tex(slot, ctx, &img, name);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
            });

        self.draw_popup_in(ctx, win);
    }

    fn compose_window(&self, win: ClWin) -> image::RgbaImage {
        let cs = self.classic.as_ref().unwrap();
        match win {
            ClWin::Main => {
                let st = self.make_main_state();
                if self.main_shade {
                    cs.compose_main_shade(&st)
                } else {
                    cs.compose_main(&st)
                }
            }
            ClWin::Eq => {
                if self.eq_shade {
                    cs.compose_eq_shade(true)
                } else {
                    cs.compose_eq(&self.make_eq_state())
                }
            }
            ClWin::Pl => {
                if self.pl_shade {
                    cs.compose_pl_shade(true)
                } else {
                    let rows = self.build_pl_rows();
                    let colors = self.pl_colors();
                    let st = classic::PlState {
                        focused: true,
                        colors,
                        rows: &rows,
                        first: self.pl_first,
                    };
                    cs.compose_playlist(&st, self.pl_px_h as u32)
                }
            }
        }
    }

    // ---- skins / theming ---------------------------------------------------

    fn load_skin_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Winamp skin", &["wsz", "zip"])
            .pick_file()
        {
            self.apply_skin_path(ctx, &path);
        }
    }

    fn ui_skins(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(format!("Skin: {}", self.theme.name))
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Default").clicked() {
                        self.theme = Theme::default();
                        apply_theme(ui.ctx(), &self.theme);
                        self.status = "Theme: default".to_string();
                    }
                    if ui.button("Load .wsz...").clicked() {
                        self.load_skin_dialog(ui.ctx());
                    }
                    if ui.button("Rescan").clicked() {
                        self.discover_skins();
                    }
                });
                if self.skins.is_empty() {
                    ui.label(
                        RichText::new("No skins found in ./skins — use \"Load .wsz...\".")
                            .italics()
                            .size(10.0)
                            .color(Color32::GRAY),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    for path in self.skins.clone() {
                        let name = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if ui.button(name).clicked() {
                            self.apply_skin_path(ui.ctx(), &path);
                        }
                    }
                });
            });
    }

    // ---- keyboard ----------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
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
            k.key_d = i.key_pressed(Key::D);
            k.key_p = i.key_pressed(Key::P);
        });

        if k.play {
            self.play_pause();
        }
        if k.pause {
            self.player.pause();
        }
        if k.stop {
            self.player.stop();
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
        if k.open {
            self.open_files();
        }
        if k.repeat {
            self.repeat_all = !self.repeat_all;
        }
        if k.shuffle {
            self.shuffle = !self.shuffle;
        }
        if k.vol_up {
            self.adjust_volume(0.05);
        }
        if k.vol_down {
            self.adjust_volume(-0.05);
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
    }

    // ---- UI sections -------------------------------------------------------

    fn ui_display(&mut self, ui: &mut egui::Ui, dt: f32) {
        let (title, dur) = match self.playlist.current_track() {
            Some(t) => (t.display_name(), t.duration),
            None => ("LinAmp - no track loaded".to_string(), None),
        };

        let pos = if self.seeking {
            Duration::from_secs_f32(self.seek_preview)
        } else {
            self.player.position()
        };

        let lcd = self.theme.lcd;
        let lcd_dim = self.theme.lcd_dim;
        egui::Frame::none()
            .fill(self.theme.list_bg)
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .stroke(Stroke::new(1.0_f32, lcd_dim))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.ui_album_art(ui, 64.0);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let state_str = match self.player.state() {
                                PlayState::Playing => "\u{25B6}",
                                PlayState::Paused => "II",
                                PlayState::Stopped => "\u{25A0}",
                            };
                            ui.label(RichText::new(state_str).color(lcd).monospace().size(16.0));
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(fmt_time(pos)).color(lcd).monospace().size(22.0),
                            );
                            if let Some(d) = dur {
                                ui.label(
                                    RichText::new(format!("/ {}", fmt_time(d)))
                                        .color(lcd_dim)
                                        .monospace()
                                        .size(14.0),
                                );
                            }
                        });

                        let shown = marquee(&title, self.marquee_offset);
                        ui.label(RichText::new(shown).color(lcd).monospace().size(14.0));

                        if let Some(t) = self.playlist.current_track() {
                            let khz = t
                                .sample_rate
                                .map(|s| format!("{} kHz", s / 1000))
                                .unwrap_or_default();
                            let kbps =
                                t.bitrate.map(|b| format!("{b} kbps")).unwrap_or_default();
                            let ch = match t.channels {
                                Some(1) => "mono",
                                Some(_) => "stereo",
                                None => "",
                            };
                            ui.label(
                                RichText::new(format!("{kbps}   {khz}   {ch}"))
                                    .color(lcd_dim)
                                    .monospace()
                                    .size(11.0),
                            );
                        }
                    });
                });
            });

        let _ = dt;
    }

    fn ui_album_art(&mut self, ui: &mut egui::Ui, size: f32) {
        if let Some(tex) = &self.album_art {
            let sized = egui::load::SizedTexture::new(tex.id(), egui::vec2(size, size));
            ui.add(egui::Image::new(sized));
        } else {
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(size, size), egui::Sense::hover());
            painter.rect_filled(resp.rect, 3.0, self.theme.panel);
            painter.text(
                resp.rect.center(),
                egui::Align2::CENTER_CENTER,
                "\u{266B}",
                egui::FontId::proportional(size * 0.5),
                self.theme.lcd_dim,
            );
        }
    }

    fn ui_viz(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) =
            ui.allocate_painter(egui::vec2(ui.available_width(), 60.0), egui::Sense::click());
        if resp.clicked() {
            self.viz_mode = self.viz_mode.next();
        }
        let rect = resp.rect;
        painter.rect_filled(rect, 2.0, self.theme.viz_bg);
        painter.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, self.theme.lcd_dim));

        match self.viz_mode {
            VizMode::Off => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "visualization off (click to cycle)",
                    egui::FontId::proportional(11.0),
                    self.theme.lcd_dim,
                );
            }
            VizMode::Spectrum => {
                self.viz.snapshot(&mut self.viz_samples, FFT_SIZE);
                let rate = self.viz.rate();
                let bars =
                    viz::compute_bars(&self.viz_samples, &*self.fft, FFT_SIZE, VIZ_BANDS, rate);
                for (i, b) in self.viz_bars.iter_mut().enumerate() {
                    let target = bars.get(i).copied().unwrap_or(0.0);
                    if target > *b {
                        *b = target;
                    } else {
                        *b = (*b * 0.82).max(target); // peak decay
                    }
                }
                let n = self.viz_bars.len();
                let gap = 2.0;
                let bw = ((rect.width() - gap * (n as f32 + 1.0)) / n as f32).max(1.0);
                for (i, &b) in self.viz_bars.iter().enumerate() {
                    let x0 = rect.left() + gap + i as f32 * (bw + gap);
                    let h = b * (rect.height() - 4.0);
                    let bar = egui::Rect::from_min_max(
                        egui::pos2(x0, rect.bottom() - 2.0 - h),
                        egui::pos2(x0 + bw, rect.bottom() - 2.0),
                    );
                    let spec = &self.theme.viz_spectrum;
                    let col = if spec.is_empty() {
                        self.theme.lcd
                    } else {
                        let idx = (((1.0 - b) * (spec.len() - 1) as f32).round() as usize)
                            .min(spec.len() - 1);
                        spec[idx]
                    };
                    painter.rect_filled(bar, 0.0, col);
                }
            }
            VizMode::Oscilloscope => {
                let n = (rect.width() as usize).clamp(16, 2048);
                self.viz.snapshot(&mut self.viz_samples, n);
                let mid = rect.center().y;
                let amp = (rect.height() / 2.0) - 3.0;
                let denom = (self.viz_samples.len().max(2) - 1) as f32;
                let step = rect.width() / denom;
                let pts: Vec<egui::Pos2> = self
                    .viz_samples
                    .iter()
                    .enumerate()
                    .map(|(i, &s)| {
                        egui::pos2(rect.left() + i as f32 * step, mid - s.clamp(-1.0, 1.0) * amp)
                    })
                    .collect();
                if pts.len() >= 2 {
                    painter.add(egui::Shape::line(pts, Stroke::new(1.5_f32, self.theme.viz_osc)));
                }
            }
        }
    }

    fn ui_seekbar(&mut self, ui: &mut egui::Ui) {
        let dur = self
            .playlist
            .current_track()
            .and_then(|t| t.duration)
            .map(|d| d.as_secs_f32())
            .unwrap_or(0.0);

        if !self.seeking {
            self.seek_preview = self.player.position().as_secs_f32();
        }

        let max = if dur > 0.0 { dur } else { 1.0 };
        self.seek_preview = self.seek_preview.clamp(0.0, max);
        let resp = ui.add_enabled(
            dur > 0.0,
            egui::Slider::new(&mut self.seek_preview, 0.0..=max)
                .show_value(false)
                .trailing_fill(true),
        );
        if resp.dragged() {
            self.seeking = true;
        }
        if resp.drag_stopped() {
            self.player.seek(Duration::from_secs_f32(self.seek_preview));
            self.seeking = false;
        }
    }

    fn ui_transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(RichText::new("|<").monospace()).on_hover_text("Previous (Z)").clicked() {
                self.prev();
            }
            if ui.button(RichText::new(" > ").monospace()).on_hover_text("Play (X)").clicked() {
                self.play_pause();
            }
            if ui.button(RichText::new("II").monospace()).on_hover_text("Pause (C)").clicked() {
                self.player.pause();
            }
            if ui.button(RichText::new("[]").monospace()).on_hover_text("Stop (V)").clicked() {
                self.player.stop();
            }
            if ui.button(RichText::new(">|").monospace()).on_hover_text("Next (B)").clicked() {
                self.next();
            }

            ui.separator();

            let mut vol = self.player.volume();
            ui.label("Vol");
            if ui
                .add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false))
                .changed()
            {
                self.player.set_volume(vol);
            }
        });

        ui.horizontal(|ui| {
            toggle(ui, &mut self.shuffle, "Shuffle (S)");
            toggle(ui, &mut self.repeat_all, "Repeat (R)");
            toggle(ui, &mut self.repeat_one, "Repeat 1");
            ui.separator();
            if ui.button("Open files (L)").clicked() {
                self.open_files();
            }
            if ui.button("Open folder").clicked() {
                self.open_folder();
            }
        });
    }

    fn ui_equalizer(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Equalizer")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if toggle(ui, &mut self.eq_enabled, "EQ On").changed() {
                        self.apply_eq();
                    }
                    ui.separator();
                    ui.label("Preamp");
                    if ui
                        .add(egui::Slider::new(&mut self.eq_preamp_db, -12.0..=12.0).suffix(" dB"))
                        .changed()
                    {
                        self.apply_eq();
                    }
                    if ui.button("Reset").clicked() {
                        self.eq_bands = [0.0; 10];
                        self.eq_preamp_db = 0.0;
                        self.apply_eq();
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for (i, label) in EQ_LABELS.iter().enumerate() {
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Slider::new(&mut self.eq_bands[i], -12.0..=12.0)
                                    .vertical()
                                    .show_value(false),
                            );
                            ui.label(RichText::new(*label).size(9.0).color(self.theme.lcd_dim));
                        });
                    }
                });
                ui.label(
                    RichText::new(
                        "Note: per-band DSP is phase 2; preamp currently drives output gain.",
                    )
                    .size(10.0)
                    .italics()
                    .color(Color32::GRAY),
                );
            });
    }

    fn apply_eq(&mut self) {
        let preamp = if self.eq_enabled { self.eq_preamp_db } else { 0.0 };
        self.player.set_preamp_db(preamp);
    }

    fn ui_playlist(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Find:");
            ui.add(
                egui::TextEdit::singleline(&mut self.pl_search)
                    .hint_text("filter playlist")
                    .desired_width(130.0),
            );
            if !self.pl_search.is_empty() && ui.button("x").clicked() {
                self.pl_search.clear();
            }
            ui.separator();
            ui.label("Sort:");
            let prev = self.sort_key;
            egui::ComboBox::from_id_salt("sort_combo")
                .selected_text(self.sort_key.label())
                .show_ui(ui, |ui| {
                    for k in [
                        SortKey::Custom,
                        SortKey::Title,
                        SortKey::Artist,
                        SortKey::Album,
                        SortKey::Duration,
                    ] {
                        ui.selectable_value(&mut self.sort_key, k, k.label());
                    }
                });
            if self.sort_key != prev {
                self.sort_playlist();
            }
        });

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} tracks", self.playlist.len())).color(Color32::GRAY),
            );
            if ui.button("Save").clicked() {
                self.save_playlist_dialog();
            }
            if ui.button("Load").clicked() {
                self.load_playlist_dialog();
            }
            if ui.button("Clear").clicked() {
                self.playlist.clear();
                self.player.stop();
            }
        });
        ui.separator();

        let filtering = !self.pl_search.trim().is_empty();
        let needle = self.pl_search.to_lowercase();
        let current = self.playlist.current;

        let mut to_play: Option<usize> = None;
        let mut to_remove: Option<usize> = None;
        let mut reorder: Option<(usize, usize)> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, track) in self.playlist.tracks.iter().enumerate() {
                    if filtering && !track.display_name().to_lowercase().contains(&needle) {
                        continue;
                    }
                    let is_cur = Some(i) == current;

                    let row = ui
                        .horizontal(|ui| {
                            if !filtering {
                                ui.dnd_drag_source(Id::new(("pl_grip", i)), i, |ui| {
                                    ui.label(RichText::new("\u{2630}").color(Color32::GRAY));
                                });
                            }

                            ui.label(
                                RichText::new(format!("{:>3}.", i + 1))
                                    .monospace()
                                    .color(Color32::GRAY),
                            );

                            let name = track.display_name();
                            let text = if is_cur {
                                RichText::new(name).color(self.theme.list_current).strong()
                            } else {
                                RichText::new(name).color(self.theme.list_normal)
                            };
                            let resp = ui.add(
                                egui::Label::new(text).sense(egui::Sense::click()).truncate(),
                            );
                            if resp.double_clicked() {
                                to_play = Some(i);
                            }
                            resp.context_menu(|ui| {
                                if ui.button("Play").clicked() {
                                    to_play = Some(i);
                                    ui.close_menu();
                                }
                                if ui.button("Remove").clicked() {
                                    to_remove = Some(i);
                                    ui.close_menu();
                                }
                            });

                            if let Some(d) = track.duration {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(fmt_time(d))
                                                .monospace()
                                                .color(Color32::GRAY),
                                        );
                                    },
                                );
                            }
                        })
                        .response;

                    // Drag-and-drop reordering (only in unfiltered view).
                    if !filtering {
                        if row.dnd_hover_payload::<usize>().is_some() {
                            let rect = row.rect;
                            let above = ui
                                .input(|inp| inp.pointer.interact_pos())
                                .map(|p| p.y < rect.center().y)
                                .unwrap_or(false);
                            let y = if above { rect.top() } else { rect.bottom() };
                            ui.painter().hline(
                                rect.x_range(),
                                y,
                                Stroke::new(2.0_f32, self.theme.accent),
                            );
                        }
                        if let Some(payload) = row.dnd_release_payload::<usize>() {
                            let rect = row.rect;
                            let above = ui
                                .input(|inp| inp.pointer.interact_pos())
                                .map(|p| p.y < rect.center().y)
                                .unwrap_or(false);
                            let target = if above { i } else { i + 1 };
                            reorder = Some((*payload, target));
                        }
                    }
                }
            });

        if let Some(i) = to_play {
            self.play_index(i);
        }
        if let Some(i) = to_remove {
            self.playlist.remove(i);
        }
        if let Some((from, to)) = reorder {
            self.move_track(from, to);
        }
    }

    fn ui_library(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Search:");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.lib_query)
                        .hint_text("title / artist / album")
                        .desired_width(160.0),
                )
                .changed()
            {
                self.refresh_library();
            }
            if ui.button("Scan folder...").clicked() {
                self.scan_into_library();
            }
        });

        ui.horizontal(|ui| {
            match &self.library {
                Some(lib) => ui.label(
                    RichText::new(format!("{} tracks in library", lib.count()))
                        .color(Color32::GRAY),
                ),
                None => ui.colored_label(Color32::LIGHT_RED, "Library database unavailable"),
            };
            if !self.lib_results.is_empty() && ui.button("Add all shown to playlist").clicked() {
                let was_empty = self.playlist.is_empty();
                for t in self.lib_results.clone() {
                    self.playlist.tracks.push(t);
                }
                if was_empty && !self.playlist.is_empty() {
                    self.play_index(0);
                }
                self.status = format!("{} track(s) in playlist.", self.playlist.len());
            }
        });
        ui.separator();

        let mut add_play: Option<usize> = None;
        let mut add_only: Option<usize> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.lib_results.is_empty() {
                    ui.label(
                        RichText::new("No results. Use \"Scan folder...\" to index your music.")
                            .color(Color32::GRAY)
                            .italics(),
                    );
                }
                for (i, track) in self.lib_results.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let resp = ui.add(
                            egui::Label::new(
                                RichText::new(track.display_name()).color(Color32::LIGHT_GRAY),
                            )
                            .sense(egui::Sense::click())
                            .truncate(),
                        );
                        if resp.double_clicked() {
                            add_play = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Add to playlist").clicked() {
                                add_only = Some(i);
                                ui.close_menu();
                            }
                            if ui.button("Add & play").clicked() {
                                add_play = Some(i);
                                ui.close_menu();
                            }
                        });
                        if let Some(d) = track.duration {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(fmt_time(d)).monospace().color(Color32::GRAY),
                                    );
                                },
                            );
                        }
                    });
                }
            });

        if let Some(i) = add_only {
            if let Some(t) = self.lib_results.get(i).cloned() {
                self.playlist.tracks.push(t);
                self.status = format!("{} track(s) in playlist.", self.playlist.len());
            }
        }
        if let Some(i) = add_play {
            if let Some(t) = self.lib_results.get(i).cloned() {
                self.playlist.tracks.push(t);
                let idx = self.playlist.tracks.len() - 1;
                self.play_index(idx);
            }
        }
    }
}

impl eframe::App for LinAmpApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        if self.mode_init {
            let m = self.ui_mode;
            self.set_mode(ctx, m);
            self.mode_init = false;
        }
        if self.player.state() == PlayState::Playing {
            self.marquee_offset = (self.marquee_offset + dt * 4.0) % 100000.0;
        }

        // Follow the system default output device if it changes mid-session.
        if now.duration_since(self.last_device_check).as_secs_f32() > 1.5 {
            self.player.poll_device();
            self.last_device_check = now;
        }

        self.handle_shortcuts(ctx);

        if self.player.finished() {
            if self.repeat_one {
                if let Some(i) = self.playlist.current {
                    self.play_index(i);
                }
            } else {
                self.next();
            }
        }

        // Decode any pending cover art into a GPU texture (needs ctx).
        if self.art_dirty {
            self.album_art = self.art_bytes.as_ref().and_then(|b| load_texture(ctx, b));
            self.art_dirty = false;
        }

        // Classic skin view takes over the whole (borderless) window.
        if self.ui_mode == UiMode::Classic && self.classic.is_some() {
            self.render_classic(ctx);
            ctx.request_repaint_after(Duration::from_millis(33));
            return;
        }

        // Modern fallback window: handle drops here.
        self.handle_dropped(ctx);

        egui::TopBottomPanel::top("player").show(ctx, |ui| {
            ui.add_space(4.0);
            self.ui_display(ui, dt);
            ui.add_space(4.0);
            self.ui_viz(ui);
            ui.add_space(4.0);
            self.ui_seekbar(ui);
            self.ui_transport(ui);
            ui.add_space(2.0);
            self.ui_equalizer(ui);
            self.ui_skins(ui);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.status).size(11.0).color(Color32::GRAY));
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let prev_view = self.view;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.view, View::Playlist, "Playlist");
                ui.selectable_value(&mut self.view, View::Library, "Library");
            });
            if self.view != prev_view && self.view == View::Library {
                self.refresh_library();
            }
            ui.separator();
            match self.view {
                View::Playlist => self.ui_playlist(ui),
                View::Library => self.ui_library(ui),
            }
        });

        let interval =
            if self.player.state() == PlayState::Playing && self.viz_mode != VizMode::Off {
                33
            } else {
                100
            };
        ctx.request_repaint_after(Duration::from_millis(interval));
    }
}

/// Upload (or update in place) a composed RGBA frame into a nearest-neighbour
/// texture, returning a handle to draw with.
fn upload_tex(
    slot: &mut Option<egui::TextureHandle>,
    ctx: &egui::Context,
    img: &image::RgbaImage,
    name: &str,
) -> egui::TextureHandle {
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    );
    match slot {
        Some(t) => {
            t.set(color, egui::TextureOptions::NEAREST);
            t.clone()
        }
        None => {
            let t = ctx.load_texture(name, color, egui::TextureOptions::NEAREST);
            *slot = Some(t.clone());
            t
        }
    }
}

/// Decode encoded image bytes into an egui texture.
fn load_texture(ctx: &egui::Context, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture("cover", color, egui::TextureOptions::LINEAR))
}

// ---- free helpers ----------------------------------------------------------

fn fmt_time(d: Duration) -> String {
    let total = d.as_secs();
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// Produce a horizontally scrolled view of `text` for the LCD marquee.
fn marquee(text: &str, offset: f32) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 40 {
        return text.to_string();
    }
    let padded: Vec<char> = text.chars().chain("      ".chars()).collect();
    let start = (offset as usize) % padded.len();
    padded.iter().cycle().skip(start).take(40).collect()
}

/// Scroll `text` within a fixed character `width` (Winamp-style marquee).
fn scroll_text(text: &str, offset: f32, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let padded: Vec<char> = text.chars().chain("   *** ".chars()).collect();
    let start = (offset as usize) % padded.len();
    padded.iter().cycle().skip(start).take(width).collect()
}

/// A Winamp-style on/off toggle rendered as a selectable label.
fn toggle(ui: &mut egui::Ui, value: &mut bool, label: &str) -> egui::Response {
    let mut resp = ui.selectable_label(*value, label);
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    resp
}

fn apply_theme(ctx: &egui::Context, t: &Theme) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = t.bg;
    visuals.window_fill = t.bg;
    visuals.extreme_bg_color = t.list_bg;
    visuals.faint_bg_color = t.panel;
    visuals.widgets.inactive.bg_fill = t.panel;
    visuals.widgets.hovered.bg_fill = skin::blend(t.panel, Color32::WHITE, 0.12);
    visuals.widgets.active.bg_fill = t.accent;
    visuals.selection.bg_fill = t.list_sel_bg;
    visuals.override_text_color = Some(skin::contrast_text(t.bg));
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.slider_width = 120.0;
    ctx.set_style(style);
}
