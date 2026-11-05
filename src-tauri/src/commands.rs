//! Tauri commands: a thin, typed adapter over `gall-core`.
//! Every command returns `Result<T, String>` so errors surface as rejected
//! promises in the UI and become notifications.

use crate::state::{AppState, FileClipboard};
use gall_core::fs::index::FileIndex;
use gall_core::fs::ops::{self, ConflictPolicy, TaskKind, TaskRequest};
use gall_core::fs::{archive_view, drives, grep, listing, search, FileEntry, ListOptions};
use gall_core::nav::path::{self as npath, PathFormat};
use gall_core::nav::session::Layout;
use gall_core::nav::Session;
use gall_core::preview::{self, PreviewInfo};
use gall_core::storage::db::{FolderPrefs, ResolvedPrefs, Tagged};
use gall_core::storage::{Favorites, Settings};
use gall_core::util::{clipname, notes, templates, terminal};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

// ───────────────────────── app / settings ─────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    platform: String,
    home: String,
    separator: String,
    config_dir: String,
    data_dir: String,
    templates_dir: String,
    trash_dir: Option<String>,
    startup_notice: Option<String>,
    /// First start (no settings file): show the welcome setup.
    first_run: bool,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    let trash_dir = if cfg!(target_os = "linux") {
        dirs::data_dir().map(|d| d.join("Trash/files").to_string_lossy().into_owned())
    } else {
        None
    };
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        platform: std::env::consts::OS.into(),
        home: crate::state::home(),
        separator: std::path::MAIN_SEPARATOR.to_string(),
        config_dir: state.dirs.config.to_string_lossy().into_owned(),
        data_dir: state.dirs.data.to_string_lossy().into_owned(),
        templates_dir: state.dirs.templates().to_string_lossy().into_owned(),
        trash_dir,
        startup_notice: state.startup_notice.lock().unwrap().take(),
        first_run: state.first_run,
    }
}

/// Shows the main window the first time (it is created hidden); later calls do nothing.
pub fn reveal_main(app: &tauri::AppHandle) {
    static SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SHOWN.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
    log::info!("startup: window shown at {} ms", crate::startup_ms());
}

/// The UI has painted its first frame: show the window.
#[tauri::command]
pub fn window_ready(app: tauri::AppHandle) {
    reveal_main(&app);
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.read().unwrap().clone()
}

#[tauri::command]
pub fn set_settings(state: State<'_, AppState>, settings: Settings) -> R<()> {
    state.settings_store.save(&settings).map_err(e)?;
    *state.settings.write().unwrap() = settings;
    Ok(())
}

// ───────────────────────── listing / drives / prefs ─────────────────────────

#[tauri::command]
pub async fn list_dir(path: String, opts: ListOptions) -> R<Vec<FileEntry>> {
    tauri::async_runtime::spawn_blocking(move || {
        // A stopped WSL distribution is started first, with a timeout and a clear message.
        gall_core::fs::wsl::prepare(Path::new(&path)).map_err(e)?;
        listing::list_dir(Path::new(&path), opts).map_err(e)
    })
    .await
    .map_err(e)?
}

#[tauri::command]
pub async fn list_drives() -> R<Vec<drives::Drive>> {
    tauri::async_runtime::spawn_blocking(drives::list_drives)
        .await
        .map_err(e)
}

/// Cloud storage folders (OneDrive, Dropbox, Google Drive, iCloud, …) on this computer.
#[tauri::command]
pub async fn list_cloud_folders() -> R<Vec<gall_core::fs::cloud::CloudFolder>> {
    tauri::async_runtime::spawn_blocking(|| gall_core::fs::cloud::detect(&drives::list_drives()))
        .await
        .map_err(e)
}

// ───────────────────────── compare ─────────────────────────

#[tauri::command]
pub async fn diff_compare(
    a: gall_core::fs::diff::DiffSource,
    b: gall_core::fs::diff::DiffSource,
    opts: gall_core::fs::diff::DiffOptions,
) -> R<gall_core::fs::diff::DiffResult> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::diff::compare(&a, &b, opts))
        .await
        .map_err(e)?
        .map_err(e)
}

/// Syntax colours for the lines of one compare side (None: unknown language).
#[tauri::command]
pub async fn diff_highlight(
    name: String,
    lines: Vec<String>,
) -> R<Option<gall_core::fs::syntax::Highlight>> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::syntax::highlight(&name, &lines))
        .await
        .map_err(e)
}

#[tauri::command]
pub async fn diff_patch(
    a: gall_core::fs::diff::DiffSource,
    b: gall_core::fs::diff::DiffSource,
) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::diff::unified_patch(&a, &b))
        .await
        .map_err(e)?
        .map_err(e)
}

/// Writes the unified patch of `a` → `b` to `dest`.
#[tauri::command]
pub async fn diff_save_patch(
    a: gall_core::fs::diff::DiffSource,
    b: gall_core::fs::diff::DiffSource,
    dest: PathBuf,
) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || -> R<()> {
        let patch = gall_core::fs::diff::unified_patch(&a, &b).map_err(e)?;
        std::fs::write(&dest, patch).map_err(e)
    })
    .await
    .map_err(e)?
}

fn new_dircmp_cancel(state: &AppState) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(prev) = state.dircmp_cancel.lock().unwrap().replace(cancel.clone()) {
        prev.store(true, Ordering::Relaxed);
    }
    cancel
}

#[tauri::command]
pub async fn dircmp_compare(
    state: State<'_, AppState>,
    left: PathBuf,
    right: PathBuf,
    opts: gall_core::fs::dircmp::DirCmpOptions,
) -> R<gall_core::fs::dircmp::DirCmpResult> {
    let cancel = new_dircmp_cancel(&state);
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::dircmp::compare_dirs(&left, &right, opts, &cancel)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

#[tauri::command]
pub fn dircmp_cancel(state: State<'_, AppState>) {
    if let Some(c) = state.dircmp_cancel.lock().unwrap().take() {
        c.store(true, Ordering::Relaxed);
    }
}

/// Applies a folder sync (copies + trash) as one undoable operation.
#[tauri::command]
pub async fn sync_apply(
    state: State<'_, AppState>,
    ops: Vec<gall_core::fs::ops::SyncOp>,
) -> R<gall_core::fs::ops::SyncReport> {
    let cancel = new_dircmp_cancel(&state);
    let journal = state.journal().clone();
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::ops::apply_sync(&ops, &journal, &cancel)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Saves text edited in the compare view (previous version to trash, one undo step).
#[tauri::command]
/// Returns whether the save can be undone (false where the old version could not be trashed).
pub async fn save_text(state: State<'_, AppState>, path: PathBuf, text: String) -> R<bool> {
    let journal = state.journal().clone();
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::ops::save_text(&path, &text, &journal)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Pixel comparison of two images; writes a highlight PNG to the temp folder.
#[tauri::command]
pub async fn image_diff(a: PathBuf, b: PathBuf) -> R<gall_core::fs::diff::ImageDiff> {
    tauri::async_runtime::spawn_blocking(move || {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&a, &b).hash(&mut h);
        let out = std::env::temp_dir().join(format!("gall-imgdiff-{:x}.png", h.finish()));
        gall_core::fs::diff::image_diff(&a, &b, &out)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Pairwise similarity of several files (compare view for 3+ files).
#[tauri::command]
pub async fn similarity_matrix(paths: Vec<PathBuf>) -> R<gall_core::fs::diff::SimilarityMatrix> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::diff::similarity_matrix(&paths))
        .await
        .map_err(e)?
        .map_err(e)
}

/// Table comparison (CSV/TSV, xlsx/xls/xlsb/ods).
#[tauri::command]
pub async fn table_compare(
    a: PathBuf,
    b: PathBuf,
    opts: gall_core::fs::table::TableDiffOptions,
) -> R<gall_core::fs::table::TableDiffResult> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::table::compare_tables(&a, &b, &opts)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Git work tree root for a path (None outside a repository).
#[tauri::command]
pub async fn git_root(path: PathBuf) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::diff::git_root(&path).map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .ok()
    .flatten()
}

/// One sample for the sidebar system monitor (rates are since the previous call).
#[tauri::command]
pub async fn system_stats(state: State<'_, AppState>) -> R<gall_core::sysmon::SysStats> {
    let m = state.monitor.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut g = m.lock().unwrap();
        g.get_or_insert_with(gall_core::sysmon::Monitor::new)
            .sample()
    })
    .await
    .map_err(e)
}

/// Unmounts / ejects a drive (runs the system's tool, which can take a while).
#[tauri::command]
pub async fn eject(app: AppHandle, mount: String) -> R<()> {
    off_main(app, move |state| {
        // Folders on the drive that are still watched (a column that has not closed yet)
        // would keep it busy: let go of them first, back if the eject fails.
        let released = state.watcher.release_under(Path::new(&mount));
        let res = drives::eject(&mount).map_err(e);
        state.watcher.resume(&released);
        res
    })
    .await
}

/// Volumes that are not mounted yet (sidebar "Not mounted").
#[tauri::command]
pub async fn other_volumes() -> R<Vec<gall_core::fs::volumes::Volume>> {
    tauri::async_runtime::spawn_blocking(gall_core::fs::volumes::other_volumes)
        .await
        .map_err(e)
}

/// Mounts a volume; returns where. May wait for a system password prompt (polkit).
#[tauri::command]
pub async fn volume_mount(id: String) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::volumes::mount_volume(&id))
        .await
        .map_err(e)?
        .map_err(e)
}

/// Unlocks and mounts an encrypted volume; returns where (None on Windows, which
/// shows its own dialog).
#[tauri::command]
pub async fn volume_unlock(id: String, passphrase: String) -> R<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::volumes::unlock_volume(&id, &passphrase)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Unmounts an unlocked encrypted volume and locks it.
#[tauri::command]
pub async fn drive_lock(mount: String) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::volumes::lock_mount(&mount))
        .await
        .map_err(e)?
        .map_err(e)
}

/// Every setting back to its default and the folder views forgotten; favourites,
/// colour tags, history and the open tabs stay. Returns the new settings.
#[tauri::command]
pub fn reset_settings(state: State<'_, AppState>) -> R<Settings> {
    let settings = Settings::default();
    state.settings_store.save(&settings).map_err(e)?;
    *state.settings.write().unwrap() = settings.clone();
    state.db.clear_prefs().map_err(e)?;
    Ok(settings)
}

#[tauri::command]
pub fn resolve_prefs(state: State<'_, AppState>, path: String) -> R<ResolvedPrefs> {
    let s = state.settings.read().unwrap();
    if !s.remember_folder_views {
        return Ok(ResolvedPrefs {
            sort_key: s.default_sort,
            descending: s.default_descending,
            folders_on_top: s.folders_on_top,
            view: s.default_view.clone(),
            view_source: None,
        });
    }
    state
        .db
        .resolve_prefs(
            Path::new(&path),
            s.default_sort,
            s.default_descending,
            s.folders_on_top,
            &s.default_view,
        )
        .map_err(e)
}

#[tauri::command]
pub fn clear_prefs(state: State<'_, AppState>) -> R<()> {
    state.db.clear_prefs().map_err(e)
}

#[tauri::command]
pub fn set_prefs(state: State<'_, AppState>, path: String, prefs: FolderPrefs) -> R<()> {
    state.db.set_prefs(&path, &prefs).map_err(e)
}

// ───────────────────────── session ─────────────────────────

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SessionCmd {
    Navigate {
        b: usize,
        path: String,
    },
    Back {
        b: usize,
    },
    Forward {
        b: usize,
    },
    Up {
        b: usize,
    },
    NewTab {
        b: usize,
        path: Option<String>,
        activate: Option<bool>,
    },
    Activate {
        b: usize,
        idx: usize,
    },
    Close {
        b: usize,
        idx: usize,
    },
    CloseOthers {
        b: usize,
        idx: usize,
    },
    CloseStale {
        b: usize,
    },
    Reopen {
        b: usize,
    },
    Reorder {
        b: usize,
        from: usize,
        to: usize,
    },
    Transfer {
        b: usize,
        idx: usize,
        copy: bool,
    },
    Alias {
        b: usize,
        idx: usize,
        alias: Option<String>,
        keep: bool,
    },
    Layout {
        layout: Layout,
    },
    Collapse {
        b: usize,
        collapsed: bool,
    },
    Focus {
        b: usize,
    },
    /// Show or hide the preview & details pane of one browser.
    Details {
        b: usize,
        on: bool,
    },
    /// Miller columns (on) or breadcrumbs (off) in one browser, for the current layout.
    Columns {
        b: usize,
        on: bool,
    },
    /// Sort / view of one browser while folders do not remember their own (set fields
    /// replace, unset ones stay).
    View {
        b: usize,
        prefs: FolderPrefs,
    },
    /// Swap the folders (active tabs) of the two browsers; the panes stay put.
    Swap,
}

#[tauri::command]
pub fn session_get(state: State<'_, AppState>) -> Session {
    state.session.lock().unwrap().clone()
}

#[tauri::command]
pub fn session_cmd(state: State<'_, AppState>, cmd: SessionCmd) -> R<Session> {
    let (placement, new_tab_path, history_count) = {
        let s = state.settings.read().unwrap();
        (s.new_tab_placement, s.new_tab_path.clone(), s.history_count)
    };
    let mut visited: Option<String> = None;
    let snapshot = {
        let mut s = state.session.lock().unwrap();
        match cmd {
            SessionCmd::Navigate { b, path } => {
                s.navigate(b, &path);
                visited = Some(path);
            }
            SessionCmd::Back { b } => {
                if s.back(b) {
                    visited = Some(s.active_tab(b).path.clone());
                }
            }
            SessionCmd::Forward { b } => {
                if s.forward(b) {
                    visited = Some(s.active_tab(b).path.clone());
                }
            }
            SessionCmd::Up { b } => {
                let cur = s.active_tab(b).path.clone();
                if let Some(p) = Path::new(&cur).parent() {
                    let p = p.to_string_lossy().into_owned();
                    s.navigate(b, &p);
                    visited = Some(p);
                }
            }
            SessionCmd::NewTab { b, path, activate } => {
                let configured =
                    (!new_tab_path.trim().is_empty()).then(|| npath::expand(&new_tab_path));
                let p = path.or(configured);
                s.new_tab(b, p.as_deref(), placement, activate.unwrap_or(true));
            }
            SessionCmd::Activate { b, idx } => s.activate(b, idx),
            SessionCmd::Close { b, idx } => {
                s.close_tab(b, idx);
            }
            SessionCmd::CloseOthers { b, idx } => s.close_others(b, idx),
            SessionCmd::CloseStale { b } => {
                s.close_stale(b, chrono::Utc::now().timestamp_millis());
            }
            SessionCmd::Reopen { b } => {
                s.reopen_closed(b);
            }
            SessionCmd::Reorder { b, from, to } => s.move_tab_within(b, from, to),
            SessionCmd::Transfer { b, idx, copy } => s.transfer_tab(b, idx, copy),
            SessionCmd::Alias {
                b,
                idx,
                alias,
                keep,
            } => s.set_alias(b, idx, alias, keep),
            SessionCmd::Layout { layout } => s.layout = layout,
            SessionCmd::Collapse { b, collapsed } => {
                if let Some(br) = s.browsers.get_mut(b) {
                    br.collapsed = collapsed;
                }
                if collapsed && s.active_browser == b {
                    s.active_browser = 1 - b.min(1);
                }
            }
            SessionCmd::Focus { b } => s.active_browser = b.min(1),
            SessionCmd::Details { b, on } => {
                if let Some(br) = s.browsers.get_mut(b) {
                    br.details = Some(on);
                }
            }
            SessionCmd::Columns { b, on } => {
                let layout = s.layout;
                if let Some(br) = s.browsers.get_mut(b) {
                    br.columns.insert(layout, on);
                }
            }
            SessionCmd::View { b, prefs } => {
                if let Some(br) = s.browsers.get_mut(b) {
                    let v = &mut br.view;
                    v.sort_key = prefs.sort_key.or(v.sort_key);
                    v.descending = prefs.descending.or(v.descending);
                    v.folders_on_top = prefs.folders_on_top.or(v.folders_on_top);
                    v.view = prefs.view.or(v.view.take());
                }
            }
            SessionCmd::Swap => s.swap_active_tabs(),
        }
        s.clone()
    };
    if let Some(p) = visited {
        if let Err(err) = state.db.record_visit(&p, history_count) {
            log::warn!("history: {err}");
        }
    }
    state.save_session();
    Ok(snapshot)
}

// ───────────────────────── favourites / history ─────────────────────────

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FavCmd {
    AddGroup {
        name: String,
    },
    RenameGroup {
        id: u64,
        name: String,
    },
    DeleteGroup {
        id: u64,
    },
    MoveGroup {
        id: u64,
        delta: i32,
    },
    Collapse {
        id: u64,
        collapsed: bool,
    },
    AddItem {
        id: u64,
        path: String,
        index: Option<usize>,
    },
    RemoveItem {
        id: u64,
        index: usize,
    },
    Alias {
        id: u64,
        index: usize,
        alias: Option<String>,
    },
    MoveItem {
        from_group: u64,
        from_index: usize,
        to_group: u64,
        to_index: usize,
    },
    RestoreDefault,
}

#[tauri::command]
pub fn favorites_get(state: State<'_, AppState>) -> Favorites {
    state.favorites.lock().unwrap().clone()
}

#[tauri::command]
pub fn favorites_cmd(state: State<'_, AppState>, cmd: FavCmd) -> Favorites {
    let snap = {
        let mut f = state.favorites.lock().unwrap();
        match cmd {
            FavCmd::AddGroup { name } => {
                f.add_group(&name);
            }
            FavCmd::RenameGroup { id, name } => f.rename_group(id, &name),
            FavCmd::DeleteGroup { id } => f.delete_group(id),
            FavCmd::MoveGroup { id, delta } => f.move_group(id, delta),
            FavCmd::Collapse { id, collapsed } => f.set_collapsed(id, collapsed),
            FavCmd::AddItem { id, path, index } => {
                f.add_item(id, &path, index);
            }
            FavCmd::RemoveItem { id, index } => f.remove_item(id, index),
            FavCmd::Alias { id, index, alias } => f.set_alias(id, index, alias),
            FavCmd::MoveItem {
                from_group,
                from_index,
                to_group,
                to_index,
            } => f.move_item(from_group, from_index, to_group, to_index),
            FavCmd::RestoreDefault => {
                f.restore_default_group();
            }
        }
        f.clone()
    };
    state.save_favorites();
    snap
}

/// The history, newest first, without folders that were deleted or renamed since
/// (dropped from the store too); folders on a drive that is not connected stay.
#[tauri::command]
pub async fn history_get(state: State<'_, AppState>) -> R<Vec<String>> {
    let n = state.settings.read().unwrap().history_count;
    let list = state.db.history(n).map_err(e)?;
    let gone: Vec<String> = tauri::async_runtime::spawn_blocking({
        let list = list.clone();
        move || {
            list.into_iter()
                .filter(|p| drives::folder_gone(Path::new(p)))
                .collect()
        }
    })
    .await
    .map_err(e)?;
    for p in &gone {
        state.db.remove_history(Some(p)).map_err(e)?;
    }
    Ok(list.into_iter().filter(|p| !gone.contains(p)).collect())
}

#[tauri::command]
pub fn history_remove(state: State<'_, AppState>, path: Option<String>) -> R<()> {
    state.db.remove_history(path.as_deref()).map_err(e)
}

// ───────────────────────── preview ─────────────────────────

#[tauri::command]
pub async fn preview_info(state: State<'_, AppState>, path: String) -> R<PreviewInfo> {
    let limit = state.settings.read().unwrap().preview_text_limit;
    tauri::async_runtime::spawn_blocking(move || {
        preview::preview_info(Path::new(&path), limit).map_err(e)
    })
    .await
    .map_err(e)?
}

/// Codec and a still frame for a video the built-in player could not play (needs FFmpeg).
#[tauri::command]
pub async fn video_probe(
    state: State<'_, AppState>,
    path: String,
) -> R<preview::video::VideoProbe> {
    let cache = state.dirs.data.join("cache").join("posters");
    tauri::async_runtime::spawn_blocking(move || preview::video::probe(Path::new(&path), &cache))
        .await
        .map_err(e)
}

#[tauri::command]
pub async fn excerpts(paths: Vec<String>, lines: usize) -> Vec<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .iter()
            .map(|p| preview::excerpt(Path::new(p), lines))
            .collect()
    })
    .await
    .unwrap_or_default()
}

// ───────────────────────── watching ─────────────────────────

#[tauri::command]
pub async fn watch(app: AppHandle, path: String) -> R<()> {
    off_main(app, move |state| {
        state.watcher.watch(Path::new(&path)).map_err(e)
    })
    .await
}

#[tauri::command]
pub fn unwatch(state: State<'_, AppState>, path: String) {
    state.watcher.unwatch(Path::new(&path));
}

// ───────────────────────── file operations ─────────────────────────

/// Starts a background file task (checking the destination touches the disk).
#[tauri::command]
pub async fn start_task(app: AppHandle, req: TaskRequest) -> R<u64> {
    off_main(app, move |state| start_task_in(state, req)).await
}

fn start_task_in(state: &AppState, req: TaskRequest) -> R<u64> {
    if let (Some(dest), TaskKind::Copy | TaskKind::Move) = (&req.dest, req.kind) {
        let keep = state.settings.read().unwrap().recent_destinations;
        let _ = state.db.record_destination(&dest.to_string_lossy(), keep);
    }
    state.tasks.start(req).map_err(e)
}

#[tauri::command]
pub fn cancel_task(state: State<'_, AppState>, id: u64) -> bool {
    state.tasks.cancel(id)
}

/// Runs file-system work on a worker thread, with the app state at hand. Commands that
/// are not `async` run on the main thread, where a slow or hung drive (a sleeping NFS or
/// SMB share) would freeze the whole window.
async fn off_main<T, F>(app: AppHandle, f: F) -> R<T>
where
    T: Send + 'static,
    F: FnOnce(&AppState) -> R<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || f(&app.state::<AppState>()))
        .await
        .map_err(e)?
}

fn lossy(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

#[tauri::command]
pub async fn find_conflicts(
    app: AppHandle,
    sources: Vec<PathBuf>,
    dest: PathBuf,
) -> R<Vec<PathBuf>> {
    off_main(app, move |_| Ok(ops::find_conflicts(&sources, &dest))).await
}

/// Conflicts of a copy or move into `dest`, with what merging folders would leave.
#[tauri::command]
pub async fn conflict_info(
    app: AppHandle,
    sources: Vec<PathBuf>,
    dest: PathBuf,
) -> R<ops::ConflictInfo> {
    off_main(app, move |_| Ok(ops::conflict_info(&sources, &dest))).await
}

/// The paths of `paths` that still exist (after a delete that stopped part-way).
#[tauri::command]
pub async fn find_existing(app: AppHandle, paths: Vec<PathBuf>) -> R<Vec<PathBuf>> {
    off_main(app, move |_| {
        Ok(paths
            .into_iter()
            .filter(|p| std::fs::symlink_metadata(p).is_ok())
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn rename(app: AppHandle, path: String, new_name: String) -> R<String> {
    off_main(app, move |state| {
        // Tags follow through the move hook (see lib.rs).
        ops::rename(Path::new(&path), &new_name, state.journal())
            .map(lossy)
            .map_err(e)
    })
    .await
}

// ───────────────────────── folder sizes ─────────────────────────

/// Running folder size calculations by ticket: a browser cancels its own when it leaves
/// the folder, without stopping the other browser's.
type SizeJobs = std::sync::Mutex<Vec<(u64, Arc<AtomicBool>)>>;
static SIZE_JOBS: std::sync::LazyLock<SizeJobs> = std::sync::LazyLock::new(Default::default);
/// The latest cancelled tickets, for calls that start after their cancel.
static SIZE_CANCELLED: std::sync::Mutex<std::collections::VecDeque<u64>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SizeProgress {
    path: String,
    size: gall_core::fs::dirsize::DirSize,
}

/// Total size of everything in folder `path`; running totals arrive as
/// `folder-size-progress` events. Null when cancelled (see [`folder_sizes_cancel`]).
#[tauri::command]
pub async fn folder_size(
    app: AppHandle,
    path: PathBuf,
    ticket: u64,
) -> R<Option<gall_core::fs::dirsize::DirSize>> {
    // A cancel can overtake this call (it runs on the main thread, this one on a worker):
    // then the calculation is over before it starts.
    let cancelled = SIZE_CANCELLED.lock().unwrap().contains(&ticket);
    let cancel = Arc::new(AtomicBool::new(cancelled));
    SIZE_JOBS.lock().unwrap().push((ticket, cancel.clone()));
    let emitter = app.clone();
    let res = off_main(app, move |_| {
        let name = path.to_string_lossy().into_owned();
        Ok(gall_core::fs::dirsize::dir_size(&path, &cancel, &|s| {
            let _ = emitter.emit(
                "folder-size-progress",
                SizeProgress {
                    path: name.clone(),
                    size: *s,
                },
            );
        }))
    })
    .await;
    SIZE_JOBS
        .lock()
        .unwrap()
        .retain(|(_, c)| Arc::strong_count(c) > 1);
    res
}

/// Stops the folder size calculations started with `ticket`.
#[tauri::command]
pub fn folder_sizes_cancel(ticket: u64) {
    {
        let mut done = SIZE_CANCELLED.lock().unwrap();
        if done.len() >= 64 {
            done.pop_front();
        }
        done.push_back(ticket);
    }
    let mut jobs = SIZE_JOBS.lock().unwrap();
    for (_, c) in jobs.iter().filter(|(t, _)| *t == ticket) {
        c.store(true, Ordering::Relaxed);
    }
    jobs.retain(|(t, _)| *t != ticket);
}

/// Git status of the items in `dir` and its branch; null outside a repository.
#[tauri::command]
pub async fn git_status(
    app: AppHandle,
    dir: PathBuf,
) -> R<Option<gall_core::fs::gitstatus::GitStatus>> {
    off_main(app, move |_| Ok(gall_core::fs::gitstatus::status(&dir))).await
}

/// Batch rename: `(path, new name)` pairs as one undo step; returns the new paths.
#[tauri::command]
pub async fn batch_rename(app: AppHandle, items: Vec<(PathBuf, String)>) -> R<Vec<String>> {
    off_main(app, move |state| {
        let out = ops::batch_rename(&items, state.journal()).map_err(e)?;
        Ok(out.into_iter().map(lossy).collect())
    })
    .await
}

#[tauri::command]
pub async fn create_dir(app: AppHandle, parent: String, name: String) -> R<String> {
    off_main(app, move |state| {
        ops::create_dir(Path::new(&parent), &name, state.journal())
            .map(lossy)
            .map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn create_file(
    app: AppHandle,
    parent: String,
    name: String,
    template: Option<String>,
) -> R<String> {
    off_main(app, move |state| {
        ops::create_file(
            Path::new(&parent),
            &name,
            template.as_deref().map(Path::new),
            None,
            state.journal(),
        )
        .map(lossy)
        .map_err(e)
    })
    .await
}

/// Ctrl+D: copies each item next to itself as a background task (progress, cancel).
/// Returns where the copies will be, so the UI can select them as they appear.
#[tauri::command]
pub async fn duplicate(app: AppHandle, paths: Vec<PathBuf>) -> R<Vec<String>> {
    off_main(app, move |state| {
        let targets = ops::duplicate_targets(&paths);
        state
            .tasks
            .start(TaskRequest {
                kind: TaskKind::Duplicate,
                sources: paths,
                dest: None,
                conflict: ConflictPolicy::KeepBoth,
                archive_name: None,
                format: None,
                extract_mode: Default::default(),
                trash_archive: false,
                merge: false,
            })
            .map_err(e)?;
        Ok(targets.into_iter().map(lossy).collect())
    })
    .await
}

#[tauri::command]
pub async fn trash(state: State<'_, AppState>, paths: Vec<PathBuf>) -> R<()> {
    let journal = state.journal().clone();
    tauri::async_runtime::spawn_blocking(move || ops::trash_items(&paths, &journal).map_err(e))
        .await
        .map_err(e)?
}

#[tauri::command]
pub async fn symlink(app: AppHandle, target: String, dest_dir: String) -> R<String> {
    off_main(app, move |state| {
        ops::symlink(Path::new(&target), Path::new(&dest_dir), state.journal())
            .map(lossy)
            .map_err(e)
    })
    .await
}

/// Undo can move items back across drives or read the whole trash: off the main thread.
#[tauri::command]
pub async fn undo(app: AppHandle, permanent: Option<bool>) -> R<String> {
    let permanent = permanent.unwrap_or(false);
    off_main(app, move |state| {
        state.journal().undo_with(permanent).map_err(e)
    })
    .await
}

#[tauri::command]
pub fn undo_peek(state: State<'_, AppState>) -> Option<String> {
    state.journal().peek_description()
}

/// Per-panel undo: the most recent operation that touched `dir`.
#[tauri::command]
pub async fn undo_in(app: AppHandle, dir: String, permanent: Option<bool>) -> R<String> {
    let permanent = permanent.unwrap_or(false);
    off_main(app, move |state| {
        state
            .journal()
            .undo_in_with(Path::new(&dir), permanent)
            .map_err(e)
    })
    .await
}

#[tauri::command]
pub fn undo_peek_in(state: State<'_, AppState>, dir: String) -> Option<String> {
    state.journal().peek_in(Path::new(&dir))
}

/// The real file for a path inside an archive (extracted into the cache); other paths
/// unchanged. Opening, "Open with" and dragging out work on the extracted copy.
fn real_path(path: &Path) -> R<PathBuf> {
    archive_view::materialize(&[path.to_path_buf()])
        .map(|mut v| v.remove(0))
        .map_err(e)
}

/// Real files for paths that may lie inside archives (copy, clipboard, drag out).
#[tauri::command]
pub async fn archive_materialize(paths: Vec<PathBuf>) -> R<Vec<PathBuf>> {
    tauri::async_runtime::spawn_blocking(move || archive_view::materialize(&paths).map_err(e))
        .await
        .map_err(e)?
}

/// Password for an encrypted archive, kept in memory until Gall exits.
#[tauri::command]
pub fn archive_password(path: PathBuf, password: String) {
    archive_view::set_password(&path, &password);
}

#[tauri::command]
pub async fn open_path(app: AppHandle, state: State<'_, AppState>, path: String) -> R<()> {
    let path = {
        let p = PathBuf::from(&path);
        tauri::async_runtime::spawn_blocking(move || real_path(&p))
            .await
            .map_err(e)??
            .to_string_lossy()
            .into_owned()
    };
    // An application chosen with Open with › Ctrl+Enter wins over the system default.
    let rule = gall_core::storage::settings::OpenRule::find(
        &state.settings.read().unwrap().open_rules,
        Path::new(&path),
    )
    .cloned();
    if let Some(rule) = rule {
        return gall_core::util::openwith::run_choice(Path::new(&path), &rule.command).map_err(e);
    }
    // Windows: ShellExecute + permission to come to the front (else files open behind).
    if let Some(r) = gall_core::util::openwith::open_default(Path::new(&path)) {
        return r.map_err(e);
    }
    app.opener().open_path(path, None::<&str>).map_err(e)
}

// ───────────────────────── open with ─────────────────────────

/// Applications that can open `path` (Linux), default first; empty on Windows,
/// which uses the system dialog instead.
#[tauri::command]
pub async fn open_with_apps(path: PathBuf) -> R<Vec<gall_core::util::openwith::AppChoice>> {
    tauri::async_runtime::spawn_blocking(move || gall_core::util::openwith::apps_for(&path))
        .await
        .map_err(e)
}

#[tauri::command]
pub fn open_with_app(path: PathBuf, id: String) -> R<()> {
    gall_core::util::openwith::open_with_app(&real_path(&path)?, &id).map_err(e)
}

/// Shows the system "Open with" dialog; false where there is none.
#[tauri::command]
pub fn open_with_dialog(path: PathBuf) -> R<bool> {
    gall_core::util::openwith::system_dialog(&real_path(&path)?).map_err(e)
}

/// Opens each path with a choice's command (see `AppChoice::command`).
#[tauri::command]
pub fn open_with_run(paths: Vec<PathBuf>, command: String) -> R<()> {
    for p in archive_view::materialize(&paths).map_err(e)? {
        gall_core::util::openwith::run_choice(&p, &command).map_err(e)?;
    }
    Ok(())
}

/// Terminals installed on this computer, preferred first.
#[tauri::command]
pub async fn terminal_apps() -> R<Vec<gall_core::util::openwith::AppChoice>> {
    tauri::async_runtime::spawn_blocking(gall_core::util::terminal::terminal_apps)
        .await
        .map_err(e)
}

/// Text editors installed on this computer (Edit with…).
#[tauri::command]
pub async fn editor_apps() -> R<Vec<gall_core::util::openwith::AppChoice>> {
    tauri::async_runtime::spawn_blocking(gall_core::util::terminal::editor_apps)
        .await
        .map_err(e)
}

/// Opens a terminal in `dir` with a command template (Terminal here with…).
#[tauri::command]
pub fn open_terminal_with(dir: String, command: String) -> R<()> {
    terminal::open_terminal_with(Path::new(&dir), &command).map_err(e)
}

#[tauri::command]
pub fn open_with_command(path: PathBuf, command: String) -> R<()> {
    gall_core::util::openwith::open_with_command(&real_path(&path)?, &command).map_err(e)
}

#[tauri::command]
pub fn reveal_path(app: AppHandle, path: String) -> R<()> {
    // Inside an archive: show the archive file itself.
    let path = archive_view::split(Path::new(&path))
        .map(|(ar, _)| ar.to_string_lossy().into_owned())
        .unwrap_or(path);
    app.opener().reveal_item_in_dir(path).map_err(e)
}

/// What is in the trash (sidebar › Trash window), most recently deleted first.
#[tauri::command]
pub async fn trash_list() -> R<Vec<gall_core::fs::trashbin::TrashEntry>> {
    tauri::async_runtime::spawn_blocking(gall_core::fs::trashbin::list)
        .await
        .map_err(e)?
        .map_err(e)
}

/// Puts trash items back where they were deleted from.
#[tauri::command]
pub async fn trash_restore(ids: Vec<String>) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::trashbin::restore(&ids))
        .await
        .map_err(e)?
        .map_err(e)
}

/// Deletes trash items for good (`ids` empty: empties the trash).
#[tauri::command]
pub async fn trash_purge(ids: Vec<String>) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || {
        if ids.is_empty() {
            gall_core::fs::trashbin::empty()
        } else {
            gall_core::fs::trashbin::purge(&ids)
        }
    })
    .await
    .map_err(e)?
    .map_err(e)
}

#[tauri::command]
pub fn open_terminal(state: State<'_, AppState>, dir: String) -> R<()> {
    let t = state.settings.read().unwrap().terminal.clone();
    terminal::open_terminal(Path::new(&dir), &t).map_err(e)
}

// ───────────────────────── notes / templates ─────────────────────────

#[tauri::command]
pub async fn create_note(app: AppHandle, dir: String, text: String) -> R<String> {
    off_main(app, move |state| {
        notes::create_note(Path::new(&dir), &text, state.journal())
            .map(lossy)
            .map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn toggle_note(app: AppHandle, path: String) -> R<String> {
    off_main(app, move |state| {
        notes::toggle_done(Path::new(&path), state.journal())
            .map(lossy)
            .map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn update_note(app: AppHandle, path: String, text: String) -> R<()> {
    off_main(app, move |_| {
        notes::update_note(Path::new(&path), &text).map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn templates_list(app: AppHandle) -> R<Vec<templates::Template>> {
    off_main(app, |state| {
        templates::list(&state.dirs.templates()).map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn template_add(app: AppHandle, file: String) -> R<String> {
    off_main(app, move |state| {
        templates::add_from(&state.dirs.templates(), Path::new(&file))
            .map(lossy)
            .map_err(e)
    })
    .await
}

// ───────────────────────── paths ─────────────────────────

#[tauri::command]
pub async fn path_complete(typed: String) -> Vec<String> {
    tauri::async_runtime::spawn_blocking(move || npath::complete(&typed, 30))
        .await
        .unwrap_or_default()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPath {
    path: String,
    is_dir: bool,
    exists: bool,
}

/// Expands and normalises a user-typed path.
#[tauri::command]
pub async fn path_resolve(input: String, base: Option<String>) -> ResolvedPath {
    let fallback = input.clone();
    tauri::async_runtime::spawn_blocking(move || resolve_path(input, base))
        .await
        .unwrap_or(ResolvedPath {
            path: fallback,
            is_dir: false,
            exists: false,
        })
}

fn resolve_path(input: String, base: Option<String>) -> ResolvedPath {
    // file:// URIs decoded (%20) and native (C:\ on Windows).
    let expanded = file_uri_path(&npath::expand(&input));
    // Windows: a Linux path (/home/u) is that folder in the open (or default) WSL distribution.
    let expanded = if cfg!(windows) {
        use gall_core::fs::wsl;
        let default = wsl::default_distro();
        wsl::resolve_typed(
            &expanded,
            base.as_deref(),
            default.as_ref().map(|(d, h)| (d.as_str(), *h)),
        )
        .unwrap_or(expanded)
    } else {
        expanded
    };
    let mut p = PathBuf::from(&expanded);
    if p.is_relative() {
        if let Some(b) = base {
            p = Path::new(&b).join(p);
        }
    }
    let p = npath::canonicalize(&p).unwrap_or(p);
    ResolvedPath {
        exists: p.exists(),
        is_dir: p.is_dir(),
        path: p.to_string_lossy().into_owned(),
    }
}

#[tauri::command]
pub fn path_format(path: String, format: Option<PathFormat>, state: State<'_, AppState>) -> String {
    let f = format.unwrap_or(state.settings.read().unwrap().path_copy_format);
    npath::format_path(&path, f)
}

// ───────────────────────── search ─────────────────────────

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchBatch {
    id: u64,
    entries: Vec<FileEntry>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchDone {
    id: u64,
    hits: usize,
    cancelled: bool,
}

#[tauri::command]
pub fn search_start(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
    root: String,
    pattern: String,
    include_hidden: bool,
    everything: Option<bool>,
) {
    let everything = everything.unwrap_or(false);
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(prev) = state.search_cancel.lock().unwrap().replace(cancel.clone()) {
        prev.store(true, Ordering::Relaxed);
    }
    std::thread::spawn(move || {
        let mut batch = Vec::new();
        let mut last = std::time::Instant::now();
        let opts = {
            let st = app.state::<AppState>();
            let s = st.settings.read().unwrap();
            search::SearchOptions {
                include_hidden,
                respect_gitignore: !everything && s.search_respect_gitignore,
                exclude: if everything {
                    vec![]
                } else {
                    s.index_exclude.clone()
                },
                everything,
                ..Default::default()
            }
        };
        let hits = search::search(Path::new(&root), &pattern, opts, &cancel, |entry| {
            batch.push(entry);
            if batch.len() >= 200 || last.elapsed().as_millis() > 150 {
                let _ = app.emit(
                    "search-batch",
                    SearchBatch {
                        id,
                        entries: std::mem::take(&mut batch),
                    },
                );
                last = std::time::Instant::now();
            }
        });
        if !batch.is_empty() {
            let _ = app.emit("search-batch", SearchBatch { id, entries: batch });
        }
        let _ = app.emit(
            "search-done",
            SearchDone {
                id,
                hits,
                cancelled: cancel.load(Ordering::Relaxed),
            },
        );
    });
}

#[tauri::command]
pub fn search_cancel(state: State<'_, AppState>) {
    if let Some(c) = state.search_cancel.lock().unwrap().take() {
        c.store(true, Ordering::Relaxed);
    }
}

// ───────────────────────── quick open index / find in files ─────────────────────────

/// Adds `path` to the `.gallignore` next to it, so Quick Open, find in files and the
/// name search skip it. Returns the `.gallignore` path, or None when it was already
/// listed there. Quick Open drops it at once, and every later rebuild reads the
/// `.gallignore`, so no rebuild is forced (a whole-system walk per hidden item); an
/// already-running scan is cancelled if it may still publish the newly hidden item.
#[tauri::command]
pub async fn hide_from_search(app: AppHandle, path: String) -> R<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = Path::new(&path);
        let (file, added) = gall_core::fs::ignorefile::hide_from_search(p).map_err(e)?;
        let st = app.state::<AppState>();
        st.index.forget_under(p);
        let mut folder_scan = st.folder_scan.lock().unwrap();
        if let Some(f) = folder_scan.as_ref() {
            let active = f.index.is_building() || f.index.age().is_none();
            if active {
                f.cancel.store(true, Ordering::Relaxed);
            }
            f.index.forget_under(p);
            if active {
                folder_scan.take();
            }
        }
        drop(folder_scan);
        if st.index.is_building() {
            index_rebuild_bg(&app, &st, 0);
        }
        Ok(added.then(|| file.to_string_lossy().into_owned()))
    })
    .await
    .map_err(e)?
}

/// Undoes "Hide from search", or makes an exception for an item a pattern hides.
#[tauri::command]
pub async fn show_in_search(
    app: AppHandle,
    path: String,
) -> R<gall_core::fs::ignorefile::ShowOutcome> {
    use gall_core::fs::ignorefile::ShowOutcome;
    tauri::async_runtime::spawn_blocking(move || {
        let out = gall_core::fs::ignorefile::show_in_search(Path::new(&path)).map_err(e)?;
        if matches!(
            out,
            ShowOutcome::Removed { .. } | ShowOutcome::Excepted { .. }
        ) {
            let st = app.state::<AppState>();
            // The folder scan is read again on its next use, with the item.
            if let Some(f) = st.folder_scan.lock().unwrap().take() {
                f.cancel.store(true, Ordering::Relaxed);
            }
            index_rebuild_bg(&app, &st, 0);
        }
        Ok(out)
    })
    .await
    .map_err(e)?
}

/// Which items of `dir` a `.gallignore` hides from search (dimmed in the list).
#[tauri::command]
pub async fn search_hidden(
    app: AppHandle,
    dir: String,
) -> R<gall_core::fs::ignorefile::HiddenState> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = gall_core::fs::ignorefile::hidden_state(Path::new(&dir));
        // Noticed a hand edit to a .gallignore here or above: Quick Open follows it.
        if gall_core::fs::ignorefile::take_changed() {
            index_rebuild_bg(&app, &app.state::<AppState>(), 0);
        }
        st
    })
    .await
    .map_err(e)
}

fn index_options(s: &Settings) -> (Vec<PathBuf>, gall_core::fs::index::IndexOptions) {
    use gall_core::fs::index::{system_roots, IndexOptions};
    let roots: Vec<PathBuf> = if s.index_roots.is_empty() {
        system_roots()
    } else {
        s.index_roots
            .iter()
            .map(|r| PathBuf::from(npath::expand(r)))
            .collect()
    };
    let opts = IndexOptions {
        include_hidden: s.index_hidden,
        respect_gitignore: s.index_respect_gitignore,
        exclude: s.index_exclude.clone(),
        ..Default::default()
    };
    (roots, opts)
}

/// Starts a background rebuild unless one is running or the index is younger
/// than `max_age_s`. Emits `index-updated` when done.
/// Set when a forced rebuild is requested while one is already running (e.g. the
/// indexed folders changed during the first full scan); the running build then
/// starts over with the current settings instead of dropping the request.
static INDEX_RERUN: AtomicBool = AtomicBool::new(false);
/// Cancels the running build so a queued rerun starts right away.
static INDEX_CANCEL: AtomicBool = AtomicBool::new(false);

pub fn index_rebuild_bg(app: &AppHandle, state: &AppState, max_age_s: u64) -> bool {
    let idx = state.index.clone();
    if idx.is_building() {
        if max_age_s == 0 {
            INDEX_RERUN.store(true, Ordering::SeqCst);
            INDEX_CANCEL.store(true, Ordering::SeqCst);
        }
        return false;
    }
    if idx.age().is_some_and(|a| a.as_secs() < max_age_s) {
        return false;
    }
    idx.set_preferred(&dirs::home_dir().into_iter().collect::<Vec<_>>());
    let app = app.clone();
    std::thread::Builder::new()
        .name("file-index".into())
        .spawn(move || loop {
            INDEX_RERUN.store(false, Ordering::SeqCst);
            INDEX_CANCEL.store(false, Ordering::SeqCst);
            let (roots, opts) = {
                let st = app.state::<AppState>();
                let s = st.settings.read().unwrap();
                index_options(&s)
            };
            if idx.rebuild(&roots, &opts, &INDEX_CANCEL) {
                let _ = app.emit("index-updated", idx.status());
            }
            if !INDEX_RERUN.load(Ordering::SeqCst) {
                break;
            }
        })
        .is_ok()
}

#[tauri::command]
pub fn index_status(state: State<'_, AppState>) -> gall_core::fs::index::IndexStatus {
    state.index.status()
}

/// `force` rebuilds now; otherwise only when the index is older than a minute.
#[tauri::command]
pub fn index_refresh(app: AppHandle, state: State<'_, AppState>, force: bool) -> bool {
    index_rebuild_bg(&app, &state, if force { 0 } else { 60 })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeInfo {
    /// The system index holds the folder.
    covered: bool,
    /// Not covered: Quick Open's own scan of it is still running.
    scanning: bool,
    /// Not covered: items found by that scan.
    entries: usize,
}

/// Whether the system index holds `path`. When it does not (WSL, a network share),
/// the folder scan starts right away, so results are ready by the time one types.
#[tauri::command]
pub fn index_covers(app: AppHandle, state: State<'_, AppState>, path: String) -> ScopeInfo {
    let roots = index_options(&state.settings.read().unwrap()).0;
    let p = Path::new(&path);
    if gall_core::fs::index::index_covers(&roots, p) {
        return ScopeInfo {
            covered: true,
            scanning: false,
            entries: 0,
        };
    }
    let fi = folder_scan(&app, &state, p);
    ScopeInfo {
        covered: false,
        scanning: fi.is_building() || fi.age().is_none(),
        entries: fi.status().entries,
    }
}

/// Seconds a folder scan is reused before Quick Open scans the folder again.
const FOLDER_SCAN_FRESH_S: u64 = 120;

/// What a folder scan depends on besides the folder: scanned again when it changes.
fn scan_key(o: &gall_core::fs::index::IndexOptions) -> String {
    format!(
        "{}|{}|{}",
        o.include_hidden,
        o.respect_gitignore,
        o.exclude.join("\n")
    )
}

/// The index of a folder the system index does not hold (see [`index_covers`]):
/// reused while fresh and built with the current settings, else scanned again in the
/// background (`index-updated` when done).
fn folder_scan(app: &AppHandle, state: &AppState, root: &Path) -> Arc<FileIndex> {
    let opts = index_options(&state.settings.read().unwrap()).1;
    let key = scan_key(&opts);
    let mut g = state.folder_scan.lock().unwrap();
    if let Some(f) = g.as_ref() {
        let stale = f
            .index
            .age()
            .is_some_and(|a| a.as_secs() >= FOLDER_SCAN_FRESH_S);
        if root.starts_with(&f.root) && !stale && f.key == key && !f.cancel.load(Ordering::Relaxed)
        {
            return f.index.clone();
        }
        f.cancel.store(true, Ordering::Relaxed);
    }
    let index = Arc::new(FileIndex::new());
    let cancel = Arc::new(AtomicBool::new(false));
    *g = Some(crate::state::FolderScan {
        root: root.to_path_buf(),
        index: index.clone(),
        cancel: cancel.clone(),
        key,
    });
    let (app, idx, root) = (app.clone(), index.clone(), root.to_path_buf());
    let _ = std::thread::Builder::new()
        .name("folder-scan".into())
        .spawn(move || {
            // A stopped WSL distribution is started first, as when opening it.
            if gall_core::fs::wsl::prepare(&root).is_err() {
                return;
            }
            idx.rebuild(std::slice::from_ref(&root), &opts, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                let st = app.state::<AppState>();
                let _ = app.emit("index-updated", st.index.status());
            }
        });
    index
}

#[tauri::command]
pub async fn index_query(
    app: AppHandle,
    state: State<'_, AppState>,
    query: String,
    scope: Option<String>,
    limit: usize,
) -> R<gall_core::fs::index::IndexResult> {
    let scope = scope.map(PathBuf::from);
    let roots = index_options(&state.settings.read().unwrap()).0;
    let live = scope
        .as_deref()
        .filter(|s| !query.trim().is_empty() && !gall_core::fs::index::index_covers(&roots, s))
        .map(|s| folder_scan(&app, &state, s));
    let idx = live.clone().unwrap_or_else(|| state.index.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let mut r = idx.query(&query, scope.as_deref(), limit.clamp(1, 500));
        r.scanning = live.is_some() && (idx.is_building() || idx.age().is_none());
        r
    })
    .await
    .map_err(e)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrepBatch {
    id: u64,
    files: Vec<grep::FileMatches>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrepDone {
    id: u64,
    stats: grep::GrepStats,
    cancelled: bool,
    error: Option<String>,
}

#[tauri::command]
pub fn grep_start(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
    root: String,
    pattern: String,
    mut opts: grep::GrepOptions,
) {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(prev) = state.grep_cancel.lock().unwrap().replace(cancel.clone()) {
        prev.store(true, Ordering::Relaxed);
    }
    {
        let s = state.settings.read().unwrap();
        if opts.everything {
            opts.respect_gitignore = false;
        } else {
            opts.exclude.extend(s.index_exclude.iter().cloned());
            opts.respect_gitignore = opts.respect_gitignore && s.search_respect_gitignore;
        }
    }
    std::thread::spawn(move || {
        // Workers push into a channel; this thread batches emits (~10/s).
        let (tx, rx) = crossbeam_channel::unbounded::<grep::FileMatches>();
        let app2 = app.clone();
        let pump = std::thread::spawn(move || {
            let mut batch = Vec::new();
            let mut last = std::time::Instant::now();
            loop {
                match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(f) => batch.push(f),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
                if !batch.is_empty() && (batch.len() >= 100 || last.elapsed().as_millis() >= 100) {
                    let _ = app2.emit(
                        "grep-batch",
                        GrepBatch {
                            id,
                            files: std::mem::take(&mut batch),
                        },
                    );
                    last = std::time::Instant::now();
                }
            }
            if !batch.is_empty() {
                let _ = app2.emit("grep-batch", GrepBatch { id, files: batch });
            }
        });
        let res = grep::grep(Path::new(&root), &pattern, &opts, &cancel, &|f| {
            let _ = tx.send(f);
        });
        drop(tx);
        let _ = pump.join();
        let (stats, error) = match res {
            Ok(s) => (s, None),
            Err(err) => (grep::GrepStats::default(), Some(err)),
        };
        let _ = app.emit(
            "grep-done",
            GrepDone {
                id,
                stats,
                cancelled: cancel.load(Ordering::Relaxed),
                error,
            },
        );
    });
}

#[tauri::command]
pub fn grep_cancel(state: State<'_, AppState>) {
    if let Some(c) = state.grep_cancel.lock().unwrap().take() {
        c.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub async fn grep_context(
    path: String,
    line: u64,
    before: u64,
    after: u64,
) -> R<Vec<(u64, String)>> {
    tauri::async_runtime::spawn_blocking(move || {
        grep::read_context(Path::new(&path), line, before.min(200), after.min(200)).map_err(e)
    })
    .await
    .map_err(e)?
}

// ───────────────────────── tags / destinations ─────────────────────────

#[tauri::command]
pub fn tags_in(state: State<'_, AppState>, dir: String) -> R<Vec<Tagged>> {
    state.db.tags_in(&dir).map_err(e)
}

#[tauri::command]
pub fn set_tag(state: State<'_, AppState>, paths: Vec<String>, color: u8) -> R<()> {
    for p in paths {
        state.db.set_tag(&p, color).map_err(e)?;
    }
    Ok(())
}

#[tauri::command]
pub fn tagged(state: State<'_, AppState>, color: Option<u8>) -> R<Vec<Tagged>> {
    state.db.tagged(color).map_err(e)
}

#[tauri::command]
pub fn destinations(state: State<'_, AppState>) -> R<Vec<String>> {
    state.db.destinations().map_err(e)
}

#[tauri::command]
pub fn clear_destinations(state: State<'_, AppState>) -> R<()> {
    state.db.clear_destinations().map_err(e)
}

// ───────────────────────── clipboard ─────────────────────────

fn system_clipboard() -> R<arboard::Clipboard> {
    arboard::Clipboard::new().map_err(|err| format!("clipboard unavailable: {err}"))
}

/// Ctrl+C / Ctrl+X: remember files internally and put their paths on the
/// system clipboard as text so they can be pasted into other programs.
#[tauri::command]
pub fn clip_set_files(state: State<'_, AppState>, paths: Vec<PathBuf>, cut: bool) -> R<()> {
    let text = paths
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    *state.clipboard.lock().unwrap() = FileClipboard { paths, cut };
    if let Ok(mut c) = system_clipboard() {
        let _ = c.set_text(text);
    }
    Ok(())
}

#[tauri::command]
pub fn clip_get_files(state: State<'_, AppState>) -> FileClipboard {
    state.clipboard.lock().unwrap().clone()
}

#[tauri::command]
pub fn clip_clear_files(state: State<'_, AppState>) {
    *state.clipboard.lock().unwrap() = FileClipboard::default();
}

#[tauri::command]
pub fn clip_set_text(text: String) -> R<()> {
    system_clipboard()?.set_text(text).map_err(e)
}

#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClipContent {
    Empty,
    Text { text: String },
    Url { url: String },
    Paths { paths: Vec<String> },
    Image { width: usize, height: usize },
}

/// Inspects the system clipboard (O-10): what would Ctrl+V do?
#[tauri::command]
pub async fn clip_inspect() -> R<ClipContent> {
    // Reading the clipboard waits for the program that owns it (X11): off the main thread.
    tauri::async_runtime::spawn_blocking(inspect_clipboard)
        .await
        .map_err(e)?
}

/// `file:///home/u/My%20File.txt` → `/home/u/My File.txt` (Windows: `file:///C:/…` → `C:/…`);
/// other text unchanged.
fn file_uri_path(line: &str) -> String {
    let Some(rest) = line.strip_prefix("file://") else {
        return line.to_string();
    };
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let p = String::from_utf8_lossy(&out).into_owned();
    if cfg!(windows) {
        return windows_uri_path(&p);
    }
    p
}

/// The decoded part of a Windows `file://` URI as a native path: `/C:/x` → `C:\x`,
/// `server/share/x` (a host) → `\\server\share\x`.
fn windows_uri_path(p: &str) -> String {
    let b = p.as_bytes();
    let native = if b.len() > 2 && b[0] == b'/' && b[2] == b':' {
        p[1..].to_string()
    } else if !p.starts_with('/') && !p.is_empty() {
        format!("//{p}")
    } else {
        p.to_string()
    };
    native.replace('/', "\\")
}

fn inspect_clipboard() -> R<ClipContent> {
    let mut c = system_clipboard()?;
    if let Ok(t) = c.get_text() {
        // A list of existing absolute paths (one per line) = files from another app.
        let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let paths: Vec<String> = lines
            .iter()
            .map(|l| file_uri_path(l))
            .filter(|l| Path::new(l).is_absolute() && Path::new(l).exists())
            .collect();
        if !paths.is_empty() && paths.len() == lines.len() {
            return Ok(ClipContent::Paths { paths });
        }
        return Ok(match clipname::classify_text(&t) {
            clipname::ClipText::Url(url) => ClipContent::Url { url },
            clipname::ClipText::Path(p) => ClipContent::Paths { paths: vec![p] },
            clipname::ClipText::Text if t.trim().is_empty() => ClipContent::Empty,
            clipname::ClipText::Text => ClipContent::Text { text: t },
        });
    }
    if let Ok(img) = c.get_image() {
        return Ok(ClipContent::Image {
            width: img.width,
            height: img.height,
        });
    }
    Ok(ClipContent::Empty)
}

/// Writes clipboard text / image / URL shortcut into `dir` as a new file.
#[tauri::command]
pub async fn clip_paste_as_file(app: AppHandle, dir: String, as_kind: String) -> R<String> {
    // Encoding a large screenshot as PNG takes a moment: off the main thread.
    off_main(app, move |state| paste_as_file(state, &dir, &as_kind)).await
}

fn paste_as_file(state: &AppState, dir: &str, as_kind: &str) -> R<String> {
    let mut c = system_clipboard()?;
    let now = chrono::Local::now();
    let dirp = Path::new(dir);
    let unique = |ext: &str| ops::unique_path(dirp, &clipname::timestamp_name(now, ext));
    let journal = state.journal();
    let created = match as_kind {
        "image" => {
            let img = c.get_image().map_err(e)?;
            let target = unique("png");
            let buf = image::RgbaImage::from_raw(
                img.width as u32,
                img.height as u32,
                img.bytes.into_owned(),
            )
            .ok_or("invalid image data on clipboard")?;
            buf.save_with_format(&target, image::ImageFormat::Png)
                .map_err(e)?;
            journal.push(ops::UndoRecord::Created {
                paths: vec![target.clone()],
            });
            target
        }
        "url" => {
            let url = c.get_text().map_err(e)?.trim().to_string();
            let (ext, body) = clipname::url_shortcut(&url);
            let name = unique(&ext);
            ops::create_file(
                dirp,
                &name.file_name().unwrap().to_string_lossy(),
                None,
                Some(body.as_bytes()),
                journal,
            )
            .map_err(e)?
        }
        _ => {
            let text = c.get_text().map_err(e)?;
            let name = unique("txt");
            ops::create_file(
                dirp,
                &name.file_name().unwrap().to_string_lossy(),
                None,
                Some(text.as_bytes()),
                journal,
            )
            .map_err(e)?
        }
    };
    Ok(created.to_string_lossy().into_owned())
}

/// Pastes Gall's file clipboard (or external paths) into `dest`.
#[tauri::command]
pub async fn clip_paste_files(
    app: AppHandle,
    dest: PathBuf,
    paths: Option<Vec<PathBuf>>,
    conflict: ConflictPolicy,
    merge: Option<bool>,
) -> R<u64> {
    off_main(app, move |state| {
        let (sources, cut) = match paths {
            Some(p) => (p, false),
            None => {
                let c = state.clipboard.lock().unwrap().clone();
                (c.paths, c.cut)
            }
        };
        if sources.is_empty() {
            return Err("nothing to paste".into());
        }
        let id = start_task_in(
            state,
            TaskRequest {
                kind: if cut { TaskKind::Move } else { TaskKind::Copy },
                sources,
                dest: Some(dest),
                conflict,
                archive_name: None,
                format: None,
                extract_mode: Default::default(),
                trash_archive: false,
                merge: merge.unwrap_or(false),
            },
        )?;
        if cut {
            *state.clipboard.lock().unwrap() = FileClipboard::default();
        }
        Ok(id)
    })
    .await
}

// ───────────────────────── keyboard modifiers ─────────────────────────

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Modifiers {
    /// False when the platform cannot report modifier state.
    known: bool,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

/// Live state of Ctrl/Shift/Alt, read from the windowing system.
///
/// During a drag, WebKitGTK reports the last *negotiated drag action* in
/// `event.ctrlKey/shiftKey` instead of the keys actually held, which makes a
/// pressed modifier "stick". Asking GDK directly gives the real state.
#[tauri::command]
pub async fn modifiers(app: AppHandle) -> Modifiers {
    #[cfg(target_os = "linux")]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let posted = app.run_on_main_thread(move || {
            let state = gdk::Display::default()
                .and_then(|d| gdk::Keymap::for_display(&d))
                .map(|k| k.modifier_state());
            let _ = tx.send(state);
        });
        if posted.is_err() {
            return Modifiers::default();
        }
        let state = tauri::async_runtime::spawn_blocking(move || {
            rx.recv_timeout(std::time::Duration::from_millis(250))
                .ok()
                .flatten()
        })
        .await
        .ok()
        .flatten();
        match state {
            // GDK_SHIFT_MASK = 1, GDK_CONTROL_MASK = 4, GDK_MOD1_MASK (Alt) = 8
            Some(m) => Modifiers {
                known: true,
                shift: m & 1 != 0,
                ctrl: m & 4 != 0,
                alt: m & 8 != 0,
            },
            None => Modifiers::default(),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
        Modifiers::default()
    }
}

/// The running hex comparison: a new one (or closing the view) stops it.
static HEX_CANCEL: std::sync::Mutex<Option<Arc<AtomicBool>>> = std::sync::Mutex::new(None);

/// Where two files differ byte by byte, for the hex view; null when replaced by a newer
/// comparison or cancelled.
#[tauri::command]
pub async fn hex_diff(left: PathBuf, right: PathBuf) -> R<Option<gall_core::fs::hexdiff::HexDiff>> {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(prev) = HEX_CANCEL.lock().unwrap().replace(cancel.clone()) {
        prev.store(true, Ordering::Relaxed);
    }
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::hexdiff::compare(&left, &right, &cancel).map_err(e)
    })
    .await
    .map_err(e)?
}

#[tauri::command]
pub fn hex_diff_cancel() {
    if let Some(c) = HEX_CANCEL.lock().unwrap().take() {
        c.store(true, Ordering::Relaxed);
    }
}

/// Bytes of a file for the hex view (at most 256 KiB from `offset`).
#[tauri::command]
pub async fn read_bytes(path: PathBuf, offset: u64, len: u64) -> R<Vec<u8>> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::hexdiff::read_at(&path, offset, len).map_err(e)
    })
    .await
    .map_err(e)?
}

/// Drags `paths` as real files, so other apps (Explorer, Finder, mail) can take them: the
/// UI cancels the page's own drag, which carries only text, and calls this while the
/// mouse button is still down. Resolves when the drag ends: "dropped" or "cancelled".
/// Windows and macOS; Linux's GTK drag already offers file URIs.
#[tauri::command]
pub async fn start_native_drag(app: AppHandle, paths: Vec<PathBuf>) -> R<String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let win = app.get_webview_window("main").ok_or("no window")?;
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
        let tx2 = tx.clone();
        app.run_on_main_thread(move || {
            const ICON: &[u8] = include_bytes!("../icons/32x32.png");
            let res = drag::start_drag(
                &win,
                drag::DragItem::Files(paths),
                drag::Image::Raw(ICON.to_vec()),
                move |result, _| {
                    let _ = tx2.send(Ok(match result {
                        drag::DragResult::Dropped => "dropped".into(),
                        drag::DragResult::Cancel => "cancelled".into(),
                    }));
                },
                drag::Options::default(),
            );
            if let Err(e) = res {
                let _ = tx.send(Err(e.to_string()));
            }
        })
        .map_err(e)?;
        tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or(Err("drag ended".into())))
            .await
            .map_err(e)?
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (app, paths);
        Err("native drag is not used on this platform".into())
    }
}

/// Installed font families for the font picker (scanned on demand, off the main thread).
#[tauri::command]
pub async fn font_families() -> Vec<gall_core::util::fonts::FontFamily> {
    tauri::async_runtime::spawn_blocking(gall_core::util::fonts::families)
        .await
        .unwrap_or_default()
}

// ───────────────────────── self-test (debug builds) ─────────────────────────

fn selftest_dir() -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var_os("GALL_SELFTEST_DIR").map(PathBuf::from)
}

/// Captures the active window (our window during the self-test) with Spectacle.
#[tauri::command]
pub async fn selftest_shot(name: String) -> R<String> {
    let dir = selftest_dir().ok_or("self-test not enabled")?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let out = dir.join(format!("{safe}.png"));
    let o = out.clone();
    let status = tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("spectacle");
        cmd.args(["-b", "-n", "-a", "-o"]).arg(&o);
        // The app runs on a private D-Bus; Spectacle needs the desktop session bus.
        if let Ok(bus) = std::env::var("GALL_SELFTEST_DBUS") {
            cmd.env("DBUS_SESSION_BUS_ADDRESS", bus);
        }
        // Spectacle occasionally never returns; don't let that hang the self-test.
        let mut child = cmd.spawn()?;
        let t0 = std::time::Instant::now();
        loop {
            if let Some(st) = child.try_wait()? {
                return Ok(st);
            }
            if t0.elapsed() > std::time::Duration::from_secs(10) {
                let _ = child.kill();
                return Err(std::io::Error::other("spectacle timed out"));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    })
    .await
    .map_err(e)?
    .map_err(e)?;
    if !status.success() || !out.exists() {
        return Err(format!("spectacle failed ({status})"));
    }
    Ok(out.to_string_lossy().into_owned())
}

/// Writes the self-test report and quits.
#[tauri::command]
pub fn selftest_done(app: AppHandle, report: String) -> R<()> {
    let dir = selftest_dir().ok_or("self-test not enabled")?;
    std::fs::write(dir.join("report.txt"), report).map_err(e)?;
    app.exit(0);
    Ok(())
}

// ───────────────────────── diagnostics ─────────────────────────

/// Writes a UI message into the application log (errors, DnD diagnostics).
#[tauri::command]
pub fn ui_log(level: String, msg: String) {
    match level.as_str() {
        "error" => log::error!(target: "ui", "{msg}"),
        "warn" => log::warn!(target: "ui", "{msg}"),
        _ => log::info!(target: "ui", "{msg}"),
    }
}

/// Backend warnings and errors logged before the UI started listening for "backend-log".
#[tauri::command]
pub fn log_backlog() -> Vec<crate::UiLogLine> {
    crate::take_log_backlog()
}

// ───────────────────────── startup args ─────────────────────────

#[tauri::command]
pub fn startup_args(args: State<'_, crate::cli::CliArgs>) -> crate::cli::CliArgs {
    args.inner().clone()
}

#[cfg(test)]
mod tests {
    use super::{file_uri_path, windows_uri_path};

    #[test]
    fn windows_file_uris_become_native_paths() {
        assert_eq!(
            windows_uri_path("/C:/Users/u/My File.txt"),
            r"C:\Users\u\My File.txt"
        );
        assert_eq!(
            windows_uri_path("server/share/a.txt"),
            r"\\server\share\a.txt"
        );
        assert_eq!(file_uri_path(r"C:\plain\path"), r"C:\plain\path");
    }

    #[cfg(not(windows))]
    #[test]
    fn file_uris_become_paths() {
        assert_eq!(
            file_uri_path("file:///home/u/My%20File.txt"),
            "/home/u/My File.txt"
        );
        assert_eq!(file_uri_path("file://localhost/tmp/a%25b"), "/tmp/a%b");
        assert_eq!(
            file_uri_path("file:///home/u/%C4%8Crka.txt"),
            "/home/u/Črka.txt"
        );
        assert_eq!(file_uri_path("/plain/path"), "/plain/path");
        assert_eq!(file_uri_path("file:///bad%2"), "/bad%2");
    }
}
