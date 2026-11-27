//! Recursive wildcard search (Q-01). Streaming and cancellable.

use super::entry::FileEntry;
use super::search_filter::{
    walk_hidden_matches, CountBudget, FilterConfig, FilterMatcher, HiddenCount, ProbeMatch,
};
use serde::Serialize;
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
    /// The pattern as bytes when it is plain ASCII: most names are too, and those are
    /// then matched as they are, without building a lower-case copy of every name.
    ascii: Option<Vec<u8>>,
}

impl Wildcard {
    pub fn new(pattern: &str) -> Self {
        let p = crate::util::unicode::nfc(pattern.trim()).to_lowercase();
        let substring = !p.contains('*') && !p.contains('?');
        Wildcard {
            ascii: p.is_ascii().then(|| p.as_bytes().to_vec()),
            fold: p.chars().all(|c| super::fuzzy::fold_char(c) == c),
            pat: p.chars().collect(),
            substring,
        }
    }

    pub fn matches(&self, name: &str) -> bool {
        if let (Some(pat), true) = (&self.ascii, name.is_ascii()) {
            let name = name.as_bytes();
            if self.substring {
                return pat.is_empty()
                    || name.windows(pat.len()).any(|w| w.eq_ignore_ascii_case(pat));
            }
            return glob(pat, name, |p, c| p == c.to_ascii_lowercase());
        }
        let lower = crate::util::unicode::nfc(name).to_lowercase();
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
        glob(&self.pat, &n, |p, c| p == c)
    }
}

/// Iterative glob match with single backtrack point (linear for typical input).
fn glob<T: Copy + PartialEq + From<u8>>(p: &[T], s: &[T], same: impl Fn(T, T) -> bool) -> bool {
    let (any_one, any_run) = (T::from(b'?'), T::from(b'*'));
    let (mut pi, mut si) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == any_one || same(p[pi], s[si])) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == any_run {
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
    while pi < p.len() && p[pi] == any_run {
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

/// One name-search hit, optionally marked when normal search filters would hide it
/// (shown because `Alt+I` / "search everything" is enabled).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    #[serde(flatten)]
    pub entry: FileEntry,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// Like [`search`], returning [`SearchHit`]s. When `opts.everything` is set and
/// `normal_filter` is given, each hit is annotated with whether normal filters
/// would have hidden it.
pub fn search_annotated(
    root: &Path,
    pattern: &str,
    opts: SearchOptions,
    normal_filter: Option<&FilterConfig>,
    cancel: &AtomicBool,
    mut on_hit: impl FnMut(SearchHit),
) -> usize {
    let matcher = if opts.everything {
        normal_filter.map(|cfg| FilterMatcher::new(root, cfg.clone()))
    } else {
        None
    };
    search(root, pattern, opts, cancel, |entry| {
        let hidden = matcher
            .as_ref()
            .is_some_and(|m| m.is_path_hidden(Path::new(&entry.path), entry.is_dir));
        on_hit(SearchHit { entry, hidden });
    })
}

/// Counts entries hidden by `filter` under `root` whose names match `pattern`,
/// within `budget`. Returns zero when `pattern` is empty or cancelled.
pub fn count_hidden(
    root: &Path,
    pattern: &str,
    filter: &FilterConfig,
    budget: CountBudget,
    cancel: &AtomicBool,
) -> HiddenCount {
    if pattern.trim().is_empty() {
        return HiddenCount::default();
    }
    let wc = Wildcard::new(pattern);
    walk_hidden_matches(
        root,
        filter,
        None,
        budget,
        cancel,
        |_path, name, _is_dir| {
            if wc.matches(name) {
                ProbeMatch::Matched
            } else {
                ProbeMatch::NoMatch
            }
        },
    )
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
    // The folders are walked on several threads (the walk is most of the time in a large
    // tree); the hits come back to this thread, which hands them on in the order they
    // are found.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    let done = AtomicBool::new(false);
    // Bounded, so a slow receiver holds the walk back instead of letting every match of
    // the tree queue up in memory.
    let (found, results) = std::sync::mpsc::sync_channel::<FileEntry>(4096);
    std::thread::scope(|scope| {
        let (wc, done) = (&wc, &done);
        let walker = wb.threads(threads).build_parallel();
        scope.spawn(move || {
            walker.run(|| {
                let found = found.clone();
                Box::new(move |item| {
                    if cancel.load(Ordering::Relaxed) || done.load(Ordering::Relaxed) {
                        return ignore::WalkState::Quit;
                    }
                    let Ok(item) = item else {
                        return ignore::WalkState::Continue;
                    };
                    if item.depth() > 0 && wc.matches(&item.file_name().to_string_lossy()) {
                        if let Ok(meta) = item.path().symlink_metadata() {
                            if found
                                .send(FileEntry::from_meta(item.path(), &meta))
                                .is_err()
                            {
                                return ignore::WalkState::Quit;
                            }
                        }
                    }
                    ignore::WalkState::Continue
                })
            });
        });
        for entry in results {
            // Nothing is handed on after a cancel, also not what was already found.
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            on_hit(entry);
            hits += 1;
            if hits >= opts.max_results {
                done.store(true, Ordering::Relaxed);
                break;
            }
        }
        done.store(true, Ordering::Relaxed);
    });
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

    /// Names that are plain ASCII are matched without building a lower-case copy: the
    /// answer has to be the one the general way gives.
    #[test]
    fn ascii_names_match_as_the_general_way_does() {
        let patterns = [
            "",
            "a",
            "A",
            "txt",
            ".TXT",
            "*",
            "?",
            "*.txt",
            "*.TXT",
            "a*",
            "*a",
            "a?c",
            "a*c*",
            "??",
            "*?*",
            "re*me.md",
            "Read?e*",
            "a.b.c",
            "**",
            "*.*",
            "no such name here",
            "x*y?z",
            "file[1]",
            "a b",
        ];
        let names = [
            "",
            "a",
            "A",
            "abc",
            "ABC",
            "a.txt",
            "A.TXT",
            "readme.md",
            "README.MD",
            "ReadMe.md",
            "a*c",
            "a?c",
            "*",
            "?",
            "a.b.c",
            ".txt",
            "txt",
            "file[1]",
            "a b",
            "xAAyBz",
            "xyz",
        ];
        for pattern in patterns {
            let fast = Wildcard::new(pattern);
            assert!(fast.ascii.is_some());
            let general = Wildcard {
                ascii: None,
                ..fast.clone()
            };
            for name in names {
                assert_eq!(
                    fast.matches(name),
                    general.matches(name),
                    "pattern {pattern:?}, name {name:?}"
                );
            }
        }
    }

    /// The folders are walked on several threads: every match is still found exactly
    /// once, the limit is kept exactly, and nothing arrives after a cancel.
    #[test]
    fn many_folders_give_every_hit_once_and_keep_the_limit() {
        let d = tempfile::tempdir().unwrap();
        let mut expected = vec![];
        for a in 0..12 {
            for b in 0..12 {
                let dir = d.path().join(format!("a{a}/b{b}"));
                std::fs::create_dir_all(&dir).unwrap();
                for f in 0..10 {
                    let hit = f % 2 == 0;
                    let name = if hit {
                        format!("{a}-{b}-{f}.md")
                    } else {
                        format!("{a}-{b}-{f}.txt")
                    };
                    std::fs::write(dir.join(&name), "").unwrap();
                    if hit {
                        expected.push(dir.join(name));
                    }
                }
            }
        }
        expected.sort();
        let cancel = AtomicBool::new(false);
        let mut found = vec![];
        let n = search(d.path(), "*.md", SearchOptions::default(), &cancel, |e| {
            found.push(std::path::PathBuf::from(e.path))
        });
        found.sort();
        assert_eq!(n, expected.len());
        assert_eq!(found, expected);

        let few = SearchOptions {
            max_results: 37,
            ..Default::default()
        };
        let mut got = 0;
        assert_eq!(search(d.path(), "*.md", few, &cancel, |_| got += 1), 37);
        assert_eq!(got, 37);

        // Cancelled from the first hit: no further hit is handed on.
        let mut got = 0;
        search(d.path(), "*.md", SearchOptions::default(), &cancel, |_| {
            got += 1;
            cancel.store(true, Ordering::Relaxed);
        });
        assert_eq!(got, 1);
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

    #[test]
    fn hidden_count_and_annotated_search_cover_all_filter_states() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        for dir in [".git", "src", "gen", "out", "node_modules/pkg"] {
            std::fs::create_dir_all(p.join(dir)).unwrap();
        }
        std::fs::write(p.join("src/report.ts"), "").unwrap();
        std::fs::write(p.join("gen/report_gen.ts"), "").unwrap();
        std::fs::write(p.join("out/secret_only.ts"), "").unwrap();
        std::fs::write(p.join("node_modules/pkg/secret_only2.ts"), "").unwrap();
        std::fs::write(p.join(".gitignore"), "out/\n").unwrap();
        std::fs::write(p.join(".gallignore"), "/gen/\n").unwrap();

        let filter = FilterConfig {
            include_hidden: false,
            respect_gitignore: true,
            exclude: vec!["node_modules".into()],
            use_gallignore: true,
        };
        let cancel = AtomicBool::new(false);

        // Zero visible + 2 hidden (one .gitignore, one node_modules).
        let mut vis = vec![];
        search(
            p,
            "secret_only*",
            SearchOptions {
                respect_gitignore: true,
                exclude: vec!["node_modules".into()],
                ..Default::default()
            },
            &cancel,
            |e| vis.push(e.name),
        );
        assert!(vis.is_empty());
        let hc = count_hidden(
            p,
            "secret_only*",
            &filter,
            CountBudget::for_tests(),
            &cancel,
        );
        assert_eq!(
            hc,
            HiddenCount {
                count: 2,
                capped: false
            }
        );

        // Some visible (1) + 1 hidden (.gallignore).
        let mut vis = vec![];
        search(
            p,
            "report*",
            SearchOptions {
                respect_gitignore: true,
                exclude: vec!["node_modules".into()],
                ..Default::default()
            },
            &cancel,
            |e| vis.push(e.name),
        );
        assert_eq!(vis, ["report.ts"]);
        let hc = count_hidden(p, "report*", &filter, CountBudget::for_tests(), &cancel);
        assert_eq!(
            hc,
            HiddenCount {
                count: 1,
                capped: false
            }
        );

        // Nothing hidden matches.
        let hc = count_hidden(
            p,
            "no_such_name",
            &filter,
            CountBudget::for_tests(),
            &cancel,
        );
        assert_eq!(
            hc,
            HiddenCount {
                count: 0,
                capped: false
            }
        );

        // Alt+I (everything = true): returns both visible and hidden, marking only hidden ones.
        let mut hits = vec![];
        search_annotated(
            p,
            "report*",
            SearchOptions {
                everything: true,
                ..Default::default()
            },
            Some(&filter),
            &cancel,
            |h| hits.push((h.entry.name, h.hidden)),
        );
        hits.sort();
        assert_eq!(
            hits,
            vec![
                ("report.ts".to_string(), false),
                ("report_gen.ts".to_string(), true),
            ]
        );
    }
}
