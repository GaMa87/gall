//! In-memory file-name index for instant "find anything" (Quick Open).
//!
//! The index is built in the background with a parallel directory walk
//! (the `ignore` crate from ripgrep) and stored as one compact string arena.
//! Queries scan the whole index in parallel with the fuzzy scorer, so a
//! million paths answer in tens of milliseconds. While a rebuild runs,
//! queries keep using the previous snapshot.

use super::fuzzy::{Query, Scratch};
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
    /// Hard cap on indexed entries, to bound memory.
    pub max_entries: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions {
            include_hidden: false,
            respect_gitignore: true,
            exclude: default_excludes(),
            max_entries: 3_000_000,
        }
    }
}

/// Default roots for a whole-system index: `/` on Unix, every drive on Windows.
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
        vec![PathBuf::from("/")]
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
    #[cfg(not(target_os = "linux"))]
    {
        Default::default()
    }
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
}

#[derive(Default)]
struct Snapshot {
    arena: String,
    entries: Vec<Entry>,
    roots: Vec<PathBuf>,
    built_at: Option<SystemTime>,
    build_ms: u64,
}

impl Snapshot {
    fn path(&self, e: &Entry) -> &str {
        &self.arena[e.start as usize..(e.start + e.len) as usize]
    }
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
    pending_roots: Mutex<Vec<PathBuf>>,
    /// Paths under these prefixes rank higher (e.g. the home folder).
    preferred: RwLock<Vec<String>>,
}

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
            building: self.building.load(Ordering::Relaxed),
            built_at: s.built_at.and_then(|t| super::entry::to_millis(Ok(t))),
            build_ms: s.build_ms,
        }
    }

    pub fn is_building(&self) -> bool {
        self.building.load(Ordering::Relaxed)
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
        let roots = normalize_roots(roots);
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
        let t0 = Instant::now();
        let snap = self.snap.read().unwrap().clone();
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
                            if let Some(s) = scope {
                                if !p.starts_with(s) {
                                    continue;
                                }
                            }
                            if let Some(m) = q.score(p, e.name as usize, &mut scratch) {
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

/// Whether the system index (built from `roots`) holds what is in `path`. Not for a
/// folder outside every root (a WSL distribution `\\wsl.localhost\…` on Windows, whose
/// default roots are the drives; a folder outside custom indexed folders) or on a
/// network mount below a root, which the index never walks into: Quick Open scans those
/// itself. A WSL folder listed as an indexed folder is covered like any other root.
pub fn index_covers(roots: &[PathBuf], path: &Path) -> bool {
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
    let mut r: Vec<PathBuf> = roots.iter().filter(|p| p.is_dir()).cloned().collect();
    r.sort();
    r.dedup();
    let all = r.clone();
    r.retain(|p| !all.iter().any(|o| o != p && p.starts_with(o)));
    r
}

fn build(roots: &[PathBuf], opts: &IndexOptions, cancel: &AtomicBool) -> Snapshot {
    let mut snap = Snapshot {
        roots: roots.to_vec(),
        ..Default::default()
    };
    for root in roots {
        let collected: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());
        let count = std::sync::atomic::AtomicUsize::new(snap.entries.len());
        let remote = remote_mounts();
        let mut wb = ignore::WalkBuilder::new(root);
        wb.hidden(!opts.include_hidden)
            .git_ignore(opts.respect_gitignore)
            .git_global(opts.respect_gitignore)
            .git_exclude(opts.respect_gitignore)
            .ignore(opts.respect_gitignore)
            .parents(opts.respect_gitignore)
            .require_git(true)
            .follow_links(false)
            .filter_entry(move |e| {
                !is_system_skip(e.path())
                    && (e.depth() == 0 || !remote.contains(e.path()))
                    && !super::ignorefile::skip_cache(e)
            })
            .threads(
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4)
                    .min(8),
            );
        super::ignorefile::configure(&mut wb, true);
        if !opts.exclude.is_empty() {
            let mut ob = ignore::overrides::OverrideBuilder::new(root);
            for g in &opts.exclude {
                let _ = ob.add(&format!("!{g}"));
            }
            if let Ok(o) = ob.build() {
                wb.overrides(o);
            }
        }
        wb.build_parallel().run(|| {
            let collected = &collected;
            let count = &count;
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
                if let Ok(d) = res {
                    if d.depth() > 0 {
                        let is_dir = d.file_type().is_some_and(|t| t.is_dir());
                        local
                            .items
                            .push((d.path().to_string_lossy().into_owned(), is_dir));
                        if local.items.len() >= 4096 {
                            count.fetch_add(local.items.len(), Ordering::Relaxed);
                            local.flush();
                        }
                    }
                }
                WalkState::Continue
            })
        });
        // Each thread's remaining buffer is flushed when its closure drops,
        // which happens before `run` returns.
        let items = collected.into_inner().unwrap();
        push_items(&mut snap, items, opts.max_entries);
        if cancel.load(Ordering::Relaxed) {
            break;
        }
    }
    snap.built_at = Some(SystemTime::now());
    snap
}

/// Per-thread buffer that moves its items into the shared list on drop.
struct Flush<'a> {
    items: Vec<(String, bool)>,
    into: &'a Mutex<Vec<(String, bool)>>,
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

fn push_items(snap: &mut Snapshot, items: Vec<(String, bool)>, max: usize) {
    for (p, is_dir) in items {
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
                 /dev/sdb1 /run/media/u/USB fuseblk rw 0 0\n";
        let r = parse_remote_mounts(t);
        assert!(r.contains(Path::new("/mnt/nas")));
        assert!(r.contains(Path::new("/mnt/My Share")));
        assert!(r.contains(Path::new("/home/u/Google Drive")));
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

        let r = idx.query("fpane", None, 10);
        assert_eq!(r.hits.len(), 1);
        assert!(r.hits[0].path.ends_with("FolderPane.tsx"));
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
        assert_eq!(idx.query("fpane/", None, 10).total, 0);
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
        assert_eq!(idx.query("FolderPane", None, 10).total, 0);
        assert_eq!(idx.query("ui2", None, 10).total, 1);
        assert_eq!(idx.forget_under(&p.join("ui")), 0);

        // After a .gallignore entry, a rebuild keeps it out.
        crate::fs::ignorefile::hide_from_search(&p.join("src")).unwrap();
        idx.rebuild(&[p.to_path_buf()], &opts, &cancel);
        assert_eq!(rs(&idx), 0);
        assert!(idx.query("App.tsx", None, 10).total > 0);
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
}
