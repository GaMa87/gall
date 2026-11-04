//! Recursive wildcard search (Q-01). Streaming and cancellable.

use super::entry::FileEntry;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Case-insensitive glob supporting `*` and `?`. A pattern without wildcards
/// matches as a substring, which is what users expect when typing a fragment.
#[derive(Debug, Clone)]
pub struct Wildcard {
    pat: Vec<char>,
    substring: bool,
    /// No accented letters typed: "porocilo" also finds "Poročilo" (see `fuzzy::fold_char`).
    fold: bool,
}

impl Wildcard {
    pub fn new(pattern: &str) -> Self {
        let p = pattern.trim().to_lowercase();
        let substring = !p.contains('*') && !p.contains('?');
        Wildcard {
            fold: p.chars().all(|c| super::fuzzy::fold_char(c) == c),
            pat: p.chars().collect(),
            substring,
        }
    }

    pub fn matches(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        let n: Vec<char> = if self.fold {
            lower.chars().map(super::fuzzy::fold_char).collect()
        } else {
            lower.chars().collect()
        };
        if self.substring {
            if self.pat.is_empty() {
                return true;
            }
            return n.windows(self.pat.len()).any(|w| w == self.pat.as_slice());
        }
        glob(&self.pat, &n)
    }
}

/// Iterative glob match with single backtrack point (linear for typical input).
fn glob(p: &[char], s: &[char]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[derive(Debug, Clone, Copy)]
pub struct SearchOptions {
    pub include_hidden: bool,
    pub max_results: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            max_results: 10_000,
        }
    }
}

/// Walks `root` and calls `on_hit` for each match. Returns the number of hits.
/// Stops early when `cancel` is set or `max_results` is reached.
pub fn search(
    root: &Path,
    pattern: &str,
    opts: SearchOptions,
    cancel: &AtomicBool,
    mut on_hit: impl FnMut(FileEntry),
) -> usize {
    let wc = Wildcard::new(pattern);
    let mut hits = 0;
    let walker = walkdir::WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter();
    // Like the index: no virtual file systems (/proc, /sys) and no network shares inside.
    let remote = super::index::remote_mounts();
    let walker = walker.filter_entry(|e| {
        (opts.include_hidden || !e.file_name().to_string_lossy().starts_with('.'))
            && !super::index::is_system_skip(e.path())
            && !remote.contains(e.path())
    });
    for item in walker {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(item) = item else { continue };
        if wc.matches(&item.file_name().to_string_lossy()) {
            if let Ok(meta) = item.path().symlink_metadata() {
                on_hit(FileEntry::from_meta(item.path(), &meta));
                hits += 1;
                if hits >= opts.max_results {
                    break;
                }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_fold_unless_typed() {
        assert!(Wildcard::new("porocilo").matches("Poročilo čšž.txt"));
        assert!(Wildcard::new("por*csz*").matches("Poročilo čšž.txt"));
        assert!(Wildcard::new("poročilo").matches("Poročilo.txt"));
        assert!(!Wildcard::new("poročilo").matches("porocilo.txt"));
    }

    #[test]
    fn wildcard_semantics() {
        assert!(Wildcard::new("*.docx").matches("Report.DOCX"));
        assert!(!Wildcard::new("*.docx").matches("report.doc"));
        assert!(Wildcard::new("filename?.txt").matches("filename1.txt"));
        assert!(!Wildcard::new("filename?.txt").matches("filename12.txt"));
        assert!(Wildcard::new("*.*").matches("a.b"));
        assert!(Wildcard::new("port").matches("Report.docx"));
        assert!(Wildcard::new("a*b*c").matches("aXXbYYc"));
        assert!(!Wildcard::new("a*b*c").matches("aXXbYY"));
    }

    #[test]
    fn recursive_search_and_cancel() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("x/y")).unwrap();
        std::fs::write(d.path().join("x/y/deep.txt"), "").unwrap();
        std::fs::write(d.path().join("top.txt"), "").unwrap();
        std::fs::write(d.path().join(".hid.txt"), "").unwrap();
        let cancel = AtomicBool::new(false);
        let mut found = vec![];
        search(d.path(), "*.txt", SearchOptions::default(), &cancel, |e| {
            found.push(e.name)
        });
        found.sort();
        assert_eq!(found, ["deep.txt", "top.txt"]);

        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            search(d.path(), "*", SearchOptions::default(), &cancel, |_| {}),
            0
        );
    }
}
