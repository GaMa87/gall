//! Debounced folder watching (F-11) with an event-storm guard.

use crate::Result;
use notify::{
    event::{AccessKind, AccessMode},
    Event, EventKind, RecursiveMode,
};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Callback receives the watched folder whose contents changed, and whether
/// the change rate looks like a storm (UI should then throttle refreshes).
pub type ChangeFn = dyn Fn(&Path, bool) + Send + Sync + 'static;

struct Watched {
    /// None when watching failed (an unreadable folder): the reference still counts, so
    /// its unwatch pairs with it; a later watch tries again.
    debouncer: Option<Debouncer<notify::RecommendedWatcher, RecommendedCache>>,
    refs: usize,
}

#[derive(Default)]
struct State {
    watched: HashMap<PathBuf, Watched>,
    /// Unwatches that arrived before their watch. The UI's watch runs on a worker thread
    /// and its unwatch on the main thread, so while folders are browsed quickly the
    /// unwatch can come first; without this the late watch would stay forever (on
    /// Windows holding the folder open, so its drive could not be ejected).
    early_unwatch: HashMap<PathBuf, usize>,
}

/// Reference-counted, non-recursive watchers; one per visible folder.
pub struct FolderWatcher {
    inner: Mutex<State>,
    on_change: Arc<ChangeFn>,
    debounce: Duration,
}

/// More than this many debounced batches per second marks a storm.
const STORM_BATCHES_PER_SEC: usize = 8;

// Opening/reading a preview (or listing a folder) must not invalidate itself.
// Keep close-after-write: it also signals that an interrupted write is ready to retry.
fn changes_contents(event: &Event) -> bool {
    event.need_rescan()
        || match event.kind {
            EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
            EventKind::Access(_) => false,
            _ => true,
        }
}

impl FolderWatcher {
    pub fn new(debounce: Duration, on_change: Arc<ChangeFn>) -> Self {
        Self {
            inner: Mutex::new(State::default()),
            on_change,
            debounce,
        }
    }

    pub fn watch(&self, dir: &Path) -> Result<()> {
        let mut st = self.inner.lock().unwrap();
        if let Some(n) = st.early_unwatch.get_mut(dir) {
            // Already unwatched: this watch is over before it started.
            *n -= 1;
            if *n == 0 {
                st.early_unwatch.remove(dir);
            }
            return Ok(());
        }
        let w = st.watched.entry(dir.to_path_buf()).or_insert(Watched {
            debouncer: None,
            refs: 0,
        });
        w.refs += 1;
        if w.debouncer.is_some() {
            return Ok(());
        }
        let d = self.start(dir)?;
        w.debouncer = Some(d);
        Ok(())
    }

    fn start(&self, dir: &Path) -> Result<Debouncer<notify::RecommendedWatcher, RecommendedCache>> {
        let cb = self.on_change.clone();
        let folder = dir.to_path_buf();
        let window: Mutex<Vec<Instant>> = Mutex::new(Vec::new());
        let mut debouncer = new_debouncer(self.debounce, None, move |res: DebounceEventResult| {
            if let Ok(events) = res {
                if !events.iter().any(|event| changes_contents(event)) {
                    return;
                }
                let now = Instant::now();
                let storm = {
                    let mut w = window.lock().unwrap();
                    w.retain(|t| now.duration_since(*t) < Duration::from_secs(1));
                    w.push(now);
                    w.len() > STORM_BATCHES_PER_SEC
                };
                cb(&folder, storm);
            }
        })?;
        debouncer.watch(dir, RecursiveMode::NonRecursive)?;
        Ok(debouncer)
    }

    pub fn unwatch(&self, dir: &Path) {
        let mut st = self.inner.lock().unwrap();
        match st.watched.get_mut(dir) {
            Some(w) => {
                w.refs -= 1;
                if w.refs == 0 {
                    st.watched.remove(dir);
                }
            }
            None => *st.early_unwatch.entry(dir.to_path_buf()).or_default() += 1,
        }
    }

    /// Stops watching `root` and the watched folders inside it, and waits until their
    /// handles are closed; returns them for [`resume`](Self::resume). Windows keeps a
    /// watched folder open and then refuses to rename, move or delete any folder above
    /// it ("Access is denied"), e.g. a folder whose subfolder the other browser shows.
    pub fn release_under(&self, root: &Path) -> Vec<PathBuf> {
        let released: Vec<PathBuf> = {
            let mut st = self.inner.lock().unwrap();
            st.watched
                .iter_mut()
                .filter(|(p, w)| p.starts_with(root) && w.debouncer.is_some())
                .map(|(p, w)| {
                    w.debouncer = None;
                    p.clone()
                })
                .collect()
        };
        // notify closes the handles on its own thread a moment later.
        #[cfg(windows)]
        for d in &released {
            wait_until_closed(d);
        }
        released
    }

    /// Watches the folders of [`release_under`](Self::release_under) again, those still
    /// wanted and still there. One that is gone (it was inside the renamed or deleted
    /// folder) is reported as changed, so the browser showing it notices and leaves.
    pub fn resume(&self, dirs: &[PathBuf]) {
        let mut gone = Vec::new();
        {
            let mut st = self.inner.lock().unwrap();
            for d in dirs {
                if let Some(w) = st.watched.get_mut(d) {
                    if w.debouncer.is_some() {
                        continue;
                    }
                    if d.is_dir() {
                        w.debouncer = self.start(d).ok();
                    } else {
                        gone.push(d);
                    }
                }
            }
        }
        for d in gone {
            (self.on_change)(d, false);
        }
    }

    /// Folders being watched now (failed watches not included).
    pub fn watched(&self) -> Vec<PathBuf> {
        let st = self.inner.lock().unwrap();
        st.watched
            .iter()
            .filter(|(_, w)| w.debouncer.is_some())
            .map(|(p, _)| p.clone())
            .collect()
    }
}

/// Waits (up to half a second) until no handle with read access is open on folder `d`:
/// an open without sharing succeeds only then.
#[cfg(windows)]
fn wait_until_closed(d: &Path) {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let probe = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(d);
        match probe {
            Ok(_) => return,
            // Sharing violation: still open. Anything else (gone, no access): stop waiting.
            Err(e) if e.raw_os_error() == Some(32) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn detects_change_and_refcounts() {
        let d = tempfile::tempdir().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let w = FolderWatcher::new(
            Duration::from_millis(50),
            Arc::new(move |_, _| {
                h.fetch_add(1, Ordering::SeqCst);
            }),
        );
        w.watch(d.path()).unwrap();
        w.watch(d.path()).unwrap();
        std::fs::write(d.path().join("new.txt"), "x").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while hits.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(hits.load(Ordering::SeqCst) > 0);
        w.unwatch(d.path());
        assert_eq!(w.watched().len(), 1);
        w.unwatch(d.path());
        assert!(w.watched().is_empty());
    }

    #[test]
    fn detects_overwrite_and_atomic_replacement() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("image.png");
        std::fs::write(&path, b"A").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = FolderWatcher::new(
            Duration::from_millis(50),
            Arc::new(move |_, _| {
                let _ = tx.send(());
            }),
        );
        watcher.watch(d.path()).unwrap();
        std::fs::write(&path, b"B").unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let temp = d.path().join("temporary.png");
        std::fs::write(&temp, b"C").unwrap();
        // Windows rename does not overwrite an existing destination.
        #[cfg(windows)]
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&temp, &path).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"C");
        watcher.unwatch(d.path());
        assert!(watcher.watched().is_empty());
    }

    #[test]
    fn filters_read_access_but_keeps_completed_writes_and_renames() {
        use notify::event::{ModifyKind, RenameMode};
        for access in [
            AccessKind::Open(AccessMode::Any),
            AccessKind::Read,
            AccessKind::Close(AccessMode::Read),
            AccessKind::Any,
        ] {
            assert!(!changes_contents(&Event::new(EventKind::Access(access))));
        }
        assert!(changes_contents(&Event::new(EventKind::Access(
            AccessKind::Close(AccessMode::Write)
        ))));
        assert!(changes_contents(&Event::new(EventKind::Modify(
            ModifyKind::Name(RenameMode::Both)
        ))));
        assert!(changes_contents(&Event::new(EventKind::Remove(
            notify::event::RemoveKind::File
        ))));
    }

    #[test]
    fn reading_a_preview_does_not_trigger_another_refresh() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("image.png");
        std::fs::write(&path, b"image").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = FolderWatcher::new(
            Duration::from_millis(50),
            Arc::new(move |_, _| {
                let _ = tx.send(());
            }),
        );
        watcher.watch(d.path()).unwrap();
        // FSEvents on macOS can still deliver the creation of the file written just above
        // after the stream started; let that settle so only the read is measured.
        if cfg!(target_os = "macos") {
            while rx.recv_timeout(Duration::from_millis(1000)).is_ok() {}
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"image");
        assert!(
            rx.recv_timeout(Duration::from_millis(400)).is_err(),
            "reading the preview must not invalidate itself"
        );
        std::fs::write(&path, b"updated").unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        watcher.unwatch(d.path());
    }

    fn quiet() -> FolderWatcher {
        FolderWatcher::new(Duration::from_millis(50), Arc::new(|_, _| {}))
    }

    /// Quick browsing: the unwatch overtakes its watch. Nothing may stay watched.
    #[test]
    fn unwatch_before_watch_leaves_nothing_watched() {
        let d = tempfile::tempdir().unwrap();
        let w = quiet();
        w.unwatch(d.path());
        w.watch(d.path()).unwrap();
        assert!(w.watched().is_empty());
        // The next real visit is watched again.
        w.watch(d.path()).unwrap();
        assert_eq!(w.watched(), vec![d.path().to_path_buf()]);
        w.unwatch(d.path());
        assert!(w.watched().is_empty());
    }

    /// A folder whose subfolder is watched (shown in the other browser) can be renamed
    /// once the watchers inside are released; afterwards they are watched again if the
    /// folder is still there.
    #[test]
    fn released_watchers_let_the_folder_above_be_renamed() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("A");
        let b = a.join("B");
        std::fs::create_dir_all(&b).unwrap();
        let w = quiet();
        w.watch(&b).unwrap();
        let released = w.release_under(&a);
        assert_eq!(released, vec![b.clone()]);
        assert!(w.watched().is_empty());
        std::fs::rename(&a, d.path().join("A2")).unwrap();
        w.resume(&released);
        assert!(
            w.watched().is_empty(),
            "B moved away: not watched at its old path"
        );
        // Not moved: watched again.
        let c = d.path().join("C");
        std::fs::create_dir(&c).unwrap();
        w.watch(&c).unwrap();
        let released = w.release_under(d.path());
        w.resume(&released);
        assert_eq!(w.watched(), vec![c]);
    }

    /// A failed watch still pairs with its unwatch, and a later visit watches again.
    #[test]
    fn failed_watch_pairs_with_its_unwatch() {
        let d = tempfile::tempdir().unwrap();
        let later = d.path().join("later");
        let w = quiet();
        assert!(w.watch(&later).is_err());
        w.unwatch(&later);
        std::fs::create_dir(&later).unwrap();
        w.watch(&later).unwrap();
        assert_eq!(w.watched(), vec![later.clone()]);
    }
}
