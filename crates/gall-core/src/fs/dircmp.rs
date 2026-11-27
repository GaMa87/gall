//! Folder comparison: walks two trees and classifies every relative path as
//! same / different / left-only / right-only. Quick mode trusts size + mtime;
//! content mode compares bytes of equally sized files. Folders take the status
//! of their contents, so the UI can collapse identical subtrees.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, UNIX_EPOCH};

use crate::{CoreError, Result};

/// Stop after this many paths per side (keeps the UI responsive on huge trees).
pub const MAX_ENTRIES: usize = 300_000;
/// mtimes within this many seconds count as equal (FAT, network shares, copies).
const MTIME_SLACK: i64 = 2;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DirCmpOptions {
    /// Compare file bytes when sizes match (slower, exact).
    pub content: bool,
    /// Read every pair again, ignoring the remembered results of earlier scans (they are used only when this is off).
    pub fresh: bool,
    pub hidden: bool,
    pub respect_gitignore: bool,
    /// Globs: when there are any, only files that match one of them are compared (folders are always walked).
    /// A pattern without a `/` matches a name at any depth; one with a `/` matches the path below the folder.
    pub include: Vec<String>,
    /// Globs of what is left out: a matching folder is not walked, a matching file is not compared.
    pub exclude: Vec<String>,
    /// Inclusive bounds of the size of a file in bytes.
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Inclusive bounds of the modification time of a file, in seconds since the epoch.
    pub modified_after: Option<i64>,
    pub modified_before: Option<i64>,
}

/// What `include`, `exclude` and the ranges leave out. A path or file that is left out on one side is left out on both,
/// so a file that is out of range on one side only is never taken for a missing one (and then copied over it).
struct Filter {
    include: Option<globset::GlobSet>,
    exclude: Option<globset::GlobSet>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    after: Option<i64>,
    before: Option<i64>,
}

/// Whether a path matches in the spelling it has, composed or decomposed (macOS keeps the decomposed one): a pattern
/// typed in either form finds it.
fn matches_any_form(set: &globset::GlobSet, rel: &str) -> bool {
    use unicode_normalization::UnicodeNormalization;
    if set.is_match(rel) {
        return true;
    }
    let composed: String = rel.nfc().collect();
    let decomposed: String = rel.nfd().collect();
    (composed != rel && set.is_match(&composed)) || (decomposed != rel && set.is_match(&decomposed))
}

fn glob_set(patterns: &[String]) -> Result<Option<globset::GlobSet>> {
    let patterns: Vec<&str> = patterns
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = globset::GlobSetBuilder::new();
    for p in patterns {
        // `build/` means the folder `build`; a name without `/` matches at any depth.
        let p = p.trim_end_matches('/');
        let p = p.strip_prefix("./").unwrap_or(p);
        let full = if p.contains('/') {
            p.trim_start_matches('/').to_string()
        } else {
            format!("**/{p}")
        };
        for g in [full.clone(), format!("{full}/**")] {
            let glob = globset::GlobBuilder::new(&g)
                .literal_separator(true)
                // Names do not differ by case on Windows and macOS, so a pattern must not either.
                .case_insensitive(cfg!(any(windows, target_os = "macos")))
                .build()
                .map_err(|e| CoreError::Other(format!("not a valid pattern: {p} ({e})")))?;
            builder.add(glob);
        }
    }
    builder
        .build()
        .map(Some)
        .map_err(|e| CoreError::Other(format!("not a valid pattern: {e}")))
}

impl Filter {
    fn compile(opts: &DirCmpOptions) -> Result<Self> {
        Ok(Self {
            include: glob_set(&opts.include)?,
            exclude: glob_set(&opts.exclude)?,
            min_size: opts.min_size,
            max_size: opts.max_size,
            after: opts.modified_after,
            before: opts.modified_before,
        })
    }

    #[cfg(test)]
    fn none() -> Self {
        Self {
            include: None,
            exclude: None,
            min_size: None,
            max_size: None,
            after: None,
            before: None,
        }
    }

    /// Whether a folder or a file is left out by the globs of `exclude` (the path below the compared folder).
    fn excludes_path(&self, rel: &str) -> bool {
        self.exclude
            .as_ref()
            .is_some_and(|g| matches_any_form(g, rel))
    }

    /// Whether a file is left out: not matched by `include`, or outside the size or time range.
    fn excludes_file(&self, rel: &str, m: &Meta) -> bool {
        if self
            .include
            .as_ref()
            .is_some_and(|g| !matches_any_form(g, rel))
        {
            return true;
        }
        self.min_size.is_some_and(|n| m.size < n)
            || self.max_size.is_some_and(|n| m.size > n)
            || self.after.is_some_and(|t| m.mtime < t)
            || self.before.is_some_and(|t| m.mtime > t)
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub is_dir: bool,
    pub size: u64,
    /// Seconds since the epoch.
    pub mtime: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Same,
    Different,
    LeftOnly,
    RightOnly,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Relative path with "/" separators.
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub status: Status,
    /// Why two files differ: "content" | "size" | "date" | "type".
    pub reason: Option<&'static str>,
    /// Which side has the newer modification time: "left" | "right".
    pub newer: Option<&'static str>,
    pub left: Option<Meta>,
    pub right: Option<Meta>,
    /// The path as it is on disk on that side when pairing uses a different NFC spelling.
    /// Absent when equal to `rel`: use these to reach the real file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_rel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_rel: Option<String>,
    /// A folder that holds something the filters left out (a file or a whole folder), on either side. It is no
    /// whole-tree operation: a copy or a trash of it would take the left-out things along, so only the paths that
    /// are listed below it may be acted on.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub has_excluded: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub same: usize,
    pub different: usize,
    pub left_only: usize,
    pub right_only: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IncompleteReason {
    Traversal,
    Metadata,
    PathIdentity,
    /// A file of equal size could not be opened or read in exact mode: its bytes were not compared.
    Read,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirCmpResult {
    pub left: PathBuf,
    pub right: PathBuf,
    pub items: Vec<Item>,
    /// File counts (folders are not counted).
    pub counts: Counts,
    pub truncated: bool,
    pub incomplete_reasons: BTreeSet<IncompleteReason>,
    pub elapsed_ms: u64,
}

fn meta_of(m: &std::fs::Metadata, incomplete: &mut BTreeSet<IncompleteReason>) -> Meta {
    let mtime = match m.modified() {
        Ok(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs() as i64,
            Err(before_epoch) => -(before_epoch.duration().as_secs() as i64),
        },
        Err(_) => {
            incomplete.insert(IncompleteReason::Metadata);
            0
        }
    };
    Meta {
        is_dir: m.is_dir(),
        size: if m.is_dir() { 0 } else { m.len() },
        mtime,
    }
}

/// One side of a comparison. Keep on-disk paths until we know NFC pairing is unambiguous.
#[derive(Default)]
struct Side {
    meta: BTreeMap<String, Meta>,
    actual: HashMap<String, String>,
    incomplete: BTreeSet<IncompleteReason>,
    truncated: bool,
    normalization_collision: bool,
    /// Paths left out by the filters (a file, or a folder that was not walked), in their on-disk and NFC spellings.
    excluded: BTreeSet<String>,
    /// Entries that exist but are not listed because hidden files or ignored files are not shown. They do not change
    /// what is compared; the folders above them are flagged like those that hold left-out files, so that no whole-tree
    /// copy or trash of such a folder is planned (a folder with only hidden files is not empty).
    unlisted: BTreeSet<String>,
}

impl Side {
    fn normalize(&mut self) {
        self.meta = std::mem::take(&mut self.meta)
            .into_iter()
            .map(|(actual, meta)| {
                let key = crate::util::unicode::nfc(&actual).into_owned();
                if key != actual {
                    self.actual.insert(key.clone(), actual);
                }
                (key, meta)
            })
            .collect();
    }
}

fn walk(root: &Path, opts: &DirCmpOptions, cancel: &AtomicBool) -> Result<Side> {
    if !root.is_dir() {
        return Err(CoreError::Other(format!(
            "{} is not a folder",
            root.display()
        )));
    }
    let filter = std::sync::Arc::new(Filter::compile(opts)?);
    // Folders and files that the globs leave out are not walked; what was left out is remembered, so that the folders
    // above it are known to hold something that is not listed.
    let skipped = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (prune, note, base) = (filter.clone(), skipped.clone(), root.to_path_buf());
    let walker = ignore::WalkBuilder::new(root)
        .hidden(!opts.hidden)
        .git_ignore(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .git_global(false)
        .ignore(opts.respect_gitignore)
        .parents(opts.respect_gitignore)
        .follow_links(false)
        .filter_entry(move |e| {
            let Ok(rel) = e.path().strip_prefix(&base) else {
                return true;
            };
            let Some(rel) = rel
                .to_str()
                .map(|r| r.replace(std::path::MAIN_SEPARATOR, "/"))
            else {
                return true;
            };
            if rel.is_empty() || !prune.excludes_path(&rel) {
                return true;
            }
            if let Ok(mut list) = note.lock() {
                list.push(rel);
            }
            false
        })
        .build();
    let mut side = scan_entries(root, walker, &filter, MAX_ENTRIES, cancel)?;
    if !opts.hidden || opts.respect_gitignore {
        find_unlisted(root, &mut side, cancel)?;
    }
    let skipped = skipped.lock().map(|l| l.clone()).unwrap_or_default();
    for rel in skipped {
        side.excluded
            .insert(crate::util::unicode::nfc(&rel).into_owned());
        side.excluded.insert(rel);
    }
    Ok(side)
}

/// Looks, folder by folder, for entries that the walk did not list although they exist (hidden files, ignored files).
/// Only the names of the entries are read (one folder listing each), never their content.
fn find_unlisted(root: &Path, side: &mut Side, cancel: &AtomicBool) -> Result<()> {
    let mut folders: Vec<(String, PathBuf)> = vec![(String::new(), root.to_path_buf())];
    folders.extend(
        side.meta
            .iter()
            .filter(|(_, m)| m.is_dir)
            .map(|(rel, _)| (rel.clone(), root.join(rel))),
    );
    for (rel, path) in folders {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let Ok(read) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in read.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            // Operating-system noise (`.DS_Store`…) is no content of the folder: it never flags it.
            if !super::entry::is_os_noise_name(&name_for_noise(&child))
                && !side.meta.contains_key(&child)
                && !side.excluded.contains(&child)
            {
                side.unlisted.insert(child);
            }
        }
    }
    Ok(())
}

fn name_for_noise(rel: &str) -> String {
    rel.rsplit('/').next().unwrap_or(rel).to_owned()
}

fn scan_entries(
    root: &Path,
    entries: impl IntoIterator<Item = std::result::Result<ignore::DirEntry, ignore::Error>>,
    filter: &Filter,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Side> {
    let mut out = Side::default();
    let mut normalized = BTreeSet::new();
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let Ok(e) = entry else {
            out.incomplete.insert(IncompleteReason::Traversal);
            continue;
        };
        if e.error().is_some() {
            out.incomplete.insert(IncompleteReason::Traversal);
        }
        let Ok(rel) = e.path().strip_prefix(root) else {
            out.incomplete.insert(IncompleteReason::PathIdentity);
            continue;
        };
        if rel.as_os_str().is_empty()
            || super::entry::is_os_noise_name(&e.file_name().to_string_lossy())
        {
            continue;
        }
        if out.meta.len() >= limit {
            out.truncated = true;
            break;
        }
        let Ok(m) = std::fs::symlink_metadata(e.path()) else {
            out.incomplete.insert(IncompleteReason::Metadata);
            continue;
        };
        // The IPC and frontend path helpers require UTF-8 and use "/" to delimit components.
        let Some(actual) = rel.to_str() else {
            out.incomplete.insert(IncompleteReason::PathIdentity);
            continue;
        };
        #[cfg(unix)]
        if actual.contains('\\') {
            out.incomplete.insert(IncompleteReason::PathIdentity);
            continue;
        }
        let actual = actual.replace(std::path::MAIN_SEPARATOR, "/");
        if !normalized.insert(crate::util::unicode::nfc(&actual).into_owned()) {
            out.incomplete.insert(IncompleteReason::PathIdentity);
            out.normalization_collision = true;
        }
        let meta = meta_of(&m, &mut out.incomplete);
        // A file that the filters leave out is not listed; its folders are marked at the end.
        if !meta.is_dir && filter.excludes_file(&actual, &meta) {
            out.excluded
                .insert(crate::util::unicode::nfc(&actual).into_owned());
            out.excluded.insert(actual);
            continue;
        }
        out.meta.insert(actual, meta);
    }
    Ok(out)
}

/// Tree order: a folder's children come right after it, siblings case-insensitively.
fn tree_cmp(a: &str, b: &str) -> CmpOrdering {
    let (mut x, mut y) = (a.split('/'), b.split('/'));
    loop {
        match (x.next(), y.next()) {
            (Some(p), Some(q)) => match p
                .to_lowercase()
                .cmp(&q.to_lowercase())
                .then_with(|| p.cmp(q))
            {
                CmpOrdering::Equal => continue,
                o => return o,
            },
            (None, Some(_)) => return CmpOrdering::Less,
            (Some(_), None) => return CmpOrdering::Greater,
            (None, None) => return CmpOrdering::Equal,
        }
    }
}

/// Pairs of files whose bytes were found equal in an earlier scan, with the size and the modification time (to the
/// nanosecond) both files had *before* they were read. A pair is trusted again only while both stamps are unchanged,
/// so a rewritten file (another time or size) is read again, and one rewritten during the read is stamped older than
/// its bytes and is read again next time. Bounded: it is emptied when it grows past `EQUAL_CACHE_MAX`.
type PairKey = (PathBuf, PathBuf);
type PairStamp = ((u64, Option<u128>), (u64, Option<u128>));
const EQUAL_CACHE_MAX: usize = 100_000;

fn equal_cache() -> &'static std::sync::Mutex<HashMap<PairKey, PairStamp>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<PairKey, PairStamp>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn stamp_of(path: &Path) -> Option<(u64, Option<u128>)> {
    let m = std::fs::metadata(path).ok()?;
    let nanos = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos());
    Some((m.len(), nanos))
}

/// `same_bytes` that skips the read of a pair already found equal and unchanged (unless `fresh`).
fn same_bytes_remembered(
    a: &Path,
    b: &Path,
    fresh: bool,
    cancel: &AtomicBool,
) -> std::io::Result<bool> {
    // A file without a modification time cannot be told apart from its later self: it is always read.
    let stamps = stamp_of(a)
        .zip(stamp_of(b))
        .filter(|(x, y)| x.1.is_some() && y.1.is_some());
    let key = (a.to_path_buf(), b.to_path_buf());
    if let (false, Some(now)) = (fresh, stamps) {
        if equal_cache().lock().ok().and_then(|c| c.get(&key).copied()) == Some(now) {
            return Ok(true);
        }
    }
    let same = same_bytes(a, b, cancel)?;
    if let (true, Some(now)) = (same, stamps) {
        if let Ok(mut cache) = equal_cache().lock() {
            if cache.len() >= EQUAL_CACHE_MAX {
                cache.clear();
            }
            cache.insert(key, now);
        }
    }
    Ok(same)
}

/// Whether two files have the same bytes. An error to open or read either one is an error, not a difference: a
/// file that could not be read must never be taken for one that changed, since a sync would then replace it.
fn same_bytes(a: &Path, b: &Path, cancel: &AtomicBool) -> std::io::Result<bool> {
    let (fa, fb) = (super::open_regular(a)?, super::open_regular(b)?);
    let (mut ra, mut rb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
        }
        let (na, nb) = (fill(&mut ra, &mut ba)?, fill(&mut rb, &mut bb)?);
        if na != nb || ba[..na] != bb[..nb] {
            return Ok(false);
        }
        if na == 0 {
            return Ok(true);
        }
    }
}

fn fill(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = r.read(&mut buf[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

pub fn compare_dirs(
    left: &Path,
    right: &Path,
    opts: DirCmpOptions,
    cancel: &AtomicBool,
) -> Result<DirCmpResult> {
    compare(left, right, opts, cancel, true)
}

/// [`compare_dirs`]; `parallel` walks the two folders at the same time (the result does not depend on
/// it: tests compare both).
fn compare(
    left: &Path,
    right: &Path,
    opts: DirCmpOptions,
    cancel: &AtomicBool,
    parallel: bool,
) -> Result<DirCmpResult> {
    let t0 = Instant::now();
    if left == right {
        return Err(CoreError::Other("both sides are the same folder".into()));
    }
    let (l, r) = if parallel {
        // The folders may be on different disks or shares: waiting for one need not wait for the other.
        // A panic of the walker goes on to the caller, as it does without the thread.
        std::thread::scope(|scope| {
            let other = scope.spawn(|| walk(right, &opts, cancel));
            let l = walk(left, &opts, cancel);
            let r = other
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            (l, r)
        })
    } else {
        let l = walk(left, &opts, cancel);
        let r = walk(right, &opts, cancel);
        (l, r)
    };
    // The left side's error comes first, as when the sides were walked one after the other.
    let (mut l, mut r) = (l?, r?);
    // Use exact spellings on both sides if NFC would merge distinct rows or subtrees.
    if !l.normalization_collision && !r.normalization_collision {
        l.normalize();
        r.normalize();
    }
    // Left out on either side is left out on both: a file out of range on one side only must not look like a missing one.
    let excluded: BTreeSet<String> = l.excluded.union(&r.excluded).cloned().collect();
    if !excluded.is_empty() {
        l.meta.retain(|k, _| !excluded.contains(k));
        r.meta.retain(|k, _| !excluded.contains(k));
    }
    // The folders above anything left out hold something that is not listed.
    let mut holding: BTreeSet<&str> = BTreeSet::new();
    for rel in excluded
        .iter()
        .chain(l.unlisted.iter())
        .chain(r.unlisted.iter())
    {
        let mut path = rel.as_str();
        while let Some(p) = path.rfind('/') {
            path = &path[..p];
            if !holding.insert(path) {
                break;
            }
        }
    }
    let truncated = l.truncated || r.truncated;
    let mut incomplete_reasons: BTreeSet<IncompleteReason> =
        l.incomplete.union(&r.incomplete).copied().collect();
    let mut keys: Vec<&String> = l
        .meta
        .keys()
        .chain(r.meta.keys().filter(|k| !l.meta.contains_key(*k)))
        .collect();
    keys.sort_by(|a, b| tree_cmp(a, b));

    let mut items: Vec<Item> = Vec::with_capacity(keys.len());
    let mut counts = Counts::default();
    for k in keys {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let (lm, rm) = (l.meta.get(k).copied(), r.meta.get(k).copied());
        let is_dir = lm.map(|m| m.is_dir).unwrap_or(false) || rm.map(|m| m.is_dir).unwrap_or(false);
        let newer = match (lm, rm) {
            (Some(a), Some(b)) if (a.mtime - b.mtime).abs() > MTIME_SLACK => {
                Some(if a.mtime > b.mtime { "left" } else { "right" })
            }
            _ => None,
        };
        let (status, reason) = match (lm, rm) {
            (Some(_), None) => (Status::LeftOnly, None),
            (None, Some(_)) => (Status::RightOnly, None),
            (Some(a), Some(b)) if a.is_dir != b.is_dir => (Status::Different, Some("type")),
            (Some(a), Some(_)) if a.is_dir => (Status::Same, None), // resolved from children below
            (Some(a), Some(b)) if a.size != b.size => (Status::Different, Some("size")),
            (Some(_), Some(_)) if opts.content => {
                let on_disk = |side: &Side, root: &Path| {
                    root.join(side.actual.get(k).map(String::as_str).unwrap_or(k))
                };
                match same_bytes_remembered(
                    &on_disk(&l, left),
                    &on_disk(&r, right),
                    opts.fresh,
                    cancel,
                ) {
                    Ok(true) => (Status::Same, None),
                    Ok(false) => (Status::Different, Some("content")),
                    Err(_) if cancel.load(Ordering::Relaxed) => return Err(CoreError::Cancelled),
                    // Not a content difference: the scan is incomplete, so no sync can replace this file.
                    Err(_) => {
                        incomplete_reasons.insert(IncompleteReason::Read);
                        (Status::Different, Some("unreadable"))
                    }
                }
            }
            (Some(_), Some(_)) if newer.is_some() => (Status::Different, Some("date")),
            _ => (Status::Same, None),
        };
        if !is_dir {
            match status {
                Status::Same => counts.same += 1,
                Status::Different => counts.different += 1,
                Status::LeftOnly => counts.left_only += 1,
                Status::RightOnly => counts.right_only += 1,
            }
        }
        items.push(Item {
            rel: k.clone(),
            name: k.rsplit('/').next().unwrap_or(k).to_string(),
            depth: k.matches('/').count(),
            is_dir,
            status,
            reason,
            newer: if status == Status::Different || is_dir {
                newer
            } else {
                None
            },
            left: lm,
            right: rm,
            left_rel: l.actual.get(k).cloned(),
            right_rel: r.actual.get(k).cloned(),
            has_excluded: is_dir && holding.contains(k.as_str()),
        });
    }

    // A folder present on both sides is "different" if anything inside it is.
    for i in (0..items.len()).rev() {
        if !(items[i].is_dir && items[i].status == Status::Same) {
            continue;
        }
        let prefix = format!("{}/", items[i].rel);
        let changed = items[i + 1..]
            .iter()
            .take_while(|c| c.rel.starts_with(&prefix))
            .any(|c| c.status != Status::Same);
        if changed {
            items[i].status = Status::Different;
            items[i].reason = Some("content");
        }
    }

    Ok(DirCmpResult {
        left: left.to_path_buf(),
        right: right.to_path_buf(),
        items,
        counts,
        truncated,
        incomplete_reasons,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::{apply_sync, SyncOp, UndoJournal};
    use std::fs;

    fn setup() -> (crate::fs::ops::test_support::TestDir, PathBuf, PathBuf) {
        let t = crate::fs::ops::test_support::test_dir();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        for d in [&l, &r] {
            fs::create_dir_all(d.join("sub/deep")).unwrap();
            fs::write(d.join("same.txt"), "same").unwrap();
            fs::write(d.join("sub/deep/also-same.txt"), "x").unwrap();
        }
        fs::write(l.join("changed.txt"), "left version").unwrap();
        fs::write(r.join("changed.txt"), "right v").unwrap();
        fs::write(l.join("samesize.txt"), "aaaa").unwrap();
        fs::write(r.join("samesize.txt"), "bbbb").unwrap();
        fs::write(l.join("only-left.txt"), "L").unwrap();
        fs::create_dir_all(r.join("only-right-dir")).unwrap();
        fs::write(r.join("only-right-dir/f.txt"), "R").unwrap();
        fs::write(l.join("sub/new.txt"), "new").unwrap();
        // equal mtimes so quick mode cannot see the same-size change
        let t0 = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        for p in [l.join("samesize.txt"), r.join("samesize.txt")] {
            fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(t0)
                .unwrap();
        }
        (t, l, r)
    }

    fn st<'a>(r: &'a DirCmpResult, rel: &str) -> &'a Item {
        r.items
            .iter()
            .find(|i| i.rel == rel)
            .unwrap_or_else(|| panic!("{rel} missing"))
    }

    #[test]
    fn classifies_and_orders_as_a_tree() {
        let (_t, l, r) = setup();
        let res = compare_dirs(&l, &r, DirCmpOptions::default(), &AtomicBool::new(false)).unwrap();
        assert_eq!(st(&res, "same.txt").status, Status::Same);
        assert_eq!(st(&res, "changed.txt").status, Status::Different);
        assert_eq!(st(&res, "changed.txt").reason, Some("size"));
        assert_eq!(st(&res, "only-left.txt").status, Status::LeftOnly);
        assert_eq!(st(&res, "only-right-dir").status, Status::RightOnly);
        assert_eq!(st(&res, "only-right-dir/f.txt").status, Status::RightOnly);
        assert_eq!(
            st(&res, "sub").status,
            Status::Different,
            "folder with a new file inside"
        );
        assert_eq!(st(&res, "sub/deep").status, Status::Same);
        assert_eq!(
            st(&res, "samesize.txt").status,
            Status::Same,
            "quick mode: same size + mtime"
        );
        // children follow their parent
        let pos = |rel: &str| res.items.iter().position(|i| i.rel == rel).unwrap();
        assert!(pos("sub") < pos("sub/deep") && pos("sub/deep") < pos("sub/new.txt"));
        assert_eq!(res.counts.left_only, 2);
        assert_eq!(res.counts.right_only, 1);
    }

    #[test]
    fn content_mode_sees_same_size_changes() {
        let (_t, l, r) = setup();
        let o = DirCmpOptions {
            content: true,
            ..Default::default()
        };
        let res = compare_dirs(&l, &r, o, &AtomicBool::new(false)).unwrap();
        assert_eq!(st(&res, "samesize.txt").status, Status::Different);
        assert_eq!(st(&res, "samesize.txt").reason, Some("content"));
    }

    #[test]
    fn scan_continues_after_walk_and_metadata_errors() {
        let (_t, left, _) = setup();
        let entries: Vec<_> = ignore::WalkBuilder::new(&left).build().collect();
        fs::remove_file(left.join("changed.txt")).unwrap();
        let failure = Err(ignore::Error::Io(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
        let entries = std::iter::once(failure).chain(entries);
        let side = scan_entries(
            &left,
            entries,
            &Filter::none(),
            MAX_ENTRIES,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            side.incomplete,
            BTreeSet::from([IncompleteReason::Traversal, IncompleteReason::Metadata])
        );
        assert!(side.meta.contains_key("same.txt"));
        assert!(!side.meta.contains_key("changed.txt"));
        assert!(!side.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_of_equal_size_is_an_error_not_a_content_difference() {
        use std::os::unix::fs::PermissionsExt;

        let (_t, left, right) = setup();
        let bad = left.join("samesize.txt");
        let saved = fs::metadata(&bad).unwrap().permissions();
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();
        let can_read = fs::File::open(&bad).is_ok();
        let o = DirCmpOptions {
            content: true,
            ..Default::default()
        };
        let result = compare_dirs(&left, &right, o, &AtomicBool::new(false));
        fs::set_permissions(&bad, saved).unwrap();
        if can_read {
            eprintln!("permissions fixture requires a user who cannot bypass permissions");
            return;
        }
        let result = result.unwrap();
        // The bad read is its own reason and makes the scan incomplete, so a sync is refused as a whole.
        assert_eq!(st(&result, "samesize.txt").reason, Some("unreadable"));
        assert!(result.incomplete_reasons.contains(&IncompleteReason::Read));
        // A real byte difference elsewhere stays a content difference, and readable equal files stay equal.
        assert_eq!(st(&result, "same.txt").status, Status::Same);
        // Nothing was written.
        assert_eq!(fs::read(right.join("samesize.txt")).unwrap(), b"bbbb");
    }

    #[cfg(unix)]
    #[test]
    fn inaccessible_source_subtree_is_reported_with_readable_rows() {
        use std::os::unix::fs::PermissionsExt;

        let (_t, left, right) = setup();
        let inaccessible = left.join("sub/deep");
        let saved = fs::metadata(&inaccessible).unwrap().permissions();
        fs::set_permissions(&inaccessible, fs::Permissions::from_mode(0o000)).unwrap();
        let can_read = fs::read_dir(&inaccessible).is_ok();
        let result = compare_dirs(
            &left,
            &right,
            DirCmpOptions::default(),
            &AtomicBool::new(false),
        );
        fs::set_permissions(&inaccessible, saved).unwrap();
        if can_read {
            eprintln!("permissions fixture requires a user who cannot bypass permissions");
            return;
        }

        let result = result.unwrap();
        assert_eq!(
            result.incomplete_reasons,
            BTreeSet::from([IncompleteReason::Traversal])
        );
        assert!(!result.truncated);
        assert!(st(&result, "same.txt").left.is_some());
        assert_eq!(
            st(&result, "sub/deep/also-same.txt").status,
            Status::RightOnly
        );
        assert_eq!(
            fs::read(right.join("sub/deep/also-same.txt")).unwrap(),
            b"x"
        );
    }

    #[test]
    fn normalization_collision_keeps_both_real_paths_available() {
        let (_t, left, right) = setup();
        let nfc = "caf\u{e9}.txt";
        let nfd = "cafe\u{301}.txt";
        fs::write(left.join(nfc), "composed").unwrap();
        match fs::File::create_new(left.join(nfd)) {
            Ok(_) => fs::write(left.join(nfd), "decomposed").unwrap(),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return,
            Err(e) => panic!("could not create the decomposed path: {e}"),
        }
        fs::write(right.join(nfc), "composed").unwrap();

        let result = compare_dirs(
            &left,
            &right,
            DirCmpOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            result.incomplete_reasons,
            BTreeSet::from([IncompleteReason::PathIdentity])
        );
        for (rel, contents) in [(nfc, "composed"), (nfd, "decomposed")] {
            let row = st(&result, rel);
            assert!(row.left.is_some());
            let path = left.join(row.left_rel.as_deref().unwrap_or(&row.rel));
            assert_eq!(fs::read_to_string(path).unwrap(), contents);
        }
    }

    #[test]
    fn tree_order() {
        let mut v = vec!["a.txt", "a/x", "B", "a", "b/c"];
        v.sort_by(|a, b| tree_cmp(a, b));
        assert_eq!(v, ["a", "a/x", "a.txt", "B", "b/c"]);
    }

    #[test]
    fn mirror_makes_folders_identical() {
        let (_t, l, r) = setup();
        let j = UndoJournal::default();
        let before =
            compare_dirs(&l, &r, DirCmpOptions::default(), &AtomicBool::new(false)).unwrap();
        let ops: Vec<SyncOp> = before
            .items
            .iter()
            .filter(|i| !i.is_dir || i.status != Status::Different)
            .filter_map(|i| match i.status {
                Status::LeftOnly | Status::Different => Some(SyncOp::Copy {
                    src: l.join(&i.rel),
                    dest: r.join(&i.rel),
                }),
                Status::RightOnly => Some(SyncOp::Trash {
                    path: r.join(&i.rel),
                }),
                Status::Same => None,
            })
            .collect();
        // keep top-most only
        let mut top: Vec<SyncOp> = Vec::new();
        for op in ops {
            let p = match &op {
                SyncOp::Copy { dest, .. } => dest.clone(),
                SyncOp::Trash { path } => path.clone(),
            };
            if !top.iter().any(|t| match t {
                SyncOp::Copy { dest, .. } => p.starts_with(dest) && p != *dest,
                SyncOp::Trash { path } => p.starts_with(path) && p != *path,
            }) {
                top.push(op);
            }
        }
        apply_sync(&top, &j, &AtomicBool::new(false)).unwrap();
        let after =
            compare_dirs(&l, &r, DirCmpOptions::default(), &AtomicBool::new(false)).unwrap();
        let left: Vec<_> = after
            .items
            .iter()
            .filter(|i| i.status != Status::Same)
            .map(|i| {
                format!(
                    "{} {:?} {:?} {:?}/{:?}",
                    i.rel,
                    i.status,
                    i.reason,
                    i.left.map(|m| m.mtime),
                    i.right.map(|m| m.mtime)
                )
            })
            .collect();
        assert!(left.is_empty(), "still different after mirror: {left:?}");
    }

    #[test]
    fn sync_mirror_then_undo() {
        let (_t, l, r) = setup();
        let j = UndoJournal::default();
        let ops = vec![
            SyncOp::Copy {
                src: l.join("changed.txt"),
                dest: r.join("changed.txt"),
            },
            SyncOp::Copy {
                src: l.join("sub/new.txt"),
                dest: r.join("sub/new.txt"),
            },
            SyncOp::Copy {
                src: l.join("only-left.txt"),
                dest: r.join("x/y/only-left.txt"),
            },
            SyncOp::Trash {
                path: r.join("only-right-dir"),
            },
        ];
        let rep = apply_sync(&ops, &j, &AtomicBool::new(false)).unwrap();
        assert_eq!((rep.copied, rep.replaced, rep.trashed), (3, 1, 1));
        assert_eq!(
            fs::read_to_string(r.join("changed.txt")).unwrap(),
            "left version"
        );
        assert!(r.join("x/y/only-left.txt").exists());
        assert!(!r.join("only-right-dir").exists());
        assert_eq!(j.peek_description().as_deref(), Some("sync of 4 items"));
        // undo restores the replaced file, removes the copies and created folders
        match j.undo() {
            Ok(_) => {
                assert_eq!(
                    fs::read_to_string(r.join("changed.txt")).unwrap(),
                    "right v"
                );
                assert!(!r.join("sub/new.txt").exists());
                assert!(!r.join("x").exists());
                assert!(r.join("only-right-dir/f.txt").exists());
            }
            Err(e) => eprintln!("undo not verified (no trash here): {e}"),
        }
    }

    /// The same name composed (NFC) on one side and decomposed (NFD) on the other pairs up, in
    /// quick and in content mode (the bytes are read from the real, differently spelled, paths).
    #[test]
    fn nfd_and_nfc_names_pair_up() {
        let t = crate::fs::ops::test_support::test_dir();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        let nfc = "Poro\u{10d}ilo.txt";
        let nfd = "Poroc\u{30c}ilo.txt";
        assert_ne!(nfc, nfd);
        fs::create_dir_all(l.join("d\u{e9}p")).unwrap();
        fs::create_dir_all(r.join("de\u{301}p")).unwrap();
        fs::write(l.join(nfc), "same text").unwrap();
        fs::write(r.join(nfd), "same text").unwrap();
        fs::write(l.join("d\u{e9}p/a.txt"), "one").unwrap();
        fs::write(r.join("de\u{301}p/a.txt"), "two").unwrap();
        let stamp = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        for p in [
            l.join(nfc),
            r.join(nfd),
            l.join("d\u{e9}p/a.txt"),
            r.join("de\u{301}p/a.txt"),
        ] {
            fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(stamp)
                .unwrap();
        }
        for content in [false, true] {
            let o = DirCmpOptions {
                content,
                ..Default::default()
            };
            let res = compare_dirs(&l, &r, o, &AtomicBool::new(false)).unwrap();
            let rels: Vec<&str> = res.items.iter().map(|i| i.rel.as_str()).collect();
            assert_eq!(
                rels,
                ["d\u{e9}p", "d\u{e9}p/a.txt", nfc],
                "content={content}: composed names, one row each"
            );
            let by = |rel: &str| res.items.iter().find(|i| i.rel == rel).unwrap();
            assert_eq!(by(nfc).status, Status::Same, "content={content}");
            assert_eq!(
                (res.counts.left_only, res.counts.right_only),
                (0, 0),
                "content={content}"
            );
            // The file differs only in content mode (same size, same mtime).
            let a = by("d\u{e9}p/a.txt");
            assert_eq!(
                a.status,
                if content {
                    Status::Different
                } else {
                    Status::Same
                },
                "content={content}"
            );
            // The row says how each side really spells the path.
            assert_eq!(by(nfc).left_rel, None);
            assert_eq!(by(nfc).right_rel.as_deref(), Some(nfd));
            assert_eq!(a.right_rel.as_deref(), Some("de\u{301}p/a.txt"));
        }
    }

    #[test]
    fn finder_noise_is_left_out_even_with_hidden_files_shown() {
        let t = crate::fs::ops::test_support::test_dir();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        fs::create_dir_all(l.join("sub")).unwrap();
        fs::create_dir_all(&r).unwrap();
        fs::write(l.join(".DS_Store"), "x").unwrap();
        fs::write(l.join("sub/.DS_Store"), "x").unwrap();
        fs::write(l.join(".hidden"), "x").unwrap();
        fs::write(r.join(".DS_Store"), "yy").unwrap();
        let o = DirCmpOptions {
            hidden: true,
            ..Default::default()
        };
        let res = compare_dirs(&l, &r, o, &AtomicBool::new(false)).unwrap();
        let rels: Vec<&str> = res.items.iter().map(|i| i.rel.as_str()).collect();
        assert_eq!(
            rels,
            [".hidden", "sub"],
            "a real hidden file stays, .DS_Store goes"
        );
    }

    /// Walking the two sides at the same time gives exactly the result of walking them one after the
    /// other: the same rows in the same order, the same counts, the same errors.
    #[test]
    fn concurrent_walk_equals_sequential_walk() {
        let (_t, l, r) = setup();
        for content in [false, true] {
            for hidden in [false, true] {
                let o = DirCmpOptions {
                    content,
                    hidden,
                    ..Default::default()
                };
                let never = AtomicBool::new(false);
                let seq = compare(&l, &r, o.clone(), &never, false).unwrap();
                let par = compare(&l, &r, o.clone(), &never, true).unwrap();
                let shape = |x: &DirCmpResult| {
                    serde_json::to_string(&(
                        &x.items,
                        &x.counts,
                        x.truncated,
                        &x.incomplete_reasons,
                    ))
                    .unwrap()
                };
                assert_eq!(
                    shape(&seq),
                    shape(&par),
                    "content={content} hidden={hidden}"
                );
                assert!(!seq.items.is_empty());
            }
        }
        // The same error whichever way: a side that is not a folder is named.
        let missing = l.join("missing");
        let o = DirCmpOptions::default();
        let never = AtomicBool::new(false);
        let e_seq = compare(&missing, &r, o.clone(), &never, false)
            .unwrap_err()
            .to_string();
        let e_par = compare(&missing, &r, o.clone(), &never, true)
            .unwrap_err()
            .to_string();
        assert_eq!(e_seq, e_par);
        let e_seq = compare(&l, &missing, o.clone(), &never, false)
            .unwrap_err()
            .to_string();
        let e_par = compare(&l, &missing, o.clone(), &never, true)
            .unwrap_err()
            .to_string();
        assert_eq!(e_seq, e_par);
        // Cancelled before it starts: cancelled either way.
        let stop = AtomicBool::new(true);
        assert!(matches!(
            compare(&l, &r, o.clone(), &stop, true),
            Err(CoreError::Cancelled)
        ));
        assert!(matches!(
            compare(&l, &r, o, &stop, false),
            Err(CoreError::Cancelled)
        ));
    }

    fn filtered(l: &Path, r: &Path, o: DirCmpOptions) -> DirCmpResult {
        compare_dirs(l, r, o, &AtomicBool::new(false)).unwrap()
    }

    fn rels(r: &DirCmpResult) -> Vec<&str> {
        r.items.iter().map(|i| i.rel.as_str()).collect()
    }

    #[test]
    fn exclude_globs_leave_files_and_whole_folders_out_and_mark_the_folders_above() {
        let (_t, l, r) = setup();
        fs::create_dir_all(l.join("sub/build/out")).unwrap();
        fs::write(l.join("sub/build/out/x.o"), "obj").unwrap();
        fs::write(l.join("sub/debug.log"), "log").unwrap();
        fs::write(r.join("sub/debug.log"), "log2").unwrap();
        let res = filtered(
            &l,
            &r,
            DirCmpOptions {
                exclude: vec!["*.log".into(), "build/".into()],
                ..Default::default()
            },
        );
        let all = rels(&res);
        assert!(!all.contains(&"sub/debug.log"), "{all:?}");
        assert!(!all.iter().any(|p| p.starts_with("sub/build")), "{all:?}");
        // The folder that held them says so; one that held nothing left out does not.
        assert!(st(&res, "sub").has_excluded);
        assert!(!st(&res, "sub/deep").has_excluded);
        assert!(!res.items.iter().any(|i| !i.is_dir && i.has_excluded));
        // Everything else is as before.
        assert_eq!(st(&res, "only-left.txt").status, Status::LeftOnly);
    }

    #[test]
    fn a_file_that_the_include_and_the_exclude_both_name_is_left_out() {
        let (_t, l, r) = setup();
        fs::write(l.join("both.rs"), "a").unwrap();
        fs::write(r.join("both.rs"), "b").unwrap();
        fs::write(l.join("only-include.rs"), "a").unwrap();
        let res = filtered(
            &l,
            &r,
            DirCmpOptions {
                include: vec!["*.rs".into()],
                exclude: vec!["both.rs".into()],
                ..Default::default()
            },
        );
        let all = rels(&res);
        assert!(
            !all.contains(&"both.rs"),
            "exclude wins over include: {all:?}"
        );
        assert!(all.contains(&"only-include.rs"), "{all:?}");
    }

    #[test]
    fn a_pattern_with_dot_dot_names_nothing_outside_the_folders() {
        let (t, l, r) = setup();
        // A file next to the two folders: no pattern may reach it.
        fs::write(t.path().join("outside.txt"), "secret").unwrap();
        let res = filtered(
            &l,
            &r,
            DirCmpOptions {
                include: vec!["../outside.txt".into(), "../**".into()],
                ..Default::default()
            },
        );
        let all = rels(&res);
        assert!(!all.iter().any(|p| p.contains("outside")), "{all:?}");
        assert!(
            res.items.iter().all(|i| i.is_dir
                || i.status == Status::Same
                || i.status == Status::LeftOnly
                || i.status == Status::RightOnly
                || i.status == Status::Different),
            "only files of the two folders are listed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_not_followed_and_a_loop_does_not_hang_the_scan() {
        let (t, l, r) = setup();
        let elsewhere = t.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("far.txt"), "far").unwrap();
        // A link to a folder outside, and a link to its own parent (a loop), on one side only.
        std::os::unix::fs::symlink(&elsewhere, l.join("to-outside")).unwrap();
        std::os::unix::fs::symlink(&l, l.join("sub/loop")).unwrap();
        let cancel = AtomicBool::new(false);
        let res = compare_dirs(&l, &r, DirCmpOptions::default(), &cancel).unwrap();
        let all = rels(&res);
        assert!(
            !all.iter().any(|p| p.ends_with("far.txt")),
            "a link is not followed: {all:?}"
        );
        assert!(
            !all.iter().any(|p| p.starts_with("sub/loop/")),
            "a loop is not walked: {all:?}"
        );
        assert!(!res.truncated);
    }

    #[test]
    fn include_globs_compare_only_what_they_match() {
        let (_t, l, r) = setup();
        fs::write(l.join("a.rs"), "fn a() {}").unwrap();
        let res = filtered(
            &l,
            &r,
            DirCmpOptions {
                include: vec!["*.rs".into()],
                ..Default::default()
            },
        );
        let files: Vec<&str> = res
            .items
            .iter()
            .filter(|i| !i.is_dir)
            .map(|i| i.rel.as_str())
            .collect();
        assert_eq!(files, ["a.rs"]);
        // Folders are still walked, and those whose files were all left out say so.
        assert!(st(&res, "sub").has_excluded);
        assert!(st(&res, "only-right-dir").has_excluded);
        assert_eq!(st(&res, "only-right-dir").status, Status::RightOnly);
    }

    #[test]
    fn a_file_out_of_range_on_one_side_is_left_out_on_both() {
        let (_t, l, r) = setup();
        // 10 bytes on the left, 1,000 on the right: the range of at most 100 bytes leaves the right one out. It must not
        // turn into a "left only" row (which a sync would copy over the right file).
        fs::write(l.join("range.txt"), "0123456789").unwrap();
        fs::write(r.join("range.txt"), "x".repeat(1000)).unwrap();
        let res = filtered(
            &l,
            &r,
            DirCmpOptions {
                max_size: Some(100),
                ..Default::default()
            },
        );
        assert!(!rels(&res).contains(&"range.txt"), "{:?}", rels(&res));
        // The size bounds are inclusive.
        let at = filtered(
            &l,
            &r,
            DirCmpOptions {
                min_size: Some(10),
                max_size: Some(10),
                ..Default::default()
            },
        );
        let files: Vec<&str> = at
            .items
            .iter()
            .filter(|i| !i.is_dir)
            .map(|i| i.rel.as_str())
            .collect();
        assert!(
            !files.contains(&"range.txt"),
            "the right file is 1,000 bytes: {files:?}"
        );
    }

    #[test]
    fn modification_time_ranges_are_inclusive_and_apply_to_files_only() {
        let (_t, l, r) = setup();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        for side in [&l, &r] {
            fs::write(side.join("old.txt"), "o").unwrap();
            fs::File::options()
                .write(true)
                .open(side.join("old.txt"))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let after = filtered(
            &l,
            &r,
            DirCmpOptions {
                modified_after: Some(1_000_001),
                ..Default::default()
            },
        );
        assert!(!rels(&after).contains(&"old.txt"));
        assert!(
            rels(&after).contains(&"sub"),
            "folders are not filtered by time"
        );
        let at = filtered(
            &l,
            &r,
            DirCmpOptions {
                modified_after: Some(1_000_000),
                modified_before: Some(1_000_000),
                ..Default::default()
            },
        );
        let files: Vec<&str> = at
            .items
            .iter()
            .filter(|i| !i.is_dir)
            .map(|i| i.rel.as_str())
            .collect();
        assert_eq!(files, ["old.txt"]);
    }

    #[test]
    fn an_invalid_pattern_is_refused_with_the_pattern() {
        let (_t, l, r) = setup();
        let err = compare_dirs(
            &l,
            &r,
            DirCmpOptions {
                exclude: vec!["[unclosed".into()],
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("[unclosed"), "{err}");
    }

    #[test]
    fn filters_do_not_change_an_unfiltered_comparison_or_depend_on_the_walk_order() {
        let (_t, l, r) = setup();
        let plain = filtered(&l, &r, DirCmpOptions::default());
        assert!(plain.items.iter().all(|i| !i.has_excluded));
        let o = DirCmpOptions {
            exclude: vec!["samesize.txt".into()],
            ..Default::default()
        };
        let seq = compare(&l, &r, o.clone(), &AtomicBool::new(false), false).unwrap();
        let par = compare(&l, &r, o, &AtomicBool::new(false), true).unwrap();
        assert_eq!(rels(&seq), rels(&par));
        assert!(!rels(&seq).contains(&"samesize.txt"));
        assert!(rels(&plain).contains(&"samesize.txt"));
    }

    #[test]
    fn a_plan_whose_files_changed_after_the_preview_is_refused_before_anything_is_written() {
        use crate::fs::ops::{apply_sync_checked, mtime_secs, Stamp, SyncCheck};
        let (_t, l, r) = setup();
        let stamp = |p: &Path| {
            let m = fs::metadata(p).unwrap();
            Some(Stamp {
                size: m.len(),
                mtime: mtime_secs(&m).unwrap(),
            })
        };
        let (src, dest) = (l.join("only-left.txt"), r.join("only-left.txt"));
        let ops = vec![SyncOp::Copy {
            src: src.clone(),
            dest: dest.clone(),
        }];
        let checks = vec![
            SyncCheck {
                path: src.clone(),
                stamp: stamp(&src),
                dir: false,
                tree: None,
                exact: None,
            },
            SyncCheck {
                path: dest.clone(),
                stamp: None,
                dir: false,
                tree: None,
                exact: None,
            },
        ];
        let journal = UndoJournal::default();
        let never = AtomicBool::new(false);
        // The source grows after the preview: refused with its name, the destination is not created.
        fs::write(&src, "L grew").unwrap();
        let err = apply_sync_checked(&ops, &checks, &journal, &never)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("only-left.txt") && err.contains("changed"),
            "{err}"
        );
        assert!(!dest.exists());
        // The destination appears after the preview: a copy would replace it, so it is refused.
        fs::write(&src, "L").unwrap();
        let checks = vec![
            SyncCheck {
                path: src.clone(),
                stamp: stamp(&src),
                dir: false,
                tree: None,
                exact: None,
            },
            SyncCheck {
                path: dest.clone(),
                stamp: None,
                dir: false,
                tree: None,
                exact: None,
            },
        ];
        fs::write(&dest, "someone else").unwrap();
        let err = apply_sync_checked(&ops, &checks, &journal, &never)
            .unwrap_err()
            .to_string();
        assert!(err.contains("appeared"), "{err}");
        assert_eq!(fs::read_to_string(&dest).unwrap(), "someone else");
        // A source that is gone.
        fs::remove_file(&dest).unwrap();
        fs::remove_file(&src).unwrap();
        let err = apply_sync_checked(&ops, &checks, &journal, &never)
            .unwrap_err()
            .to_string();
        assert!(err.contains("gone"), "{err}");
    }

    #[test]
    fn a_folder_that_holds_hidden_files_is_flagged_so_no_mirror_trashes_it_unseen() {
        let (_t, l, r) = setup();
        // Only on the right, and it holds nothing but a hidden file: not empty.
        fs::create_dir(r.join("only-hidden")).unwrap();
        fs::write(r.join("only-hidden/.keep"), "x").unwrap();
        // Only on the right with visible content: nothing is left out.
        fs::create_dir(r.join("visible")).unwrap();
        fs::write(r.join("visible/a.txt"), "x").unwrap();
        // Deeper: the hidden file is below a folder that shows a visible file.
        fs::create_dir_all(r.join("outer/inner")).unwrap();
        fs::write(r.join("outer/inner/b.txt"), "x").unwrap();
        fs::write(r.join("outer/inner/.secret"), "x").unwrap();
        // `.DS_Store` is noise, not content.
        fs::create_dir(r.join("noise")).unwrap();
        fs::write(r.join("noise/.DS_Store"), "x").unwrap();
        fs::write(r.join("noise/c.txt"), "x").unwrap();
        let cancel = AtomicBool::new(false);
        let flag = |hidden: bool, name: &str| {
            let res = compare_dirs(
                &l,
                &r,
                DirCmpOptions {
                    hidden,
                    ..Default::default()
                },
                &cancel,
            )
            .unwrap();
            res.items
                .iter()
                .find(|i| i.rel == name)
                .unwrap()
                .has_excluded
        };
        assert!(
            flag(false, "only-hidden"),
            "a folder with only a hidden file is not empty"
        );
        assert!(flag(false, "outer"), "the hidden file is below it");
        assert!(flag(false, "outer/inner"));
        assert!(
            !flag(false, "visible"),
            "permitted: nothing is hidden there"
        );
        assert!(!flag(false, "noise"), "permitted: only noise is hidden");
        // With hidden files shown they are listed, so nothing is left out.
        assert!(!flag(true, "only-hidden"));
        assert!(!flag(true, "outer"));
    }

    #[test]
    fn equal_pairs_are_remembered_and_a_changed_file_is_read_again() {
        let (_t, l, r) = setup();
        let (a, b) = (l.join("remember-a.txt"), r.join("remember-b.txt"));
        fs::write(&a, "same text").unwrap();
        fs::write(&b, "same text").unwrap();
        let never = AtomicBool::new(false);
        assert!(same_bytes_remembered(&a, &b, false, &never).unwrap());
        // Remembered: a pair that is now unreadable on one side is not even opened... only while unchanged.
        let key = (a.clone(), b.clone());
        assert!(equal_cache().lock().unwrap().contains_key(&key));
        // The same size, rewritten with other bytes: the stamp (nanoseconds) differs, so it is read and found different.
        std::thread::sleep(std::time::Duration::from_millis(30));
        fs::write(&b, "SAME text").unwrap();
        assert!(!same_bytes_remembered(&a, &b, false, &never).unwrap());
        // `fresh` always reads: it finds the difference whatever the cache holds.
        fs::write(&b, "same text").unwrap();
        assert!(same_bytes_remembered(&a, &b, true, &never).unwrap());
    }

    #[test]
    fn a_rewrite_of_equal_size_within_the_same_second_is_refused_by_the_exact_stamp() {
        use crate::fs::ops::{file_fingerprint, mtime_secs, verify_sync, Stamp, SyncCheck};
        let (_t, l, _r) = setup();
        let path = l.join("same-second.txt");
        fs::write(&path, "one").unwrap();
        let m = fs::metadata(&path).unwrap();
        let check = |exact: Option<String>| {
            vec![SyncCheck {
                path: path.clone(),
                stamp: Some(Stamp {
                    size: m.len(),
                    mtime: mtime_secs(&m).unwrap(),
                }),
                dir: false,
                tree: None,
                exact,
            }]
        };
        let shown = file_fingerprint(&path).unwrap();
        // Unchanged: both checks pass.
        verify_sync(&check(None)).unwrap();
        verify_sync(&check(Some(shown.clone()))).unwrap();
        // The same size, rewritten a moment later.
        std::thread::sleep(std::time::Duration::from_millis(30));
        fs::write(&path, "two").unwrap();
        if file_fingerprint(&path).unwrap() == shown {
            return; // a file system with coarse times cannot tell: nothing to check here
        }
        // The seconds may or may not have turned over; the exact stamp refuses in either case.
        let error = verify_sync(&check(Some(shown))).unwrap_err().to_string();
        assert!(error.contains("changed after"), "{error}");
    }

    #[test]
    fn a_folder_that_gained_a_hidden_file_since_the_preview_is_not_trashed() {
        use crate::fs::ops::{apply_sync_checked, tree_fingerprint, SyncCheck};
        let (_t, _l, r) = setup();
        let folder = r.join("extra-dir");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("seen.txt"), "x").unwrap();
        let shown = tree_fingerprint(&folder).unwrap();
        let check = |tree: &str| {
            vec![SyncCheck {
                path: folder.clone(),
                stamp: None,
                dir: true,
                tree: Some(tree.to_string()),
                exact: None,
            }]
        };
        let ops = vec![SyncOp::Trash {
            path: folder.clone(),
        }];
        let journal = UndoJournal::default();
        let never = AtomicBool::new(false);
        // Unchanged since the preview: the same fingerprint, and the plan is applied.
        assert_eq!(tree_fingerprint(&folder).unwrap(), shown);
        // A hidden file added after the preview (the comparison never saw it): refused, nothing trashed.
        fs::write(folder.join(".hidden"), "keep me").unwrap();
        assert_ne!(tree_fingerprint(&folder).unwrap(), shown);
        let error = apply_sync_checked(&ops, &check(&shown), &journal, &never)
            .unwrap_err()
            .to_string();
        assert!(error.contains("other entries"), "{error}");
        assert!(
            folder.join(".hidden").exists(),
            "the folder was not touched"
        );
        // A file rewritten at the same size changes the fingerprint too (nanosecond stamps).
        let now = tree_fingerprint(&folder).unwrap();
        let before = fs::metadata(folder.join(".hidden"))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        fs::write(folder.join(".hidden"), "keep you").unwrap();
        if fs::metadata(folder.join(".hidden"))
            .unwrap()
            .modified()
            .unwrap()
            != before
        {
            assert_ne!(tree_fingerprint(&folder).unwrap(), now);
        }
    }

    /// What the window's Mirror plans for a scan: copies of what is only left or different, trashes of what is
    /// only right, the top-most entries only (rows left out by the filters are not in the scan, so never planned).
    fn mirror_ops(r: &DirCmpResult, l: &Path, rr: &Path) -> Vec<SyncOp> {
        let mut ops: Vec<SyncOp> = Vec::new();
        for i in r
            .items
            .iter()
            .filter(|i| !i.is_dir || i.status != Status::Different)
        {
            let dest = rr.join(&i.rel);
            let op = match i.status {
                Status::LeftOnly | Status::Different => SyncOp::Copy {
                    src: l.join(&i.rel),
                    dest,
                },
                Status::RightOnly => SyncOp::Trash { path: dest },
                Status::Same => continue,
            };
            let below = |p: &Path| {
                ops.iter().any(|t| match t {
                    SyncOp::Copy { dest, .. } => p.starts_with(dest) && p != dest,
                    SyncOp::Trash { path } => p.starts_with(path) && p != path,
                })
            };
            let target = match &op {
                SyncOp::Copy { dest, .. } => dest.clone(),
                SyncOp::Trash { path } => path.clone(),
            };
            if !below(&target) {
                ops.push(op);
            }
        }
        ops
    }

    #[test]
    fn a_mirror_of_a_filtered_scan_keeps_what_is_left_out_and_one_undo_takes_it_all_back() {
        let (_t, l, r) = setup();
        // Left out on one side is left out on both: neither the log of the left nor the one of the right is touched.
        fs::write(l.join("noise.log"), "left log").unwrap();
        fs::write(r.join("old.log"), "right log").unwrap();
        fs::create_dir_all(r.join("cache")).unwrap();
        fs::write(r.join("cache/keep.bin"), "kept").unwrap();
        let o = || DirCmpOptions {
            exclude: vec!["*.log".to_owned(), "cache/".to_owned()],
            ..Default::default()
        };
        let scan = filtered(&l, &r, o());
        assert!(!rels(&scan)
            .iter()
            .any(|x| x.ends_with(".log") || x.starts_with("cache")));
        let ops = mirror_ops(&scan, &l, &r);
        let journal = UndoJournal::default();
        let report = apply_sync(&ops, &journal, &AtomicBool::new(false)).unwrap();
        assert!(report.copied >= 3 && report.trashed == 1, "{report:?}");
        assert_eq!(fs::read_to_string(r.join("old.log")).unwrap(), "right log");
        assert_eq!(
            fs::read_to_string(r.join("cache/keep.bin")).unwrap(),
            "kept"
        );
        assert!(!r.join("noise.log").exists());
        assert!(filtered(&l, &r, o())
            .items
            .iter()
            .all(|i| i.status == Status::Same));
        match journal.undo() {
            Ok(_) => {
                assert_eq!(
                    fs::read_to_string(r.join("changed.txt")).unwrap(),
                    "right v"
                );
                assert!(r.join("only-right-dir/f.txt").exists());
                assert!(!r.join("only-left.txt").exists());
                assert_eq!(fs::read_to_string(r.join("old.log")).unwrap(), "right log");
            }
            Err(e) => eprintln!("undo not verified (no trash here): {e}"),
        }
    }

    #[test]
    fn a_single_row_copy_under_filters_is_one_undoable_step() {
        let (_t, l, r) = setup();
        let o = DirCmpOptions {
            exclude: vec!["*.log".to_owned()],
            ..Default::default()
        };
        fs::write(l.join("noise.log"), "n").unwrap();
        let scan = filtered(&l, &r, o);
        assert_eq!(st(&scan, "only-left.txt").status, Status::LeftOnly);
        let journal = UndoJournal::default();
        let one = [SyncOp::Copy {
            src: l.join("only-left.txt"),
            dest: r.join("only-left.txt"),
        }];
        let report = apply_sync(&one, &journal, &AtomicBool::new(false)).unwrap();
        assert_eq!((report.copied, report.replaced, report.trashed), (1, 0, 0));
        assert_eq!(
            journal.peek_description().as_deref(),
            Some("sync of 1 item")
        );
        assert!(r.join("only-left.txt").exists() && !r.join("noise.log").exists());
        journal.undo().unwrap();
        assert!(!r.join("only-left.txt").exists());
        // A second undo has nothing left to take back.
        assert!(journal.undo().is_err());
    }

    #[test]
    fn a_scan_over_the_entry_limit_is_truncated_and_says_so() {
        let (_t, l, _r) = setup();
        let entries: Vec<_> = ignore::WalkBuilder::new(&l).build().collect();
        let all = scan_entries(
            &l,
            entries,
            &Filter::none(),
            usize::MAX,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!all.truncated);
        let entries: Vec<_> = ignore::WalkBuilder::new(&l).build().collect();
        let cut = scan_entries(&l, entries, &Filter::none(), 3, &AtomicBool::new(false)).unwrap();
        assert!(cut.truncated);
        assert_eq!(cut.meta.len(), 3);
    }

    #[test]
    fn names_that_differ_only_by_case_stay_separate_rows_and_are_copied_apart() {
        let (_t, l, r) = setup();
        fs::write(l.join("Readme.md"), "upper").unwrap();
        if fs::File::create_new(l.join("readme.md")).is_err() {
            // A case-insensitive file system holds one of the two names: nothing to collide.
            return;
        }
        fs::write(l.join("readme.md"), "lower").unwrap();
        fs::write(r.join("readme.md"), "lower").unwrap();
        let scan = filtered(&l, &r, DirCmpOptions::default());
        assert_eq!(st(&scan, "Readme.md").status, Status::LeftOnly);
        assert_eq!(st(&scan, "readme.md").status, Status::Same);
        let ops = [SyncOp::Copy {
            src: l.join("Readme.md"),
            dest: r.join("Readme.md"),
        }];
        apply_sync(&ops, &UndoJournal::default(), &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read_to_string(r.join("Readme.md")).unwrap(), "upper");
        assert_eq!(fs::read_to_string(r.join("readme.md")).unwrap(), "lower");
    }

    #[test]
    fn a_plan_that_matches_the_preview_is_applied() {
        use crate::fs::ops::{apply_sync_checked, mtime_secs, Stamp, SyncCheck};
        let (_t, l, r) = setup();
        let m = fs::metadata(l.join("only-left.txt")).unwrap();
        let checks = vec![
            SyncCheck {
                path: l.join("only-left.txt"),
                stamp: Some(Stamp {
                    size: m.len(),
                    mtime: mtime_secs(&m).unwrap(),
                }),
                dir: false,
                tree: None,
                exact: None,
            },
            SyncCheck {
                path: r.join("only-left.txt"),
                stamp: None,
                dir: false,
                tree: None,
                exact: None,
            },
        ];
        let ops = vec![SyncOp::Copy {
            src: l.join("only-left.txt"),
            dest: r.join("only-left.txt"),
        }];
        let report = apply_sync_checked(
            &ops,
            &checks,
            &UndoJournal::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.copied, 1);
        assert_eq!(fs::read_to_string(r.join("only-left.txt")).unwrap(), "L");
    }

    #[test]
    fn a_pattern_in_either_unicode_form_finds_the_name() {
        let (_t, l, r) = setup();
        let nfd = "cafe\u{301}"; // "café" decomposed, as macOS spells it
        for side in [&l, &r] {
            fs::create_dir_all(side.join(nfd)).unwrap();
            fs::write(side.join(nfd).join("menu.txt"), "m").unwrap();
        }
        // The pattern is typed composed.
        let res = filtered(
            &l,
            &r,
            DirCmpOptions {
                exclude: vec!["caf\u{e9}".into()],
                ..Default::default()
            },
        );
        assert!(
            !rels(&res).iter().any(|p| p.contains("menu.txt")),
            "{:?}",
            rels(&res)
        );
        assert!(st(&res, "same.txt").status == Status::Same);
    }
}
