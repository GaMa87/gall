//! Windows refuses to rename, move or delete a folder while a folder inside it is open,
//! and a watched folder is held open. With the release hook installed (as the app does),
//! a folder whose subfolder is shown in the other browser can still be renamed and
//! trashed, and undo works too. On every OS, a browser left in a subfolder that went away
//! with the folder hears of it (Linux's inotify does not say when a folder above moves).

// Undo of the trash (restore) needs the trash listing: Windows and Linux.
#![cfg(any(windows, target_os = "linux"))]

use gall_core::fs::ops::{rename, set_release_hook, trash_items, UndoJournal};
use gall_core::fs::watcher::FolderWatcher;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

/// Folders reported as changed.
static CHANGED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static WATCHER: LazyLock<FolderWatcher> = LazyLock::new(|| {
    FolderWatcher::new(
        Duration::from_millis(50),
        Arc::new(|p, _| CHANGED.lock().unwrap().push(p.to_path_buf())),
    )
});

#[test]
fn folder_above_a_watched_one_can_be_renamed_and_trashed() {
    set_release_hook(|p| {
        let released = WATCHER.release_under(p);
        Box::new(move || WATCHER.resume(&released))
    });
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("A");
    std::fs::create_dir_all(a.join("B")).unwrap();
    std::fs::write(a.join("B/f.txt"), "x").unwrap();
    WATCHER.watch(&a.join("B")).unwrap();
    let j = UndoJournal::default();

    let a2 = rename(&a, "A2", &j).expect("rename the folder above a watched one");
    assert!(a2.join("B/f.txt").exists());
    assert!(
        CHANGED.lock().unwrap().contains(&a.join("B")),
        "the browser showing A/B hears that it is gone"
    );
    j.undo().expect("undo the rename");
    assert!(a.join("B/f.txt").exists());
    // The browser, back in A\B, watches it again.
    WATCHER.unwatch(&a.join("B"));
    WATCHER.watch(&a.join("B")).unwrap();
    assert_eq!(WATCHER.watched(), vec![a.join("B")]);

    trash_items(std::slice::from_ref(&a), &j).expect("trash the folder above a watched one");
    assert!(!a.exists());
    j.undo().expect("restore it");
    assert!(a.join("B/f.txt").exists());
    WATCHER.unwatch(&a.join("B"));
}
