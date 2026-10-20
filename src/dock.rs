//! Docking + magnetic snapping geometry for the three classic windows. Pure
//! `egui::Rect` math, independent of the app state.

/// Snap distance in points: a dragged edge within this of another snaps to it.
pub(crate) const SNAP: f32 = 16.0;

/// Push rect `r` out of `keep`, returning its new top-left, or `None` when they
/// don't overlap.
pub(crate) fn push_off(r: egui::Rect, keep: egui::Rect) -> Option<egui::Pos2> {
    let separated = r.right() <= keep.left()
        || r.left() >= keep.right()
        || r.bottom() <= keep.top()
        || r.top() >= keep.bottom();
    if separated {
        return None;
    }
    use egui::vec2;
    // Least-movement escape, but favour below: go above only when the window's
    // centre is above `keep`'s. Left/right compete on least movement.
    let vpush = if r.center().y < keep.center().y {
        keep.top() - r.bottom() // up: the window sits higher than main's centre
    } else {
        keep.bottom() - r.top() // down: favour below
    };
    let hpush = if (keep.left() - r.right()).abs() < (keep.right() - r.left()).abs() {
        keep.left() - r.right() // out to the left
    } else {
        keep.right() - r.left() // out to the right
    };
    let push = if vpush.abs() <= hpush.abs() {
        vec2(0.0, vpush)
    } else {
        vec2(hpush, 0.0)
    };
    Some(r.min + push)
}

/// Magnetic snap for a dragged window `r` to the *nearest* in-range edge across
/// the other visible windows and the screen, smallest offset per axis. Nearest,
/// not window-order: collapsed 14px windows sit within [`SNAP`] (16px) of several
/// edges at once, so order-dependent snapping would leapfrog. Returns the top-left.
pub(crate) fn snap_rect(
    r: egui::Rect,
    others: &[egui::Rect],
    monitor: Option<egui::Vec2>,
) -> egui::Pos2 {
    let s = SNAP;
    let mut best_dx: Option<f32> = None;
    let mut best_dy: Option<f32> = None;
    let consider = |cur: &mut Option<f32>, off: f32| {
        if off.abs() < s && cur.map_or(true, |b: f32| off.abs() < b.abs()) {
            *cur = Some(off);
        }
    };
    if let Some(m) = monitor {
        consider(&mut best_dx, -r.left());
        consider(&mut best_dx, m.x - r.right());
        consider(&mut best_dy, -r.top());
        consider(&mut best_dy, m.y - r.bottom());
    }
    for o in others {
        // Stacked (same column): the windows overlap horizontally -> snap the nearer
        // horizontal edge to edge and align the column's left side.
        let xover = r.left() < o.right() + s && r.right() > o.left() - s;
        if xover {
            consider(&mut best_dy, o.bottom() - r.top());
            consider(&mut best_dy, o.top() - r.bottom());
            consider(&mut best_dx, o.left() - r.left());
        }
        // Side by side (same row): the windows overlap vertically -> snap the nearer
        // vertical edge to edge. Align tops only when they are *not* also stacked: a
        // thin window dropped just above/below another is within the top-alignment
        // range, and aligning its top would drag it onto the other window (what
        // blocked collapsed windows from landing above main).
        let yover = r.top() < o.bottom() + s && r.bottom() > o.top() - s;
        if yover {
            consider(&mut best_dx, o.right() - r.left());
            consider(&mut best_dx, o.left() - r.right());
            if !xover {
                consider(&mut best_dy, o.top() - r.top());
            }
        }
    }
    r.translate(egui::vec2(best_dx.unwrap_or(0.0), best_dy.unwrap_or(0.0)))
        .min
}

/// Two windows are docked when an edge is flush (within tolerance) and they
/// overlap on the shared axis.
pub(crate) fn rects_docked(a: egui::Rect, b: egui::Rect) -> bool {
    let tol = 3.0;
    let x_overlap = a.left() < b.right() - 1.0 && a.right() > b.left() + 1.0;
    let y_overlap = a.top() < b.bottom() - 1.0 && a.bottom() > b.top() + 1.0;
    let v_adj =
        x_overlap && ((a.bottom() - b.top()).abs() <= tol || (a.top() - b.bottom()).abs() <= tol);
    let h_adj =
        y_overlap && ((a.right() - b.left()).abs() <= tol || (a.left() - b.right()).abs() <= tol);
    v_adj || h_adj
}

/// Which of the three windows are transitively docked to the main window (0).
pub(crate) fn dock_group_from_main(rects: &[egui::Rect; 3], visible: &[bool; 3]) -> [bool; 3] {
    let mut in_group = [true, false, false];
    let mut changed = true;
    while changed {
        changed = false;
        for j in [1usize, 2usize] {
            if in_group[j] || !visible[j] {
                continue;
            }
            if (0..3).any(|k| in_group[k] && rects_docked(rects[j], rects[k])) {
                in_group[j] = true;
                changed = true;
            }
        }
    }
    in_group
}

/// When window `grown` changes height, the other windows that should move with
/// it: the docked sub-group hanging below it. Seeds with the windows docked
/// directly below `grown`'s bottom edge, then transitively pulls in any window
/// docked to one already moving — so a window docked to the *side* of a pushed
/// window rides along and stays docked. The grown window itself, and anything
/// reachable only through it (above or beside it), stay put.
pub(crate) fn docked_group_below(rects: &[Option<egui::Rect>; 3], grown: usize) -> [bool; 3] {
    let mut moved = [false; 3];
    let Some(g) = rects.get(grown).copied().flatten() else {
        return moved;
    };
    let tol = 2.0;
    // Seed: windows flush directly below the grown window's bottom edge.
    for (j, rj) in rects.iter().enumerate() {
        if j == grown {
            continue;
        }
        if let Some(r) = rj {
            let x_overlap = r.left() < g.right() && r.right() > g.left();
            if x_overlap && (r.top() - g.bottom()).abs() <= tol {
                moved[j] = true;
            }
        }
    }
    // Transitive: a window docked to any moving window joins the group.
    let mut changed = true;
    while changed {
        changed = false;
        for j in 0..3 {
            if j == grown || moved[j] {
                continue;
            }
            let Some(rj) = rects[j] else { continue };
            for k in 0..3 {
                if k == grown || !moved[k] {
                    continue;
                }
                if let Some(rk) = rects[k] {
                    if rects_docked(rj, rk) {
                        moved[j] = true;
                        changed = true;
                        break;
                    }
                }
            }
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Rect};

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), vec2(w, h))
    }

    // Collapsed windows are 14px tall; these mirror the shade-expand reflow.
    #[test]
    fn expanding_main_pushes_the_stack_below() {
        // Main > EQ > PL, all collapsed and flush.
        let rects = [Some(r(0.0, 0.0, 275.0, 14.0)), Some(r(0.0, 14.0, 275.0, 14.0)), Some(r(0.0, 28.0, 275.0, 14.0))];
        assert_eq!(docked_group_below(&rects, 0), [false, true, true]);
    }

    #[test]
    fn expanding_eq_pushes_only_below_it() {
        // Main > EQ > PL: growing EQ moves PL (below), not Main (above).
        let rects = [Some(r(0.0, 0.0, 275.0, 14.0)), Some(r(0.0, 14.0, 275.0, 14.0)), Some(r(0.0, 28.0, 275.0, 14.0))];
        assert_eq!(docked_group_below(&rects, 1), [false, false, true]);
    }

    #[test]
    fn expanding_top_window_pushes_main_and_pl() {
        // EQ on top of Main, then PL: EQ(1) y0, Main(0) y14, PL(2) y28.
        let rects = [Some(r(0.0, 14.0, 275.0, 14.0)), Some(r(0.0, 0.0, 275.0, 14.0)), Some(r(0.0, 28.0, 275.0, 14.0))];
        assert_eq!(docked_group_below(&rects, 1), [true, false, true]);
    }

    #[test]
    fn a_gap_ends_the_chain() {
        // EQ flush below main, but PL has a gap below EQ -> PL not pushed.
        let rects = [Some(r(0.0, 0.0, 275.0, 14.0)), Some(r(0.0, 14.0, 275.0, 14.0)), Some(r(0.0, 60.0, 275.0, 14.0))];
        assert_eq!(docked_group_below(&rects, 0), [false, true, false]);
    }

    #[test]
    fn a_floating_sidebyside_window_is_not_pushed() {
        // EQ sits beside main (no x overlap with main's column) -> not in the chain.
        let rects = [Some(r(0.0, 0.0, 275.0, 14.0)), Some(r(400.0, 14.0, 275.0, 14.0)), None];
        assert_eq!(docked_group_below(&rects, 0), [false, false, false]);
    }

    #[test]
    fn side_docked_window_rides_along_with_the_pushed_one() {
        // Main; EQ docked below main; PL docked to EQ's right (same row as EQ).
        // Expanding main must push EQ *and* PL (docked to EQ), not just EQ.
        let rects = [
            Some(r(0.0, 0.0, 275.0, 14.0)),   // main
            Some(r(0.0, 14.0, 275.0, 14.0)),  // eq, below main
            Some(r(275.0, 14.0, 275.0, 14.0)),// pl, right of eq
        ];
        assert_eq!(docked_group_below(&rects, 0), [false, true, true]);
        // PL on the left of EQ behaves the same.
        let rects_l = [
            Some(r(0.0, 0.0, 275.0, 14.0)),
            Some(r(0.0, 14.0, 275.0, 14.0)),
            Some(r(-275.0, 14.0, 275.0, 14.0)),
        ];
        assert_eq!(docked_group_below(&rects_l, 0), [false, true, true]);
    }

    #[test]
    fn stacked_windows_are_docked() {
        assert!(rects_docked(r(0.0, 0.0, 275.0, 116.0), r(0.0, 116.0, 275.0, 116.0)));
    }

    #[test]
    fn push_off_main_keeps_it_uncovered() {
        let main = r(100.0, 100.0, 200.0, 100.0); // x 100..300, y 100..200
        // Flush below -> not overlapping, no push.
        assert_eq!(push_off(r(100.0, 200.0, 200.0, 100.0), main), None);
        // Overlapping mainly on main's left edge -> pushed out to the left.
        let w = r(60.0, 120.0, 60.0, 40.0);
        let p = push_off(w, main).unwrap();
        assert!((p.x + 60.0 - main.left()).abs() < 0.01, "pushed flush to the left edge");
        // Fully inside -> pushed to the nearest edge; result no longer overlaps.
        let inside = r(150.0, 130.0, 50.0, 40.0);
        let p = push_off(inside, main).unwrap();
        let moved = Rect::from_min_size(p, vec2(50.0, 40.0));
        assert!(push_off(moved, main).is_none(), "result still overlaps main");
        // Dropped mostly ABOVE main (only a sliver overlaps its top edge) -> it is
        // allowed above: pushed up so its bottom meets main's top.
        let above = r(150.0, 70.0, 50.0, 40.0); // y 70..110, overlaps main's top 100..110
        let p = push_off(above, main).unwrap();
        assert!((p.y + 40.0 - main.top()).abs() < 0.01, "clearly-above window snaps above");
        // Centred vertically -> favours below over above despite similar distance.
        let centred = r(150.0, 150.0, 50.0, 100.0); // y150..250 around main 100..200
        let p = push_off(centred, main).unwrap();
        assert!(p.y >= main.top(), "centred window favours below, not above");
    }

    #[test]
    fn gap_is_not_docked() {
        assert!(!rects_docked(r(0.0, 0.0, 275.0, 116.0), r(0.0, 140.0, 275.0, 116.0)));
    }

    // Collapsed classic windows are 14px tall; SNAP is 16px. These guard the
    // window-shade docking that was broken: a thin window dropped above another
    // must land above it, not snap onto / past it onto a farther edge.
    const COLL: f32 = 14.0;

    #[test]
    fn collapsed_eq_snaps_flush_above_main() {
        // Main collapsed at y 473..487; EQ dropped just above it (bottom at 478,
        // 5px into main). It must snap flush ABOVE main, not onto it.
        let main = r(800.0, 473.0, 275.0, COLL);
        let eq = r(794.0, 464.0, 275.0, COLL); // 464..478
        let p = snap_rect(eq, &[main], None);
        assert!((p.y + COLL - main.top()).abs() < 0.01, "EQ bottom should meet main top (flush above)");
        assert!((p.x - main.left()).abs() < 0.01, "EQ should align to main's column");
        // And the overlap resolver then leaves it alone (already separated).
        let snapped = r(p.x, p.y, 275.0, COLL);
        assert!(push_off(snapped, main).is_none(), "flush-above EQ must not be pushed");
    }

    #[test]
    fn collapsed_playlist_lands_above_stacked_eq_not_on_main() {
        // Regression: PL dropped above an EQ that is itself stacked above main must
        // snap to EQ's top (0px away), not leapfrog past it onto main's top (14px
        // away) and get shoved below main.
        let main = r(800.0, 473.0, 275.0, COLL); // 473..487
        let eq = r(800.0, 459.0, 275.0, COLL); //   459..473 (above main)
        let pl_drop = r(794.0, 445.0, 275.0, COLL); // 445..459 (above eq)
        let p = snap_rect(pl_drop, &[main, eq], None);
        assert!((p.y + COLL - eq.top()).abs() < 0.01, "PL bottom should meet EQ top, staying above EQ");
        assert!(p.y < main.top(), "PL must not snap down onto/under main");
        // End-to-end: the child overlap resolver keeps it above both.
        let pl = r(p.x, p.y, 275.0, COLL);
        assert!(push_off(pl, main).is_none(), "PL must not overlap main");
        assert!(rects_docked(eq, main), "EQ is docked to main (part of the stack)");
        assert!(push_off(pl, eq).is_none(), "PL must not overlap the EQ it sits above");
    }

    #[test]
    fn snap_picks_the_nearest_edge() {
        // A window dropped between two stacked windows, within SNAP of both facing
        // edges, snaps to the nearer one (not whichever is checked first). Here the
        // drop sits 2px below the upper window's bottom and 12px above the lower
        // window's top, so it must snap up to the upper window's bottom.
        let upper = r(0.0, 0.0, 275.0, COLL); //   0..14
        let lower = r(0.0, 42.0, 275.0, COLL); // 42..56
        let drop = r(0.0, 16.0, 275.0, COLL); //  16..30: top 2px from upper.bottom, bottom 12px from lower.top
        // Pass the farther window first to prove slice order doesn't decide the snap.
        let p = snap_rect(drop, &[lower, upper], None);
        assert!((p.y - upper.bottom()).abs() < 0.01, "should snap flush below the nearer (upper) window");
    }

    #[test]
    fn eq_docks_below_main_normally() {
        let main = r(800.0, 100.0, 275.0, 116.0); // 100..216
        let eq_drop = r(805.0, 210.0, 275.0, 116.0); // top 210, 6px from main bottom
        let p = snap_rect(eq_drop, &[main], None);
        assert!((p.y - main.bottom()).abs() < 0.01, "EQ should dock flush below main");
        assert!((p.x - main.left()).abs() < 0.01, "EQ should align to main's column");
    }

    #[test]
    fn window_snaps_flush_to_the_side() {
        let main = r(800.0, 100.0, 275.0, 116.0); // x 800..1075
        let side = r(1070.0, 105.0, 275.0, 116.0); // left 1070, 5px from main's right
        let p = snap_rect(side, &[main], None);
        assert!((p.x - main.right()).abs() < 0.01, "side window should snap flush to main's right edge");
    }

    #[test]
    fn snaps_to_screen_edges() {
        let mon = Some(egui::vec2(1920.0, 1200.0));
        let near_top_left = r(5.0, 9.0, 275.0, 116.0);
        let p = snap_rect(near_top_left, &[], mon);
        assert!(p.x.abs() < 0.01 && p.y.abs() < 0.01, "should snap into the top-left screen corner");
    }

    #[test]
    fn full_stack_all_follow_main() {
        let rects = [
            r(0.0, 0.0, 275.0, 116.0),
            r(0.0, 116.0, 275.0, 116.0),
            r(0.0, 232.0, 275.0, 180.0),
        ];
        assert_eq!(dock_group_from_main(&rects, &[true, true, true]), [true, true, true]);
    }

    #[test]
    fn eq_pl_docked_only_to_each_other_dont_follow_main() {
        // main alone; eq+pl docked to each other but far from main.
        let rects = [
            r(0.0, 0.0, 275.0, 116.0),
            r(600.0, 0.0, 275.0, 116.0),
            r(600.0, 116.0, 275.0, 180.0),
        ];
        assert_eq!(dock_group_from_main(&rects, &[true, true, true]), [true, false, false]);
    }

    #[test]
    fn detached_eq_breaks_the_chain_to_pl() {
        // eq floated away; pl is flush under eq but neither connects to main.
        let rects = [
            r(0.0, 0.0, 275.0, 116.0),
            r(0.0, 300.0, 275.0, 116.0),
            r(0.0, 416.0, 275.0, 180.0),
        ];
        assert_eq!(dock_group_from_main(&rects, &[true, true, true]), [true, false, false]);
    }

    #[test]
    fn hidden_windows_never_follow() {
        let rects = [
            r(0.0, 0.0, 275.0, 116.0),
            r(0.0, 116.0, 275.0, 116.0),
            r(0.0, 232.0, 275.0, 180.0),
        ];
        // eq hidden -> it can't be in the group, and can't bridge to pl.
        assert_eq!(dock_group_from_main(&rects, &[true, false, true]), [true, false, false]);
    }
}
