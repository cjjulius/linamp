//! The application logo, and the optional "Icon Dark Mode" recolour.
//!
//! LinAmp's logo is black and green. The Icon Dark Mode preference recolours it to
//! black and grey for people who would rather it not stand out, without touching the
//! installed files: the recolour happens at runtime from the one embedded source, so
//! there is a single logo to maintain and both looks stay in sync. It applies to the
//! window and taskbar icon and, while running, the tray icon. See [`desaturate`].

use image::RgbaImage;

/// The embedded logo at the two sizes the app needs: the large one for the window
/// and taskbar, the small one for the tray. Both are the same green-on-black art.
const LOGO_256: &[u8] = include_bytes!("../assets/linamp_logo_256.png");
const LOGO_32: &[u8] = include_bytes!("../assets/linamp_logo_32.png");
/// The scalable logo, the source for the per-user `scalable/apps/linamp.svg` the
/// desktop icon theme serves. On GNOME the dock and the app launcher render this
/// (or the PNG) by icon name, never the window's `_NET_WM_ICON`, so Icon Dark Mode
/// has to recolour this file too. See [`logo_svg`].
///
/// Only the Linux file-association path reads this; on other platforms the icon is
/// set live from the raster assets, so the SVG source and its recolour are unused.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const LOGO_SVG: &str = include_str!("../assets/linamp_logo.svg");

/// Recolour the logo in place for Icon Dark Mode: desaturate every pixel to its
/// luminance grey, leaving alpha untouched. The black backing circle stays black
/// and the green bolt and ring become grey, so the mark reads the same shape in
/// black and grey instead of black and green. A no-op shape-wise on already-grey
/// pixels, so applying it to the normal logo is the only transform needed.
pub fn desaturate(img: &mut RgbaImage) {
    for px in img.pixels_mut() {
        let [r, g, b, a] = px.0;
        // Rec. 709 luma: matches how the eye weights the channels, so the grey keeps
        // the logo's shading (the bright bolt stays light, the darker ring mid-grey).
        let y = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32).round();
        let y = y.clamp(0.0, 255.0) as u8;
        px.0 = [y, y, y, a];
    }
}

/// Decode an embedded PNG to RGBA, applying the dark-mode recolour when `dark`.
/// Returns `None` only if the embedded bytes fail to decode, which cannot happen
/// for the bundled assets but keeps callers from unwrapping.
fn load(bytes: &[u8], dark: bool) -> Option<RgbaImage> {
    let mut img = image::load_from_memory(bytes).ok()?.to_rgba8();
    if dark {
        desaturate(&mut img);
    }
    Some(img)
}

/// The window / taskbar icon as raw RGBA pixels (256px), recoloured for dark mode
/// when `dark`. Returns `(width, height, rgba)`. This is the large icon Windows uses
/// for the taskbar button and Alt-Tab, and the one X11 stores in `_NET_WM_ICON`.
pub fn window_rgba(dark: bool) -> Option<(u32, u32, Vec<u8>)> {
    let img = load(LOGO_256, dark)?;
    let (w, h) = img.dimensions();
    Some((w, h, img.into_raw()))
}

/// The window / taskbar icon as egui `IconData` (256px), for the startup
/// `ViewportBuilder::with_icon`, recoloured for dark mode when `dark`.
pub fn window_icon(dark: bool) -> Option<egui::IconData> {
    let (width, height, rgba) = window_rgba(dark)?;
    Some(egui::IconData {
        rgba,
        width,
        height,
    })
}

/// The tray icon as raw RGBA pixels (32px), recoloured for dark mode when `dark`.
/// Returns `(width, height, rgba)`; the platform tray code packs these into whatever
/// format its API wants (SNI's ARGB pixmap on Linux, an `HICON` on Windows).
pub fn tray_rgba(dark: bool) -> Option<(u32, u32, Vec<u8>)> {
    let img = load(LOGO_32, dark)?;
    let (w, h) = img.dimensions();
    Some((w, h, img.into_raw()))
}

/// The window / launcher icon encoded as PNG bytes (256px) for the per-user
/// `256x256/apps/linamp.png` the desktop icon theme serves, recoloured for dark
/// mode when `dark`. In light mode this is the embedded asset verbatim, so the
/// file-association installer skips rewriting an unchanged file; in dark mode it is
/// the desaturated logo re-encoded, which is deterministic, so the installer also
/// settles to a no-op after the first write. `None` only on a decode/encode
/// failure, which cannot happen for the bundled asset.
///
/// Only the Linux file-association installer calls this; elsewhere the taskbar
/// icon is set live from raw RGBA, not an installed PNG file.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn window_png(dark: bool) -> Option<Vec<u8>> {
    if !dark {
        return Some(LOGO_256.to_vec());
    }
    let img = load(LOGO_256, true)?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(out.into_inner())
}

/// The scalable logo SVG, recoloured for dark mode when `dark`. Every `#rgb` or
/// `#rrggbb` colour literal is mapped to its Rec. 709 luminance grey, matching the
/// raster [`desaturate`]; already-grey literals map to themselves, so light mode
/// returns the asset unchanged. `url(#id)` references are left alone. This feeds the
/// per-user `scalable/apps/linamp.svg` the desktop theme serves.
///
/// Only the Linux file-association installer calls this.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn logo_svg(dark: bool) -> String {
    if !dark {
        return LOGO_SVG.to_string();
    }
    desaturate_hex_colors(LOGO_SVG)
}

/// Replace every `#rgb`/`#rrggbb` colour literal in `s` with its luminance grey,
/// leaving all other text intact, so an SVG recolours in step with the raster
/// [`desaturate`] without an XML parse. A `#` run that is not exactly 3 or 6 hex
/// digits (example: a `url(#glow)` id) is left as is, as is any `#` that directly
/// follows `(`, which guards `url(#abc)` ids that happen to be all hex.
///
/// Reached only through [`logo_svg`], i.e. the Linux file-association path.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn desaturate_hex_colors(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut copied = 0; // first byte not yet flushed to `out`
    while i < b.len() {
        let is_url_ref = i > 0 && b[i - 1] == b'(';
        if b[i] == b'#' && !is_url_ref {
            let start = i + 1;
            let mut j = start;
            while j < b.len() && (b[j] as char).is_ascii_hexdigit() {
                j += 1;
            }
            let hex = &s[start..j];
            if hex.len() == 6 || hex.len() == 3 {
                let (r, g, b_) = if hex.len() == 6 {
                    (
                        u8::from_str_radix(&hex[0..2], 16).unwrap_or(0),
                        u8::from_str_radix(&hex[2..4], 16).unwrap_or(0),
                        u8::from_str_radix(&hex[4..6], 16).unwrap_or(0),
                    )
                } else {
                    // #rgb shorthand: each nibble is doubled (#1a2 == #11aa22).
                    let nib = |k: usize| {
                        let v = u8::from_str_radix(&hex[k..k + 1], 16).unwrap_or(0);
                        (v << 4) | v
                    };
                    (nib(0), nib(1), nib(2))
                };
                let y = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b_ as f32)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                out.push_str(&s[copied..i]);
                out.push_str(&format!("#{y:02x}{y:02x}{y:02x}"));
                i = j;
                copied = j;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desaturate_makes_channels_equal_and_keeps_alpha() {
        let mut img = RgbaImage::from_pixel(2, 1, image::Rgba([0x39, 0xff, 0x14, 200]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 255]));
        desaturate(&mut img);
        // Green pixel: R, G and B now match (grey), alpha preserved.
        let g = img.get_pixel(0, 0).0;
        assert_eq!(g[0], g[1]);
        assert_eq!(g[1], g[2]);
        assert_eq!(g[3], 200);
        // The bright green maps to a light grey, not black and not full white.
        assert!(g[0] > 150 && g[0] < 255, "bright green -> light grey, got {}", g[0]);
        // Black stays black, alpha preserved.
        assert_eq!(img.get_pixel(1, 0).0, [0, 0, 0, 255]);
    }

    #[test]
    fn embedded_icons_decode_in_both_modes() {
        for dark in [false, true] {
            assert!(window_icon(dark).is_some(), "window icon decodes (dark={dark})");
            let (w, h, rgba) = tray_rgba(dark).expect("tray icon decodes");
            assert_eq!(rgba.len(), (w * h * 4) as usize);
        }
    }

    #[test]
    fn dark_mode_actually_changes_the_window_icon() {
        let normal = window_icon(false).unwrap().rgba;
        let dark = window_icon(true).unwrap().rgba;
        assert_ne!(normal, dark, "dark mode must change the pixels");
    }

    #[test]
    fn window_png_is_asset_in_light_and_decodable_grey_in_dark() {
        // Light mode is the embedded asset verbatim (so the installer can no-op).
        assert_eq!(window_png(false).unwrap(), LOGO_256);
        // Dark mode is a different, still-valid PNG whose pixels are grey.
        let dark = window_png(true).unwrap();
        assert_ne!(dark, LOGO_256);
        let img = image::load_from_memory(&dark).expect("dark png decodes").to_rgba8();
        for px in img.pixels() {
            let [r, g, b, _] = px.0;
            assert!(r == g && g == b, "dark png pixel not grey: {:?}", px.0);
        }
    }

    #[test]
    fn logo_svg_light_is_source_dark_is_greyed() {
        assert_eq!(logo_svg(false), LOGO_SVG);
        let dark = logo_svg(true);
        // The green bolt/ring colour maps to its luminance grey, and no original
        // green literal survives.
        assert!(dark.contains("#c4c4c4"), "bolt green should map to #c4c4c4");
        assert!(!dark.contains("#39ff14"), "no source green should remain");
        assert!(!dark.contains("#b6ff5c"));
        // Gradient/filter id references are untouched.
        assert!(dark.contains("url(#bolt)"));
        assert!(dark.contains("url(#glow)"));
        // Already-neutral backing colours are preserved.
        assert!(dark.contains("#000000"));
    }

    #[test]
    fn desaturate_hex_handles_shorthand_and_leaves_url_ids() {
        // #rgb shorthand doubles each nibble: #0f0 == (0,255,0) -> luma 182 -> #b6b6b6.
        assert_eq!(desaturate_hex_colors("fill=\"#0f0\""), "fill=\"#b6b6b6\"");
        // An all-hex url id (len 3) must not be rewritten.
        assert_eq!(desaturate_hex_colors("url(#abc)"), "url(#abc)");
    }
}
