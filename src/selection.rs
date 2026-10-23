//! Playlist multi-selection logic for the Winamp-style ctrl/shift gestures,
//! kept pure (operating on a plain set + anchor) so it can be unit-tested
//! without constructing the whole app.

use std::collections::HashSet;

/// What a Ctrl-modified gesture does to the target row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CtrlMode {
    /// Flip the target in/out of the selection (mouse ctrl-click).
    Toggle,
    /// Add the target to the selection, never removing (keyboard ctrl-nav, so
    /// arrowing over a row can't silently drop it).
    Add,
}

/// Apply a selection gesture that lands on `target`, mutating `selection`:
///
/// - `shift`: select the contiguous range between the anchor and `target`.
/// - `ctrl` (no shift): toggle or add `target` per `ctrl_mode`.
/// - neither: select only `target`.
///
/// Returns the new anchor. The caller also records `target` as the lead row.
pub fn apply(
    selection: &mut HashSet<usize>,
    anchor: Option<usize>,
    target: usize,
    ctrl: bool,
    shift: bool,
    ctrl_mode: CtrlMode,
) -> usize {
    if shift {
        let a = anchor.unwrap_or(target);
        let (lo, hi) = (a.min(target), a.max(target));
        *selection = (lo..=hi).collect();
        a // the anchor is fixed across a shift-range
    } else if ctrl {
        match ctrl_mode {
            CtrlMode::Toggle => {
                if !selection.remove(&target) {
                    selection.insert(target);
                }
            }
            CtrlMode::Add => {
                selection.insert(target);
            }
        }
        target
    } else {
        selection.clear();
        selection.insert(target);
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[usize]) -> HashSet<usize> {
        items.iter().copied().collect()
    }

    #[test]
    fn plain_click_selects_only_the_target() {
        let mut s = set(&[1, 2, 3]);
        let anchor = apply(&mut s, Some(5), 7, false, false, CtrlMode::Toggle);
        assert_eq!(s, set(&[7]));
        assert_eq!(anchor, 7);
    }

    #[test]
    fn ctrl_toggle_adds_then_removes() {
        let mut s = set(&[2]);
        let a = apply(&mut s, Some(2), 5, true, false, CtrlMode::Toggle);
        assert_eq!(s, set(&[2, 5]));
        assert_eq!(a, 5);
        // Toggling the same row again removes it.
        apply(&mut s, Some(5), 5, true, false, CtrlMode::Toggle);
        assert_eq!(s, set(&[2]));
    }

    #[test]
    fn ctrl_add_never_removes() {
        let mut s = set(&[5]);
        apply(&mut s, Some(5), 5, true, false, CtrlMode::Add);
        assert_eq!(s, set(&[5]), "Add re-selecting a row keeps it selected");
        apply(&mut s, Some(5), 6, true, false, CtrlMode::Add);
        assert_eq!(s, set(&[5, 6]));
    }

    #[test]
    fn shift_selects_inclusive_range_from_anchor() {
        let mut s = set(&[0]);
        let a = apply(&mut s, Some(2), 5, false, true, CtrlMode::Toggle);
        assert_eq!(s, set(&[2, 3, 4, 5]));
        assert_eq!(a, 2, "anchor is unchanged by a shift-range");
    }

    #[test]
    fn shift_range_works_upward_too() {
        let mut s = HashSet::new();
        apply(&mut s, Some(8), 5, false, true, CtrlMode::Toggle);
        assert_eq!(s, set(&[5, 6, 7, 8]));
    }

    #[test]
    fn shift_without_anchor_falls_back_to_target() {
        let mut s = HashSet::new();
        let a = apply(&mut s, None, 4, false, true, CtrlMode::Toggle);
        assert_eq!(s, set(&[4]));
        assert_eq!(a, 4);
    }
}
