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
    // No control character (a lone CR would start a new line in a Windows .url file).
    if !s.chars().any(char::is_control)
        && (s.starts_with("http://") || s.starts_with("https://"))
        && !s.contains(' ')
    {
        return ClipText::Url(s.to_string());
    }
    let unquoted = s.trim_matches('"');
    // A `file://` URI is decoded (`%20`); anything else is a path as written.
    let expanded = crate::nav::path::expand(&crate::nav::path::file_uri_or_raw(unquoted));
    let p = expanded.as_str();
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
        assert_eq!(classify_text("http://a.example/x\ry"), ClipText::Text);
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            classify_text(&format!("\"{}\"", d.path().display())),
            ClipText::Path(d.path().to_string_lossy().into())
        );
    }

    #[test]
    fn a_file_uri_with_escapes_is_a_path() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("a b");
        std::fs::create_dir(&dir).unwrap();
        let uri = crate::nav::path::encode_file_uri(&dir);
        assert!(uri.contains("%20"));
        assert_eq!(
            classify_text(&uri),
            ClipText::Path(dir.to_string_lossy().into())
        );
    }
}
