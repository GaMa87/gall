//! Per-project search exclusions: `.gallignore`.
//!
//! A `.gallignore` file uses gitignore syntax and applies to the folder it is in
//! and everything below it, like a `.gitignore`, but it also works outside a git
//! repository and does not touch the project's git setup. Quick Open, find in
//! files and the name search (F3) all honour it, also when the search starts in
//! a subfolder of the project. Folders tagged as caches (`CACHEDIR.TAG`, e.g.
//! Cargo's `target`) are skipped the same way.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

pub const GALL_IGNORE: &str = ".gallignore";
const HEADER: &str =
    "# Hidden from Gall's search (Quick Open, find in files, F3). Gitignore syntax.";

/// Applies the walker settings every search shares. With `enabled`, `.gallignore`
/// files are read (always, even with gitignore support turned off: they are
/// Gall's own); off is the "search everything" mode. Ignore files in the folders
/// above the search root are read too, so a project's rules also hold when a
/// search starts in one of its subfolders.
pub fn configure(wb: &mut ignore::WalkBuilder, enabled: bool) {
    if enabled {
        wb.add_custom_ignore_filename(GALL_IGNORE);
    }
    wb.parents(true);
}

/// A folder tagged as a cache by the Cache Directory Tagging Specification
/// (<https://bford.info/cachedir/>): Cargo's `target`, ccache, many tool caches.
/// Its contents are regenerated, so searches skip it like build output.
pub fn is_cache_dir(dir: &Path) -> bool {
    const SIG: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
    let tag = dir.join("CACHEDIR.TAG");
    // Asked for every folder a search walks, and almost never there: a cheap existence
    // check first (opening a file costs a handle on Windows), the read only when it is.
    if !exists_cheap(&tag) {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(&tag) else {
        return false;
    };
    let mut buf = [0u8; SIG.len()];
    f.read_exact(&mut buf).is_ok() && buf == SIG
}

/// Whether `p` exists, with one attribute query and no open handle.
fn exists_cheap(p: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFileAttributesW(name: *const u16) -> u32;
        }
        const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
        let w: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: a valid NUL-terminated wide path.
        unsafe { GetFileAttributesW(w.as_ptr()) != INVALID_FILE_ATTRIBUTES }
    }
    #[cfg(not(windows))]
    {
        std::fs::symlink_metadata(p).is_ok()
    }
}

/// Whether a walker should skip this entry because it is a tagged cache folder
/// (never the folder the search starts in).
pub fn skip_cache(e: &ignore::DirEntry) -> bool {
    e.depth() > 0 && e.file_type().is_some_and(|t| t.is_dir()) && is_cache_dir(e.path())
}

/// Escapes a file name so it matches literally as one gitignore pattern.
fn escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    for (i, c) in name.chars().enumerate() {
        match c {
            '*' | '?' | '[' | ']' | '\\' => out.push('\\'),
            '#' | '!' if i == 0 => out.push('\\'),
            _ => {}
        }
        out.push(c);
    }
    // Trailing spaces are dropped by gitignore unless escaped.
    let trimmed = out.trim_end_matches(' ').len();
    if trimmed < out.len() {
        let spaces = out.len() - trimmed;
        out.truncate(trimmed);
        for _ in 0..spaces {
            out.push_str("\\ ");
        }
    }
    out
}

fn invalid(m: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, m.to_string())
}

/// The `.gallignore` next to `path` and the line that names exactly `path`.
fn own_line(path: &Path) -> std::io::Result<(PathBuf, String, bool)> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| invalid("this item cannot be hidden from search"))?;
    if name.contains(['\n', '\r']) {
        return Err(invalid(
            "a name with a line break cannot be hidden from search",
        ));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| invalid("a drive or root folder cannot be hidden from search"))?;
    let is_dir = std::fs::symlink_metadata(path)?.is_dir();
    let line = format!("/{}{}", escape(name), if is_dir { "/" } else { "" });
    Ok((parent.join(GALL_IGNORE), line, is_dir))
}

fn read_or_empty(file: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(file) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

fn append_line(file: &Path, existing: &str, line: &str) -> std::io::Result<()> {
    let mut text = String::new();
    if existing.is_empty() {
        text.push_str(HEADER);
        text.push('\n');
    } else if !existing.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?
        .write_all(text.as_bytes())?;
    note_written(file);
    Ok(())
}

/// Hides `path` from every search by adding it to the `.gallignore` next to it
/// (`/name/` for a folder, `/name` for a file). Returns the `.gallignore` that was
/// written, and whether a line was added (false when it was already there). A
/// `!` line that showed the item again is removed instead.
pub fn hide_from_search(path: &Path) -> std::io::Result<(PathBuf, bool)> {
    let (file, line, _) = own_line(path)?;
    let existing = read_or_empty(&file)?;
    let negated = format!("!{line}");
    if existing
        .lines()
        .any(|l| l.trim_end_matches('\r') == negated)
    {
        remove_lines(&file, &existing, &negated)?;
        return Ok((file, true));
    }
    if existing.lines().any(|l| l.trim_end_matches('\r') == line) {
        return Ok((file, false));
    }
    append_line(&file, &existing, &line)?;
    Ok((file, true))
}

/// Removes every line equal to `line`; deletes the file when only the header is left.
fn remove_lines(file: &Path, existing: &str, line: &str) -> std::io::Result<()> {
    let kept: Vec<&str> = existing
        .lines()
        .filter(|l| l.trim_end_matches('\r') != line)
        .collect();
    if kept.iter().all(|l| l.trim().is_empty() || *l == HEADER) {
        std::fs::remove_file(file)?;
    } else {
        let mut text = kept.join("\n");
        text.push('\n');
        std::fs::write(file, text)?;
    }
    note_written(file);
    Ok(())
}

/// What [`show_in_search`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ShowOutcome {
    /// Its own line was taken out of this `.gallignore`.
    Removed { file: String },
    /// A broader rule hides it; a `!` line in this `.gallignore` lets it through.
    Excepted { file: String },
    /// A folder above it is hidden; that folder has to be shown first.
    InsideHidden { folder: String },
    /// No `.gallignore` hides it.
    NotHidden,
}

/// Undoes [`hide_from_search`], or makes an exception for an item a pattern hides.
pub fn show_in_search(path: &Path) -> std::io::Result<ShowOutcome> {
    let (file, line, is_dir) = own_line(path)?;
    let parent = path.parent().unwrap_or(path);
    let rules = Rules::for_dir(parent);
    if let Some((folder, _)) = rules.hidden_ancestor() {
        return Ok(ShowOutcome::InsideHidden {
            folder: folder.to_string_lossy().into_owned(),
        });
    }
    if rules.hidden_by(0, path, is_dir).is_none() {
        return Ok(ShowOutcome::NotHidden);
    }
    let existing = read_or_empty(&file)?;
    if existing.lines().any(|l| l.trim_end_matches('\r') == line) {
        remove_lines(&file, &existing, &line)?;
        if Rules::for_dir(parent).hidden_by(0, path, is_dir).is_none() {
            return Ok(ShowOutcome::Removed {
                file: file.to_string_lossy().into_owned(),
            });
        }
    }
    let existing = read_or_empty(&file)?;
    append_line(&file, &existing, &format!("!{line}"))?;
    Ok(ShowOutcome::Excepted {
        file: file.to_string_lossy().into_owned(),
    })
}

/// The `.gallignore` rules that apply in one folder: its own file and those of
/// every folder above it, nearest first (a nearer file overrides a farther one,
/// as the search walkers apply them).
pub struct Rules {
    chain: Vec<(PathBuf, Option<Gitignore>)>,
}

/// The rule that hides an item: the `.gallignore` and its line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HiddenBy {
    pub file: String,
    pub line: String,
}

impl Rules {
    pub fn for_dir(dir: &Path) -> Self {
        let chain = dir
            .ancestors()
            .filter(|d| !d.as_os_str().is_empty())
            .map(|d| {
                let file = d.join(GALL_IGNORE);
                let gi = file.is_file().then(|| {
                    let mut b = GitignoreBuilder::new(d);
                    b.add(&file);
                    b.build().ok()
                });
                (d.to_path_buf(), gi.flatten())
            })
            .collect();
        Rules { chain }
    }

    /// Whether `path` (an item in `chain[level]`) is hidden by the rules from that
    /// folder upwards.
    fn hidden_by(&self, level: usize, path: &Path, is_dir: bool) -> Option<HiddenBy> {
        for (dir, gi) in self.chain.iter().skip(level) {
            let Some(gi) = gi else { continue };
            match gi.matched(path, is_dir) {
                Match::Ignore(g) => {
                    return Some(HiddenBy {
                        file: g
                            .from()
                            .map(Path::to_path_buf)
                            .unwrap_or_else(|| dir.join(GALL_IGNORE))
                            .to_string_lossy()
                            .into_owned(),
                        line: g.original().to_string(),
                    })
                }
                Match::Whitelist(_) => return None,
                Match::None => {}
            }
        }
        None
    }

    /// The folder itself or one above it that a `.gallignore` hides, if any.
    pub fn hidden_ancestor(&self) -> Option<(PathBuf, HiddenBy)> {
        (0..self.chain.len().saturating_sub(1)).find_map(|i| {
            let d = &self.chain[i].0;
            self.hidden_by(i + 1, d, true).map(|h| (d.clone(), h))
        })
    }
}

/// Which items of a folder `.gallignore` files hide, for dimming them in the list.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HiddenState {
    /// Set when the folder itself (or one above it) is hidden: everything in it is.
    pub folder: Option<String>,
    pub folder_rule: Option<HiddenBy>,
    /// Hidden items by name.
    pub entries: HashMap<String, HiddenBy>,
}

pub fn hidden_state(dir: &Path) -> HiddenState {
    let rules = Rules::for_dir(dir);
    // Remembered, so a later edit or removal is noticed. Only files that exist or
    // existed: one per folder ever listed would grow without end. One that appears in
    // a folder open in Gall is caught by the watcher (`gallignore_changed_in`).
    for (d, gi) in &rules.chain {
        let file = d.join(GALL_IGNORE);
        if gi.is_some() || is_known(&file) {
            note_seen(&file);
        }
    }
    if let Some((folder, rule)) = rules.hidden_ancestor() {
        return HiddenState {
            folder: Some(folder.to_string_lossy().into_owned()),
            folder_rule: Some(rule),
            entries: HashMap::new(),
        };
    }
    let mut st = HiddenState::default();
    if rules.chain.iter().all(|(_, g)| g.is_none()) {
        return st;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return st;
    };
    for e in rd.flatten() {
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        if let Some(h) = rules.hidden_by(0, &e.path(), is_dir) {
            st.entries
                .insert(e.file_name().to_string_lossy().into_owned(), h);
        }
    }
    st
}

// ── noticing edits, so the Quick Open index follows a hand-edited .gallignore ──

static SEEN: Mutex<Option<HashMap<PathBuf, Option<SystemTime>>>> = Mutex::new(None);

fn mtime(file: &Path) -> Option<SystemTime> {
    std::fs::metadata(file).and_then(|m| m.modified()).ok()
}

/// Records the current state of a `.gallignore`; true when it changed since the
/// last time it was seen (not on the first sighting).
fn observe(file: &Path) -> bool {
    let now = mtime(file);
    let mut g = SEEN.lock().unwrap();
    let map = g.get_or_insert_with(HashMap::new);
    match map.insert(file.to_path_buf(), now) {
        Some(before) => before != now,
        None => false,
    }
}

fn is_known(file: &Path) -> bool {
    SEEN.lock()
        .unwrap()
        .as_ref()
        .is_some_and(|m| m.contains_key(file))
}

static CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn note_seen(file: &Path) {
    if observe(file) {
        CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// True once after [`hidden_state`] found a `.gallignore` that changed since it
/// was last seen (edited while its folder was not open in Gall).
pub fn take_changed() -> bool {
    CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// Gall wrote this file itself and already handled the change.
fn note_written(file: &Path) {
    observe(file);
}

/// Call when `dir` changed on disk: true when its `.gallignore` was created,
/// edited or deleted since Gall last looked, so the index should be rebuilt.
pub fn gallignore_changed_in(dir: &Path) -> bool {
    let file = dir.join(GALL_IGNORE);
    if is_known(&file) {
        observe(&file)
    } else {
        // First sighting: a file that appeared just now is news. A folder without one
        // is not remembered (see `hidden_state`).
        let fresh = mtime(&file).is_some_and(|t| t.elapsed().is_ok_and(|e| e.as_secs() < 10));
        if mtime(&file).is_some() {
            observe(&file);
        }
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn walk(start: &Path) -> Vec<String> {
        let mut wb = ignore::WalkBuilder::new(start);
        wb.hidden(true)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .ignore(false);
        configure(&mut wb, true);
        let mut v: Vec<String> = wb
            .build()
            .filter_map(|e| e.ok())
            .filter(|e| e.depth() > 0)
            .map(|e| {
                e.path()
                    .strip_prefix(start)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn hides_folders_and_files_also_outside_git() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        fs::create_dir_all(p.join("out/gen")).unwrap();
        fs::create_dir_all(p.join("src")).unwrap();
        fs::write(p.join("out/gen/a.js"), "").unwrap();
        fs::write(p.join("src/a.ts"), "").unwrap();
        fs::write(p.join("bundle.min.js"), "").unwrap();

        let (file, added) = hide_from_search(&p.join("out")).unwrap();
        assert!(added);
        assert_eq!(file, p.join(GALL_IGNORE));
        assert!(hide_from_search(&p.join("bundle.min.js")).unwrap().1);
        // Twice is a no-op.
        assert!(!hide_from_search(&p.join("out")).unwrap().1);
        let text = fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().filter(|l| *l == "/out/").count(), 1);
        assert!(text.lines().any(|l| l == "/bundle.min.js"));

        assert_eq!(walk(p), vec!["src", "src/a.ts"]);
    }

    #[test]
    fn rules_of_the_project_hold_in_a_subfolder() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        fs::create_dir_all(p.join("app/dist")).unwrap();
        fs::write(p.join("app/dist/x.js"), "").unwrap();
        fs::write(p.join("app/main.rs"), "").unwrap();
        fs::write(p.join(GALL_IGNORE), "dist/\n").unwrap();
        assert_eq!(walk(&p.join("app")), vec!["main.rs"]);
    }

    #[test]
    fn odd_names_match_literally() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        #[cfg(not(windows))]
        let names = ["#notes", "!bang", "a[1]*.txt", "x"];
        #[cfg(windows)]
        let names = ["#notes", "!bang", "a[1].txt", "x"];
        for n in names {
            fs::write(p.join(n), "").unwrap();
            if n != "x" {
                assert!(hide_from_search(&p.join(n)).unwrap().1, "{n}");
            }
        }
        assert_eq!(walk(p), vec!["x"]);
    }

    #[test]
    fn keeps_existing_lines_and_adds_a_newline() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        fs::create_dir(p.join("cache")).unwrap();
        fs::write(p.join(GALL_IGNORE), "*.log").unwrap();
        hide_from_search(&p.join("cache")).unwrap();
        assert_eq!(
            fs::read_to_string(p.join(GALL_IGNORE)).unwrap(),
            "*.log\n/cache/\n"
        );
    }

    #[test]
    fn root_cannot_be_hidden() {
        #[cfg(unix)]
        assert!(hide_from_search(Path::new("/")).is_err());
        #[cfg(windows)]
        assert!(hide_from_search(Path::new("C:\\")).is_err());
    }

    #[test]
    fn show_again_undoes_hide_or_makes_an_exception() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        fs::create_dir_all(p.join("out/keep")).unwrap();
        fs::write(p.join("a.min.js"), "").unwrap();
        fs::write(p.join("b.min.js"), "").unwrap();

        // Own line: removed, and the file goes when only the header is left.
        hide_from_search(&p.join("out")).unwrap();
        assert_eq!(hidden_state(p).entries.len(), 1);
        assert_eq!(
            show_in_search(&p.join("out")).unwrap(),
            ShowOutcome::Removed {
                file: p.join(GALL_IGNORE).to_string_lossy().into_owned()
            }
        );
        assert!(!p.join(GALL_IGNORE).exists());
        assert_eq!(
            show_in_search(&p.join("out")).unwrap(),
            ShowOutcome::NotHidden
        );

        // Hidden by a pattern: a ! line lets this one through, the other stays hidden.
        fs::write(p.join(GALL_IGNORE), "*.min.js\n").unwrap();
        assert!(matches!(
            show_in_search(&p.join("a.min.js")).unwrap(),
            ShowOutcome::Excepted { .. }
        ));
        let st = hidden_state(p);
        assert!(!st.entries.contains_key("a.min.js"));
        assert_eq!(st.entries["b.min.js"].line, "*.min.js");
        assert_eq!(walk(p), vec!["a.min.js", "out", "out/keep"]);
        // Hiding it again drops the exception instead of adding a second line.
        hide_from_search(&p.join("a.min.js")).unwrap();
        assert_eq!(
            fs::read_to_string(p.join(GALL_IGNORE)).unwrap(),
            "*.min.js\n"
        );

        // Inside a hidden folder: that folder has to be shown first.
        hide_from_search(&p.join("out")).unwrap();
        assert_eq!(
            show_in_search(&p.join("out/keep")).unwrap(),
            ShowOutcome::InsideHidden {
                folder: p.join("out").to_string_lossy().into_owned()
            }
        );
        let st = hidden_state(&p.join("out/keep"));
        assert_eq!(
            st.folder,
            Some(p.join("out").to_string_lossy().into_owned())
        );
        assert_eq!(st.folder_rule.unwrap().line, "/out/");
    }

    #[test]
    fn cache_folders_are_recognised() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        fs::create_dir_all(p.join("target/debug")).unwrap();
        fs::create_dir_all(p.join("notes")).unwrap();
        fs::write(
            p.join("target/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n# cargo\n",
        )
        .unwrap();
        fs::write(p.join("notes/CACHEDIR.TAG"), "just a file called so").unwrap();
        assert!(is_cache_dir(&p.join("target")));
        assert!(!is_cache_dir(&p.join("notes")));
        assert!(!is_cache_dir(p));
    }

    #[test]
    fn edits_to_a_gallignore_are_noticed_once() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        // Looked at while there is none (not remembered: that would grow per folder),
        // then one appears.
        hidden_state(p);
        assert!(!is_known(&p.join(GALL_IGNORE)));
        assert!(!gallignore_changed_in(p));
        fs::write(p.join(GALL_IGNORE), "x\n").unwrap();
        assert!(gallignore_changed_in(p));
        assert!(!gallignore_changed_in(p));
        // Gall's own writes are not reported again.
        fs::create_dir(p.join("d")).unwrap();
        hide_from_search(&p.join("d")).unwrap();
        assert!(!gallignore_changed_in(p));
        fs::remove_file(p.join(GALL_IGNORE)).unwrap();
        assert!(gallignore_changed_in(p));
    }
}
