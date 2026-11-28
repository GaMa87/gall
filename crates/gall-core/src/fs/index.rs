//! In-memory file-name index for instant "find anything" (Quick Open).
//!
//! The index is built in the background with a parallel directory walk
//! (the `ignore` crate from ripgrep) and stored as one compact string arena.
//! Queries scan the whole index in parallel with the fuzzy scorer, so a
//! million paths answer in tens of milliseconds. While a rebuild runs,
//! queries keep using the previous snapshot.

use super::fuzzy::{Query, Scratch};
use super::search_filter::{
    walk_hidden_matches, walk_hidden_matches_multi, CountBudget, FilterConfig, FilterMatcher,
    HiddenCount, ProbeMatch,
};
use serde::Serialize;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub include_hidden: bool,
    pub respect_gitignore: bool,
    /// Glob patterns (gitignore syntax) that are never indexed.
    pub exclude: Vec<String>,
    /// Index everything: no `.gallignore`, no `.gitignore`, no `exclude`, no cache
    /// folders skipped; entries that normal filters would skip are flagged `hidden`.
    pub everything: bool,
    /// Optional subtrees inside `roots` to walk first (e.g. current folder, home)
    /// so `everything` scans index user folders before hitting `max_entries`.
    pub priority_roots: Vec<PathBuf>,
    /// Hard cap on indexed entries, to bound memory.
    pub max_entries: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions {
            include_hidden: false,
            respect_gitignore: true,
            exclude: default_excludes(),
            everything: false,
            priority_roots: vec![],
            max_entries: 3_000_000,
        }
    }
}

/// Default roots for the Quick Open index: the user's home folder on Unix (avoiding
/// background sweeps of `/etc`, `/opt`, `/usr`, `/var` — typing `/path` in Quick Open
/// still browses anywhere live), and every local drive on Windows.
pub fn system_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        // Local disks and USB drives; not network drives (walking a NAS every few
        // minutes) or optical drives (slow, and they spin up).
        #[link(name = "kernel32")]
        extern "system" {
            fn GetDriveTypeW(root: *const u16) -> u32;
        }
        const DRIVE_REMOVABLE: u32 = 2;
        const DRIVE_FIXED: u32 = 3;
        (b'A'..=b'Z')
            .map(|c| PathBuf::from(format!("{}:\\", c as char)))
            .filter(|p| {
                let w: Vec<u16> = p
                    .as_os_str()
                    .to_string_lossy()
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                // SAFETY: a valid NUL-terminated root path.
                let t = unsafe { GetDriveTypeW(w.as_ptr()) };
                matches!(t, DRIVE_REMOVABLE | DRIVE_FIXED) && p.is_dir()
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        dirs::home_dir()
            .map(|h| vec![h])
            .unwrap_or_else(|| vec![PathBuf::from("/")])
    }
}

/// File system types that live on another computer or in the cloud.
const REMOTE_FS: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "ncpfs",
    "afs",
    "ceph",
    "glusterfs",
    "lustre",
    "9p",
    "davfs",
    "fuse.sshfs",
    "fuse.rclone",
    "fuse.s3fs",
    "fuse.gcsfuse",
    "fuse.goofys",
    "fuse.gvfsd-fuse",
    "fuse.kio-fuse",
    "fuse.curlftpfs",
    "fuse.onedriver",
    "fuse.google-drive-ocamlfuse",
    "fuse.davfs2",
    "fuse.smbnetfs",
];

/// Mount points of network and cloud file systems (NFS, SMB, sshfs, rclone, …). A
/// whole-system index or search does not walk into them: it would pull the whole share
/// over the network every few minutes, and a share that stopped answering would stall
/// the walk. Searching such a folder itself still works (it is then the root).
pub fn remote_mounts() -> std::collections::HashSet<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/mounts")
            .map(|t| parse_remote_mounts(&t))
            .unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        macos_remote_mounts()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Default::default()
    }
}

/// macOS: mounted SMB, NFS, AFP and WebDAV shares (under `/Volumes`), from `getmntinfo`.
#[cfg(target_os = "macos")]
fn macos_remote_mounts() -> std::collections::HashSet<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let mut buf: *mut libc::statfs = std::ptr::null_mut();
    // SAFETY: getmntinfo fills `buf` with a library-owned array of `n` entries that stays
    // valid until the next call; the entries are only read here.
    let n = unsafe { libc::getmntinfo(&mut buf, libc::MNT_NOWAIT) };
    let mut out = std::collections::HashSet::new();
    if n <= 0 || buf.is_null() {
        return out;
    }
    for fs in unsafe { std::slice::from_raw_parts(buf, n as usize) } {
        let kind = unsafe { CStr::from_ptr(fs.f_fstypename.as_ptr()) }.to_string_lossy();
        if matches!(kind.as_ref(), "smbfs" | "nfs" | "afpfs" | "webdav" | "cifs") {
            let at = unsafe { CStr::from_ptr(fs.f_mntonname.as_ptr()) };
            out.insert(PathBuf::from(std::ffi::OsStr::from_bytes(at.to_bytes())));
        }
    }
    out
}

/// The remote mount points in a `/proc/self/mounts` text (`\040` = space, …).
pub fn parse_remote_mounts(mounts: &str) -> std::collections::HashSet<PathBuf> {
    let unescape = |s: &str| -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            let octal = b
                .get(i + 1..i + 4)
                .filter(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)));
            if let (b'\\', Some(d)) = (b[i], octal) {
                out.push(
                    d.iter()
                        .fold(0u8, |v, c| v.wrapping_mul(8).wrapping_add(c - b'0')),
                );
                i += 4;
                continue;
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split(' ');
            let (_dev, point, kind) = (f.next()?, f.next()?, f.next()?);
            REMOTE_FS
                .contains(&kind)
                .then(|| PathBuf::from(unescape(point)))
        })
        .collect()
}

/// Virtual or device filesystems that must never be walked: reading them is
/// slow, endless or can block (e.g. `/proc/kmsg`).
pub fn is_system_skip(path: &Path) -> bool {
    #[cfg(unix)]
    {
        const SKIP: &[&str] = &[
            "/proc",
            "/sys",
            "/dev",
            "/run",
            "/var/run",
            "/var/lock",
            "/lost+found",
            "/snap",
            "/var/lib/docker",
            "/var/lib/containers",
            // macOS: firmlinked system volumes (the data is reached through `/`), swap, and
            // the metadata folders of Spotlight and the file system events log.
            "/System/Volumes",
            "/private/var/vm",
            "/.Spotlight-V100",
            "/.fseventsd",
            "/.vol",
        ];
        SKIP.iter().any(|s| path == Path::new(s))
    }
    #[cfg(windows)]
    {
        let n = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        matches!(
            n.as_str(),
            "$recycle.bin" | "system volume information" | "$windows.~bt" | "$windows.~ws"
        ) || path.ends_with("Windows\\WinSxS")
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        false
    }
}

pub fn default_excludes() -> Vec<String> {
    [
        "node_modules",
        ".git",
        "target",
        "__pycache__",
        ".cache",
        ".venv",
        "dist",
        "build",
        "coverage",
        ".next",
        ".nuxt",
        ".svelte-kit",
        ".turbo",
        ".parcel-cache",
        ".gradle",
        ".tox",
        ".mypy_cache",
        ".pytest_cache",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    start: u32,
    len: u32,
    /// Byte offset of the file name inside the path.
    name: u16,
    is_dir: bool,
    /// Normally hidden by search filters (set only in `everything` scans).
    hidden: bool,
}

#[derive(Default, Clone)]
struct Snapshot {
    arena: String,
    entries: Vec<Entry>,
    roots: Vec<PathBuf>,
    built_at: Option<SystemTime>,
    build_ms: u64,
    restricted_dirs: usize,
}

impl Snapshot {
    fn path(&self, e: &Entry) -> &str {
        &self.arena[e.start as usize..(e.start + e.len) as usize]
    }
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub entries: usize,
    pub roots: Vec<String>,
    pub building: bool,
    /// Unix epoch milliseconds of the last completed build.
    pub built_at: Option<i64>,
    pub build_ms: u64,
    /// macOS: TCC-protected folders (such as `~/Library` subfolders) skipped due to
    /// missing Full Disk Access during the last index build.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub restricted_dirs: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexHit {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub score: i32,
    /// Highlighted positions as UTF-16 offsets into `path` (for JavaScript).
    pub positions: Vec<u32>,
    /// True when normal search filters would have hidden this item (shown under `Alt+I`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexResult {
    pub hits: Vec<IndexHit>,
    /// Number of matching entries before truncation to `limit`.
    pub total: usize,
    pub searched: usize,
    pub elapsed_ms: u64,
    /// The folder is not in the system index and is being scanned for this query
    /// (a WSL distribution, a network share, a folder outside the indexed ones).
    pub scanning: bool,
}

pub struct FileIndex {
    snap: RwLock<Arc<Snapshot>>,
    building: AtomicBool,
    /// A build was requested and has not started yet (see [`FileIndex::request_rebuild`]).
    requested: AtomicBool,
    pending_roots: Mutex<Vec<PathBuf>>,
    /// Paths under these prefixes rank higher (e.g. the home folder).
    preferred: RwLock<Vec<String>>,
}

const CACHE_MAGIC: &str = "gall-index-cache 1";

/// Score bonus for hits under a preferred prefix.
const BONUS_PREFERRED: i32 = 25;

impl Default for FileIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl FileIndex {
    pub fn new() -> Self {
        FileIndex {
            snap: RwLock::new(Arc::new(Snapshot::default())),
            building: AtomicBool::new(false),
            requested: AtomicBool::new(false),
            pending_roots: Mutex::new(vec![]),
            preferred: RwLock::new(vec![]),
        }
    }

    /// Sets folders whose contents rank higher in results.
    pub fn set_preferred(&self, dirs: &[PathBuf]) {
        *self.preferred.write().unwrap() = dirs
            .iter()
            .map(|d| {
                let mut s = d.to_string_lossy().into_owned();
                if !s.ends_with(std::path::MAIN_SEPARATOR) {
                    s.push(std::path::MAIN_SEPARATOR);
                }
                s
            })
            .collect();
    }

    pub fn status(&self) -> IndexStatus {
        let s = self.snap.read().unwrap().clone();
        IndexStatus {
            entries: s.entries.len(),
            roots: s
                .roots
                .iter()
                .map(|r| r.to_string_lossy().into_owned())
                .collect(),
            // A requested build counts as running from the request on: a caller that polls
            // `building` right after asking for a refresh must not see the gap before the
            // build thread starts, or the one between a cancelled build and its rerun.
            building: self.building.load(Ordering::Acquire)
                || self.requested.load(Ordering::Acquire),
            built_at: s.built_at.and_then(|t| super::entry::to_millis(Ok(t))),
            build_ms: s.build_ms,
            restricted_dirs: s.restricted_dirs,
        }
    }

    /// A build is running right now.
    pub fn is_building(&self) -> bool {
        self.building.load(Ordering::Acquire)
    }

    /// Records that a build must start (or restart) with what exists now. Call it before
    /// looking at [`FileIndex::is_building`] and before spawning the build thread:
    /// `status().building` is true from here until a build that began afterwards ends.
    pub fn request_rebuild(&self) {
        self.requested.store(true, Ordering::SeqCst);
    }

    /// Withdraws a request whose build thread could not be started.
    pub fn withdraw_request(&self) {
        self.requested.store(false, Ordering::SeqCst);
    }

    /// A request arrived that no build has taken over yet: a running build began before it.
    pub fn rebuild_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Age of the current snapshot, or `None` if never built.
    pub fn age(&self) -> Option<Duration> {
        self.snap
            .read()
            .unwrap()
            .built_at
            .and_then(|t| t.elapsed().ok())
    }

    /// True when `path` lies inside one of the indexed roots.
    pub fn covers(&self, path: &Path) -> bool {
        let s = self.snap.read().unwrap();
        s.roots.iter().any(|r| path.starts_with(r))
            || self
                .pending_roots
                .lock()
                .unwrap()
                .iter()
                .any(|r| path.starts_with(r))
    }

    /// Rebuilds the index for `roots` on the current thread. Returns `false`
    /// if another build is already running. Roots nested in other roots are
    /// dropped so nothing is indexed twice.
    pub fn rebuild(&self, roots: &[PathBuf], opts: &IndexOptions, cancel: &AtomicBool) -> bool {
        if self.building.swap(true, Ordering::AcqRel) {
            return false;
        }
        // The build takes over every request made so far: it walks the disk after this point.
        // `building` is already true, so `status()` never shows a gap.
        self.requested.store(false, Ordering::SeqCst);
        // A root the user chose on a network share (NFS, SMB, sshfs) is indexed like any other root; only
        // GVFS paths (FUSE views that stat every entry over the network) are never walked.
        let filtered_roots: Vec<PathBuf> = roots
            .iter()
            .filter(|r| !super::gvfs::is_gvfs_path(r))
            .cloned()
            .collect();
        let roots = normalize_roots(&filtered_roots);
        *self.pending_roots.lock().unwrap() = roots.clone();
        let t0 = Instant::now();
        let mut snap = build(&roots, opts, cancel);
        snap.build_ms = t0.elapsed().as_millis() as u64;
        if !cancel.load(Ordering::Relaxed) {
            *self.snap.write().unwrap() = Arc::new(snap);
        }
        self.pending_roots.lock().unwrap().clear();
        self.building.store(false, Ordering::Release);
        log::info!(
            "file index: {} entries in {} ms",
            self.status().entries,
            t0.elapsed().as_millis()
        );
        true
    }

    /// Drops `path` and everything below it from the current snapshot at once, so
    /// Quick Open stops showing what was just hidden from search before the next
    /// rebuild finishes. Returns how many entries were dropped.
    pub fn forget_under(&self, path: &Path) -> usize {
        let p = path.to_string_lossy();
        let p = p.trim_end_matches(['/', '\\']);
        if p.is_empty() {
            return 0;
        }
        let under = |s: &str| {
            s.strip_prefix(p)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '\\']))
        };
        let mut w = self.snap.write().unwrap();
        let old = w.clone();
        let removed = old.entries.iter().filter(|e| under(old.path(e))).count();
        if removed == 0 {
            return 0;
        }
        let mut snap = Snapshot {
            arena: String::with_capacity(old.arena.len()),
            entries: Vec::with_capacity(old.entries.len() - removed),
            roots: old.roots.clone(),
            built_at: old.built_at,
            build_ms: old.build_ms,
            restricted_dirs: old.restricted_dirs,
        };
        for e in &old.entries {
            let s = old.path(e);
            if under(s) {
                continue;
            }
            let start = snap.arena.len() as u32;
            snap.arena.push_str(s);
            snap.entries.push(Entry { start, ..*e });
        }
        *w = Arc::new(snap);
        removed
    }

    /// Osmosis: adds paths seen while browsing or after a file operation to the snapshot, so active
    /// folders stay fresh without a rebuild. Paths already indexed, outside the indexed roots, or
    /// seen before the first build finished are ignored. Returns how many were added.
    pub fn upsert_paths(&self, items: &[(PathBuf, bool)]) -> usize {
        if items.is_empty() {
            return 0;
        }
        let mut w = self.snap.write().unwrap();
        if w.built_at.is_none() {
            return 0;
        }
        let wanted: std::collections::HashSet<&str> = items
            .iter()
            .filter(|(p, _)| w.roots.iter().any(|r| p.starts_with(r)))
            .filter_map(|(p, _)| p.to_str())
            .collect();
        let existing: std::collections::HashSet<String> = w
            .entries
            .iter()
            .map(|e| w.path(e))
            .filter(|p| wanted.contains(p))
            .map(str::to_string)
            .collect();
        let fresh: Vec<(String, bool)> = items
            .iter()
            .filter_map(|(p, d)| p.to_str().map(|s| (s.to_string(), *d)))
            .filter(|(s, _)| wanted.contains(s.as_str()) && !existing.contains(s))
            .collect();
        if fresh.is_empty() {
            return 0;
        }
        let n = fresh.len();
        let snap = Arc::make_mut(&mut w);
        push_items(
            snap,
            fresh.into_iter().map(|(p, d)| (p, d, false)).collect(),
            usize::MAX,
        );
        n
    }

    /// Osmosis for one listed folder: its visible, non-hidden children join the index.
    /// `children` are `(name, is_dir)`; names starting with a dot are left to the next rebuild,
    /// which applies the ignore rules.
    pub fn upsert_dir(&self, dir: &Path, children: &[(String, bool)]) -> usize {
        let items: Vec<(PathBuf, bool)> = children
            .iter()
            .filter(|(n, _)| !n.starts_with('.'))
            .map(|(n, d)| (dir.join(n), *d))
            .collect();
        self.upsert_paths(&items)
    }

    /// Renames `from` and everything below it to `to` in place (a rename or move done in the app).
    /// Returns how many entries moved.
    pub fn rename_prefix(&self, from: &Path, to: &Path) -> usize {
        let (Some(f), Some(t)) = (from.to_str(), to.to_str()) else {
            return 0;
        };
        let f = f.trim_end_matches(['/', '\\']);
        if f.is_empty() {
            return 0;
        }
        let mut w = self.snap.write().unwrap();
        let moved = w
            .entries
            .iter()
            .filter(|e| {
                w.path(e)
                    .strip_prefix(f)
                    .is_some_and(|r| r.is_empty() || r.starts_with(['/', '\\']))
            })
            .count();
        if moved == 0 {
            return 0;
        }
        let old = w.clone();
        let mut snap = Snapshot {
            arena: String::with_capacity(old.arena.len()),
            entries: Vec::with_capacity(old.entries.len()),
            roots: old.roots.clone(),
            built_at: old.built_at,
            build_ms: old.build_ms,
            restricted_dirs: old.restricted_dirs,
        };
        for e in &old.entries {
            let s = old.path(e);
            let moved_path = s
                .strip_prefix(f)
                .filter(|r| r.is_empty() || r.starts_with(['/', '\\']))
                .map(|r| format!("{t}{r}"));
            let path = moved_path.as_deref().unwrap_or(s);
            let start = snap.arena.len();
            snap.arena.push_str(path);
            let name = path.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
            snap.entries.push(Entry {
                start: start as u32,
                len: path.len() as u32,
                name: name.min(u16::MAX as usize) as u16,
                ..*e
            });
        }
        *w = Arc::new(snap);
        moved
    }

    /// Writes the snapshot as one compact file (`path` created with its folders). Call it once after a
    /// completed build, never periodically.
    pub fn save_cache(&self, path: &Path) -> std::io::Result<()> {
        let s = self.snap.read().unwrap().clone();
        let Some(built) = s.built_at else {
            return Ok(());
        };
        let ms = built
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let mut out = String::with_capacity(s.arena.len() + s.entries.len() * 3 + 256);
        out.push_str(CACHE_MAGIC);
        out.push('\n');
        out.push_str(&format!("{ms}\t{}\n", s.build_ms));
        for r in &s.roots {
            out.push_str(&format!("R{}\n", r.to_string_lossy()));
        }
        for e in &s.entries {
            let flags = (e.is_dir as u8) | ((e.hidden as u8) << 1);
            out.push((b'0' + flags) as char);
            out.push_str(s.path(e));
            out.push('\n');
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, out)?;
        std::fs::rename(&tmp, path)
    }

    /// Loads a snapshot written by [`FileIndex::save_cache`] when nothing was built yet. The snapshot
    /// keeps its original build time, so an on-demand refresh still replaces it. Returns the number of
    /// entries loaded; a missing or unreadable cache loads nothing.
    pub fn load_cache(&self, path: &Path) -> usize {
        let Ok(text) = std::fs::read_to_string(path) else {
            return 0;
        };
        let mut lines = text.lines();
        if lines.next() != Some(CACHE_MAGIC) {
            return 0;
        }
        let Some((ms, build_ms)) = lines.next().and_then(|l| l.split_once('\t')) else {
            return 0;
        };
        let (Ok(ms), Ok(build_ms)) = (ms.parse::<u64>(), build_ms.parse::<u64>()) else {
            return 0;
        };
        let mut snap = Snapshot {
            built_at: Some(SystemTime::UNIX_EPOCH + Duration::from_millis(ms)),
            build_ms,
            ..Default::default()
        };
        let mut items = Vec::new();
        for l in lines {
            if let Some(r) = l.strip_prefix('R') {
                snap.roots.push(PathBuf::from(r));
            } else if let Some(flags) = l.bytes().next().filter(|b| (b'0'..=b'3').contains(b)) {
                let f = flags - b'0';
                items.push((l[1..].to_string(), f & 1 != 0, f & 2 != 0));
            }
        }
        push_items(&mut snap, items, usize::MAX);
        let n = snap.entries.len();
        let mut w = self.snap.write().unwrap();
        if w.built_at.is_some() {
            return 0;
        }
        *w = Arc::new(snap);
        n
    }

    /// Current roots, plus `extra` if it is not already covered.
    pub fn roots_with(&self, extra: &Path) -> Vec<PathBuf> {
        let mut r = self.snap.read().unwrap().roots.clone();
        if !r.iter().any(|x| extra.starts_with(x)) {
            r.push(extra.to_path_buf());
        }
        r
    }

    /// Runs `query`, optionally limited to paths under `scope`.
    pub fn query(&self, query: &str, scope: Option<&Path>, limit: usize) -> IndexResult {
        let started = Instant::now();
        let snap = self.snap.read().unwrap().clone();
        let limit = limit.max(1);
        let mut candidates = limit;
        let mut checked = std::collections::HashMap::new();
        loop {
            let mut result = self.query_cached(&snap, query, scope, candidates);
            let exhausted = result.hits.len() < candidates || result.hits.len() == result.total;
            result.hits.retain(|hit| {
                *checked.entry(hit.path.clone()).or_insert_with(|| {
                    // Only confirmed absence is grounds for hiding a result.
                    // Permission and transient network errors must not erase it.
                    !matches!(std::fs::symlink_metadata(&hit.path), Err(e)
                        if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory))
                })
            });
            if result.hits.len() >= limit || exhausted {
                result.hits.truncate(limit);
                result.total = result
                    .total
                    .saturating_sub(checked.values().filter(|v| !**v).count());
                result.elapsed_ms = started.elapsed().as_millis() as u64;
                return result;
            }
            // Validate ranked candidates, not every indexed path on every keystroke.
            candidates = candidates.saturating_mul(2);
        }
    }

    fn query_cached(
        &self,
        snap: &Snapshot,
        query: &str,
        scope: Option<&Path>,
        limit: usize,
    ) -> IndexResult {
        let t0 = Instant::now();
        let q = Query::parse(query);
        let scope = scope.map(|p| {
            let mut s = p.to_string_lossy().into_owned();
            if !s.ends_with('/') && !s.ends_with('\\') {
                s.push(std::path::MAIN_SEPARATOR);
            }
            s
        });
        if q.is_empty() {
            return IndexResult {
                hits: vec![],
                total: 0,
                searched: snap.entries.len(),
                elapsed_ms: 0,
                scanning: false,
            };
        }

        let scope_chars = scope.as_ref().map_or(0, |s| s.chars().count());
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(16);
        let chunk = snap.entries.len().div_ceil(threads).max(1);
        let limit = limit.max(1);
        let preferred = self.preferred.read().unwrap().clone();
        let (mut heap, total) = std::thread::scope(|sc| {
            let handles: Vec<_> = snap
                .entries
                .chunks(chunk)
                .enumerate()
                .map(|(ci, part)| {
                    let (q, snap, scope, preferred) = (&q, &snap, scope.as_deref(), &preferred);
                    sc.spawn(move || {
                        let mut scratch = Scratch::default();
                        let mut heap: BinaryHeap<Ranked> = BinaryHeap::new();
                        let mut total = 0usize;
                        for (k, e) in part.iter().enumerate() {
                            if q.dirs_only() && !e.is_dir {
                                continue;
                            }
                            let p = snap.path(e);
                            let (target, name_start, pos_shift) = if let Some(s) = scope {
                                let Some(rel) = p.strip_prefix(s) else {
                                    continue;
                                };
                                (rel, (e.name as usize).saturating_sub(s.len()), scope_chars)
                            } else {
                                (p, e.name as usize, 0)
                            };
                            let matched = if e.hidden {
                                q.score_name_focused(target, name_start, &mut scratch)
                            } else {
                                q.score(target, name_start, &mut scratch)
                            };
                            if let Some(mut m) = matched {
                                if pos_shift > 0 {
                                    for pos in &mut m.positions {
                                        *pos += pos_shift;
                                    }
                                }
                                total += 1;
                                let boost = if preferred.iter().any(|d| p.starts_with(d.as_str())) {
                                    BONUS_PREFERRED
                                } else {
                                    0
                                };
                                let r = Ranked {
                                    score: m.score + boost,
                                    len: e.len,
                                    idx: ci * chunk + k,
                                    positions: m.positions,
                                };
                                push_top(&mut heap, r, limit);
                            }
                        }
                        (heap, total)
                    })
                })
                .collect();
            let mut all = BinaryHeap::new();
            let mut total = 0;
            for h in handles {
                let (heap, t) = h.join().unwrap_or_default();
                total += t;
                for r in heap {
                    push_top(&mut all, r, limit);
                }
            }
            (all, total)
        });

        let mut ranked: Vec<Ranked> = std::mem::take(&mut heap).into_vec();
        ranked.sort_by(|a, b| b.score.cmp(&a.score).then(a.len.cmp(&b.len)));
        let hits = ranked
            .into_iter()
            .map(|r| {
                let e = &snap.entries[r.idx];
                let path = snap.path(e);
                IndexHit {
                    path: path.to_string(),
                    name: path[e.name as usize..].to_string(),
                    is_dir: e.is_dir,
                    score: r.score,
                    positions: utf16_positions(path, &r.positions),
                    hidden: e.hidden,
                }
            })
            .collect();
        IndexResult {
            hits,
            total,
            searched: snap.entries.len(),
            elapsed_ms: t0.elapsed().as_millis() as u64,
            scanning: false,
        }
    }
}

/// True when `query` is a direct filesystem path prefix (`/…`, `~/…`, `~`, or `C:\…`)
/// that Quick Open completes live from a single `read_dir` without touching the index.
pub fn is_path_prefix_query(query: &str) -> bool {
    let raw = query.trim_start();
    if raw.is_empty() {
        return false;
    }
    if raw.starts_with('/') || raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        return true;
    }
    let b = raw.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')
}

/// Live single-directory completion for absolute / home-relative path queries (`/etc/ng`,
/// `~/Proj`, `C:\Win`). Reads only the target parent directory (zero recursive walking).
pub fn query_path_prefix(query: &str, include_hidden: bool, limit: usize) -> Option<IndexResult> {
    if !is_path_prefix_query(query) {
        return None;
    }
    let t0 = Instant::now();
    let raw = query.trim_start();
    let expanded = if raw == "~" {
        let mut h = dirs::home_dir()?.to_string_lossy().into_owned();
        if !h.ends_with('/') && !h.ends_with('\\') {
            h.push(std::path::MAIN_SEPARATOR);
        }
        h
    } else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        let mut h = dirs::home_dir()?.to_string_lossy().into_owned();
        if !h.ends_with('/') && !h.ends_with('\\') {
            h.push(std::path::MAIN_SEPARATOR);
        }
        h.push_str(rest);
        h
    } else {
        raw.to_string()
    };

    let split_at = if cfg!(windows) {
        expanded.rfind(['/', '\\'])?
    } else {
        expanded.rfind('/')?
    };
    let dir_str = if split_at == 0 {
        "/"
    } else if cfg!(windows) && split_at == 2 && expanded.as_bytes().get(1) == Some(&b':') {
        &expanded[..=2]
    } else {
        &expanded[..split_at]
    };
    let needle = &expanded[split_at + 1..];
    let dir = Path::new(dir_str);

    let Ok(rd) = std::fs::read_dir(dir) else {
        return Some(IndexResult {
            hits: Vec::new(),
            total: 0,
            searched: 0,
            elapsed_ms: t0.elapsed().as_millis() as u64,
            scanning: false,
        });
    };

    let show_dots = include_hidden || needle.starts_with('.');
    let parsed = (!needle.is_empty()).then(|| Query::parse(needle));
    let mut scratch = Scratch::default();
    let mut searched = 0usize;
    let mut scored: Vec<(i32, bool, String, String, Vec<u32>)> = Vec::new();

    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && !show_dots {
            continue;
        }
        searched += 1;
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .ok()
            .map(|ft| {
                if ft.is_symlink() {
                    path.is_dir()
                } else {
                    ft.is_dir()
                }
            })
            .unwrap_or(false);
        let path_str = path.to_string_lossy().into_owned();
        if let Some(q) = parsed.as_ref() {
            if q.dirs_only() && !is_dir {
                continue;
            }
            let Some(mut m) = q.score(&name, 0, &mut scratch) else {
                continue;
            };
            let prefix_chars = path_str
                .chars()
                .count()
                .saturating_sub(name.chars().count());
            for pos in &mut m.positions {
                *pos += prefix_chars;
            }
            let positions = utf16_positions(&path_str, &m.positions);
            scored.push((m.score, is_dir, name, path_str, positions));
        } else {
            scored.push((0, is_dir, name, path_str, Vec::new()));
        }
    }

    let total = scored.len();
    if parsed.is_some() {
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.len().cmp(&b.2.len()))
                .then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase()))
        });
    } else {
        scored.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase()))
                .then_with(|| a.2.cmp(&b.2))
        });
    }
    scored.truncate(limit.max(1));

    let hits = scored
        .into_iter()
        .map(|(score, is_dir, name, path, positions)| IndexHit {
            path,
            name,
            is_dir,
            score,
            positions,
            hidden: false,
        })
        .collect();

    Some(IndexResult {
        hits,
        total,
        searched,
        elapsed_ms: t0.elapsed().as_millis() as u64,
        scanning: false,
    })
}

/// Lowers the calling thread's I/O scheduling class to idle on Linux so background
/// indexing stays polite to foreground work and security monitors.
fn set_idle_io_priority() {
    #[cfg(target_os = "linux")]
    {
        const IOPRIO_WHO_PROCESS: libc::c_int = 1;
        const IOPRIO_CLASS_IDLE: libc::c_int = 3 << 13;
        // SAFETY: `ioprio_set(IOPRIO_WHO_PROCESS, 0, ...)` targets only the calling thread;
        // errors (e.g. restricted sandbox) are benign and ignored.
        unsafe {
            libc::syscall(
                libc::SYS_ioprio_set,
                IOPRIO_WHO_PROCESS,
                0,
                IOPRIO_CLASS_IDLE,
            );
        }
    }
}

/// Whether the system index (built from `roots`) holds what is in `path`. Not for a
/// folder outside every root (a WSL distribution `\\wsl.localhost\…` on Windows, whose
/// default roots are the drives; a folder outside custom indexed folders) or on a
/// network mount below a root, which the index never walks into: Quick Open scans those
/// itself. A WSL folder listed as an indexed folder is covered like any other root.
pub fn index_covers(roots: &[PathBuf], path: &Path) -> bool {
    if super::gvfs::is_slow_or_gvfs_mount(path) {
        return false;
    }
    let remote = remote_mounts();
    roots.iter().any(|r| {
        path.starts_with(r)
            && !remote
                .iter()
                .any(|m| m != r && m.starts_with(r) && path.starts_with(m))
    })
}

/// Max-heap ordering is inverted so the heap root is the *worst* kept hit.
struct Ranked {
    score: i32,
    len: u32,
    idx: usize,
    positions: Vec<usize>,
}

impl PartialEq for Ranked {
    fn eq(&self, o: &Self) -> bool {
        self.score == o.score && self.len == o.len
    }
}
impl Eq for Ranked {}
impl PartialOrd for Ranked {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Ranked {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        // "Greater" = worse: lower score, then longer path.
        o.score.cmp(&self.score).then(self.len.cmp(&o.len))
    }
}

fn push_top(heap: &mut BinaryHeap<Ranked>, r: Ranked, limit: usize) {
    if heap.len() < limit {
        heap.push(r);
    } else if let Some(worst) = heap.peek() {
        if r < *worst {
            heap.pop();
            heap.push(r);
        }
    }
}

fn utf16_positions(path: &str, char_pos: &[usize]) -> Vec<u32> {
    let mut out = Vec::with_capacity(char_pos.len());
    let mut it = char_pos.iter().peekable();
    let mut u16 = 0u32;
    for (ci, c) in path.chars().enumerate() {
        let Some(&&want) = it.peek() else { break };
        if ci == want {
            out.push(u16);
            it.next();
        }
        u16 += c.len_utf16() as u32;
    }
    out
}

fn normalize_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut r: Vec<PathBuf> = roots
        .iter()
        .filter(|p| !super::gvfs::is_gvfs_path(p) && p.is_dir())
        .cloned()
        .collect();
    r.sort();
    r.dedup();
    let all = r.clone();
    r.retain(|p| !all.iter().any(|o| o != p && p.starts_with(o)));
    r
}

/// Counts entries hidden by `filter` under `root` that match Quick Open's fuzzy
/// `query`, within `budget`. Returns zero when `query` is empty or cancelled.
pub fn count_hidden(
    root: &Path,
    query: &str,
    filter: &FilterConfig,
    budget: CountBudget,
    cancel: &AtomicBool,
) -> HiddenCount {
    if super::gvfs::is_gvfs_path(root) {
        return HiddenCount::default();
    }
    let q = Query::parse(query);
    if q.is_empty() {
        return HiddenCount::default();
    }
    let mut prefix = root.to_string_lossy().into_owned();
    if !prefix.ends_with('/') && !prefix.ends_with('\\') {
        prefix.push(std::path::MAIN_SEPARATOR);
    }
    let mut scratch = Scratch::default();
    walk_hidden_matches(root, filter, None, budget, cancel, |path, _name, is_dir| {
        if q.dirs_only() && !is_dir {
            return ProbeMatch::NoMatch;
        }
        let s = path.to_string_lossy();
        let rel = s.strip_prefix(prefix.as_str()).unwrap_or(&s);
        let name_start = rel.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
        if q.score_name_focused(rel, name_start, &mut scratch)
            .is_some()
        {
            ProbeMatch::Matched
        } else {
            ProbeMatch::NoMatch
        }
    })
}

/// Counts entries hidden by `filter` across multiple `roots` (for Quick Open
/// "everywhere"), probing `priority` folders first without double-counting.
pub fn count_hidden_roots(
    roots: &[PathBuf],
    priority: &[PathBuf],
    query: &str,
    filter: &FilterConfig,
    budget: CountBudget,
    cancel: &AtomicBool,
) -> HiddenCount {
    let filtered_roots: Vec<PathBuf> = roots
        .iter()
        .filter(|r| !super::gvfs::is_gvfs_path(r))
        .cloned()
        .collect();
    if filtered_roots.is_empty() {
        return HiddenCount::default();
    }
    let filtered_priority: Vec<PathBuf> = priority
        .iter()
        .filter(|r| !super::gvfs::is_gvfs_path(r))
        .cloned()
        .collect();
    let q = Query::parse(query);
    if q.is_empty() {
        return HiddenCount::default();
    }
    let mut scratch = Scratch::default();
    walk_hidden_matches_multi(
        &filtered_roots,
        &filtered_priority,
        filter,
        budget,
        cancel,
        |path, _name, is_dir| {
            if q.dirs_only() && !is_dir {
                return ProbeMatch::NoMatch;
            }
            let s = path.to_string_lossy();
            let name_start = s.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
            if q.score_name_focused(&s, name_start, &mut scratch).is_some() {
                ProbeMatch::Matched
            } else {
                ProbeMatch::NoMatch
            }
        },
    )
}

fn build(roots: &[PathBuf], opts: &IndexOptions, cancel: &AtomicBool) -> Snapshot {
    let mut snap = Snapshot {
        roots: roots.to_vec(),
        ..Default::default()
    };
    // Build ordered (walk_dir, filter_root) pairs: priority folders inside `roots`
    // first so `everything` scans over `/` index user folders before `max_entries`.
    let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
    for p in &opts.priority_roots {
        if super::gvfs::is_gvfs_path(p) || !p.is_dir() {
            continue;
        }
        let Some(containing) = roots.iter().find(|r| p.starts_with(r)) else {
            continue;
        };
        if plan.iter().any(|(d, _)| p.starts_with(d)) {
            continue;
        }
        plan.push((p.clone(), containing.clone()));
    }
    for r in roots {
        if super::gvfs::is_gvfs_path(r) || !r.is_dir() || plan.iter().any(|(d, _)| r == d) {
            continue;
        }
        plan.push((r.clone(), r.clone()));
    }

    let mut walked_prefixes: Vec<PathBuf> = Vec::new();
    let restricted_dirs = std::sync::atomic::AtomicUsize::new(0);
    #[cfg(target_os = "macos")]
    let macos_home = dirs::home_dir();
    for (root, filter_root) in plan {
        let collected: Mutex<Vec<(String, bool, bool)>> = Mutex::new(Vec::new());
        let count = std::sync::atomic::AtomicUsize::new(snap.entries.len());
        let remote = remote_mounts();
        let gvfs_roots = super::gvfs::gvfs_mount_roots();
        let skip_already: std::collections::HashSet<PathBuf> =
            walked_prefixes.iter().cloned().collect();
        let use_git = !opts.everything && opts.respect_gitignore;
        let everything = opts.everything;
        let filter_matcher = opts.everything.then(|| {
            FilterMatcher::new(
                &filter_root,
                FilterConfig {
                    include_hidden: opts.include_hidden,
                    respect_gitignore: opts.respect_gitignore,
                    exclude: opts.exclude.clone(),
                    use_gallignore: true,
                },
            )
        });
        // If this priority folder is strictly inside its containing root, include the
        // folder entry itself once so searching for its own name finds it too.
        if root != filter_root {
            let hidden = filter_matcher
                .as_ref()
                .is_some_and(|m| m.is_path_hidden(&root, true));
            collected
                .lock()
                .unwrap()
                .push((root.to_string_lossy().into_owned(), true, hidden));
            count.fetch_add(1, Ordering::Relaxed);
        }
        let mut wb = ignore::WalkBuilder::new(&root);
        wb.hidden(!opts.include_hidden)
            .git_ignore(use_git)
            .git_global(use_git)
            .git_exclude(use_git)
            .ignore(use_git)
            .parents(use_git)
            .require_git(true)
            .follow_links(false)
            .filter_entry(move |e| {
                !is_system_skip(e.path())
                    && (e.depth() == 0 || !remote.contains(e.path()))
                    && (e.depth() == 0 || !gvfs_roots.iter().any(|g| e.path() == g.as_path()))
                    && (e.depth() == 0 || !skip_already.contains(e.path()))
                    && (everything || !super::ignorefile::skip_cache(e))
            })
            .threads(
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4)
                    .min(4),
            );
        super::ignorefile::configure(&mut wb, !opts.everything);
        if !opts.everything && !opts.exclude.is_empty() {
            let mut ob = ignore::overrides::OverrideBuilder::new(&root);
            for g in &opts.exclude {
                let _ = ob.add(&format!("!{g}"));
            }
            if let Ok(o) = ob.build() {
                wb.overrides(o);
            }
        }
        wb.build_parallel().run(|| {
            set_idle_io_priority();
            let collected = &collected;
            let count = &count;
            let restricted_dirs = &restricted_dirs;
            #[cfg(target_os = "macos")]
            let macos_home = &macos_home;
            let filter_matcher = &filter_matcher;
            let mut local = Flush {
                items: Vec::with_capacity(4096),
                into: collected,
            };
            Box::new(move |res| {
                use ignore::WalkState;
                if cancel.load(Ordering::Relaxed)
                    || count.load(Ordering::Relaxed) >= opts.max_entries
                {
                    return WalkState::Quit;
                }
                match res {
                    Ok(d) => {
                        if d.depth() > 0 {
                            let is_dir = d.file_type().is_some_and(|t| t.is_dir());
                            let hidden = filter_matcher
                                .as_ref()
                                .is_some_and(|m| m.is_path_hidden(d.path(), is_dir));
                            local.items.push((
                                d.path().to_string_lossy().into_owned(),
                                is_dir,
                                hidden,
                            ));
                            if local.items.len() >= 4096 {
                                count.fetch_add(local.items.len(), Ordering::Relaxed);
                                local.flush();
                            }
                        }
                    }
                    Err(err) => {
                        #[cfg(target_os = "macos")]
                        if is_permission_denied_tcc(&err, macos_home.as_deref()) {
                            restricted_dirs.fetch_add(1, Ordering::Relaxed);
                        }
                        #[cfg(not(target_os = "macos"))]
                        let _ = (&err, restricted_dirs);
                    }
                }
                WalkState::Continue
            })
        });
        // Each thread's remaining buffer is flushed when its closure drops,
        // which happens before `run` returns.
        let items = collected.into_inner().unwrap();
        push_items(&mut snap, items, opts.max_entries);
        walked_prefixes.push(root);
        if cancel.load(Ordering::Relaxed) || snap.entries.len() >= opts.max_entries {
            break;
        }
    }
    snap.restricted_dirs = restricted_dirs.load(Ordering::Relaxed);
    snap.built_at = Some(SystemTime::now());
    snap
}

/// True when `path` lies inside a macOS TCC / Full Disk Access protected folder under
/// `home` (such as `~/Library` or `~/.Trash`).
pub fn is_tcc_protected_path(path: &Path, home: Option<&Path>) -> bool {
    let Some(home) = home else { return false };
    let Ok(rel) = path.strip_prefix(home) else {
        return false;
    };
    let Some(first) = rel.components().next() else {
        return false;
    };
    matches!(
        first.as_os_str().to_str(),
        Some("Library" | ".Trash" | "Desktop" | "Documents" | "Downloads")
    )
}

#[cfg(target_os = "macos")]
fn is_permission_denied_tcc(err: &ignore::Error, home: Option<&Path>) -> bool {
    let Some(io_err) = err.io_error() else {
        return false;
    };
    if io_err.kind() != std::io::ErrorKind::PermissionDenied {
        return false;
    }
    fn extract_path(err: &ignore::Error) -> Option<&Path> {
        match err {
            ignore::Error::WithPath { path, .. } => Some(path.as_path()),
            ignore::Error::WithDepth { err, .. } => extract_path(err),
            ignore::Error::WithLineNumber { err, .. } => extract_path(err),
            ignore::Error::Loop { child, .. } => Some(child.as_path()),
            ignore::Error::Partial(list) => list.iter().find_map(extract_path),
            _ => None,
        }
    }
    extract_path(err).is_some_and(|p| is_tcc_protected_path(p, home))
}

/// Per-thread buffer that moves its items into the shared list on drop.
struct Flush<'a> {
    items: Vec<(String, bool, bool)>,
    into: &'a Mutex<Vec<(String, bool, bool)>>,
}

impl Flush<'_> {
    fn flush(&mut self) {
        if !self.items.is_empty() {
            self.into.lock().unwrap().append(&mut self.items);
        }
    }
}

impl Drop for Flush<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

fn push_items(snap: &mut Snapshot, items: Vec<(String, bool, bool)>, max: usize) {
    for (p, is_dir, hidden) in items {
        if snap.entries.len() >= max {
            break;
        }
        let start = snap.arena.len();
        if start + p.len() > u32::MAX as usize {
            break;
        }
        let name = p.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
        snap.arena.push_str(&p);
        snap.entries.push(Entry {
            start: start as u32,
            len: p.len() as u32,
            name: name.min(u16::MAX as usize) as u16,
            is_dir,
            hidden,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_mounts_from_proc_mounts() {
        let t = "/dev/nvme0n1p3 /home btrfs rw 0 0\n\
                 nas:/export /mnt/nas nfs4 rw 0 0\n\
                 //srv/share /mnt/My\\040Share cifs rw 0 0\n\
                 gdrive: /home/u/Google\\040Drive fuse.rclone rw 0 0\n\
                 kio-fuse /run/user/1000/kio-fuse-AbCdEf fuse.kio-fuse rw 0 0\n\
                 /dev/sdb1 /run/media/u/USB fuseblk rw 0 0\n";
        let r = parse_remote_mounts(t);
        assert!(r.contains(Path::new("/mnt/nas")));
        assert!(r.contains(Path::new("/mnt/My Share")));
        assert!(r.contains(Path::new("/home/u/Google Drive")));
        assert!(r.contains(Path::new("/run/user/1000/kio-fuse-AbCdEf")));
        assert!(!r.contains(Path::new("/home")));
        assert!(
            !r.contains(Path::new("/run/media/u/USB")),
            "a local NTFS drive (fuseblk) is indexed"
        );
    }

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        for f in [
            "src/main.rs",
            "src/fs/index.rs",
            "ui/src/App.tsx",
            "ui/src/FolderPane.tsx",
            "README.md",
            ".hidden/x.rs",
            "node_modules/pkg/index.js",
        ] {
            let f = p.join(f);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, "").unwrap();
        }
        d
    }

    #[test]
    fn requested_build_counts_as_building_until_a_later_build_ends() {
        let d = tree();
        let idx = FileIndex::new();
        let opts = IndexOptions::default();
        let roots = [d.path().to_path_buf()];
        assert!(!idx.status().building);
        // Right after the request, before any thread runs: a poller must not see "done".
        idx.request_rebuild();
        assert!(idx.status().building && !idx.is_building());
        assert!(idx.rebuild(&roots, &opts, &AtomicBool::new(false)));
        assert!(!idx.status().building, "the build took over the request");
        assert!(!idx.rebuild_requested());
        // A request that arrives while a build runs outlives it: that build began earlier.
        idx.request_rebuild();
        std::fs::write(d.path().join("late.txt"), "x").unwrap();
        assert!(idx.status().building);
        assert!(idx.rebuild(&roots, &opts, &AtomicBool::new(false)));
        assert!(idx
            .query("late.txt", None, 5)
            .hits
            .iter()
            .any(|h| h.name == "late.txt"));
        idx.request_rebuild();
        idx.withdraw_request();
        assert!(!idx.status().building);
    }

    #[test]
    fn builds_and_queries() {
        let d = tree();
        let idx = FileIndex::new();
        assert!(idx.rebuild(
            &[d.path().to_path_buf()],
            &IndexOptions::default(),
            &AtomicBool::new(false)
        ));
        let st = idx.status();
        // src, src/fs, ui, ui/src dirs + 5 files (hidden and node_modules excluded)
        assert_eq!(st.entries, 9, "{:?}", st);

        // Matching is fuzzy over the whole path, so the random temp folder name (on macOS a
        // long /var/folders/... path) can supply the letters for other files too. The
        // intended file must rank first; do not assert that it is the only hit.
        let r = idx.query("fpane", None, 10);
        assert!(!r.hits.is_empty());
        assert!(r.hits[0].path.ends_with("FolderPane.tsx"), "{:?}", r.hits);
        assert_eq!(r.hits[0].name, "FolderPane.tsx");

        let r = idx.query("ext:rs", None, 10);
        assert_eq!(r.total, 2);

        // Scope restricts results.
        let r = idx.query("ext:tsx", Some(&d.path().join("src")), 10);
        assert_eq!(r.total, 0);

        // Limit truncates but reports total, best first.
        let r = idx.query("s", None, 2);
        assert_eq!(r.hits.len(), 2);
        assert!(r.total > 2);
        assert!(r.hits[0].score >= r.hits[1].score);

        assert!(idx
            .query("index", None, 10)
            .hits
            .iter()
            .all(|h| !h.path.contains("node_modules")));

        // A trailing separator lists only directories ("src" and "ui/src", not files under them).
        let r = idx.query("src/", None, 10);
        assert!(
            r.total >= 2 && r.hits.iter().all(|h| h.is_dir),
            "{:?}",
            r.hits
        );
        assert!(r.hits.iter().any(|h| h.name == "src"));
        // Files never appear with a trailing separator (the temp path may still match "fpane").
        assert!(idx
            .query("fpane/", None, 10)
            .hits
            .iter()
            .all(|h| h.is_dir && h.name != "FolderPane.tsx"));
    }

    #[test]
    fn deleted_matches_are_filtered_and_live_results_fill_the_limit() {
        let d = tree();
        let idx = FileIndex::new();
        idx.rebuild(
            &[d.path().to_path_buf()],
            &IndexOptions::default(),
            &AtomicBool::new(false),
        );
        std::fs::remove_file(d.path().join("src/main.rs")).unwrap();
        std::fs::remove_dir_all(d.path().join("ui")).unwrap();

        assert_eq!(
            idx.query_cached(&idx.snap.read().unwrap(), "'main.rs", None, 10)
                .total,
            1,
            "the cached snapshot still contains the deleted file"
        );
        assert_eq!(idx.query("'main.rs", None, 10).total, 0);
        assert!(idx.query("ext:tsx", None, 10).hits.is_empty());
        assert!(idx
            .query("'ui/", Some(&d.path().join("ui")), 10)
            .hits
            .is_empty());
        let result = idx.query("ext:rs", None, 1);
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].name, "index.rs");
        assert_eq!(result.total, 1);

        // A path recreated before the next rebuild is searchable again.
        std::fs::write(d.path().join("src/main.rs"), "").unwrap();
        assert_eq!(idx.query("'main.rs", None, 10).hits.len(), 1);
    }

    #[test]
    fn hidden_items_leave_at_once_and_stay_out() {
        let d = tree();
        let p = d.path();
        std::fs::create_dir_all(p.join("target/debug")).unwrap();
        std::fs::write(
            p.join("target/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
        std::fs::write(p.join("target/debug/app.rs"), "").unwrap();
        let opts = IndexOptions {
            exclude: vec![],
            ..Default::default()
        };
        let idx = FileIndex::new();
        let cancel = AtomicBool::new(false);
        idx.rebuild(&[p.to_path_buf()], &opts, &cancel);
        let rs = |idx: &FileIndex| idx.query("ext:rs", None, 50).total;
        // Cargo's target is a tagged cache: skipped even without an exclude for it.
        assert_eq!(rs(&idx), 2);

        // ui and everything below it, not "ui2" or "src".
        std::fs::create_dir(p.join("ui2")).unwrap();
        idx.rebuild(&[p.to_path_buf()], &opts, &cancel);
        let before = idx.status().entries;
        assert_eq!(idx.forget_under(&p.join("ui")), 4);
        assert_eq!(idx.status().entries, before - 4);
        assert_eq!(idx.query("'FolderPane", None, 10).total, 0);
        // Use an exact query and filter by name so random tempdir paths on Windows
        // (`C:\Users\runneradmin\...\tmp...2...`) cannot fuzzy-match `"ui2"`.
        assert_eq!(
            idx.query("'ui2", None, 10)
                .hits
                .iter()
                .filter(|h| h.name == "ui2")
                .count(),
            1
        );
        assert_eq!(idx.forget_under(&p.join("ui")), 0);

        // After a .gallignore entry, a rebuild keeps it out.
        crate::fs::ignorefile::hide_from_search(&p.join("src")).unwrap();
        idx.rebuild(&[p.to_path_buf()], &opts, &cancel);
        assert_eq!(rs(&idx), 0);
        assert!(idx.query("App.tsx", None, 10).total > 0);
    }

    #[test]
    fn learns_renames_and_listings_without_a_rebuild_and_survives_a_cache_round_trip() {
        let d = tree();
        let p = d.path();
        let idx = FileIndex::new();
        let new_file = p.join("src/fresh.rs");
        std::fs::write(&new_file, "").unwrap();
        // Nothing is learned before the first build.
        assert_eq!(idx.upsert_paths(&[(new_file.clone(), false)]), 0);
        let opts = IndexOptions {
            exclude: vec![],
            ..Default::default()
        };
        idx.rebuild(&[p.to_path_buf()], &opts, &AtomicBool::new(false));
        assert_eq!(idx.query("'fresh.rs", None, 10).total, 1);

        let later = p.join("src").join("later.rs");
        std::fs::write(&later, "").unwrap();
        assert_eq!(
            idx.upsert_dir(
                &p.join("src"),
                &[("later.rs".into(), false), (".dot".into(), false)]
            ),
            1
        );
        assert_eq!(idx.query("'later.rs", None, 10).total, 1);
        // Already known paths are not added twice.
        assert_eq!(idx.upsert_paths(&[(later, false)]), 0);

        std::fs::rename(p.join("ui"), p.join("web")).unwrap();
        assert!(idx.rename_prefix(&p.join("ui"), &p.join("web")) > 0);
        let hits = idx.query("'App.tsx", None, 10).hits;
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.contains("web"));

        let cache = p.join("index.cache");
        idx.save_cache(&cache).unwrap();
        let fresh = FileIndex::new();
        assert!(fresh.load_cache(&cache) > 0);
        assert_eq!(fresh.query("'App.tsx", None, 10).total, 1);
        assert_eq!(fresh.query("'later.rs", None, 10).total, 1);
        // Garbage is ignored.
        std::fs::write(&cache, "nonsense").unwrap();
        assert_eq!(FileIndex::new().load_cache(&cache), 0);
    }

    #[test]
    fn covers_only_folders_inside_the_roots() {
        let d = tree();
        let p = d.path();
        let roots = [p.join("src")];
        assert!(index_covers(&roots, &p.join("src")));
        assert!(index_covers(&roots, &p.join("src/fs")));
        assert!(!index_covers(&roots, &p.join("ui")));
        assert!(!index_covers(&roots, p));
        assert!(!index_covers(&[], p));
    }

    #[test]
    fn nested_roots_are_merged() {
        let d = tree();
        let r = normalize_roots(&[d.path().to_path_buf(), d.path().join("src")]);
        assert_eq!(r, vec![d.path().to_path_buf()]);
    }

    #[test]
    fn utf16_offsets() {
        assert_eq!(utf16_positions("a😀b", &[0, 2]), vec![0, 3]);
    }

    #[test]
    fn hidden_count_and_everything_index_marking() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        for dir in [".git", "src", "gen", "dist", "node_modules/pkg"] {
            std::fs::create_dir_all(p.join(dir)).unwrap();
        }
        std::fs::write(p.join("src/widget.rs"), "").unwrap();
        std::fs::write(p.join("gen/widget_gen.rs"), "").unwrap();
        std::fs::write(p.join("dist/only_secret.js"), "").unwrap();
        std::fs::write(p.join("node_modules/pkg/only_secret_dep.js"), "").unwrap();
        std::fs::write(p.join(".gitignore"), "dist/\n").unwrap();
        std::fs::write(p.join(".gallignore"), "/gen/\n").unwrap();

        let filter = FilterConfig {
            include_hidden: false,
            respect_gitignore: true,
            exclude: vec!["node_modules".into()],
            use_gallignore: true,
        };
        let cancel = AtomicBool::new(false);

        // Normal index excludes hidden files.
        let normal = FileIndex::new();
        normal.rebuild(
            &[p.to_path_buf()],
            &IndexOptions {
                exclude: vec!["node_modules".into()],
                ..Default::default()
            },
            &cancel,
        );
        assert_eq!(normal.query("only_secret", None, 20).total, 0);
        assert_eq!(normal.query("widget", None, 20).total, 1);

        // Zero visible + 2 hidden.
        let hc = count_hidden(p, "only_secret", &filter, CountBudget::for_tests(), &cancel);
        assert_eq!(
            hc,
            HiddenCount {
                count: 2,
                capped: false
            }
        );

        // Some visible (1) + 1 hidden.
        let hc = count_hidden(p, "widget", &filter, CountBudget::for_tests(), &cancel);
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
            "definitely_missing_xyz",
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

        // Everything index includes all and flags only normally-hidden entries.
        let everything = FileIndex::new();
        everything.rebuild(
            &[p.to_path_buf()],
            &IndexOptions {
                exclude: vec!["node_modules".into()],
                everything: true,
                ..Default::default()
            },
            &cancel,
        );
        let res = everything.query("widget", None, 20);
        let mut tagged: Vec<(String, bool)> =
            res.hits.into_iter().map(|h| (h.name, h.hidden)).collect();
        tagged.sort();
        assert_eq!(
            tagged,
            vec![
                ("widget.rs".to_string(), false),
                ("widget_gen.rs".to_string(), true),
            ]
        );

        // Multi-root / everywhere with a priority folder inside the root: no duplicates,
        // and priority folder inside an ignored dir still counts its files as hidden.
        let hc_multi = count_hidden_roots(
            &[p.to_path_buf()],
            &[p.join("gen")],
            "widget",
            &filter,
            CountBudget::for_tests(),
            &cancel,
        );
        assert_eq!(
            hc_multi,
            HiddenCount {
                count: 1,
                capped: false
            }
        );

        let everything_prio = FileIndex::new();
        everything_prio.rebuild(
            &[p.to_path_buf()],
            &IndexOptions {
                exclude: vec!["node_modules".into()],
                everything: true,
                priority_roots: vec![p.join("gen")],
                ..Default::default()
            },
            &cancel,
        );
        let mut prio_tagged: Vec<(String, bool)> = everything_prio
            .query("widget", None, 20)
            .hits
            .into_iter()
            .map(|h| (h.name, h.hidden))
            .collect();
        prio_tagged.sort();
        assert_eq!(
            prio_tagged,
            vec![
                ("widget.rs".to_string(), false),
                ("widget_gen.rs".to_string(), true),
            ]
        );
    }

    #[test]
    fn hidden_count_ignores_ancestor_path_and_scattered_target_noise() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("search-hidden-proj");
        std::fs::create_dir_all(proj.join("target/debug/deps")).unwrap();
        std::fs::create_dir_all(proj.join("gen")).unwrap();
        std::fs::write(proj.join(".gallignore"), "/gen/\n").unwrap();
        for i in 0..150 {
            std::fs::write(
                proj.join(format!("target/debug/deps/crate_{i:03}_out.rlib")),
                "",
            )
            .unwrap();
        }
        for name in ["hidden_a.txt", "hidden_b.txt", "hidden_c.txt"] {
            std::fs::write(proj.join("gen").join(name), "").unwrap();
        }

        let filter = FilterConfig {
            include_hidden: false,
            respect_gitignore: false,
            exclude: vec!["target".into()],
            use_gallignore: true,
        };
        let cancel = AtomicBool::new(false);

        // Ancestor folder name ("search-hidden-proj") must NOT inflate the count to 100+.
        let hc = count_hidden(&proj, "hidden", &filter, CountBudget::for_tests(), &cancel);
        assert_eq!(
            hc,
            HiddenCount {
                count: 3,
                capped: false
            }
        );

        // Scattered path letters ("report" across "target/debug/deps/crate_..._out") must
        // NOT inflate the count to 100+.
        let hc = count_hidden(&proj, "report", &filter, CountBudget::for_tests(), &cancel);
        assert_eq!(
            hc,
            HiddenCount {
                count: 0,
                capped: false
            }
        );

        // Explicit path query with `/` still matches hidden folder paths.
        let hc = count_hidden(
            &proj,
            "gen/hidden",
            &filter,
            CountBudget::for_tests(),
            &cancel,
        );
        assert_eq!(
            hc,
            HiddenCount {
                count: 3,
                capped: false
            }
        );

        // Alt+I (everything) agrees 1-to-1 with the hidden count (3 matches, not 153).
        let everything = FileIndex::new();
        everything.rebuild(
            std::slice::from_ref(&proj),
            &IndexOptions {
                exclude: vec!["target".into()],
                everything: true,
                ..Default::default()
            },
            &cancel,
        );
        let res = everything.query("hidden", Some(&proj), 200);
        assert_eq!(res.total, 3);
        assert!(res.hits.iter().all(|h| h.hidden));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn gvfs_paths_are_not_covered_or_rebuilt() {
        let gvfs = Path::new("/run/user/1000/gvfs/mtp:host=Pixel");
        assert!(!index_covers(&[PathBuf::from("/")], gvfs));
        assert!(!index_covers(&[gvfs.to_path_buf()], gvfs));

        let idx = FileIndex::new();
        let cancel = AtomicBool::new(false);
        assert!(idx.rebuild(&[gvfs.to_path_buf()], &IndexOptions::default(), &cancel));
        assert_eq!(idx.status().entries, 0);
        assert!(idx.status().built_at.is_some());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn priority_roots_inside_gvfs_mount_are_not_walked() {
        struct XdgRuntimeGuard(Option<std::ffi::OsString>);
        impl Drop for XdgRuntimeGuard {
            fn drop(&mut self) {
                if let Some(prev) = self.0.take() {
                    std::env::set_var("XDG_RUNTIME_DIR", prev);
                } else {
                    std::env::remove_var("XDG_RUNTIME_DIR");
                }
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let gvfs_sub = tmp
            .path()
            .join("gvfs")
            .join("smb-share:server=nas,share=media");
        std::fs::create_dir_all(&gvfs_sub).unwrap();
        std::fs::write(gvfs_sub.join("gvfs_secret_file.txt"), b"remote").unwrap();
        std::fs::write(tmp.path().join("local_visible_file.txt"), b"local").unwrap();

        let _guard = XdgRuntimeGuard(std::env::var_os("XDG_RUNTIME_DIR"));
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());

        assert!(super::super::gvfs::is_slow_or_gvfs_mount(&gvfs_sub));
        assert!(!index_covers(
            std::slice::from_ref(&tmp.path().to_path_buf()),
            &gvfs_sub
        ));

        let idx = FileIndex::new();
        let cancel = AtomicBool::new(false);
        assert!(idx.rebuild(
            &[tmp.path().to_path_buf()],
            &IndexOptions {
                priority_roots: vec![gvfs_sub],
                ..Default::default()
            },
            &cancel,
        ));
        assert_eq!(idx.query("gvfs_secret_file", None, 10).total, 0);
        assert_eq!(idx.query("local_visible_file", None, 10).total, 1);
    }

    #[test]
    fn tcc_protected_path_classification() {
        let home = Path::new("/Users/alice");
        assert!(is_tcc_protected_path(
            Path::new("/Users/alice/Library/Mail"),
            Some(home)
        ));
        assert!(is_tcc_protected_path(
            Path::new("/Users/alice/.Trash"),
            Some(home)
        ));
        assert!(is_tcc_protected_path(
            Path::new("/Users/alice/Documents/secret"),
            Some(home)
        ));
        assert!(!is_tcc_protected_path(
            Path::new("/Users/alice/Projects/app"),
            Some(home)
        ));
        assert!(!is_tcc_protected_path(Path::new("/tmp/foo"), Some(home)));
        assert!(!is_tcc_protected_path(
            Path::new("/Users/alice/Library"),
            None
        ));
    }

    #[test]
    fn path_prefix_query_detects_direct_paths_only() {
        assert!(is_path_prefix_query("/"));
        assert!(is_path_prefix_query("/etc/ng"));
        assert!(is_path_prefix_query("  /var/log/"));
        assert!(is_path_prefix_query("~"));
        assert!(is_path_prefix_query("~/Projects"));
        assert!(is_path_prefix_query("C:\\Windows"));
        assert!(is_path_prefix_query("D:/src"));

        // Normal fuzzy queries and trailing-slash folder filters must stay index queries.
        assert!(!is_path_prefix_query(""));
        assert!(!is_path_prefix_query("   "));
        assert!(!is_path_prefix_query("src/"));
        assert!(!is_path_prefix_query("main.rs"));
        assert!(!is_path_prefix_query("ext:rs"));
        assert!(!is_path_prefix_query("~foo"));
    }

    #[test]
    fn path_prefix_query_completes_single_directory_live() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("alpha/nested")).unwrap();
        std::fs::write(root.join("alpha/nested/deep.txt"), "x").unwrap();
        std::fs::write(root.join("almanac.md"), "x").unwrap();
        std::fs::write(root.join("beta.txt"), "x").unwrap();
        std::fs::write(root.join(".hidden"), "x").unwrap();

        // Trailing separator lists direct children only (never recurses into alpha/nested),
        // folders first, hiding dotfiles unless asked.
        let sep = std::path::MAIN_SEPARATOR;
        let listed = query_path_prefix(&format!("{}{sep}", root.display()), false, 50).unwrap();
        let names: Vec<&str> = listed.hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["alpha", "almanac.md", "beta.txt"]);
        assert!(listed.hits[0].is_dir);
        assert!(!listed.scanning);
        assert_eq!(
            Path::new(&listed.hits[0].path),
            root.join("alpha"),
            "hit paths point into the listed folder"
        );

        // Fuzzy filter narrows within that directory and highlights characters inside the filename.
        let filtered = query_path_prefix(&format!("{}{sep}al", root.display()), false, 50).unwrap();
        let fnames: Vec<&str> = filtered.hits.iter().map(|h| h.name.as_str()).collect();
        assert!(fnames.contains(&"alpha") && fnames.contains(&"almanac.md"));
        assert!(!fnames.contains(&"beta.txt"));
        for hit in &filtered.hits {
            assert!(!hit.positions.is_empty(), "matched hit must have positions");
        }

        // Dot prefix reveals hidden entries even when include_hidden is false.
        let dots = query_path_prefix(&format!("{}{sep}.hid", root.display()), false, 50).unwrap();
        assert_eq!(dots.hits.len(), 1);
        assert_eq!(dots.hits[0].name, ".hidden");

        // Missing directory returns empty without falling back to the index.
        let missing = query_path_prefix(
            &format!("{}{sep}no-such-dir{sep}x", root.display()),
            false,
            50,
        )
        .unwrap();
        assert!(missing.hits.is_empty());

        // Unix default system_roots() targets the user's home folder instead of `/`.
        #[cfg(unix)]
        if let Some(home) = dirs::home_dir() {
            assert_eq!(system_roots(), vec![home]);
        }

        #[cfg(windows)]
        {
            // `C:/…` with forward slashes reads the same folder as `C:\…`.
            let fwd = format!("{}/al", root.display()).replace('\\', "/");
            let fwd_names: Vec<String> = query_path_prefix(&fwd, false, 50)
                .unwrap()
                .hits
                .into_iter()
                .map(|h| h.name)
                .collect();
            assert!(
                fwd_names.contains(&"alpha".into()) && fwd_names.contains(&"almanac.md".into())
            );
            assert!(!fwd_names.contains(&"beta.txt".into()));

            // A bare drive root `C:\` lists that drive, not the current directory.
            let drive = &root.to_string_lossy()[..2];
            let at_root = query_path_prefix(&format!("{drive}\\"), true, 500).unwrap();
            assert!(!at_root.hits.is_empty(), "drive root must list entries");
            for hit in &at_root.hits {
                let parent = Path::new(&hit.path).parent().map(Path::to_path_buf);
                assert_eq!(
                    parent,
                    Some(PathBuf::from(format!("{drive}\\"))),
                    "{}",
                    hit.path
                );
            }
        }
    }
}
