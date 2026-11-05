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
            let fwd = path.replace('\\', "/");
            let encoded: String = fwd
                .bytes()
                .map(|b| match b {
                    b'A'..=b'Z'
                    | b'a'..=b'z'
                    | b'0'..=b'9'
                    | b'-'
                    | b'_'
                    | b'.'
                    | b'~'
                    | b'/'
                    | b':' => (b as char).to_string(),
                    _ => format!("%{b:02X}"),
                })
                .collect();
            if encoded.starts_with('/') {
                format!("file://{encoded}")
            } else {
                format!("file:///{encoded}")
            }
        }
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
