//! Maps a skin's derived colours onto egui's `Visuals`/`Style`.

use egui::Color32;

use crate::skin::{self, Theme};

pub(super) fn apply_theme(ctx: &egui::Context, t: &Theme) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = t.bg;
    visuals.window_fill = t.bg;
    visuals.extreme_bg_color = t.list_bg;
    visuals.faint_bg_color = t.panel;
    visuals.widgets.inactive.bg_fill = t.panel;
    visuals.widgets.hovered.bg_fill = skin::blend(t.panel, Color32::WHITE, 0.12);
    visuals.widgets.active.bg_fill = t.accent;
    visuals.selection.bg_fill = t.list_sel_bg;
    // Outline colour, derived from the skin so bevels/outlines track the theme
    // (used by the tag editor's tabs, buttons and field frames).
    let outline = egui::Stroke::new(1.0_f32, skin::blend(t.bg, skin::contrast_text(t.bg), 0.35));
    visuals.widgets.noninteractive.bg_stroke = outline;
    // Menu/popup outline, so the Media Library menu and the classic popups share
    // one theme-derived border colour.
    visuals.window_stroke = outline;
    // Drop the drop shadow from egui popups/menus (Media Library, dropdowns) so
    // they match the classic context menus, which, being separate borderless OS
    // windows, can't render one (see issue #108).
    visuals.popup_shadow = egui::epaint::Shadow::NONE;
    visuals.override_text_color = Some(skin::contrast_text(t.bg));
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.slider_width = 120.0;
    ctx.set_style(style);
}
