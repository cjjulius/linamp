//! Playlist-view helpers (selection, navigation, scroll, remove, colours) and
//! bookmarks, split out of app.rs as methods on `LinAmpApp`. Behavior is unchanged.

use super::*;

impl super::LinAmpApp {
    /// Bookmark the current track (the classic player Alt+I). No-op with nothing
    /// playing, and a duplicate path is skipped.
    pub(super) fn add_bookmark(&mut self) {
        let (path, title) = match self.playlist.current_track() {
            Some(t) => (t.path.to_string_lossy().into_owned(), t.display_name()),
            None => {
                self.status = "Nothing playing to bookmark".into();
                return;
            }
        };
        if self.bookmarks.add(path, title.clone()) {
            self.bookmarks.save();
            self.status = format!("Bookmarked: {title}");
            self.refresh_bookmarks_view_if_shown();
        } else {
            self.status = "Already bookmarked".into();
        }
    }

    /// Play a bookmarked track, appending it to the playlist first if it isn't
    /// already there.
    pub(super) fn play_bookmark(&mut self, idx: usize) {
        let Some(bm) = self.bookmarks.items.get(idx) else {
            return;
        };
        let path = PathBuf::from(&bm.path);
        let title = bm.title.clone();
        match self.playlist.tracks.iter().position(|t| t.path == path) {
            Some(i) => self.play_index(i),
            None if path.exists() => {
                self.playlist.tracks.push(crate::playlist::Track::new(path));
                let last = self.playlist.len() - 1;
                self.play_index(last);
            }
            // A moved/deleted bookmark shouldn't drop a permanently-unplayable row
            // into the playlist, just report it. (URL bookmarks wait on #15.)
            None => self.status = format!("Bookmark file not found: {title}"),
        }
    }

    /// Path of the currently-playing track as stored in bookmarks, if any.
    pub(super) fn current_bookmark_path(&self) -> Option<String> {
        self.playlist
            .current_track()
            .map(|t| t.path.to_string_lossy().into_owned())
    }

    /// Whether the current track is already bookmarked.
    pub(super) fn current_is_bookmarked(&self) -> bool {
        match self.current_bookmark_path() {
            Some(p) => self.bookmarks.items.iter().any(|b| b.path == p),
            None => false,
        }
    }

    /// Remove the current track's bookmark.
    pub(super) fn remove_current_bookmark(&mut self) {
        if let Some(p) = self.current_bookmark_path() {
            self.bookmarks.remove_path(&p);
            self.bookmarks.save();
            self.status = "Removed bookmark".into();
            self.refresh_bookmarks_view_if_shown();
        }
    }

    /// Clear every bookmark.
    pub(super) fn clear_bookmarks(&mut self) {
        self.bookmarks.items.clear();
        self.bookmarks.save();
        self.status = "Bookmarks cleared".into();
        self.refresh_bookmarks_view_if_shown();
    }

    /// Rebuild the Media Library Bookmarks list after the bookmark store changes,
    /// so an open library on that view reflects the add/remove/clear immediately.
    pub(super) fn refresh_bookmarks_view_if_shown(&mut self) {
        if self.lib.nav == crate::library::LibView::Bookmarks {
            self.refresh_library();
        }
    }

    pub(super) fn pl_select_all(&mut self) {
        self.plview.selection = (0..self.playlist.len()).collect();
        self.plview.anchor = Some(0);
        self.plview.lead = self.playlist.len().checked_sub(1);
    }

    /// Apply a selection gesture landing on `target`, honoring Ctrl and Shift.
    /// `ctrl_mode` differs by input source: mouse ctrl-click toggles a row,
    /// keyboard ctrl-nav adds it. The lead row is recorded and scrolled in.
    pub(super) fn pl_select_with(
        &mut self,
        target: usize,
        ctrl: bool,
        shift: bool,
        ctrl_mode: crate::selection::CtrlMode,
    ) {
        if self.playlist.is_empty() {
            return;
        }
        let max = self.playlist.len() - 1;
        let target = target.min(max);
        // Clamp a stale anchor (the list may have shrunk since it was set) so a
        // shift-range can't run past the end.
        let anchor = self.plview.anchor.map(|a| a.min(max));
        self.plview.anchor = Some(crate::selection::apply(
            &mut self.plview.selection,
            anchor,
            target,
            ctrl,
            shift,
            ctrl_mode,
        ));
        self.plview.lead = Some(target);
        self.pl_scroll_to(target);
    }

    /// Keyboard navigation by `delta` rows from the lead, with Ctrl/Shift
    /// semantics (Shift extends the range, Ctrl adds the new row).
    pub(super) fn pl_navigate(&mut self, delta: i32, ctrl: bool, shift: bool) {
        if self.playlist.is_empty() {
            return;
        }
        let len = self.playlist.len() as i32;
        let from = self
            .plview.lead
            .or(self.plview.anchor)
            .map(|b| b as i32)
            // Nothing focused yet: enter from the near end.
            .unwrap_or(if delta < 0 { len } else { -1 });
        let target = (from + delta).clamp(0, len - 1) as usize;
        self.pl_select_with(target, ctrl, shift, crate::selection::CtrlMode::Add);
    }

    /// Jump the selection to an absolute row (Home / End), with Ctrl/Shift.
    pub(super) fn pl_navigate_to(&mut self, target: usize, ctrl: bool, shift: bool) {
        self.pl_select_with(target, ctrl, shift, crate::selection::CtrlMode::Add);
    }

    /// Grab the scrollbar thumb at cursor y (unscaled px) without moving it: capture the
    /// scroll range (including any blank-row overshoot) and where on the thumb it was
    /// grabbed, so the drag is smooth and a grab never snaps the view.
    pub(super) fn pl_scroll_grab(&mut self, ny: f32) {
        let len = self.playlist.len();
        let visible = classic::pl_visible_rows(self.plview.px_h as u32);
        let range = classic::max_pl_first(len, visible).max(self.plview.first);
        self.plview.scroll_drag_max = range;
        let thumb_y = classic::pl_scroll_thumb_y(self.plview.first, range, self.plview.px_h);
        self.plview.scroll_grab_dy = ny - thumb_y;
        self.plview.scroll_drag = true;
    }

    /// Move the scroll offset while the scrollbar is dragged: keep the grab point under the
    /// cursor (relative drag) over the captured range.
    pub(super) fn pl_scroll_drag(&mut self, ny: f32) {
        let thumb_y = ny - self.plview.scroll_grab_dy;
        self.plview.first =
            classic::pl_scroll_first_at(thumb_y, self.plview.scroll_drag_max, self.plview.px_h);
    }

    /// Adjust the playlist scroll offset so row `idx` is visible.
    pub(super) fn pl_scroll_to(&mut self, idx: usize) {
        let visible = classic::pl_visible_rows(self.plview.px_h as u32).max(1);
        if idx < self.plview.first {
            self.plview.first = idx;
        } else if idx >= self.plview.first + visible {
            self.plview.first = idx + 1 - visible;
        }
    }

    pub(super) fn pl_invert_selection(&mut self) {
        self.plview.selection = (0..self.playlist.len())
            .filter(|i| !self.plview.selection.contains(i))
            .collect();
    }

    /// "Play Next": move the selected track(s) to play right after the current
    /// one (or to the front when nothing is playing), keeping their order. The
    /// moved block becomes the new selection.
    pub(super) fn play_next_selected(&mut self) {
        if self.plview.selection.is_empty() {
            return;
        }
        let sel: Vec<usize> = self.plview.selection.iter().copied().collect();
        let (cur, start, count) =
            reorder_play_next(&mut self.playlist.tracks, self.playlist.current, &sel);
        if count == 0 {
            return;
        }
        self.playlist.current = cur;
        self.plview.selection = (start..start + count).collect();
        self.sort_key = SortKey::Custom;
    }

    /// Remove tracks: when `keep` is true keep only selected (crop), else remove
    /// the selected ones. Preserves the current track by path where possible.
    pub(super) fn pl_remove(&mut self, keep: bool) {
        if self.plview.selection.is_empty() {
            return;
        }
        let cur_path = self.playlist.current_track().map(|t| t.path.clone());
        let sel = self.plview.selection.clone();
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
        self.plview.selection.clear();
        self.plview.anchor = None;
        self.plview.lead = None;
        self.playlist.current =
            cur_path.and_then(|p| self.playlist.tracks.iter().position(|t| t.path == p));
        self.plview.first = self.plview.first.min(self.playlist.len().saturating_sub(1));
        self.status = format!("{} track(s) in playlist.", self.playlist.len());
    }

    pub(super) fn pl_colors(&self) -> classic::PlColors {
        let c = |x: Color32| [x.r(), x.g(), x.b()];
        classic::PlColors {
            bg: c(self.theme.list_bg),
            normal: c(self.theme.list_normal),
            current: c(self.theme.list_current),
            selbg: c(self.theme.list_sel_bg),
        }
    }
}
