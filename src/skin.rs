//! Winamp classic skin (.wsz) theming.
//!
//! A `.wsz` file is a ZIP archive of bitmaps plus a couple of plain-text colour
//! config files. We don't render the classic sprite layout (that's a separate
//! phase); instead we derive a cohesive colour [`Theme`] from each skin's own
//! `PLEDIT.TXT` (playlist colours), `VISCOLOR.TXT` (visualizer palette) and
//! `MAIN.BMP` (sampled for window/panel tone), and apply it to LinAmp's UI.

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow::Result;
use egui::Color32;
use zip::ZipArchive;

/// A full colour theme used across the LinAmp UI.
#[derive(Clone)]
pub struct Theme {
    pub name: String,
    pub bg: Color32,
    pub panel: Color32,
    pub accent: Color32,
    pub lcd: Color32,
    pub lcd_dim: Color32,
    pub list_bg: Color32,
    pub list_normal: Color32,
    pub list_current: Color32,
    pub list_sel_bg: Color32,
    pub viz_bg: Color32,
    pub viz_grid: Color32,
    /// Spectrum gradient, index 0 = top (loudest) .. last = bottom.
    pub viz_spectrum: Vec<Color32>,
    pub viz_osc: Color32,
    pub viz_peak: Color32,
}

impl Default for Theme {
    fn default() -> Self {
        let green = Color32::from_rgb(0, 255, 120);
        Theme {
            name: "LinAmp Dark (built-in)".to_string(),
            bg: Color32::from_rgb(20, 22, 28),
            panel: Color32::from_rgb(34, 38, 46),
            accent: Color32::from_rgb(120, 170, 255),
            lcd: green,
            lcd_dim: Color32::from_rgb(0, 140, 70),
            list_bg: Color32::BLACK,
            list_normal: Color32::from_rgb(200, 200, 200),
            list_current: green,
            list_sel_bg: Color32::from_rgb(40, 70, 120),
            viz_bg: Color32::BLACK,
            viz_grid: Color32::from_rgb(24, 33, 41),
            viz_spectrum: default_spectrum(),
            viz_osc: green,
            viz_peak: Color32::from_rgb(150, 150, 150),
        }
    }
}

#[derive(Default)]
struct PlEdit {
    normal: Option<Color32>,
    current: Option<Color32>,
    normal_bg: Option<Color32>,
    selected_bg: Option<Color32>,
}

/// Load a `.wsz`/`.zip` skin and derive a [`Theme`] from it.
pub fn load_wsz(path: &Path) -> Result<Theme> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;

    // Snapshot entry names so we can look them up case-insensitively.
    let names: Vec<(usize, String)> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| (i, f.name().to_string())))
        .collect();

    let pledit = read_named(&mut archive, &names, "pledit.txt")
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let viscolor = read_named(&mut archive, &names, "viscolor.txt")
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let main_bmp = read_named(&mut archive, &names, "main.bmp");

    let pl = parse_pledit(&pledit);
    let vis = parse_viscolor(&viscolor);

    let mut theme = Theme::default();
    theme.name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Skin".to_string());

    if let Some(c) = pl.normal_bg {
        theme.list_bg = c;
    }
    if let Some(c) = pl.normal {
        theme.list_normal = c;
        theme.lcd = c;
    }
    if let Some(c) = pl.current {
        theme.list_current = c;
    }
    if let Some(c) = pl.selected_bg {
        theme.list_sel_bg = c;
        theme.accent = c;
    }
    theme.lcd_dim = blend(theme.lcd, theme.list_bg, 0.45);

    if vis.len() >= 18 {
        theme.viz_bg = vis[0];
        theme.viz_grid = vis[1];
        theme.viz_spectrum = vis[2..18].to_vec();
    }
    if vis.len() >= 19 {
        theme.viz_osc = vis[18];
    }
    if vis.len() >= 24 {
        theme.viz_peak = vis[23];
    }

    if let Some(bytes) = main_bmp {
        if let Some((panel, bg)) = sample_bmp(&bytes) {
            theme.panel = panel;
            theme.bg = bg;
        }
    }

    Ok(theme)
}

fn read_named<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    names: &[(usize, String)],
    want: &str,
) -> Option<Vec<u8>> {
    let want = want.to_ascii_lowercase();
    let idx = names
        .iter()
        .find(|(_, n)| {
            let base = n
                .rsplit(|c| c == '/' || c == '\\')
                .next()
                .unwrap_or(n)
                .to_ascii_lowercase();
            base == want
        })
        .map(|(i, _)| *i)?;
    let mut f = archive.by_index(idx).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn parse_pledit(text: &str) -> PlEdit {
    let mut m = PlEdit::default();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let col = parse_hex(v);
            match k.trim().to_ascii_lowercase().as_str() {
                "normal" => m.normal = col,
                "current" => m.current = col,
                "normalbg" => m.normal_bg = col,
                "selectedbg" => m.selected_bg = col,
                _ => {}
            }
        }
    }
    m
}

/// Parse `VISCOLOR.TXT`: one `r,g,b // comment` triple per line.
fn parse_viscolor(text: &str) -> Vec<Color32> {
    let mut out = Vec::new();
    for line in text.lines() {
        let code = line.split("//").next().unwrap_or("");
        let parts: Vec<&str> = code
            .split(',')
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .collect();
        if parts.len() >= 3 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                parts[0].parse::<u8>(),
                parts[1].parse::<u8>(),
                parts[2].parse::<u8>(),
            ) {
                out.push(Color32::from_rgb(r, g, b));
            }
        }
    }
    out
}

fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() < 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color32::from_rgb(r, g, b))
}

/// Return (average colour, darkened colour) of a bitmap.
fn sample_bmp(bytes: &[u8]) -> Option<(Color32, Color32)> {
    let img = image::load_from_memory(bytes).ok()?.to_rgb8();
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for p in img.pixels() {
        r += p[0] as u64;
        g += p[1] as u64;
        b += p[2] as u64;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let avg = Color32::from_rgb((r / n) as u8, (g / n) as u8, (b / n) as u8);
    let dark = Color32::from_rgb(
        (r / n * 6 / 10) as u8,
        (g / n * 6 / 10) as u8,
        (b / n * 6 / 10) as u8,
    );
    Some((avg, dark))
}

/// Linear blend: `t` of the way from `a` to `b`.
pub fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
    )
}

/// Pick a readable text colour (black/white) for a given background.
pub fn contrast_text(bg: Color32) -> Color32 {
    let lum = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if lum > 140.0 {
        Color32::from_rgb(15, 15, 15)
    } else {
        Color32::from_rgb(225, 225, 225)
    }
}

fn default_spectrum() -> Vec<Color32> {
    (0..16)
        .map(|i| {
            let t = i as f32 / 15.0;
            Color32::from_rgb(0, (255.0 - t * 120.0) as u8, (120.0 - t * 60.0) as u8)
        })
        .collect()
}

/// Print a one-line-per-field summary of a skin's derived theme (CLI verifier).
pub fn describe(path: &Path) {
    match load_wsz(path) {
        Ok(t) => {
            let c = |x: Color32| format!("#{:02X}{:02X}{:02X}", x.r(), x.g(), x.b());
            println!("OK  {:<24} name={:<22} bg={} panel={} accent={} lcd={} list_bg={} list_cur={} spectrum={}cols osc={}",
                path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                t.name, c(t.bg), c(t.panel), c(t.accent), c(t.lcd),
                c(t.list_bg), c(t.list_current), t.viz_spectrum.len(), c(t.viz_osc));
        }
        Err(e) => println!("ERR {:<24} {}", path.display(), e),
    }
}

#[cfg(test)]
mod tests {
    use super::{blend, contrast_text, parse_hex, parse_viscolor};
    use egui::Color32;

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#00FF00"), Some(Color32::from_rgb(0, 255, 0)));
        assert_eq!(parse_hex("0000C6"), Some(Color32::from_rgb(0, 0, 0xC6)));
        assert_eq!(parse_hex("#fff"), None);
    }

    #[test]
    fn viscolor_parsing() {
        let v = parse_viscolor("0,0,0 // bg\n255, 255, 255, // white\nbad line\n1,2,3");
        assert_eq!(v.len(), 3);
        assert_eq!(v[1], Color32::from_rgb(255, 255, 255));
    }

    #[test]
    fn blend_midpoint() {
        let g = blend(Color32::BLACK, Color32::WHITE, 0.5);
        assert!((g.r() as i32 - 127).abs() <= 1);
    }

    #[test]
    fn contrast_picks_readable() {
        assert_eq!(contrast_text(Color32::BLACK), Color32::from_rgb(225, 225, 225));
        assert_eq!(contrast_text(Color32::WHITE), Color32::from_rgb(15, 15, 15));
    }
}
