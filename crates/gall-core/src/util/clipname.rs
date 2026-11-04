//! Names for files created from clipboard contents (O-10): `YYYY-MM-DD HH-mm-ss.ext`.

use chrono::{DateTime, Local};

pub fn timestamp_name(at: DateTime<Local>, ext: &str) -> String {
    format!("{}.{ext}", at.format("%Y-%m-%d %H-%M-%S"))
}

/// Classifies clipboard text: a URL, an existing path, or plain text.
#[derive(Debug, PartialEq, Eq)]
pub enum ClipText {
    Url(String),
    Path(String),
    Text,
}

pub fn classify_text(t: &str) -> ClipText {
    let s = t.trim();
    if !s.contains('\n')
        && (s.starts_with("http://") || s.starts_with("https://"))
        && !s.contains(' ')
    {
        return ClipText::Url(s.to_string());
    }
    let unquoted = s.trim_matches('"');
    let expanded = crate::nav::path::expand(unquoted);
    let p = expanded.strip_prefix("file://").unwrap_or(&expanded);
    if !s.contains('\n')
        && std::path::Path::new(p).is_absolute()
        && std::path::Path::new(p).exists()
    {
        return ClipText::Path(p.to_string());
    }
    ClipText::Text
}

/// Text body for a `.url`-style internet shortcut (opens in the browser).
pub fn url_shortcut(url: &str) -> (String, String) {
    if cfg!(windows) {
        ("url".into(), format!("[InternetShortcut]\r\nURL={url}\r\n"))
    } else {
        (
            "desktop".into(),
            format!("[Desktop Entry]\nType=Link\nName={url}\nURL={url}\nIcon=text-html\n"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn names_and_classification() {
        let t = Local.with_ymd_and_hms(2024, 4, 1, 10, 56, 0).unwrap();
        assert_eq!(timestamp_name(t, "txt"), "2024-04-01 10-56-00.txt");
        assert_eq!(
            classify_text("https://example.com/a.pdf"),
            ClipText::Url("https://example.com/a.pdf".into())
        );
        assert_eq!(classify_text("hello world"), ClipText::Text);
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            classify_text(&format!("\"{}\"", d.path().display())),
            ClipText::Path(d.path().to_string_lossy().into())
        );
    }
}
