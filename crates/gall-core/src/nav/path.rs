//! Path helpers: Miller-column ancestors, copy formats, env expansion.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// The chain of folders from the root to `path` inclusive; each one becomes a
/// Miller column (N-01). `/home/u/docs` → `/`, `/home`, `/home/u`, `/home/u/docs`.
pub fn ancestors(path: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = path.ancestors().map(Path::to_path_buf).collect();
    v.reverse();
    v.retain(|p| !p.as_os_str().is_empty());
    v
}

/// `std::fs::canonicalize` with the Windows verbatim prefix removed:
/// `\\?\C:\x` → `C:\x`, `\\?\UNC\srv\share\x` → `\\srv\share\x`. Paths are compared
/// as strings in the UI (colours, pins, "same folder" checks), so every path handed
/// out must use the ordinary form. Unchanged on other platforms.
pub fn canonicalize(p: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(p).map(strip_verbatim)
}

pub fn strip_verbatim(p: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return p;
    }
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        // Only drive paths; other verbatim forms (\\?\Volume{…}) have no plain spelling.
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    p
}

/// Display name for a path segment / tab: the last component, or the root itself.
pub fn display_name(path: &Path) -> String {
    match path.components().next_back() {
        Some(Component::Normal(n)) => n.to_string_lossy().into_owned(),
        _ => path.to_string_lossy().into_owned(),
    }
}

/// Clipboard path formats (N-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PathFormat {
    /// Native separators.
    #[default]
    Native,
    NativeQuoted,
    /// Backslashes doubled (escaped).
    Escaped,
    Forward,
    ForwardQuoted,
    /// `file://` URI (useful on Linux desktops).
    Uri,
    /// The path as Linux sees it under WSL: `\\wsl.localhost\Ubuntu\home\u` → `/home/u`,
    /// `C:\Users\me` → `/mnt/c/Users/me`. Other paths stay as they are.
    Linux,
}

pub fn format_path(path: &str, fmt: PathFormat) -> String {
    match fmt {
        PathFormat::Native => path.to_string(),
        PathFormat::NativeQuoted => format!("\"{path}\""),
        PathFormat::Escaped => path.replace('\\', "\\\\"),
        PathFormat::Forward => path.replace('\\', "/"),
        PathFormat::ForwardQuoted => format!("\"{}\"", path.replace('\\', "/")),
        PathFormat::Linux => {
            crate::fs::wsl::linux_path_for(path).unwrap_or_else(|| path.to_string())
        }
        PathFormat::Uri => {
            let looks_windows = cfg!(windows)
                || path.starts_with(r"\\")
                || (path.len() >= 2
                    && path.as_bytes()[0].is_ascii_alphabetic()
                    && path.as_bytes()[1] == b':');
            encode_file_uri_for(path, looks_windows)
        }
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode_utf8(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    if out.contains(&0) {
        return None;
    }
    String::from_utf8(out).ok()
}

fn percent_encode_uri_bytes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Decodes a `file://` URI into a local filesystem `PathBuf`.
/// Returns `None` if `uri` is not a `file://` URI (case-insensitive scheme),
/// contains NUL (`%00`) or invalid UTF-8 sequences, or names a non-local host
/// (on POSIX, anything other than empty or `localhost`).
pub fn decode_file_uri(uri: &str) -> Option<PathBuf> {
    decode_file_uri_for(uri, cfg!(windows))
}

/// Platform-parameterized `file://` URI decoder so POSIX and Windows rules can both be
/// unit-tested on any host OS.
pub fn decode_file_uri_for(uri: &str, windows: bool) -> Option<PathBuf> {
    let rest = uri
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("file://"))
        .map(|_| &uri[7..])?;
    let (host, raw_path, had_leading_slash) = if rest.starts_with('/') {
        ("", rest, true)
    } else if let Some((h, p)) = rest.split_once('/') {
        (h, p, false)
    } else {
        return None;
    };

    let local_host = host.is_empty() || host.eq_ignore_ascii_case("localhost");
    if !windows {
        if !local_host {
            return None;
        }
        let decoded = percent_decode_utf8(raw_path)?;
        let path = if had_leading_slash {
            decoded
        } else {
            format!("/{decoded}")
        };
        return Some(PathBuf::from(path));
    }

    if local_host {
        let decoded = percent_decode_utf8(raw_path)?;
        let p = if had_leading_slash {
            decoded
        } else {
            format!("/{decoded}")
        };
        if let Some(unc) = p.strip_prefix("//") {
            return Some(PathBuf::from(format!(r"\\{}", unc.replace('/', "\\"))));
        }
        let b = p.as_bytes();
        if b.len() >= 3
            && b[0] == b'/'
            && b[1].is_ascii_alphabetic()
            && b[2] == b':'
            && (b.len() == 3 || b[3] == b'/')
        {
            let mut drive = p[1..].replace('/', "\\");
            if b.len() == 3 {
                drive.push('\\'); // `file:///C:` is the root of C:, not "the current folder of C:"
            }
            return Some(PathBuf::from(drive));
        }
        return Some(PathBuf::from(p.replace('/', "\\")));
    }

    let decoded_host = percent_decode_utf8(host)?;
    let decoded_path = percent_decode_utf8(raw_path)?;
    let hb = decoded_host.as_bytes();
    if hb.len() == 2 && hb[0].is_ascii_alphabetic() && hb[1] == b':' {
        return Some(PathBuf::from(format!(
            "{decoded_host}\\{}",
            decoded_path.replace('/', "\\")
        )));
    }
    Some(PathBuf::from(format!(
        r"\\{decoded_host}\{}",
        decoded_path.replace('/', "\\")
    )))
}

/// Convenience helper for CLI and address-bar inputs: if `input` is a `file://` URI
/// that decodes to a local path, returns that path string; otherwise returns `input`
/// unchanged.
pub fn file_uri_or_raw(input: &str) -> String {
    decode_file_uri(input)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.to_string())
}

/// Encodes a local path as a canonical `file://` URI (RFC 8089).
/// Strips Windows verbatim prefixes (`\\?\`, `\\?\UNC\`) before encoding.
pub fn encode_file_uri(path: &Path) -> String {
    encode_file_uri_for(&path.to_string_lossy(), cfg!(windows))
}

/// Platform-parameterized `file://` URI encoder.
pub fn encode_file_uri_for(path: &str, windows: bool) -> String {
    let stripped = if windows {
        if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else if let Some(rest) = path.strip_prefix(r"\\?\") {
            let b = rest.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                rest.to_string()
            } else {
                path.to_string()
            }
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    };
    // Only on Windows is a backslash a separator: on POSIX it is a character of the file name.
    let fwd = if windows {
        stripped.replace('\\', "/")
    } else {
        stripped
    };
    if windows {
        if let Some(unc) = fwd.strip_prefix("//") {
            return format!("file://{}", percent_encode_uri_bytes(unc));
        }
    }
    let encoded = percent_encode_uri_bytes(&fwd);
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

/// Expands `~`, `$VAR`, `${VAR}` and `%VAR%` (C-05). Unknown variables are
/// left untouched so typos stay visible to the user.
pub fn expand(input: &str) -> String {
    let mut s = input.trim().to_string();
    // A pasted path in quotes (Explorer's "Copy as path", a shell): the quotes go.
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            s = s[1..s.len() - 1].trim().to_string();
        }
    }
    if s == "~" || s.starts_with("~/") || (cfg!(windows) && s.starts_with("~\\")) {
        if let Some(h) = dirs::home_dir() {
            s = format!("{}{}", h.to_string_lossy(), &s[1..]);
        }
    }
    let lookup = |name: &str| -> Option<String> {
        std::env::var(name).ok().or_else(|| {
            (name.eq_ignore_ascii_case("USERPROFILE") || name == "HOME")
                .then(|| dirs::home_dir().map(|h| h.to_string_lossy().into_owned()))
                .flatten()
        })
    };
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let rest: String = chars[i + 1..].iter().collect();
        if c == '%' {
            if let Some(end) = rest.find('%') {
                let name = &rest[..end];
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    if let Some(v) = lookup(name) {
                        out.push_str(&v);
                        // `end` counts bytes, `i` characters.
                        i += name.chars().count() + 2;
                        continue;
                    }
                }
            }
        } else if c == '$' {
            let (name, consumed) = if rest.starts_with('{') {
                match rest.find('}') {
                    Some(end) => (rest[1..end].to_string(), rest[..=end].chars().count()),
                    None => (String::new(), 0),
                }
            } else {
                let n: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let l = n.chars().count();
                (n, l)
            };
            if !name.is_empty() {
                if let Some(v) = lookup(&name) {
                    out.push_str(&v);
                    i += consumed + 1;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Autocomplete for the path editor (N-09): folders in the parent of what was
/// typed whose names start with the typed fragment (case-insensitive). Capped.
pub fn complete(typed: &str, limit: usize) -> Vec<String> {
    let typed = expand(typed);
    let p = Path::new(&typed);
    let (dir, frag) = if typed.ends_with(std::path::MAIN_SEPARATOR) || typed.ends_with('/') {
        (p.to_path_buf(), String::new())
    } else {
        match (p.parent(), p.file_name()) {
            (Some(d), Some(f)) => (d.to_path_buf(), f.to_string_lossy().to_lowercase()),
            _ => return vec![],
        }
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut v: Vec<String> = rd
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.starts_with(&frag) && (!n.starts_with('.') || frag.starts_with('.'))
        })
        // All matches, then the first by name: the folder's own order is arbitrary. The
        // entry's type needs no extra stat, except for symlinks (followed to folders).
        .filter(|e| match e.file_type() {
            Ok(t) if !t.is_symlink() => t.is_dir(),
            _ => e.path().is_dir(),
        })
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    v.sort_by_key(|s| s.to_lowercase());
    v.truncate(limit);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn ancestors_chain() {
        let a = ancestors(Path::new("/home/u/docs"));
        assert_eq!(
            a,
            ["/", "/home", "/home/u", "/home/u/docs"].map(PathBuf::from)
        );
        assert_eq!(display_name(Path::new("/")), "/");
        assert_eq!(display_name(Path::new("/home/u")), "u");
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_prefix_stripped() {
        let s = |p: &str| strip_verbatim(PathBuf::from(p));
        assert_eq!(s(r"\\?\C:\Users\x"), PathBuf::from(r"C:\Users\x"));
        assert_eq!(s(r"\\?\UNC\srv\share\d"), PathBuf::from(r"\\srv\share\d"));
        assert_eq!(s(r"C:\plain"), PathBuf::from(r"C:\plain"));
        let c = canonicalize(Path::new(".")).unwrap();
        assert!(!c.to_string_lossy().starts_with(r"\\?\"), "{c:?}");
    }

    #[cfg(unix)]
    #[test]
    fn canonicalize_is_plain_on_unix() {
        assert_eq!(canonicalize(Path::new("/")).unwrap(), PathBuf::from("/"));
    }

    #[test]
    fn formats() {
        let p = r"C:\My Docs\a.txt";
        assert_eq!(
            format_path(p, PathFormat::NativeQuoted),
            r#""C:\My Docs\a.txt""#
        );
        assert_eq!(format_path(p, PathFormat::Escaped), r"C:\\My Docs\\a.txt");
        assert_eq!(format_path(p, PathFormat::Forward), "C:/My Docs/a.txt");
        assert_eq!(format_path("/a b/c", PathFormat::Uri), "file:///a%20b/c");
        assert_eq!(
            format_path(r"C:\My Docs\a.txt", PathFormat::Uri),
            "file:///C:/My%20Docs/a.txt"
        );
        assert_eq!(
            format_path(r"\\server\share\a b.txt", PathFormat::Uri),
            "file://server/share/a%20b.txt"
        );
    }

    #[test]
    fn backslashes_and_bare_drives_in_file_uris() {
        // POSIX: a backslash is part of the name and is escaped, not turned into a folder separator.
        assert_eq!(encode_file_uri_for("/tmp/a\\b", false), "file:///tmp/a%5Cb");
        assert_eq!(
            decode_file_uri_for("file:///tmp/a%5Cb", false),
            Some(PathBuf::from("/tmp/a\\b"))
        );
        // Windows: a separator.
        assert_eq!(
            encode_file_uri_for(r"C:\Users\a b", true),
            "file:///C:/Users/a%20b"
        );
        // `file:///C:` is the root of the drive.
        assert_eq!(
            decode_file_uri_for("file:///C:", true),
            Some(PathBuf::from("C:\\"))
        );
        assert_eq!(
            decode_file_uri_for("file:///C:/", true),
            Some(PathBuf::from("C:\\"))
        );
        assert_eq!(
            decode_file_uri_for("file:///C:/x", true),
            Some(PathBuf::from("C:\\x"))
        );
    }

    #[test]
    fn decode_file_uri_posix_and_validation() {
        assert_eq!(
            decode_file_uri_for("file:///home/u/My%20File.txt", false),
            Some(PathBuf::from("/home/u/My File.txt"))
        );
        assert_eq!(
            decode_file_uri_for("file://localhost/tmp/a%25b", false),
            Some(PathBuf::from("/tmp/a%b"))
        );
        assert_eq!(
            decode_file_uri_for("FILE://LOCALHOST/tmp/a", false),
            Some(PathBuf::from("/tmp/a"))
        );
        assert_eq!(
            decode_file_uri_for("file:///home/u/%C4%8Crka.txt", false),
            Some(PathBuf::from("/home/u/Črka.txt"))
        );
        // omarchy-transcode writes unencoded paths in file:// URIs.
        assert_eq!(
            decode_file_uri_for("file:///tmp/a b#1?q.jpg", false),
            Some(PathBuf::from("/tmp/a b#1?q.jpg"))
        );
        // Incomplete or invalid % sequences remain literal when valid UTF-8.
        assert_eq!(
            decode_file_uri_for("file:///bad%2", false),
            Some(PathBuf::from("/bad%2"))
        );
        assert_eq!(
            decode_file_uri_for("file:///bad%zz", false),
            Some(PathBuf::from("/bad%zz"))
        );
        // Reject NUL, invalid UTF-8, non-local hosts, localhost boundary tricks, non-file URIs.
        assert_eq!(decode_file_uri_for("file:///tmp/a%00b", false), None);
        assert_eq!(decode_file_uri_for("file:///tmp/%FF%FE", false), None);
        assert_eq!(decode_file_uri_for("file://localhost2/tmp/a", false), None);
        assert_eq!(decode_file_uri_for("file://remotehost/tmp/a", false), None);
        assert_eq!(decode_file_uri_for("file://localhost", false), None);
        assert_eq!(decode_file_uri_for("https://example.com/a", false), None);
        assert_eq!(decode_file_uri_for("/plain/path", false), None);
        // Multi-byte UTF-8 straddling byte index 7 must not panic.
        assert_eq!(decode_file_uri_for("abčdefg", false), None);
        assert_eq!(
            decode_file_uri_for("file:///", false),
            Some(PathBuf::from("/"))
        );
        assert_eq!(
            decode_file_uri_for("file://localhost/", false),
            Some(PathBuf::from("/"))
        );
    }

    #[test]
    fn decode_file_uri_windows_drive_and_unc() {
        assert_eq!(
            decode_file_uri_for("file:///C:/Users/u/My%20File.txt", true),
            Some(PathBuf::from(r"C:\Users\u\My File.txt"))
        );
        assert_eq!(
            decode_file_uri_for("file:///C%3A/a.txt", true),
            Some(PathBuf::from(r"C:\a.txt"))
        );
        assert_eq!(
            decode_file_uri_for("file://localhost/D:/x", true),
            Some(PathBuf::from(r"D:\x"))
        );
        assert_eq!(
            decode_file_uri_for("file://server/share/a%20b.txt", true),
            Some(PathBuf::from(r"\\server\share\a b.txt"))
        );
        assert_eq!(
            decode_file_uri_for("file:////server/share/x", true),
            Some(PathBuf::from(r"\\server\share\x"))
        );
        // Non-alphabetic drive prefix must not be stripped.
        assert_eq!(
            decode_file_uri_for("file:///1:/foo", true),
            Some(PathBuf::from(r"\1:\foo"))
        );
    }

    #[test]
    fn encode_file_uri_round_trips() {
        let posix_raw = "/home/u/Črka #1?a%2.txt";
        let posix_uri = encode_file_uri_for(posix_raw, false);
        assert_eq!(posix_uri, "file:///home/u/%C4%8Crka%20%231%3Fa%252.txt");
        assert_eq!(
            decode_file_uri_for(&posix_uri, false),
            Some(PathBuf::from(posix_raw))
        );

        let win_uri = encode_file_uri_for(r"C:\My Docs\a #1.txt", true);
        assert_eq!(win_uri, "file:///C:/My%20Docs/a%20%231.txt");
        assert_eq!(
            decode_file_uri_for(&win_uri, true),
            Some(PathBuf::from(r"C:\My Docs\a #1.txt"))
        );
        assert_eq!(
            encode_file_uri_for(r"\\?\C:\My Docs\a.txt", true),
            "file:///C:/My%20Docs/a.txt"
        );

        let unc_uri = encode_file_uri_for(r"\\my server\share\a b.txt", true);
        assert_eq!(unc_uri, "file://my%20server/share/a%20b.txt");
        assert_eq!(
            decode_file_uri_for(&unc_uri, true),
            Some(PathBuf::from(r"\\my server\share\a b.txt"))
        );
        assert_eq!(
            encode_file_uri_for(r"\\?\UNC\my server\share\a b.txt", true),
            unc_uri
        );

        assert_eq!(file_uri_or_raw("/plain/path"), "/plain/path");
        #[cfg(not(windows))]
        assert_eq!(
            file_uri_or_raw("file:///tmp/hello%20world"),
            "/tmp/hello world"
        );
    }

    #[test]
    fn expansion() {
        std::env::set_var("GALL_T", "/x");
        assert_eq!(expand("%GALL_T%/y"), "/x/y");
        assert_eq!(expand("$GALL_T/y"), "/x/y");
        assert_eq!(expand("${GALL_T}y"), "/xy");
        assert_eq!(expand("%NOPE_NOT_SET%"), "%NOPE_NOT_SET%");
        assert_eq!(expand("100%"), "100%");
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand("~/q"), format!("{}/q", home.to_string_lossy()));
        // Pasted in quotes (Explorer's "Copy as path", a shell).
        assert_eq!(expand(r#""C:\Users\u\My Files""#), r"C:\Users\u\My Files");
        assert_eq!(expand("  '/home/u/My Files'  "), "/home/u/My Files");
        assert_eq!(expand("\"%GALL_T%/y\""), "/x/y");
        // Characters after a variable with letters beyond ASCII stay in place.
        std::env::set_var("GALL_Č", "/c");
        assert_eq!(expand("%GALL_Č%/rest"), "/c/rest");
        assert_eq!(expand("$GALL_Č/rest"), "/c/rest");
        assert_eq!(expand("${GALL_Č}/rest"), "/c/rest");
        #[cfg(windows)]
        assert_eq!(expand(r"~\q"), format!(r"{}\q", home.to_string_lossy()));
    }

    #[test]
    fn completion() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("Windows")).unwrap();
        std::fs::create_dir(d.path().join("work")).unwrap();
        std::fs::create_dir(d.path().join("other")).unwrap();
        std::fs::write(d.path().join("wfile"), "").unwrap();
        let typed = format!("{}/w", d.path().display());
        let r = complete(&typed, 10);
        assert_eq!(r.len(), 2);
        assert!(r[0].ends_with("Windows") && r[1].ends_with("work"));
    }

    /// In a big folder the suggestions are the first matches by name, not whichever
    /// ones the folder happens to list first.
    #[test]
    fn completion_in_a_big_folder_offers_the_first_names() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..300 {
            std::fs::create_dir(d.path().join(format!("w{i:03}"))).unwrap();
        }
        let r = complete(&format!("{}/w", d.path().display()), 5);
        let names: Vec<_> = r.iter().map(|p| display_name(Path::new(p))).collect();
        assert_eq!(names, ["w000", "w001", "w002", "w003", "w004"]);
        // A symlink to a folder is still offered.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path().join("w100"), d.path().join("a-link")).unwrap();
            let r = complete(&format!("{}/a", d.path().display()), 5);
            assert_eq!(r.len(), 1);
        }
    }
}
