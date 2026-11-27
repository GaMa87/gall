//! Application state shared by all commands and windows.

use crate::cli::CliArgs;
use crate::filemanager_dbus::FileManagerAction;
use crate::omarchy::OmarchyBridge;
use gall_core::fs::index::FileIndex;
use gall_core::fs::ops::{TaskManager, UndoJournal};
use gall_core::fs::watcher::FolderWatcher;
use gall_core::handoff::{self, Outcome};
use gall_core::nav::{Session, StoredSession, WorkspaceSnapshot, WorkspaceStore};
use gall_core::storage::json_store::LoadedFrom;
use gall_core::storage::{Db, Favorites, JsonStore, Marks, Settings};
use gall_core::util::appdirs::AppDirs;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::{AppHandle, Manager};

/// Files placed on Gall's clipboard with Ctrl+C / Ctrl+X.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileClipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

/// Synchronized window readiness and pending delivery queues. Keeping all three under
/// a single mutex prevents TOCTOU races between `window_ready` / `*_take_pending` and
/// concurrent D-Bus / single-instance dispatches.
#[derive(Default)]
struct WindowDeliveryState {
    ready: HashSet<String>,
    cli_pending: HashMap<String, Vec<CliArgs>>,
    fm_pending: HashMap<String, Vec<FileManagerAction>>,
}

enum CompareWaitCompletion {
    Pending,
    NeedsReceipt(Outcome),
    Finished,
    Withdrawn,
}

/// A waiting comparison is tied to exactly one native window. Its CLI-selected
/// output path never comes back from the webview as a command argument.
pub(crate) struct CompareWaitWindow {
    pub result: PathBuf,
    pub output: Option<PathBuf>,
    completion: Mutex<CompareWaitCompletion>,
}

impl CompareWaitWindow {
    pub fn from_args(args: &CliArgs) -> Option<Self> {
        Some(Self {
            result: args.handoff.clone()?,
            output: args
                .merge
                .as_ref()
                .map(|merge| PathBuf::from(&merge.output)),
            completion: Mutex::new(CompareWaitCompletion::Pending),
        })
    }

    pub fn mark_started(&self) -> std::io::Result<()> {
        let completion = self.completion.lock().unwrap();
        if matches!(*completion, CompareWaitCompletion::Pending)
            && !handoff::cancel_marker(&self.result).exists()
        {
            crate::compare_wait::mark_started(&self.result)?;
        }
        Ok(())
    }

    /// Reserve completion before touching the merge output. The caller reserves
    /// the same gate when withdrawing the request, before it creates its marker.
    pub fn finish(&self, outcome: impl FnOnce() -> Outcome) -> std::io::Result<bool> {
        let mut completion = self.completion.lock().unwrap();
        if matches!(*completion, CompareWaitCompletion::Withdrawn) {
            return Ok(false);
        }
        if matches!(*completion, CompareWaitCompletion::Pending) {
            if handoff::cancel_marker(&self.result).exists()
                || !crate::compare_wait::reserve_result(&self.result)?
            {
                return Ok(false);
            }
            *completion = CompareWaitCompletion::NeedsReceipt(outcome());
        }
        if let CompareWaitCompletion::NeedsReceipt(outcome) = &*completion {
            // Keep the actual outcome on failure. Retrying the receipt must
            // neither repeat the save nor turn an accepted merge into a cancel.
            handoff::write_outcome(&self.result, outcome)?;
            *completion = CompareWaitCompletion::Finished;
        }
        Ok(true)
    }

    pub fn withdraw(&self) -> bool {
        let mut completion = self.completion.lock().unwrap();
        if !matches!(*completion, CompareWaitCompletion::Pending)
            || !handoff::cancel_marker(&self.result).exists()
        {
            return false;
        }
        // Cancellation already owns the on-disk gate, so no completion can
        // modify the merge output or write a competing result.
        *completion = CompareWaitCompletion::Withdrawn;
        crate::compare_wait::clean_cancelled(&self.result);
        true
    }
}

pub(crate) struct CompareJob {
    pub id: u64,
    pub left: gall_core::fs::diff::DiffSource,
    pub right: gall_core::fs::diff::DiffSource,
    pub options: gall_core::fs::diff::DiffOptions,
    pub reply: tauri::async_runtime::Sender<Result<serde_json::Value, String>>,
}

#[derive(Default)]
pub(crate) struct CompareSlot {
    latest: u64,
    running: Option<Arc<AtomicBool>>,
    pending: Option<CompareJob>,
    pub result: Option<(u64, gall_core::fs::diff::PagedDiff)>,
}

impl CompareSlot {
    /// Reserve the only worker, or replace the pending request while it exits.
    pub fn submit(&mut self, job: CompareJob) -> bool {
        if job.id <= self.latest {
            return false;
        }
        self.latest = job.id;
        self.result = None;
        self.pending = Some(job);
        if let Some(cancel) = &self.running {
            cancel.store(true, Ordering::Relaxed);
            false
        } else {
            self.running = Some(Arc::new(AtomicBool::new(false)));
            true
        }
    }

    pub fn take(&mut self) -> Option<(CompareJob, Arc<AtomicBool>)> {
        match self.pending.take() {
            Some(job) => {
                let cancel = Arc::new(AtomicBool::new(false));
                self.running = Some(cancel.clone());
                Some((job, cancel))
            }
            None => {
                self.running = None;
                None
            }
        }
    }

    pub fn accepts(&self, id: u64, cancel: &AtomicBool) -> bool {
        id == self.latest && !cancel.load(Ordering::Relaxed)
    }

    pub fn close(&mut self, through: u64) {
        if through < self.latest {
            return;
        }
        self.latest = through;
        self.result = None;
        self.pending = None;
        if let Some(cancel) = &self.running {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Last `(window, folder, selection, focused)` written to `selection.json`, to skip no-op writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedSelection {
    pub window: String,
    pub folder: String,
    pub selection: Vec<String>,
    pub focused: Option<String>,
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
    /// Per-window workspace stores keyed by WebviewWindow label (`"main"`, `"win-1"`, ...).
    /// Only `"main"` persists to `session.json`; secondary windows keep isolated in-memory stores.
    pub sessions: Mutex<HashMap<String, WorkspaceStore>>,
    pub session_store: JsonStore,
    /// Initial `CliArgs` associated with each window label.
    pub window_cli_initial: Mutex<HashMap<String, CliArgs>>,
    compare_wait_windows: Mutex<HashMap<String, Arc<CompareWaitWindow>>>,
    /// Synchronized window readiness + pending CLI & D-Bus FileManager1 queues.
    window_delivery: Mutex<WindowDeliveryState>,
    /// Window labels that have already been shown (via `window_ready` or 4s fallback).
    pub revealed_windows: Mutex<HashSet<String>>,
    /// Window labels created with `.transparent(true)` (Linux Omarchy Glassy).
    transparent_windows: Mutex<HashSet<String>>,
    /// Picker windows whose result has been written (so closing the window afterwards is no cancel).
    pub pick_done: Mutex<HashSet<String>>,
    /// True while the `"main"` label belongs to a picker (a cold start for `gall --pick`): its
    /// session is the picker's, so it must never be written to `session.json`.
    pub main_is_picker: AtomicBool,
    /// Most recently focused or created window label.
    pub active_window: Mutex<Option<String>>,
    /// Serializes cold-launch window allocation/creation and target-resolution decisions
    /// so concurrent `FileManager1` and `single-instance` launches cannot both observe
    /// zero windows and allocate duplicate `"main"` labels.
    ///
    /// Lock ordering: NEVER acquire `window_open_lock` inside `remove_window`,
    /// `window_ready`, `reveal_window`, or `Focused`/`Destroyed` window event handlers.
    pub window_open_lock: Arc<Mutex<()>>,
    /// Monotonic counter for secondary window labels (`"win-1"`, `"win-2"`, ...).
    pub next_window_id: AtomicU64,
    /// Generation of the window currently registered under each label. A `Destroyed` event of an
    /// earlier window with the same label carries an older generation and must not clean up.
    window_generation: Mutex<HashMap<String, u64>>,
    next_generation: AtomicU64,
    /// Keep process running without windows when launched with `--daemon`.
    pub daemon_mode: AtomicBool,
    /// Live Omarchy desktop bridge (inert when inactive/non-Linux).
    pub omarchy: OmarchyBridge,
    pub db: Db,
    pub tasks: Arc<TaskManager>,
    pub watcher: FolderWatcher,
    pub search_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub grep_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub quick_hidden_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub index: Arc<FileIndex>,
    /// Quick Open in a folder the system index does not hold (WSL, a network share):
    /// a separate index of just that folder, scanned on demand.
    pub folder_scan: Mutex<Option<FolderScan>>,
    /// Quick Open with `Alt+I` (filters disabled): cached unfiltered scan of one folder
    /// with per-entry `hidden` flags, built on demand.
    pub everything_scan: Mutex<Option<FolderScan>>,
    pub clipboard: Mutex<FileClipboard>,
    /// No settings file yet: the UI shows the welcome setup.
    pub first_run: bool,
    /// Sidebar system monitor; created on first use (keeps the previous sample for deltas).
    pub monitor: Arc<Mutex<Option<gall_core::sysmon::Monitor>>>,
    /// Cancels the running folder comparison or sync.
    pub(crate) compare_slots: Mutex<HashMap<String, CompareSlot>>,
    pub dircmp_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// Last published selection snapshot written to `selection.json`, to skip no-op writes.
    pub last_selection: Mutex<Option<PublishedSelection>>,
}

pub struct FolderScan {
    pub root: PathBuf,
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

fn default_session_with(settings: &Settings, init_args: Option<&CliArgs>) -> Session {
    let h = home();
    let start = init_args
        .and_then(|a| a.path1.clone().or_else(|| a.paths.first().cloned()))
        .or_else(|| {
            settings
                .startup_paths
                .first()
                .map(|p| gall_core::nav::path::expand(p))
        })
        .unwrap_or_else(system_root);
    let second = init_args
        .and_then(|a| a.path2.clone().or_else(|| a.paths.get(1).cloned()))
        .or_else(|| {
            settings
                .startup_paths
                .get(1)
                .map(|p| gall_core::nav::path::expand(p))
        })
        .unwrap_or_else(|| h.clone());
    let mut session = Session::new(&start, &second);
    session.sanitize(&h);
    session
}

impl AppState {
    pub fn load(
        dirs: AppDirs,
        tasks: Arc<TaskManager>,
        watcher: FolderWatcher,
        mut settings: Settings,
        from: LoadedFrom,
        omarchy: OmarchyBridge,
    ) -> Self {
        let settings_store = JsonStore::new(dirs.settings());
        let first_run = from == LoadedFrom::Default;
        // Old defaults move to the current ones once; later the same values are choices.
        if settings.migrate_defaults() {
            let _ = settings_store.save(&settings);
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
        let (stored, _): (Option<StoredSession>, _) = session_store.load();
        let h = home();
        let main_workspaces = match stored {
            Some(s) => s.into_store(&h),
            None => {
                let mut ws = WorkspaceStore::from_session(default_session_with(&settings, None));
                ws.sanitize(&h);
                ws
            }
        };

        let mut sessions = HashMap::new();
        sessions.insert("main".to_string(), main_workspaces);

        let db = Db::open(&dirs.database()).unwrap_or_else(|e| {
            log::warn!(
                "Could not open the database. Folder preferences and tags may not be saved. {e}"
            );
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
            sessions: Mutex::new(sessions),
            session_store,
            window_cli_initial: Mutex::new(HashMap::new()),
            compare_wait_windows: Mutex::new(HashMap::new()),
            window_delivery: Mutex::new(WindowDeliveryState::default()),
            revealed_windows: Mutex::new(HashSet::new()),
            transparent_windows: Mutex::new(HashSet::new()),
            pick_done: Mutex::new(HashSet::new()),
            main_is_picker: AtomicBool::new(false),
            active_window: Mutex::new(None),
            window_open_lock: Arc::new(Mutex::new(())),
            next_window_id: AtomicU64::new(1),
            window_generation: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
            daemon_mode: AtomicBool::new(false),
            omarchy,
            db,
            tasks,
            watcher,
            search_cancel: Mutex::new(None),
            grep_cancel: Mutex::new(None),
            quick_hidden_cancel: Mutex::new(None),
            index: Arc::new(FileIndex::new()),
            folder_scan: Mutex::new(None),
            everything_scan: Mutex::new(None),
            clipboard: Mutex::new(FileClipboard::default()),
            first_run,
            monitor: Arc::new(Mutex::new(None)),
            compare_slots: Mutex::new(HashMap::new()),
            dircmp_cancel: Mutex::new(None),
            last_selection: Mutex::new(None),
        }
    }

    pub fn journal(&self) -> &Arc<UndoJournal> {
        self.tasks.journal()
    }

    pub fn lock_window_open(&self) -> std::sync::MutexGuard<'_, ()> {
        self.window_open_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn allocate_window_label(&self) -> String {
        let id = self.next_window_id.fetch_add(1, Ordering::Relaxed);
        format!("win-{id}")
    }

    /// A label for a picker window: never `"main"`, so a picker cannot share state with (or be
    /// confused by the teardown of) the primary window.
    pub fn allocate_pick_label(&self) -> String {
        let id = self.next_window_id.fetch_add(1, Ordering::Relaxed);
        format!("pick-{id}")
    }

    /// Registers a new window under `label` and returns its generation token.
    pub fn begin_window(&self, label: &str) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        self.window_generation
            .lock()
            .unwrap()
            .insert(label.to_string(), generation);
        generation
    }

    /// The generation of the window now registered under `label`.
    pub fn window_generation(&self, label: &str) -> Option<u64> {
        self.window_generation.lock().unwrap().get(label).copied()
    }

    /// Tears down `label` only when `generation` is still the registered one; a late event from a
    /// window that was replaced under the same label changes nothing. Returns whether it ran.
    pub fn remove_window_generation(&self, label: &str, generation: u64) -> bool {
        if self.window_generation(label) != Some(generation) {
            return false;
        }
        self.remove_window(label);
        true
    }

    /// Selects the window label for a new window (`label_override`, `"main"` when no windows
    /// exist yet, or `"win-<N>"`). Callers must hold [`Self::lock_window_open`] across this
    /// decision and window registration (`WebviewWindowBuilder::build`).
    pub fn choose_window_label(
        &self,
        label_override: Option<&str>,
        has_window: impl Fn(&str) -> bool,
        has_any_window: impl FnOnce() -> bool,
    ) -> String {
        match label_override {
            Some(l) if !has_window(l) => l.to_string(),
            _ if !has_window("main") && !has_any_window() => "main".to_string(),
            _ => self.allocate_window_label(),
        }
    }

    /// Ensures a workspace store exists for `label`, creating a fresh one from `init_args` or
    /// settings defaults if missing, and returns its active [`Session`].
    pub fn ensure_session(&self, label: &str, init_args: Option<&CliArgs>) -> Session {
        let settings = self.settings.read().unwrap().clone();
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(existing) = sessions.get(label) {
            return existing.active_session().clone();
        }
        let created = WorkspaceStore::from_session(default_session_with(&settings, init_args));
        let active = created.active_session().clone();
        sessions.insert(label.to_string(), created);
        active
    }

    pub fn session_for(&self, label: &str) -> Session {
        if let Some(ws) = self.sessions.lock().unwrap().get(label) {
            return ws.active_session().clone();
        }
        self.ensure_session(label, None)
    }

    pub fn workspace_snapshot_for(&self, label: &str) -> WorkspaceSnapshot {
        {
            let sessions = self.sessions.lock().unwrap();
            if let Some(ws) = sessions.get(label) {
                return ws.snapshot();
            }
        }
        let settings = self.settings.read().unwrap().clone();
        let mut sessions = self.sessions.lock().unwrap();
        let ws = sessions
            .entry(label.to_string())
            .or_insert_with(|| WorkspaceStore::from_session(default_session_with(&settings, None)));
        ws.snapshot()
    }

    pub fn with_workspaces_mut<F, R>(&self, label: &str, f: F) -> R
    where
        F: FnOnce(&mut WorkspaceStore) -> R,
    {
        let settings = self.settings.read().unwrap().clone();
        let mut sessions = self.sessions.lock().unwrap();
        let ws = sessions
            .entry(label.to_string())
            .or_insert_with(|| WorkspaceStore::from_session(default_session_with(&settings, None)));
        f(ws)
    }

    #[cfg(test)]
    pub fn with_session_mut<F, R>(&self, label: &str, f: F) -> R
    where
        F: FnOnce(&mut Session) -> R,
    {
        self.with_workspaces_mut(label, |ws| f(ws.active_session_mut()))
    }

    pub fn with_session_for_mut<F, R>(
        &self,
        label: &str,
        workspace_id: Option<u64>,
        f: F,
    ) -> Option<R>
    where
        F: FnOnce(&mut Session) -> R,
    {
        self.with_workspaces_mut(label, |ws| match workspace_id {
            Some(id) => ws.session_mut_for_id(id).map(f),
            None => Some(f(ws.active_session_mut())),
        })
    }

    /// Saves the primary `"main"` workspace store without making the caller wait.
    /// Secondary windows (`"win-*"`) keep independent in-memory stores and never
    /// overwrite `"main"`'s persisted `session.json`.
    pub fn save_session_for(&self, label: &str) {
        if label != "main" || self.main_is_picker.load(Ordering::Acquire) {
            return;
        }
        let (ws, generation) = {
            let sessions = self.sessions.lock().unwrap();
            let Some(store) = sessions.get("main") else {
                return;
            };
            let generation = SESSION_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            (store.clone(), generation)
        };
        let sender = SESSION_WRITER.get_or_init(|| {
            const SESSION_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(3);
            let (tx, rx) = std::sync::mpsc::channel::<(u64, WorkspaceStore)>();
            let store = JsonStore::new(self.session_store.path().to_path_buf());
            std::thread::spawn(move || {
                while let Ok(mut newest) = rx.recv() {
                    // Coalesce bursts of navigation/focus changes into a single fsynced write;
                    // `flush_session()` still persists synchronously on clean exit or restart.
                    let deadline = std::time::Instant::now() + SESSION_DEBOUNCE;
                    loop {
                        let remaining =
                            deadline.saturating_duration_since(std::time::Instant::now());
                        if remaining.is_zero() {
                            break;
                        }
                        match rx.recv_timeout(remaining) {
                            Ok(later) => newest = later,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                            | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                    while let Ok(later) = rx.try_recv() {
                        newest = later;
                    }
                    write_session(&store, newest.0, &newest.1);
                }
            });
            Mutex::new(tx)
        });
        let sent = sender
            .lock()
            .is_ok_and(|tx| tx.send((generation, ws.clone())).is_ok());
        if !sent {
            write_session(&self.session_store, generation, &ws);
        }
    }

    /// Writes the `"main"` workspace store and waits for it: before the app exits.
    pub fn flush_session(&self) {
        if self.main_is_picker.load(Ordering::Acquire) {
            return;
        }
        let (ws, generation) = {
            let sessions = self.sessions.lock().unwrap();
            let Some(store) = sessions.get("main") else {
                return;
            };
            let generation = SESSION_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            (store.clone(), generation)
        };
        write_session(&self.session_store, generation, &ws);
    }

    pub fn set_window_initial_cli(&self, label: &str, args: CliArgs) {
        if label == "main" {
            self.main_is_picker
                .store(args.pick.is_some(), Ordering::Release);
        }
        self.window_delivery.lock().unwrap().ready.remove(label);
        if let Some(wait) = CompareWaitWindow::from_args(&args) {
            self.compare_wait_windows
                .lock()
                .unwrap()
                .insert(label.to_string(), Arc::new(wait));
        }
        self.window_cli_initial
            .lock()
            .unwrap()
            .insert(label.to_string(), args);
    }

    pub fn get_window_initial_cli(&self, label: &str) -> Option<CliArgs> {
        self.window_cli_initial.lock().unwrap().get(label).cloned()
    }

    /// The picker request the window was opened with, if it is a picker.
    pub fn window_pick(&self, label: &str) -> Option<crate::cli::PickArgs> {
        self.get_window_initial_cli(label).and_then(|a| a.pick)
    }

    /// Records that the picker of `label` has answered; false when it already had.
    pub fn mark_pick_done(&self, label: &str) -> bool {
        self.pick_done.lock().unwrap().insert(label.to_string())
    }

    pub fn mark_window_ready(&self, label: &str) {
        self.window_delivery
            .lock()
            .unwrap()
            .ready
            .insert(label.to_string());
    }

    pub fn window_is_ready(&self, label: &str) -> bool {
        self.window_delivery.lock().unwrap().ready.contains(label)
    }

    pub fn compare_wait_window(&self, label: &str) -> Option<Arc<CompareWaitWindow>> {
        self.compare_wait_windows
            .lock()
            .unwrap()
            .get(label)
            .cloned()
    }

    pub fn compare_wait_windows(&self) -> Vec<(String, Arc<CompareWaitWindow>)> {
        self.compare_wait_windows
            .lock()
            .unwrap()
            .iter()
            .map(|(label, wait)| (label.clone(), wait.clone()))
            .collect()
    }

    fn is_ordinary_window(&self, label: &str) -> bool {
        !self
            .get_window_initial_cli(label)
            .is_some_and(|args| args.pick.is_some() || args.handoff.is_some())
    }

    /// Returns `true` on the first call for `label` (until `remove_window`), preventing
    /// the 4-second fallback timer from re-focusing an already revealed window.
    pub fn mark_window_revealed(&self, label: &str) -> bool {
        self.revealed_windows
            .lock()
            .unwrap()
            .insert(label.to_string())
    }

    pub fn set_window_transparent(&self, label: &str, transparent: bool) {
        let mut set = self.transparent_windows.lock().unwrap();
        if transparent {
            set.insert(label.to_string());
        } else {
            set.remove(label);
        }
    }

    pub fn window_is_transparent(&self, label: &str) -> bool {
        self.transparent_windows.lock().unwrap().contains(label)
    }

    /// Atomically checks window readiness and either returns `true` (window is already ready
    /// to receive live events) or enqueues `args` into `cli_pending` under the same lock
    /// and returns `false`.
    pub fn dispatch_cli_or_enqueue(&self, label: &str, args: CliArgs) -> bool {
        let mut delivery = self.window_delivery.lock().unwrap();
        if delivery.ready.contains(label) {
            return true;
        }
        delivery
            .cli_pending
            .entry(label.to_string())
            .or_default()
            .push(args);
        false
    }

    pub fn take_pending_cli(&self, label: &str) -> Vec<CliArgs> {
        self.window_delivery
            .lock()
            .unwrap()
            .cli_pending
            .remove(label)
            .unwrap_or_default()
    }

    /// Atomically checks window readiness and either returns `true` (window is already ready
    /// to receive live events) or enqueues both `cli_args` and `action` under the same lock
    /// and returns `false`.
    #[cfg(any(target_os = "linux", test))]
    pub fn dispatch_fm_or_enqueue(
        &self,
        label: &str,
        cli_args: CliArgs,
        action: FileManagerAction,
    ) -> bool {
        let mut delivery = self.window_delivery.lock().unwrap();
        if delivery.ready.contains(label) {
            return true;
        }
        delivery
            .cli_pending
            .entry(label.to_string())
            .or_default()
            .push(cli_args);
        delivery
            .fm_pending
            .entry(label.to_string())
            .or_default()
            .push(action);
        false
    }

    /// Seeds a `FileManagerAction` for a brand-new window before its WebView is built,
    /// clearing any stale readiness for `label`.
    #[cfg(any(target_os = "linux", test))]
    pub fn seed_pending_fm(&self, label: &str, action: FileManagerAction) {
        let mut delivery = self.window_delivery.lock().unwrap();
        delivery.ready.remove(label);
        delivery
            .fm_pending
            .entry(label.to_string())
            .or_default()
            .push(action);
    }

    pub fn take_pending_fm(&self, label: &str) -> Vec<FileManagerAction> {
        self.window_delivery
            .lock()
            .unwrap()
            .fm_pending
            .remove(label)
            .unwrap_or_default()
    }

    pub fn set_active_window(&self, label: &str) {
        *self.active_window.lock().unwrap() = Some(label.to_string());
    }

    pub fn remove_window(&self, label: &str) {
        {
            let mut slots = self.compare_slots.lock().unwrap();
            self.window_generation.lock().unwrap().remove(label);
            if let Some(mut slot) = slots.remove(label) {
                slot.close(u64::MAX);
            }
        }
        {
            let mut active = self.active_window.lock().unwrap();
            if active.as_deref() == Some(label) {
                *active = None;
            }
        }
        {
            let mut delivery = self.window_delivery.lock().unwrap();
            delivery.ready.remove(label);
            delivery.cli_pending.remove(label);
            delivery.fm_pending.remove(label);
        }
        self.revealed_windows.lock().unwrap().remove(label);
        self.transparent_windows.lock().unwrap().remove(label);
        self.pick_done.lock().unwrap().remove(label);
        self.compare_wait_windows.lock().unwrap().remove(label);
        self.window_cli_initial.lock().unwrap().remove(label);
        if label != "main" {
            self.sessions.lock().unwrap().remove(label);
        } else if self.main_is_picker.load(Ordering::Acquire) {
            // The picker's throwaway session must not become the next normal window's session.
            self.sessions.lock().unwrap().remove(label);
        }
    }

    /// The window an ordinary launch request (`gall /dir`, FileManager1, reveal) goes to: the active
    /// one, else "main", else any. Picker and waiting compare windows belong to their callers.
    pub fn resolve_target_window(&self, app: &AppHandle) -> Option<String> {
        let usable =
            |label: &str| app.get_webview_window(label).is_some() && self.is_ordinary_window(label);
        if let Some(active) = self.active_window.lock().unwrap().clone() {
            if usable(&active) {
                return Some(active);
            }
        }
        if usable("main") {
            return Some("main".to_string());
        }
        app.webview_windows().keys().find(|l| usable(l)).cloned()
    }

    pub fn save_favorites(&self) {
        let f = self.favorites.lock().unwrap().clone();
        if let Err(e) = self.favorites_store.save(&f) {
            log_save_failure("favorites", e);
        }
    }

    pub fn save_marks(&self, m: &Marks) -> gall_core::error::Result<()> {
        self.marks_store.save(m)
    }
}

/// Counts the session states handed to [`AppState::save_session_for`].
static SESSION_GENERATION: AtomicU64 = AtomicU64::new(0);
/// The channel to the thread that writes the session.
static SESSION_WRITER: std::sync::OnceLock<Mutex<std::sync::mpsc::Sender<(u64, WorkspaceStore)>>> =
    std::sync::OnceLock::new();
/// The generation of the session state on disk. Held while writing, so two writers never
/// write at once and an older state never replaces a newer one.
static SESSION_WRITTEN: Mutex<u64> = Mutex::new(0);

fn write_session(store: &JsonStore, generation: u64, workspaces: &WorkspaceStore) {
    let mut written = SESSION_WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    if generation <= *written {
        return;
    }
    match store.save(workspaces) {
        Ok(()) => *written = generation,
        Err(e) => log_save_failure("the session", e),
    }
}

fn log_save_failure(label: &str, error: gall_core::CoreError) {
    let level = if matches!(&error, gall_core::CoreError::Io { .. }) {
        log::Level::Warn
    } else {
        log::Level::Error
    };
    log::log!(level, "Could not save {label}. {error}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn compare_slot_replaces_pending_work_and_rejects_stale_results() {
        use gall_core::fs::diff::DiffSource;

        fn job(id: u64) -> CompareJob {
            let source = DiffSource::Text {
                label: "input".into(),
                text: String::new(),
            };
            let (reply, _) = tauri::async_runtime::channel(1);
            CompareJob {
                id,
                left: source.clone(),
                right: source,
                options: Default::default(),
                reply,
            }
        }

        let mut slot = CompareSlot::default();
        assert!(slot.submit(job(1)));
        let (first, first_cancel) = slot.take().unwrap();
        assert!(slot.accepts(first.id, &first_cancel));

        assert!(!slot.submit(job(2)));
        assert!(!slot.submit(job(3)));
        assert!(!slot.submit(job(2)));
        assert!(first_cancel.load(Ordering::Relaxed));
        assert!(!slot.accepts(first.id, &first_cancel));

        let (latest, live) = slot.take().unwrap();
        assert_eq!(latest.id, 3);
        slot.close(2);
        assert!(slot.accepts(latest.id, &live));
        slot.close(3);
        assert!(!slot.accepts(latest.id, &live));
        assert!(slot.take().is_none());
        assert!(!slot.submit(job(3)));
        assert!(slot.submit(job(4)));
    }

    struct TestDirs(PathBuf);
    impl TestDirs {
        fn new() -> Self {
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "gall-state-test-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TestDirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_test_state(root: &std::path::Path) -> AppState {
        let dirs = AppDirs::at(root);
        let journal = Arc::new(UndoJournal::default());
        let tasks = Arc::new(TaskManager::new(journal, Arc::new(|_| {})));
        let watcher =
            FolderWatcher::new(std::time::Duration::from_millis(100), Arc::new(|_, _| {}));
        AppState::load(
            dirs,
            tasks,
            watcher,
            Settings::default(),
            LoadedFrom::Default,
            OmarchyBridge::new_inactive(),
        )
    }

    #[test]
    fn multi_window_sessions_are_isolated_and_only_main_persists() {
        let tmp = TestDirs::new();
        let state = make_test_state(&tmp.0);

        let w1 = state.allocate_window_label();
        assert_eq!(w1, "win-1");
        let args = CliArgs {
            path1: Some("/tmp".into()),
            ..Default::default()
        };
        let win1_sess = state.ensure_session(&w1, Some(&args));
        assert_eq!(win1_sess.active_tab(0).path, "/tmp");

        // Mutating `"main"` does not affect `"win-1"`, and mutating `"win-1"` does not affect `"main"`.
        state.with_session_mut("main", |s| s.navigate(0, "/etc"));
        state.with_session_mut(&w1, |s| s.navigate(0, "/var"));
        assert_eq!(state.session_for("main").active_tab(0).path, "/etc");
        assert_eq!(state.session_for(&w1).active_tab(0).path, "/var");

        // Secondary window save does not write `session.json`; flushing main persists `"/etc"`.
        state.save_session_for(&w1);
        assert!(!state.session_store.path().exists());
        state.flush_session();
        let (loaded, _): (Option<StoredSession>, _) = state.session_store.load();
        assert_eq!(
            loaded
                .unwrap()
                .into_store("/")
                .active_session()
                .active_tab(0)
                .path,
            "/etc"
        );

        // Removing `"win-1"` clears secondary session but never removes `"main"`.
        state.remove_window(&w1);
        assert!(!state.sessions.lock().unwrap().contains_key(&w1));
        state.remove_window("main");
        assert!(state.sessions.lock().unwrap().contains_key("main"));
    }

    #[test]
    fn legacy_session_file_loads_as_workspace_one_and_persists_multiple_workspaces() {
        let tmp = TestDirs::new();
        let store = JsonStore::new(AppDirs::at(&tmp.0).session());
        let mut legacy = Session::new("/legacy-left", "/legacy-right");
        legacy.navigate(0, "/legacy-left/sub");
        store.save(&legacy).unwrap();

        let state = make_test_state(&tmp.0);
        let snap = state.workspace_snapshot_for("main");
        assert_eq!(snap.workspaces.len(), 1);
        assert_eq!(snap.active, 0);
        assert_eq!(snap.session.active_tab(0).path, "/legacy-left/sub");
        assert_eq!(snap.session.active_tab(1).path, "/legacy-right");

        state.with_workspaces_mut("main", |ws| {
            ws.create(
                "/ws2-a",
                "/ws2-b",
                Some("Ops".into()),
                Some("#3fb27f".into()),
                true,
            );
        });
        state.flush_session();

        let reloaded = make_test_state(&tmp.0);
        let snap2 = reloaded.workspace_snapshot_for("main");
        assert_eq!(snap2.workspaces.len(), 2);
        assert_eq!(snap2.active, 1);
        assert_eq!(snap2.workspaces[1].name.as_deref(), Some("Ops"));
        assert_eq!(snap2.workspaces[1].accent.as_deref(), Some("#3fb27f"));
        assert_eq!(snap2.session.active_tab(0).path, "/ws2-a");
    }

    #[test]
    fn a_picker_on_main_never_persists_its_session() {
        let tmp = TestDirs::new();
        let state = make_test_state(&tmp.0);
        let pick = CliArgs {
            path1: Some("/tmp".into()),
            pick: Some(crate::cli::PickArgs::default()),
            ..Default::default()
        };
        state.ensure_session("main", Some(&pick));
        state.set_window_initial_cli("main", pick);
        state.with_session_mut("main", |s| s.navigate(0, "/etc"));
        state.save_session_for("main");
        state.flush_session();
        assert!(!state.session_store.path().exists());
        state.remove_window("main");
        assert!(!state.sessions.lock().unwrap().contains_key("main"));
    }

    #[test]
    fn pickers_get_their_own_label_never_main() {
        let tmp = TestDirs::new();
        let state = make_test_state(&tmp.0);
        let a = state.allocate_pick_label();
        let b = state.allocate_pick_label();
        assert!(a.starts_with("pick-") && b.starts_with("pick-"));
        assert_ne!(a, b);
        assert_ne!(a, "main");
    }

    #[test]
    fn waiting_compare_is_excluded_from_ordinary_launches_for_its_generation() {
        let tmp = TestDirs::new();
        let state = make_test_state(&tmp.0);
        let label = state.allocate_window_label();
        let generation = state.begin_window(&label);
        let result = handoff::new_result_path(&tmp.0);
        let args = CliArgs {
            wait: true,
            handoff: Some(result.clone()),
            ..Default::default()
        };
        state.set_window_initial_cli(&label, args);
        assert!(state.is_ordinary_window("main"));
        assert!(!state.is_ordinary_window(&label));
        assert_eq!(state.compare_wait_window(&label).unwrap().result, result);
        assert!(!state.remove_window_generation(&label, generation + 1));
        assert!(!state.is_ordinary_window(&label));
        assert!(state.remove_window_generation(&label, generation));
        assert!(state.compare_wait_window(&label).is_none());
    }

    #[test]
    fn waiting_compare_retry_publishes_the_original_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let result = handoff::new_result_path(tmp.path());
        let wait = CompareWaitWindow::from_args(&CliArgs {
            handoff: Some(result.clone()),
            ..Default::default()
        })
        .unwrap();
        std::fs::create_dir(&result).unwrap();
        assert!(wait.finish(|| Outcome::Accepted).is_err());
        std::fs::remove_dir(&result).unwrap();
        assert!(wait
            .finish(|| panic!("receipt retry must not repeat completion"))
            .unwrap());
        assert_eq!(handoff::read_outcome(&result), Some(Outcome::Accepted));
    }

    #[test]
    fn waiting_compare_never_finishes_after_caller_withdraws() {
        let tmp = tempfile::tempdir().unwrap();
        let result = handoff::new_result_path(tmp.path());
        let wait = CompareWaitWindow::from_args(&CliArgs {
            handoff: Some(result.clone()),
            ..Default::default()
        })
        .unwrap();
        assert!(crate::compare_wait::reserve_result(&result).unwrap());
        assert!(!wait
            .finish(|| panic!("an interrupted caller owns the decision"))
            .unwrap());
        std::fs::write(handoff::cancel_marker(&result), b"").unwrap();
        assert!(wait.withdraw());
        assert!(!wait
            .finish(|| panic!("cleanup must not reopen the decision"))
            .unwrap());
        assert!(handoff::read_outcome(&result).is_none());
    }

    #[test]
    fn a_late_destroyed_of_a_replaced_window_leaves_the_new_one_alone() {
        let tmp = TestDirs::new();
        let state = make_test_state(&tmp.0);
        let old = state.begin_window("win-9");
        // The label is reused before the old window's `Destroyed` runs.
        let new = state.begin_window("win-9");
        assert_ne!(old, new);
        let args = CliArgs {
            path1: Some("/tmp".into()),
            ..Default::default()
        };
        state.ensure_session("win-9", Some(&args));
        state.set_window_initial_cli("win-9", args);
        state.mark_window_ready("win-9");

        assert!(!state.remove_window_generation("win-9", old));
        assert!(state.get_window_initial_cli("win-9").is_some());
        assert!(state.sessions.lock().unwrap().contains_key("win-9"));
        assert!(state.dispatch_cli_or_enqueue("win-9", CliArgs::default()));

        assert!(state.remove_window_generation("win-9", new));
        assert!(state.get_window_initial_cli("win-9").is_none());
        // A repeated event for the same window is a no-op.
        assert!(!state.remove_window_generation("win-9", new));
    }

    #[test]
    fn pending_cli_queues_are_window_scoped() {
        let tmp = TestDirs::new();
        let state = make_test_state(&tmp.0);
        assert!(!state.dispatch_cli_or_enqueue(
            "win-1",
            CliArgs {
                path1: Some("/a".into()),
                ..Default::default()
            },
        ));
        assert!(!state.dispatch_cli_or_enqueue(
            "main",
            CliArgs {
                path1: Some("/b".into()),
                ..Default::default()
            },
        ));

        let w1 = state.take_pending_cli("win-1");
        assert_eq!(w1.len(), 1);
        assert_eq!(w1[0].path1.as_deref(), Some("/a"));
        assert!(state.take_pending_cli("win-1").is_empty());

        let main = state.take_pending_cli("main");
        assert_eq!(main.len(), 1);
        assert_eq!(main[0].path1.as_deref(), Some("/b"));
    }

    #[test]
    fn atomic_dispatch_respects_readiness_and_never_strands_or_drops() {
        let tmp = TestDirs::new();
        let state = make_test_state(&tmp.0);

        let cli1 = CliArgs {
            path1: Some("/first".into()),
            ..Default::default()
        };
        let fm1 = FileManagerAction::ShowFolders {
            folders: vec!["/first".into()],
            startup_id: "s1".into(),
        };
        // Not ready -> atomically queued, returns false.
        assert!(!state.dispatch_fm_or_enqueue("main", cli1, fm1.clone()));

        // Window becomes ready and drains pending queues.
        state.mark_window_ready("main");
        let drained_cli = state.take_pending_cli("main");
        let drained_fm = state.take_pending_fm("main");
        assert_eq!(drained_cli.len(), 1);
        assert_eq!(drained_cli[0].path1.as_deref(), Some("/first"));
        assert_eq!(drained_fm, vec![fm1]);

        // Already ready -> returns true and does not strand items in pending queues.
        let cli2 = CliArgs {
            path1: Some("/second".into()),
            ..Default::default()
        };
        let fm2 = FileManagerAction::ShowFolders {
            folders: vec!["/second".into()],
            startup_id: "s2".into(),
        };
        assert!(state.dispatch_fm_or_enqueue("main", cli2.clone(), fm2));
        assert!(state.dispatch_cli_or_enqueue("main", cli2));
        assert!(state.take_pending_cli("main").is_empty());
        assert!(state.take_pending_fm("main").is_empty());

        // `remove_window` clears readiness so future dispatches queue again.
        state.remove_window("main");
        assert!(!state.dispatch_cli_or_enqueue(
            "main",
            CliArgs {
                path1: Some("/third".into()),
                ..Default::default()
            }
        ));
        assert_eq!(state.take_pending_cli("main").len(), 1);

        // `seed_pending_fm` seeds before readiness and drains cleanly.
        let fm3 = FileManagerAction::ShowFolders {
            folders: vec!["/cold".into()],
            startup_id: "s3".into(),
        };
        state.seed_pending_fm("win-2", fm3.clone());
        assert_eq!(state.take_pending_fm("win-2"), vec![fm3]);
    }

    #[test]
    fn concurrent_dispatch_and_window_ready_never_loses_or_duplicates_delivery() {
        let tmp = TestDirs::new();
        let state = Arc::new(make_test_state(&tmp.0));
        let total = 200usize;
        let emitted_count = Arc::new(AtomicUsize::new(0));

        let s_disp = state.clone();
        let emitted_disp = emitted_count.clone();
        let producer = std::thread::spawn(move || {
            for i in 0..total {
                let cli = CliArgs {
                    path1: Some(format!("/item/{i}")),
                    ..Default::default()
                };
                let fm = FileManagerAction::ShowFolders {
                    folders: vec![format!("/item/{i}")],
                    startup_id: format!("id-{i}"),
                };
                if s_disp.dispatch_fm_or_enqueue("main", cli, fm) {
                    emitted_disp.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        let s_ready = state.clone();
        let consumer = std::thread::spawn(move || {
            std::thread::yield_now();
            s_ready.mark_window_ready("main");
            let cli_drained = s_ready.take_pending_cli("main");
            let fm_drained = s_ready.take_pending_fm("main");
            (cli_drained, fm_drained)
        });

        producer.join().unwrap();
        let (cli_drained, fm_drained) = consumer.join().unwrap();
        let emitted = emitted_count.load(Ordering::Relaxed);

        assert_eq!(cli_drained.len(), fm_drained.len());
        assert_eq!(emitted + fm_drained.len(), total);
        assert!(state.take_pending_cli("main").is_empty());
        assert!(state.take_pending_fm("main").is_empty());
    }

    #[test]
    fn concurrent_cold_launch_serialization_never_duplicates_main_window() {
        let tmp = TestDirs::new();
        let state = Arc::new(make_test_state(&tmp.0));
        let windows = Arc::new(Mutex::new(Vec::<String>::new()));
        let threads = 8usize;
        let barrier = Arc::new(std::sync::Barrier::new(threads));

        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let st = state.clone();
                let wins = windows.clone();
                let bar = barrier.clone();
                std::thread::spawn(move || {
                    bar.wait();
                    let _open_guard = st.lock_window_open();
                    let route_or_open = i % 2 == 0;
                    let existing_target = if route_or_open {
                        wins.lock().unwrap().first().cloned()
                    } else {
                        None
                    };
                    if let Some(target) = existing_target {
                        let cli = CliArgs {
                            path1: Some(format!("/routed/{i}")),
                            ..Default::default()
                        };
                        let fm = FileManagerAction::ShowFolders {
                            folders: vec![format!("/routed/{i}")],
                            startup_id: format!("id-{i}"),
                        };
                        assert!(!st.dispatch_fm_or_enqueue(&target, cli, fm));
                    } else {
                        let label = {
                            let snap = wins.lock().unwrap();
                            st.choose_window_label(
                                None,
                                |l| snap.iter().any(|w| w == l),
                                || !snap.is_empty(),
                            )
                        };
                        // Simulate WebviewWindowBuilder::build() latency before registration.
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        let cli = CliArgs {
                            path1: Some(format!("/opened/{i}")),
                            ..Default::default()
                        };
                        st.ensure_session(&label, Some(&cli));
                        st.set_window_initial_cli(&label, cli);
                        if route_or_open {
                            st.seed_pending_fm(
                                &label,
                                FileManagerAction::ShowFolders {
                                    folders: vec![format!("/opened/{i}")],
                                    startup_id: format!("id-{i}"),
                                },
                            );
                        }
                        st.set_active_window(&label);
                        wins.lock().unwrap().push(label);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let created = windows.lock().unwrap().clone();
        assert_eq!(
            created.iter().filter(|l| l.as_str() == "main").count(),
            1,
            "expected exactly one 'main' window, got {created:?}"
        );
        let unique: HashSet<&str> = created.iter().map(String::as_str).collect();
        assert_eq!(
            unique.len(),
            created.len(),
            "duplicate window labels created: {created:?}"
        );

        let mut total_delivered = 0usize;
        for label in &created {
            assert!(state.get_window_initial_cli(label).is_some());
            let pending_cli = state.take_pending_cli(label);
            let pending_fm = state.take_pending_fm(label);
            if label == "main" {
                total_delivered += 1 + pending_cli.len();
                assert!(pending_fm.len() >= pending_cli.len());
            } else {
                total_delivered += 1;
                assert!(pending_cli.is_empty());
            }
        }
        assert_eq!(total_delivered, threads);
    }
}
