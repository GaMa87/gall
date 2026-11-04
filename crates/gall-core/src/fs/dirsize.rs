//! Folder sizes (F-14): the total size of everything inside a folder, on demand.
//!
//! A parallel walk that does not follow symlinks and stays on the folder's file system
//! (like `du -x`: `/` does not count `/proc` or mounted drives), counts a file with
//! several hard links once, reports running totals while it counts, and stops when
//! cancelled.

use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirSize {
    /// Sum of the file sizes (apparent size, as shown for files).
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
    /// Items that could not be read (no permission); the total is then a lower bound.
    pub unreadable: u64,
}

/// How often running totals are reported.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Size of everything under `dir`. `progress` gets the running totals now and then;
/// returns None when `cancel` is set before the walk ends.
pub fn dir_size(
    dir: &Path,
    cancel: &AtomicBool,
    progress: &(dyn Fn(&DirSize) + Sync),
) -> Option<DirSize> {
    let bytes = AtomicU64::new(0);
    let files = AtomicU64::new(0);
    let dirs = AtomicU64::new(0);
    let unreadable = AtomicU64::new(0);
    // Hard-linked files seen so far: (device, inode).
    let linked: Mutex<std::collections::HashSet<(u64, u64)>> = Mutex::default();
    let last = Mutex::new(Instant::now());
    let snapshot = || DirSize {
        bytes: bytes.load(Ordering::Relaxed),
        files: files.load(Ordering::Relaxed),
        dirs: dirs.load(Ordering::Relaxed),
        unreadable: unreadable.load(Ordering::Relaxed),
    };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    ignore::WalkBuilder::new(dir)
        .standard_filters(false)
        .hidden(false)
        .follow_links(false)
        .same_file_system(true)
        .threads(threads)
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                if cancel.load(Ordering::Relaxed) {
                    return ignore::WalkState::Quit;
                }
                let e = match entry {
                    Ok(e) => e,
                    Err(_) => {
                        unreadable.fetch_add(1, Ordering::Relaxed);
                        return ignore::WalkState::Continue;
                    }
                };
                if e.depth() == 0 {
                    return ignore::WalkState::Continue;
                }
                match e.file_type() {
                    Some(t) if t.is_dir() => {
                        dirs.fetch_add(1, Ordering::Relaxed);
                    }
                    Some(t) if t.is_file() => {
                        files.fetch_add(1, Ordering::Relaxed);
                        match e.metadata() {
                            Ok(m) if counted_once(&m, &linked) => {
                                bytes.fetch_add(m.len(), Ordering::Relaxed);
                            }
                            Ok(_) => {}
                            Err(_) => {
                                unreadable.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    // Symlinks and special files: counted, their targets are not.
                    _ => {
                        files.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if let Ok(mut t) = last.try_lock() {
                    if t.elapsed() >= PROGRESS_EVERY {
                        *t = Instant::now();
                        progress(&snapshot());
                    }
                }
                ignore::WalkState::Continue
            })
        });
    (!cancel.load(Ordering::Relaxed)).then(snapshot)
}

/// False for the second and later hard links of a file (Unix).
fn counted_once(
    m: &std::fs::Metadata,
    linked: &Mutex<std::collections::HashSet<(u64, u64)>>,
) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.nlink() > 1 {
            return linked.lock().unwrap().insert((m.dev(), m.ino()));
        }
    }
    #[cfg(not(unix))]
    let _ = (m, linked);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn sums_nested_files_and_counts_items() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("a/b/c")).unwrap();
        fs::write(t.path().join("one.bin"), vec![0u8; 1000]).unwrap();
        fs::write(t.path().join("a/two.bin"), vec![0u8; 2000]).unwrap();
        fs::write(t.path().join("a/b/c/three.bin"), vec![0u8; 3000]).unwrap();
        fs::write(t.path().join("a/.hidden"), vec![0u8; 5]).unwrap();
        let s = dir_size(t.path(), &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(
            s,
            DirSize {
                bytes: 6005,
                files: 4,
                dirs: 3,
                unreadable: 0
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_count_once_and_symlinks_are_not_followed() {
        let t = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("big"), vec![0u8; 50_000]).unwrap();
        fs::write(t.path().join("f"), vec![0u8; 700]).unwrap();
        fs::hard_link(t.path().join("f"), t.path().join("f-link")).unwrap();
        std::os::unix::fs::symlink(outside.path(), t.path().join("elsewhere")).unwrap();
        let s = dir_size(t.path(), &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(s.bytes, 700);
        assert_eq!(s.files, 3, "f, f-link and the symlink");
    }

    #[test]
    fn cancelled_returns_none() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("x"), b"x").unwrap();
        assert_eq!(dir_size(t.path(), &AtomicBool::new(true), &|_| {}), None);
    }
}
