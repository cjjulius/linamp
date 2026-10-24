//! LCD text helpers: track-time formatting and Winamp-style marquee scrolling.

use std::time::Duration;

/// `mm:ss` for a track position / length, or `h:mm:ss` once it reaches an hour
/// (so a 75-minute track reads `1:15:30`, not `75:30`), matching the classic
/// playlist's `fmt_pl_time`.
pub(crate) fn fmt_time(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
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

/// A stream URL shown in the title LCD, with the scheme trimmed for brevity.
pub(crate) fn display_url(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_url_trims_scheme() {
        assert_eq!(display_url("https://stream.example.com/radio"), "stream.example.com/radio");
        assert_eq!(display_url("http://host/path"), "host/path");
        // No scheme (or an unusual one) passes through unchanged.
        assert_eq!(display_url("stream.example.com"), "stream.example.com");
    }

    #[test]
    fn fmt_time_handles_hours() {
        assert_eq!(fmt_time(Duration::from_secs(0)), "00:00");
        assert_eq!(fmt_time(Duration::from_secs(225)), "03:45"); // under an hour: mm:ss
        assert_eq!(fmt_time(Duration::from_secs(59 * 60 + 59)), "59:59");
        assert_eq!(fmt_time(Duration::from_secs(3600)), "1:00:00"); // exactly an hour
        assert_eq!(fmt_time(Duration::from_secs(75 * 60 + 30)), "1:15:30"); // was 75:30
    }

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
