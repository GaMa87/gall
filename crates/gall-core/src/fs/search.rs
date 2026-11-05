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

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub include_hidden: bool,
    pub max_results: usize,
    /// Skip what `.gitignore` (and `.ignore`) leave out. `.gallignore` is always honoured.
    pub respect_gitignore: bool,
    /// Names or globs (gitignore syntax) never searched, e.g. `node_modules`.
    pub exclude: Vec<String>,
    /// Search everything: no `.gallignore`, no cache folders skipped.
    pub everything: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            max_results: 10_000,
            respect_gitignore: false,
            exclude: vec![],
            everything: false,
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
    // Like the index: no virtual file systems (/proc, /sys) and no network shares inside.
    let remote = super::index::remote_mounts();
    let include_hidden = opts.include_hidden;
    let everything = opts.everything;
    let mut wb = ignore::WalkBuilder::new(root);
    wb.standard_filters(false)
        .git_ignore(opts.respect_gitignore)
        .git_global(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .ignore(opts.respect_gitignore)
        .require_git(true)
        .follow_links(false)
        .filter_entry(move |e| {
            if e.depth() == 0 {
                return true;
            }
            // Hidden as in the file list: a dot name, or (Windows) the hidden attribute.
            let hidden = || {
                e.file_name().to_string_lossy().starts_with('.')
                    || (cfg!(windows) && e.metadata().is_ok_and(|m| super::entry::hidden_attr(&m)))
            };
            (include_hidden || !hidden())
                && !super::index::is_system_skip(e.path())
                && !remote.contains(e.path())
                && (everything || !super::ignorefile::skip_cache(e))
        });
    super::ignorefile::configure(&mut wb, !opts.everything);
    let exclude: Vec<&str> = opts
        .exclude
        .iter()
        .flat_map(|g| g.split(','))
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .collect();
    if !exclude.is_empty() {
        let mut ob = ignore::overrides::OverrideBuilder::new(root);
        for g in exclude {
            // A broken glob in the settings must not stop the search.
            let _ = ob.add(&format!("!{g}"));
        }
        if let Ok(o) = ob.build() {
            wb.overrides(o);
        }
    }
    for item in wb.build() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(item) = item else { continue };
        if item.depth() == 0 {
            continue;
        }
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

    /// Windows: folders and files with the hidden attribute (`AppData`) are skipped
    /// like dot names, as the file list and find-in-files skip them.
    #[cfg(windows)]
    #[test]
    fn hidden_attribute_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("AppData/Local")).unwrap();
        std::fs::write(d.path().join("AppData/Local/cache.txt"), "").unwrap();
        std::fs::write(d.path().join("desktop.txt"), "").unwrap();
        std::fs::write(d.path().join("visible.txt"), "").unwrap();
        for p in ["AppData", "desktop.txt"] {
            let ok = std::process::Command::new("attrib")
                .arg("+h")
                .arg(d.path().join(p))
                .status()
                .unwrap()
                .success();
            assert!(ok, "attrib +h {p}");
        }
        let cancel = AtomicBool::new(false);
        let mut found = vec![];
        search(d.path(), "*.txt", SearchOptions::default(), &cancel, |e| {
            found.push(e.name)
        });
        assert_eq!(found, ["visible.txt"]);
        let all = SearchOptions {
            include_hidden: true,
            ..Default::default()
        };
        assert_eq!(search(d.path(), "*.txt", all, &cancel, |_| {}), 3);
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

    #[test]
    fn skips_excluded_gitignored_and_gallignored() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        for dir in [".git", "node_modules/pkg", "gen", "src", "out"] {
            std::fs::create_dir_all(p.join(dir)).unwrap();
        }
        for f in [
            "node_modules/pkg/index.ts",
            "gen/api.ts",
            "src/main.ts",
            "out/main.ts",
        ] {
            std::fs::write(p.join(f), "").unwrap();
        }
        std::fs::write(p.join(".gitignore"), "out/\n").unwrap();
        std::fs::write(p.join(".gallignore"), "/gen/\n").unwrap();
        let cancel = AtomicBool::new(false);
        let run = |opts: SearchOptions| {
            let mut v = vec![];
            search(p, "*.ts", opts, &cancel, |e| {
                v.push(
                    e.path
                        .strip_prefix(p.to_string_lossy().as_ref())
                        .unwrap_or(&e.path)
                        .replace('\\', "/")
                        .trim_start_matches('/')
                        .to_string(),
                )
            });
            v.sort();
            v
        };
        // .gallignore always applies.
        assert_eq!(
            run(SearchOptions::default()),
            ["node_modules/pkg/index.ts", "out/main.ts", "src/main.ts"]
        );
        let opts = SearchOptions {
            respect_gitignore: true,
            exclude: vec!["node_modules".into()],
            ..Default::default()
        };
        assert_eq!(run(opts), ["src/main.ts"]);
        // A search started inside an ignored folder still lists it.
        let mut inside = vec![];
        search(
            &p.join("gen"),
            "*",
            SearchOptions::default(),
            &cancel,
            |e| inside.push(e.name),
        );
        assert_eq!(inside, ["api.ts"]);
        // Everything: no .gallignore, no cache folders skipped.
        std::fs::create_dir_all(p.join("cache")).unwrap();
        std::fs::write(
            p.join("cache/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
        std::fs::write(p.join("cache/c.ts"), "").unwrap();
        assert!(!run(SearchOptions::default()).contains(&"cache/c.ts".to_string()));
        let all = run(SearchOptions {
            everything: true,
            ..Default::default()
        });
        assert_eq!(all.len(), 5, "{all:?}");
    }
}
