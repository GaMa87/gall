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

#[derive(Default)]
pub struct UndoJournal {
    stack: Mutex<Vec<UndoRecord>>,
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
    pub fn push(&self, r: UndoRecord) {
        let mut s = self.stack.lock().unwrap();
        s.push(r);
        if s.len() > UNDO_LIMIT {
            s.remove(0);
        }
    }

    /// Number of steps that can be undone.
    pub fn len(&self) -> usize {
        self.stack.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn peek_description(&self) -> Option<String> {
        self.stack.lock().unwrap().last().map(|r| r.describe())
    }

    /// Description of the most recent operation that touched `dir` (per-panel undo).
    pub fn peek_in(&self, dir: &Path) -> Option<String> {
        self.stack
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|r| r.touches(dir))
            .map(|r| r.describe())
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
        let (at, rec) = {
            let mut s = self.stack.lock().unwrap();
            let at = s.len().checked_sub(1).ok_or(CoreError::NothingToUndo)?;
            (at, s.pop().expect("stack length checked"))
        };
        revert(rec, permanent).map_err(|(e, left)| {
            if let Some(r) = left {
                let mut s = self.stack.lock().unwrap();
                let at = at.min(s.len());
                s.insert(at, r);
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
        let (i, rec) = {
            let mut s = self.stack.lock().unwrap();
            let i = s
                .iter()
                .rposition(|r| r.touches(dir))
                .ok_or(CoreError::NothingToUndo)?;
            (i, s.remove(i))
        };
        Self::revert(rec, permanent).map_err(|(e, left)| {
            if let Some(r) = left {
                let mut s = self.stack.lock().unwrap();
                let at = i.min(s.len());
                s.insert(at, r);
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
    let mut items = with_trash(trash::os_limited::list)?;
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
/// Items sent to the trash so far (tests: see `tests::TestDir`).
#[cfg(test)]
static TRASHED: AtomicU64 = AtomicU64::new(0);

fn trash_paths(paths: &[PathBuf]) -> Result<()> {
    #[cfg(test)]
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
    let owned = paths.to_vec();
    with_trash(move || trash::delete_all(owned))
}

#[cfg(not(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
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
fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
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
        return Ok(out);
    }
    rename_many(&pairs)?;
    journal.push(UndoRecord::Moved { pairs });
    Ok(out)
}

pub fn create_dir(parent: &Path, name: &str, journal: &UndoJournal) -> Result<PathBuf> {
    validate_name(name)?;
    let p = parent.join(name);
    if exists_no_follow(&p) {
        return Err(CoreError::AlreadyExists(p));
    }
    fs::create_dir(&p).ctx("create folder", &p)?;
    journal.push(UndoRecord::Created {
        paths: vec![p.clone()],
    });
    Ok(p)
}

/// Creates a new file, either empty, with `contents`, or cloned from a template.
pub fn create_file(
    parent: &Path,
    name: &str,
    template: Option<&Path>,
    contents: Option<&[u8]>,
    journal: &UndoJournal,
) -> Result<PathBuf> {
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
    journal.push(UndoRecord::Created {
        paths: vec![p.clone()],
    });
    Ok(p)
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

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub copied: usize,
    pub replaced: usize,
    pub trashed: usize,
    pub bytes: u64,
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
        journal.push(UndoRecord::Batch {
            label: format!("sync of {}", items(rep.copied + rep.trashed)),
            records,
            dirs: Vec::new(),
        });
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
pub fn save_text(path: &Path, text: &str, journal: &UndoJournal) -> Result<bool> {
    let real = if exists_no_follow(path) {
        fs::canonicalize(path).ctx("resolve", path)?
    } else {
        path.to_path_buf()
    };
    let dir = real
        .parent()
        .ok_or_else(|| CoreError::NotFound(real.clone()))?;
    let tmp = dir.join(format!(
        ".{}.gall-save-{}",
        file_name(&real)?,
        std::process::id()
    ));
    let guard = DeleteOnDrop(Some(tmp.clone()));
    {
        let mut f = fs::File::create(&tmp).ctx("save", &tmp)?;
        f.write_all(text.as_bytes()).ctx("save", &tmp)?;
        f.sync_all().ctx("save", &tmp)?;
    }
    if let Ok(m) = fs::metadata(&real) {
        let _ = fs::set_permissions(&tmp, m.permissions());
    }
    let mut records = Vec::new();
    let mut undoable = true;
    if exists_no_follow(&real) {
        match trash_timed(std::slice::from_ref(&real)) {
            Ok((at, until)) => records.push(UndoRecord::Trashed {
                paths: vec![real.clone()],
                at,
                until,
            }),
            Err(e) => {
                log::warn!("saving {} without undo: {e}", real.display());
                undoable = false;
            }
        }
    }
    // Over the old file when it could not be trashed: rename replaces atomically.
    let res = fs::rename(&tmp, &real).ctx("save", &real);
    if res.is_ok() {
        guard.disarm();
        records.push(UndoRecord::Created {
            paths: vec![real.clone()],
        });
    }
    // Recorded even when the rename failed: undo then brings the old version back.
    if undoable && !records.is_empty() {
        journal.push(UndoRecord::Batch {
            label: format!("save {}", quoted(&real)),
            records,
            dirs: Vec::new(),
        });
    }
    res.map(|()| undoable)
}

pub fn trash_items(paths: &[PathBuf], journal: &UndoJournal) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
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
    if !gone.is_empty() {
        journal.push(UndoRecord::Trashed {
            paths: gone,
            at,
            until: chrono::Utc::now().timestamp_millis(),
        });
    }
    res
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
    },
    Failed {
        id: u64,
        kind: TaskKind,
        error: String,
    },
    Cancelled {
        id: u64,
        kind: TaskKind,
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

fn ensure_not_into_itself(op: &'static str, src: &Path, dest_dir: &Path) -> Result<()> {
    let src_c = src.canonicalize().unwrap_or_else(|_| src.to_path_buf());
    let dst_c = dest_dir
        .canonicalize()
        .unwrap_or_else(|_| dest_dir.to_path_buf());
    if dst_c.starts_with(&src_c)
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
    #[cfg(not(target_os = "linux"))]
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
    #[cfg(target_os = "linux")]
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
        #[cfg(target_os = "linux")]
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
                self.merge_into(src, &target, out, tick)?;
                return Ok(true);
            }
            let policy = match self.conflict {
                ConflictPolicy::OverwriteOlder if newer(src, &target) => ConflictPolicy::Overwrite,
                ConflictPolicy::OverwriteOlder => ConflictPolicy::Skip,
                p => p,
            };
            // What this operation put there itself (two sources with one name, from search
            // results) is never replaced: the first item would go to the trash.
            let ours = out.created.contains(&target) || out.moved.iter().any(|(_, t)| *t == target);
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
    ) -> Result<()> {
        out.merged.push(target.to_path_buf());
        let mut children: Vec<PathBuf> = fs::read_dir(src)
            .ctx("list", src)?
            .map(|e| e.map(|e| e.path()).ctx("list", src))
            .collect::<Result<_>>()?;
        children.sort();
        for child in &children {
            self.place(child, target, out, tick)?;
        }
        tick(0); // the folder itself, as counted by `measure`
        if self.kind == TaskKind::Move && fs::remove_dir(src).is_ok() {
            out.emptied.push(src.to_path_buf());
        }
        Ok(())
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
                let ev = match me.run_named(&handle, req) {
                    Ok((items, skipped, output)) => TaskEvent::Done {
                        id,
                        kind,
                        items,
                        skipped,
                        name: output.or(name.take()),
                        to,
                    },
                    Err(CoreError::Cancelled) => TaskEvent::Cancelled { id, kind },
                    Err(e) => TaskEvent::Failed {
                        id,
                        kind,
                        error: e.to_string(),
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
        self.run_named(handle, req).map(|(d, s, _)| (d, s))
    }

    /// Like [`run`](Self::run), plus the name of the single item created
    /// (the archive, or the extracted folder), for the "done" message.
    fn run_named(
        &self,
        handle: &TaskHandle,
        req: TaskRequest,
    ) -> Result<(u64, u64, Option<String>)> {
        // Names inside WSL that Windows cannot hold or address: refused before anything.
        super::wsl::check_task(req.kind, &req.sources, req.dest.as_deref(), &handle.cancel)?;
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
        // Record whatever completed, even on failure/cancel, so it can be undone.
        if !trashed.is_empty() {
            // Extract and trash: one undo restores the archives and removes what was unpacked.
            let label = format!("extract and delete {}", what(&trashed));
            self.journal.push(UndoRecord::Batch {
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
            self.journal.push(transfer_batch(
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
                self.journal.push(UndoRecord::Created { paths: created });
            }
            if !moved.is_empty() {
                self.journal.push(UndoRecord::Moved { pairs: moved });
            }
        }
        result.map(|_| (done_top, skipped, output))
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
    use super::*;

    /// A test's temporary folder. Replace, delete and undo use the real trash: when the
    /// test ends, what went there from this folder is purged as well (it piled up in the
    /// developer's Recycle Bin / ~/.local/share/Trash with every `cargo test`).
    pub(crate) struct TestDir {
        dir: tempfile::TempDir,
        trashed_before: u64,
    }

    impl std::ops::Deref for TestDir {
        type Target = tempfile::TempDir;
        fn deref(&self) -> &tempfile::TempDir {
            &self.dir
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            if TRASHED.load(Ordering::Relaxed) != self.trashed_before {
                purge_trashed_from(self.dir.path());
            }
        }
    }

    pub(crate) fn test_dir() -> TestDir {
        TestDir {
            trashed_before: TRASHED.load(Ordering::Relaxed),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    #[cfg(any(
        windows,
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    ))]
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

    #[cfg(not(any(
        windows,
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    )))]
    fn purge_trashed_from(_dir: &Path) {}
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
        // macOS cannot restore from the trash (see `restore_from_trash`): the rest is untestable there.
        if cfg!(target_os = "macos") {
            return;
        }
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
        // macOS cannot restore from the trash (see `restore_from_trash`): the rest is untestable there.
        if cfg!(target_os = "macos") {
            return;
        }
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
        // macOS cannot restore from the trash (see `restore_from_trash`): the rest is untestable there.
        if cfg!(target_os = "macos") {
            return;
        }
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
        if cfg!(target_os = "macos") {
            return; // restoring from the trash is not supported there
        }
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
            stack.last(),
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
        if cfg!(target_os = "macos") {
            return; // restoring from the trash is not supported there
        }
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
        if cfg!(target_os = "macos") {
            return; // restoring from the trash is not supported there
        }
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
}
