//! Content search ("find in files"), built on ripgrep's libraries.
//!
//! Walks a folder in parallel, honours `.gitignore`, skips binary files and
//! streams results per file. Matches carry UTF-16 ranges so the UI can
//! highlight them directly.

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{sinks::Lossy, BinaryDetection, SearcherBuilder};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CaseMode {
    /// Case-insensitive unless the pattern has an upper-case letter.
    #[default]
    Smart,
    Sensitive,
    Insensitive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GrepOptions {
    pub case: CaseMode,
    pub whole_word: bool,
    pub regex: bool,
    /// Globs a file must match (empty = all), e.g. `*.rs, src/**`.
    pub include: Vec<String>,
    /// Globs to skip, e.g. `*.min.js, vendor/`.
    pub exclude: Vec<String>,
    pub include_hidden: bool,
    pub respect_gitignore: bool,
    /// Search everything: no `.gallignore`, no cache folders skipped (the caller
    /// also leaves out the excluded-folders setting and `.gitignore`).
    pub everything: bool,
    pub max_matches: usize,
    /// Files larger than this (bytes) are skipped.
    pub max_filesize: u64,
}

impl Default for GrepOptions {
    fn default() -> Self {
        GrepOptions {
            case: CaseMode::Smart,
            whole_word: false,
            regex: false,
            include: vec![],
            exclude: vec![],
            include_hidden: false,
            respect_gitignore: true,
            everything: false,
            max_matches: 20_000,
            max_filesize: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineMatch {
    pub line: u64,
    /// The line, possibly shortened around the first match.
    pub text: String,
    /// Highlight ranges as UTF-16 `[start, end)` offsets into `text`.
    pub ranges: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMatches {
    pub path: String,
    pub matches: Vec<LineMatch>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepStats {
    pub files: usize,
    pub matches: usize,
    pub searched: usize,
    /// Files kept only in the cloud (OneDrive and the like on Windows) that were not
    /// searched: reading them would download every one.
    pub cloud_only: usize,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// Longest line text sent to the UI; longer lines are cut around the match.
const MAX_LINE: usize = 300;

pub fn build_matcher(pattern: &str, opts: &GrepOptions) -> Result<RegexMatcher, String> {
    let mut b = RegexMatcherBuilder::new();
    match opts.case {
        CaseMode::Smart => b.case_smart(true),
        CaseMode::Sensitive => b.case_insensitive(false),
        CaseMode::Insensitive => b.case_insensitive(true),
    };
    b.word(opts.whole_word)
        .fixed_strings(!opts.regex)
        .line_terminator(Some(b'\n'))
        // `$` also matches before the `\r` of Windows (CRLF) line endings.
        .crlf(true)
        .multi_line(true);
    b.build(pattern).map_err(|e| e.to_string())
}

/// Searches `root`, calling `on_file` (from worker threads) for each file
/// with matches. Stops when `cancel` is set or `max_matches` is reached.
pub fn grep(
    root: &Path,
    pattern: &str,
    opts: &GrepOptions,
    cancel: &AtomicBool,
    on_file: &(dyn Fn(FileMatches) + Sync),
) -> Result<GrepStats, String> {
    let t0 = Instant::now();
    if pattern.is_empty() {
        return Ok(GrepStats::default());
    }
    let matcher = build_matcher(pattern, opts)?;

    let mut wb = ignore::WalkBuilder::new(root);
    wb.hidden(!opts.include_hidden)
        .git_ignore(opts.respect_gitignore)
        .git_global(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .ignore(opts.respect_gitignore)
        .parents(opts.respect_gitignore)
        .follow_links(false)
        .filter_entry({
            // Network shares inside the folder are not searched (see `remote_mounts`).
            let remote = super::index::remote_mounts();
            let everything = opts.everything;
            move |e| {
                !super::index::is_system_skip(e.path())
                    && (e.depth() == 0 || !remote.contains(e.path()))
                    && (everything || !super::ignorefile::skip_cache(e))
            }
        })
        .max_filesize(Some(opts.max_filesize))
        .threads(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(12),
        );
    super::ignorefile::configure(&mut wb, !opts.everything);
    let globs = |list: &[String]| -> Vec<String> {
        list.iter()
            .flat_map(|g| g.split(','))
            .map(|g| g.trim().to_string())
            .filter(|g| !g.is_empty())
            .collect()
    };
    let (inc, exc) = (globs(&opts.include), globs(&opts.exclude));
    let include = if inc.is_empty() {
        None
    } else {
        let mut ob = ignore::overrides::OverrideBuilder::new(root);
        for g in &inc {
            ob.add(g).map_err(|e| format!("include '{g}': {e}"))?;
        }
        Some(ob.build().map_err(|e| e.to_string())?)
    };
    if !exc.is_empty() {
        let mut ob = ignore::overrides::OverrideBuilder::new(root);
        for g in &exc {
            ob.add(&format!("!{g}"))
                .map_err(|e| format!("exclude '{g}': {e}"))?;
        }
        wb.overrides(ob.build().map_err(|e| e.to_string())?);
    }

    let files = AtomicUsize::new(0);
    let matches = AtomicUsize::new(0);
    let searched = AtomicUsize::new(0);
    let cloud_only = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);

    wb.build_parallel().run(|| {
        let matcher = matcher.clone();
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            // Must agree with the matcher's CRLF mode, or every search fails.
            .line_terminator(grep_matcher::LineTerminator::crlf())
            .line_number(true)
            .build();
        let (files, matches, searched, truncated) = (&files, &matches, &searched, &truncated);
        let cloud_only = &cloud_only;
        let include = &include;
        Box::new(move |res| {
            use ignore::WalkState;
            if cancel.load(Ordering::Relaxed) || truncated.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            let Ok(entry) = res else {
                return WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            // Apply includes to files only, rather than letting an override
            // whitelist them during traversal and bypass .gallignore.
            if include
                .as_ref()
                .is_some_and(|patterns| !patterns.matched(entry.path(), false).is_whitelist())
            {
                return WalkState::Continue;
            }
            if cfg!(any(windows, target_os = "macos"))
                && entry
                    .metadata()
                    .is_ok_and(|m| super::entry::is_cloud_only(&m))
            {
                cloud_only.fetch_add(1, Ordering::Relaxed);
                return WalkState::Continue;
            }
            searched.fetch_add(1, Ordering::Relaxed);
            let mut found: Vec<LineMatch> = Vec::new();
            let _ = searcher.search_path(
                &matcher,
                entry.path(),
                Lossy(|lnum, line| {
                    let line = line.trim_end_matches(['\n', '\r']);
                    let mut ranges = Vec::new();
                    let _ = matcher.find_iter(line.as_bytes(), |m| {
                        ranges.push((m.start(), m.end()));
                        ranges.len() < 50
                    });
                    if ranges.is_empty() {
                        return Ok(true);
                    }
                    found.push(shorten(line, lnum, &ranges));
                    let n = matches.fetch_add(1, Ordering::Relaxed) + 1;
                    if n >= opts.max_matches {
                        truncated.store(true, Ordering::Relaxed);
                        return Ok(false);
                    }
                    Ok(!cancel.load(Ordering::Relaxed))
                }),
            );
            if !found.is_empty() {
                files.fetch_add(1, Ordering::Relaxed);
                on_file(FileMatches {
                    path: entry.path().to_string_lossy().into_owned(),
                    matches: found,
                });
            }
            WalkState::Continue
        })
    });

    Ok(GrepStats {
        files: files.into_inner(),
        matches: matches.into_inner(),
        searched: searched.into_inner(),
        cloud_only: cloud_only.into_inner(),
        truncated: truncated.into_inner(),
        elapsed_ms: t0.elapsed().as_millis() as u64,
    })
}

/// Builds a [`LineMatch`], cutting very long lines to a window around the
/// first match and converting byte ranges to UTF-16 offsets.
fn shorten(line: &str, lnum: u64, ranges: &[(usize, usize)]) -> LineMatch {
    let (mut from, mut to) = (0, line.len());
    let mut prefix = "";
    if line.len() > MAX_LINE {
        let first = ranges[0].0;
        from = floor_char(line, first.saturating_sub(80));
        to = floor_char(line, (from + MAX_LINE).min(line.len()));
        if from > 0 {
            prefix = "…";
        }
    }
    // Leading indentation is noise in a result list; drop it.
    let body = &line[from..to];
    let trimmed = body.trim_start();
    let from = from + (body.len() - trimmed.len());
    let text = format!("{prefix}{}", &line[from..to]);
    let base = prefix.encode_utf16().count() as u32;
    let u16_at = |b: usize| base + line[from..b.clamp(from, to)].encode_utf16().count() as u32;
    let ranges = ranges
        .iter()
        .filter(|(s, e)| *e > from && *s < to)
        // A regex match can start or end inside a letter (`x*` matches empty between the
        // bytes of `č`); widen it to whole letters.
        .map(|&(s, e)| (u16_at(floor_char(line, s)), u16_at(ceil_char(line, e))))
        .collect();
    LineMatch {
        line: lnum,
        text,
        ranges,
    }
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Lines `[line - before, line + after]` (1-based) of a text file, for the
/// result preview. Long lines are truncated.
pub fn read_context(
    path: &Path,
    line: u64,
    before: u64,
    after: u64,
) -> std::io::Result<Vec<(u64, String)>> {
    use std::io::{BufRead, BufReader};
    let f = super::open_regular(path)?;
    let first = line.saturating_sub(before).max(1);
    let last = line + after;
    let mut out = Vec::new();
    let mut buf = Vec::new();
    let mut r = BufReader::new(f);
    let mut n = 0u64;
    loop {
        buf.clear();
        if r.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        n += 1;
        if n < first {
            continue;
        }
        if n > last {
            break;
        }
        let mut s = String::from_utf8_lossy(&buf)
            .trim_end_matches(['\n', '\r'])
            .to_string();
        if s.len() > 1000 {
            s.truncate(floor_char(&s, 1000));
            s.push('…');
        }
        out.push((n, s));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn run(root: &Path, pat: &str, opts: GrepOptions) -> (Vec<FileMatches>, GrepStats) {
        let out = Mutex::new(vec![]);
        let st = grep(root, pat, &opts, &AtomicBool::new(false), &|f| {
            out.lock().unwrap().push(f)
        })
        .unwrap();
        let mut v = out.into_inner().unwrap();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        (v, st)
    }

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        std::fs::create_dir_all(p.join("src")).unwrap();
        std::fs::write(
            p.join("src/a.rs"),
            "fn main() {\n    let Foo = 1;\n    foo(); foo();\n}\n",
        )
        .unwrap();
        std::fs::write(p.join("notes.txt"), "Foobar\nfood\n").unwrap();
        std::fs::write(p.join("bin.dat"), b"foo\x00\x01\x02").unwrap();
        d
    }

    #[test]
    fn smart_case_and_ranges() {
        let d = tree();
        let (files, st) = run(d.path(), "foo", GrepOptions::default());
        assert_eq!(files.len(), 2, "binary file skipped");
        assert_eq!(st.files, 2);
        let a = files.iter().find(|f| f.path.ends_with("a.rs")).unwrap();
        assert_eq!(a.matches.len(), 2);
        // Indentation trimmed; ranges relative to trimmed text.
        assert_eq!(a.matches[1].text, "foo(); foo();");
        assert_eq!(a.matches[1].ranges, vec![(0, 3), (7, 10)]);
        assert_eq!(a.matches[1].line, 3);

        let (files, _) = run(d.path(), "Foo", GrepOptions::default());
        assert_eq!(files.iter().map(|f| f.matches.len()).sum::<usize>(), 2);
    }

    #[test]
    fn word_regex_and_globs() {
        let d = tree();
        let o = GrepOptions {
            whole_word: true,
            ..Default::default()
        };
        let (files, _) = run(d.path(), "foo", o);
        assert_eq!(files.len(), 1); // "Foobar"/"food" are not whole words

        let o = GrepOptions {
            regex: true,
            ..Default::default()
        };
        let (files, _) = run(d.path(), r"fo+d$", o);
        assert_eq!(files.len(), 1);

        let o = GrepOptions {
            include: vec!["*.rs".into()],
            ..Default::default()
        };
        assert!(run(d.path(), "foo", o)
            .0
            .iter()
            .all(|f| f.path.ends_with(".rs")));
        let o = GrepOptions {
            exclude: vec!["src/".into()],
            ..Default::default()
        };
        assert!(run(d.path(), "foo", o)
            .0
            .iter()
            .all(|f| !f.path.contains("/src/")));
        // Literal mode treats regex characters literally.
        assert!(run(d.path(), "main()", GrepOptions::default()).0.len() == 1);
        assert!(grep(
            d.path(),
            "(",
            &GrepOptions {
                regex: true,
                ..Default::default()
            },
            &AtomicBool::new(false),
            &|_| {}
        )
        .is_err());
    }

    #[test]
    fn include_globs_do_not_resurface_gallignored_files() {
        let d = tree();
        let secret = d.path().join("src/secret.rs");
        std::fs::write(&secret, "foo\n").unwrap();
        std::fs::write(d.path().join("src/visible.rs"), "foo\n").unwrap();
        super::super::ignorefile::hide_from_search(&secret).unwrap();

        let opts = GrepOptions {
            include: vec!["*.rs".into()],
            ..Default::default()
        };
        let (files, _) = run(d.path(), "foo", opts.clone());
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| !f.path.ends_with("secret.rs")));
        assert!(files.iter().all(|f| f.path.ends_with(".rs")));

        let opts = GrepOptions {
            everything: true,
            ..opts
        };
        let (files, _) = run(d.path(), "foo", opts);
        assert!(files.iter().any(|f| f.path.ends_with("secret.rs")));
        assert!(files.iter().all(|f| f.path.ends_with(".rs")));
    }

    /// Files saved on Windows end their lines with CRLF; `$` still matches at the end.
    #[test]
    fn regex_end_anchor_matches_crlf_lines() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("win.txt"), "food\r\nbar\r\n").unwrap();
        let o = GrepOptions {
            regex: true,
            ..Default::default()
        };
        let (files, _) = run(d.path(), r"fo+d$", o.clone());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].matches[0].text, "food");
        assert_eq!(files[0].matches[0].ranges, vec![(0, 4)]);
        let (files, _) = run(d.path(), r"^bar$", o);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].matches[0].line, 2);
    }

    /// Regex matches may start or end inside a multi-byte letter (empty matches of `x*`,
    /// byte classes); the search still finishes and highlights whole letters.
    #[test]
    fn matches_inside_a_letter_do_not_crash() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("u.txt"), "čšž abc\n").unwrap();
        let o = GrepOptions {
            regex: true,
            ..Default::default()
        };
        let (files, _) = run(d.path(), "x*", o.clone());
        assert_eq!(files.len(), 1);
        let (files, _) = run(d.path(), r"(?-u:\xC4)", o);
        assert_eq!(files[0].matches[0].ranges, vec![(0, 1)], "č highlighted");
    }

    #[test]
    fn max_matches_truncates() {
        let d = tree();
        let o = GrepOptions {
            max_matches: 1,
            ..Default::default()
        };
        let (_, st) = run(d.path(), "foo", o);
        assert!(st.truncated);
    }

    #[test]
    fn long_lines_and_unicode() {
        let line = format!("{}čšž needle", "x".repeat(500));
        let m = shorten(&line, 1, &[(line.find("needle").unwrap(), line.len())]);
        assert!(m.text.starts_with('…'));
        let (s, e) = m.ranges[0];
        let u: Vec<u16> = m.text.encode_utf16().collect();
        assert_eq!(
            String::from_utf16(&u[s as usize..e as usize]).unwrap(),
            "needle"
        );
    }

    #[test]
    fn context_lines() {
        let d = tree();
        let c = read_context(&d.path().join("src/a.rs"), 2, 1, 1).unwrap();
        assert_eq!(c.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 2, 3]);
    }
}
