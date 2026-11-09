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
    /// Kernel virtual filesystem: byte totals are not applicable and no walk was run.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub virtual_fs: bool,
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
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    if is_kernel_filesystem(dir) {
        return (!cancel.load(Ordering::Relaxed)).then_some(DirSize {
            virtual_fs: true,
            ..DirSize::default()
        });
    }
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
        virtual_fs: false,
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

/// Detect the containing filesystem, including subfolders and alternate mount paths.
/// tmpfs is deliberately allowed: it contains ordinary memory-backed files.
#[cfg(target_os = "linux")]
fn is_kernel_filesystem(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    // On failure the normal walker reports unreadable entries; do not invent a type.
    if unsafe { libc::statfs(path.as_ptr(), &mut stats) } != 0 {
        return false;
    }
    // Linux kernel API filesystem types from linux/magic.h, not directory names.
    matches!(
        stats.f_type as u32,
        0x00009fa0 // proc
            | 0x62656572 // sysfs
            | 0x0027e0eb // cgroup
            | 0x63677270 // cgroup2
            | 0x64626720 // debugfs
            | 0x74726163 // tracefs
            | 0x73636673 // securityfs
            | 0xf97cff8c // selinuxfs
            | 0x42494e4d // binfmt_misc
            | 0xcafe4a11 // bpf
            | 0x00001cd1 // devpts
            | 0x6165676c // pstore
    )
}

#[cfg(not(target_os = "linux"))]
fn is_kernel_filesystem(_path: &Path) -> bool {
    false
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
                unreadable: 0,
                virtual_fs: false
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
    #[cfg(target_os = "linux")]
    #[test]
    fn proc_subfolders_are_not_applicable_without_progress() {
        let path = Path::new("/proc/self/fd");
        if !path.is_dir() {
            return;
        }
        let result = dir_size(path, &AtomicBool::new(false), &|_| {
            panic!("virtual filesystems must not emit counting progress")
        })
        .unwrap();
        assert_eq!(serde_json::to_value(result).unwrap()["virtualFs"], true);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kernel_filesystems_and_aliases_are_not_walked() {
        let alias = tempfile::tempdir().unwrap();
        for (i, path) in ["/proc", "/sys", "/sys/kernel"].iter().enumerate() {
            let path = Path::new(path);
            if !path.is_dir() || !is_kernel_filesystem(path) {
                continue;
            }
            let link = alias.path().join(i.to_string());
            std::os::unix::fs::symlink(path, &link).unwrap();
            for target in [path, link.as_path()] {
                let size = dir_size(target, &AtomicBool::new(false), &|_| {
                    panic!("kernel filesystems must not be walked")
                })
                .unwrap();
                assert!(size.virtual_fs);
                assert_eq!(size.bytes, 0);
                assert_eq!(size.files, 0);
                assert_eq!(dir_size(target, &AtomicBool::new(true), &|_| {}), None);
            }
        }
    }

    #[test]
    fn ordinary_folders_named_proc_or_sys_still_count() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["proc", "sys"] {
            let path = temp.path().join(name);
            fs::create_dir(&path).unwrap();
            fs::write(path.join("data"), [0; 123]).unwrap();
            let size = dir_size(&path, &AtomicBool::new(false), &|_| {}).unwrap();
            assert_eq!(size.bytes, 123);
            assert!(!size.virtual_fs);
            assert!(serde_json::to_value(size)
                .unwrap()
                .get("virtualFs")
                .is_none());
        }
        let missing = temp.path().join("missing");
        let size = dir_size(&missing, &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(!size.virtual_fs);
        assert!(size.unreadable > 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn memory_backed_user_files_are_counted() {
        let Ok(temp) = tempfile::tempdir_in("/dev/shm") else {
            return;
        };
        fs::write(temp.path().join("data"), [0; 321]).unwrap();
        let size = dir_size(temp.path(), &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(!size.virtual_fs);
        assert_eq!(size.bytes, 321);
        assert_eq!(size.files, 1);
    }
}
