//! LCD text helpers: track-time formatting and Winamp-style marquee scrolling.

use std::time::Duration;

/// `mm:ss` for a track position / length.
pub(crate) fn fmt_time(d: Duration) -> String {
    let total = d.as_secs();
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// Horizontally scrolled view of `text` for the modern-view LCD marquee.
pub(crate) fn marquee(text: &str, offset: f32) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 40 {
        return text.to_string();
    }
    let padded: Vec<char> = text.chars().chain("      ".chars()).collect();
    let start = (offset as usize) % padded.len();
    padded.iter().cycle().skip(start).take(40).collect()
}

/// Scroll `text` within a fixed character `width` (classic-view marquee). Short
/// strings pass through; longer ones wrap with a `"   *** "` separator.
pub(crate) fn scroll_text(text: &str, offset: f32, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let padded: Vec<char> = text.chars().chain("   *** ".chars()).collect();
    let start = (offset as usize) % padded.len();
    padded.iter().cycle().skip(start).take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_text_passthrough_when_fits() {
        assert_eq!(scroll_text("hi", 0.0, 10), "hi");
        assert_eq!(scroll_text("abcde", 5.0, 5), "abcde"); // exactly width, offset ignored
    }

    #[test]
    fn scroll_text_wraps_to_fixed_width() {
        let out = scroll_text("abcdefgh", 0.0, 5);
        assert_eq!(out.chars().count(), 5);
        assert_eq!(out, "abcde");
    }

    #[test]
    fn scroll_text_offset_wraps_modulo() {
        // padded len = 8 + 7-char separator = 15; offset 15 == offset 0, no panic.
        assert_eq!(scroll_text("abcdefgh", 15.0, 5), scroll_text("abcdefgh", 0.0, 5));
        assert_eq!(scroll_text("abcdefgh", 1000.0, 5).chars().count(), 5);
    }
}
