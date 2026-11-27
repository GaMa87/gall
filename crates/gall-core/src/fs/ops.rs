//! File operations engine (O-02…O-09, O-15, O-16).
//!
//! * Quick operations (rename, mkdir, create file, trash) run synchronously.
//! * Copy / move / permanent delete run as background tasks with progress,
//!   cancellation and an explicit conflict policy.
//! * Every reversible operation is recorded in an undo journal (Ctrl+Z).

use crate::error::{CoreError, IoContext, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

// ───────────────────────────── names ─────────────────────────────

/// Validates a single path component typed by the user.
pub fn validate_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\0')
        || (cfg!(windows)
            && name
                .chars()
                .any(|c| "<>:\"\\|?*".contains(c) || (c as u32) < 32))
        || (cfg!(windows) && name.ends_with([' ', '.']))
        || (cfg!(windows) && windows_reserved(name));
    if bad {
        Err(CoreError::InvalidName(name.to_string()))
    } else {
        Ok(())
    }
}

fn files_equal(a: &Path, b: &Path) -> Result<bool> {
    let mut a_file = fs::File::open(a).ctx("read", a)?;
    let mut b_file = fs::File::open(b).ctx("read", b)?;
    let mut left = [0; 64 * 1024];
    let mut right = [0; 64 * 1024];
    loop {
        let a_len = a_file.read(&mut left).ctx("read", a)?;
        let b_len = b_file.read(&mut right).ctx("read", b)?;
        if a_len != b_len || left[..a_len] != right[..b_len] {
            return Ok(false);
        }
        if a_len == 0 {
            return Ok(true);
        }
    }
}

/// Device names Windows reserves in every folder, also with an extension (`NUL.txt`).
fn windows_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// Splits `name.ext` into (`name`, `.ext`); dot-files keep their name whole.
pub fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Returns a free path in `dir` for `name`, adding ` (2)`, ` (3)`, … before
/// the extension when needed.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    unique_path_avoiding(dir, name, &Default::default())
}

/// [`unique_path`] that also avoids `taken` (names chosen for other items of a batch).
fn unique_path_avoiding(
    dir: &Path,
    name: &str,
    taken: &std::collections::HashSet<PathBuf>,
) -> PathBuf {
    let free = |p: &PathBuf| !exists_no_follow(p) && !taken.contains(p);
    let first = dir.join(name);
    if free(&first) {
        return first;
    }
    // A folder has no extension: "my.project" → "my.project (2)".
    let (stem, ext) = if fs::metadata(&first).is_ok_and(|m| m.is_dir()) {
        (name, "")
    } else {
        split_ext(name)
    };
    // Continue an existing " (n)" suffix instead of nesting "(2) (2)", but only when it
    // is a counter, i.e. the name without it exists too ("Budget (2026).xlsx" alone is
    // a name, not the 2026th copy of "Budget.xlsx").
    let (base, start) = match stem.rfind(" (") {
        Some(i) if stem.ends_with(')') => match stem[i + 2..stem.len() - 1].parse::<u32>() {
            Ok(n) if !free(&dir.join(format!("{}{ext}", &stem[..i]))) => (&stem[..i], n + 1),
            _ => (stem, 2),
        },
        _ => (stem, 2),
    };
    // Names are at most 255 bytes (UTF-8 on Linux and macOS, fewer UTF-16 units on
    // Windows): a long name is shortened to make room for the counter.
    let name_n = |n: u32| {
        let tail = format!(" ({n}){ext}");
        let mut end = 255usize.saturating_sub(tail.len()).min(base.len());
        while !base.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}{tail}", &base[..end])
    };
    (start..)
        .map(|n| dir.join(name_n(n)))
        .find(free)
        .expect("unbounded range")
}

/// Where Ctrl+D puts the copies of `sources`: next to each, with a free " (n)" name.
/// The duplicate task picks the same names, so the UI can select them in advance.
pub fn duplicate_targets(sources: &[PathBuf]) -> Vec<PathBuf> {
    let mut taken = std::collections::HashSet::new();
    sources
        .iter()
        .map(|s| {
            let dir = s.parent().unwrap_or(Path::new(""));
            let name = s
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let t = unique_path_avoiding(dir, &name, &taken);
            taken.insert(t.clone());
            t
        })
        .collect()
}

/// Called after every move or rename (also by undo): colour tags and folder settings,
/// stored by path, follow the item. Set once at start-up by the app.
type MoveHook = Box<dyn Fn(&Path, &Path) + Send + Sync>;
static MOVE_HOOK: std::sync::OnceLock<MoveHook> = std::sync::OnceLock::new();

pub fn set_move_hook(f: impl Fn(&Path, &Path) + Send + Sync + 'static) {
    let _ = MOVE_HOOK.set(Box::new(f));
}

/// [`moved`] for other modules that rename (notes).
pub(crate) fn notify_moved(from: &Path, to: &Path) {
    moved(from, to);
}

fn moved(from: &Path, to: &Path) {
    if let Some(f) = MOVE_HOOK.get() {
        f(from, to);
    }
}

/// Lets go of what the app holds open inside a folder (its folder watchers) before the
/// folder is renamed, moved or deleted, and returns what takes it back afterwards. Windows
/// refuses those operations on a folder while anything inside it is open; on every OS,
/// taking the watchers back finds those that went away with the folder. Set once at
/// start-up by the app.
type ReleaseHook = Box<dyn Fn(&Path) -> Box<dyn FnOnce() + Send> + Send + Sync>;
static RELEASE_HOOK: std::sync::OnceLock<ReleaseHook> = std::sync::OnceLock::new();

pub fn set_release_hook(f: impl Fn(&Path) -> Box<dyn FnOnce() + Send> + Send + Sync + 'static) {
    let _ = RELEASE_HOOK.set(Box::new(f));
}

/// Held while `paths` are renamed, moved or deleted (see [`set_release_hook`]); dropping
/// it takes back what was let go.
struct Released(Vec<Box<dyn FnOnce() + Send>>);

impl Drop for Released {
    fn drop(&mut self) {
        for f in self.0.drain(..) {
            f();
        }
    }
}

fn release<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Released {
    Released(match RELEASE_HOOK.get() {
        Some(f) => paths.into_iter().map(f).collect(),
        None => Vec::new(),
    })
}

fn exists_no_follow(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// True when `a` and `b` name the same directory, also when one is reached through a
/// symlink, a bind mount or a different letter case. `dest/name` is then the source
/// itself, and replacing it would delete the source.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// True when `src` already lies directly in `dest_dir` (see [`same_dir`]).
fn in_dir(src: &Path, dest_dir: &Path) -> bool {
    src.parent().is_some_and(|p| same_dir(p, dest_dir))
}

fn file_name(p: &Path) -> Result<&str> {
    p.file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| CoreError::InvalidName(p.to_string_lossy().into_owned()))
}

// ───────────────────────────── undo journal ─────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UndoRecord {
    /// Items were created (copy, mkdir, new file); undo trashes them.
    Created { paths: Vec<PathBuf> },
    /// Items were moved or renamed; undo moves each `to` back to `from`.
    Moved { pairs: Vec<(PathBuf, PathBuf)> },
    /// Items were trashed between `at` and `until` (epoch milliseconds); undo restores
    /// them. The trash stamps each item when it reaches it, so a large delete spans seconds.
    Trashed {
        paths: Vec<PathBuf>,
        at: i64,
        until: i64,
    },
    /// Empty folders removed (a move that merged their contents elsewhere); undo
    /// creates them again.
    RemovedDirs { dirs: Vec<PathBuf> },
    /// Several steps done as one operation (folder sync); undone in reverse order.
    Batch {
        label: String,
        records: Vec<UndoRecord>,
        /// Folders the operation changed inside (merged folders): per-panel undo in their
        /// parent folder finds it too.
        #[serde(skip)]
        dirs: Vec<PathBuf>,
    },
}

impl UndoRecord {
    /// True when the operation created, removed or moved something directly in `dir`.
    pub fn touches(&self, dir: &Path) -> bool {
        let in_dir = |p: &PathBuf| p.parent() == Some(dir);
        match self {
            UndoRecord::Created { paths } | UndoRecord::Trashed { paths, .. } => {
                paths.iter().any(in_dir)
            }
            UndoRecord::RemovedDirs { dirs } => dirs.iter().any(in_dir),
            UndoRecord::Moved { pairs } => pairs.iter().any(|(f, t)| in_dir(f) || in_dir(t)),
            UndoRecord::Batch { records, dirs, .. } => {
                records.iter().any(|r| r.touches(dir)) || dirs.iter().any(in_dir)
            }
        }
    }

    fn affected_paths(&self, out: &mut Vec<PathBuf>) {
        match self {
            UndoRecord::Created { paths } | UndoRecord::Trashed { paths, .. } => {
                out.extend(paths.iter().cloned());
            }
            UndoRecord::RemovedDirs { dirs } => out.extend(dirs.iter().cloned()),
            UndoRecord::Moved { pairs } => {
                for (from, to) in pairs {
                    out.push(from.clone());
                    out.push(to.clone());
                }
            }
            UndoRecord::Batch { records, dirs, .. } => {
                for r in records {
                    r.affected_paths(out);
                }
                out.extend(dirs.iter().cloned());
            }
        }
    }

    /// True when two operations touch the same path or an ancestor/descendant pair,
    /// meaning reversing the older one out of order could corrupt or miss newer changes.
    fn overlaps(&self, other: &UndoRecord) -> bool {
        let mut a = Vec::new();
        let mut b = Vec::new();
        self.affected_paths(&mut a);
        other.affected_paths(&mut b);
        a.iter().any(|pa| {
            b.iter()
                .any(|pb| same_path_ci(pa, pb) || starts_with_ci(pa, pb) || starts_with_ci(pb, pa))
        })
    }

    /// What the operation did, for "Undo: …" tooltips: `add “a.txt”`, `rename “a” to “b”`.
    pub fn describe(&self) -> String {
        match self {
            UndoRecord::Created { paths } => format!("add {}", what(paths)),
            UndoRecord::Moved { pairs } => match pairs.as_slice() {
                [(f, t)] if f.parent() == t.parent() => {
                    format!("rename {} to {}", quoted(f), quoted(t))
                }
                [(f, t)] => format!("move {} to {}", quoted(f), folder(t)),
                _ if pairs.iter().all(|(f, t)| f.parent() == t.parent()) => {
                    format!("rename {}", items(pairs.len()))
                }
                _ => format!("move {} to {}", items(pairs.len()), folder(&pairs[0].1)),
            },
            UndoRecord::Trashed { paths, .. } => format!("delete {}", what(paths)),
            UndoRecord::RemovedDirs { dirs } => format!("remove {}", what(dirs)),
            UndoRecord::Batch { label, .. } => label.clone(),
        }
    }

    /// What undoing it did, shown after the undo: `Removed “a.txt” (in the trash)`.
    pub fn undone(&self) -> String {
        match self {
            UndoRecord::Created { paths } => format!("Removed {} (in the trash)", what(paths)),
            UndoRecord::Moved { pairs } => match pairs.as_slice() {
                [(f, t)] if f.parent() == t.parent() => {
                    format!("Renamed {} back to {}", quoted(t), quoted(f))
                }
                [(f, t)] => format!("Moved {} back to {}", quoted(t), folder(f)),
                _ if pairs.iter().all(|(f, t)| f.parent() == t.parent()) => {
                    format!("Renamed {} back", items(pairs.len()))
                }
                _ => format!(
                    "Moved {} back to {}",
                    items(pairs.len()),
                    folder(&pairs[0].0)
                ),
            },
            UndoRecord::Trashed { paths, .. } => format!("Restored {}", what(paths)),
            UndoRecord::RemovedDirs { dirs } => format!("Created {} again", what(dirs)),
            UndoRecord::Batch { label, .. } => format!("Undid {label}"),
        }
    }
}

/// "1 item", "3 items".
pub fn items(n: usize) -> String {
    format!("{n} item{}", if n == 1 { "" } else { "s" })
}

/// `“name”` of a path.
fn quoted(p: &Path) -> String {
    format!(
        "“{}”",
        p.file_name()
            .map_or_else(|| p.to_string_lossy(), |n| n.to_string_lossy())
    )
}

/// Name of the folder that contains `p`.
fn folder(p: &Path) -> String {
    p.parent().map_or_else(String::new, |d| {
        d.file_name().map_or_else(
            || d.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    })
}

/// The single item's quoted name, else "N items".
fn what(paths: &[PathBuf]) -> String {
    match paths {
        [one] => quoted(one),
        _ => items(paths.len()),
    }
}

struct UndoEntry {
    id: u64,
    record: UndoRecord,
}

pub struct UndoJournal {
    next_id: AtomicU64,
    stack: Mutex<Vec<UndoEntry>>,
}

impl Default for UndoJournal {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            stack: Mutex::new(Vec::new()),
        }
    }
}

const UNDO_LIMIT: usize = 100;

#[allow(clippy::result_large_err)] // rare path; the record is handed back by value
fn revert_moved(
    pairs: Vec<(PathBuf, PathBuf)>,
    rename_op: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
    remove_source: &dyn Fn(&Path) -> Result<()>,
) -> std::result::Result<(), (CoreError, Option<UndoRecord>)> {
    if pairs.iter().all(|(from, to)| from.parent() == to.parent()) {
        for (from, to) in &pairs {
            let freed = pairs.iter().any(|(_, target)| target == from) || same_entry(from, to);
            if exists_no_follow(from) && !freed {
                return Err((
                    CoreError::AlreadyExists(from.clone()),
                    Some(UndoRecord::Moved { pairs }),
                ));
            }
        }
        let back: Vec<_> = pairs
            .iter()
            .map(|(from, to)| (to.clone(), from.clone()))
            .collect();
        return rename_many(&back).map_err(|error| {
            (
                error,
                Some(UndoRecord::Moved {
                    pairs: pairs.clone(),
                }),
            )
        });
    }

    if let Some((from, _)) = pairs.iter().find(|(from, _)| exists_no_follow(from)) {
        return Err((
            CoreError::AlreadyExists(from.clone()),
            Some(UndoRecord::Moved { pairs }),
        ));
    }
    for k in (0..pairs.len()).rev() {
        let (from, to) = &pairs[k];
        if let Some(parent) = from.parent() {
            if let Err(error) = fs::create_dir_all(parent).ctx("create folder", parent) {
                return Err((
                    error,
                    Some(UndoRecord::Moved {
                        pairs: pairs[..=k].to_vec(),
                    }),
                ));
            }
        }
        if let Err(failure) = move_one_with_ops(
            to,
            from,
            &AtomicBool::new(false),
            &mut |_| {},
            rename_op,
            remove_source,
        ) {
            let left = match failure.recovered_source {
                Some(true) => {
                    let mut records = vec![UndoRecord::Created {
                        paths: vec![to.clone()],
                    }];
                    if k > 0 {
                        records.push(UndoRecord::Moved {
                            pairs: pairs[..k].to_vec(),
                        });
                    }
                    UndoRecord::Batch {
                        label: "recovered move copy".into(),
                        records,
                        dirs: Vec::new(),
                    }
                }
                Some(false) | None => UndoRecord::Moved {
                    pairs: pairs[..=k].to_vec(),
                },
            };
            return Err((failure.error, Some(left)));
        }
    }
    Ok(())
}

impl UndoJournal {
    pub fn push(&self, r: UndoRecord) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut s = self.stack.lock().unwrap();
        s.push(UndoEntry { id, record: r });
        if s.len() > UNDO_LIMIT {
            s.remove(0);
        }
        id
    }

    /// Number of steps that can be undone.
    pub fn len(&self) -> usize {
        self.stack.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Stable ID of the most recently recorded step, if any.
    pub fn latest_id(&self) -> Option<u64> {
        self.stack.lock().unwrap().last().map(|e| e.id)
    }

    /// True while the entry with `id` is still present in the journal.
    pub fn has_id(&self, id: u64) -> bool {
        self.stack.lock().unwrap().iter().any(|e| e.id == id)
    }

    pub fn peek_description(&self) -> Option<String> {
        self.stack
            .lock()
            .unwrap()
            .last()
            .map(|e| e.record.describe())
    }

    /// Description of the most recent operation that touched `dir` (per-panel undo).
    pub fn peek_in(&self, dir: &Path) -> Option<String> {
        self.stack
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|e| e.record.touches(dir))
            .map(|e| e.record.describe())
    }

    /// Reverts the most recent operation. Returns its description. When the undo fails,
    /// what was not undone stays on the stack, so Ctrl+Z can be tried again after the
    /// cause (a name taken in the meantime, a full trash) is fixed.
    pub fn undo(&self) -> Result<String> {
        self.undo_with(false)
    }

    /// [`undo`](Self::undo); `permanent`: items the operation created are deleted for good
    /// instead of trashed. Offered after the trash refused them (a network drive) with an
    /// error starting `no-trash:`, which would otherwise block Ctrl+Z for good.
    pub fn undo_with(&self, permanent: bool) -> Result<String> {
        self.undo_with_reverter(permanent, Self::revert)
    }

    fn undo_with_reverter(
        &self,
        permanent: bool,
        revert: impl FnOnce(
            UndoRecord,
            bool,
        ) -> std::result::Result<String, (CoreError, Option<UndoRecord>)>,
    ) -> Result<String> {
        let (at, entry) = {
            let mut s = self.stack.lock().unwrap();
            let at = s.len().checked_sub(1).ok_or(CoreError::NothingToUndo)?;
            (at, s.pop().expect("stack length checked"))
        };
        let entry_id = entry.id;
        revert(entry.record, permanent).map_err(|(e, left)| {
            if let Some(r) = left {
                let mut s = self.stack.lock().unwrap();
                let at = at.min(s.len());
                s.insert(
                    at,
                    UndoEntry {
                        id: entry_id,
                        record: r,
                    },
                );
                if s.len() > UNDO_LIMIT {
                    s.remove(0);
                }
            }
            e
        })
    }

    /// Reverts the most recent operation that touched `dir`, leaving newer
    /// operations elsewhere in place (see [`undo`](Self::undo) for failures).
    pub fn undo_in(&self, dir: &Path) -> Result<String> {
        self.undo_in_with(dir, false)
    }

    /// [`undo_in`](Self::undo_in); `permanent` as in [`undo_with`](Self::undo_with).
    pub fn undo_in_with(&self, dir: &Path, permanent: bool) -> Result<String> {
        let (i, entry) = {
            let mut s = self.stack.lock().unwrap();
            let i = s
                .iter()
                .rposition(|e| e.record.touches(dir))
                .ok_or(CoreError::NothingToUndo)?;
            (i, s.remove(i))
        };
        let entry_id = entry.id;
        Self::revert(entry.record, permanent).map_err(|(e, left)| {
            if let Some(r) = left {
                let mut s = self.stack.lock().unwrap();
                let at = i.min(s.len());
                s.insert(
                    at,
                    UndoEntry {
                        id: entry_id,
                        record: r,
                    },
                );
            }
            e
        })
    }

    /// Reverts the specific operation identified by `id` (used by notification Undo buttons).
    /// Returns [`CoreError::NothingToUndo`] when `id` is no longer in the journal, and refuses
    /// without changing files when a newer journal entry overlaps the same paths.
    pub fn undo_by_id(&self, id: u64) -> Result<String> {
        self.undo_by_id_with(id, false)
    }

    /// [`undo_by_id`](Self::undo_by_id); `permanent` as in [`undo_with`](Self::undo_with).
    pub fn undo_by_id_with(&self, id: u64, permanent: bool) -> Result<String> {
        self.undo_by_id_with_reverter(id, permanent, Self::revert)
    }

    fn undo_by_id_with_reverter(
        &self,
        id: u64,
        permanent: bool,
        revert: impl FnOnce(
            UndoRecord,
            bool,
        ) -> std::result::Result<String, (CoreError, Option<UndoRecord>)>,
    ) -> Result<String> {
        let (i, entry) = {
            let mut s = self.stack.lock().unwrap();
            let i = s
                .iter()
                .position(|e| e.id == id)
                .ok_or(CoreError::NothingToUndo)?;
            if s[i + 1..]
                .iter()
                .any(|newer| newer.record.overlaps(&s[i].record))
            {
                return Err(CoreError::Other(
                    "A newer operation changed the same files. Undo that first.".into(),
                ));
            }
            (i, s.remove(i))
        };
        let entry_id = entry.id;
        revert(entry.record, permanent).map_err(|(e, left)| {
            if let Some(r) = left {
                let mut s = self.stack.lock().unwrap();
                let at = i.min(s.len());
                s.insert(
                    at,
                    UndoEntry {
                        id: entry_id,
                        record: r,
                    },
                );
                if s.len() > UNDO_LIMIT {
                    s.remove(0);
                }
            }
            e
        })
    }

    /// Reverts `rec`; returns what was done, for the notification. On failure, the error
    /// and the part of `rec` that was not undone (None when nothing is left to undo).
    /// `permanent`: created items are deleted for good, not trashed.
    #[allow(clippy::result_large_err)] // rare path; the record is handed back by value
    fn revert(
        rec: UndoRecord,
        permanent: bool,
    ) -> std::result::Result<String, (CoreError, Option<UndoRecord>)> {
        let mut desc = rec.undone();
        match rec {
            UndoRecord::Created { paths } => {
                let existing: Vec<_> = paths.into_iter().filter(|p| exists_no_follow(p)).collect();
                if !existing.is_empty() {
                    let res = if permanent {
                        desc = format!("Deleted {} for good", what(&existing));
                        existing.iter().try_for_each(|p| remove_any(p))
                    } else {
                        trash_paths(&existing).map_err(|e| {
                            CoreError::Other(format!(
                                "no-trash: {} cannot be moved to the trash ({}).",
                                what(&existing),
                                e.to_string().trim_start_matches("trash: ")
                            ))
                        })
                    };
                    if let Err(e) = res {
                        let left: Vec<_> = existing
                            .into_iter()
                            .filter(|p| exists_no_follow(p))
                            .collect();
                        return Err((
                            e,
                            (!left.is_empty()).then_some(UndoRecord::Created { paths: left }),
                        ));
                    }
                }
            }
            UndoRecord::Moved { pairs } => {
                let rename_op = |a: &Path, b: &Path| fs::rename(a, b);
                revert_moved(pairs, &rename_op, &remove_any)?;
            }
            UndoRecord::RemovedDirs { dirs } => {
                for (k, d) in dirs.iter().enumerate() {
                    if let Err(e) = fs::create_dir_all(d).ctx("create folder", d) {
                        let left = dirs[k..].to_vec();
                        return Err((e, Some(UndoRecord::RemovedDirs { dirs: left })));
                    }
                }
            }
            UndoRecord::Trashed { paths, at, until } => {
                if let Err(e) = restore_from_trash(&paths, at, until) {
                    // Those back already are not looked for again.
                    let left: Vec<_> = paths
                        .iter()
                        .filter(|p| !exists_no_follow(p))
                        .cloned()
                        .collect();
                    let rec = (!left.is_empty()).then_some(UndoRecord::Trashed {
                        paths: left,
                        at,
                        until,
                    });
                    return Err((e, rec));
                }
            }
            UndoRecord::Batch {
                label,
                records,
                dirs,
            } => {
                // A sync copy that replaced a file must not go to the trash: the trash
                // would then hold two items with the same original path (the replaced
                // file and the copy) and refuse to restore either. Such copies only
                // duplicate the other side, so they are removed directly.
                let restoring: std::collections::HashSet<PathBuf> = records
                    .iter()
                    .filter_map(|r| match r {
                        UndoRecord::Trashed { paths, .. } => Some(paths.clone()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                let mut records = records;
                while let Some(r) = records.pop() {
                    let res = match r {
                        UndoRecord::Created { paths } => {
                            let (direct, rest): (Vec<_>, Vec<_>) =
                                paths.into_iter().partition(|p| restoring.contains(p));
                            let mut failed = None;
                            for p in direct.iter().filter(|p| exists_no_follow(p)) {
                                if let Err(e) = remove_any(p) {
                                    failed = Some(e);
                                    break;
                                }
                            }
                            match failed {
                                Some(e) => {
                                    let left: Vec<_> = direct
                                        .into_iter()
                                        .chain(rest)
                                        .filter(|p| exists_no_follow(p))
                                        .collect();
                                    Err((
                                        e,
                                        (!left.is_empty())
                                            .then_some(UndoRecord::Created { paths: left }),
                                    ))
                                }
                                None if rest.is_empty() => Ok(String::new()),
                                None => {
                                    Self::revert(UndoRecord::Created { paths: rest }, permanent)
                                }
                            }
                        }
                        other => Self::revert(other, permanent),
                    };
                    if let Err((e, left)) = res {
                        records.extend(left);
                        let rec = (!records.is_empty()).then_some(UndoRecord::Batch {
                            label,
                            records,
                            dirs,
                        });
                        return Err((e, rec));
                    }
                }
            }
        }
        Ok(desc)
    }
}

/// Restores trashed items: those inside WSL from their distribution's trash, the others
/// from the system trash.
fn restore_from_trash(paths: &[PathBuf], at: i64, until: i64) -> Result<()> {
    let (wsl, rest): (Vec<PathBuf>, Vec<PathBuf>) = paths
        .iter()
        .cloned()
        .partition(|p| super::wsl::is_wsl_path(p));
    let from_wsl = if wsl.is_empty() {
        Ok(())
    } else {
        super::wsl::restore(&wsl, at, until)
    };
    let from_system = if rest.is_empty() {
        Ok(())
    } else {
        restore_from_system_trash(&rest, at, until)
    };
    from_wsl.and(from_system)
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn restore_from_system_trash(paths: &[PathBuf], at: i64, until: i64) -> Result<()> {
    let items = list_trash()?;
    let wanted = pick_trashed(items, paths, at, until, |i| {
        (i.original_path(), i.time_deleted, trash_fine_time(&i.id))
    });
    if wanted.is_empty() {
        return Err(CoreError::Trash("items are no longer in the trash".into()));
    }
    let missing = paths.len() - wanted.len();
    with_trash(move || restore_trash_items(wanted))?;
    if missing > 0 {
        return Err(CoreError::Trash(format!(
            "{} no longer in the trash",
            if missing == 1 {
                "1 item is".to_string()
            } else {
                format!("{missing} items are")
            }
        )));
    }
    Ok(())
}

/// Puts trash items back where they were deleted from (`trash::os_limited::restore_all`).
/// On freedesktop systems the crate follows a trashed symlink: one to a folder became an
/// empty folder at its original place and the restore failed. Symlinks are moved back
/// here instead.
#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
pub(crate) fn restore_trash_items(
    items: Vec<trash::TrashItem>,
) -> std::result::Result<(), trash::Error> {
    #[cfg(not(windows))]
    let items = {
        let fs_err = |path: &Path, source| trash::Error::FileSystem {
            path: path.to_path_buf(),
            source,
        };
        let mut rest = Vec::new();
        let mut iter = items.into_iter();
        while let Some(item) = iter.next() {
            // The id is `<trash>/info/<x>.trashinfo`, the item is `<trash>/files/<x>`.
            let info = PathBuf::from(&item.id);
            let stored = match (info.parent().and_then(Path::parent), info.file_stem()) {
                (Some(trash_dir), Some(stem)) => trash_dir.join("files").join(stem),
                _ => PathBuf::new(),
            };
            if !fs::symlink_metadata(&stored).is_ok_and(|m| m.file_type().is_symlink()) {
                rest.push(item);
                continue;
            }
            let original = item.original_path();
            if exists_no_follow(&original) {
                return Err(trash::Error::RestoreCollision {
                    path: original,
                    remaining_items: std::iter::once(item).chain(rest).chain(iter).collect(),
                });
            }
            fs::create_dir_all(&item.original_parent)
                .map_err(|e| fs_err(&item.original_parent, e))?;
            fs::rename(&stored, &original).map_err(|e| fs_err(&stored, e))?;
            fs::remove_file(&info).map_err(|e| fs_err(&info, e))?;
        }
        rest
    };
    if items.is_empty() {
        return Ok(());
    }
    trash::os_limited::restore_all(items)
}

/// The trash items that undo restores: deleted from one of `paths` between `at` and
/// `until` (epoch ms), and the newest one per path when a path was trashed more than
/// once in that span (restoring two would collide). `key` gives the original path, the
/// deletion time in seconds as the trash stores it, and a finer time in ns (0 when not
/// known; see [`trash_fine_time`]). The fine time keeps out an item with the same path
/// trashed right after (undoing a copy trashes what it made, maybe under that name).
#[allow(dead_code)] // unused where restoring is not supported (macOS)
fn pick_trashed<T>(
    items: Vec<T>,
    paths: &[PathBuf],
    at: i64,
    until: i64,
    key: impl Fn(&T) -> (PathBuf, i64, u128),
) -> Vec<T> {
    let mut best: std::collections::HashMap<PathBuf, ((i64, u128), T)> = Default::default();
    let wanted: Vec<PathBuf> = paths.iter().map(|p| trash_path_key(p)).collect();
    for item in items {
        let (orig, when, fine) = key(&item);
        // Seconds as stored by the trash: 2 s slack for rounding.
        if when < at.div_euclid(1000) - 2 || when > until.div_euclid(1000) + 2 {
            continue;
        }
        let orig = trash_path_key(&orig);
        if !wanted.contains(&orig) {
            continue;
        }
        // The fine time is a file time written during the delete: file times lag the clock
        // (the kernel's coarse time), so in whole ms never later than `until`; on FAT
        // they are up to 2 s earlier.
        let fine_ms = (fine / 1_000_000) as i64;
        if fine != 0 && (fine_ms > until || fine_ms < at - 2_100) {
            continue;
        }
        if best.get(&orig).is_none_or(|(t, _)| (when, fine) > *t) {
            best.insert(orig, ((when, fine), item));
        }
    }
    best.into_values().map(|(_, i)| i).collect()
}

/// `trash::os_limited::list`, tried again when it panics. On freedesktop systems the crate reads
/// every `.trashinfo` file and *panics* on one that another process or thread is writing at that
/// moment (a half-written file has no `key=value` line). That passes at once, so listing is
/// retried a few times instead of taking the worker thread down.
#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
pub(crate) fn list_trash_items() -> Result<Vec<trash::TrashItem>> {
    #[cfg(all(
        test,
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    ))]
    if let Some(items) = test_support::list_isolated_trash() {
        return items;
    }
    const ATTEMPTS: u64 = 5;
    for attempt in 1..=ATTEMPTS {
        match std::panic::catch_unwind(|| with_trash(trash::os_limited::list)) {
            Ok(result) => return result,
            Err(_) if attempt < ATTEMPTS => {
                std::thread::sleep(std::time::Duration::from_millis(25 * attempt));
            }
            Err(_) => break,
        }
    }
    Err(CoreError::Trash(
        "the trash could not be read (a file in it kept changing)".into(),
    ))
}

/// The trash's items, with their real names. On Windows the `trash` crate takes an
/// item's name from the shell's display name, which has no extension when Explorer hides
/// known extensions (the default): `a.txt` was listed, matched and restored as `a`. The
/// Recycle Bin's `$I` file holds the full original path, so the name comes from there.
#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
pub(crate) fn list_trash() -> Result<Vec<trash::TrashItem>> {
    #[allow(unused_mut)]
    let mut items = list_trash_items()?;
    #[cfg(windows)]
    for item in &mut items {
        if let Some(name) = recycle_bin_original_path(&item.id)
            .as_deref()
            .and_then(Path::file_name)
        {
            item.name = name.to_os_string();
        }
    }
    Ok(items)
}

/// The original path stored in a Recycle Bin item's `$I` file: version (8 bytes), size
/// (8), deletion time (8), then the path in UTF-16: version 1 (Vista–8.1) a fixed
/// 260-character field, version 2 (Windows 10+) a character count (4 bytes) first.
#[cfg(windows)]
fn recycle_bin_original_path(id: &std::ffi::OsStr) -> Option<PathBuf> {
    let p = Path::new(id);
    let rest = p.file_name()?.to_str()?.strip_prefix("$R")?;
    let b = fs::read(p.with_file_name(format!("$I{rest}"))).ok()?;
    let version = u64::from_le_bytes(b.get(0..8)?.try_into().ok()?);
    let raw = match version {
        1 => b.get(24..24 + 520)?,
        2 => {
            let n = u32::from_le_bytes(b.get(24..28)?.try_into().ok()?) as usize;
            b.get(28..28 + n.checked_mul(2)?)?
        }
        _ => return None,
    };
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .collect();
    (!units.is_empty()).then(|| PathBuf::from(String::from_utf16_lossy(&units)))
}

/// A path as the trash and Gall can both spell it. On Windows the Recycle Bin keeps the
/// long, resolved path, while Gall may hold an 8.3 short form (`C:\Users\RUNNER~1\…`, as
/// in %TEMP%) or other letter case: the existing parent is resolved and the result
/// compared without case. Elsewhere the path itself.
#[allow(dead_code)]
fn trash_path_key(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let full = match (p.parent(), p.file_name()) {
            (Some(dir), Some(name)) => dunce::canonicalize(dir)
                .map(|d| d.join(name))
                .unwrap_or_else(|_| p.to_path_buf()),
            _ => p.to_path_buf(),
        };
        PathBuf::from(full.to_string_lossy().to_lowercase())
    }
    #[cfg(not(windows))]
    p.to_path_buf()
}

/// When a trash item was deleted, finer than the trash's seconds: on Linux the time of
/// its `.trashinfo` file (nanoseconds), on Windows the deletion time in the Recycle Bin's
/// `$I` file (100 ns). 0 when unknown.
#[allow(dead_code)]
fn trash_fine_time(id: &std::ffi::OsStr) -> u128 {
    let p = Path::new(id);
    #[cfg(windows)]
    {
        // "…\$Recycle.Bin\<SID>\$Rxxxxxx.ext" → "$Ixxxxxx.ext": version (8 bytes), size
        // (8), then the deletion FILETIME (8, little-endian): 100 ns units since 1601,
        // converted to ns since the Unix epoch like the other platforms.
        const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            if let Some(rest) = name.strip_prefix("$R") {
                if let Ok(b) = fs::read(p.with_file_name(format!("$I{rest}"))) {
                    if let Some(t) = b.get(16..24) {
                        let ft = u64::from_le_bytes(t.try_into().unwrap());
                        return ft.saturating_sub(UNIX_EPOCH_FILETIME) as u128 * 100;
                    }
                }
            }
        }
        0
    }
    #[cfg(not(windows))]
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

/// Trashes `paths`; returns the span (epoch milliseconds) it took, for [`UndoRecord::Trashed`].
fn trash_timed(paths: &[PathBuf]) -> Result<(i64, i64)> {
    let at = chrono::Utc::now().timestamp_millis();
    trash_paths(paths)?;
    Ok((at, chrono::Utc::now().timestamp_millis()))
}

/// Runs a `trash` crate call. On Windows it runs on a fresh thread: the crate initialises
/// COM as single-threaded and *panics* if the calling thread already has COM in another
/// mode (RPC_E_CHANGED_MODE), which happens on async worker threads that other libraries
/// (sysinfo, the webview) used. A fresh thread has no COM state; a panic becomes an error.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn with_trash<T, E, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    E: std::fmt::Display,
    F: FnOnce() -> std::result::Result<T, E> + Send + 'static,
{
    #[cfg(windows)]
    let r = std::thread::Builder::new()
        .name("trash".into())
        .spawn(move || f().map_err(|e| e.to_string()))
        .map_err(|e| CoreError::Trash(e.to_string()))?
        .join()
        .map_err(|p| {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown error".into());
            CoreError::Trash(format!("trash failed: {msg}"))
        })?;
    #[cfg(not(windows))]
    let r = f().map_err(|e| e.to_string());
    r.map_err(CoreError::Trash)
}

/// True for UNC paths and mapped network drives.
#[cfg(windows)]
fn on_network_drive(p: &Path) -> bool {
    use std::path::{Component, Prefix};
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const DRIVE_REMOTE: u32 = 4;
    match p.components().next() {
        Some(Component::Prefix(pre)) => match pre.kind() {
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) => true,
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                let root: Vec<u16> = format!("{}:\\", d as char)
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                // SAFETY: a valid NUL-terminated root path.
                unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
            }
            _ => false,
        },
        _ => false,
    }
}

/// Moves `paths` to the trash (see [`with_trash`]).
/// Items sent to the trash so far (Windows tests: see `tests::TestDir`).
#[cfg(all(test, windows))]
static TRASHED: AtomicU64 = AtomicU64::new(0);

fn trash_paths(paths: &[PathBuf]) -> Result<()> {
    #[cfg(all(test, windows))]
    TRASHED.fetch_add(1, Ordering::Relaxed);
    // Inside WSL: the distribution's own trash (Windows has no Recycle Bin there and
    // would delete for good when asked to recycle).
    let (wsl, rest): (Vec<PathBuf>, Vec<PathBuf>) = paths
        .iter()
        .cloned()
        .partition(|p| super::wsl::is_wsl_path(p));
    if !wsl.is_empty() {
        let _released = release(wsl.iter().map(PathBuf::as_path));
        super::wsl::trash(&wsl)?;
    }
    if rest.is_empty() {
        return Ok(());
    }
    let paths = rest.as_slice();
    // Windows deletes items on network drives for good when asked to recycle them.
    #[cfg(windows)]
    if let Some(p) = paths.iter().find(|p| on_network_drive(p)) {
        return Err(CoreError::Trash(format!(
            "“{}” is on a network drive, which has no Recycle Bin",
            p.display()
        )));
    }
    let _released = release(paths.iter().map(PathBuf::as_path));
    #[cfg(all(
        test,
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    ))]
    if let Some(res) = test_support::trash_into_isolated(paths) {
        return res;
    }
    #[cfg(target_os = "macos")]
    return super::mactrash::trash(paths);
    #[cfg(not(target_os = "macos"))]
    {
        let owned = paths.to_vec();
        with_trash(move || trash::delete_all(owned))
    }
}

#[cfg(target_os = "macos")]
fn restore_from_system_trash(paths: &[PathBuf], at: i64, until: i64) -> Result<()> {
    let wanted = pick_trashed(super::mactrash::items(), paths, at, until, |i| {
        (
            i.original.clone(),
            i.time_ms.div_euclid(1000),
            i.time_ms as u128 * 1_000_000,
        )
    });
    if wanted.is_empty() {
        return Err(CoreError::Trash("items are no longer in the trash".into()));
    }
    let missing = paths.len() - wanted.len();
    super::mactrash::restore(wanted)?;
    if missing > 0 {
        return Err(CoreError::Trash(format!(
            "{} no longer in the trash",
            if missing == 1 {
                "1 item is".to_string()
            } else {
                format!("{missing} items are")
            }
        )));
    }
    Ok(())
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    all(unix, not(target_os = "ios"), not(target_os = "android"))
)))]
fn restore_from_system_trash(_paths: &[PathBuf], _at: i64, _until: i64) -> Result<()> {
    Err(CoreError::Trash(
        "restoring from trash is not supported on this platform".into(),
    ))
}

// ───────────────────────────── quick operations ─────────────────────────────

pub fn rename(path: &Path, new_name: &str, journal: &UndoJournal) -> Result<PathBuf> {
    validate_name(new_name)?;
    super::wsl::check_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::InvalidName(new_name.into()))?;
    let dest = parent.join(new_name);
    if dest == path {
        return Ok(dest);
    }
    // Case-only rename on case-insensitive filesystems: target "exists" but is
    // the same file, so allow it.
    // (Any letters, not just ASCII: "čebela" → "Čebela".)
    if exists_no_follow(&dest) && !same_entry(path, &dest) {
        return Err(CoreError::AlreadyExists(dest));
    }
    let released = release([path]);
    fs::rename(path, &dest).ctx("rename", path)?;
    drop(released);
    moved(path, &dest);
    journal.push(UndoRecord::Moved {
        pairs: vec![(path.to_path_buf(), dest.clone())],
    });
    Ok(dest)
}

/// True when `a` and `b` are the same directory entry: one name differs only in case on a
/// case-insensitive file system (or both are the same path).
fn same_entry(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    let (Ok(x), Ok(y)) = (fs::symlink_metadata(a), fs::symlink_metadata(b)) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Two hard links of one file are two entries: the same file under one name only
        // when it has no other link, or the names differ only in case.
        let one_name = x.is_dir()
            || x.nlink() == 1
            || (cfg!(target_os = "macos")
                && a.file_name().map(|n| n.to_string_lossy().to_lowercase())
                    == b.file_name().map(|n| n.to_string_lossy().to_lowercase()));
        x.dev() == y.dev() && x.ino() == y.ino() && a.parent() == b.parent() && one_name
    }
    #[cfg(not(unix))]
    {
        let _ = (x, y);
        // Folders inside WSL tell case apart: "README" and "Readme" are two files there.
        !super::wsl::is_wsl_path(a)
            && a.parent() == b.parent()
            && a.file_name().map(|n| n.to_string_lossy().to_lowercase())
                == b.file_name().map(|n| n.to_string_lossy().to_lowercase())
    }
}

/// Renames every `(from, to)`; all or nothing. When a name is taken by another item of
/// the set (swaps, a → b → c) or only its case changes, everything goes through a
/// temporary name first. A failure reverts the renames already done.
fn rename_many(pairs: &[(PathBuf, PathBuf)]) -> Result<()> {
    rename_many_with_ids(pairs, &mut || {
        RENAME_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    })
}

fn rename_many_with_ids(
    pairs: &[(PathBuf, PathBuf)],
    next_id: &mut dyn FnMut() -> u64,
) -> Result<()> {
    let _released = release(pairs.iter().map(|(f, _)| f.as_path()));
    let froms: std::collections::HashSet<&PathBuf> = pairs.iter().map(|(f, _)| f).collect();
    let overlap = pairs
        .iter()
        .any(|(f, t)| froms.contains(t) || (exists_no_follow(t) && same_entry(f, t)));
    // Every rename that happens is reported (tags follow, also through temporary names).
    let mv = |a: &Path, b: &Path| -> Result<()> {
        rename_noreplace(a, b).ctx("rename", a)?;
        moved(a, b);
        Ok(())
    };
    if !overlap {
        for (i, (from, to)) in pairs.iter().enumerate() {
            if let Err(e) = mv(from, to) {
                for (f, t) in pairs[..i].iter().rev() {
                    let _ = mv(t, f);
                }
                return Err(e);
            }
        }
        return Ok(());
    }
    let mut tmp = Vec::with_capacity(pairs.len());
    for (i, (from, _)) in pairs.iter().enumerate() {
        let staged = loop {
            let id = next_id();
            let candidate =
                from.with_file_name(format!(".gall-rename-{}-{id}", std::process::id()));
            match mv(from, &candidate) {
                Ok(()) => break Ok(candidate),
                Err(CoreError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::AlreadyExists =>
                {
                    continue
                }
                Err(e) => break Err(e),
            }
        };
        match staged {
            Ok(path) => tmp.push(path),
            Err(e) => {
                for j in (0..i).rev() {
                    let _ = mv(&tmp[j], &pairs[j].0);
                }
                return Err(e);
            }
        }
    }
    for (i, (_, to)) in pairs.iter().enumerate() {
        if let Err(e) = mv(&tmp[i], to) {
            for j in (0..i).rev() {
                let _ = mv(&pairs[j].1, &pairs[j].0);
            }
            for j in i..pairs.len() {
                let _ = mv(&tmp[j], &pairs[j].0);
            }
            return Err(e);
        }
    }
    Ok(())
}

static RENAME_TEMP_ID: AtomicU64 = AtomicU64::new(1);

/// Atomically renames without replacing an entry created concurrently at `to`.
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: both paths are NUL-terminated and the syscall does not retain them.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: both paths are NUL-terminated and the call does not retain them.
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn MoveFileW(existing: *const u16, new: *const u16) -> i32;
        }
        let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        // MoveFileW fails rather than replacing an existing destination.
        if unsafe { MoveFileW(from.as_ptr(), to.as_ptr()) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios", windows)))]
    {
        let _ = (from, to);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic no-replace rename is unsupported on this platform",
        ))
    }
}

/// Renames several items at once (batch rename) as one undo step; returns the new paths
/// in the order given. Items may swap names or change only the case of a letter. When
/// one rename fails, the others are reverted.
pub fn batch_rename(items: &[(PathBuf, String)], journal: &UndoJournal) -> Result<Vec<PathBuf>> {
    batch_rename_recorded(items, journal).map(|(out, _)| out)
}

/// [`batch_rename`], also returning the stable undo record ID when anything changed.
pub fn batch_rename_recorded(
    items: &[(PathBuf, String)],
    journal: &UndoJournal,
) -> Result<(Vec<PathBuf>, Option<u64>)> {
    let mut out = Vec::with_capacity(items.len());
    let mut pairs = Vec::new();
    for (path, name) in items {
        validate_name(name)?;
        super::wsl::check_path(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| CoreError::InvalidName(name.clone()))?;
        let dest = parent.join(name);
        if dest != *path {
            pairs.push((path.clone(), dest.clone()));
        }
        out.push(dest);
    }
    // Two items must not end up with one name (nor one differing only in case, where
    // the file system ignores case).
    let key = |p: &Path| -> String {
        let s = p.to_string_lossy();
        if cfg!(any(windows, target_os = "macos")) {
            s.to_lowercase()
        } else {
            s.into_owned()
        }
    };
    let mut seen = std::collections::HashSet::new();
    for d in &out {
        if !seen.insert(key(d)) {
            return Err(CoreError::Other(format!(
                "two items would be named {}",
                quoted(d)
            )));
        }
    }
    let froms: std::collections::HashSet<&PathBuf> = pairs.iter().map(|(f, _)| f).collect();
    for (from, to) in &pairs {
        if exists_no_follow(to) && !froms.contains(to) && !same_entry(from, to) {
            return Err(CoreError::AlreadyExists(to.clone()));
        }
    }
    if pairs.is_empty() {
        return Ok((out, None));
    }
    rename_many(&pairs)?;
    let id = journal.push(UndoRecord::Moved { pairs });
    Ok((out, Some(id)))
}

pub fn create_dir(parent: &Path, name: &str, journal: &UndoJournal) -> Result<PathBuf> {
    create_dir_recorded(parent, name, journal).map(|(p, _)| p)
}

/// [`create_dir`], also returning the stable undo record ID.
pub fn create_dir_recorded(
    parent: &Path,
    name: &str,
    journal: &UndoJournal,
) -> Result<(PathBuf, u64)> {
    validate_name(name)?;
    let p = parent.join(name);
    if exists_no_follow(&p) {
        return Err(CoreError::AlreadyExists(p));
    }
    fs::create_dir(&p).ctx("create folder", &p)?;
    let id = journal.push(UndoRecord::Created {
        paths: vec![p.clone()],
    });
    Ok((p, id))
}

/// Creates a new file, either empty, with `contents`, or cloned from a template.
pub fn create_file(
    parent: &Path,
    name: &str,
    template: Option<&Path>,
    contents: Option<&[u8]>,
    journal: &UndoJournal,
) -> Result<PathBuf> {
    create_file_recorded(parent, name, template, contents, journal).map(|(p, _)| p)
}

/// [`create_file`], also returning the stable undo record ID.
pub fn create_file_recorded(
    parent: &Path,
    name: &str,
    template: Option<&Path>,
    contents: Option<&[u8]>,
    journal: &UndoJournal,
) -> Result<(PathBuf, u64)> {
    validate_name(name)?;
    let p = parent.join(name);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                CoreError::AlreadyExists(p.clone())
            } else {
                CoreError::io("create file", &p, e)
            }
        })?;
    let written = (|| -> Result<()> {
        if let Some(t) = template {
            let mut src = fs::File::open(t).ctx("open template", t)?;
            std::io::copy(&mut src, &mut f).ctx("write", &p)?;
        } else if let Some(c) = contents {
            f.write_all(c).ctx("write", &p)?;
        }
        Ok(())
    })();
    drop(f);
    // Closed by now: removed again when the template cannot be read or the write fails.
    if let Err(e) = written {
        let _ = fs::remove_file(&p);
        return Err(e);
    }
    let id = journal.push(UndoRecord::Created {
        paths: vec![p.clone()],
    });
    Ok((p, id))
}

/// Moves items to the system trash / Recycle Bin.
/// One step of a folder sync.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum SyncOp {
    /// Copy file or folder `src` to exactly `dest` (parents are created). An existing
    /// `dest` is moved to the trash first, so the sync can be undone.
    Copy { src: PathBuf, dest: PathBuf },
    /// Move `path` to the trash (mirror: remove what the source side does not have).
    Trash { path: PathBuf },
}

/// What a file looked like when a comparison was made: its size and modification time (whole seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Stamp {
    pub size: u64,
    pub mtime: i64,
}

/// A path of a sync plan as the comparison saw it, checked again right before anything is written. `stamp: None` means
/// that the path did not exist.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncCheck {
    pub path: PathBuf,
    #[serde(default)]
    pub stamp: Option<Stamp>,
    /// The path must still be a folder (a whole-tree copy or trash of it is planned); `stamp` is then not used.
    #[serde(default)]
    pub dir: bool,
    /// For a folder that is copied or trashed as a whole: the fingerprint of everything below it when the plan was
    /// shown (see [`tree_fingerprint`]); a folder that holds other entries by the time of the write is refused.
    #[serde(default)]
    pub tree: Option<String>,
    /// For a file: its size and modification time to the nanosecond when the plan was shown (see
    /// [`file_fingerprint`]); the seconds of `stamp` miss a rewrite of equal size within the same second.
    #[serde(default)]
    pub exact: Option<String>,
}

/// The size and the modification time to the nanosecond of a file (not followed if it is a link), or `None` when
/// there is no such file. Two states of one file that differ by a rewrite have different fingerprints even within a
/// second, wherever the file system keeps finer times.
pub fn file_fingerprint(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    Some(format!("{}:{nanos}", meta.len()))
}

/// Most entries a fingerprint walk looks at; a bigger tree cannot be checked and a whole-tree operation on it is
/// refused instead of being run unchecked.
pub const MAX_TREE_ENTRIES: u64 = 2_000_000;

/// A fingerprint of everything below `root`: every entry (hidden and ignored ones too, links not followed) with its
/// relative path, size and modification time to the nanosecond, combined without regard to the order of the walk.
/// A file added, removed, renamed or rewritten below it changes the fingerprint.
pub fn tree_fingerprint(root: &Path) -> Result<String> {
    use std::hash::{Hash, Hasher};
    let mut sum: u64 = 0;
    let mut entries: u64 = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ctx("read", &dir)? {
            let entry = entry.ctx("read", &dir)?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).ctx("read", &path)?;
            entries += 1;
            if entries > MAX_TREE_ENTRIES {
                return Err(CoreError::Other(format!(
                    "{} holds more than {MAX_TREE_ENTRIES} entries and cannot be checked as a whole",
                    root.display()
                )));
            }
            let nanos = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos());
            let mut h = std::collections::hash_map::DefaultHasher::new();
            path.strip_prefix(root).unwrap_or(&path).hash(&mut h);
            (meta.len(), nanos, meta.is_dir(), meta.is_symlink()).hash(&mut h);
            sum = sum.wrapping_add(h.finish());
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(format!("{entries}:{sum:016x}"))
}

/// Whole seconds since the epoch of the modification time (negative before it), as the folder comparison counts them.
pub fn mtime_secs(m: &std::fs::Metadata) -> Option<i64> {
    let time = m.modified().ok()?;
    Some(match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    })
}

/// Checks every path of a plan against what the comparison saw. The first path that differs ends the check with its
/// name, and nothing has been touched yet.
pub fn verify_sync(checks: &[SyncCheck]) -> Result<()> {
    use std::io::ErrorKind;
    for check in checks {
        let shown = check.path.display();
        let refuse = |what: &str| {
            Err(CoreError::Other(format!(
                "{shown} {what}; compare again before syncing"
            )))
        };
        let now = match std::fs::symlink_metadata(&check.path) {
            Ok(m) => Some(m),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            // Only "not there" is absence: a path that cannot be looked at cannot be checked, so it is not written.
            Err(_) => return refuse("cannot be checked"),
        };
        match (check.dir, &check.stamp, now) {
            // A folder that is to be copied or trashed as a whole must still be a folder.
            (true, _, Some(m)) => {
                if !m.is_dir() {
                    return refuse("is no longer a folder");
                }
                if let Some(expected) = &check.tree {
                    match tree_fingerprint(&check.path) {
                        Ok(now) if &now == expected => {}
                        Ok(_) => return refuse("holds other entries than when it was compared"),
                        Err(_) => return refuse("cannot be checked as a whole"),
                    }
                }
            }
            (true, _, None) | (false, Some(_), None) => {
                return refuse("is gone since the comparison")
            }
            (false, None, Some(_)) => return refuse("appeared after the comparison"),
            (false, None, None) => {}
            (false, Some(stamp), Some(m)) => {
                if !m.is_file() || m.len() != stamp.size || mtime_secs(&m) != Some(stamp.mtime) {
                    return refuse("changed after the comparison");
                }
                // Finer than the seconds of the comparison: the state when the plan was shown.
                if let Some(exact) = &check.exact {
                    if file_fingerprint(&check.path).as_deref() != Some(exact.as_str()) {
                        return refuse("changed after the plan was shown");
                    }
                }
            }
        }
    }
    Ok(())
}

/// [`apply_sync`] after [`verify_sync`]: a plan whose files changed since the preview is refused as a whole.
pub fn apply_sync_checked(
    ops: &[SyncOp],
    checks: &[SyncCheck],
    journal: &UndoJournal,
    cancel: &AtomicBool,
) -> Result<SyncReport> {
    verify_sync(checks)?;
    apply_sync(ops, journal, cancel)
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub copied: usize,
    pub replaced: usize,
    pub trashed: usize,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_id: Option<u64>,
}

/// Applies a folder sync as one undoable operation. Stops at the first error;
/// what was done until then is still recorded for undo.
pub fn apply_sync(
    ops: &[SyncOp],
    journal: &UndoJournal,
    cancel: &AtomicBool,
) -> Result<SyncReport> {
    let mut rep = SyncReport::default();
    let mut created: Vec<PathBuf> = Vec::new();
    // Each trashed item keeps its own timestamp: restoring matches on the deletion time.
    let mut trashed: Vec<(PathBuf, (i64, i64))> = Vec::new();
    let result = (|| -> Result<()> {
        for op in ops {
            if cancel.load(Ordering::Relaxed) {
                return Err(CoreError::Cancelled);
            }
            match op {
                SyncOp::Copy { src, dest } => {
                    if dest.starts_with(src) {
                        return Err(CoreError::IntoItself {
                            op: "copy",
                            src: src.clone(),
                            dest: dest.clone(),
                        });
                    }
                    if let Some(parent) = dest.parent() {
                        // Record the top-most created folder so undo removes it too.
                        let mut missing = parent.to_path_buf();
                        let mut top = None;
                        while !missing.exists() {
                            top = Some(missing.clone());
                            match missing.parent() {
                                Some(p) => missing = p.to_path_buf(),
                                None => break,
                            }
                        }
                        std::fs::create_dir_all(parent).ctx("create folder", parent)?;
                        if let Some(t) = top {
                            created.push(t);
                        }
                    }
                    if exists_no_follow(dest) {
                        let when = trash_timed(std::slice::from_ref(dest))?;
                        trashed.push((dest.clone(), when));
                        rep.replaced += 1;
                    }
                    let mut bytes = 0u64;
                    // A failed or cancelled copy is removed again (see `copy_fresh`).
                    copy_fresh(src, dest, cancel, &mut |b| bytes += b, false)?;
                    rep.bytes += bytes;
                    rep.copied += 1;
                    if !created.iter().any(|c| dest.starts_with(c)) {
                        created.push(dest.clone());
                    }
                }
                SyncOp::Trash { path } => {
                    let when = trash_timed(std::slice::from_ref(path))?;
                    trashed.push((path.clone(), when));
                    rep.trashed += 1;
                }
            }
        }
        Ok(())
    })();
    let mut records: Vec<UndoRecord> = trashed
        .into_iter()
        .map(|(p, (at, until))| UndoRecord::Trashed {
            paths: vec![p],
            at,
            until,
        })
        .collect();
    if !created.is_empty() {
        records.push(UndoRecord::Created { paths: created });
    }
    if !records.is_empty() {
        rep.undo_id = Some(journal.push(UndoRecord::Batch {
            label: format!("sync of {}", items(rep.copied + rep.trashed)),
            records,
            dirs: Vec::new(),
        }));
    }
    result.map(|_| rep)
}

/// Saves edited text over `path` (compare view merges). Returns whether the save can be
/// undone.
///
/// The text goes to a temporary file next to the target first, so a failed write never
/// damages the file. Then the previous version moves to the trash and the new file takes
/// its place (one undo step). A symlink is followed: the file it points to is saved and
/// the link stays. Where no trash is available (a network share, a read-only trash), the
/// new file replaces the old one atomically, without undo.
/// Size and modification time of a file, as one text; changes when the file is written.
pub fn file_revision(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    file_revision_of(&meta)
}

/// Captures a revision from the open file rather than looking up its path again.
pub fn file_revision_of(meta: &fs::Metadata) -> Option<String> {
    let nanos = meta
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("{}-{nanos}", meta.len()))
}

/// `save_text` for a file that was read at `expected` (see [`file_revision`]): when it changed on
/// disk since, nothing is written, so an edit made against an old copy cannot overwrite newer work.
/// An empty or missing `expected` saves without the check, as for a new file.
pub fn save_text_checked(
    path: &Path,
    text: &str,
    expected: Option<&str>,
    journal: &UndoJournal,
) -> Result<bool> {
    save_text_recorded(path, text, expected, false, journal).map(|(u, _)| u)
}

pub fn save_text(path: &Path, text: &str, journal: &UndoJournal) -> Result<bool> {
    save_text_recorded(path, text, None, false, journal).map(|(u, _)| u)
}

/// Publishes a new result only if the selected name is still free.
pub fn save_new_text(path: &Path, text: &str, journal: &UndoJournal) -> Result<bool> {
    save_text_recorded(path, text, None, true, journal).map(|(u, _)| u)
}

/// Saves text and returns `(undoable, undo_id)`.
pub fn save_text_recorded(
    path: &Path,
    text: &str,
    expected: Option<&str>,
    expect_missing: bool,
    journal: &UndoJournal,
) -> Result<(bool, Option<u64>)> {
    save_text_inner(
        path,
        text,
        expected.filter(|want| !want.is_empty()),
        expect_missing,
        journal,
    )
}

fn save_text_inner(
    path: &Path,
    text: &str,
    expected: Option<&str>,
    expect_missing: bool,
    journal: &UndoJournal,
) -> Result<(bool, Option<u64>)> {
    let real = if !expect_missing && exists_no_follow(path) {
        fs::canonicalize(path).ctx("resolve", path)?
    } else {
        path.to_path_buf()
    };
    let dir = real
        .parent()
        .ok_or_else(|| CoreError::NotFound(real.clone()))?;
    let tmp = dir.join(format!(
        ".gall-save-{}-{}",
        std::process::id(),
        RENAME_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .ctx("save", &tmp)?;
    let guard = DeleteOnDrop(Some(tmp.clone()));
    {
        let mut file = file;
        file.write_all(text.as_bytes()).ctx("save", &tmp)?;
        file.sync_all().ctx("save", &tmp)?;
    }
    if let Some(want) = expected {
        if file_revision(&real).as_deref() != Some(want) {
            return Err(CoreError::Other(format!(
                "{} changed on disk since it was compared. Reload the comparison before saving; your edits are still in the window",
                path.display()
            )));
        }
    }

    let mut records = Vec::new();
    let mut undoable = true;
    let res = if expect_missing {
        rename_noreplace(&tmp, &real)
    } else {
        if let Ok(m) = fs::metadata(&real) {
            let _ = fs::set_permissions(&tmp, m.permissions());
        }
        if exists_no_follow(&real) {
            match trash_timed(std::slice::from_ref(&real)) {
                Ok((at, until)) => records.push(UndoRecord::Trashed {
                    paths: vec![real.clone()],
                    at,
                    until,
                }),
                Err(e) => {
                    log::info!("saving {} without undo: {e}", real.display());
                    undoable = false;
                }
            }
        }
        // Over the old file when it could not be trashed: rename replaces atomically.
        fs::rename(&tmp, &real)
    }
    .ctx("save", &real);
    if res.is_ok() {
        guard.disarm();
        records.push(UndoRecord::Created {
            paths: vec![real.clone()],
        });
    }
    // Recorded even when the rename failed: undo then brings the old version back.
    let undo_id = if undoable && !records.is_empty() {
        Some(journal.push(UndoRecord::Batch {
            label: format!("save {}", quoted(&real)),
            records,
            dirs: Vec::new(),
        }))
    } else {
        None
    };
    res.map(|()| (undoable, undo_id))
}

pub fn trash_items(paths: &[PathBuf], journal: &UndoJournal) -> Result<()> {
    trash_items_recorded(paths, journal).0
}

/// Moves items to the system trash and returns `(result, undo_id)`. On partial failure,
/// `undo_id` still identifies the items that reached the trash.
pub fn trash_items_recorded(paths: &[PathBuf], journal: &UndoJournal) -> (Result<()>, Option<u64>) {
    if paths.is_empty() {
        return (Ok(()), None);
    }
    let at = chrono::Utc::now().timestamp_millis();
    let existed: Vec<&PathBuf> = paths.iter().filter(|p| exists_no_follow(p)).collect();
    let res = trash_paths(paths);
    // A failure part-way: what did reach the trash can still be undone.
    let gone: Vec<PathBuf> = match res {
        Ok(()) => paths.to_vec(),
        Err(_) => existed
            .into_iter()
            .filter(|p| !exists_no_follow(p))
            .cloned()
            .collect(),
    };
    let undo_id = if !gone.is_empty() {
        Some(journal.push(UndoRecord::Trashed {
            paths: gone,
            at,
            until: chrono::Utc::now().timestamp_millis(),
        }))
    } else {
        None
    };
    (res, undo_id)
}

/// Creates a symbolic link to `target` inside `dest_dir`. (O-20)
pub fn symlink(target: &Path, dest_dir: &Path, journal: &UndoJournal) -> Result<PathBuf> {
    let link = unique_path(dest_dir, file_name(target)?);
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &link).ctx("symlink", &link)?;
    #[cfg(windows)]
    {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, &link).ctx("symlink", &link)?;
        } else {
            std::os::windows::fs::symlink_file(target, &link).ctx("symlink", &link)?;
        }
    }
    journal.push(UndoRecord::Created {
        paths: vec![link.clone()],
    });
    Ok(link)
}

// ───────────────────────────── background tasks ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// Keep both: the new item gets a " (n)" suffix.
    #[default]
    KeepBoth,
    /// Replace: the existing item goes to the trash first (undoable).
    Overwrite,
    Skip,
    /// Replace and delete the existing item for good: offered when it cannot be trashed.
    OverwritePermanently,
    /// Replace (to the trash) only when the incoming file is newer; skip it otherwise.
    OverwriteOlder,
}

/// The trash refused an item that "Replace" had to move away. The message starts with
/// `no-trash:` so the UI can offer to replace it permanently.
fn no_trash_to_replace(target: &Path, e: CoreError) -> CoreError {
    CoreError::Other(format!(
        "no-trash: “{}” was not replaced: it cannot be moved to the trash ({}).",
        target
            .file_name()
            .map_or_else(|| target.to_string_lossy(), |n| n.to_string_lossy()),
        e.to_string().trim_start_matches("trash: ")
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    Copy,
    Move,
    Delete,
    /// Pack all sources into one archive in `dest` (see [`TaskRequest::archive_name`]).
    Compress,
    /// Unpack each source archive into `dest`.
    Extract,
    /// Copy each source next to itself with a " (n)" name (Ctrl+D); no `dest`.
    Duplicate,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRequest {
    pub kind: TaskKind,
    pub sources: Vec<PathBuf>,
    /// Destination folder (ignored for delete).
    #[serde(default)]
    pub dest: Option<PathBuf>,
    #[serde(default)]
    pub conflict: ConflictPolicy,
    /// Compress: file name of the new archive (its extension picks nothing; see `format`).
    #[serde(default)]
    pub archive_name: Option<String>,
    /// Compress: archive format.
    #[serde(default)]
    pub format: Option<super::archive::ArchiveFormat>,
    /// Extract: where the contents go.
    #[serde(default)]
    pub extract_mode: super::archive::ExtractMode,
    /// Extract: move each archive to the trash once it is unpacked.
    #[serde(default)]
    pub trash_archive: bool,
    /// Copy/move: a folder that exists on both sides is merged (its contents are placed
    /// into the existing folder, `conflict` decides for each file that exists in both)
    /// instead of being handled as one item.
    #[serde(default)]
    pub merge: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgress {
    pub id: u64,
    pub kind: TaskKind,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub done_items: u64,
    pub total_items: u64,
    pub current: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum TaskEvent {
    Progress(TaskProgress),
    Done {
        id: u64,
        kind: TaskKind,
        items: u64,
        skipped: u64,
        /// File name when the task had a single source, so the UI can name it.
        name: Option<String>,
        /// Name of the destination folder (copy/move).
        to: Option<String>,
        /// Stable undo journal record produced by this task, if undoable.
        #[serde(rename = "undoId", skip_serializing_if = "Option::is_none")]
        undo_id: Option<u64>,
    },
    Failed {
        id: u64,
        kind: TaskKind,
        error: String,
        /// Stable undo journal record for any partial work completed before failure.
        #[serde(rename = "undoId", skip_serializing_if = "Option::is_none")]
        undo_id: Option<u64>,
    },
    Cancelled {
        id: u64,
        kind: TaskKind,
        /// Stable undo journal record for any partial work completed before cancellation.
        #[serde(rename = "undoId", skip_serializing_if = "Option::is_none")]
        undo_id: Option<u64>,
    },
}

/// Lists destination names that already exist, so the UI can ask the user
/// which [`ConflictPolicy`] to apply before starting a task.
pub fn find_conflicts(sources: &[PathBuf], dest: &Path) -> Vec<PathBuf> {
    sources
        .iter()
        // A copy into its own folder (also through a symlink) is a duplicate, not a conflict.
        .filter(|s| !in_dir(s, dest))
        .filter_map(|s| s.file_name().map(|n| dest.join(n)))
        .filter(|d| exists_no_follow(d))
        .collect()
}

/// Whether two paths name one item: equal, or, where the file system ignores case (macOS
/// and Windows by default), equal apart from letter case. Purely textual, no disk access.
fn same_path_ci(a: &Path, b: &Path) -> bool {
    a == b
        || (cfg!(any(windows, target_os = "macos"))
            && a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase())
}

/// Whether `child` is `parent` or inside it, ignoring letter case on case-insensitive
/// platforms (see [`same_path_ci`]).
fn starts_with_ci(child: &Path, parent: &Path) -> bool {
    child.starts_with(parent)
        || (cfg!(any(windows, target_os = "macos"))
            && PathBuf::from(child.to_string_lossy().to_lowercase())
                .starts_with(PathBuf::from(parent.to_string_lossy().to_lowercase())))
}

fn ensure_not_into_itself(op: &'static str, src: &Path, dest_dir: &Path) -> Result<()> {
    let src_c = src.canonicalize().unwrap_or_else(|_| src.to_path_buf());
    let dst_c = dest_dir
        .canonicalize()
        .unwrap_or_else(|_| dest_dir.to_path_buf());
    if starts_with_ci(&dst_c, &src_c)
        && fs::symlink_metadata(src)
            .map(|m| m.is_dir())
            .unwrap_or(false)
    {
        return Err(CoreError::IntoItself {
            op,
            src: src.into(),
            dest: dest_dir.into(),
        });
    }
    Ok(())
}

struct Totals {
    bytes: u64,
    items: u64,
}

fn measure(paths: &[PathBuf]) -> Totals {
    let mut t = Totals { bytes: 0, items: 0 };
    for p in paths {
        for e in walkdir::WalkDir::new(p)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            t.items += 1;
            if e.file_type().is_file() {
                t.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    t
}

/// Every source can go to `dest` with one rename: all are on the drive of `dest` and
/// none has a namesake there (a merge or a replace handles the items inside one by one).
/// When this is wrong (a folder that is another drive in disguise), the move still works
/// by copying, only without a total to show.
fn moves_by_rename(sources: &[PathBuf], dest: &Path) -> bool {
    #[cfg(unix)]
    let same_drive = {
        use std::os::unix::fs::MetadataExt;
        let Ok(destination) = fs::metadata(dest) else {
            return false;
        };
        if !destination.is_dir() {
            return false;
        }
        let drive = destination.dev();
        move |src: &Path| fs::symlink_metadata(src).is_ok_and(|m| m.dev() == drive)
    };
    // The drive letter or the share: what the path begins with.
    #[cfg(not(unix))]
    let same_drive = {
        if !dest.is_dir() {
            return false;
        }
        let root = |p: &Path| match p.components().next() {
            Some(std::path::Component::Prefix(prefix)) => {
                Some(prefix.as_os_str().to_string_lossy().to_lowercase())
            }
            _ => None,
        };
        let drive = root(dest);
        move |src: &Path| drive.is_some() && root(src) == drive && fs::symlink_metadata(src).is_ok()
    };
    sources.iter().all(|src| {
        same_drive(src)
            && src
                .file_name()
                .is_some_and(|name| !exists_no_follow(&dest.join(name)))
    })
}

/// Progress sink: number of bytes just processed (0 for directory/item events).
type Tick<'a> = dyn FnMut(u64) + 'a;

const CHUNK: usize = 1 << 20;

/// Copies one file. `durable`: the data is on the disk (fsync) before returning, for
/// moves that delete the source next.
fn copy_file(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    tick: &mut Tick,
    durable: bool,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Cancelled);
    }
    #[cfg(target_os = "macos")]
    if clone_apfs(src, dst, cancel, tick, durable)? {
        return Ok(());
    }
    let mut r = fs::File::open(src).ctx("open", src)?;
    let meta = r.metadata().ctx("stat", src)?;
    // Never truncate an existing file: a replaced one went to the trash before.
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    // Unix: created no more open than the source (a private 0600 file is never
    // readable by others while it is being copied).
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        opts.mode((meta.permissions().mode() & 0o777) | 0o200);
    }
    let mut w = opts.open(dst).ctx("create", dst)?;
    let tmp_guard = DeleteOnDrop(Some(dst.to_path_buf()));
    #[cfg(target_os = "linux")]
    let in_kernel = copy_in_kernel(&r, &w, src, cancel, tick)?;
    #[cfg(target_os = "macos")]
    let in_kernel = copy_in_kernel_macos(&r, &w, src, cancel, tick)?;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let in_kernel = false;
    if !in_kernel {
        let mut buf = vec![0u8; CHUNK];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(CoreError::Cancelled);
            }
            let n = r.read(&mut buf).ctx("read", src)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).ctx("write", dst)?;
            tick(n as u64);
        }
    }
    w.flush().ctx("write", dst)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    xattrs::copy_fd(&r, &w);
    let _ = w.set_permissions(meta.permissions());
    if let Ok(m) = meta.modified() {
        let _ = w.set_modified(m);
    }
    if durable {
        w.sync_all().ctx("sync", dst)?;
    }
    tmp_guard.disarm();
    tick(0);
    Ok(())
}

/// macOS: attempts an atomic copy-on-write APFS clone via `clonefile(2)`.
/// Instant and zero extra disk blocks on the same APFS volume while preserving data,
/// mode, timestamps, and extended attributes. Returns `Ok(false)` when cloning is not
/// supported (cross-volume `EXDEV`, non-APFS filesystem `ENOTSUP`, etc.) so the caller
/// falls back to `fcopyfile` / chunked copy. Fails immediately on `EEXIST` so an
/// existing destination is never overwritten.
#[cfg(target_os = "macos")]
fn clone_apfs(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    tick: &mut Tick,
    durable: bool,
) -> Result<bool> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Cancelled);
    }
    // Verify the source is readable before cloning (preserves error semantics for
    // unreadable mode-0000 files where `clonefile` might succeed for the owner).
    let src_file = fs::File::open(src).ctx("open", src)?;
    let meta = src_file.metadata().ctx("stat", src)?;
    drop(src_file);

    let Ok(src_c) = CString::new(src.as_os_str().as_bytes()) else {
        return Ok(false);
    };
    let Ok(dst_c) = CString::new(dst.as_os_str().as_bytes()) else {
        return Ok(false);
    };

    // SAFETY: NUL-terminated path pointers; `flags = 0` follows symlinks on `src`
    // (matching `File::open`) and atomically fails with `EEXIST` if `dst` exists.
    let rc = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EEXIST) {
            return Err(CoreError::io("create", dst, err));
        }
        // Any other failure (EXDEV across volumes, ENOTSUP/EOPNOTSUPP on FAT/ExFAT/SMB/HFS+,
        // EINVAL, EPERM, EACCES, ENOSYS, ELOOP) safely falls back to descriptor copy.
        return Ok(false);
    }

    let tmp_guard = DeleteOnDrop(Some(dst.to_path_buf()));
    if durable {
        let f = fs::File::open(dst).ctx("sync", dst)?;
        f.sync_all().ctx("sync", dst)?;
    }
    tmp_guard.disarm();
    tick(meta.len());
    tick(0);
    Ok(true)
}

/// macOS cross-volume / non-APFS fallback: copies data via `fcopyfile(3)` with a
/// progress and cancellation callback. Returns `Ok(false)` before writing if `fcopyfile`
/// is unsupported so the caller falls back to the chunked read/write loop.
#[cfg(target_os = "macos")]
fn copy_in_kernel_macos(
    r: &fs::File,
    w: &fs::File,
    src: &Path,
    cancel: &AtomicBool,
    tick: &mut Tick,
) -> Result<bool> {
    use std::os::fd::AsRawFd;

    struct Ctx<'a, 'b> {
        cancel: &'a AtomicBool,
        tick: &'a mut Tick<'b>,
        last_copied: i64,
        cancelled: bool,
    }

    extern "C" fn status_cb(
        what: libc::c_int,
        stage: libc::c_int,
        state: libc::copyfile_state_t,
        _src: *const libc::c_char,
        _dst: *const libc::c_char,
        ctx_ptr: *mut libc::c_void,
    ) -> libc::c_int {
        if ctx_ptr.is_null() {
            return libc::COPYFILE_CONTINUE;
        }
        // SAFETY: `ctx_ptr` points to our stack-allocated `Ctx` for the duration of `fcopyfile`.
        let ctx = unsafe { &mut *(ctx_ptr as *mut Ctx<'_, '_>) };
        if ctx.cancel.load(Ordering::Relaxed) {
            ctx.cancelled = true;
            return libc::COPYFILE_QUIT;
        }
        if what == libc::COPYFILE_COPY_DATA && stage == libc::COPYFILE_PROGRESS {
            let mut copied: libc::off_t = 0;
            // SAFETY: `state` is valid during the callback; `COPYFILE_STATE_COPIED` writes `off_t`.
            let rc = unsafe {
                libc::copyfile_state_get(
                    state,
                    libc::COPYFILE_STATE_COPIED as u32,
                    (&mut copied as *mut libc::off_t).cast(),
                )
            };
            if rc == 0 && copied > ctx.last_copied {
                let delta = (copied - ctx.last_copied) as u64;
                ctx.last_copied = copied;
                (ctx.tick)(delta);
            }
        }
        libc::COPYFILE_CONTINUE
    }

    // SAFETY: allocates an opaque `copyfile_state_t` freed unconditionally below.
    let state = unsafe { libc::copyfile_state_alloc() };
    if state.is_null() {
        return Ok(false);
    }
    let mut ctx = Ctx {
        cancel,
        tick,
        last_copied: 0,
        cancelled: false,
    };
    let cb: libc::copyfile_callback_t = Some(status_cb);
    let rc = unsafe {
        libc::copyfile_state_set(
            state,
            libc::COPYFILE_STATE_STATUS_CB as u32,
            cb.map_or(std::ptr::null(), |f| f as *const libc::c_void),
        );
        libc::copyfile_state_set(
            state,
            libc::COPYFILE_STATE_STATUS_CTX as u32,
            (&mut ctx as *mut Ctx<'_, '_>).cast(),
        );
        let rc = libc::fcopyfile(r.as_raw_fd(), w.as_raw_fd(), state, libc::COPYFILE_DATA);
        libc::copyfile_state_free(state);
        rc
    };
    if ctx.cancelled || cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Cancelled);
    }
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if ctx.last_copied == 0
        && matches!(
            err.raw_os_error(),
            Some(libc::ENOTSUP | libc::EOPNOTSUPP | libc::EINVAL | libc::ENOSYS)
        )
    {
        return Ok(false);
    }
    Err(CoreError::io("copy", src, err))
}

/// Linux: copies with `copy_file_range`, in chunks so progress and cancel keep working.
/// The kernel copies without passing the data through Gall, and on btrfs and XFS (and
/// NFS 4.2 / SMB server-side copy) it shares the blocks instead (a reflink): copying a
/// large file on the same drive is instant and takes no extra space. Returns false when
/// the file system cannot do it (another drive, an old kernel), before anything is
/// written, so the caller copies the bytes itself.
#[cfg(target_os = "linux")]
fn copy_in_kernel(
    r: &fs::File,
    w: &fs::File,
    src: &Path,
    cancel: &AtomicBool,
    tick: &mut Tick,
) -> Result<bool> {
    use std::os::fd::AsRawFd;
    const RANGE_CHUNK: usize = 16 << 20;
    let mut copied = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        // SAFETY: two open descriptors; null offsets use (and advance) the file positions.
        let n = unsafe {
            libc::copy_file_range(
                r.as_raw_fd(),
                std::ptr::null_mut(),
                w.as_raw_fd(),
                std::ptr::null_mut(),
                RANGE_CHUNK,
                0,
            )
        };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            let unsupported = matches!(
                err.raw_os_error(),
                Some(libc::EXDEV | libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP | libc::EBADF)
                    | Some(libc::EPERM | libc::ETXTBSY)
            );
            if copied == 0 && unsupported {
                return Ok(false);
            }
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(CoreError::io("copy", src, err));
        }
        if n == 0 {
            // Some virtual files (/proc, /sys) report a size but copy nothing: let the
            // byte loop handle them.
            if copied == 0 && r.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                return Ok(false);
            }
            return Ok(true);
        }
        copied += n as u64;
        tick(n as u64);
    }
}

/// Removes a partially written file if the copy is cancelled or fails.
struct DeleteOnDrop(Option<PathBuf>);
impl DeleteOnDrop {
    fn disarm(mut self) {
        self.0 = None;
    }
}
impl Drop for DeleteOnDrop {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::remove_file(p);
        }
    }
}

/// Copies one file or link as a file operation does (shared blocks where the file system
/// can, times and permissions kept, an existing destination refused), without undo.
pub(super) fn copy_item(src: &Path, dst: &Path, cancel: &AtomicBool) -> Result<()> {
    copy_tree(src, dst, cancel, &mut |_| {})
}

fn copy_tree(src: &Path, dst: &Path, cancel: &AtomicBool, tick: &mut Tick) -> Result<()> {
    copy_tree_with(src, dst, cancel, tick, false)
}

/// Sets a folder's modified time (best effort).
fn set_dir_modified(dir: &Path, t: SystemTime) {
    #[cfg(unix)]
    let f = fs::File::open(dir);
    // Windows opens a folder only with FILE_FLAG_BACKUP_SEMANTICS, and needs
    // FILE_WRITE_ATTRIBUTES to change its times.
    #[cfg(windows)]
    let f = {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .access_mode(0x0100)
            .custom_flags(0x0200_0000)
            .open(dir)
    };
    if let Ok(f) = f {
        let _ = f.set_modified(t);
    }
}

/// macOS: extended attributes (Finder tags and colours, comments, the resource fork) are
/// copied with the file, like Finder does. Best effort: a volume without them (FAT, exFAT,
/// some network shares) just does not get them.
#[cfg(target_os = "macos")]
mod xattrs {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// Set by the system itself and refused (or meaningless) on a copy.
    const SKIPPED: [&[u8]; 2] = [b"com.apple.provenance", b"com.apple.rootless"];

    fn wanted(name: &[u8]) -> bool {
        !SKIPPED.contains(&name)
    }

    /// Calls `f(buf)` with a growing buffer until the value fits.
    fn read(f: impl Fn(&mut [u8]) -> isize) -> Option<Vec<u8>> {
        let n = f(&mut []);
        if n < 0 {
            return None;
        }
        let mut buf = vec![0u8; n as usize + 64];
        let n = f(&mut buf);
        (n >= 0).then(|| {
            buf.truncate(n as usize);
            buf
        })
    }

    pub fn copy_fd(src: &std::fs::File, dst: &std::fs::File) {
        let (s, d) = (src.as_raw_fd(), dst.as_raw_fd());
        // SAFETY: valid descriptors and buffers of the given lengths.
        let Some(names) =
            read(|b| unsafe { libc::flistxattr(s, b.as_mut_ptr().cast(), b.len(), 0) })
        else {
            return;
        };
        for name in names
            .split(|&c| c == 0)
            .filter(|n| !n.is_empty() && wanted(n))
        {
            let Ok(cname) = CString::new(name) else {
                continue;
            };
            let value = read(|b| unsafe {
                libc::fgetxattr(s, cname.as_ptr(), b.as_mut_ptr().cast(), b.len(), 0, 0)
            });
            if let Some(v) = value {
                unsafe { libc::fsetxattr(d, cname.as_ptr(), v.as_ptr().cast(), v.len(), 0, 0) };
            }
        }
    }

    pub fn copy_path(src: &Path, dst: &Path) {
        let (Ok(s), Ok(d)) = (
            CString::new(src.as_os_str().as_bytes()),
            CString::new(dst.as_os_str().as_bytes()),
        ) else {
            return;
        };
        let nofollow = libc::XATTR_NOFOLLOW;
        // SAFETY: NUL-terminated paths and buffers of the given lengths.
        let Some(names) = read(|b| unsafe {
            libc::listxattr(s.as_ptr(), b.as_mut_ptr().cast(), b.len(), nofollow)
        }) else {
            return;
        };
        for name in names
            .split(|&c| c == 0)
            .filter(|n| !n.is_empty() && wanted(n))
        {
            let Ok(cname) = CString::new(name) else {
                continue;
            };
            let value = read(|b| unsafe {
                libc::getxattr(
                    s.as_ptr(),
                    cname.as_ptr(),
                    b.as_mut_ptr().cast(),
                    b.len(),
                    0,
                    nofollow,
                )
            });
            if let Some(v) = value {
                unsafe {
                    libc::setxattr(
                        d.as_ptr(),
                        cname.as_ptr(),
                        v.as_ptr().cast(),
                        v.len(),
                        0,
                        nofollow,
                    )
                };
            }
        }
    }
}

/// Linux extended attributes (`user.*`: download origin, desktop tags and ratings;
/// POSIX ACLs), copied with the file. Best effort: a file system without them (FAT, some
/// network shares) just does not get them.
#[cfg(target_os = "linux")]
mod xattrs {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const COPIED: [&[u8]; 3] = [
        b"user.",
        b"system.posix_acl_access",
        b"system.posix_acl_default",
    ];

    fn wanted(name: &[u8]) -> bool {
        COPIED.iter().any(|p| name.starts_with(p))
    }

    /// Calls `f(buf)` with a growing buffer until the value fits.
    fn read(f: impl Fn(&mut [u8]) -> isize) -> Option<Vec<u8>> {
        let n = f(&mut []);
        if n < 0 {
            return None;
        }
        let mut buf = vec![0u8; n as usize + 64];
        let n = f(&mut buf);
        (n >= 0).then(|| {
            buf.truncate(n as usize);
            buf
        })
    }

    pub fn copy_fd(src: &std::fs::File, dst: &std::fs::File) {
        let (s, d) = (src.as_raw_fd(), dst.as_raw_fd());
        // SAFETY: valid descriptors and buffers of the given lengths.
        let Some(names) = read(|b| unsafe { libc::flistxattr(s, b.as_mut_ptr().cast(), b.len()) })
        else {
            return;
        };
        for name in names
            .split(|&c| c == 0)
            .filter(|n| !n.is_empty() && wanted(n))
        {
            let Ok(cname) = CString::new(name) else {
                continue;
            };
            let value = read(|b| unsafe {
                libc::fgetxattr(s, cname.as_ptr(), b.as_mut_ptr().cast(), b.len())
            });
            if let Some(v) = value {
                unsafe { libc::fsetxattr(d, cname.as_ptr(), v.as_ptr().cast(), v.len(), 0) };
            }
        }
    }

    pub fn copy_path(src: &Path, dst: &Path) {
        let (Ok(s), Ok(d)) = (
            CString::new(src.as_os_str().as_bytes()),
            CString::new(dst.as_os_str().as_bytes()),
        ) else {
            return;
        };
        // SAFETY: NUL-terminated paths and buffers of the given lengths.
        let Some(names) =
            read(|b| unsafe { libc::llistxattr(s.as_ptr(), b.as_mut_ptr().cast(), b.len()) })
        else {
            return;
        };
        for name in names
            .split(|&c| c == 0)
            .filter(|n| !n.is_empty() && wanted(n))
        {
            let Ok(cname) = CString::new(name) else {
                continue;
            };
            let value = read(|b| unsafe {
                libc::lgetxattr(s.as_ptr(), cname.as_ptr(), b.as_mut_ptr().cast(), b.len())
            });
            if let Some(v) = value {
                unsafe {
                    libc::lsetxattr(d.as_ptr(), cname.as_ptr(), v.as_ptr().cast(), v.len(), 0)
                };
            }
        }
    }
}

/// Writes a directory's entries to the disk (Unix; NTFS journals its metadata).
fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .ctx("sync", dir)?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// [`copy_tree`]; `durable` also flushes every file and folder to the disk.
fn copy_tree_with(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    tick: &mut Tick,
    durable: bool,
) -> Result<()> {
    let meta = fs::symlink_metadata(src).ctx("stat", src)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(src).ctx("read link", src)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dst).ctx("symlink", dst)?;
        #[cfg(windows)]
        {
            if src.is_dir() {
                std::os::windows::fs::symlink_dir(&target, dst).ctx("symlink", dst)?;
            } else {
                std::os::windows::fs::symlink_file(&target, dst).ctx("symlink", dst)?;
            }
        }
        tick(0);
    } else if meta.is_dir() {
        // Unix: never more open than the source while the copy fills it (a private 0700
        // folder must not be readable by others half-way); owner access to write into it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            fs::DirBuilder::new()
                .mode((meta.permissions().mode() & 0o777) | 0o700)
                .create(dst)
                .ctx("create folder", dst)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(dst).ctx("create folder", dst)?;
        tick(0);
        for child in fs::read_dir(src).ctx("list", src)? {
            let child = child.ctx("list", src)?;
            copy_tree_with(
                &child.path(),
                &dst.join(child.file_name()),
                cancel,
                tick,
                durable,
            )?;
        }
        // After the contents (adding them changes the time; a read-only folder would
        // refuse them): the source's permissions, attributes and modified time.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        xattrs::copy_path(src, dst);
        #[cfg(unix)]
        let _ = fs::set_permissions(dst, meta.permissions());
        if let Ok(m) = meta.modified() {
            set_dir_modified(dst, m);
        }
        if durable {
            sync_dir(dst)?;
        }
    } else if meta.is_file() {
        copy_file(src, dst, cancel, tick, durable)?;
    } else {
        copy_special(src, dst, &meta)?;
        tick(0);
    }
    Ok(())
}

/// Pipes, sockets and devices (Unix): reading one would wait forever (a pipe) or never
/// end (a device). A named pipe is created again, as `cp -a` does; sockets belong to a
/// running program and devices need root, so they are left out (logged).
fn copy_special(src: &Path, dst: &Path, meta: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        if meta.file_type().is_fifo() {
            let c = std::ffi::CString::new(dst.as_os_str().as_encoded_bytes())
                .map_err(|_| CoreError::InvalidName(dst.to_string_lossy().into_owned()))?;
            // SAFETY: a NUL-terminated path.
            if unsafe {
                libc::mkfifo(
                    c.as_ptr(),
                    (meta.permissions().mode() & 0o777) as libc::mode_t,
                )
            } != 0
            {
                return Err(CoreError::io(
                    "create",
                    dst,
                    std::io::Error::last_os_error(),
                ));
            }
            return Ok(());
        }
    }
    let _ = (meta, dst);
    log::warn!("not copied (a socket or device): {}", src.display());
    Ok(())
}

fn is_cross_device(e: &std::io::Error) -> bool {
    // EXDEV = 18 on Linux/macOS; ERROR_NOT_SAME_DEVICE = 17 on Windows.
    e.kind() == std::io::ErrorKind::CrossesDevices
        || matches!(e.raw_os_error(), Some(18) if cfg!(unix))
        || matches!(e.raw_os_error(), Some(17) if cfg!(windows))
}

fn remove_any(p: &Path) -> Result<()> {
    let _released = release([p]);
    let m = fs::symlink_metadata(p).ctx("stat", p)?;
    if m.is_dir() {
        fs::remove_dir_all(p).ctx("delete", p)
    } else {
        fs::remove_file(p).ctx("delete", p)
    }
}

/// Copies `src` to the new path `dst`. When the copy fails or is cancelled, the partial
/// copy is removed (unless `dst` was there before and the copy never started).
fn copy_fresh(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    tick: &mut Tick,
    durable: bool,
) -> Result<()> {
    if exists_no_follow(dst) {
        return Err(CoreError::AlreadyExists(dst.to_path_buf()));
    }
    // Both inside WSL: Linux copies (faster; permissions, owners and links stay).
    if let Some(done) = super::wsl::copy_inside_linux(src, dst, cancel, tick, durable) {
        return done;
    }
    copy_tree_with(src, dst, cancel, tick, durable).inspect_err(|err| {
        let foreign = matches!(err, CoreError::Io { path, source, .. }
            if path == dst && source.kind() == std::io::ErrorKind::AlreadyExists);
        if !foreign {
            let _ = remove_any(dst);
        }
    })
}

struct MoveFailure {
    error: CoreError,
    /// `Some(true)` means the full source is intact alongside the copy; `Some(false)`
    /// means recovery could not prove that, so undo must keep both paths protected.
    recovered_source: Option<bool>,
}

fn move_one_with_ops(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    tick: &mut Tick,
    rename_op: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
    remove_source: &dyn Fn(&Path) -> Result<()>,
) -> std::result::Result<(), MoveFailure> {
    let _released = release([src]);
    match rename_op(src, dst) {
        Ok(()) => {
            moved(src, dst);
            tick(0);
            Ok(())
        }
        Err(e) if is_cross_device(&e) => {
            // Another drive: copy, make sure the copy is on the disk, only then delete the
            // source. A failed or cancelled copy is removed; the source stays untouched.
            copy_fresh(src, dst, cancel, tick, true).map_err(|error| MoveFailure {
                error,
                recovered_source: None,
            })?;
            if let Some(parent) = dst.parent() {
                if let Err(error) = sync_dir(parent) {
                    return Err(MoveFailure {
                        error,
                        recovered_source: Some(true),
                    });
                }
            }
            if let Err(error) = remove_source(src) {
                if !exists_no_follow(src) {
                    moved(src, dst);
                    return Ok(());
                }
                let recovered_source = restore_missing_from_copy(dst, src).is_ok();
                return Err(MoveFailure {
                    error,
                    recovered_source: Some(recovered_source),
                });
            }
            moved(src, dst);
            Ok(())
        }
        Err(e) => Err(MoveFailure {
            error: CoreError::io("move", src, e),
            recovered_source: None,
        }),
    }
}

/// Fills only missing entries of a partially removed source from its completed copy.
/// Existing source entries are never overwritten.
fn restore_missing_from_copy(copy: &Path, source: &Path) -> Result<()> {
    if !exists_no_follow(source) {
        copy_fresh(copy, source, &AtomicBool::new(false), &mut |_| {}, true)?;
        return Ok(());
    }
    let copy_meta = fs::symlink_metadata(copy).ctx("stat", copy)?;
    let source_meta = fs::symlink_metadata(source).ctx("stat", source)?;
    if copy_meta.file_type().is_symlink() || source_meta.file_type().is_symlink() {
        if copy_meta.file_type().is_symlink()
            && source_meta.file_type().is_symlink()
            && fs::read_link(copy).ctx("read link", copy)?
                == fs::read_link(source).ctx("read link", source)?
        {
            return Ok(());
        }
        return Err(CoreError::Other(format!(
            "cannot safely restore conflicting entry {}",
            quoted(source)
        )));
    }
    if copy_meta.is_file() || source_meta.is_file() {
        if copy_meta.is_file() && source_meta.is_file() && files_equal(copy, source)? {
            return Ok(());
        }
        return Err(CoreError::Other(format!(
            "cannot safely restore conflicting entry {}",
            quoted(source)
        )));
    }
    if !copy_meta.is_dir() || !source_meta.is_dir() {
        return Err(CoreError::Other(format!(
            "cannot safely restore conflicting entry {}",
            quoted(source)
        )));
    }
    let mut failure = None;
    for entry in fs::read_dir(copy).ctx("list", copy)? {
        let entry = entry.ctx("list", copy)?;
        let from = entry.path();
        let to = source.join(entry.file_name());
        let result = if !exists_no_follow(&to) {
            copy_fresh(&from, &to, &AtomicBool::new(false), &mut |_| {}, true)
        } else {
            restore_missing_from_copy(&from, &to)
        };
        if let Err(error) = result {
            failure.get_or_insert(error);
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    let metadata = fs::metadata(copy).ctx("stat", copy)?;
    fs::set_permissions(source, metadata.permissions()).ctx("permissions", source)?;
    if let Ok(modified) = metadata.modified() {
        set_dir_modified(source, modified);
    }
    sync_dir(source)?;
    Ok(())
}

/// True when `inner` is `outer` or lies inside it (also through symlinks).
fn contains(outer: &Path, inner: &Path) -> bool {
    match (outer.canonicalize(), inner.canonicalize()) {
        (Ok(o), Ok(i)) => i.starts_with(o),
        _ => inner.starts_with(outer),
    }
}

/// A folder itself, not a symlink to one: only real folders are merged.
fn real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// True when `src` was modified later than `dst` (2 s slack: FAT stores even seconds).
fn newer(src: &Path, dst: &Path) -> bool {
    let mtime = |p: &Path| fs::symlink_metadata(p).and_then(|m| m.modified()).ok();
    match (mtime(src), mtime(dst)) {
        (Some(a), Some(b)) => a
            .duration_since(b)
            .is_ok_and(|d| d > Duration::from_secs(2)),
        (Some(_), None) => true,
        _ => false,
    }
}

/// What a copy or move did, for undo and the "done" message.
#[derive(Default)]
struct Placed {
    created: Vec<PathBuf>,
    moved: Vec<(PathBuf, PathBuf)>,
    /// Items replaced: moved to the trash first, and when.
    replaced: Vec<(PathBuf, (i64, i64))>,
    /// Existing folders that items were merged into.
    merged: Vec<PathBuf>,
    /// Source folders a merging move emptied and removed.
    emptied: Vec<PathBuf>,
    skipped: u64,
}

/// Copies or moves items into a folder, resolving name conflicts.
struct Transfer<'a> {
    kind: TaskKind,
    conflict: ConflictPolicy,
    merge: bool,
    cancel: &'a AtomicBool,
}

impl Transfer<'_> {
    /// Copies or moves `src` into `dest_dir`. Returns false when it was skipped.
    fn place(
        &self,
        src: &Path,
        dest_dir: &Path,
        out: &mut Placed,
        tick: &mut Tick,
    ) -> Result<bool> {
        let rename_op = |from: &Path, to: &Path| fs::rename(from, to);
        self.place_with_ops(src, dest_dir, out, tick, &rename_op, &remove_any)
    }

    fn place_with_ops(
        &self,
        src: &Path,
        dest_dir: &Path,
        out: &mut Placed,
        tick: &mut Tick,
        rename_op: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
        remove_source: &dyn Fn(&Path) -> Result<()>,
    ) -> Result<bool> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        // Any name, also one that is not valid UTF-8 (found inside merged folders).
        let name = src
            .file_name()
            .ok_or_else(|| CoreError::InvalidName(src.to_string_lossy().into_owned()))?;
        let mut target = dest_dir.join(name);
        // Also through a symlink: `target` is then the source itself.
        let same_place = in_dir(src, dest_dir);
        let mut replaced_here = false;
        if exists_no_follow(&target) {
            if self.kind == TaskKind::Move && same_place {
                out.skipped += 1; // moving onto itself is a no-op
                return Ok(false);
            }
            if self.merge && !same_place && real_dir(src) && real_dir(&target) {
                return self.merge_into(src, &target, out, tick);
            }
            let policy = match self.conflict {
                ConflictPolicy::OverwriteOlder if newer(src, &target) => ConflictPolicy::Overwrite,
                ConflictPolicy::OverwriteOlder => ConflictPolicy::Skip,
                p => p,
            };
            // What this operation put there itself (two sources with one name, from search
            // results) is never replaced: the first item would go to the trash.
            let ours = out.created.iter().any(|c| same_path_ci(c, &target))
                || out.moved.iter().any(|(_, t)| same_path_ci(t, &target));
            let policy = match policy {
                ConflictPolicy::Overwrite | ConflictPolicy::OverwritePermanently if ours => {
                    ConflictPolicy::KeepBoth
                }
                p => p,
            };
            // Replacing a folder that holds the source would remove the source with it
            // (copying ~/Photos/Photos to ~ and replacing ~/Photos).
            let replacing = matches!(
                policy,
                ConflictPolicy::Overwrite | ConflictPolicy::OverwritePermanently
            );
            if replacing && !same_place && contains(&target, src) {
                return Err(CoreError::Other(format!(
                    "{} cannot be replaced: it contains {}",
                    quoted(&target),
                    quoted(src)
                )));
            }
            match (policy, same_place) {
                (_, true) | (ConflictPolicy::KeepBoth, _) => {
                    // A name that is not UTF-8 keeps both under a readable name.
                    target = unique_path(dest_dir, &name.to_string_lossy())
                }
                (ConflictPolicy::Skip | ConflictPolicy::OverwriteOlder, _) => {
                    out.skipped += 1;
                    return Ok(false);
                }
                (ConflictPolicy::Overwrite, _) => {
                    // The replaced item goes to the trash, so undo brings it back.
                    let when = trash_timed(std::slice::from_ref(&target))
                        .map_err(|e| no_trash_to_replace(&target, e))?;
                    out.replaced.push((target.clone(), when));
                    replaced_here = true;
                }
                // Chosen after the trash failed (see `no_trash_to_replace`).
                (ConflictPolicy::OverwritePermanently, _) => remove_any(&target)?,
            }
        }
        let res = if self.kind == TaskKind::Copy {
            copy_fresh(src, &target, self.cancel, tick, false)
        } else {
            move_one_with_ops(src, &target, self.cancel, tick, rename_op, remove_source).map_err(
                |failure| {
                    if let Some(source_recovered) = failure.recovered_source {
                        if source_recovered {
                            out.created.push(target.clone());
                        } else {
                            out.moved.push((src.to_path_buf(), target.clone()));
                        }
                    }
                    failure.error
                },
            )
        };
        if let Err(e) = res {
            // Cancelled or failed after the old item went to the trash: put it back, so
            // the destination is not left with neither version. Where that fails, undo
            // can still restore it.
            if replaced_here {
                let (p, (at, until)) = out.replaced.pop().expect("checked");
                if restore_from_trash(std::slice::from_ref(&p), at, until).is_err() {
                    out.replaced.push((p, (at, until)));
                }
            }
            return Err(e);
        }
        if self.kind == TaskKind::Copy {
            out.created.push(target);
        } else {
            out.moved.push((src.to_path_buf(), target));
        }
        Ok(true)
    }

    /// Places the contents of folder `src` into the existing folder `target`. A move
    /// removes `src` when it is empty afterwards (skipped items keep it).
    fn merge_into(
        &self,
        src: &Path,
        target: &Path,
        out: &mut Placed,
        tick: &mut Tick,
    ) -> Result<bool> {
        let merge_index = out.merged.len();
        out.merged.push(target.to_path_buf());
        let mut children: Vec<PathBuf> = fs::read_dir(src)
            .ctx("list", src)?
            .map(|e| e.map(|e| e.path()).ctx("list", src))
            .collect::<Result<_>>()?;
        children.sort();
        let mut changed = false;
        for child in &children {
            changed |= self.place(child, target, out, tick)?;
        }
        tick(0); // the folder itself, as counted by `measure`
        if self.kind == TaskKind::Move && fs::remove_dir(src).is_ok() {
            changed = true;
            out.emptied.push(src.to_path_buf());
        }
        if !changed {
            out.merged.truncate(merge_index);
        }
        Ok(changed)
    }
}

/// The undo step for a copy or move that replaced or merged: removing the copies (or
/// moving the items back) first, then restoring what was replaced.
fn transfer_batch(
    kind: TaskKind,
    sources: &[PathBuf],
    created: Vec<PathBuf>,
    moved: Vec<(PathBuf, PathBuf)>,
    replaced: Vec<(PathBuf, (i64, i64))>,
    merged: Vec<PathBuf>,
    emptied: Vec<PathBuf>,
) -> UndoRecord {
    let mut label = format!(
        "{} {}",
        if kind == TaskKind::Move {
            "move"
        } else {
            "copy"
        },
        what(sources)
    );
    if !merged.is_empty() {
        label.push_str(" (merged)");
    }
    if !replaced.is_empty() {
        let one = replaced.len() == 1 && sources.len() == 1 && merged.is_empty();
        label.push_str(&format!(
            ", replacing {}",
            if one {
                "the old one".to_string()
            } else {
                items(replaced.len())
            }
        ));
    }
    let mut records: Vec<UndoRecord> = replaced
        .into_iter()
        .map(|(p, (at, until))| UndoRecord::Trashed {
            paths: vec![p],
            at,
            until,
        })
        .collect();
    if !created.is_empty() {
        records.push(UndoRecord::Created { paths: created });
    }
    if !moved.is_empty() {
        records.push(UndoRecord::Moved { pairs: moved });
    }
    // Last, so undo recreates the emptied folders first.
    if !emptied.is_empty() {
        records.push(UndoRecord::RemovedDirs { dirs: emptied });
    }
    UndoRecord::Batch {
        label,
        records,
        dirs: merged,
    }
}

/// What a copy or move into `dest` would run into, for the conflict dialog.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictInfo {
    /// Items in `dest` with the name of a source (see [`find_conflicts`]).
    pub paths: Vec<PathBuf>,
    /// How many of them are folders on both sides (they can be merged).
    pub folders: usize,
    /// Files that would still exist on both sides when the folders are merged: the
    /// conflicting files, plus those inside the merged folders.
    pub files_when_merged: usize,
    /// The first few of those, for the dialog.
    pub sample: Vec<PathBuf>,
}

pub fn conflict_info(sources: &[PathBuf], dest: &Path) -> ConflictInfo {
    fn walk(src: &Path, dst: &Path, info: &mut ConflictInfo) {
        if real_dir(src) && real_dir(dst) {
            let Ok(rd) = fs::read_dir(src) else { return };
            for e in rd.flatten() {
                let t = dst.join(e.file_name());
                if exists_no_follow(&t) {
                    walk(&e.path(), &t, info);
                }
            }
        } else {
            info.files_when_merged += 1;
            if info.sample.len() < 5 {
                info.sample.push(dst.to_path_buf());
            }
        }
    }
    let paths = find_conflicts(sources, dest);
    let mut info = ConflictInfo::default();
    for s in sources.iter().filter(|s| !in_dir(s, dest)) {
        let Some(name) = s.file_name() else { continue };
        let t = dest.join(name);
        if !exists_no_follow(&t) {
            continue;
        }
        if real_dir(s) && real_dir(&t) {
            info.folders += 1;
        }
        walk(s, &t, &mut info);
    }
    info.paths = paths;
    info
}

pub struct TaskHandle {
    pub id: u64,
    cancel: AtomicBool,
}

pub type EventFn = dyn Fn(TaskEvent) + Send + Sync + 'static;

/// Runs file tasks on worker threads and reports progress through a callback.
pub struct TaskManager {
    next_id: AtomicU64,
    running: Mutex<Vec<Arc<TaskHandle>>>,
    journal: Arc<UndoJournal>,
    on_event: Arc<EventFn>,
}

impl TaskManager {
    pub fn new(journal: Arc<UndoJournal>, on_event: Arc<EventFn>) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            running: Mutex::new(Vec::new()),
            journal,
            on_event,
        }
    }

    pub fn journal(&self) -> &Arc<UndoJournal> {
        &self.journal
    }

    pub fn cancel(&self, id: u64) -> bool {
        let r = self.running.lock().unwrap();
        r.iter()
            .find(|h| h.id == id)
            .map(|h| h.cancel.store(true, Ordering::Relaxed))
            .is_some()
    }

    pub fn running_ids(&self) -> Vec<u64> {
        self.running.lock().unwrap().iter().map(|h| h.id).collect()
    }

    /// Starts a task on a new thread and returns its id immediately.
    pub fn start(self: &Arc<Self>, req: TaskRequest) -> Result<u64> {
        if req.kind == TaskKind::Compress && (req.archive_name.is_none() || req.format.is_none()) {
            return Err(CoreError::Other("archive name or format missing".into()));
        }
        if !matches!(req.kind, TaskKind::Delete | TaskKind::Duplicate) {
            let dest = req
                .dest
                .as_ref()
                .ok_or_else(|| CoreError::Other("destination missing".into()))?;
            if !dest.is_dir() {
                return Err(CoreError::NotFound(dest.clone()));
            }
            for s in req
                .sources
                .iter()
                .filter(|_| matches!(req.kind, TaskKind::Copy | TaskKind::Move))
            {
                ensure_not_into_itself(
                    if req.kind == TaskKind::Copy {
                        "copy"
                    } else {
                        "move"
                    },
                    s,
                    dest,
                )?;
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let handle = Arc::new(TaskHandle {
            id,
            cancel: AtomicBool::new(false),
        });
        self.running.lock().unwrap().push(handle.clone());
        let me = self.clone();
        std::thread::Builder::new()
            .name(format!("task-{id}"))
            .spawn(move || {
                let kind = req.kind;
                let file_name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
                let mut name = match req.sources.as_slice() {
                    [one] => file_name(one),
                    _ => None,
                };
                let to = req
                    .dest
                    .as_deref()
                    .and_then(|d| file_name(d).or_else(|| Some(d.display().to_string())));
                let (outcome, undo_id) = me.run_named(&handle, req);
                let ev = match outcome {
                    Ok((items, skipped, output)) => TaskEvent::Done {
                        id,
                        kind,
                        items,
                        skipped,
                        name: output.or(name.take()),
                        to,
                        undo_id,
                    },
                    Err(CoreError::Cancelled) => TaskEvent::Cancelled { id, kind, undo_id },
                    Err(e) => TaskEvent::Failed {
                        id,
                        kind,
                        error: e.to_string(),
                        undo_id,
                    },
                };
                me.running.lock().unwrap().retain(|h| h.id != id);
                (me.on_event)(ev);
            })
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(id)
    }

    /// Runs a task on the calling thread: (items done, items skipped).
    pub fn run(&self, handle: &TaskHandle, req: TaskRequest) -> Result<(u64, u64)> {
        self.run_named(handle, req).0.map(|(d, s, _)| (d, s))
    }

    /// Like [`run`](Self::run), plus the name of the single item created
    /// (the archive, or the extracted folder), and the stable undo record ID if any.
    #[allow(clippy::type_complexity)]
    fn run_named(
        &self,
        handle: &TaskHandle,
        req: TaskRequest,
    ) -> (Result<(u64, u64, Option<String>)>, Option<u64>) {
        // Names inside WSL that Windows cannot hold or address: refused before anything.
        if let Err(err) =
            super::wsl::check_task(req.kind, &req.sources, req.dest.as_deref(), &handle.cancel)
        {
            return (Err(err), None);
        }
        let totals = match req.kind {
            TaskKind::Compress => Totals {
                bytes: super::archive::source_bytes(&req.sources),
                items: 1,
            },
            TaskKind::Extract => Totals {
                bytes: req
                    .sources
                    .iter()
                    .map(|s| s.metadata().map(|m| m.len()).unwrap_or(0))
                    .sum(),
                items: req.sources.len() as u64,
            },
            // Nothing is read or written per file: the progress counts the items
            // themselves, and the tree is not walked first (in a large tree that walk
            // took longer than the operation, could not be cancelled and showed nothing).
            TaskKind::Delete => Totals {
                bytes: 0,
                items: req.sources.len() as u64,
            },
            TaskKind::Move
                if req
                    .dest
                    .as_deref()
                    .is_some_and(|dest| moves_by_rename(&req.sources, dest)) =>
            {
                Totals {
                    bytes: 0,
                    items: req.sources.len() as u64,
                }
            }
            _ => measure(&req.sources),
        };
        let mut progress = TaskProgress {
            id: handle.id,
            kind: req.kind,
            done_bytes: 0,
            total_bytes: totals.bytes,
            done_items: 0,
            total_items: totals.items,
            current: String::new(),
        };
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let on_event = self.on_event.clone();
        let cancel = &handle.cancel;
        let mut done_top = 0u64;
        let mut created: Vec<PathBuf> = Vec::new();
        // Copies and moves (with what they replaced or merged into).
        let mut placed = Placed::default();
        let transfer = Transfer {
            kind: req.kind,
            conflict: req.conflict,
            merge: req.merge,
            cancel,
        };
        // Archives trashed after extraction, and when (for undo).
        let mut trashed: Vec<PathBuf> = Vec::new();
        let trashed_at = chrono::Utc::now().timestamp_millis();

        let result = (|| -> Result<()> {
            if req.kind == TaskKind::Compress {
                let dest_dir = req.dest.as_ref().expect("validated");
                let name = req.archive_name.as_deref().expect("validated");
                validate_name(name)?;
                progress.current = name.to_string();
                let mut tick = |bytes: u64| {
                    progress.done_bytes += bytes;
                    if last_emit.elapsed() >= Duration::from_millis(100) {
                        last_emit = Instant::now();
                        on_event(TaskEvent::Progress(progress.clone()));
                    }
                };
                let out = super::archive::compress(
                    &req.sources,
                    &dest_dir.join(name),
                    req.format.expect("validated"),
                    cancel,
                    &mut tick,
                )?;
                created.push(out);
                done_top = 1;
                return Ok(());
            }
            let dup_targets = if req.kind == TaskKind::Duplicate {
                duplicate_targets(&req.sources)
            } else {
                Vec::new()
            };
            for (i, src) in req.sources.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(CoreError::Cancelled);
                }
                progress.current = src.to_string_lossy().into_owned();
                let mut tick = |bytes: u64| {
                    progress.done_bytes += bytes;
                    if bytes == 0 {
                        progress.done_items += 1;
                    }
                    if last_emit.elapsed() >= Duration::from_millis(100) {
                        last_emit = Instant::now();
                        on_event(TaskEvent::Progress(progress.clone()));
                    }
                };
                match req.kind {
                    TaskKind::Delete => {
                        remove_any(src)?;
                        tick(0);
                    }
                    TaskKind::Compress => unreachable!("handled above"),
                    TaskKind::Duplicate => {
                        let target = &dup_targets[i];
                        copy_fresh(src, target, cancel, &mut tick, false)?;
                        created.push(target.clone());
                    }
                    TaskKind::Extract => {
                        let dest_dir = req.dest.as_ref().expect("validated");
                        let mut bytes = |n: u64| tick(n);
                        created.extend(super::archive::extract(
                            src,
                            dest_dir,
                            req.extract_mode,
                            cancel,
                            &mut bytes,
                        )?);
                        if req.trash_archive {
                            trash_paths(std::slice::from_ref(src))?;
                            trashed.push(src.clone());
                        }
                        tick(0);
                    }
                    TaskKind::Copy | TaskKind::Move => {
                        let dest_dir = req.dest.as_ref().expect("validated");
                        if !transfer.place(src, dest_dir, &mut placed, &mut tick)? {
                            continue; // skipped
                        }
                    }
                }
                done_top += 1;
            }
            Ok(())
        })();

        let output = match created.as_slice() {
            [one]
                if matches!(
                    req.kind,
                    TaskKind::Compress | TaskKind::Extract | TaskKind::Duplicate
                ) =>
            {
                one.file_name().map(|n| n.to_string_lossy().into_owned())
            }
            _ => None,
        };
        // Record whatever completed, even on failure/cancel, as one atomic undo step so a
        // notification can target this exact task by its stable record ID.
        let mut step_records: Vec<UndoRecord> = Vec::new();
        if !trashed.is_empty() {
            // Extract and trash: one undo restores the archives and removes what was unpacked.
            let label = format!("extract and delete {}", what(&trashed));
            step_records.push(UndoRecord::Batch {
                label,
                dirs: Vec::new(),
                records: vec![
                    UndoRecord::Created {
                        paths: std::mem::take(&mut created),
                    },
                    UndoRecord::Trashed {
                        paths: trashed,
                        at: trashed_at,
                        until: chrono::Utc::now().timestamp_millis(),
                    },
                ],
            });
        }
        created.append(&mut placed.created);
        let Placed {
            moved,
            replaced,
            merged,
            emptied,
            skipped,
            ..
        } = placed;
        if !replaced.is_empty() || !merged.is_empty() {
            // One undo step: the copies are removed (or moved back), then the replaced
            // items come back from the trash.
            step_records.push(transfer_batch(
                req.kind,
                &req.sources,
                created,
                moved,
                replaced,
                merged,
                emptied,
            ));
        } else {
            if !created.is_empty() {
                step_records.push(UndoRecord::Created { paths: created });
            }
            if !moved.is_empty() {
                step_records.push(UndoRecord::Moved { pairs: moved });
            }
        }
        let undo_id = match step_records.len() {
            0 => None,
            1 => Some(
                self.journal
                    .push(step_records.pop().expect("single record")),
            ),
            _ => Some(self.journal.push(UndoRecord::Batch {
                label: format!(
                    "{} {}",
                    if req.kind == TaskKind::Move {
                        "move"
                    } else {
                        "copy"
                    },
                    what(&req.sources)
                ),
                records: step_records,
                dirs: Vec::new(),
            })),
        };
        (result.map(|_| (done_top, skipped, output)), undo_id)
    }
}

/// Ctrl+D duplicate: copy next to the original with a unique name. (O-05)
pub fn duplicate(path: &Path, journal: &UndoJournal) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::NotFound(path.into()))?;
    let target = unique_path(parent, file_name(path)?);
    copy_tree(path, &target, &AtomicBool::new(false), &mut |_| {})?;
    journal.push(UndoRecord::Created {
        paths: vec![target.clone()],
    });
    Ok(target)
}

/// Seconds since epoch for a `SystemTime` (helper for callers / tests).
pub fn epoch_secs(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Test helpers shared by the crate's tests.
#[cfg(test)]
pub(crate) mod test_support {
    #[cfg_attr(target_os = "macos", allow(unused_imports))]
    use super::*;

    /// A test's temporary folder. On freedesktop Unix, unit tests run against a per-test
    /// isolated trash directory inside a disposable tempdir so parallel tests and chroots
    /// never touch or race on the user's real trash (integration tests such as `undo_fuzz`
    /// and production builds are unaffected). On Windows, items sent to the Recycle Bin
    /// from this folder are purged on drop.
    pub(crate) struct TestDir {
        dir: tempfile::TempDir,
        #[cfg(all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        ))]
        _trash: isolated_trash::Guard,
        #[cfg(windows)]
        trashed_before: u64,
    }

    impl std::ops::Deref for TestDir {
        type Target = tempfile::TempDir;
        fn deref(&self) -> &tempfile::TempDir {
            &self.dir
        }
    }

    #[cfg(windows)]
    impl Drop for TestDir {
        fn drop(&mut self) {
            if TRASHED.load(Ordering::Relaxed) != self.trashed_before {
                purge_trashed_from(self.dir.path());
            }
        }
    }

    pub(crate) fn test_dir() -> TestDir {
        TestDir {
            #[cfg(windows)]
            trashed_before: TRASHED.load(Ordering::Relaxed),
            #[cfg(all(
                unix,
                not(target_os = "macos"),
                not(target_os = "ios"),
                not(target_os = "android")
            ))]
            _trash: isolated_trash::Guard::enter(),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    #[cfg(all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    ))]
    pub(super) fn trash_into_isolated(paths: &[PathBuf]) -> Option<Result<()>> {
        isolated_trash::trash_paths(paths)
    }

    #[cfg(all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    ))]
    pub(super) fn list_isolated_trash() -> Option<Result<Vec<trash::TrashItem>>> {
        isolated_trash::list()
    }

    #[cfg(all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    ))]
    pub(super) fn isolated_trash_root() -> Option<PathBuf> {
        isolated_trash::current_root()
    }

    #[cfg(all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    ))]
    mod isolated_trash {
        use super::*;
        use std::cell::{OnceCell, RefCell};
        use std::ffi::{OsStr, OsString};
        use std::io::Write;
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        use std::path::Component;
        use std::rc::Rc;

        thread_local! {
            static STACK: RefCell<Vec<Rc<OnceCell<tempfile::TempDir>>>> = const { RefCell::new(Vec::new()) };
        }

        pub(super) struct Guard {
            slot: Rc<OnceCell<tempfile::TempDir>>,
        }

        impl Guard {
            pub(super) fn enter() -> Self {
                let slot = Rc::new(OnceCell::new());
                STACK.with(|s| s.borrow_mut().push(Rc::clone(&slot)));
                Self { slot }
            }
        }

        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = STACK.try_with(|s| {
                    let mut stack = s.borrow_mut();
                    if let Some(idx) = stack.iter().rposition(|x| Rc::ptr_eq(x, &self.slot)) {
                        stack.remove(idx);
                    }
                });
            }
        }

        fn current() -> Option<Rc<OnceCell<tempfile::TempDir>>> {
            STACK.with(|s| s.borrow().last().cloned())
        }

        pub(super) fn current_root() -> Option<PathBuf> {
            current().and_then(|slot| slot.get().map(|d| d.path().to_path_buf()))
        }

        fn encode_uri_path(path: &Path) -> String {
            let mut out = String::new();
            for component in path.components() {
                match component {
                    Component::RootDir => out.push('/'),
                    Component::Normal(part) => {
                        if !out.is_empty() && !out.ends_with('/') {
                            out.push('/');
                        }
                        for &b in part.as_bytes() {
                            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                                out.push(b as char);
                            } else {
                                use std::fmt::Write as _;
                                let _ = write!(out, "%{b:02X}");
                            }
                        }
                    }
                    _ => {}
                }
            }
            out
        }

        fn hex_nibble(b: u8) -> Option<u8> {
            match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            }
        }

        fn decode_uri_path(raw: &str) -> PathBuf {
            let bytes = raw.as_bytes();
            let mut decoded = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' && i + 2 < bytes.len() {
                    if let (Some(hi), Some(lo)) =
                        (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2]))
                    {
                        decoded.push((hi << 4) | lo);
                        i += 3;
                        continue;
                    }
                }
                decoded.push(bytes[i]);
                i += 1;
            }
            PathBuf::from(OsString::from_vec(decoded))
        }

        fn canonicalize_parent_keep_leaf(path: &Path) -> Result<PathBuf> {
            if path.as_os_str().is_empty() {
                return Err(CoreError::Trash("empty path".into()));
            }
            let target = if path.is_relative() {
                std::env::current_dir()
                    .map_err(|e| CoreError::Trash(e.to_string()))?
                    .join(path)
            } else {
                path.to_path_buf()
            };
            let parent = target
                .parent()
                .ok_or_else(|| CoreError::Trash("cannot trash root directory".into()))?;
            let name = target
                .file_name()
                .ok_or_else(|| CoreError::Trash("cannot trash path without file name".into()))?;
            let canonical_parent = fs::canonicalize(parent)
                .map_err(|e| CoreError::Trash(format!("{}: {e}", parent.display())))?;
            Ok(canonical_parent.join(name))
        }

        fn move_into_trash(src: &Path, dst: &Path) -> std::io::Result<()> {
            match fs::rename(src, dst) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
                    let meta = fs::symlink_metadata(src)?;
                    if meta.file_type().is_symlink() {
                        let target = fs::read_link(src)?;
                        std::os::unix::fs::symlink(target, dst)?;
                        fs::remove_file(src)
                    } else if meta.is_dir() {
                        copy_dir_all_for_trash(src, dst)?;
                        fs::remove_dir_all(src)
                    } else {
                        fs::copy(src, dst)?;
                        fs::remove_file(src)
                    }
                }
                Err(e) => Err(e),
            }
        }

        fn copy_dir_all_for_trash(src: &Path, dst: &Path) -> std::io::Result<()> {
            fs::create_dir_all(dst)?;
            for entry in fs::read_dir(src)? {
                let entry = entry?;
                let src_path = entry.path();
                let dst_path = dst.join(entry.file_name());
                let ft = entry.file_type()?;
                if ft.is_dir() {
                    copy_dir_all_for_trash(&src_path, &dst_path)?;
                } else if ft.is_symlink() {
                    std::os::unix::fs::symlink(fs::read_link(&src_path)?, &dst_path)?;
                } else {
                    fs::copy(&src_path, &dst_path)?;
                }
            }
            Ok(())
        }

        pub(super) fn trash_paths(paths: &[PathBuf]) -> Option<Result<()>> {
            let slot = current()?;
            Some(trash_paths_in(&slot, paths))
        }

        fn trash_paths_in(slot: &OnceCell<tempfile::TempDir>, paths: &[PathBuf]) -> Result<()> {
            let full_paths: Vec<PathBuf> = paths
                .iter()
                .map(|p| {
                    let full = canonicalize_parent_keep_leaf(p)?;
                    fs::symlink_metadata(&full)
                        .map_err(|e| CoreError::Trash(format!("{}: {e}", full.display())))?;
                    Ok(full)
                })
                .collect::<Result<Vec<_>>>()?;

            let root = slot
                .get_or_init(|| tempfile::tempdir().expect("isolated test trash tempdir"))
                .path();
            let files_dir = root.join("files");
            let info_dir = root.join("info");
            fs::create_dir_all(&files_dir).map_err(|e| CoreError::Trash(e.to_string()))?;
            fs::create_dir_all(&info_dir).map_err(|e| CoreError::Trash(e.to_string()))?;

            for full in full_paths {
                let filename = full
                    .file_name()
                    .ok_or_else(|| CoreError::Trash("missing file name".into()))?;
                let mut suffix = 0usize;
                loop {
                    suffix += 1;
                    let mut in_trash_name = filename.to_os_string();
                    if suffix > 1 {
                        in_trash_name.push(format!(".{suffix}"));
                    }
                    let mut info_name = in_trash_name.clone();
                    info_name.push(".trashinfo");
                    let info_path = info_dir.join(&info_name);
                    let mut info_file = match fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&info_path)
                    {
                        Ok(f) => f,
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(e) => return Err(CoreError::Trash(e.to_string())),
                    };
                    let now = chrono::Local::now();
                    let encoded = encode_uri_path(&full);
                    let write_res = writeln!(info_file, "[Trash Info]")
                        .and_then(|_| writeln!(info_file, "Path={encoded}"))
                        .and_then(|_| {
                            writeln!(
                                info_file,
                                "DeletionDate={}",
                                now.format("%Y-%m-%dT%H:%M:%S")
                            )
                        })
                        .and_then(|_| info_file.flush());
                    drop(info_file);
                    if let Err(e) = write_res {
                        let _ = fs::remove_file(&info_path);
                        return Err(CoreError::Trash(e.to_string()));
                    }
                    let stored_path = files_dir.join(&in_trash_name);
                    if exists_no_follow(&stored_path) {
                        let _ = fs::remove_file(&info_path);
                        continue;
                    }
                    if let Err(e) = move_into_trash(&full, &stored_path) {
                        let _ = fs::remove_file(&info_path);
                        return Err(CoreError::Trash(e.to_string()));
                    }
                    break;
                }
            }
            Ok(())
        }

        pub(super) fn list() -> Option<Result<Vec<trash::TrashItem>>> {
            let slot = current()?;
            let Some(dir) = slot.get() else {
                return Some(Ok(Vec::new()));
            };
            Some(list_in(dir.path()))
        }

        fn list_in(root: &Path) -> Result<Vec<trash::TrashItem>> {
            let info_dir = root.join("info");
            let entries = match fs::read_dir(&info_dir) {
                Ok(d) => d,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(CoreError::Trash(e.to_string())),
            };
            let mut items = Vec::new();
            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let info_path = entry.path();
                if info_path.extension() != Some(OsStr::new("trashinfo")) {
                    continue;
                }
                let Ok(content) = fs::read_to_string(&info_path) else {
                    continue;
                };
                let mut name = None;
                let mut original_parent = None;
                let mut time_deleted = None;
                for line in content.lines().skip(1) {
                    let Some((key, value)) = line.split_once('=') else {
                        continue;
                    };
                    let key = key.trim();
                    let value = value.trim();
                    if key == "Path" {
                        let decoded = decode_uri_path(value);
                        name = decoded.file_name().map(|n| n.to_os_string());
                        original_parent = decoded.parent().map(Path::to_path_buf);
                    } else if key == "DeletionDate" {
                        if let Ok(naive) =
                            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
                        {
                            use chrono::TimeZone as _;
                            if let Some(dt) = chrono::Local.from_local_datetime(&naive).earliest() {
                                time_deleted = Some(dt.timestamp());
                            }
                        }
                    }
                }
                if let (Some(name), Some(original_parent)) = (name, original_parent) {
                    items.push(trash::TrashItem {
                        id: info_path.into_os_string(),
                        name,
                        original_parent,
                        time_deleted: time_deleted.unwrap_or(-1),
                    });
                }
            }
            Ok(items)
        }
    }

    #[cfg(windows)]
    fn purge_trashed_from(dir: &Path) {
        let Ok(items) = list_trash() else {
            return;
        };
        let mine: Vec<_> = items
            .into_iter()
            .filter(|i| i.original_parent.starts_with(dir))
            .collect();
        if !mine.is_empty() {
            let _ = with_trash(move || trash::os_limited::purge_all(mine));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::test_dir;
    use super::*;

    /// Where the trash refuses (a network drive), undo can delete what it created for good.
    #[test]
    fn undo_permanently_deletes_created_items() {
        let d = test_dir();
        let j = UndoJournal::default();
        let dir = create_dir(d.path(), "made", &j).unwrap();
        fs::write(dir.join("inside.txt"), "x").unwrap();
        let msg = j.undo_with(true).unwrap();
        assert_eq!(msg, "Deleted “made” for good");
        assert!(!dir.exists());
        assert!(j.is_empty());
    }

    #[test]
    #[allow(clippy::result_large_err)] // matches the reverter's by-value failure record
    fn failed_undo_stays_before_operations_added_during_revert() {
        let j = UndoJournal::default();
        let older = UndoRecord::RemovedDirs {
            dirs: vec![PathBuf::from("older")],
        };
        let newer = UndoRecord::RemovedDirs {
            dirs: vec![PathBuf::from("newer")],
        };
        j.push(older.clone());
        let result = j.undo_with_reverter(false, |record, _| {
            j.push(newer.clone());
            Err((CoreError::Cancelled, Some(record)))
        });
        assert!(matches!(result, Err(CoreError::Cancelled)));
        assert_eq!(j.peek_description(), Some(newer.describe()));
        j.undo().unwrap();
        assert_eq!(j.peek_description(), Some(older.describe()));
    }

    /// A case-only rename with non-ASCII letters (Slovenian, German, …) works where the
    /// file system ignores case (Windows, macOS).
    #[test]
    fn case_only_rename_with_non_ascii_letters() {
        let d = test_dir();
        let j = UndoJournal::default();
        let f = d.path().join("čebela.txt");
        fs::write(&f, "x").unwrap();
        let r = rename(&f, "Čebela.txt", &j).unwrap();
        assert_eq!(names(d.path()), ["Čebela.txt=x"]);
        assert_eq!(r, d.path().join("Čebela.txt"));
    }

    /// A number in brackets that is part of the name is not a copy counter.
    #[test]
    fn unique_path_keeps_numbers_that_belong_to_the_name() {
        let d = test_dir();
        fs::write(d.path().join("Budget (2026).xlsx"), "").unwrap();
        assert_eq!(
            unique_path(d.path(), "Budget (2026).xlsx"),
            d.path().join("Budget (2026) (2).xlsx")
        );
        // A real counter continues: "a (2).txt" next to "a.txt" → "a (3).txt".
        fs::write(d.path().join("a.txt"), "").unwrap();
        fs::write(d.path().join("a (2).txt"), "").unwrap();
        assert_eq!(
            unique_path(d.path(), "a (2).txt"),
            d.path().join("a (3).txt")
        );
    }

    /// A folder has no extension: "my.project" → "my.project (2)".
    #[test]
    fn unique_path_of_a_folder_with_a_dot() {
        let d = test_dir();
        fs::create_dir(d.path().join("my.project")).unwrap();
        assert_eq!(
            unique_path(d.path(), "my.project"),
            d.path().join("my.project (2)")
        );
    }

    #[test]
    fn save_text_is_undoable() {
        let t = test_dir();
        let p = t.path().join("f.txt");
        std::fs::write(&p, "old\n").unwrap();
        let j = UndoJournal::default();
        save_text(&p, "new\n", &j).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new\n");
        assert_eq!(j.peek_description().as_deref(), Some("save “f.txt”"));
        match j.undo() {
            Ok(_) => assert_eq!(std::fs::read_to_string(&p).unwrap(), "old\n"),
            Err(e) => eprintln!("undo not verified (no trash here): {e}"),
        }
    }

    /// A save made against an old copy of a file is refused and writes nothing; a save made against
    /// the file as it is, or without a revision, goes through.
    #[test]
    fn a_checked_save_is_refused_when_the_file_changed_on_disk() {
        let t = test_dir();
        let p = t.path().join("a.txt");
        fs::write(&p, "one").unwrap();
        let j = UndoJournal::default();
        let read = file_revision(&p).unwrap();
        // Another program writes the file (a different length, so the revision differs at once).
        fs::write(&p, "changed elsewhere").unwrap();
        let error = save_text_checked(&p, "my edit", Some(&read), &j)
            .unwrap_err()
            .to_string();
        assert!(error.contains("changed on disk"), "{error}");
        assert_eq!(fs::read_to_string(&p).unwrap(), "changed elsewhere");
        let leftovers = fs::read_dir(t.path()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary file left");
        // Against the file as it is now, the save goes through; a missing or empty revision is not checked.
        let now = file_revision(&p).unwrap();
        save_text_checked(&p, "second", Some(&now), &j).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "second");
        save_text_checked(&p, "third", None, &j).unwrap();
        save_text_checked(&p, "fourth", Some(""), &j).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "fourth");
        // The revision of a missing file is nothing.
        assert_eq!(file_revision(&t.path().join("missing")), None);
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn a_new_text_save_does_not_replace_a_file_created_after_selection() {
        let t = test_dir();
        let p = t.path().join("new.txt");
        let j = UndoJournal::default();
        assert_eq!(file_revision(&p), None);
        fs::write(&p, "created elsewhere").unwrap();
        assert!(save_new_text(&p, "my result", &j).is_err());
        assert_eq!(fs::read_to_string(&p).unwrap(), "created elsewhere");
        assert_eq!(j.peek_description(), None);
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 1);
        fs::remove_file(&p).unwrap();
        assert!(save_new_text(&p, "my result", &j).unwrap());
        assert_eq!(fs::read_to_string(&p).unwrap(), "my result");
        assert_eq!(j.peek_description().as_deref(), Some("save “new.txt”"));
    }

    /// Saving through a symlink writes the file it points to and keeps the link
    /// (dotfiles managed with stow or a dotfiles repo); permissions are kept.
    #[cfg(unix)]
    #[test]
    fn save_text_keeps_symlinks_and_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let t = test_dir();
        let real = t.path().join("dotfiles/bashrc");
        fs::create_dir(t.path().join("dotfiles")).unwrap();
        fs::write(&real, "old").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = t.path().join(".bashrc");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let j = UndoJournal::default();
        assert!(save_text(&link, "new", &j).unwrap());
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let leftovers: Vec<_> = fs::read_dir(t.path().join("dotfiles"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "no temporary file left: {leftovers:?}");
        j.undo().unwrap();
        assert_eq!(fs::read_to_string(&link).unwrap(), "old");
    }

    /// Undo restores what a long delete trashed: the trash stamps every item when it
    /// gets to it, so the times span more than a moment. When a path was trashed twice,
    /// the newest copy comes back.
    #[test]
    fn trashed_items_are_picked_by_time_span() {
        let p = |s: &str| PathBuf::from("/d").join(s);
        let items = vec![
            (p("a"), 100), // start of the delete
            (p("b"), 130), // 30 s later, same delete
            (p("c"), 500), // not ours
            (p("b"), 95),  // an older "b", trashed before
            (p("b"), 120), // trashed twice within the span: the newer one wins
            (p("x"), 110), // another path
        ];
        let mut got = pick_trashed(
            items,
            &[p("a"), p("b"), p("c")],
            100_000,
            130_000,
            |(p, t)| (p.clone(), *t, 0),
        );
        got.sort();
        assert_eq!(got, vec![(p("a"), 100), (p("b"), 130)]);

        // Same second, told apart by the fine time: "d" trashed at 200.300 by this delete,
        // and again at 200.700 when a later copy that took the name was undone.
        let ns = |ms: u128| ms * 1_000_000;
        let items = vec![(p("d"), ns(200_300)), (p("d"), ns(200_700))];
        let got = pick_trashed(items, &[p("d")], 200_250, 200_310, |(p, f)| {
            (p.clone(), 200, *f)
        });
        assert_eq!(got, vec![(p("d"), ns(200_300))]);
    }

    #[test]
    fn undo_in_reverts_only_that_folder() {
        let t = test_dir();
        let a = t.path().join("a");
        let b = t.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let j = UndoJournal::default();
        let fa = a.join("x.txt");
        std::fs::write(&fa, "1").unwrap();
        let ra = rename(&fa, "y.txt", &j).unwrap(); // in a
        let db = create_dir(&b, "new", &j).unwrap(); // in b (newer)
        assert_eq!(j.peek_in(&a).as_deref(), Some("rename “x.txt” to “y.txt”"));
        assert!(j.peek_in(&t.path().join("c")).is_none());
        assert_eq!(j.undo_in(&a).unwrap(), "Renamed “y.txt” back to “x.txt”");
        assert!(fa.exists() && !ra.exists(), "rename in a reverted");
        assert!(db.exists(), "newer op in b untouched");
        assert_eq!(j.peek_description().as_deref(), Some("add “new”"));
        assert!(matches!(j.undo_in(&a), Err(CoreError::NothingToUndo)));
    }

    /// Regression: Delete panicked with RPC_E_CHANGED_MODE when the calling thread
    /// already had COM initialised as multithreaded (as on async worker threads).
    #[cfg(windows)]
    #[test]
    fn trash_works_when_thread_already_uses_com() {
        #[link(name = "ole32")]
        extern "system" {
            fn CoInitializeEx(reserved: *mut std::ffi::c_void, coinit: u32) -> i32;
        }
        const COINIT_MULTITHREADED: u32 = 0;
        // SAFETY: plain COM initialisation of this test thread.
        assert!(unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_MULTITHREADED) } >= 0);
        let t = test_dir();
        let f = t.path().join("com-test.txt");
        std::fs::write(&f, "x").unwrap();
        trash_items(std::slice::from_ref(&f), &UndoJournal::default()).unwrap();
        assert!(!f.exists());
    }

    #[test]
    fn undo_texts_name_single_items() {
        let p = |s: &str| PathBuf::from("/home/u").join(s);
        let created = UndoRecord::Created {
            paths: vec![p("Projects/a.pdf")],
        };
        assert_eq!(created.describe(), "add “a.pdf”");
        assert_eq!(created.undone(), "Removed “a.pdf” (in the trash)");
        let moved = UndoRecord::Moved {
            pairs: vec![(p("Downloads/a.pdf"), p("Archive/a.pdf"))],
        };
        assert_eq!(moved.describe(), "move “a.pdf” to Archive");
        assert_eq!(moved.undone(), "Moved “a.pdf” back to Downloads");
        let trashed = UndoRecord::Trashed {
            paths: vec![p("a.pdf"), p("b.pdf")],
            at: 0,
            until: 0,
        };
        assert_eq!(trashed.describe(), "delete 2 items");
        assert_eq!(trashed.undone(), "Restored 2 items");
        assert_eq!(items(1), "1 item");
    }

    /// A move inside one drive is a rename per item and is not measured first; one that
    /// meets a namesake (a merge or a replace works inside it) or leaves the drive is.
    #[test]
    fn a_move_inside_one_drive_is_not_measured_first() {
        let t = test_dir();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        std::fs::create_dir_all(src.join("tree/deep")).unwrap();
        std::fs::create_dir(&dst).unwrap();
        std::fs::write(src.join("tree/deep/a.txt"), "a").unwrap();
        std::fs::write(src.join("file.txt"), "f").unwrap();
        let both = [src.join("tree"), src.join("file.txt")];
        assert!(moves_by_rename(&both, &dst));
        assert!(!moves_by_rename(&both, &t.path().join("missing")));
        assert!(!moves_by_rename(&both, &src.join("file.txt")));
        assert!(!moves_by_rename(&[src.join("gone.txt")], &dst));
        std::fs::write(dst.join("file.txt"), "there").unwrap();
        assert!(moves_by_rename(&both[..1], &dst));
        assert!(!moves_by_rename(&both, &dst));
        // The totals reported for it are the items themselves.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let m = TaskManager::new(Arc::new(UndoJournal::default()), {
            let seen = seen.clone();
            Arc::new(move |e| {
                if let TaskEvent::Progress(p) = e {
                    seen.lock().unwrap().push((p.total_items, p.total_bytes));
                }
            })
        });
        let handle = TaskHandle {
            id: 1,
            cancel: AtomicBool::new(false),
        };
        m.run(
            &handle,
            task(
                TaskKind::Move,
                &src.join("tree"),
                &dst,
                ConflictPolicy::KeepBoth,
            ),
        )
        .unwrap();
        assert!(dst.join("tree/deep/a.txt").is_file());
        assert_eq!(*seen.lock().unwrap(), [(1, 0)]);
    }

    fn mgr() -> Arc<TaskManager> {
        Arc::new(TaskManager::new(
            Arc::new(UndoJournal::default()),
            Arc::new(|_| {}),
        ))
    }

    #[test]
    fn compress_then_extract_and_trash_as_tasks() {
        use super::super::archive::{ArchiveFormat, ExtractMode};
        let t = test_dir();
        let src = t.path().join("docs");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("a.txt"), "a").unwrap();
        let m = mgr();
        let base = |kind| TaskRequest {
            kind,
            sources: vec![],
            dest: Some(t.path().to_path_buf()),
            conflict: ConflictPolicy::KeepBoth,
            archive_name: None,
            format: None,
            extract_mode: ExtractMode::Smart,
            trash_archive: false,
            merge: false,
        };
        let mut req = base(TaskKind::Compress);
        req.sources = vec![src.clone()];
        req.archive_name = Some("docs.zip".into());
        req.format = Some(ArchiveFormat::Zip);
        assert_eq!(run(&m, req).unwrap(), (1, 0));
        let zip = t.path().join("docs.zip");
        assert!(zip.is_file());
        assert_eq!(
            m.journal().peek_description().as_deref(),
            Some("add “docs.zip”")
        );

        let mut req = base(TaskKind::Extract);
        req.sources = vec![zip.clone()];
        req.trash_archive = true;
        match run(&m, req) {
            Ok(_) => {
                assert!(t.path().join("docs (2)/a.txt").is_file());
                assert!(!zip.exists(), "archive went to the trash");
                assert_eq!(
                    m.journal().peek_description().as_deref(),
                    Some("extract and delete “docs.zip”")
                );
            }
            Err(CoreError::Trash(e)) => eprintln!("trash unavailable here, not verified: {e}"),
            Err(e) => panic!("{e}"),
        }
    }

    fn run(m: &TaskManager, req: TaskRequest) -> Result<(u64, u64)> {
        m.run(
            &TaskHandle {
                id: 1,
                cancel: AtomicBool::new(false),
            },
            req,
        )
    }

    #[test]
    fn unique_names() {
        let d = test_dir();
        assert_eq!(unique_path(d.path(), "a.txt"), d.path().join("a.txt"));
        fs::write(d.path().join("a.txt"), "").unwrap();
        assert_eq!(unique_path(d.path(), "a.txt"), d.path().join("a (2).txt"));
        fs::write(d.path().join("a (2).txt"), "").unwrap();
        assert_eq!(
            unique_path(d.path(), "a (2).txt"),
            d.path().join("a (3).txt")
        );
        fs::create_dir(d.path().join(".cfg")).unwrap();
        assert_eq!(unique_path(d.path(), ".cfg"), d.path().join(".cfg (2)"));
    }

    #[test]
    fn windows_reserved_names() {
        for n in ["CON", "con.txt", "Nul", "COM1", "lpt9.log", "aux.tar.gz"] {
            assert!(windows_reserved(n), "{n}");
        }
        for n in ["CONSOLE", "com0", "COM10", "icon.png", "nul_", "a.con"] {
            assert!(!windows_reserved(n), "{n}");
        }
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("ok.txt").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
    }

    #[test]
    fn copy_tree_with_conflicts_and_undo() {
        let d = test_dir();
        let src = d.path().join("src");
        let dst = d.path().join("dst");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/f.txt"), b"hello").unwrap();
        fs::create_dir(&dst).unwrap();
        let m = mgr();
        let req = |c| TaskRequest {
            kind: TaskKind::Copy,
            sources: vec![src.clone()],
            dest: Some(dst.clone()),
            conflict: c,
            archive_name: None,
            format: None,
            extract_mode: Default::default(),
            trash_archive: false,
            merge: false,
        };

        run(&m, req(ConflictPolicy::KeepBoth)).unwrap();
        assert_eq!(fs::read(dst.join("src/sub/f.txt")).unwrap(), b"hello");
        // Second copy keeps both.
        run(&m, req(ConflictPolicy::KeepBoth)).unwrap();
        assert!(dst.join("src (2)/sub/f.txt").exists());
        // Skip leaves things alone.
        assert_eq!(run(&m, req(ConflictPolicy::Skip)).unwrap(), (0, 1));
        assert_eq!(
            find_conflicts(std::slice::from_ref(&src), &dst),
            vec![dst.join("src")]
        );
    }

    fn task(kind: TaskKind, src: &Path, dest: &Path, conflict: ConflictPolicy) -> TaskRequest {
        TaskRequest {
            kind,
            sources: vec![src.to_path_buf()],
            dest: Some(dest.to_path_buf()),
            conflict,
            archive_name: None,
            format: None,
            extract_mode: Default::default(),
            trash_archive: false,
            merge: false,
        }
    }

    /// Regression: "Replace" into a symlink to the source's own folder deleted the source
    /// (the target *was* the source) and then failed to copy it.
    #[cfg(unix)]
    #[test]
    fn replace_through_symlinked_folder_keeps_source() {
        let d = test_dir();
        let real = d.path().join("real");
        let link = d.path().join("link");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let a = real.join("a.txt");
        fs::write(&a, "precious").unwrap();
        assert!(same_dir(&real, &link));
        assert!(
            find_conflicts(std::slice::from_ref(&a), &link).is_empty(),
            "a copy into its own folder is no conflict"
        );
        let m = mgr();

        // Copy: a duplicate next to the original, whatever the policy.
        run(
            &m,
            task(TaskKind::Copy, &a, &link, ConflictPolicy::Overwrite),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "precious");
        assert_eq!(
            fs::read_to_string(real.join("a (2).txt")).unwrap(),
            "precious"
        );

        // Move: onto itself is a no-op.
        assert_eq!(
            run(
                &m,
                task(TaskKind::Move, &a, &link, ConflictPolicy::Overwrite)
            )
            .unwrap(),
            (0, 1)
        );
        assert_eq!(fs::read_to_string(&a).unwrap(), "precious");

        // A folder reached through a symlink as the source's parent.
        let sub = link.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("b.txt"), "keep").unwrap();
        run(
            &m,
            task(TaskKind::Copy, &sub, &real, ConflictPolicy::Overwrite),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(real.join("sub/b.txt")).unwrap(), "keep");
        assert!(real.join("sub (2)/b.txt").exists());
    }

    /// "Replace" puts the old item in the trash; one undo brings it back.
    #[test]
    fn replace_trashes_old_item_and_undo_restores_it() {
        let d = test_dir();
        let src = d.path().join("src");
        let dst = d.path().join("dst");
        fs::create_dir_all(src.join("folder")).unwrap();
        fs::create_dir_all(dst.join("folder")).unwrap();
        fs::write(src.join("a.txt"), "new").unwrap();
        fs::write(dst.join("a.txt"), "old").unwrap();
        fs::write(src.join("folder/new.txt"), "new").unwrap();
        fs::write(dst.join("folder/only-old.txt"), "old").unwrap();
        let m = mgr();
        let j = m.journal().clone();

        // Copy over a file.
        run(
            &m,
            task(
                TaskKind::Copy,
                &src.join("a.txt"),
                &dst,
                ConflictPolicy::Overwrite,
            ),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "new");
        assert_eq!(
            j.peek_description().as_deref(),
            Some("copy “a.txt”, replacing the old one")
        );
        j.undo().unwrap();
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "old");
        assert_eq!(fs::read_to_string(src.join("a.txt")).unwrap(), "new");

        // Copy over a folder: replaced as a whole, and restored as a whole.
        run(
            &m,
            task(
                TaskKind::Copy,
                &src.join("folder"),
                &dst,
                ConflictPolicy::Overwrite,
            ),
        )
        .unwrap();
        assert!(dst.join("folder/new.txt").exists());
        assert!(!dst.join("folder/only-old.txt").exists());
        j.undo().unwrap();
        assert!(dst.join("folder/only-old.txt").exists());
        assert!(!dst.join("folder/new.txt").exists());

        // Move over a file: undo moves it back and restores the replaced one.
        run(
            &m,
            task(
                TaskKind::Move,
                &src.join("a.txt"),
                &dst,
                ConflictPolicy::Overwrite,
            ),
        )
        .unwrap();
        assert!(!src.join("a.txt").exists());
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "new");
        j.undo().unwrap();
        assert_eq!(fs::read_to_string(src.join("a.txt")).unwrap(), "new");
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "old");
    }

    /// Regression: unit-test trash operations stay isolated per test on freedesktop Unix,
    /// never touching the real OS trash and remaining reliable under concurrent runs.
    #[cfg(all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    ))]
    #[test]
    fn isolated_test_trash_is_concurrent_and_avoids_os_trash() {
        let handles: Vec<_> = (0..8)
            .map(|worker| {
                std::thread::spawn(move || {
                    for round in 0..5 {
                        let d = test_dir();
                        let src = d.path().join("src");
                        let dst = d.path().join("dst");
                        fs::create_dir(&src).unwrap();
                        fs::create_dir(&dst).unwrap();
                        let name = format!("100% čebela w{worker} r{round}.txt");
                        fs::write(src.join(&name), format!("new-{worker}-{round}")).unwrap();
                        fs::write(dst.join(&name), format!("old-{worker}-{round}")).unwrap();

                        let m = mgr();
                        let j = m.journal().clone();
                        run(
                            &m,
                            task(
                                TaskKind::Copy,
                                &src.join(&name),
                                &dst,
                                ConflictPolicy::Overwrite,
                            ),
                        )
                        .unwrap();

                        let root = test_support::isolated_trash_root()
                            .expect("isolated trash initialized");
                        let trashed = list_trash().unwrap();
                        assert_eq!(
                            trashed.len(),
                            1,
                            "worker {worker} round {round} must only see its own trash"
                        );
                        assert!(
                            Path::new(&trashed[0].id).starts_with(&root),
                            "trashed item {:?} must live inside isolated root {:?}",
                            trashed[0].id,
                            root
                        );
                        assert_eq!(trashed[0].original_path(), dst.join(&name));

                        j.undo().unwrap();
                        assert_eq!(
                            fs::read_to_string(dst.join(&name)).unwrap(),
                            format!("old-{worker}-{round}")
                        );
                        assert!(list_trash().unwrap().is_empty());
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    /// A copy that fails half-way leaves nothing behind, and a copy never truncates a
    /// file that is already there.
    #[cfg(unix)]
    #[test]
    fn failed_copy_is_cleaned_up_and_never_overwrites() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads unreadable files
        }
        let d = test_dir();
        let src = d.path().join("src");
        let dst = d.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir(&dst).unwrap();
        fs::write(src.join("1-ok.txt"), "ok").unwrap();
        let locked = src.join("2-locked.txt");
        fs::write(&locked, "secret").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let m = mgr();
        let r = run(
            &m,
            task(TaskKind::Copy, &src, &dst, ConflictPolicy::KeepBoth),
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(r.is_err());
        assert!(!dst.join("src").exists(), "partial copy removed");
        assert!(m.journal().peek_description().is_none(), "nothing to undo");

        let existing = dst.join("x.txt");
        fs::write(&existing, "keep me").unwrap();
        let e = copy_fresh(
            &src.join("1-ok.txt"),
            &existing,
            &AtomicBool::new(false),
            &mut |_| {},
            false,
        );
        assert!(matches!(e, Err(CoreError::AlreadyExists(_))));
        assert_eq!(fs::read_to_string(&existing).unwrap(), "keep me");
    }

    /// The durable copy used by moves across drives (fsync of files and folders).
    #[test]
    fn durable_copy_of_a_tree() {
        let d = test_dir();
        let src = d.path().join("src");
        fs::create_dir_all(src.join("a/b")).unwrap();
        fs::write(src.join("a/b/f.txt"), "data").unwrap();
        let dst = d.path().join("dst");
        let mut bytes = 0;
        copy_fresh(
            &src,
            &dst,
            &AtomicBool::new(false),
            &mut |n| bytes += n,
            true,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dst.join("a/b/f.txt")).unwrap(), "data");
        assert_eq!(bytes, 4);
        // Cancelled before the first byte: nothing is left behind.
        let dst2 = d.path().join("dst2");
        let e = copy_fresh(&src, &dst2, &AtomicBool::new(true), &mut |_| {}, true);
        assert!(matches!(e, Err(CoreError::Cancelled)));
        assert!(!dst2.exists());
    }

    /// Large files copy in chunks (in the kernel where it can), with progress and the
    /// exact bytes; an empty file copies too.
    #[test]
    fn large_and_empty_file_copies() {
        let d = test_dir();
        let data: Vec<u8> = (0..(40u32 << 20) / 4)
            .flat_map(|i| i.to_le_bytes())
            .collect();
        let src = d.path().join("big.bin");
        fs::write(&src, &data).unwrap();
        let mut seen = 0u64;
        copy_fresh(
            &src,
            &d.path().join("copy.bin"),
            &AtomicBool::new(false),
            &mut |n| seen += n,
            false,
        )
        .unwrap();
        assert_eq!(seen, data.len() as u64);
        assert!(fs::read(d.path().join("copy.bin")).unwrap() == data);
        fs::write(d.path().join("empty"), b"").unwrap();
        copy_fresh(
            &d.path().join("empty"),
            &d.path().join("empty2"),
            &AtomicBool::new(false),
            &mut |_| {},
            true,
        )
        .unwrap();
        assert_eq!(fs::metadata(d.path().join("empty2")).unwrap().len(), 0);
    }

    /// Ctrl+D as a task: several items at once, names predicted for the UI and unique
    /// within the batch ("a.txt" and "a (2).txt" do not both become "a (3).txt").
    #[test]
    fn duplicate_task_names_and_undo() {
        let d = test_dir();
        let a = d.path().join("a.txt");
        let a2 = d.path().join("a (2).txt");
        let dir = d.path().join("folder");
        fs::write(&a, "a").unwrap();
        fs::write(&a2, "a2").unwrap();
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("f"), "f").unwrap();
        let sources = vec![a.clone(), a2.clone(), dir.clone()];
        let predicted = duplicate_targets(&sources);
        assert_eq!(
            predicted,
            vec![
                d.path().join("a (3).txt"),
                d.path().join("a (4).txt"),
                d.path().join("folder (2)"),
            ]
        );
        let m = mgr();
        let req = TaskRequest {
            kind: TaskKind::Duplicate,
            sources,
            dest: None,
            conflict: ConflictPolicy::KeepBoth,
            archive_name: None,
            format: None,
            extract_mode: Default::default(),
            trash_archive: false,
            merge: false,
        };
        assert_eq!(run(&m, req).unwrap(), (3, 0));
        assert_eq!(fs::read_to_string(&predicted[0]).unwrap(), "a");
        assert_eq!(fs::read_to_string(&predicted[1]).unwrap(), "a2");
        assert_eq!(fs::read_to_string(predicted[2].join("f")).unwrap(), "f");
        m.journal().undo().unwrap();
        assert!(predicted.iter().all(|p| !p.exists()));
        assert!(a.exists() && a2.exists() && dir.exists());
    }

    /// Builds `root/{src,dst}/Photos` with files on one side, both sides and in a
    /// subfolder on both sides.
    fn photos(root: &Path) -> (PathBuf, PathBuf) {
        let (src, dst) = (root.join("src"), root.join("dst"));
        for d in [&src, &dst] {
            fs::create_dir_all(d.join("Photos/sub")).unwrap();
        }
        fs::write(src.join("Photos/a.jpg"), "new a").unwrap();
        fs::write(src.join("Photos/c.jpg"), "c").unwrap();
        fs::write(src.join("Photos/sub/b.jpg"), "b").unwrap();
        fs::write(dst.join("Photos/a.jpg"), "old a").unwrap();
        fs::write(dst.join("Photos/d.jpg"), "d").unwrap();
        fs::write(dst.join("Photos/sub/e.jpg"), "e").unwrap();
        (src, dst)
    }

    fn tree(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = walkdir::WalkDir::new(dir)
            .min_depth(1)
            .into_iter()
            .flatten()
            .map(|e| {
                let rel = e
                    .path()
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                match fs::read_to_string(e.path()) {
                    Ok(t) => format!("{rel}={t}"),
                    Err(_) => rel,
                }
            })
            .collect();
        v.sort();
        v
    }

    fn merge_task(
        kind: TaskKind,
        src: &Path,
        dest: &Path,
        conflict: ConflictPolicy,
    ) -> TaskRequest {
        let mut r = task(kind, src, dest, conflict);
        r.merge = true;
        r
    }

    #[test]
    fn conflict_info_counts_files_inside_merged_folders() {
        let d = test_dir();
        let (src, dst) = photos(d.path());
        fs::write(src.join("loose.txt"), "1").unwrap();
        fs::write(dst.join("loose.txt"), "2").unwrap();
        let info = conflict_info(&[src.join("Photos"), src.join("loose.txt")], &dst);
        assert_eq!(info.paths, vec![dst.join("Photos"), dst.join("loose.txt")]);
        assert_eq!(info.folders, 1);
        assert_eq!(info.files_when_merged, 2, "a.jpg inside, loose.txt outside");
        assert!(info.sample.contains(&dst.join("Photos/a.jpg")));
    }

    #[test]
    fn copy_merges_folders_and_one_undo_reverts_it() {
        let d = test_dir();
        let (src, dst) = photos(d.path());
        let before = tree(&dst);
        let m = mgr();
        let j = m.journal().clone();
        let r = run(
            &m,
            merge_task(
                TaskKind::Copy,
                &src.join("Photos"),
                &dst,
                ConflictPolicy::Overwrite,
            ),
        );
        assert_eq!(r.unwrap(), (1, 0));
        assert_eq!(
            tree(&dst),
            [
                "Photos",
                "Photos/a.jpg=new a",
                "Photos/c.jpg=c",
                "Photos/d.jpg=d",
                "Photos/sub",
                "Photos/sub/b.jpg=b",
                "Photos/sub/e.jpg=e"
            ]
        );
        assert_eq!(
            j.peek_description().as_deref(),
            Some("copy “Photos” (merged), replacing 1 item")
        );
        assert!(
            j.peek_in(&dst).is_some(),
            "per-panel undo finds the merge from the parent folder"
        );
        j.undo().unwrap();
        assert_eq!(tree(&dst), before);
        assert_eq!(tree(&src).len(), 5, "source untouched");
    }

    #[test]
    fn move_merge_with_skip_keeps_skipped_files_and_undo_restores_the_source() {
        let d = test_dir();
        let (src, dst) = photos(d.path());
        let (src_before, dst_before) = (tree(&src), tree(&dst));
        let m = mgr();
        let r = run(
            &m,
            merge_task(
                TaskKind::Move,
                &src.join("Photos"),
                &dst,
                ConflictPolicy::Skip,
            ),
        );
        assert_eq!(r.unwrap(), (1, 1));
        // a.jpg was skipped: it stays, and so does its folder; the emptied "sub" is gone.
        assert_eq!(tree(&src), ["Photos", "Photos/a.jpg=new a"]);
        assert_eq!(
            tree(&dst),
            [
                "Photos",
                "Photos/a.jpg=old a",
                "Photos/c.jpg=c",
                "Photos/d.jpg=d",
                "Photos/sub",
                "Photos/sub/b.jpg=b",
                "Photos/sub/e.jpg=e"
            ]
        );
        m.journal().undo().unwrap();
        assert_eq!(tree(&src), src_before);
        assert_eq!(tree(&dst), dst_before);
    }

    #[test]
    fn merge_keep_both_and_replace_older() {
        let d = test_dir();
        let (src, dst) = photos(d.path());
        let m = mgr();
        run(
            &m,
            merge_task(
                TaskKind::Copy,
                &src.join("Photos"),
                &dst,
                ConflictPolicy::KeepBoth,
            ),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(dst.join("Photos/a (2).jpg")).unwrap(),
            "new a"
        );
        assert_eq!(
            fs::read_to_string(dst.join("Photos/a.jpg")).unwrap(),
            "old a"
        );
        m.journal().undo().unwrap();

        // Replace older: the incoming a.jpg is older → skipped; then newer → replaced.
        let set_age = |p: &Path, secs_ago: u64| {
            let f = fs::File::options().write(true).open(p).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
                .unwrap();
        };
        set_age(&src.join("Photos/a.jpg"), 3600);
        set_age(&dst.join("Photos/a.jpg"), 60);
        let r = run(
            &m,
            merge_task(
                TaskKind::Copy,
                &src.join("Photos"),
                &dst,
                ConflictPolicy::OverwriteOlder,
            ),
        );
        assert_eq!(r.unwrap(), (1, 1));
        assert_eq!(
            fs::read_to_string(dst.join("Photos/a.jpg")).unwrap(),
            "old a"
        );
        m.journal().undo().unwrap();
        set_age(&src.join("Photos/a.jpg"), 10);
        set_age(&dst.join("Photos/a.jpg"), 3600);
        run(
            &m,
            merge_task(
                TaskKind::Copy,
                &src.join("Photos"),
                &dst,
                ConflictPolicy::OverwriteOlder,
            ),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(dst.join("Photos/a.jpg")).unwrap(),
            "new a"
        );
        // Without merge, "Replace older" decides for the folder as a whole by its date.
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                format!(
                    "{}={}",
                    e.file_name().to_string_lossy(),
                    fs::read_to_string(e.path()).unwrap()
                )
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn batch_rename_with_swaps_and_case_changes_and_one_undo() {
        let d = test_dir();
        let p = |n: &str| d.path().join(n);
        let collision = p(&format!(".gall-rename-{}-0", std::process::id()));
        fs::write(&collision, "unrelated").unwrap();
        for (n, c) in [
            ("a.txt", "A"),
            ("b.txt", "B"),
            ("c.txt", "C"),
            ("x.txt", "X"),
        ] {
            fs::write(p(n), c).unwrap();
        }
        let j = UndoJournal::default();
        // a ↔ b swap, c → C.txt (case only), x unchanged.
        let out = batch_rename(
            &[
                (p("a.txt"), "b.txt".into()),
                (p("b.txt"), "a.txt".into()),
                (p("c.txt"), "C.TXT".into()),
                (p("x.txt"), "x.txt".into()),
            ],
            &j,
        )
        .unwrap();
        assert_eq!(out, vec![p("b.txt"), p("a.txt"), p("C.TXT"), p("x.txt")]);
        assert_eq!(
            names(d.path()),
            [
                ".gall-rename-".to_string() + &std::process::id().to_string() + "-0=unrelated",
                "C.TXT=C".into(),
                "a.txt=B".into(),
                "b.txt=A".into(),
                "x.txt=X".into()
            ]
        );
        assert_eq!(fs::read_to_string(collision).unwrap(), "unrelated");
        assert_eq!(j.peek_description().as_deref(), Some("rename 3 items"));
        j.undo().unwrap();
        assert_eq!(
            names(d.path()),
            [
                ".gall-rename-".to_string() + &std::process::id().to_string() + "-0=unrelated",
                "a.txt=A".into(),
                "b.txt=B".into(),
                "c.txt=C".into(),
                "x.txt=X".into()
            ]
        );
    }

    #[test]
    fn no_replace_rename_preserves_existing_destination() {
        let d = test_dir();
        let source = d.path().join("source");
        let destination = d.path().join("destination");
        fs::write(&source, "source data").unwrap();
        fs::write(&destination, "unrelated data").unwrap();
        let error = rename_noreplace(&source, &destination).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(source).unwrap(), "source data");
        assert_eq!(fs::read_to_string(destination).unwrap(), "unrelated data");
    }

    #[test]
    fn batch_rename_retries_a_colliding_staging_name() {
        let d = test_dir();
        let source = d.path().join("a");
        let destination = d.path().join("b");
        let colliding_temp = d
            .path()
            .join(format!(".gall-rename-{}-41", std::process::id()));
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();
        fs::write(&colliding_temp, "unrelated").unwrap();
        let ids = [41, 42, 43];
        let mut next = ids.into_iter();
        rename_many_with_ids(
            &[
                (source.clone(), destination.clone()),
                (destination.clone(), source.clone()),
            ],
            &mut || next.next().unwrap(),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&source).unwrap(), "destination");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "source");
        assert_eq!(fs::read_to_string(colliding_temp).unwrap(), "unrelated");
    }

    #[test]
    fn cross_device_move_cleanup_failure_restores_source_and_journals_copy() {
        let d = test_dir();
        let src = d.path().join("src");
        let dest = d.path().join("dest");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(src.join("first.txt"), "first").unwrap();
        fs::write(src.join("nested/second.txt"), "second").unwrap();

        let cancel = AtomicBool::new(false);
        let transfer = Transfer {
            kind: TaskKind::Move,
            conflict: ConflictPolicy::KeepBoth,
            merge: false,
            cancel: &cancel,
        };
        let mut placed = Placed::default();
        let mut tick = |_| {};
        let cross_device =
            |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices));
        let partial_remove = |path: &Path| {
            fs::remove_file(path.join("first.txt")).unwrap();
            Err(CoreError::io(
                "delete",
                path,
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ))
        };
        let result = transfer.place_with_ops(
            &src,
            &dest,
            &mut placed,
            &mut tick,
            &cross_device,
            &partial_remove,
        );
        assert!(result.is_err());
        assert_eq!(
            tree(d.path()),
            [
                "dest",
                "dest/src",
                "dest/src/first.txt=first",
                "dest/src/nested",
                "dest/src/nested/second.txt=second",
                "src",
                "src/first.txt=first",
                "src/nested",
                "src/nested/second.txt=second"
            ]
        );
        assert_eq!(placed.created, [dest.join("src")]);

        let journal = UndoJournal::default();
        journal.push(UndoRecord::Created {
            paths: placed.created,
        });
        journal.undo_with(true).unwrap();
        assert!(!dest.join("src").exists());
        assert_eq!(
            tree(&src),
            ["first.txt=first", "nested", "nested/second.txt=second"]
        );
    }

    #[test]
    fn failed_move_recovery_does_not_accept_changed_source_file() {
        let d = test_dir();
        let src = d.path().join("src");
        let dest = d.path().join("dest");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&dest).unwrap();
        let source = src.join("item.txt");
        fs::write(&source, "original").unwrap();

        let cancel = AtomicBool::new(false);
        let transfer = Transfer {
            kind: TaskKind::Move,
            conflict: ConflictPolicy::KeepBoth,
            merge: false,
            cancel: &cancel,
        };
        let mut placed = Placed::default();
        let mut tick = |_| {};
        let cross_device =
            |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices));
        let replace_during_remove = |path: &Path| {
            fs::remove_file(path).unwrap();
            fs::write(path, "concurrent replacement").unwrap();
            Err(CoreError::io(
                "delete",
                path,
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ))
        };
        assert!(transfer
            .place_with_ops(
                &source,
                &dest,
                &mut placed,
                &mut tick,
                &cross_device,
                &replace_during_remove,
            )
            .is_err());
        assert!(placed.created.is_empty());
        assert_eq!(placed.moved, [(source.clone(), dest.join("item.txt"))]);
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "concurrent replacement"
        );
        assert_eq!(
            fs::read_to_string(dest.join("item.txt")).unwrap(),
            "original"
        );
    }

    #[test]
    fn move_recovery_restores_missing_files_after_a_conflict() {
        let d = test_dir();
        let copy = d.path().join("copy");
        let source = d.path().join("source");
        fs::create_dir(&copy).unwrap();
        fs::create_dir(&source).unwrap();
        for name in ["first.txt", "second.txt"] {
            fs::write(copy.join(name), "original").unwrap();
        }
        let names: Vec<_> = fs::read_dir(&copy)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let changed = source.join(&names[0]);
        let missing = source.join(&names[1]);
        fs::write(&changed, "modified").unwrap();

        assert!(restore_missing_from_copy(&copy, &source).is_err());
        assert_eq!(fs::read_to_string(changed).unwrap(), "modified");
        assert_eq!(fs::read_to_string(missing).unwrap(), "original");
        for name in &names {
            assert_eq!(fs::read_to_string(copy.join(name)).unwrap(), "original");
        }
    }

    #[test]
    fn failed_move_recovery_detects_changed_nested_file_and_keeps_protective_undo() {
        let d = test_dir();
        let src = d.path().join("src");
        let dest = d.path().join("dest");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(src.join("nested/a-restored.txt"), "restore me").unwrap();
        fs::write(src.join("nested/b-changed.txt"), "original").unwrap();

        let cancel = AtomicBool::new(false);
        let transfer = Transfer {
            kind: TaskKind::Move,
            conflict: ConflictPolicy::KeepBoth,
            merge: false,
            cancel: &cancel,
        };
        let mut placed = Placed::default();
        let mut tick = |_| {};
        let cross_device =
            |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices));
        let partially_remove_and_modify = |path: &Path| {
            fs::remove_file(path.join("nested/a-restored.txt")).unwrap();
            fs::write(path.join("nested/b-changed.txt"), "modified").unwrap();
            Err(CoreError::io(
                "delete",
                path,
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ))
        };
        let target = dest.join("src");
        assert!(transfer
            .place_with_ops(
                &src,
                &dest,
                &mut placed,
                &mut tick,
                &cross_device,
                &partially_remove_and_modify,
            )
            .is_err());
        assert!(placed.created.is_empty());
        assert_eq!(placed.moved, [(src.clone(), target.clone())]);
        assert_eq!(
            fs::read_to_string(src.join("nested/a-restored.txt")).unwrap(),
            "restore me",
            "missing data was safely recovered"
        );
        assert_eq!(
            fs::read_to_string(src.join("nested/b-changed.txt")).unwrap(),
            "modified"
        );
        assert_eq!(
            fs::read_to_string(target.join("nested/b-changed.txt")).unwrap(),
            "original",
            "the completed copy remains intact"
        );

        let journal = UndoJournal::default();
        journal.push(UndoRecord::Moved {
            pairs: placed.moved,
        });
        assert!(matches!(journal.undo(), Err(CoreError::AlreadyExists(_))));
        assert_eq!(journal.len(), 1, "the protective undo record remains");
        assert_eq!(
            fs::read_to_string(src.join("nested/b-changed.txt")).unwrap(),
            "modified"
        );
        assert_eq!(
            fs::read_to_string(target.join("nested/b-changed.txt")).unwrap(),
            "original"
        );
    }

    #[test]
    fn failed_cross_device_undo_journals_duplicate_for_safe_cleanup() {
        let d = test_dir();
        let original = d.path().join("original");
        let moved_to = d.path().join("destination/item");
        fs::create_dir_all(moved_to.parent().unwrap()).unwrap();
        fs::write(&moved_to, "same data").unwrap();
        let cross_device =
            |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices));
        let fail_remove = |path: &Path| {
            Err(CoreError::io(
                "delete",
                path,
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ))
        };
        let (error, left) = revert_moved(
            vec![(original.clone(), moved_to.clone())],
            &cross_device,
            &fail_remove,
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::Io { .. }));
        let Some(UndoRecord::Batch { records, .. }) = left else {
            panic!("the duplicate must remain undoable");
        };
        assert!(matches!(
            records.as_slice(),
            [UndoRecord::Created { paths }] if paths == std::slice::from_ref(&moved_to)
        ));
        assert_eq!(fs::read_to_string(&original).unwrap(), "same data");
        assert_eq!(fs::read_to_string(&moved_to).unwrap(), "same data");

        let journal = UndoJournal::default();
        journal.push(UndoRecord::Batch {
            label: "recovered move copy".into(),
            records,
            dirs: Vec::new(),
        });
        journal.undo_with(true).unwrap();
        assert!(!moved_to.exists());
        assert_eq!(fs::read_to_string(original).unwrap(), "same data");
    }

    #[test]
    fn failed_replacement_keeps_copy_and_trash_in_undo_order() {
        let d = test_dir();
        let src_dir = d.path().join("src");
        let dest = d.path().join("dest");
        fs::create_dir(&src_dir).unwrap();
        fs::create_dir(&dest).unwrap();
        let source = src_dir.join("item.txt");
        let target = dest.join("item.txt");
        fs::write(&source, "new copy").unwrap();
        fs::write(&target, "old destination").unwrap();

        let cancel = AtomicBool::new(false);
        let transfer = Transfer {
            kind: TaskKind::Move,
            conflict: ConflictPolicy::Overwrite,
            merge: false,
            cancel: &cancel,
        };
        let mut placed = Placed::default();
        let mut tick = |_| {};
        let cross_device =
            |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices));
        let replace_during_remove = |path: &Path| {
            fs::remove_file(path).unwrap();
            fs::write(path, "concurrent source replacement").unwrap();
            Err(CoreError::io(
                "delete",
                path,
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ))
        };
        assert!(transfer
            .place_with_ops(
                &source,
                &dest,
                &mut placed,
                &mut tick,
                &cross_device,
                &replace_during_remove,
            )
            .is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "new copy");
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "concurrent source replacement"
        );

        let record = transfer_batch(
            TaskKind::Move,
            std::slice::from_ref(&source),
            placed.created,
            placed.moved,
            placed.replaced,
            placed.merged,
            placed.emptied,
        );
        let UndoRecord::Batch { records, .. } = &record else {
            panic!("replacement recovery must preserve its undo batch");
        };
        assert!(matches!(
            records.as_slice(),
            [UndoRecord::Trashed { .. }, UndoRecord::Moved { .. }]
        ));
        let journal = UndoJournal::default();
        journal.push(record);
        assert!(matches!(journal.undo(), Err(CoreError::AlreadyExists(_))));
        let stack = journal.stack.lock().unwrap();
        assert!(matches!(
            stack.last().map(|e| &e.record),
            Some(UndoRecord::Batch { records, .. })
                if matches!(records.as_slice(), [UndoRecord::Trashed { .. }, UndoRecord::Moved { .. }])
        ));
        assert_eq!(fs::read_to_string(&target).unwrap(), "new copy");
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "concurrent source replacement"
        );
    }

    #[test]
    fn batch_rename_refuses_duplicates_and_taken_names_without_changing_anything() {
        let d = test_dir();
        let p = |n: &str| d.path().join(n);
        for n in ["1.jpg", "2.jpg", "taken.jpg"] {
            fs::write(p(n), n).unwrap();
        }
        let j = UndoJournal::default();
        let before = names(d.path());
        let dup = batch_rename(
            &[
                (p("1.jpg"), "same.jpg".into()),
                (p("2.jpg"), "same.jpg".into()),
            ],
            &j,
        );
        assert!(dup
            .unwrap_err()
            .to_string()
            .contains("two items would be named “same.jpg”"));
        let taken = batch_rename(&[(p("1.jpg"), "taken.jpg".into())], &j);
        assert!(matches!(taken, Err(CoreError::AlreadyExists(_))));
        let bad = batch_rename(&[(p("1.jpg"), "a/b.jpg".into())], &j);
        assert!(matches!(bad, Err(CoreError::InvalidName(_))));
        assert_eq!(names(d.path()), before);
        assert!(j.peek_description().is_none());
    }

    /// A rename that fails half-way puts back those already done.
    #[cfg(unix)]
    #[test]
    fn rename_many_rolls_back_on_failure() {
        let d = test_dir();
        let p = |n: &str| d.path().join(n);
        fs::write(p("a"), "a").unwrap();
        fs::write(p("b"), "b").unwrap();
        // The second target's folder does not exist: that rename fails.
        let r = rename_many(&[(p("a"), p("a2")), (p("b"), p("missing/b2"))]);
        assert!(r.is_err());
        assert_eq!(names(d.path()), ["a=a", "b=b"]);
        // Same through temporary names (a swap forces them).
        let r = rename_many(&[(p("a"), p("b")), (p("b"), p("missing/a"))]);
        assert!(r.is_err());
        assert_eq!(names(d.path()), ["a=a", "b=b"]);
    }

    /// Copies keep permissions (also while copying: never more open than the source),
    /// folder dates and extended attributes.
    #[cfg(unix)]
    #[test]
    fn copies_keep_permissions_dates_and_attributes() {
        use std::os::unix::fs::PermissionsExt;
        let d = test_dir();
        let src = d.path().join("private");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/key"), "secret").unwrap();
        fs::set_permissions(src.join("sub/key"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(src.join("sub"), fs::Permissions::from_mode(0o750)).unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o700)).unwrap();
        let old = SystemTime::now() - Duration::from_secs(86_400 * 30);
        set_dir_modified(&src.join("sub"), old);
        set_dir_modified(&src, old);
        #[cfg(target_os = "linux")]
        let xattr_ok = {
            let p = std::ffi::CString::new(src.join("sub/key").to_str().unwrap()).unwrap();
            let n = std::ffi::CString::new("user.xdg.origin.url").unwrap();
            let v = b"https://example.com/key";
            unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), v.as_ptr().cast(), v.len(), 0) == 0 }
        };
        let dst = d.path().join("copy");
        copy_fresh(&src, &dst, &AtomicBool::new(false), &mut |_| {}, false).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dst), 0o700);
        assert_eq!(mode(&dst.join("sub")), 0o750);
        assert_eq!(mode(&dst.join("sub/key")), 0o600);
        let mtime = |p: &Path| fs::metadata(p).unwrap().modified().unwrap();
        assert_eq!(mtime(&dst), mtime(&src), "folder date kept");
        assert_eq!(mtime(&dst.join("sub")), mtime(&src.join("sub")));
        #[cfg(target_os = "linux")]
        if xattr_ok {
            let p = std::ffi::CString::new(dst.join("sub/key").to_str().unwrap()).unwrap();
            let n = std::ffi::CString::new("user.xdg.origin.url").unwrap();
            let mut buf = [0u8; 64];
            let len = unsafe {
                libc::getxattr(p.as_ptr(), n.as_ptr(), buf.as_mut_ptr().cast(), buf.len())
            };
            assert_eq!(&buf[..len.max(0) as usize], b"https://example.com/key");
        } else {
            eprintln!("no user xattrs on this file system: not checked");
        }
    }

    /// A failed undo (the old name was taken meanwhile) keeps the step: Ctrl+Z works
    /// again once the name is free, and nothing was moved half-way.
    #[test]
    fn failed_undo_can_be_retried() {
        let d = test_dir();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        for n in ["1.txt", "2.txt"] {
            fs::write(a.join(n), n).unwrap();
        }
        let m = mgr();
        let j = m.journal().clone();
        let mut req = task(
            TaskKind::Move,
            &a.join("1.txt"),
            &b,
            ConflictPolicy::KeepBoth,
        );
        req.sources.push(a.join("2.txt"));
        run(&m, req).unwrap();
        fs::write(a.join("2.txt"), "someone else").unwrap();
        assert!(matches!(j.undo(), Err(CoreError::AlreadyExists(_))));
        assert!(
            b.join("1.txt").exists() && b.join("2.txt").exists(),
            "nothing moved back half-way"
        );
        assert!(j.peek_description().is_some(), "the step is still there");
        fs::remove_file(a.join("2.txt")).unwrap();
        j.undo().unwrap();
        assert_eq!(fs::read_to_string(a.join("1.txt")).unwrap(), "1.txt");
        assert_eq!(fs::read_to_string(a.join("2.txt")).unwrap(), "2.txt");

        // A rename, through per-folder undo.
        let r = rename(&a.join("1.txt"), "one.txt", &j).unwrap();
        fs::write(a.join("1.txt"), "taken").unwrap();
        assert!(j.undo_in(&a).is_err());
        assert!(r.exists() && j.peek_in(&a).is_some());
        fs::remove_file(a.join("1.txt")).unwrap();
        j.undo_in(&a).unwrap();
        assert!(a.join("1.txt").exists() && !r.exists());
    }

    /// Replacing a folder that contains the source is refused instead of removing the
    /// source with it.
    #[test]
    fn replace_refuses_a_folder_holding_the_source() {
        let d = test_dir();
        let outer = d.path().join("Photos");
        let inner = outer.join("Photos");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("keep.jpg"), "x").unwrap();
        let m = mgr();
        for policy in [
            ConflictPolicy::Overwrite,
            ConflictPolicy::OverwritePermanently,
        ] {
            for kind in [TaskKind::Copy, TaskKind::Move] {
                let r = run(&m, task(kind, &inner, d.path(), policy));
                assert!(r.unwrap_err().to_string().contains("cannot be replaced"));
                assert!(inner.join("keep.jpg").exists());
            }
        }
        // Keep both is fine.
        run(
            &m,
            task(TaskKind::Copy, &inner, d.path(), ConflictPolicy::KeepBoth),
        )
        .unwrap();
        assert!(d.path().join("Photos (2)/keep.jpg").exists());
    }

    /// Every move and rename reaches the move hook, undo included, so tags stored by path
    /// follow. A swap goes through temporary names and still ends with the right pairs.
    #[test]
    fn move_hook_sees_renames_moves_swaps_and_undo() {
        use std::collections::HashMap;
        static TAGS: Mutex<Option<HashMap<PathBuf, u8>>> = Mutex::new(None);
        set_move_hook(|from, to| {
            let mut g = TAGS.lock().unwrap();
            let Some(tags) = g.as_mut() else { return };
            if let Some(c) = tags.remove(from) {
                tags.insert(to.to_path_buf(), c);
            }
        });
        let d = test_dir();
        let p = |n: &str| d.path().join(n);
        fs::create_dir(p("sub")).unwrap();
        for n in ["a", "b"] {
            fs::write(p(n), n).unwrap();
        }
        *TAGS.lock().unwrap() = Some(HashMap::from([(p("a"), 1), (p("b"), 2)]));
        let tag = |n: &str| TAGS.lock().unwrap().as_ref().unwrap().get(&p(n)).copied();
        let j = UndoJournal::default();
        batch_rename(&[(p("a"), "b".into()), (p("b"), "a".into())], &j).unwrap();
        assert_eq!(
            (tag("a"), tag("b")),
            (Some(2), Some(1)),
            "tags swapped with the names"
        );
        j.undo().unwrap();
        assert_eq!((tag("a"), tag("b")), (Some(1), Some(2)));
        let m = mgr();
        run(
            &m,
            task(TaskKind::Move, &p("a"), &p("sub"), ConflictPolicy::KeepBoth),
        )
        .unwrap();
        assert_eq!(tag("sub/a"), Some(1));
        m.journal().undo().unwrap();
        assert_eq!(tag("a"), Some(1), "undo brings the tag back");
        *TAGS.lock().unwrap() = None;
    }

    /// A folder with a named pipe and a socket copies (and moves to another drive)
    /// without hanging: the pipe is created again, the socket left out.
    #[cfg(unix)]
    #[test]
    fn special_files_do_not_hang_a_copy() {
        use std::os::unix::fs::FileTypeExt;
        let d = test_dir();
        let src = d.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        let fifo = std::ffi::CString::new(src.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o640) }, 0);
        let _sock = std::os::unix::net::UnixListener::bind(src.join("sock")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let dst = d.path().join("dst");
        std::thread::spawn(move || {
            let r = copy_fresh(&src, &dst, &AtomicBool::new(false), &mut |_| {}, false);
            let _ = tx.send((r.map_err(|e| e.to_string()), dst));
        });
        let (r, dst) = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the copy hung");
        r.unwrap();
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "a");
        assert!(fs::symlink_metadata(dst.join("pipe"))
            .unwrap()
            .file_type()
            .is_fifo());
        assert!(!dst.join("sock").exists());
        // Nothing that reads files blocks on the pipe either.
        assert!(crate::fs::open_regular(&dst.join("pipe")).is_err());
        let info = crate::preview::preview_info(&dst.join("pipe"), 1000).unwrap();
        assert_eq!(info.kind, Some(crate::preview::PreviewKind::None));
    }

    #[test]
    fn copy_into_itself_rejected() {
        let d = test_dir();
        let src = d.path().join("a");
        fs::create_dir_all(src.join("b")).unwrap();
        let m = mgr();
        let err = m
            .start(TaskRequest {
                kind: TaskKind::Copy,
                sources: vec![src.clone()],
                dest: Some(src.join("b")),
                conflict: Default::default(),
                archive_name: None,
                format: None,
                extract_mode: Default::default(),
                trash_archive: false,
                merge: false,
            })
            .unwrap_err();
        assert!(matches!(err, CoreError::IntoItself { .. }));
    }

    #[test]
    fn move_and_undo() {
        let d = test_dir();
        let a = d.path().join("a.txt");
        let dst = d.path().join("dst");
        fs::write(&a, "x").unwrap();
        fs::create_dir(&dst).unwrap();
        let m = mgr();
        run(
            &m,
            TaskRequest {
                kind: TaskKind::Move,
                sources: vec![a.clone()],
                dest: Some(dst.clone()),
                conflict: Default::default(),
                archive_name: None,
                format: None,
                extract_mode: Default::default(),
                trash_archive: false,
                merge: false,
            },
        )
        .unwrap();
        assert!(!a.exists() && dst.join("a.txt").exists());
        m.journal().undo().unwrap();
        assert!(a.exists() && !dst.join("a.txt").exists());
        assert!(matches!(m.journal().undo(), Err(CoreError::NothingToUndo)));
    }

    #[test]
    fn move_and_copy_folder_tree() {
        let d = test_dir();
        let src = d.path().join("Photos");
        fs::create_dir_all(src.join("2026/june")).unwrap();
        fs::write(src.join("2026/june/a.jpg"), b"img").unwrap();
        let dst = d.path().join("Archive");
        fs::create_dir(&dst).unwrap();
        let m = mgr();
        run(
            &m,
            TaskRequest {
                kind: TaskKind::Copy,
                sources: vec![src.clone()],
                dest: Some(dst.clone()),
                conflict: Default::default(),
                archive_name: None,
                format: None,
                extract_mode: Default::default(),
                trash_archive: false,
                merge: false,
            },
        )
        .unwrap();
        assert_eq!(
            fs::read(dst.join("Photos/2026/june/a.jpg")).unwrap(),
            b"img"
        );
        assert!(src.exists(), "copy keeps the source");
        let other = d.path().join("Other");
        fs::create_dir(&other).unwrap();
        run(
            &m,
            TaskRequest {
                kind: TaskKind::Move,
                sources: vec![src.clone()],
                dest: Some(other.clone()),
                conflict: Default::default(),
                archive_name: None,
                format: None,
                extract_mode: Default::default(),
                trash_archive: false,
                merge: false,
            },
        )
        .unwrap();
        assert!(!src.exists() && other.join("Photos/2026/june/a.jpg").exists());
    }

    #[test]
    fn same_folder_copy_duplicates() {
        let d = test_dir();
        let a = d.path().join("a.txt");
        fs::write(&a, "x").unwrap();
        let m = mgr();
        run(
            &m,
            TaskRequest {
                kind: TaskKind::Copy,
                sources: vec![a.clone()],
                dest: Some(d.path().into()),
                conflict: ConflictPolicy::Overwrite,
                archive_name: None,
                format: None,
                extract_mode: Default::default(),
                trash_archive: false,
                merge: false,
            },
        )
        .unwrap();
        assert!(
            d.path().join("a (2).txt").exists(),
            "same-folder paste must never overwrite the source"
        );
    }

    #[test]
    fn rename_mkdir_create_and_undo() {
        let d = test_dir();
        let j = UndoJournal::default();
        let dir = create_dir(d.path(), "New folder", &j).unwrap();
        assert!(dir.is_dir());
        assert!(matches!(
            create_dir(d.path(), "New folder", &j),
            Err(CoreError::AlreadyExists(_))
        ));
        let f = create_file(d.path(), "n.txt", None, Some(b"hi"), &j).unwrap();
        let r = rename(&f, "m.txt", &j).unwrap();
        assert_eq!(fs::read(&r).unwrap(), b"hi");
        j.undo().unwrap(); // undo rename
        assert!(f.exists() && !r.exists());
    }

    #[test]
    fn permanent_delete_task() {
        let d = test_dir();
        let sub = d.path().join("x");
        fs::create_dir_all(sub.join("y")).unwrap();
        let m = mgr();
        run(
            &m,
            TaskRequest {
                kind: TaskKind::Delete,
                sources: vec![sub.clone()],
                dest: None,
                conflict: Default::default(),
                archive_name: None,
                format: None,
                extract_mode: Default::default(),
                trash_archive: false,
                merge: false,
            },
        )
        .unwrap();
        assert!(!sub.exists());
    }

    #[test]
    fn duplicate_file() {
        let d = test_dir();
        let a = d.path().join("doc.md");
        fs::write(&a, "x").unwrap();
        let j = UndoJournal::default();
        assert_eq!(duplicate(&a, &j).unwrap(), d.path().join("doc (2).md"));
    }

    /// A "Replace" that is cancelled (or fails) after the old item went to the trash
    /// puts the old item back: the destination is never left with neither version.
    #[test]
    fn cancelled_replace_keeps_the_old_item() {
        let d = test_dir();
        let src = d.path().join("src");
        let dst = d.path().join("dst");
        fs::create_dir_all(src.join("folder")).unwrap();
        fs::create_dir_all(dst.join("folder")).unwrap();
        fs::write(src.join("folder/new.txt"), "new").unwrap();
        fs::write(dst.join("folder/old.txt"), "old").unwrap();
        let handle = Arc::new(TaskHandle {
            id: 1,
            cancel: AtomicBool::new(false),
        });
        let h = handle.clone();
        // Cancel as soon as the copy reports progress (after the old folder was trashed).
        let m = TaskManager::new(
            Arc::new(UndoJournal::default()),
            Arc::new(move |e| {
                if matches!(e, TaskEvent::Progress(_)) {
                    h.cancel.store(true, Ordering::Relaxed);
                }
            }),
        );
        let r = m.run(
            &handle,
            task(
                TaskKind::Copy,
                &src.join("folder"),
                &dst,
                ConflictPolicy::Overwrite,
            ),
        );
        assert!(matches!(r, Err(CoreError::Cancelled)), "{r:?}");
        assert_eq!(names(&dst.join("folder")), ["old.txt=old"]);
        assert!(m.journal().is_empty(), "nothing left to undo");
    }

    /// A sync copy that fails part-way leaves no partial copy behind: undo then brings
    /// back what the copy replaced, and a new item is not left half-copied.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn failed_sync_copy_leaves_no_partial_copy() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads unreadable files
        }
        let d = test_dir();
        let src = d.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("1-ok.txt"), "new").unwrap();
        let locked = src.join("2-locked.txt");
        fs::write(&locked, "secret").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let replaced = d.path().join("replaced");
        fs::create_dir(&replaced).unwrap();
        fs::write(replaced.join("old.txt"), "old").unwrap();
        let fresh = d.path().join("fresh");
        let j = UndoJournal::default();
        let cancel = AtomicBool::new(false);
        let r1 = apply_sync(
            &[SyncOp::Copy {
                src: src.clone(),
                dest: fresh.clone(),
            }],
            &j,
            &cancel,
        );
        let r2 = apply_sync(
            &[SyncOp::Copy {
                src: src.clone(),
                dest: replaced.clone(),
            }],
            &j,
            &cancel,
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(r1.is_err() && r2.is_err());
        assert!(!fresh.exists(), "no partial copy left");
        assert!(!replaced.exists(), "no partial copy left");
        j.undo().unwrap();
        assert_eq!(names(&replaced), ["old.txt=old"]);
    }

    /// Duplicating (or "keep both") a file whose name is close to the 255-byte limit
    /// shortens the name to fit the " (2)" instead of failing.
    #[test]
    fn unique_name_of_a_very_long_name_fits() {
        let d = test_dir();
        // 255 bytes, also with multi-byte letters (an "č" must not be cut in half).
        for name in [
            format!("{}.txt", "a".repeat(251)),
            format!("{}.txt", "č".repeat(125) + "a"),
        ] {
            let f = d.path().join(&name);
            // Some file systems count letters differently (decomposed "č" on HFS+).
            if fs::write(&f, "x").is_err() {
                continue;
            }
            let m = mgr();
            let mut req = task(TaskKind::Duplicate, &f, d.path(), ConflictPolicy::KeepBoth);
            req.dest = None;
            assert_eq!(run(&m, req).unwrap(), (1, 0));
            let copy = duplicate_targets(std::slice::from_ref(&f));
            let made: Vec<String> = fs::read_dir(d.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| *n != name && n.ends_with(" (2).txt"))
                .collect();
            assert_eq!(made.len(), 1, "{made:?}");
            assert!(made[0].len() <= 255);
            // The next copy continues the counter.
            assert!(copy[0].to_string_lossy().ends_with(" (3).txt"), "{copy:?}");
            fs::remove_file(d.path().join(&made[0])).unwrap();
            fs::remove_file(&f).unwrap();
        }
    }

    /// Merging folders that hold a file whose name is not valid UTF-8 (from an old
    /// archive or another system) does not stop half-way: the file is copied or moved too.
    #[cfg(target_os = "linux")]
    #[test]
    fn merge_handles_names_that_are_not_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let odd = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
        for kind in [TaskKind::Copy, TaskKind::Move] {
            let d = test_dir();
            let (src, dst) = (d.path().join("src/a"), d.path().join("dst/a"));
            fs::create_dir_all(&src).unwrap();
            fs::create_dir_all(&dst).unwrap();
            fs::write(src.join(odd), "odd").unwrap();
            fs::write(src.join("z.txt"), "z").unwrap();
            fs::write(dst.join(odd), "old").unwrap();
            let m = mgr();
            let r = run(
                &m,
                merge_task(kind, &src, &d.path().join("dst"), ConflictPolicy::KeepBoth),
            );
            assert_eq!(r.unwrap(), (1, 0), "{kind:?}");
            assert_eq!(fs::read(dst.join(odd)).unwrap(), b"old");
            assert_eq!(fs::read(dst.join("z.txt")).unwrap(), b"z");
            assert_eq!(
                fs::read_dir(&dst).unwrap().count(),
                3,
                "{kind:?}: kept both"
            );
        }
    }

    /// A new file from a template that cannot be read (deleted meanwhile, a folder)
    /// fails without leaving an empty file behind.
    #[test]
    fn create_file_from_a_missing_template_leaves_nothing() {
        let d = test_dir();
        let j = UndoJournal::default();
        for template in [d.path().join("gone.docx"), d.path().to_path_buf()] {
            let r = create_file(d.path(), "new.docx", Some(&template), None, &j);
            assert!(r.is_err(), "{template:?}");
            assert!(!d.path().join("new.docx").exists(), "{template:?}");
        }
        assert!(j.is_empty());
    }

    /// Undoing the delete of a symlink to a folder brings the symlink back. (The trash
    /// crate followed the link, made an empty folder in its place and failed.)
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn undo_delete_of_a_symlink_to_a_folder() {
        let d = test_dir();
        let target = d.path().join("Projects");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("a.txt"), "a").unwrap();
        let link = d.path().join("projects-link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let j = UndoJournal::default();
        trash_items(std::slice::from_ref(&link), &j).unwrap();
        assert!(fs::symlink_metadata(&link).is_err());
        j.undo().unwrap();
        let m = fs::symlink_metadata(&link).unwrap();
        assert!(m.file_type().is_symlink(), "a symlink again, not a folder");
        assert_eq!(fs::read_link(&link).unwrap(), target);
        assert_eq!(names(&target), ["a.txt=a"]);
    }

    /// Renaming to the name of a hard link of the same file is refused like any taken
    /// name: it is another entry, not the same one with other letter case (rename did
    /// nothing and a batch rename left a hidden temporary file).
    #[cfg(unix)]
    #[test]
    fn rename_onto_a_hard_link_is_refused() {
        let d = test_dir();
        let (a, b) = (d.path().join("a.txt"), d.path().join("b.txt"));
        fs::write(&a, "x").unwrap();
        fs::hard_link(&a, &b).unwrap();
        let j = UndoJournal::default();
        assert!(matches!(
            rename(&a, "b.txt", &j),
            Err(CoreError::AlreadyExists(_))
        ));
        assert!(matches!(
            batch_rename(&[(a.clone(), "b.txt".into())], &j),
            Err(CoreError::AlreadyExists(_))
        ));
        assert_eq!(names(d.path()), ["a.txt=x", "b.txt=x"]);
        // Where case matters (Linux), "A.txt" is another name too.
        if !cfg!(target_os = "macos") {
            fs::hard_link(&a, d.path().join("A.txt")).unwrap();
            assert!(matches!(
                rename(&a, "A.txt", &j),
                Err(CoreError::AlreadyExists(_))
            ));
        }
        assert!(j.is_empty());
    }

    /// Two sources with one name (from search results) moved or copied with "Replace":
    /// the second does not replace what the same operation just put there (the first
    /// item went to the trash and undo failed), it keeps both.
    #[test]
    fn replace_never_replaces_an_item_of_the_same_operation() {
        for kind in [TaskKind::Copy, TaskKind::Move] {
            let d = test_dir();
            let p = |s: &str| d.path().join(s);
            for dir in ["x", "y", "dest"] {
                fs::create_dir(p(dir)).unwrap();
            }
            fs::write(p("x/f.txt"), "x").unwrap();
            fs::write(p("y/f.txt"), "y").unwrap();
            fs::write(p("dest/f.txt"), "old").unwrap();
            let before = tree(d.path());
            let m = mgr();
            let mut req = task(kind, &p("x/f.txt"), &p("dest"), ConflictPolicy::Overwrite);
            req.sources.push(p("y/f.txt"));
            assert_eq!(run(&m, req).unwrap(), (2, 0), "{kind:?}");
            assert_eq!(names(&p("dest")), ["f (2).txt=y", "f.txt=x"], "{kind:?}");
            m.journal().undo().unwrap();
            assert_eq!(tree(d.path()), before, "{kind:?}");
        }
    }

    #[test]
    fn case_insensitive_path_helpers() {
        let ci = cfg!(any(windows, target_os = "macos"));
        assert!(same_path_ci(Path::new("/a/B.txt"), Path::new("/a/B.txt")));
        assert_eq!(
            same_path_ci(Path::new("/a/B.txt"), Path::new("/a/b.txt")),
            ci
        );
        assert_eq!(
            starts_with_ci(Path::new("/x/photos/sub"), Path::new("/x/Photos")),
            ci
        );
        assert!(starts_with_ci(
            Path::new("/x/Photos/sub"),
            Path::new("/x/Photos")
        ));
        assert!(!starts_with_ci(
            Path::new("/x/Photos2"),
            Path::new("/x/Photos")
        ));
    }

    /// macOS: a copy keeps the Finder tag and comment stored as extended attributes.
    #[cfg(target_os = "macos")]
    #[test]
    fn copy_keeps_extended_attributes_on_macos() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let d = test_dir();
        let src = d.path().join("tagged.txt");
        fs::write(&src, "x").unwrap();
        let (p, name) = (
            CString::new(src.as_os_str().as_bytes()).unwrap(),
            CString::new("com.apple.metadata:test_gall").unwrap(),
        );
        let v = b"value";
        let rc =
            unsafe { libc::setxattr(p.as_ptr(), name.as_ptr(), v.as_ptr().cast(), v.len(), 0, 0) };
        assert_eq!(rc, 0, "set the attribute");
        let dst = d.path().join("copy.txt");
        let (cancel, mut tick) = (AtomicBool::new(false), |_: u64| {});
        copy_file(&src, &dst, &cancel, &mut tick, false).unwrap();
        let q = CString::new(dst.as_os_str().as_bytes()).unwrap();
        let mut buf = [0u8; 16];
        let n = unsafe {
            libc::getxattr(
                q.as_ptr(),
                name.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                0,
                0,
            )
        };
        assert_eq!(&buf[..n.max(0) as usize], v);
    }

    #[test]
    fn undo_by_id_reverses_only_the_targeted_operation_and_preserves_order() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        let dst = d.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        let a = src.join("first.txt");
        let b = src.join("second.txt");
        fs::write(&a, "one").unwrap();
        fs::write(&b, "two").unwrap();

        let events = Arc::new(Mutex::new(Vec::<TaskEvent>::new()));
        let sink = events.clone();
        let m = TaskManager::new(
            Arc::new(UndoJournal::default()),
            Arc::new(move |ev| {
                sink.lock().unwrap().push(ev);
            }),
        );

        let run_copy = |file: &Path| {
            let h = TaskHandle {
                id: 1,
                cancel: AtomicBool::new(false),
            };
            let (res, undo_id) = m.run_named(
                &h,
                TaskRequest {
                    kind: TaskKind::Copy,
                    sources: vec![file.to_path_buf()],
                    dest: Some(dst.clone()),
                    conflict: ConflictPolicy::KeepBoth,
                    archive_name: None,
                    format: None,
                    extract_mode: Default::default(),
                    trash_archive: false,
                    merge: false,
                },
            );
            res.unwrap();
            undo_id.expect("copy produces an undo record id")
        };

        let first_id = run_copy(&a);
        let second_id = run_copy(&b);
        assert_ne!(first_id, second_id);
        assert!(dst.join("first.txt").exists());
        assert!(dst.join("second.txt").exists());

        // Clicking Undo on the older card removes ONLY first.txt; second.txt stays intact.
        m.journal().undo_by_id_with(first_id, true).unwrap();
        assert!(!dst.join("first.txt").exists());
        assert!(dst.join("second.txt").exists());

        // Clicking the same older card again reports NothingToUndo and leaves second.txt untouched.
        assert!(matches!(
            m.journal().undo_by_id_with(first_id, true),
            Err(CoreError::NothingToUndo)
        ));
        assert!(dst.join("second.txt").exists());
        assert_eq!(m.journal().len(), 1);

        // Global undo still reverses the remaining newer operation in normal LIFO order.
        m.journal().undo_with(true).unwrap();
        assert!(!dst.join("second.txt").exists());
        assert!(m.journal().is_empty());
    }

    #[test]
    fn undo_by_id_preserves_identity_on_retry_and_refuses_overlapping_newer_ops() {
        let d = tempfile::tempdir().unwrap();
        let f1 = d.path().join("alpha.txt");
        let f2 = d.path().join("beta.txt");
        fs::write(&f1, "alpha").unwrap();
        fs::write(&f2, "beta").unwrap();

        let j = UndoJournal::default();
        let renamed1 = rename(&f1, "alpha-moved.txt", &j).unwrap();
        let id1 = j.latest_id().unwrap();
        let renamed2 = rename(&f2, "beta-moved.txt", &j).unwrap();
        let id2 = j.latest_id().unwrap();

        // Block id1 by creating a conflicting file at alpha.txt; retry must preserve id1.
        fs::write(&f1, "blocker").unwrap();
        assert!(matches!(
            j.undo_by_id(id1),
            Err(CoreError::AlreadyExists(_))
        ));
        assert!(
            j.has_id(id1),
            "failed undo_by_id keeps its stable id for retry"
        );
        assert!(
            renamed2.exists(),
            "newer unrelated operation stays untouched"
        );

        // Remove blocker and retry id1 directly.
        fs::remove_file(&f1).unwrap();
        j.undo_by_id(id1).unwrap();
        assert!(f1.exists());
        assert!(!renamed1.exists());
        assert!(renamed2.exists());

        // Overlap guard: rename beta-moved.txt again so a newer record overlaps id2.
        let renamed2b = rename(&renamed2, "beta-final.txt", &j).unwrap();
        let id3 = j.latest_id().unwrap();
        let err = j.undo_by_id(id2).unwrap_err();
        assert!(err.to_string().contains("newer operation"));
        assert!(renamed2b.exists());
        assert!(j.has_id(id2) && j.has_id(id3));

        // Undoing the newer overlapping op first unblocks id2.
        j.undo_by_id(id3).unwrap();
        j.undo_by_id(id2).unwrap();
        assert!(f2.exists());
        assert!(j.is_empty());
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn undo_by_id_retry_after_partial_failure_keeps_same_id() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.txt");
        let b = d.path().join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        let j = UndoJournal::default();
        let id = j.push(UndoRecord::Created {
            paths: vec![a.clone(), b.clone()],
        });
        let other = d.path().join("other.txt");
        fs::write(&other, "other").unwrap();
        let other_id = j.push(UndoRecord::Created {
            paths: vec![other.clone()],
        });

        let err = j
            .undo_by_id_with_reverter(id, false, |_, _| {
                Err((
                    CoreError::Other("no-trash: simulated".into()),
                    Some(UndoRecord::Created {
                        paths: vec![b.clone()],
                    }),
                ))
            })
            .unwrap_err();
        assert!(err.to_string().starts_with("no-trash:"));
        assert!(j.has_id(id));
        assert!(j.has_id(other_id));

        // Permanent retry targets the same id and removes only its remaining path.
        j.undo_by_id_with(id, true).unwrap();
        assert!(!b.exists());
        assert!(other.exists());
        assert!(!j.has_id(id));
        assert!(j.has_id(other_id));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apfs_clonefile_preserves_data_mode_durable_and_refuses_overwrite() {
        use std::os::unix::fs::PermissionsExt;
        let d = test_dir();
        let src = d.path().join("big.bin");
        let payload = vec![0xA5u8; 2 * 1024 * 1024];
        fs::write(&src, &payload).unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o444)).unwrap();

        let dst = d.path().join("cloned.bin");
        let cancel = AtomicBool::new(false);
        let mut reported = 0u64;
        let mut tick = |n: u64| reported += n;
        copy_file(&src, &dst, &cancel, &mut tick, true).unwrap();

        assert_eq!(fs::read(&dst).unwrap(), payload);
        assert_eq!(reported, payload.len() as u64);
        assert_eq!(
            fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
            0o444
        );

        // Must never overwrite an existing destination.
        let mut tick2 = |_: u64| {};
        assert!(copy_file(&src, &dst, &cancel, &mut tick2, false).is_err());

        // Pre-cancelled copy must fail with Cancelled and not create dst.
        let dst2 = d.path().join("cancelled.bin");
        let cancelled = AtomicBool::new(true);
        let err = copy_file(&src, &dst2, &cancelled, &mut tick2, false).unwrap_err();
        assert!(matches!(err, CoreError::Cancelled));
        assert!(!dst2.exists());

        // Restore write bit so tempdir cleanup succeeds cleanly.
        let _ = fs::set_permissions(&src, fs::Permissions::from_mode(0o644));
        let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o644));
    }
}
