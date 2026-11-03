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

/// The window / taskbar icon (256px), recoloured for dark mode when `dark`.
pub fn window_icon(dark: bool) -> Option<egui::IconData> {
    let img = load(LOGO_256, dark)?;
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
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
}
