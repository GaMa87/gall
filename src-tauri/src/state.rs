//! Application state shared by all commands.

use gall_core::fs::index::FileIndex;
use gall_core::fs::ops::{TaskManager, UndoJournal};
use gall_core::fs::watcher::FolderWatcher;
use gall_core::nav::Session;
use gall_core::storage::json_store::LoadedFrom;
use gall_core::storage::{Db, Favorites, JsonStore, Marks, Settings};
use gall_core::util::appdirs::AppDirs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// Files placed on Gall's clipboard with Ctrl+C / Ctrl+X.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileClipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

pub struct AppState {
    pub dirs: AppDirs,
    pub settings: RwLock<Settings>,
    pub settings_store: JsonStore,
    pub favorites: Mutex<Favorites>,
    pub favorites_store: JsonStore,
    /// Vim folder marks (`m{letter}` / `'{letter}`).
    pub marks: Mutex<Marks>,
    pub marks_store: JsonStore,
    pub session: Mutex<Session>,
    pub session_store: JsonStore,
    pub db: Db,
    pub tasks: Arc<TaskManager>,
    pub watcher: FolderWatcher,
    pub search_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub grep_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub index: Arc<FileIndex>,
    /// Quick Open in a folder the system index does not hold (WSL, a network share):
    /// a separate index of just that folder, scanned on demand.
    pub folder_scan: Mutex<Option<FolderScan>>,
    pub clipboard: Mutex<FileClipboard>,
    /// Message shown once at startup (e.g. settings restored from backup).
    pub startup_notice: Mutex<Option<String>>,
    /// No settings file yet: the UI shows the welcome setup.
    pub first_run: bool,
    /// Sidebar system monitor; created on first use (keeps the previous sample for deltas).
    pub monitor: Arc<Mutex<Option<gall_core::sysmon::Monitor>>>,
    /// Cancels the running folder comparison or sync.
    pub dircmp_cancel: Mutex<Option<Arc<AtomicBool>>>,
}

pub struct FolderScan {
    pub root: std::path::PathBuf,
    pub index: Arc<FileIndex>,
    pub cancel: Arc<AtomicBool>,
    /// The settings it was built with (see `commands::scan_key`).
    pub key: String,
}

pub fn home() -> String {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into())
}

/// Root of the file system: `/`, or the Windows system drive (`C:\`).
pub fn system_root() -> String {
    if cfg!(windows) {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        format!("{}\\", drive.trim_end_matches('\\'))
    } else {
        "/".into()
    }
}

impl AppState {
    pub fn load(dirs: AppDirs, tasks: Arc<TaskManager>, watcher: FolderWatcher) -> Self {
        let settings_store = JsonStore::new(dirs.settings());
        let (mut settings, from): (Settings, _) = settings_store.load();
        let first_run = from == LoadedFrom::Default;
        // 0.1.0 remembered 30 folders by default; the default is now 10. Only the old
        // default is migrated, a different value the user picked is kept.
        if settings.history_count == 30 {
            settings.history_count = 10;
            let _ = settings_store.save(&settings);
        }
        // 0.1.2 and before squeezed Miller columns down to 70 px, too narrow for a name
        // (deep folders showed "C…" "Lo…"); now 120 px, and deeper paths scroll.
        if settings.column_min_width == 70 {
            settings.column_min_width = 120;
            let _ = settings_store.save(&settings);
        }
        let mut notice = None;
        if from == LoadedFrom::Backup {
            notice =
                Some("Settings file was damaged and has been restored from backup.".to_string());
        }

        let favorites_store = JsonStore::new(dirs.favorites());
        let (mut favorites, ffrom): (Favorites, _) = favorites_store.load();
        if ffrom == LoadedFrom::Default {
            favorites = Favorites::with_defaults();
            let _ = favorites_store.save(&favorites);
        }
        favorites.sanitize();

        let marks_store = JsonStore::new(dirs.marks());
        let (mut marks, _): (Marks, _) = marks_store.load();
        marks.sanitize();

        let session_store = JsonStore::new(dirs.session());
        let (session, _): (Option<Session>, _) = session_store.load();
        let h = home();
        let mut session = session.unwrap_or_else(|| {
            // First start: left browser at the file-system root, right at home
            // (unless start-up folders are configured).
            let start = settings
                .startup_paths
                .first()
                .map(|p| gall_core::nav::path::expand(p))
                .unwrap_or_else(system_root);
            let second = settings
                .startup_paths
                .get(1)
                .map(|p| gall_core::nav::path::expand(p))
                .unwrap_or_else(|| h.clone());
            Session::new(&start, &second)
        });
        session.sanitize(&h);

        let db = Db::open(&dirs.database()).unwrap_or_else(|e| {
            log::error!("cannot open database ({e}); using in-memory store");
            Db::open_in_memory().expect("in-memory sqlite")
        });
        if let Err(e) = gall_core::util::templates::ensure_seeded(&dirs.templates()) {
            log::warn!("templates: {e}");
        }

        AppState {
            dirs,
            settings: RwLock::new(settings),
            settings_store,
            favorites: Mutex::new(favorites),
            favorites_store,
            marks: Mutex::new(marks),
            marks_store,
            session: Mutex::new(session),
            session_store,
            db,
            tasks,
            watcher,
            search_cancel: Mutex::new(None),
            grep_cancel: Mutex::new(None),
            index: Arc::new(FileIndex::new()),
            folder_scan: Mutex::new(None),
            clipboard: Mutex::new(FileClipboard::default()),
            startup_notice: Mutex::new(notice),
            first_run,
            monitor: Arc::new(Mutex::new(None)),
            dircmp_cancel: Mutex::new(None),
        }
    }

    pub fn journal(&self) -> &Arc<UndoJournal> {
        self.tasks.journal()
    }

    /// Saves the session without making the caller wait. Writing it means a sync to
    /// disk, a re-read and a backup copy, and this runs on every navigation before the
    /// folder is even asked for; on a slow disk that was most of the wait. The state is
    /// handed to a writer thread, which always writes the newest one it was given.
    pub fn save_session(&self) {
        // The number is taken while the session is locked, so a later state never gets
        // the lower one.
        let (s, generation) = {
            let session = self.session.lock().unwrap();
            let generation = SESSION_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            (session.clone(), generation)
        };
        let sender = SESSION_WRITER.get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<(u64, Session)>();
            let store = JsonStore::new(self.session_store.path().to_path_buf());
            std::thread::spawn(move || {
                while let Ok(mut newest) = rx.recv() {
                    // Several navigations in a row: only the last state is written.
                    while let Ok(later) = rx.try_recv() {
                        newest = later;
                    }
                    write_session(&store, newest.0, &newest.1);
                }
            });
            std::sync::Mutex::new(tx)
        });
        let sent = sender
            .lock()
            .is_ok_and(|tx| tx.send((generation, s.clone())).is_ok());
        if !sent {
            write_session(&self.session_store, generation, &s);
        }
    }

    /// Writes the current session and waits for it: before the app exits.
    pub fn flush_session(&self) {
        // The number is taken while the session is locked, so a later state never gets
        // the lower one.
        let (s, generation) = {
            let session = self.session.lock().unwrap();
            let generation = SESSION_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            (session.clone(), generation)
        };
        write_session(&self.session_store, generation, &s);
    }

    pub fn save_favorites(&self) {
        let f = self.favorites.lock().unwrap().clone();
        if let Err(e) = self.favorites_store.save(&f) {
            log::error!("saving favorites: {e}");
        }
    }

    pub fn save_marks(&self, m: &Marks) -> gall_core::error::Result<()> {
        self.marks_store.save(m)
    }
}

/// Counts the session states handed to [`AppState::save_session`].
static SESSION_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The channel to the thread that writes the session.
static SESSION_WRITER: std::sync::OnceLock<
    std::sync::Mutex<std::sync::mpsc::Sender<(u64, Session)>>,
> = std::sync::OnceLock::new();
/// The generation of the session state on disk. Held while writing, so two writers never
/// write at once and an older state never replaces a newer one.
static SESSION_WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

fn write_session(store: &JsonStore, generation: u64, session: &Session) {
    let mut written = SESSION_WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    if generation <= *written {
        return;
    }
    match store.save(session) {
        Ok(()) => *written = generation,
        Err(e) => log::error!("saving session: {e}"),
    }
}
