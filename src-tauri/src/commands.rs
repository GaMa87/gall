//! Tauri commands: a thin, typed adapter over `gall-core`.
//! Every command returns `Result<T, String>` so errors surface as rejected
//! promises in the UI and become notifications.

use crate::state::{AppState, FileClipboard};
use gall_core::fs::index::FileIndex;
use gall_core::fs::ops::{self, ConflictPolicy, TaskKind, TaskRequest};
use gall_core::fs::{archive_view, drives, grep, listing, search, FileEntry, ListOptions};
use gall_core::handoff::{self, Outcome};
use gall_core::nav::path::{self as npath, PathFormat};
use gall_core::nav::session::Layout;
use gall_core::nav::Session;
use gall_core::preview::{self, PreviewInfo};
use gall_core::storage::db::{FolderPrefs, ResolvedPrefs, Tagged};
use gall_core::storage::{Favorites, Marks, Settings};
use gall_core::util::{clipname, notes, templates, terminal};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// Holds off application replacement until a synchronous mutation finishes.
fn mutation_guard() -> std::sync::MutexGuard<'static, ()> {
    crate::updates::INSTALL_GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    /// First start (no settings file): show the welcome setup.
    first_run: bool,
    /// URL prefix of the local media server (Linux); `None` means the asset protocol plays media.
    media_base: Option<String>,
    install_dir: String,
    updates_dir: String,
    update_strategy: String,
    install_kind: String,
    base_exe_path: String,
    base_version: String,
    running_overlay: bool,
    tiling_wm: bool,
    /// macOS: whether `~/.Trash` can be listed directly (Full Disk Access granted).
    #[serde(skip_serializing_if = "Option::is_none")]
    macos_full_disk_access: Option<bool>,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    let trash_dir = if cfg!(target_os = "linux") {
        dirs::data_dir().map(|d| d.join("Trash/files").to_string_lossy().into_owned())
    } else {
        None
    };
    #[cfg(target_os = "macos")]
    let macos_full_disk_access = Some(gall_core::fs::mactrash::trash_folder_accessible());
    #[cfg(not(target_os = "macos"))]
    let macos_full_disk_access = None;
    let cap = crate::updates::build_capability(&state.dirs);
    AppInfo {
        version: crate::updates::running_version(),
        platform: std::env::consts::OS.into(),
        home: crate::state::home(),
        separator: std::path::MAIN_SEPARATOR.to_string(),
        config_dir: state.dirs.config.to_string_lossy().into_owned(),
        data_dir: state.dirs.data.to_string_lossy().into_owned(),
        templates_dir: state.dirs.templates().to_string_lossy().into_owned(),
        trash_dir,
        first_run: state.first_run,
        media_base: crate::MEDIA_BASE.get().cloned(),
        install_dir: cap.install_dir.to_string_lossy().into_owned(),
        updates_dir: cap.updates_dir.to_string_lossy().into_owned(),
        update_strategy: cap.strategy.as_str().into(),
        install_kind: cap.install_kind.as_str().into(),
        base_exe_path: cap.base_exe_path.to_string_lossy().into_owned(),
        base_version: cap.base_version,
        running_overlay: cap.running_overlay,
        tiling_wm: crate::omarchy::is_tiling_wm(),
        macos_full_disk_access,
    }
}

/// Shows the named window the first time (windows are created hidden); later calls do nothing.
pub fn reveal_window(app: &tauri::AppHandle, label: &str) {
    let Some(w) = app.get_webview_window(label) else {
        return;
    };
    if let Some(st) = app.try_state::<AppState>() {
        if !st.mark_window_revealed(label) {
            return;
        }
    }
    #[cfg(gall_wayland_presentation)]
    let staged = is_staged_window(&w);
    let _ = w.show();
    let _ = w.set_focus();
    if let Some(st) = app.try_state::<AppState>() {
        st.omarchy.refresh_hyprland_async(app);
    }
    #[cfg(gall_wayland_presentation)]
    if staged {
        // Start the presentation watchdog at map, independent of the bootstrap fallback.
        let window = w.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if is_staged_window(&window) {
                log::warn!(
                    "startup: window presentation watchdog expired ('{}')",
                    window.label()
                );
                finish_staged_window(&window);
            }
        });
    }
    log::info!(
        "startup: window shown at {} ms ('{label}')",
        crate::startup_ms()
    );
}

/// The UI has painted its first frame: show the calling window and mark it ready.
#[tauri::command]
pub fn window_ready(window: tauri::WebviewWindow, app: tauri::AppHandle) -> bool {
    let staged = is_staged_window(&window);
    let label = window.label().to_string();
    if let Some(st) = app.try_state::<AppState>() {
        st.mark_window_ready(&label);
        if let Some(wait) = st.compare_wait_window(&label) {
            if let Err(err) = wait.mark_started() {
                log::warn!("could not acknowledge waiting comparison: {err}");
            }
        }
    }
    reveal_window(&app, &label);
    static MARKED_HEALTHY: AtomicBool = AtomicBool::new(false);
    if !MARKED_HEALTHY.swap(true, Ordering::AcqRel) {
        if let Some(st) = app.try_state::<AppState>() {
            let dirs = st.dirs.clone();
            let ver = crate::updates::running_version();
            std::thread::spawn(move || {
                gall_core::updater::bootstrap::mark_overlay_healthy(&dirs, &ver);
            });
        }
    }
    staged
}

pub(super) fn is_staged_window(window: &tauri::WebviewWindow) -> bool {
    #[cfg(gall_wayland_presentation)]
    return window.title().ok().as_deref() == Some(crate::STARTUP_STAGE_TITLE);
    #[cfg(not(gall_wayland_presentation))]
    {
        let _ = window;
        false
    }
}

#[cfg(gall_wayland_presentation)]
pub(super) fn finish_staged_window(window: &tauri::WebviewWindow) {
    if is_staged_window(window) {
        let _ = window.set_title("Gall");
    }
}

#[tauri::command]
pub fn window_frame_ready(window: tauri::WebviewWindow) {
    #[cfg(gall_wayland_presentation)]
    crate::window_presentation::prepare_first_frame(window);
    #[cfg(not(gall_wayland_presentation))]
    let _ = window;
}

/// Minimizes the calling window, routing through Omarchy's floating-workspace dock script on
/// Hyprland floating workspaces (where `xdg_toplevel.set_minimized` is ignored).
#[tauri::command]
pub fn window_minimize(window: tauri::WebviewWindow) -> R<()> {
    #[cfg(target_os = "linux")]
    {
        let title = window.title().ok();
        if crate::omarchy::minimize_window_on_omarchy_floating(title.as_deref()) {
            return Ok(());
        }
    }
    window.minimize().map_err(e)
}

/// Reports whether the calling window is currently maximized, using Hyprland's authoritative
/// `fullscreen == 1` state on Omarchy floating workspaces.
#[tauri::command]
pub fn window_is_maximized(window: tauri::WebviewWindow) -> R<bool> {
    #[cfg(target_os = "linux")]
    {
        let title = window.title().ok();
        if let Some(maximized) = crate::omarchy::is_maximized_on_omarchy_floating(title.as_deref())
        {
            return Ok(maximized);
        }
    }
    window.is_maximized().map_err(e)
}

/// Toggles maximized state, matching Omarchy's `hyprbars` fullscreen-maximized dispatch on
/// Hyprland floating workspaces and falling back to Tauri's native toggle elsewhere.
#[tauri::command]
pub fn window_toggle_maximize(window: tauri::WebviewWindow) -> R<bool> {
    #[cfg(target_os = "linux")]
    {
        let title = window.title().ok();
        if let Some(maximized) =
            crate::omarchy::toggle_maximize_on_omarchy_floating(title.as_deref())
        {
            return Ok(maximized);
        }
    }
    let was = window.is_maximized().map_err(e)?;
    if was {
        window.unmaximize().map_err(e)?;
    } else {
        window.maximize().map_err(e)?;
    }
    Ok(!was)
}

/// Syncs the native window and WebView backing surface to the active theme canvas colour so
/// resizing on Linux/GTK (e.g. KDE Plasma Wayland/X11) never exposes WebKitGTK's white default
/// background or an unstyled GTK window strip.
#[tauri::command]
pub fn set_window_background_color(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) -> R<()> {
    if cfg!(target_os = "linux") && state.window_is_transparent(window.label()) {
        // tao 0.37's transparent Linux draw handler passes raw u8 RGB channels to Cairo without
        // dividing by 255, turning non-zero RGB values into opaque white. Keep the GTK window's
        // background unset (transparent) and update only the WebKitGTK webview.
        let color = if a == 0 {
            tauri::window::Color(0, 0, 0, 0)
        } else {
            tauri::window::Color(r, g, b, a)
        };
        let webview: &tauri::Webview = window.as_ref();
        webview.set_background_color(Some(color)).map_err(e)?;
    } else {
        window
            .set_background_color(Some(tauri::window::Color(r, g, b, 255)))
            .map_err(e)?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.read().unwrap().clone()
}

/// One writer at a time per settings file (settings, favourites, marks). These commands
/// run on worker threads, because a save is a sync to disk, a re-read and a backup copy;
/// the gate keeps what is on disk the same as what is in memory when two of them overlap.
static STORE_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn store_gate() -> std::sync::MutexGuard<'static, ()> {
    STORE_GATE.lock().unwrap_or_else(|err| err.into_inner())
}

#[tauri::command]
pub async fn set_settings(app: AppHandle, settings: Settings) -> R<()> {
    off_main(app, move |state| {
        let _gate = store_gate();
        state.settings_store.save(&settings).map_err(e)?;
        *state.settings.write().unwrap() = settings;
        Ok(())
    })
    .await
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
pub async fn git_commits(path: PathBuf) -> R<Vec<gall_core::fs::diff::GitCommitInfo>> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::diff::git_commits(&path))
        .await
        .map_err(e)?
        .map_err(e)
}

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

#[tauri::command]
pub async fn diff_page_open(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    id: u64,
    a: gall_core::fs::diff::DiffSource,
    b: gall_core::fs::diff::DiffSource,
    opts: gall_core::fs::diff::DiffOptions,
) -> R<serde_json::Value> {
    use crate::state::CompareJob;
    let label = window.label().to_string();
    let state = app.state::<AppState>();
    let (reply, mut receiver) = tauri::async_runtime::channel(1);
    let (generation, start) = {
        let mut slots = state.compare_slots.lock().unwrap();
        let generation = state
            .window_generation(&label)
            .ok_or("Comparison window closed")?;
        let start = slots.entry(label.clone()).or_default().submit(CompareJob {
            id,
            left: a,
            right: b,
            options: opts,
            reply,
        });
        (generation, start)
    };
    if start {
        tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<AppState>();
            loop {
                let next = {
                    let mut slots = state.compare_slots.lock().unwrap();
                    if state.window_generation(&label) != Some(generation) {
                        return;
                    }
                    slots.get_mut(&label).and_then(|slot| slot.take())
                };
                let Some((job, cancel)) = next else { return };
                let result =
                    gall_core::fs::diff::compare_paged(&job.left, &job.right, job.options, &cancel)
                        .map_err(e);
                let mut slots = state.compare_slots.lock().unwrap();
                if state.window_generation(&label) != Some(generation) {
                    return;
                }
                let Some(slot) = slots.get_mut(&label) else {
                    return;
                };
                if !slot.accepts(job.id, &cancel) {
                    continue;
                }
                let reply = result.map(|result| {
                    let summary = serde_json::json!({
                        "left": result.left,
                        "right": result.right,
                        "binary": result.binary,
                        "limit": result.limit,
                        "text": result.text.as_ref().map(|text| serde_json::json!({
                            "left": text.left, "right": text.right,
                            "added": text.diff().added, "removed": text.diff().removed,
                            "ratio": text.diff().ratio, "onlyCrlfDiffers": text.only_crlf_differs,
                        })),
                    });
                    slot.result = Some((job.id, result));
                    summary
                });
                let _ = job.reply.try_send(reply);
            }
        });
    }
    receiver
        .recv()
        .await
        .unwrap_or_else(|| Err("Comparison superseded".into()))
}

#[tauri::command]
pub fn diff_page_range(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    id: u64,
    kind: &str,
    start: usize,
    count: usize,
) -> R<serde_json::Value> {
    use gall_core::compare::CompareSide;
    let slots = state.compare_slots.lock().unwrap();
    let (_, result) = slots
        .get(window.label())
        .and_then(|slot| slot.result.as_ref())
        .filter(|(cached_id, _)| *cached_id == id)
        .ok_or("Comparison superseded")?;
    let text = result.text.as_ref().ok_or("Text comparison unavailable")?;
    if kind == "small" {
        if !text.is_detailed() {
            return Ok(serde_json::Value::Null);
        }
        let encoded = serde_json::to_vec(text.diff()).map_err(e)?;
        if encoded.len() > gall_core::compare::MAX_PAGE_BYTES - 512 {
            return Ok(serde_json::Value::Null);
        }
        return serde_json::from_slice(&encoded).map_err(e);
    }
    match kind {
        "left" => serde_json::to_value(text.line_page(CompareSide::Left, start, count).map_err(e)?),
        "right" => serde_json::to_value(
            text.line_page(CompareSide::Right, start, count)
                .map_err(e)?,
        ),
        "hunks" => serde_json::to_value(text.hunk_page(start, count).map_err(e)?),
        _ => return Err("Invalid comparison range".into()),
    }
    .map_err(e)
}

#[tauri::command]
pub fn diff_page_close(window: tauri::WebviewWindow, state: State<'_, AppState>, through: u64) {
    let mut slots = state.compare_slots.lock().unwrap();
    if state.window_generation(window.label()).is_some() {
        slots
            .entry(window.label().into())
            .or_default()
            .close(through);
    }
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

/// What changed on each line of a unified patch when its changed lines are compared by
/// tokens: null (no change, or changed as a whole), an empty list (layout only) or the
/// changed ranges. `name` is the file the patch belongs to, for its language.
#[tauri::command]
pub async fn diff_patch_marks(
    name: String,
    patch: String,
) -> R<Vec<Option<gall_core::fs::diff::Spans>>> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::structural::patch_marks(&name, &patch)
    })
    .await
    .map_err(e)
}

/// The patch of `a` → `b` in the chosen format (unified when none is given).
#[tauri::command]
pub async fn diff_patch(
    a: gall_core::fs::diff::DiffSource,
    b: gall_core::fs::diff::DiffSource,
    format: Option<gall_core::fs::patch::Format>,
) -> R<String> {
    let format = format.unwrap_or(gall_core::fs::patch::Format::Unified);
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::diff::export_patch(&a, &b, format))
        .await
        .map_err(e)?
        .map_err(e)
}

/// Writes the patch of `a` → `b` in the chosen format to `dest`.
#[tauri::command]
pub async fn diff_save_patch(
    a: gall_core::fs::diff::DiffSource,
    b: gall_core::fs::diff::DiffSource,
    dest: PathBuf,
    format: Option<gall_core::fs::patch::Format>,
) -> R<()> {
    let format = format.unwrap_or(gall_core::fs::patch::Format::Unified);
    tauri::async_runtime::spawn_blocking(move || -> R<()> {
        let _update_gate = mutation_guard();
        let patch = gall_core::fs::diff::export_patch(&a, &b, format).map_err(e)?;
        std::fs::write(&dest, patch).map_err(e)
    })
    .await
    .map_err(e)?
}

/// What `compare_sessions_load` returns: the saved tabs with what opening each would find.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareSessionsLoaded {
    #[serde(flatten)]
    restored: gall_core::storage::compare_session::Restored,
    /// A file of another version was found: it is left alone until the user saves again.
    foreign_version: bool,
}

/// Reads the saved comparison tabs and checks each of their files (nothing is read from them).
#[tauri::command]
pub fn compare_sessions_load(state: State<'_, AppState>) -> CompareSessionsLoaded {
    let (sessions, foreign_version) =
        gall_core::storage::compare_session::load(state.dirs.compare_sessions());
    CompareSessionsLoaded {
        restored: sessions.restore(),
        foreign_version,
    }
}

/// Saves the comparison tabs. Sides that hold unsaved text arrive as `source: null`.
/// Captures the current file revision for file sides whose revision is unset so a later
/// session can tell whether a file changed on disk since the tab was saved.
#[tauri::command]
pub fn compare_sessions_save(
    state: State<'_, AppState>,
    mut sessions: gall_core::storage::compare_session::CompareSessions,
) -> R<()> {
    for tab in &mut sessions.tabs {
        for side in [&mut tab.left, &mut tab.right] {
            if side.revision.is_none() {
                if let Some(gall_core::fs::diff::DiffSource::File { path }) = &side.source {
                    side.revision = gall_core::fs::ops::file_revision(path);
                }
            }
        }
    }
    gall_core::storage::compare_session::save(state.dirs.compare_sessions(), &sessions).map_err(e)
}

/// Relinks saved comparison tabs from an old folder prefix (`from`) to a new folder (`to`),
/// persists the updated sessions (unless a foreign-version file is present), and returns the
/// refreshed check results.
#[tauri::command]
pub fn compare_sessions_relink(
    state: State<'_, AppState>,
    from: PathBuf,
    to: PathBuf,
    sessions: Option<gall_core::storage::compare_session::CompareSessions>,
) -> R<CompareSessionsLoaded> {
    let (disk, foreign_version) =
        gall_core::storage::compare_session::load(state.dirs.compare_sessions());
    if foreign_version {
        return Err(
            "Saved comparison tabs are from a newer version of Gall and were not modified.".into(),
        );
    }
    let mut target = sessions.unwrap_or(disk);
    let changed = target.relink(&from, &to);
    if changed > 0 {
        gall_core::storage::compare_session::save(state.dirs.compare_sessions(), &target)
            .map_err(e)?;
    }
    Ok(CompareSessionsLoaded {
        restored: target.restore(),
        foreign_version: false,
    })
}

static REPORT_CANCEL: AtomicBool = AtomicBool::new(false);

/// Ends the export started by `compare_report_html` (it writes nothing then).
#[tauri::command]
pub fn compare_report_cancel() {
    REPORT_CANCEL.store(true, Ordering::SeqCst);
}

/// The two formats of a report export.
#[derive(Clone, Copy)]
enum ReportFormat {
    Html,
    Pdf,
}

type ReportPairs = Vec<(
    gall_core::fs::diff::DiffSource,
    gall_core::fs::diff::DiffSource,
)>;

/// Writes a standalone report of the pairs to `dest` in the chosen format, off the UI thread.
async fn export_report(
    format: ReportFormat,
    dest: PathBuf,
    title: String,
    pairs: ReportPairs,
    options: Option<gall_core::fs::diff::DiffOptions>,
) -> R<()> {
    // One export at a time: the cancel flag belongs to it.
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("Another report is being exported. Wait for it or cancel it first.".into());
    }
    REPORT_CANCEL.store(false, Ordering::SeqCst);
    let done = tauri::async_runtime::spawn_blocking(move || {
        let _update_gate = mutation_guard();
        // The window compares with line endings ignored unless told otherwise.
        let options = options.unwrap_or(gall_core::fs::diff::DiffOptions {
            ignore_eol: true,
            ..Default::default()
        });
        match format {
            ReportFormat::Html => gall_core::fs::compare_report::export_html(
                &dest,
                &title,
                &pairs,
                &options,
                &REPORT_CANCEL,
            ),
            ReportFormat::Pdf => gall_core::fs::compare_pdf::export_pdf(
                &dest,
                &title,
                &pairs,
                &options,
                &REPORT_CANCEL,
            ),
        }
    })
    .await;
    RUNNING.store(false, Ordering::SeqCst);
    done.map_err(e)?.map_err(e)
}

/// Writes a standalone HTML report of the pairs to `dest`, off the UI thread.
#[tauri::command]
pub async fn compare_report_html(
    dest: PathBuf,
    title: String,
    pairs: ReportPairs,
    options: Option<gall_core::fs::diff::DiffOptions>,
) -> R<()> {
    export_report(ReportFormat::Html, dest, title, pairs, options).await
}

/// Writes a standalone PDF report of the pairs to `dest` (Latin-1 text in built-in fonts, see `compare_pdf`).
#[tauri::command]
pub async fn compare_report_pdf(
    dest: PathBuf,
    title: String,
    pairs: ReportPairs,
    options: Option<gall_core::fs::diff::DiffOptions>,
) -> R<()> {
    export_report(ReportFormat::Pdf, dest, title, pairs, options).await
}

/// The number of the latest syntax-tree comparison; a newer request or a cancel makes an older one stop.
static SEMANTIC_GENERATION: AtomicU64 = AtomicU64::new(0);
/// The same for the outline: it has its own counter, so that reading an outline never cancels a comparison.
static OUTLINE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Compares two sources of one file by their syntax trees (`name` gives the language). When `left_path` or
/// `right_path` is provided, that side is read verbatim from disk so bytes like CR inside string literals are
/// preserved instead of lost in display-line joining. Runs off the UI thread and stops when a newer request or
/// `semantic_compare_cancel` comes.
#[tauri::command]
pub async fn semantic_compare(
    name: String,
    left: Option<String>,
    right: Option<String>,
    left_path: Option<String>,
    right_path: Option<String>,
) -> R<gall_core::semantic_compare::Verdict> {
    let mine = SEMANTIC_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let left_input = match left_path {
        Some(p) if !p.is_empty() => {
            gall_core::semantic_compare::SourceInput::File(PathBuf::from(p))
        }
        _ => gall_core::semantic_compare::SourceInput::Text(left.unwrap_or_default()),
    };
    let right_input = match right_path {
        Some(p) if !p.is_empty() => {
            gall_core::semantic_compare::SourceInput::File(PathBuf::from(p))
        }
        _ => gall_core::semantic_compare::SourceInput::Text(right.unwrap_or_default()),
    };
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::semantic_compare::compare_sources(
            left_input,
            right_input,
            gall_core::semantic_compare::language_of(&name),
            &|| SEMANTIC_GENERATION.load(Ordering::SeqCst) != mine,
        )
    })
    .await
    .map_err(e)
}

/// Requests of the local explanation that were cancelled, by the id the window gave them. An id is cancelled for good,
/// so the order of a cancel and its request does not matter; the request removes its id when it ends.
static AI_CANCELLED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<u64>>> =
    std::sync::LazyLock::new(Default::default);
static AI_CACHE: std::sync::OnceLock<gall_core::compare_ai::Cache> = std::sync::OnceLock::new();

/// Asks the local model the user has configured (`compare-ai.json`, off by default) to explain the changes between
/// two texts. Only called when the user asks for it: nothing is started otherwise, and without opt-in the answer is
/// `disabled`. Runs off the UI thread; `compare_ai_cancel` with the same `request_id` kills the program and its
/// children.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn compare_ai_explain(
    state: State<'_, AppState>,
    request_id: u64,
    left_label: String,
    right_label: String,
    left: String,
    right: String,
    options: Option<gall_core::fs::diff::DiffOptions>,
    scope: Option<gall_core::compare_ai::ExplainScope>,
) -> R<gall_core::compare_ai::Outcome> {
    let config = gall_core::compare_ai::load_config(&state.dirs.compare_ai());
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        // The texts of both sides must fit a request before they are compared at all.
        if left.len() > gall_core::compare_ai::MAX_REQUEST_CHARS * 4
            || right.len() > gall_core::compare_ai::MAX_REQUEST_CHARS * 4
        {
            return gall_core::compare_ai::Outcome::TooLarge;
        }
        let all_hunks = gall_core::compare_ai::hunks_of_with_options(
            &left,
            &right,
            &left_label,
            &right_label,
            options.unwrap_or_default(),
        );
        let hunks = match scope {
            Some(ref s) => gall_core::compare_ai::scope_hunks(&all_hunks, s),
            None => all_hunks,
        };
        gall_core::compare_ai::explain(
            &config,
            AI_CACHE.get_or_init(Default::default),
            &left_label,
            &right_label,
            &hunks,
            &|| {
                AI_CANCELLED
                    .lock()
                    .is_ok_and(|set| set.contains(&request_id))
            },
        )
    })
    .await
    .map_err(e);
    if let Ok(mut set) = AI_CANCELLED.lock() {
        set.remove(&request_id);
    }
    outcome
}

/// The settings of the local explanation model (`compare-ai.json`), as the file holds them.
#[tauri::command]
pub fn compare_ai_config_get(state: State<'_, AppState>) -> gall_core::compare_ai::Config {
    gall_core::compare_ai::load_config(&state.dirs.compare_ai())
}

/// Saves the settings of the local explanation model. The program must be an absolute path (or empty while the model
/// is off) and the deadline between 1 second and 10 minutes; nothing is run here.
#[tauri::command]
pub fn compare_ai_config_save(
    state: State<'_, AppState>,
    config: gall_core::compare_ai::Config,
) -> R<()> {
    if !config.program.as_os_str().is_empty() && !config.program.is_absolute() {
        return Err("The program must be an absolute path (no search of PATH, no shell).".into());
    }
    if config.enabled && config.program.as_os_str().is_empty() {
        return Err("Choose the program before turning the model on.".into());
    }
    if !(1..=600).contains(&config.deadline_secs) {
        return Err("The deadline is from 1 second to 600 seconds.".into());
    }
    gall_core::storage::JsonStore::new(state.dirs.compare_ai())
        .save(&config)
        .map_err(e)
}

/// The revision of a conflict's text: a proposal is asked for and accepted only against this exact text.
#[tauri::command]
pub fn merge_ai_revision(conflict: gall_core::compare_ai::ConflictInput) -> String {
    gall_core::compare_ai::conflict_revision(&conflict)
}

/// Asks the local model (opt-in, see `compare_ai_explain`) for lines that replace one merge conflict. Nothing is
/// applied here: the answer is a proposal to preview, and accepting it, saving and staging stay the user's separate
/// actions. `seen_revision` is the revision the user is looking at; another one is `stale` and starts nothing.
#[tauri::command]
pub async fn merge_ai_propose(
    state: State<'_, AppState>,
    request_id: u64,
    conflict: gall_core::compare_ai::ConflictInput,
    seen_revision: String,
    syntax: Option<gall_core::compare_ai::SyntaxCheck>,
) -> R<gall_core::compare_ai::MergeOutcome> {
    let config = gall_core::compare_ai::load_config(&state.dirs.compare_ai());
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        gall_core::compare_ai::propose_merge(
            &config,
            AI_CACHE.get_or_init(Default::default),
            &conflict,
            &seen_revision,
            syntax.as_ref(),
            &|| {
                AI_CANCELLED
                    .lock()
                    .is_ok_and(|set| set.contains(&request_id))
            },
        )
    })
    .await
    .map_err(e);
    if let Ok(mut set) = AI_CANCELLED.lock() {
        set.remove(&request_id);
    }
    outcome
}

/// Ends the explanation with this id.
#[tauri::command]
pub fn compare_ai_cancel(request_id: u64) {
    if let Ok(mut set) = AI_CANCELLED.lock() {
        set.insert(request_id);
    }
}

/// The structural outline (modules, classes, structs, functions) of one source; `name` gives the language. The
/// answer is the nodes, or a verdict of `unknown` (unsupported language, parse error, too large) or `cancelled`.
#[tauri::command]
pub async fn semantic_outline(
    name: String,
    text: String,
    right: Option<bool>,
) -> R<Result<gall_core::semantic_compare::Outline, gall_core::semantic_compare::Verdict>> {
    let mine = OUTLINE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::semantic_compare::outline_cut_of(
            &text,
            gall_core::semantic_compare::language_of(&name),
            if right.unwrap_or(false) {
                gall_core::semantic_compare::Side::Right
            } else {
                gall_core::semantic_compare::Side::Left
            },
            &|| OUTLINE_GENERATION.load(Ordering::SeqCst) != mine,
        )
    })
    .await
    .map_err(e)
}

/// Whether a file is generated (lockfile, build output folder, generated header, minified bundle, `.gitattributes`,
/// or a user-configured extra pattern), with the reason. Presentation only: it never changes what is staged,
/// exported, merged or synchronized.
#[tauri::command]
pub fn generated_classify(
    state: State<'_, AppState>,
    path: String,
    text: String,
) -> Option<gall_core::generated_compare::Generated> {
    let extra = state
        .settings
        .read()
        .map(|s| s.generated_patterns.clone())
        .unwrap_or_default();
    gall_core::generated_compare::classify_in_repo_with_patterns(&path, &text, &extra)
}

/// Stops the running syntax-tree comparison.
#[tauri::command]
pub fn semantic_compare_cancel() {
    SEMANTIC_GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// One file of a patch preview: where it would be written and what would change there.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPreviewFile {
    path: String,
    name: String,
    hunks: usize,
    /// For each hunk, how many lines it was found away from the line the patch states.
    offsets: Vec<isize>,
}

/// What importing a patch would do. Nothing is written. A non-empty `problems` means that nothing can be applied.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPreview {
    files: Vec<PatchPreviewFile>,
    problems: Vec<String>,
    /// A fingerprint of what the patch would write (the files and their new text): `patch_apply` refuses to write when
    /// the plan it makes now is another one, so the user applies what was checked.
    fingerprint: String,
}

fn plan_fingerprint(plan: &gall_core::fs::patch::Plan) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for file in &plan.files {
        (&file.path, &file.new_text, file.hunks, &file.offsets).hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

const MAX_PATCH_TEXT: usize = 32 * 1024 * 1024;

fn patch_plan(
    root: &std::path::Path,
    text: &str,
    format: gall_core::fs::patch::Format,
    strip: Option<usize>,
    target: Option<&str>,
) -> R<Result<gall_core::fs::patch::Plan, Vec<String>>> {
    if text.len() > MAX_PATCH_TEXT {
        return Err(format!(
            "The patch is larger than {} MiB",
            MAX_PATCH_TEXT / (1024 * 1024)
        ));
    }
    let mut patches = gall_core::fs::patch::parse(format, text).map_err(e)?;
    // A normal diff carries no file name: the target the user chose, relative to the folder, is the file it is for.
    if let Some(target) = target.map(str::trim).filter(|t| !t.is_empty()) {
        for patch in patches
            .iter_mut()
            .filter(|p| p.old_name.is_none() && p.new_name.is_none())
        {
            patch.old_name = Some(target.to_owned());
        }
    }
    if patches.iter().all(|p| p.hunks.is_empty()) {
        return Err(
            "The patch has no changes in this format; check the format that is selected".into(),
        );
    }
    Ok(gall_core::fs::patch::plan(root, &patches, strip))
}

/// The format a patch text is written in, from its own marks (`---`/`+++` and `@@`, `***` and `---`, or `1c1`), or none.
#[tauri::command]
pub fn patch_sniff(text: String) -> Option<gall_core::fs::patch::Format> {
    gall_core::fs::patch::sniff(&text)
}

/// Checks a patch against the files below `root` and says what it would do; writes nothing.
#[tauri::command]
pub async fn patch_preview(
    root: PathBuf,
    text: String,
    format: gall_core::fs::patch::Format,
    strip: Option<usize>,
    target: Option<String>,
) -> R<PatchPreview> {
    tauri::async_runtime::spawn_blocking(move || -> R<PatchPreview> {
        Ok(
            match patch_plan(&root, &text, format, strip, target.as_deref())? {
                Ok(plan) => PatchPreview {
                    files: plan
                        .files
                        .iter()
                        .map(|f| PatchPreviewFile {
                            path: f.path.to_string_lossy().into_owned(),
                            name: f.name.clone(),
                            hunks: f.hunks,
                            offsets: f.offsets.clone(),
                        })
                        .collect(),
                    problems: Vec::new(),
                    fingerprint: plan_fingerprint(&plan),
                },
                Err(problems) => PatchPreview {
                    files: Vec::new(),
                    problems,
                    fingerprint: String::new(),
                },
            },
        )
    })
    .await
    .map_err(e)?
}

/// Applies a patch below `root`: every file is checked first, each write is atomic, and if one fails the files already
/// written are put back. Returns the files written.
#[tauri::command]
pub async fn patch_apply(
    root: PathBuf,
    text: String,
    format: gall_core::fs::patch::Format,
    strip: Option<usize>,
    target: Option<String>,
    fingerprint: String,
) -> R<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || -> R<Vec<String>> {
        let _update_gate = mutation_guard();
        let plan = patch_plan(&root, &text, format, strip, target.as_deref())?
            .map_err(|problems| problems.join("\n"))?;
        // The user applies what was checked: another plan now (a file changed since the check) is refused, nothing written.
        if fingerprint != plan_fingerprint(&plan) {
            return Err(
                "The patch or the files changed since it was checked; check it again before applying".into(),
            );
        }
        match gall_core::fs::patch::apply_plan(&plan) {
            Ok(written) => Ok(written
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()),
            Err(failure) => {
                let mut message = format!("{}: {}", failure.failed.display(), failure.error);
                if !failure.restored.is_empty() {
                    message.push_str(&format!(
                        "\nPut back as they were: {}",
                        failure
                            .restored
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if !failure.not_restored.is_empty() {
                    message.push_str(&format!(
                        "\nCould NOT be put back and hold the patched text: {}",
                        failure
                            .not_restored
                            .iter()
                            .map(|(p, why)| format!("{} ({why})", p.display()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                Err(message)
            }
        }
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

/// The fingerprints of the folders a plan copies or trashes as a whole, taken when the plan is shown; `sync_apply`
/// refuses a folder that holds other entries by then (see `fs::ops::tree_fingerprint`).
#[tauri::command]
pub async fn sync_tree_fingerprints(paths: Vec<PathBuf>) -> R<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .iter()
            .map(|path| gall_core::fs::ops::tree_fingerprint(path))
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// The fingerprints (size and time to the nanosecond) of the files a plan reads or replaces, taken when the plan is
/// shown; `sync_apply` refuses a file that changed in between even within the same second.
#[tauri::command]
pub async fn sync_file_fingerprints(paths: Vec<PathBuf>) -> R<Vec<Option<String>>> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .iter()
            .map(|path| gall_core::fs::ops::file_fingerprint(path))
            .collect()
    })
    .await
    .map_err(e)
}

/// Applies a folder sync (copies + trash) as one undoable operation.
#[tauri::command]
pub async fn sync_apply(
    state: State<'_, AppState>,
    ops: Vec<gall_core::fs::ops::SyncOp>,
    checks: Option<Vec<gall_core::fs::ops::SyncCheck>>,
) -> R<gall_core::fs::ops::SyncReport> {
    // A plan that carries no checks of its files is not run: the files were never compared against the preview.
    let checks = match checks {
        Some(checks) => checks,
        None if ops.is_empty() => Vec::new(),
        None => {
            return Err(
                "this sync carries no check of its files against the comparison; compare again"
                    .into(),
            )
        }
    };
    let cancel = new_dircmp_cancel(&state);
    let journal = state.journal().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _update_gate = mutation_guard();
        gall_core::fs::ops::apply_sync_checked(&ops, &checks, &journal, &cancel)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Saves text edited in the compare view (previous version to trash, one undo step).
#[tauri::command]
/// Returns whether the save can be undone (false where the old version could not be trashed).
pub async fn save_text(
    state: State<'_, AppState>,
    path: PathBuf,
    text: String,
    expected: Option<String>,
    expect_missing: Option<bool>,
) -> R<bool> {
    save_text_with_undo(state, path, text, expected, expect_missing)
        .await
        .map(|r| r.undoable)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveTextResult {
    pub undoable: bool,
    pub undo_id: Option<u64>,
}

#[tauri::command]
pub async fn save_text_with_undo(
    state: State<'_, AppState>,
    path: PathBuf,
    text: String,
    expected: Option<String>,
    expect_missing: Option<bool>,
) -> R<SaveTextResult> {
    let journal = state.journal().clone();
    let expect_missing = expect_missing.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let _update_gate = mutation_guard();
        ops::save_text_recorded(&path, &text, expected.as_deref(), expect_missing, &journal)
            .map(|(undoable, undo_id)| SaveTextResult { undoable, undo_id })
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Literal, canonical filename and the revision a later save is checked against.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFileVersion {
    canonical_path: PathBuf,
    /// Size and modification time when the file was read, to check a later save against.
    revision: Option<String>,
}

#[derive(Serialize)]
pub struct TextFile {
    text: String,
    #[serde(flatten)]
    version: TextFileVersion,
}

/// Looks up an existing output without expanding whitespace, variables, or other filename bytes.
/// An absent target is distinct from a target that cannot be inspected.
#[tauri::command]
pub async fn text_file_version(path: PathBuf) -> R<Option<TextFileVersion>> {
    tauri::async_runtime::spawn_blocking(move || -> R<Option<TextFileVersion>> {
        let canonical_path = match npath::canonicalize(&path) {
            Ok(path) => path,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(e(err)),
        };
        let meta = std::fs::metadata(&canonical_path).map_err(e)?;
        if !meta.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        Ok(Some(TextFileVersion {
            canonical_path,
            revision: ops::file_revision_of(&meta),
        }))
    })
    .await
    .map_err(e)?
}

/// Reads one regular UTF-8 text file up to the size a merge accepts. A pipe, a file with a NUL byte in its first 8 KiB
/// (binary), a file that is not UTF-8 and a too large file are refused with the reason.
#[tauri::command]
pub async fn read_text_file(path: PathBuf) -> R<TextFile> {
    tauri::async_runtime::spawn_blocking(move || -> R<TextFile> {
        use std::io::Read;
        let limit = gall_core::merge::MAX_STAGE_BYTES;
        let canonical_path = npath::canonicalize(&path).map_err(e)?;
        let file = gall_core::fs::open_regular(&canonical_path).map_err(e)?;
        // Capture the revision of this very handle, before reading any text.
        let revision = ops::file_revision_of(&file.metadata().map_err(e)?);
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(e)?;
        if bytes.len() > limit {
            return Err(format!(
                "{} is larger than {} MiB",
                path.display(),
                limit / (1024 * 1024)
            ));
        }
        if bytes[..bytes.len().min(8192)].contains(&0) {
            return Err(format!("{} is a binary file", path.display()));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("{} is not UTF-8 text", path.display()))?;
        Ok(TextFile {
            text,
            version: TextFileVersion {
                canonical_path,
                revision,
            },
        })
    })
    .await
    .map_err(e)?
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
pub async fn eject(app: AppHandle, mount: String, power_off: Option<bool>) -> R<()> {
    off_main(app, move |state| {
        // Folders on the drive that are still watched (a column that has not closed yet)
        // would keep it busy: let go of them first, back if the eject fails.
        let released = state.watcher.release_under(Path::new(&mount));
        let res = if power_off.unwrap_or(false) {
            drives::eject_and_power_off(&mount)
        } else {
            drives::eject(&mount)
        }
        .map_err(e);
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

/// Powers a removable drive off so it can be unplugged ("Not mounted" → safely remove).
#[tauri::command]
pub async fn volume_power_off(id: String) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::volumes::power_off_volume(&id))
        .await
        .map_err(e)?
        .map_err(e)
}

/// Clears the dirty flag of an NTFS volume (polkit prompt) and mounts it; returns where.
#[tauri::command]
pub async fn volume_repair_mount(id: String) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::volumes::repair_mount_volume(&id))
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
pub async fn reset_settings(app: AppHandle) -> R<Settings> {
    off_main(app, move |state| {
        let _gate = store_gate();
        let settings = Settings::default();
        state.settings_store.save(&settings).map_err(e)?;
        *state.settings.write().unwrap() = settings.clone();
        state.db.clear_prefs().map_err(e)?;
        Ok(settings)
    })
    .await
}

#[tauri::command]
pub async fn resolve_prefs(app: AppHandle, path: String) -> R<ResolvedPrefs> {
    off_main(app, move |state| {
        // A copy, so a slow database never holds the settings lock.
        let s = state.settings.read().unwrap().clone();
        if !s.remember_folder_views {
            return Ok(ResolvedPrefs {
                path,
                sort_key: s.default_sort,
                descending: s.default_descending,
                group: s.group,
                group_order: s.group_order,
                view: s.default_view.clone(),
                view_source: None,
                group_source: None,
                collapsed: Vec::new(),
            });
        }
        state
            .db
            .resolve_prefs(
                Path::new(&path),
                s.default_sort,
                s.default_descending,
                s.group,
                s.group_order,
                &s.default_view,
            )
            .map_err(e)
    })
    .await
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

/// Another file manager whose places can be imported (see `gall_core::storage::favorites_import`).
#[derive(Serialize)]
pub struct ImportSource {
    id: String,
    label: String,
}

/// A place read from another manager, with whether it is already a favorite.
#[derive(Serialize)]
pub struct ImportPlace {
    #[serde(flatten)]
    place: gall_core::storage::favorites_import::Place,
    duplicate: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    favorites: Favorites,
    receipt: gall_core::storage::favorites_import::ImportReceipt,
}

/// Which sources exist on this computer. Only asks whether their files exist; nothing is read, and nothing
/// is called at startup: the Welcome step and the Settings dialog ask when they open.
#[tauri::command]
pub async fn favorites_import_detect() -> R<Vec<ImportSource>> {
    tauri::async_runtime::spawn_blocking(|| {
        gall_core::storage::favorites_import::all_sources()
            .iter()
            .filter(|source| source.detect())
            .map(|source| ImportSource {
                id: source.id().to_owned(),
                label: source.label().to_owned(),
            })
            .collect()
    })
    .await
    .map_err(e)
}

/// The places of one detected source, read-only, with the ones that are already favorites marked.
#[tauri::command]
pub async fn favorites_import_read(app: AppHandle, id: String) -> R<Vec<ImportPlace>> {
    off_main(app, move |state| {
        let source = gall_core::storage::favorites_import::all_sources()
            .into_iter()
            .find(|source| source.id() == id)
            .ok_or_else(|| format!("Unknown source {id}"))?;
        let places = source.read().map_err(e)?;
        let favorites = state.favorites.lock().unwrap();
        Ok(places
            .into_iter()
            .map(|place| ImportPlace {
                duplicate: gall_core::storage::favorites_import::already_in(
                    &favorites,
                    &place.path,
                ),
                place,
            })
            .collect())
    })
    .await
}

/// Adds the ticked places, one new group per source. The receipt takes the whole batch back (`favorites_import_undo`).
#[tauri::command]
pub async fn favorites_import_apply(
    app: AppHandle,
    choices: Vec<gall_core::storage::favorites_import::ImportChoice>,
) -> R<ImportOutcome> {
    off_main(app, move |state| {
        let _gate = store_gate();
        let (favorites, receipt) = {
            let mut f = state.favorites.lock().unwrap();
            let receipt = gall_core::storage::favorites_import::apply_import(&mut f, &choices);
            (f.clone(), receipt)
        };
        if !receipt.groups.is_empty() {
            state.save_favorites();
        }
        Ok(ImportOutcome { favorites, receipt })
    })
    .await
}

/// Takes back an import: removes exactly the places it added, and the groups that are then empty.
#[tauri::command]
pub async fn favorites_import_undo(
    app: AppHandle,
    receipt: gall_core::storage::favorites_import::ImportReceipt,
) -> R<Favorites> {
    off_main(app, move |state| {
        let _gate = store_gate();
        let favorites = {
            let mut f = state.favorites.lock().unwrap();
            gall_core::storage::favorites_import::undo_import(&mut f, &receipt);
            f.clone()
        };
        state.save_favorites();
        Ok(favorites)
    })
    .await
}

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
    /// Show or hide hidden files in one browser (`b = Some`), or sync all browsers (`b = None`).
    Hidden {
        b: Option<usize>,
        on: Option<bool>,
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
    /// Navigate only one browser without triggering Tandem mirroring (e.g. Open parent recovery).
    NavigateSolo {
        b: usize,
        path: String,
    },
    /// Pair a left tab and a right tab with a semantic bridge mode (`linked` / `tandem` / `flow`).
    PairTabs {
        left_idx: usize,
        right_idx: usize,
        mode: gall_core::nav::PairMode,
        flow_dir: Option<gall_core::nav::FlowDirection>,
        flow_op: Option<gall_core::nav::FlowOp>,
    },
    /// Unlink an existing tab pair by ID.
    Unpair {
        pair_id: u64,
    },
    /// Change the bridge mode / Flow direction / Flow operation of an existing pair.
    PairMode {
        pair_id: u64,
        mode: gall_core::nav::PairMode,
        flow_dir: Option<gall_core::nav::FlowDirection>,
        flow_op: Option<gall_core::nav::FlowOp>,
    },
    /// Re-anchor a Tandem pair's roots to the two tabs' current paths.
    PairReanchor {
        pair_id: u64,
    },
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WorkspaceCmd {
    Switch {
        idx: usize,
    },
    Create {
        name: Option<String>,
        accent: Option<String>,
        clone_current: Option<bool>,
    },
    Duplicate {
        idx: usize,
    },
    Rename {
        idx: usize,
        name: Option<String>,
    },
    Accent {
        idx: usize,
        accent: Option<String>,
    },
    Reorder {
        from: usize,
        to: usize,
    },
    Close {
        idx: usize,
    },
}

#[tauri::command]
pub fn workspaces_get(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    window_id: Option<String>,
) -> gall_core::nav::WorkspaceSnapshot {
    let id = window_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| window.label());
    state.workspace_snapshot_for(id)
}

#[tauri::command]
pub fn workspace_cmd(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    cmd: WorkspaceCmd,
    window_id: Option<String>,
    source_workspace_id: Option<u64>,
    expected_workspace_ids: Option<Vec<u64>>,
) -> R<gall_core::nav::WorkspaceSnapshot> {
    let id = window_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| window.label())
        .to_string();
    let (default_left, default_right) = {
        let settings = state.settings.read().unwrap();
        let h = crate::state::home();
        let left = settings
            .startup_paths
            .first()
            .map(|p| npath::expand(p))
            .unwrap_or_else(|| h.clone());
        let right = settings
            .startup_paths
            .get(1)
            .map(|p| npath::expand(p))
            .unwrap_or(h);
        (left, right)
    };
    let snapshot = state.with_workspaces_mut(&id, |ws| {
        if let Some(expected_ids) = expected_workspace_ids.as_ref() {
            let actual_ids: Vec<_> = ws.workspaces.iter().map(|workspace| workspace.id).collect();
            if &actual_ids != expected_ids {
                return Err("Workspace list changed before the command was applied".to_string());
            }
        }
        match cmd {
            WorkspaceCmd::Switch { idx } => {
                ws.switch(idx);
            }
            WorkspaceCmd::Create {
                name,
                accent,
                clone_current,
            } => {
                let (left, right) = if clone_current.unwrap_or(false) {
                    let cur = match source_workspace_id {
                        Some(source_id) => ws
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.id == source_id)
                            .ok_or_else(|| "Source workspace no longer exists".to_string())?,
                        None => ws.active_workspace(),
                    };
                    let cur = &cur.session;
                    (
                        cur.active_tab(0).path.clone(),
                        cur.active_tab(1).path.clone(),
                    )
                } else {
                    (default_left.clone(), default_right.clone())
                };
                ws.create(&left, &right, name, accent, true);
            }
            WorkspaceCmd::Duplicate { idx } => {
                ws.duplicate(idx, true);
            }
            WorkspaceCmd::Rename { idx, name } => {
                ws.rename(idx, name);
            }
            WorkspaceCmd::Accent { idx, accent } => {
                ws.set_accent(idx, accent);
            }
            WorkspaceCmd::Reorder { from, to } => {
                ws.reorder(from, to);
            }
            WorkspaceCmd::Close { idx } => {
                ws.close(idx);
            }
        }
        Ok(ws.snapshot())
    });
    let snapshot = snapshot?;
    state.save_session_for(&id);
    Ok(snapshot)
}

#[tauri::command]
pub fn session_get(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    window_id: Option<String>,
) -> Session {
    let id = window_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| window.label());
    state.session_for(id)
}

#[tauri::command]
pub fn session_cmd(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    cmd: SessionCmd,
    window_id: Option<String>,
    workspace_id: Option<u64>,
) -> R<Session> {
    let id = window_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| window.label())
        .to_string();
    let (placement, new_tab_path, history_count) = {
        let s = state.settings.read().unwrap();
        (s.new_tab_placement, s.new_tab_path.clone(), s.history_count)
    };
    let mut visited: Option<String> = None;
    let mut paired_paths: Option<(String, String)> = None;
    let snapshot = state
        .with_session_for_mut(&id, workspace_id, |s| {
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
                SessionCmd::Hidden { b, on } => match b {
                    Some(idx) => {
                        if let Some(br) = s.browsers.get_mut(idx) {
                            br.show_hidden = on;
                        }
                    }
                    None => {
                        for br in &mut s.browsers {
                            br.show_hidden = on;
                        }
                    }
                },
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
                        v.group = prefs.group.or(v.group);
                        v.group_order = prefs.group_order.or(v.group_order);
                        v.view = prefs.view.or(v.view.take());
                    }
                }
                SessionCmd::Swap => s.swap_active_tabs(),
                SessionCmd::NavigateSolo { b, path } => {
                    if s.navigate_solo(b, &path) {
                        visited = Some(path);
                    }
                }
                SessionCmd::PairTabs {
                    left_idx,
                    right_idx,
                    mode,
                    flow_dir,
                    flow_op,
                } => {
                    if s.pair_tabs(left_idx, right_idx, mode, flow_dir, flow_op)
                        .is_some()
                    {
                        let lp = s.active_tab(0).path.clone();
                        let rp = s.active_tab(1).path.clone();
                        paired_paths = Some((lp, rp));
                    }
                }
                SessionCmd::Unpair { pair_id } => {
                    s.unpair(pair_id);
                }
                SessionCmd::PairMode {
                    pair_id,
                    mode,
                    flow_dir,
                    flow_op,
                } => {
                    s.set_pair_mode(pair_id, mode, flow_dir, flow_op);
                }
                SessionCmd::PairReanchor { pair_id } => {
                    s.reanchor_pair(pair_id);
                }
            }
            s.clone()
        })
        .ok_or_else(|| "Requested workspace no longer exists".to_string())?;
    if let Some(p) = visited {
        if let Err(err) = state.db.record_visit(&p, history_count) {
            log::warn!("history: {err}");
        }
    }
    if let Some((a, b)) = paired_paths {
        if let Err(err) = state.db.record_gravity(&a, &b, 2.5, true) {
            log::warn!("gravity: {err}");
        }
    }
    state.save_session_for(&id);
    Ok(snapshot)
}

#[tauri::command]
pub async fn gravity_suggest(
    app: AppHandle,
    path: String,
    opposite_paths: Vec<String>,
) -> R<Option<gall_core::storage::GravitySuggestion>> {
    off_main(app, move |state| {
        let now = chrono::Utc::now().timestamp_millis();
        gall_core::storage::gravity::suggest_partner(&state.db, &path, &opposite_paths, now)
            .map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn gravity_record(
    app: AppHandle,
    path_a: String,
    path_b: String,
    weight: f64,
    unsuppress: Option<bool>,
) -> R<()> {
    off_main(app, move |state| {
        let clamped = weight.clamp(0.1, 10.0);
        state
            .db
            .record_gravity(&path_a, &path_b, clamped, unsuppress.unwrap_or(false))
            .map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn gravity_dismiss(app: AppHandle, path_a: String, path_b: String) -> R<()> {
    off_main(app, move |state| {
        state.db.suppress_gravity(&path_a, &path_b).map_err(e)
    })
    .await
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
pub async fn favorites_cmd(app: AppHandle, cmd: FavCmd) -> R<Favorites> {
    off_main(app, move |state| {
        let _gate = store_gate();
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
        Ok(snap)
    })
    .await
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum MarkCmd {
    Set {
        key: String,
        path: String,
        item: Option<String>,
    },
    Remove {
        key: String,
    },
}

#[tauri::command]
pub fn marks_get(state: State<'_, AppState>) -> Marks {
    state.marks.lock().unwrap().clone()
}

/// Changes the vim folder marks and saves them; on an error nothing changes.
#[tauri::command]
pub async fn marks_cmd(app: AppHandle, cmd: MarkCmd) -> R<Marks> {
    off_main(app, move |state| {
        let _gate = store_gate();
        let mut next = state.marks.lock().unwrap().clone();
        match cmd {
            MarkCmd::Set { key, path, item } => {
                if !next.set(&key, &path, item) {
                    return Err(format!("Not a mark key: {key}"));
                }
            }
            MarkCmd::Remove { key } => {
                next.remove(&key);
            }
        }
        state.save_marks(&next).map_err(e)?;
        *state.marks.lock().unwrap() = next.clone();
        Ok(next)
    })
    .await
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

fn video_probe_guarded(path: &Path, cache: &Path) -> preview::video::VideoProbe {
    if gall_core::fs::gvfs::is_slow_or_gvfs_mount(path) {
        return preview::video::VideoProbe {
            platform: std::env::consts::OS.into(),
            ..Default::default()
        };
    }
    preview::video::probe(path, cache)
}

fn excerpts_guarded(paths: &[String], lines: usize) -> Vec<Option<String>> {
    paths
        .iter()
        .map(|p| {
            let path = Path::new(p);
            if gall_core::fs::gvfs::is_slow_or_gvfs_mount(path) {
                None
            } else {
                preview::excerpt(path, lines)
            }
        })
        .collect()
}

/// Codec and a still frame for a video the built-in player could not play (needs FFmpeg).
#[tauri::command]
pub async fn video_probe(
    state: State<'_, AppState>,
    path: String,
) -> R<preview::video::VideoProbe> {
    let cache = state.dirs.data.join("cache").join("posters");
    tauri::async_runtime::spawn_blocking(move || video_probe_guarded(Path::new(&path), &cache))
        .await
        .map_err(e)
}

#[tauri::command]
pub async fn excerpts(paths: Vec<String>, lines: usize) -> Vec<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || excerpts_guarded(&paths, lines))
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
    let _update_gate = mutation_guard();
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
        let _update_gate = mutation_guard();
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
    ticket: u64,
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
    let cancel = {
        let cancelled = SIZE_CANCELLED.lock().unwrap();
        let cancel = Arc::new(AtomicBool::new(cancelled.contains(&ticket)));
        SIZE_JOBS.lock().unwrap().push((ticket, cancel.clone()));
        cancel
    };
    let emitter = app.clone();
    let res = off_main(app, move |_| {
        let name = path.to_string_lossy().into_owned();
        Ok(gall_core::fs::dirsize::dir_size(&path, &cancel, &|s| {
            let _ = emitter.emit(
                "folder-size-progress",
                SizeProgress {
                    ticket,
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

/// The repository path when Git refuses the repository around `dir` as owned by another
/// user (null otherwise).
#[tauri::command]
pub async fn git_untrusted(app: AppHandle, dir: PathBuf) -> R<Option<String>> {
    off_main(app, move |_| {
        Ok(gall_core::fs::gitstatus::untrusted(&dir).map(lossy))
    })
    .await
}

/// Adds the repository Git refuses around `dir` to the global `safe.directory`.
#[tauri::command]
pub async fn git_trust(app: AppHandle, dir: PathBuf) -> R<String> {
    off_main(app, move |_| {
        gall_core::fs::gitstatus::trust(&dir).map(lossy)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathUndoResult {
    pub path: String,
    pub undo_id: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathsUndoResult {
    pub paths: Vec<String>,
    pub undo_id: Option<u64>,
}

/// Batch rename: `(path, new name)` pairs as one undo step; returns the new paths.
#[tauri::command]
pub async fn batch_rename(app: AppHandle, items: Vec<(PathBuf, String)>) -> R<Vec<String>> {
    batch_rename_with_undo(app, items).await.map(|r| r.paths)
}

#[tauri::command]
pub async fn batch_rename_with_undo(
    app: AppHandle,
    items: Vec<(PathBuf, String)>,
) -> R<PathsUndoResult> {
    off_main(app, move |state| {
        let _update_gate = mutation_guard();
        let (out, undo_id) = ops::batch_rename_recorded(&items, state.journal()).map_err(e)?;
        Ok(PathsUndoResult {
            paths: out.into_iter().map(lossy).collect(),
            undo_id,
        })
    })
    .await
}

#[tauri::command]
pub async fn create_dir(app: AppHandle, parent: String, name: String) -> R<String> {
    create_dir_with_undo(app, parent, name)
        .await
        .map(|r| r.path)
}

/// Ensures `path` exists as a directory (creating any missing parents; used by Tandem ghost recovery).
#[tauri::command]
pub async fn ensure_dir(app: AppHandle, path: String) -> R<String> {
    off_main(app, move |_| {
        let _update_gate = mutation_guard();
        let target = Path::new(&path);
        std::fs::create_dir_all(target).map_err(|err| format!("create folder: {err}"))?;
        Ok(lossy(target.to_path_buf()))
    })
    .await
}

#[tauri::command]
pub async fn create_dir_with_undo(
    app: AppHandle,
    parent: String,
    name: String,
) -> R<PathUndoResult> {
    off_main(app, move |state| {
        let _update_gate = mutation_guard();
        let (p, undo_id) =
            ops::create_dir_recorded(Path::new(&parent), &name, state.journal()).map_err(e)?;
        Ok(PathUndoResult {
            path: lossy(p),
            undo_id: Some(undo_id),
        })
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
    create_file_with_undo(app, parent, name, template)
        .await
        .map(|r| r.path)
}

#[tauri::command]
pub async fn create_file_with_undo(
    app: AppHandle,
    parent: String,
    name: String,
    template: Option<String>,
) -> R<PathUndoResult> {
    off_main(app, move |state| {
        let _update_gate = mutation_guard();
        let (p, undo_id) = ops::create_file_recorded(
            Path::new(&parent),
            &name,
            template.as_deref().map(Path::new),
            None,
            state.journal(),
        )
        .map_err(e)?;
        Ok(PathUndoResult {
            path: lossy(p),
            undo_id: Some(undo_id),
        })
    })
    .await
}

/// Ctrl+D: copies each item next to itself as a background task (progress, cancel).
/// Returns where the copies will be, so the UI can select them as they appear.
#[tauri::command]
pub async fn duplicate(app: AppHandle, paths: Vec<PathBuf>) -> R<Vec<String>> {
    off_main(app, move |state| {
        let targets = ops::duplicate_targets(&paths);
        start_task_in(
            state,
            TaskRequest {
                kind: TaskKind::Duplicate,
                sources: paths,
                dest: None,
                conflict: ConflictPolicy::KeepBoth,
                archive_name: None,
                format: None,
                extract_mode: Default::default(),
                trash_archive: false,
                merge: false,
            },
        )?;
        Ok(targets.into_iter().map(lossy).collect())
    })
    .await
}

#[tauri::command]
pub async fn trash(state: State<'_, AppState>, paths: Vec<PathBuf>) -> R<Option<u64>> {
    let journal = state.journal().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _update_gate = mutation_guard();
        let (res, undo_id) = ops::trash_items_recorded(&paths, &journal);
        res.map(|()| undo_id).map_err(e)
    })
    .await
    .map_err(e)?
}

#[tauri::command]
pub async fn symlink(app: AppHandle, target: String, dest_dir: String) -> R<String> {
    off_main(app, move |state| {
        let _update_gate = mutation_guard();
        ops::symlink(Path::new(&target), Path::new(&dest_dir), state.journal())
            .map(lossy)
            .map_err(e)
    })
    .await
}

/// An empty journal or folder history is a normal undo outcome.
fn undo_result(result: gall_core::Result<String>) -> R<Option<String>> {
    match result {
        Ok(message) => Ok(Some(message)),
        Err(gall_core::CoreError::NothingToUndo) => Ok(None),
        Err(err) => Err(e(err)),
    }
}

/// Undo can move items back across drives or read the whole trash: off the main thread.
#[tauri::command]
pub async fn undo(app: AppHandle, permanent: Option<bool>) -> R<Option<String>> {
    let permanent = permanent.unwrap_or(false);
    off_main(app, move |state| {
        let _update_gate = mutation_guard();
        undo_result(state.journal().undo_with(permanent))
    })
    .await
}

#[tauri::command]
pub async fn undo_peek(app: AppHandle) -> R<Option<String>> {
    off_main(app, |state| Ok(state.journal().peek_description())).await
}

/// Per-panel undo: the most recent operation that touched `dir`.
#[tauri::command]
pub async fn undo_in(app: AppHandle, dir: String, permanent: Option<bool>) -> R<Option<String>> {
    let permanent = permanent.unwrap_or(false);
    off_main(app, move |state| {
        let _update_gate = mutation_guard();
        undo_result(state.journal().undo_in_with(Path::new(&dir), permanent))
    })
    .await
}

#[tauri::command]
pub async fn undo_peek_in(app: AppHandle, dir: String) -> R<Option<String>> {
    off_main(app, move |state| {
        Ok(state.journal().peek_in(Path::new(&dir)))
    })
    .await
}

/// Undo a specific operation by its stable journal ID (used by notification Undo actions).
#[tauri::command]
pub async fn undo_by_id(app: AppHandle, id: u64, permanent: Option<bool>) -> R<Option<String>> {
    let permanent = permanent.unwrap_or(false);
    off_main(app, move |state| {
        let _update_gate = mutation_guard();
        undo_result(state.journal().undo_by_id_with(id, permanent))
    })
    .await
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
pub async fn shortcut_resolve(path: String) -> R<Option<FileEntry>> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = real_path(Path::new(&path))?;
        gall_core::util::shortcut::resolve(&path).map_err(e)
    })
    .await
    .map_err(e)?
}

#[tauri::command]
pub async fn open_path(app: AppHandle, state: State<'_, AppState>, path: String) -> R<()> {
    let app_for_opener = app.clone();
    let terminal = state.settings.read().unwrap().terminal.clone();
    let rules = state.settings.read().unwrap().open_rules.clone();
    let p = PathBuf::from(&path);
    let fallback_path = tauri::async_runtime::spawn_blocking(move || -> R<Option<String>> {
        let resolved = real_path(&p)?;
        // An application chosen with Open with › Ctrl+Enter wins over the system default.
        if let Some(rule) = gall_core::storage::settings::OpenRule::find(&rules, &resolved) {
            gall_core::util::openwith::run_choice_in(&resolved, &rule.command, &terminal)
                .map_err(e)?;
            return Ok(None);
        }
        // Linux: programs, AppImages, scripts and .desktop files with the execute bit run,
        // as in a file manager; MIME lookup alone would not execute them.
        if let Some(r) = gall_core::util::openwith::open_executable(&resolved, &terminal) {
            r.map_err(e)?;
            return Ok(None);
        }
        // Native default opener: Freedesktop MIME + Terminal=true on Linux, ShellExecuteW on
        // Windows, LaunchServices `/usr/bin/open` on macOS.
        if let Some(r) = gall_core::util::openwith::open_default_in(&resolved, &terminal) {
            r.map_err(e)?;
            return Ok(None);
        }
        Ok(Some(resolved.to_string_lossy().into_owned()))
    })
    .await
    .map_err(e)??;
    if let Some(p) = fallback_path {
        app_for_opener
            .opener()
            .open_path(p, None::<&str>)
            .map_err(e)?;
    }
    Ok(())
}

/// Opens multiple files, batching files that share a rule or multi-file default handler (`%F`/`%U`).
#[tauri::command]
pub async fn open_paths(app: AppHandle, state: State<'_, AppState>, paths: Vec<PathBuf>) -> R<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let app_for_opener = app.clone();
    let terminal = state.settings.read().unwrap().terminal.clone();
    let rules = state.settings.read().unwrap().open_rules.clone();
    let fallbacks = tauri::async_runtime::spawn_blocking(move || -> R<Vec<String>> {
        let materialized = archive_view::materialize(&paths).map_err(e)?;
        let mut by_rule: Vec<(String, Vec<PathBuf>)> = Vec::new();
        let mut defaults: Vec<PathBuf> = Vec::new();
        for p in materialized {
            if let Some(rule) = gall_core::storage::settings::OpenRule::find(&rules, &p) {
                if let Some(g) = by_rule.iter_mut().find(|g| g.0 == rule.command) {
                    g.1.push(p);
                } else {
                    by_rule.push((rule.command.clone(), vec![p]));
                }
                continue;
            }
            if let Some(r) = gall_core::util::openwith::open_executable(&p, &terminal) {
                r.map_err(e)?;
                continue;
            }
            defaults.push(p);
        }
        for (command, batch) in by_rule {
            gall_core::util::openwith::run_choice_many_in(&batch, &command, &terminal)
                .map_err(e)?;
        }
        let mut flatpak_fallbacks = Vec::new();
        if !defaults.is_empty() {
            if cfg!(target_os = "linux") && gall_core::util::hostspawn::in_flatpak() {
                flatpak_fallbacks.extend(
                    defaults
                        .into_iter()
                        .map(|p| p.to_string_lossy().into_owned()),
                );
            } else {
                gall_core::util::openwith::open_default_many_in(&defaults, &terminal).map_err(e)?;
            }
        }
        Ok(flatpak_fallbacks)
    })
    .await
    .map_err(e)??;
    for p in fallbacks {
        app_for_opener
            .opener()
            .open_path(p, None::<&str>)
            .map_err(e)?;
    }
    Ok(())
}

/// Runs `path` with administrator privileges (Windows `runas` verb).
#[tauri::command]
pub async fn open_elevated(app: AppHandle, path: PathBuf) -> R<()> {
    off_main(app, move |_| {
        gall_core::util::openwith::open_elevated(&real_path(&path)?).map_err(e)
    })
    .await
}

// ───────────────────────── open with ─────────────────────────

/// Applications registered for `path` (Linux, Windows, macOS), system default first.
#[tauri::command]
pub async fn open_with_apps(path: PathBuf) -> R<Vec<gall_core::util::openwith::AppChoice>> {
    tauri::async_runtime::spawn_blocking(move || gall_core::util::openwith::apps_for(&path))
        .await
        .map_err(e)
}

/// All installed applications on this OS (for "Show all applications" in Open with…).
#[tauri::command]
pub async fn open_with_all_apps() -> R<Vec<gall_core::util::openwith::AppChoice>> {
    tauri::async_runtime::spawn_blocking(gall_core::util::openwith::all_apps)
        .await
        .map_err(e)
}

/// Sets the OS-wide default application for `path`'s file type (Linux `mimeapps.list`, macOS LaunchServices).
#[tauri::command]
pub async fn open_with_set_default(app: AppHandle, path: PathBuf, id: String) -> R<String> {
    off_main(app, move |_| {
        gall_core::util::openwith::set_system_default(&real_path(&path)?, &id).map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn open_with_app(app: AppHandle, path: PathBuf, id: String) -> R<()> {
    off_main(app, move |state| {
        let terminal = state.settings.read().unwrap().terminal.clone();
        gall_core::util::openwith::open_with_app_in(&real_path(&path)?, &id, &terminal).map_err(e)
    })
    .await
}

/// Shows the system "Open with" dialog; false where there is none.
#[tauri::command]
pub async fn open_with_dialog(app: AppHandle, path: PathBuf) -> R<bool> {
    off_main(app, move |_| {
        gall_core::util::openwith::system_dialog(&real_path(&path)?).map_err(e)
    })
    .await
}

/// Opens paths with a choice's command (see `AppChoice::command`), batching when supported.
#[tauri::command]
pub async fn open_with_run(app: AppHandle, paths: Vec<PathBuf>, command: String) -> R<()> {
    off_main(app, move |state| {
        let terminal = state.settings.read().unwrap().terminal.clone();
        let materialized = archive_view::materialize(&paths).map_err(e)?;
        gall_core::util::openwith::run_choice_many_in(&materialized, &command, &terminal).map_err(e)
    })
    .await
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
pub async fn open_terminal_with(app: AppHandle, dir: String, command: String) -> R<()> {
    off_main(app, move |_| {
        terminal::open_terminal_with(Path::new(&dir), &command).map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn open_with_command(app: AppHandle, path: PathBuf, command: String) -> R<()> {
    off_main(app, move |state| {
        if command.trim().is_empty() {
            return Err("empty command".into());
        }
        let terminal = state.settings.read().unwrap().terminal.clone();
        gall_core::util::openwith::run_choice_in(
            &real_path(&path)?,
            &command.replace("{file}", "{path}"),
            &terminal,
        )
        .map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn reveal_path(app: AppHandle, window: tauri::WebviewWindow, path: String) -> R<()> {
    // Inside an archive: show the archive file itself.
    let path = archive_view::split(Path::new(&path))
        .map(|(ar, _)| ar.to_string_lossy().into_owned())
        .unwrap_or(path);
    #[cfg(target_os = "linux")]
    if crate::filemanager_dbus::should_reveal_internally(crate::filemanager_dbus::owns_name()) {
        return crate::filemanager_dbus::reveal_in_window(
            &app,
            window.label(),
            PathBuf::from(&path),
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = window;
    // The file manager is asked over D-Bus on Linux, which can take seconds.
    tauri::async_runtime::spawn_blocking(move || app.opener().reveal_item_in_dir(path).map_err(e))
        .await
        .map_err(e)?
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
    tauri::async_runtime::spawn_blocking(move || {
        let _update_gate = mutation_guard();
        gall_core::fs::trashbin::restore(&ids)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Deletes trash items for good (`ids` empty: empties the trash).
#[tauri::command]
pub async fn trash_purge(ids: Vec<String>) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _update_gate = mutation_guard();
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
pub async fn open_terminal(app: AppHandle, dir: String) -> R<()> {
    off_main(app, move |state| {
        let t = state.settings.read().unwrap().terminal.clone();
        terminal::open_terminal(Path::new(&dir), &t).map_err(e)
    })
    .await
}

// ───────────────────────── notes / templates ─────────────────────────

#[tauri::command]
pub async fn create_note(app: AppHandle, dir: String, text: String) -> R<String> {
    off_main(app, move |state| {
        let _update_gate = mutation_guard();
        notes::create_note(Path::new(&dir), &text, state.journal())
            .map(lossy)
            .map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn toggle_note(app: AppHandle, path: String) -> R<String> {
    off_main(app, move |state| {
        let _update_gate = mutation_guard();
        notes::toggle_done(Path::new(&path), state.journal())
            .map(lossy)
            .map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn update_note(app: AppHandle, path: String, text: String) -> R<()> {
    off_main(app, move |_| {
        let _update_gate = mutation_guard();
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
        let _update_gate = mutation_guard();
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
    let expanded = npath::file_uri_or_raw(&npath::expand(&input));
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
    entries: Vec<search::SearchHit>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchDone {
    id: u64,
    hits: usize,
    cancelled: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchHidden {
    id: u64,
    count: usize,
    capped: bool,
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
        use gall_core::fs::search_filter::{CountBudget, FilterConfig};
        let mut batch = Vec::new();
        let mut last = std::time::Instant::now();
        let (opts, filter_cfg) = {
            let st = app.state::<AppState>();
            let s = st.settings.read().unwrap();
            let filter_cfg = FilterConfig {
                include_hidden,
                respect_gitignore: s.search_respect_gitignore,
                exclude: s.index_exclude.clone(),
                use_gallignore: true,
            };
            let opts = search::SearchOptions {
                include_hidden,
                respect_gitignore: !everything && s.search_respect_gitignore,
                exclude: if everything {
                    vec![]
                } else {
                    s.index_exclude.clone()
                },
                everything,
                ..Default::default()
            };
            (opts, filter_cfg)
        };
        let hits = search::search_annotated(
            Path::new(&root),
            &pattern,
            opts,
            Some(&filter_cfg),
            &cancel,
            |entry| {
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
            },
        );
        if !batch.is_empty() {
            let _ = app.emit("search-batch", SearchBatch { id, entries: batch });
        }
        let cancelled = cancel.load(Ordering::Relaxed);
        let _ = app.emit(
            "search-done",
            SearchDone {
                id,
                hits,
                cancelled,
            },
        );
        // Count hidden matches lazily after visible search finishes so visible results
        // are never delayed.
        if !everything && !cancelled {
            let hidden = search::count_hidden(
                Path::new(&root),
                &pattern,
                &filter_cfg,
                CountBudget::name_default(),
                &cancel,
            );
            if !cancel.load(Ordering::Relaxed) && hidden.count > 0 {
                let _ = app.emit(
                    "search-hidden",
                    SearchHidden {
                        id,
                        count: hidden.count,
                        capped: hidden.capped,
                    },
                );
            }
        }
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
        let _update_gate = mutation_guard();
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
        if let Some(f) = st.everything_scan.lock().unwrap().take() {
            f.cancel.store(true, Ordering::Relaxed);
        }
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
        let _update_gate = mutation_guard();
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
            if let Some(f) = st.everything_scan.lock().unwrap().take() {
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
            let st = app.state::<AppState>();
            if let Some(f) = st.everything_scan.lock().unwrap().take() {
                f.cancel.store(true, Ordering::Relaxed);
            }
            index_rebuild_bg(&app, &st, 0);
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

/// Cancels the running build so a requested rebuild starts right away.
static INDEX_CANCEL: AtomicBool = AtomicBool::new(false);

/// Starts a background rebuild unless one is running or the index is younger
/// than `max_age_s`. Emits `index-updated` when done.
///
/// A forced rebuild (`max_age_s == 0`) requested while one is running (e.g. the indexed
/// folders changed during the first full scan) cancels it and makes the build thread start
/// over, so the request is never dropped. The request is recorded in the index before anything
/// else: `status().building` is then true without a gap from the request to the end of the
/// build that includes it, which is what callers wait for.
pub fn index_rebuild_bg(app: &AppHandle, state: &AppState, max_age_s: u64) -> bool {
    let idx = state.index.clone();
    let forced = max_age_s == 0;
    if forced {
        idx.request_rebuild();
    }
    if idx.is_building() {
        if forced {
            INDEX_CANCEL.store(true, Ordering::SeqCst);
        }
        return false;
    }
    if idx.age().is_some_and(|a| a.as_secs() < max_age_s) {
        return false;
    }
    idx.request_rebuild();
    idx.set_preferred(&dirs::home_dir().into_iter().collect::<Vec<_>>());
    let app = app.clone();
    let thread_idx = idx.clone();
    let started = std::thread::Builder::new()
        .name("file-index".into())
        .spawn(move || loop {
            INDEX_CANCEL.store(false, Ordering::SeqCst);
            let (roots, opts) = {
                let st = app.state::<AppState>();
                let s = st.settings.read().unwrap();
                index_options(&s)
            };
            let ran = thread_idx.rebuild(&roots, &opts, &INDEX_CANCEL);
            if ran {
                let _ = app.emit("index-updated", thread_idx.status());
            }
            // Another build owns the disk walk and the requests: it reruns if one came later.
            // A cancelled build left the old snapshot in place, so it always runs again.
            if !ran || !(thread_idx.rebuild_requested() || INDEX_CANCEL.load(Ordering::SeqCst)) {
                break;
            }
        })
        .is_ok();
    if !started {
        idx.withdraw_request();
    }
    started
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

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScopeInfo {
    /// The system index holds the folder.
    covered: bool,
    /// Not covered: Quick Open's own scan of it is still running.
    scanning: bool,
    /// Not covered: items found by that scan.
    entries: usize,
}

fn check_index_scope(roots: &[PathBuf], p: &Path) -> Option<ScopeInfo> {
    if gall_core::fs::gvfs::is_slow_or_gvfs_mount(p) {
        return Some(ScopeInfo {
            covered: false,
            scanning: false,
            entries: 0,
        });
    }
    if gall_core::fs::index::index_covers(roots, p) {
        return Some(ScopeInfo {
            covered: true,
            scanning: false,
            entries: 0,
        });
    }
    None
}

fn index_covers_guarded(
    app: &AppHandle,
    state: &AppState,
    roots: &[PathBuf],
    p: &Path,
) -> ScopeInfo {
    if let Some(info) = check_index_scope(roots, p) {
        return info;
    }
    let fi = folder_scan(app, state, p);
    ScopeInfo {
        covered: false,
        scanning: fi.is_building() || fi.age().is_none(),
        entries: fi.status().entries,
    }
}

/// Whether the system index holds `path`. When it does not (WSL, a network share),
/// the folder scan starts right away, so results are ready by the time one types.
#[tauri::command]
pub async fn index_covers(app: AppHandle, path: String) -> R<ScopeInfo> {
    off_main(app.clone(), move |state| {
        let roots = index_options(&state.settings.read().unwrap()).0;
        Ok(index_covers_guarded(&app, state, &roots, Path::new(&path)))
    })
    .await
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

/// Signal raised when an on-demand `everything_scan` finishes its initial build.
type ScanSignal = Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>;

/// Unfiltered scan of `roots` (for Quick Open's `Alt+I` mode, in either a single
/// folder or `everywhere`): keeps all ignored items and marks each normally-hidden
/// entry so Quick Open can badge/dim them. Returns the index and an optional signal
/// waited on briefly when a fresh scan starts.
fn everything_scan(
    app: &AppHandle,
    state: &AppState,
    roots: &[PathBuf],
    priority: &[PathBuf],
    max_entries: usize,
) -> (Arc<FileIndex>, Option<ScanSignal>) {
    let mut opts = index_options(&state.settings.read().unwrap()).1;
    opts.everything = true;
    opts.priority_roots = priority.to_vec();
    opts.max_entries = max_entries;
    let roots_key = roots
        .iter()
        .map(|r| r.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\0");
    let key = format!("{roots_key}::{}", scan_key(&opts));
    let first_root = roots.first().cloned().unwrap_or_default();
    let mut g = state.everything_scan.lock().unwrap();
    if let Some(f) = g.as_ref() {
        let stale = f
            .index
            .age()
            .is_some_and(|a| a.as_secs() >= FOLDER_SCAN_FRESH_S);
        // Exact roots match: hidden-ness depends on the search root (starting inside
        // an ignored folder keeps its direct children visible).
        if f.key == key && !stale && !f.cancel.load(Ordering::Relaxed) {
            return (f.index.clone(), None);
        }
        f.cancel.store(true, Ordering::Relaxed);
    }
    let index = Arc::new(FileIndex::new());
    index.set_preferred(&dirs::home_dir().into_iter().collect::<Vec<_>>());
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    *g = Some(crate::state::FolderScan {
        root: first_root,
        index: index.clone(),
        cancel: cancel.clone(),
        key,
    });
    let (app, idx, roots, done_tx) = (app.clone(), index.clone(), roots.to_vec(), done.clone());
    let _ = std::thread::Builder::new()
        .name("everything-scan".into())
        .spawn(move || {
            for r in &roots {
                let _ = gall_core::fs::wsl::prepare(r);
            }
            idx.rebuild(&roots, &opts, &cancel);
            {
                let mut finished = done_tx.0.lock().unwrap();
                *finished = true;
                done_tx.1.notify_all();
            }
            if !cancel.load(Ordering::Relaxed) {
                let st = app.state::<AppState>();
                let _ = app.emit("index-updated", st.index.status());
            }
        });
    (index, Some(done))
}

fn priority_dirs(priority: Option<String>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = priority.filter(|s| !s.is_empty()) {
        out.push(PathBuf::from(p));
    }
    if let Some(h) = dirs::home_dir() {
        if !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

#[tauri::command]
pub async fn index_query(
    app: AppHandle,
    state: State<'_, AppState>,
    query: String,
    scope: Option<String>,
    limit: usize,
    include_ignored: Option<bool>,
    priority: Option<String>,
) -> R<gall_core::fs::index::IndexResult> {
    let include_ignored = include_ignored.unwrap_or(false);
    let scope = scope.map(PathBuf::from);
    let roots = index_options(&state.settings.read().unwrap()).0;
    if include_ignored && !query.trim().is_empty() {
        if scope
            .as_deref()
            .is_some_and(gall_core::fs::gvfs::is_slow_or_gvfs_mount)
        {
            return Ok(gall_core::fs::index::IndexResult {
                hits: Vec::new(),
                total: 0,
                searched: 0,
                elapsed_ms: 0,
                scanning: false,
            });
        }
        let (idx, wait) = if let Some(root) = scope.as_ref() {
            everything_scan(&app, &state, std::slice::from_ref(root), &[], 500_000)
        } else {
            let prio = priority_dirs(priority);
            everything_scan(&app, &state, &roots, &prio, 1_500_000)
        };
        return tauri::async_runtime::spawn_blocking(move || {
            if let Some(sig) = wait {
                let guard = sig.0.lock().unwrap();
                if !*guard {
                    let _ = sig
                        .1
                        .wait_timeout(guard, std::time::Duration::from_millis(150));
                }
            }
            let mut r = idx.query(&query, scope.as_deref(), limit.clamp(1, 500));
            r.scanning = idx.is_building() || idx.age().is_none();
            r
        })
        .await
        .map_err(e);
    }
    let live = scope
        .as_deref()
        .filter(|s| {
            !query.trim().is_empty()
                && !gall_core::fs::gvfs::is_slow_or_gvfs_mount(s)
                && !gall_core::fs::index::index_covers(&roots, s)
        })
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

/// Counts entries hidden by search filters under `scope` (or across all indexed
/// roots when `scope` is `None` / "everywhere") that match Quick Open's fuzzy
/// `query`, within a strict budget. Cancels any prior in-flight count.
#[tauri::command]
pub async fn index_hidden_count(
    state: State<'_, AppState>,
    query: String,
    scope: Option<String>,
    priority: Option<String>,
) -> R<gall_core::fs::search_filter::HiddenCount> {
    use gall_core::fs::search_filter::{CountBudget, FilterConfig};
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(prev) = state
        .quick_hidden_cancel
        .lock()
        .unwrap()
        .replace(cancel.clone())
    {
        prev.store(true, Ordering::Relaxed);
    }
    if query.trim().is_empty() {
        return Ok(gall_core::fs::search_filter::HiddenCount::default());
    }
    let (roots, filter_cfg) = {
        let s = state.settings.read().unwrap();
        let roots = index_options(&s).0;
        let filter_cfg = FilterConfig {
            include_hidden: s.index_hidden,
            respect_gitignore: s.index_respect_gitignore,
            exclude: s.index_exclude.clone(),
            use_gallignore: true,
        };
        (roots, filter_cfg)
    };
    tauri::async_runtime::spawn_blocking(move || match scope {
        Some(dir) => gall_core::fs::index::count_hidden(
            Path::new(&dir),
            &query,
            &filter_cfg,
            CountBudget::name_default(),
            &cancel,
        ),
        None => {
            let prio = priority_dirs(priority);
            gall_core::fs::index::count_hidden_roots(
                &roots,
                &prio,
                &query,
                &filter_cfg,
                CountBudget::name_default(),
                &cancel,
            )
        }
    })
    .await
    .map_err(e)
}

#[tauri::command]
pub fn index_hidden_cancel(state: State<'_, AppState>) {
    if let Some(c) = state.quick_hidden_cancel.lock().unwrap().take() {
        c.store(true, Ordering::Relaxed);
    }
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrepHidden {
    id: u64,
    count: usize,
    capped: bool,
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
        opts.default_exclude = s.index_exclude.clone();
        // Same hidden-files choice as Quick Open (`.git` stays out via `default_exclude`).
        opts.include_hidden = s.index_hidden;
        // Keep the user's gitignore setting even under `everything`, so `grep::grep`
        // knows which matches would normally have been gitignored; `grep::grep` itself
        // skips gitignore enforcement when `opts.everything` is true.
        opts.respect_gitignore = s.search_respect_gitignore;
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
        let cancelled = cancel.load(Ordering::Relaxed);
        let has_error = error.is_some();
        let _ = app.emit(
            "grep-done",
            GrepDone {
                id,
                stats,
                cancelled,
                error,
            },
        );
        // Count hidden matches lazily after visible grep finishes.
        if !opts.everything && !cancelled && !has_error {
            if let Ok(hidden) = grep::count_hidden(
                Path::new(&root),
                &pattern,
                &opts,
                gall_core::fs::search_filter::CountBudget::grep_default(),
                &cancel,
            ) {
                if !cancel.load(Ordering::Relaxed) && hidden.count > 0 {
                    let _ = app.emit(
                        "grep-hidden",
                        GrepHidden {
                            id,
                            count: hidden.count,
                            capped: hidden.capped,
                        },
                    );
                }
            }
        }
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
pub async fn tags_in(app: AppHandle, dir: String) -> R<Vec<Tagged>> {
    off_main(app, move |state| state.db.tags_in(&dir).map_err(e)).await
}

#[tauri::command]
pub fn set_tag(state: State<'_, AppState>, paths: Vec<String>, color: u8) -> R<()> {
    state.db.set_tags(&paths, color).map_err(e)
}

#[tauri::command]
pub async fn tagged(app: AppHandle, color: Option<u8>) -> R<Vec<Tagged>> {
    off_main(app, move |state| state.db.tagged(color).map_err(e)).await
}

#[tauri::command]
pub async fn destinations(app: AppHandle) -> R<Vec<String>> {
    off_main(app, |state| state.db.destinations().map_err(e)).await
}

#[tauri::command]
pub fn clear_destinations(state: State<'_, AppState>) -> R<()> {
    state.db.clear_destinations().map_err(e)
}

// ───────────────────────── clipboard ─────────────────────────

fn system_clipboard() -> R<arboard::Clipboard> {
    arboard::Clipboard::new().map_err(|err| format!("clipboard unavailable: {err}"))
}

/// Ctrl+C / Ctrl+X: remember files internally and put them on the system clipboard.
/// On Linux/Wayland, offers `x-special/gnome-copied-files`, `text/uri-list` (CRLF),
/// and `text/plain;charset=utf-8` via GTK, falling back to `arboard` when headless.
#[tauri::command]
pub async fn clip_set_files(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<PathBuf>,
    cut: bool,
) -> R<()> {
    *state.clipboard.lock().unwrap() = FileClipboard {
        paths: paths.clone(),
        cut,
    };
    tauri::async_runtime::spawn_blocking(move || -> R<()> {
        #[cfg(target_os = "linux")]
        match crate::linux_clipboard::set_files(&app, &paths, cut) {
            Ok(true) => return Ok(()),
            // Timed out: the GTK write may still land, so do not write a second time.
            Err(err) => return Err(err),
            Ok(false) => {}
        }
        #[cfg(not(target_os = "linux"))]
        let _ = &app;
        // Windows: the Explorer formats (drop list + copy/cut effect). When the clipboard cannot
        // be written, the plain-text fallback below still carries the paths.
        #[cfg(windows)]
        if crate::windows_clipboard::set_files(&paths, cut).is_ok() {
            return Ok(());
        }
        // macOS: `public.file-url` items via `NSPasteboard` (Finder interoperable) plus plain text.
        #[cfg(target_os = "macos")]
        if crate::macos_clipboard::set_files(&paths, cut).is_ok() {
            return Ok(());
        }
        let text = paths
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        if let Ok(mut c) = system_clipboard() {
            let _ = c.set_text(text);
        }
        Ok(())
    })
    .await
    .map_err(e)?
}

#[tauri::command]
pub async fn clip_get_files(app: AppHandle, state: State<'_, AppState>) -> R<FileClipboard> {
    let internal = state.clipboard.lock().unwrap().clone();
    // Files another app put on the system clipboard win over the (possibly stale) internal copy;
    // the internal one still serves when the system clipboard has no files (headless, fallback text).
    #[cfg(target_os = "linux")]
    {
        let app_clone = app.clone();
        if let Ok(Some((paths, cut))) = tauri::async_runtime::spawn_blocking(move || {
            crate::linux_clipboard::read_files(&app_clone)
        })
        .await
        {
            return Ok(FileClipboard { paths, cut });
        }
    }
    #[cfg(windows)]
    if let Ok(Some((paths, cut))) =
        tauri::async_runtime::spawn_blocking(crate::windows_clipboard::read_files).await
    {
        return Ok(FileClipboard { paths, cut });
    }
    #[cfg(target_os = "macos")]
    if let Ok(Some((paths, cut))) =
        tauri::async_runtime::spawn_blocking(crate::macos_clipboard::read_files).await
    {
        return Ok(FileClipboard { paths, cut });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = app;
    Ok(internal)
}

/// macOS: reads file paths currently on `NSDragPboard` as a fallback when WKWebView omits
/// `text/uri-list` during a drag from Finder.
#[tauri::command]
pub async fn macos_drag_paths() -> R<Vec<String>> {
    #[cfg(target_os = "macos")]
    {
        tauri::async_runtime::spawn_blocking(|| {
            crate::macos_clipboard::read_drag_files()
                .into_iter()
                .map(lossy)
                .collect()
        })
        .await
        .map_err(e)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(Vec::new())
    }
}

/// macOS: opens System Settings › Privacy & Security › Full Disk Access.
#[tauri::command]
pub fn open_full_disk_access_settings() -> R<()> {
    #[cfg(target_os = "macos")]
    {
        gall_core::util::cmdline::spawn(
            &[
                "/usr/bin/open".to_string(),
                "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
                    .to_string(),
            ],
            None,
        )
        .map_err(e)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Full Disk Access settings are only available on macOS".into())
    }
}

#[tauri::command]
pub fn clip_clear_files(state: State<'_, AppState>) {
    *state.clipboard.lock().unwrap() = FileClipboard::default();
}

#[tauri::command]
pub async fn clip_set_text(app: AppHandle, text: String) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || -> R<()> {
        #[cfg(target_os = "linux")]
        match crate::linux_clipboard::set_text(&app, &text) {
            Ok(true) => return Ok(()),
            Err(err) => return Err(err),
            Ok(false) => {}
        }
        #[cfg(not(target_os = "linux"))]
        let _ = &app;
        system_clipboard()?.set_text(text).map_err(e)
    })
    .await
    .map_err(e)?
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClipContent {
    Empty,
    Text {
        text: String,
    },
    Url {
        url: String,
    },
    Paths {
        paths: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cut: Option<bool>,
    },
    Image {
        width: usize,
        height: usize,
    },
}

/// Inspects the system clipboard (O-10): what would Ctrl+V do?
#[tauri::command]
pub async fn clip_inspect(app: AppHandle) -> R<ClipContent> {
    // Reading the clipboard waits for the program that owns it (X11/Wayland): off the main thread.
    tauri::async_runtime::spawn_blocking(move || inspect_clipboard(&app))
        .await
        .map_err(e)?
}

fn classify_clipboard_text(t: String) -> ClipContent {
    // A list of existing absolute paths (one per line) = files from another app.
    let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let paths: Vec<String> = lines
        .iter()
        .map(|l| npath::file_uri_or_raw(l))
        .filter(|l| Path::new(l).is_absolute() && Path::new(l).exists())
        .collect();
    if !paths.is_empty() && paths.len() == lines.len() {
        return ClipContent::Paths { paths, cut: None };
    }
    match clipname::classify_text(&t) {
        clipname::ClipText::Url(url) => ClipContent::Url { url },
        clipname::ClipText::Path(p) => ClipContent::Paths {
            paths: vec![p],
            cut: None,
        },
        clipname::ClipText::Text if t.trim().is_empty() => ClipContent::Empty,
        clipname::ClipText::Text => ClipContent::Text { text: t },
    }
}

fn inspect_clipboard(app: &AppHandle) -> R<ClipContent> {
    #[cfg(target_os = "linux")]
    if let Some(snap) = crate::linux_clipboard::inspect(app) {
        use crate::linux_clipboard::GtkClipboardSnapshot;
        return Ok(match snap {
            GtkClipboardSnapshot::GnomeFiles { paths, cut } => ClipContent::Paths {
                paths: paths.into_iter().map(lossy).collect(),
                cut: Some(cut),
            },
            GtkClipboardSnapshot::UriList { paths } => ClipContent::Paths {
                paths: paths.into_iter().map(lossy).collect(),
                cut: Some(false),
            },
            GtkClipboardSnapshot::Text(t) => classify_clipboard_text(t),
            GtkClipboardSnapshot::Image { width, height } => ClipContent::Image { width, height },
            GtkClipboardSnapshot::Empty => ClipContent::Empty,
        });
    }
    #[cfg(windows)]
    if let Some((paths, cut)) = crate::windows_clipboard::read_files() {
        return Ok(ClipContent::Paths {
            paths: paths.into_iter().map(lossy).collect(),
            cut: Some(cut),
        });
    }
    #[cfg(target_os = "macos")]
    if let Some((paths, cut)) = crate::macos_clipboard::read_files() {
        return Ok(ClipContent::Paths {
            paths: paths.into_iter().map(lossy).collect(),
            cut: Some(cut),
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = app;
    inspect_clipboard_arboard()
}

fn inspect_clipboard_arboard() -> R<ClipContent> {
    let mut c = system_clipboard()?;
    if let Ok(t) = c.get_text() {
        return Ok(classify_clipboard_text(t));
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
    let app_for_clip = app.clone();
    off_main(app, move |state| {
        paste_as_file(&app_for_clip, state, &dir, &as_kind)
    })
    .await
}

fn read_clipboard_image_png(app: &AppHandle) -> R<Vec<u8>> {
    #[cfg(target_os = "linux")]
    if let Some(png) = crate::linux_clipboard::read_image_png(app) {
        return Ok(png);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = app;
    let mut c = system_clipboard()?;
    let img = c.get_image().map_err(e)?;
    let buf =
        image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
            .ok_or("invalid image data on clipboard")?;
    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png).map_err(e)?;
    Ok(out.into_inner())
}

fn read_clipboard_text(app: &AppHandle) -> R<String> {
    #[cfg(target_os = "linux")]
    if let Some(t) = crate::linux_clipboard::read_text(app) {
        return Ok(t);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = app;
    system_clipboard()?.get_text().map_err(e)
}

fn paste_as_file(app: &AppHandle, state: &AppState, dir: &str, as_kind: &str) -> R<String> {
    let _update_gate = mutation_guard();
    let now = chrono::Local::now();
    let dirp = Path::new(dir);
    let unique = |ext: &str| ops::unique_path(dirp, &clipname::timestamp_name(now, ext));
    let journal = state.journal();
    let created = match as_kind {
        "image" => {
            let png = read_clipboard_image_png(app)?;
            let target = unique("png");
            std::fs::write(&target, &png).map_err(e)?;
            journal.push(ops::UndoRecord::Created {
                paths: vec![target.clone()],
            });
            target
        }
        "url" => {
            let url = read_clipboard_text(app)?.trim().to_string();
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
            let text = read_clipboard_text(app)?;
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
/// Returned as a raw binary IPC response to avoid JSON number-array overhead on 60 Hz scrolls.
#[tauri::command]
pub async fn read_bytes(path: PathBuf, offset: u64, len: u64) -> R<tauri::ipc::Response> {
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::hexdiff::read_at(&path, offset, len).map_err(e)
    })
    .await
    .map_err(e)??;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Drags `paths` as real files, so other apps (Explorer, Finder, mail, Chromium, terminals)
/// can take them: the UI cancels the page's own drag, which carries only text, and calls
/// this while the mouse button is still down. Resolves when the drag ends: "dropped" or "cancelled".
#[tauri::command]
pub async fn start_native_drag(
    app: AppHandle,
    window: tauri::WebviewWindow,
    window_label: Option<String>,
    paths: Vec<PathBuf>,
) -> R<String> {
    let win = match window_label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(label) => app
            .get_webview_window(label)
            .ok_or_else(|| format!("window '{label}' not found"))?,
        None => window,
    };
    #[cfg(windows)]
    {
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
        app.run_on_main_thread(move || {
            let res = win.hwnd().map_err(e).and_then(|hwnd| {
                windows_drag::run(windows::Win32::Foundation::HWND(hwnd.0), &paths)
            });
            let _ = tx.send(res);
        })
        .map_err(e)?;
        tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or(Err("drag ended".into())))
            .await
            .map_err(e)?
    }
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
        let tx2 = tx.clone();
        app.run_on_main_thread(move || {
            const ICON: &[u8] = include_bytes!("../icons/32x32.png");
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                drag::start_drag(
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
                )
            }));
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    let _ = tx.send(Err(e.to_string()));
                }
                Err(_) => {
                    let _ = tx.send(Err("native drag could not start".into()));
                }
            }
        })
        .map_err(e)?;
        tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or(Err("drag ended".into())))
            .await
            .map_err(e)?
    }
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        crate::linux_clipboard::start_native_drag(&win, paths).await
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (app, win, paths);
        Err("native drag is not used on this platform".into())
    }
}

/// The Windows drag out of the window, done with the shell's own data object and drag
/// loop. The `drag` crate is not used here: it canonicalizes the paths, which leaves
/// `\\?\UNC\…` for WSL folders, network shares and mapped network drives (and `\\?\C:\…`
/// for long paths or reserved names), which the shell rejects, and then it unwraps the
/// missing item list and takes the whole app down.
#[cfg(windows)]
mod windows_drag {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::{
        IDropSource, OleInitialize, DROPEFFECT_COPY, DROPEFFECT_NONE,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, ILCreateFromPathW, ILFree, IShellItemArray,
        SHCreateShellItemArrayFromIDLists, SHDoDragDrop,
    };

    /// An item ID list owned by us, freed on drop.
    struct Pidl(*mut ITEMIDLIST);
    impl Drop for Pidl {
        fn drop(&mut self) {
            unsafe { ILFree(Some(self.0)) }
        }
    }

    fn pidl(path: &Path) -> Result<Pidl, String> {
        let path = gall_core::nav::path::strip_verbatim(path.to_path_buf());
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let id = unsafe { ILCreateFromPathW(PCWSTR(wide.as_ptr())) };
        if id.is_null() {
            Err(format!("cannot drag {}", path.display()))
        } else {
            Ok(Pidl(id))
        }
    }

    /// The shell's data object for `paths` (what Explorer itself hands out), or why it
    /// cannot be made. Never panics.
    pub(super) fn data_object(paths: &[PathBuf]) -> Result<IDataObject, String> {
        if paths.is_empty() {
            return Err("nothing to drag".into());
        }
        let ids = paths
            .iter()
            .map(|p| pidl(p))
            .collect::<Result<Vec<_>, _>>()?;
        let list: Vec<*const ITEMIDLIST> = ids.iter().map(|id| id.0.cast_const()).collect();
        unsafe {
            let items: IShellItemArray =
                SHCreateShellItemArrayFromIDLists(&list).map_err(|e| e.to_string())?;
            items
                .BindToHandler(None, &BHID_DataObject)
                .map_err(|e| e.to_string())
        }
    }

    /// Runs the drag on the main thread until it ends: "dropped" or "cancelled".
    pub(super) fn run(hwnd: HWND, paths: &[PathBuf]) -> Result<String, String> {
        let data = data_object(paths)?;
        // The window's event loop has already set OLE up on this (main) thread; asking
        // once more only adds a reference (S_FALSE), so do it once, not per drag.
        static OLE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        OLE.get_or_init(|| unsafe { OleInitialize(None) }.map_err(|e| e.to_string()))
            .clone()?;
        // No drop source of our own: the shell supplies one with the usual drag image.
        match unsafe { SHDoDragDrop(Some(hwnd), &data, None::<&IDropSource>, DROPEFFECT_COPY) } {
            Ok(effect) if effect != DROPEFFECT_NONE => Ok("dropped".into()),
            Ok(_) => Ok("cancelled".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn data_object_rejects_what_the_shell_cannot_name_without_panicking() {
            assert!(data_object(&[]).is_err());
            let dir = std::env::temp_dir().join(format!("gall-drag-{}", std::process::id()));
            assert!(data_object(&[dir.join("missing.txt")]).is_err());
            assert!(data_object(&[PathBuf::from("relative/forward/slashes.txt")]).is_err());
        }

        #[test]
        fn data_object_accepts_local_verbatim_and_unc_paths() {
            unsafe { OleInitialize(None) }.unwrap();
            let dir = std::env::temp_dir().join(format!("gall-drag-ok-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("a b.txt");
            std::fs::write(&file, "x").unwrap();
            let file = gall_core::nav::path::canonicalize(&file).unwrap();
            assert!(data_object(std::slice::from_ref(&file)).is_ok());
            // What `std::fs::canonicalize` returns: the form that crashed the `drag` crate.
            let verbatim = std::fs::canonicalize(&file).unwrap();
            assert!(data_object(&[verbatim]).is_ok());
            // The same file through the administrative share, as a UNC path (a WSL folder,
            // a network share or a mapped network drive); skipped where the share is off.
            let s = file.to_string_lossy();
            if let Some(rest) = s
                .get(..2)
                .filter(|d| d.ends_with(':'))
                .map(|d| (d, &s[2..]))
            {
                let unc = PathBuf::from(format!(r"\\localhost\{}${}", &rest.0[..1], rest.1));
                if unc.exists() {
                    assert!(
                        data_object(&[unc.clone(), file.clone()]).is_ok(),
                        "{}",
                        unc.display()
                    );
                    let verbatim_unc =
                        PathBuf::from(format!(r"\\?\UNC\{}", &unc.to_string_lossy()[2..]));
                    assert!(data_object(&[verbatim_unc]).is_ok());
                }
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
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

/// Times actual presented frames when the Linux compositor offers hardware feedback.
#[tauri::command]
pub async fn selftest_frame_trace(
    window: tauri::WebviewWindow,
    start: bool,
) -> R<Option<Vec<[u64; 2]>>> {
    if selftest_dir().is_none() {
        return Err("self-test not enabled".into());
    }
    #[cfg(gall_wayland_presentation)]
    {
        let (reply, mut receiver) = tauri::async_runtime::channel(1);
        let target = window.clone();
        window
            .run_on_main_thread(move || {
                let _ = reply.try_send(crate::window_presentation::frame_trace(&target, start));
            })
            .map_err(e)?;
        receiver
            .recv()
            .await
            .ok_or("frame trace ended early".into())
    }
    #[cfg(not(gall_wayland_presentation))]
    {
        let _ = (window, start);
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelftestCaptureMode {
    Auto,
    Webview,
    Spectacle,
    Grim,
}

impl SelftestCaptureMode {
    fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("auto") => Ok(Self::Auto),
            Some("webview") => Ok(Self::Webview),
            Some("spectacle") => Ok(Self::Spectacle),
            Some("grim") => Ok(Self::Grim),
            Some(other) => Err(format!(
                "unknown GALL_SELFTEST_CAPTURE mode '{other}' (expected auto, webview, spectacle, or grim)"
            )),
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn hyprland_window_geometry(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let at = value.get("at")?.as_array()?;
    let size = value.get("size")?.as_array()?;
    if at.len() < 2 || size.len() < 2 {
        return None;
    }
    let (x, y) = (at[0].as_i64()?, at[1].as_i64()?);
    let (w, h) = (size[0].as_i64()?, size[1].as_i64()?);
    if w <= 0 || h <= 0 {
        return None;
    }
    Some(format!("{x},{y} {w}x{h}"))
}

#[cfg(target_os = "linux")]
fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    tool_name: &str,
) -> std::io::Result<std::process::ExitStatus> {
    let mut child = cmd.spawn()?;
    let t0 = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if t0.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other(format!("{tool_name} timed out")));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[cfg(target_os = "linux")]
async fn capture_with_spectacle(out: PathBuf) -> R<()> {
    let o = out.clone();
    let status = tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("spectacle");
        cmd.args(["-b", "-n", "-a", "-o"]).arg(&o);
        // The app runs on a private D-Bus; Spectacle needs the desktop session bus.
        if let Ok(bus) = std::env::var("GALL_SELFTEST_DBUS") {
            if !bus.is_empty() {
                cmd.env("DBUS_SESSION_BUS_ADDRESS", bus);
            }
        }
        // Spectacle occasionally never returns; don't let that hang the self-test.
        run_with_timeout(cmd, std::time::Duration::from_secs(10), "spectacle")
    })
    .await
    .map_err(e)?
    .map_err(e)?;
    if !status.success() || !out.exists() {
        return Err(format!("spectacle failed ({status})"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn capture_with_grim(out: PathBuf) -> R<()> {
    let o = out.clone();
    let status = tauri::async_runtime::spawn_blocking(move || {
        let geometry = if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            std::process::Command::new("hyprctl")
                .args(["activewindow", "-j"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .and_then(|json| hyprland_window_geometry(&json))
        } else {
            None
        };
        let mut cmd = std::process::Command::new("grim");
        if let Some(region) = geometry {
            cmd.args(["-g", &region]);
        }
        cmd.arg(&o);
        run_with_timeout(cmd, std::time::Duration::from_secs(10), "grim")
    })
    .await
    .map_err(e)?
    .map_err(e)?;
    if !status.success() || !out.exists() {
        return Err(format!("grim failed ({status})"));
    }
    Ok(())
}

/// Captures Gall's owned webview on Linux and Windows, with optional compositor fallbacks on Linux.
#[tauri::command]
pub async fn selftest_shot(window: tauri::WebviewWindow, name: String) -> R<String> {
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
    let mode_raw = std::env::var("GALL_SELFTEST_CAPTURE").ok();
    let mode = SelftestCaptureMode::parse(mode_raw.as_deref())?;

    match mode {
        SelftestCaptureMode::Webview => {
            let png = crate::report_capture::bug_report_capture(window).await?;
            std::fs::write(&out, png).map_err(e)?;
            Ok(out.to_string_lossy().into_owned())
        }
        SelftestCaptureMode::Spectacle => {
            #[cfg(target_os = "linux")]
            {
                capture_with_spectacle(out.clone()).await?;
                Ok(out.to_string_lossy().into_owned())
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = window;
                Err("GALL_SELFTEST_CAPTURE=spectacle is only supported on Linux".into())
            }
        }
        SelftestCaptureMode::Grim => {
            #[cfg(target_os = "linux")]
            {
                capture_with_grim(out.clone()).await?;
                Ok(out.to_string_lossy().into_owned())
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = window;
                Err("GALL_SELFTEST_CAPTURE=grim is only supported on Linux".into())
            }
        }
        SelftestCaptureMode::Auto => {
            match crate::report_capture::bug_report_capture(window).await {
                Ok(png) => {
                    std::fs::write(&out, png).map_err(e)?;
                    Ok(out.to_string_lossy().into_owned())
                }
                Err(webview_err) => {
                    #[cfg(target_os = "linux")]
                    {
                        if capture_with_spectacle(out.clone()).await.is_ok()
                            || capture_with_grim(out.clone()).await.is_ok()
                        {
                            return Ok(out.to_string_lossy().into_owned());
                        }
                    }
                    Err(webview_err)
                }
            }
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn hyprland_resize_lua(pid: u32, width: u32, height: u32) -> String {
    format!(
        "for _, w in ipairs(hl.get_windows()) do \
         if w.pid == {pid} then \
         hl.dispatch(hl.dsp.window.fullscreen({{ mode = \"fullscreen\", action = \"unset\", window = w }})); \
         hl.dispatch(hl.dsp.window.fullscreen({{ mode = \"maximized\", action = \"unset\", window = w }})); \
         hl.dispatch(hl.dsp.window.float({{ action = \"on\", window = w }})); \
         hl.dispatch(hl.dsp.window.resize({{ x = {width}, y = {height}, exact = true, window = w }})); \
         hl.dispatch(hl.dsp.window.center({{ window = w }})); \
         end end"
    )
}

#[cfg(target_os = "linux")]
fn resize_on_hyprland(pid: u32, width: u32, height: u32) {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    let lua = hyprland_resize_lua(pid, width, height);
    let lua_ok = std::process::Command::new("hyprctl")
        .args(["eval", &lua])
        .output()
        .map(|out| {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            out.status.success() && !text.to_ascii_lowercase().contains("error:")
        })
        .unwrap_or(false);
    if !lua_ok {
        let batch = format!(
            "dispatch fullscreenstate 0 0; \
             dispatch setfloating pid:{pid}; \
             dispatch resizewindowpixel exact {width} {height},pid:{pid}; \
             dispatch centerwindow"
        );
        let _ = std::process::Command::new("hyprctl")
            .args(["--batch", &batch])
            .output();
    }
}

/// Resizes the self-test window to exact logical dimensions, including on tiling
/// Wayland compositors such as Hyprland that ignore client-initiated resizes.
#[tauri::command]
pub async fn selftest_resize(window: tauri::WebviewWindow, width: u32, height: u32) -> R<()> {
    if selftest_dir().is_none() {
        return Err("self-test not enabled".into());
    }
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(format!("invalid self-test window size: {width}x{height}"));
    }
    let _ = window.unmaximize();
    let _ = window.set_size(tauri::LogicalSize::new(f64::from(width), f64::from(height)));
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            let pid = std::process::id();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                resize_on_hyprland(pid, width, height);
            })
            .await;
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
            while std::time::Instant::now() < deadline {
                if let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) {
                    let lw = (f64::from(size.width) / scale).round() as i64;
                    let lh = (f64::from(size.height) / scale).round() as i64;
                    if (lw - i64::from(width)).abs() <= 8 && (lh - i64::from(height)).abs() <= 8 {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
    }
    Ok(())
}

/// Self-test only: pretends a UWSM session is running when the `uwsm-app` found on the PATH is
/// a stub owned by the current user (a system `uwsm-app` is root-owned and never faked), so the
/// `uwsm-app --` wrapping can be observed without a real session. Returns whether it did.
#[tauri::command]
pub fn selftest_simulate_uwsm() -> R<bool> {
    if selftest_dir().is_none() {
        return Err("self-test not enabled".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Some(p) = gall_core::util::cmdline::resolve("uwsm-app") {
            if std::fs::metadata(&p).is_ok_and(|m| m.uid() != 0) {
                std::env::set_var("UWSM_FINALIZE_VARNAMES", "1");
                return Ok(true);
            }
        }
    }
    Ok(false)
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

/// A picker window (`gall --pick`) answers: writes the result file the caller waits for and
/// closes the window. A second answer (or one from a window that is no picker) is ignored.
#[tauri::command]
pub fn pick_finish(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    paths: Vec<String>,
    filter: Option<usize>,
    choices: Option<Vec<(String, String)>>,
) -> R<()> {
    answer_pick(
        &window,
        &state,
        0,
        &paths,
        filter,
        &choices.unwrap_or_default(),
    )
}

/// The user gave up (Cancel, Escape): the caller gets "cancelled".
#[tauri::command]
pub fn pick_cancel(window: tauri::WebviewWindow, state: State<'_, AppState>) -> R<()> {
    answer_pick(&window, &state, 1, &[], None, &[])
}

fn answer_pick(
    window: &tauri::WebviewWindow,
    state: &AppState,
    status: u32,
    paths: &[String],
    filter: Option<usize>,
    choices: &[(String, String)],
) -> R<()> {
    let label = window.label().to_string();
    let Some(pick) = state.window_pick(&label) else {
        return Err("this window is not a picker".into());
    };
    if state.mark_pick_done(&label) {
        // A cancel carries no choices; an answer carries exactly those the caller asked for.
        let choices = if status == 0 {
            crate::pick::sanitize_choices(&pick.choices, choices)
        } else {
            Vec::new()
        };
        if let Err(err) = crate::pick::write_result(
            &pick.result,
            &crate::pick::PickResult {
                status,
                paths,
                filter,
                choices: &choices,
            },
        ) {
            // Let the user try again instead of closing the window without any answer.
            state.pick_done.lock().unwrap().remove(&label);
            return Err(e(err));
        }
    }
    window.close().map_err(e)
}

/// Reports the end of a waiting diff or merge. The native window owns the output
/// and result paths. The webview supplies content only when accepting a merge.
#[tauri::command]
pub async fn compare_wait_finish(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    outcome: String,
    content: Option<String>,
    message: Option<String>,
) -> R<()> {
    let wait = state
        .compare_wait_window(window.label())
        .ok_or("this window is not a waiting comparison")?;
    let requested = match outcome.as_str() {
        "accepted" if wait.output.is_some() == content.is_some() => Outcome::Accepted,
        "cancelled" if content.is_none() => Outcome::Cancelled,
        "error" if content.is_none() => Outcome::Error {
            message: message.unwrap_or_else(|| "The comparison could not be completed".into()),
        },
        _ => return Err("invalid waiting comparison outcome or content".into()),
    };
    let published = tauri::async_runtime::spawn_blocking(move || {
        wait.finish(|| {
            if let (Some(output), Some(content)) = (&wait.output, content) {
                let _install_guard = mutation_guard();
                if let Err(err) = handoff::save_output(output, content.as_bytes()) {
                    return Outcome::Error {
                        message: format!("Could not save {}: {err}", output.display()),
                    };
                }
            }
            requested
        })
    })
    .await
    .map_err(e)?
    .map_err(e)?;
    if published {
        window.destroy().map_err(e)?;
    }
    Ok(())
}

/// The UI reports the active window's folder and selection, so `gall selection` can print it
/// (see `headless.rs`). Newer windows overwrite older ones; the file goes away on exit.
#[tauri::command]
pub fn selection_publish(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    folder: String,
    selection: Vec<String>,
    focused: Option<String>,
) -> R<()> {
    if state.compare_wait_window(window.label()).is_some() {
        return Ok(());
    }
    let snap = crate::headless::SelectionSnapshot {
        pid: std::process::id(),
        window: window.label().to_string(),
        folder,
        selection,
        focused,
        updated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    };
    crate::headless::write_selection(&state.dirs, &snap).map_err(e)
}

/// Debug builds with a self-test: raw payloads of the file targets on the clipboard
/// (`[[target, payload], …]`, lossy UTF-8), to verify the wire formats.
#[tauri::command]
pub async fn selftest_clip_dump(app: AppHandle) -> R<Vec<(String, String)>> {
    if selftest_dir().is_none() {
        return Err("self-test not enabled".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "linux")]
        {
            Ok(crate::linux_clipboard::dump(&app)
                .unwrap_or_default()
                .into_iter()
                .map(|(t, b)| (t, String::from_utf8_lossy(&b).into_owned()))
                .collect())
        }
        #[cfg(target_os = "macos")]
        {
            let _ = &app;
            Ok(crate::macos_clipboard::dump())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = &app;
            Ok(Vec::new())
        }
    })
    .await
    .map_err(e)?
}

/// Writes a UI message into the application log (errors, DnD diagnostics).
#[tauri::command]
pub fn ui_log(level: String, msg: String) {
    if matches!(level.as_str(), "error" | "warn") {
        crate::bug_report::record(&level, &msg);
    }
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

// ───────────────────────── startup args & multi-window ─────────────────────────

#[tauri::command]
pub fn startup_args(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    args: State<'_, crate::cli::CliArgs>,
) -> crate::cli::CliArgs {
    state
        .get_window_initial_cli(window.label())
        .unwrap_or_else(|| args.inner().clone())
}

#[tauri::command]
pub fn cli_initial_args(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    args: State<'_, crate::cli::CliArgs>,
) -> crate::cli::CliArgs {
    startup_args(window, state, args)
}

#[tauri::command]
pub fn cli_take_pending(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Vec<crate::cli::CliArgs> {
    state.take_pending_cli(window.label())
}

#[tauri::command]
pub fn filemanager_take_pending(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Vec<crate::filemanager_dbus::FileManagerAction> {
    state.take_pending_fm(window.label())
}

#[tauri::command]
pub async fn new_window(
    app: AppHandle,
    paths: Option<Vec<String>>,
    path: Option<String>,
    select_paths: Option<Vec<String>>,
    select_path: Option<String>,
    compare: Option<(String, String)>,
    git_repo: Option<String>,
) -> R<String> {
    let mut resolved_paths: Vec<String> = Vec::new();
    for raw in path.into_iter().chain(paths.into_iter().flatten()) {
        let p = npath::expand(&npath::file_uri_or_raw(&raw));
        if !p.is_empty() && !resolved_paths.contains(&p) {
            resolved_paths.push(p);
        }
    }
    let mut resolved_select: Vec<String> = Vec::new();
    for raw in select_path
        .into_iter()
        .chain(select_paths.into_iter().flatten())
    {
        let p = npath::expand(&npath::file_uri_or_raw(&raw));
        if !p.is_empty() && !resolved_select.contains(&p) {
            resolved_select.push(p);
        }
    }
    let resolved_git = git_repo
        .map(|g| npath::expand(&npath::file_uri_or_raw(&g)))
        .filter(|g| !g.is_empty());
    let resolved_compare = compare.map(|(a, b)| {
        (
            npath::expand(&npath::file_uri_or_raw(&a)),
            npath::expand(&npath::file_uri_or_raw(&b)),
        )
    });
    if resolved_paths.is_empty() {
        if let Some(ref g) = resolved_git {
            resolved_paths.push(g.clone());
        }
    }
    let cli_args = crate::cli::CliArgs {
        path1: resolved_paths.first().cloned(),
        paths: resolved_paths,
        select_paths: resolved_select,
        new_window: true,
        compare: resolved_compare,
        git_repo: resolved_git,
        ..Default::default()
    };
    crate::open_window(&app, None, cli_args)
}

// ───────────────────────── GVFS / network mounts ─────────────────────────

#[tauri::command]
pub async fn gvfs_network_mounts() -> R<Vec<gall_core::fs::gvfs::NetworkMount>> {
    tauri::async_runtime::spawn_blocking(gall_core::fs::gvfs::list_network_mounts)
        .await
        .map_err(e)
}

#[tauri::command]
pub async fn connect_server(uri: String) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gvfs::connect_server(&uri))
        .await
        .map_err(e)?
        .map_err(e)
}

#[tauri::command]
pub async fn gvfs_unmount(app: AppHandle, target: String) -> R<()> {
    off_main(app, move |state| {
        let p = Path::new(&target);
        let released = if p.is_absolute() {
            state.watcher.release_under(p)
        } else {
            Vec::new()
        };
        let res = gall_core::fs::gvfs::disconnect_mount(&target).map_err(e);
        if !released.is_empty() {
            state.watcher.resume(&released);
        }
        res
    })
    .await
}

// ───────────────────────── extensible actions / zoxide / omarchy ─────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionTargetInput {
    pub path: PathBuf,
    #[serde(default)]
    pub is_dir: bool,
    #[serde(default)]
    pub ext: String,
    #[serde(default)]
    pub mime: Option<String>,
}

impl From<ActionTargetInput> for gall_core::util::actions::ActionTarget {
    fn from(v: ActionTargetInput) -> Self {
        Self {
            path: v.path,
            is_dir: v.is_dir,
            ext: v.ext,
            mime: v.mime,
        }
    }
}

#[tauri::command]
pub async fn actions_list(app: AppHandle) -> R<Vec<gall_core::util::actions::ActionDef>> {
    off_main(app, |state| {
        Ok(gall_core::util::actions::load_actions(Some(
            &state.dirs.config,
        )))
    })
    .await
}

#[tauri::command]
pub async fn actions_for_selection(
    app: AppHandle,
    targets: Vec<ActionTargetInput>,
) -> R<Vec<gall_core::util::actions::ActionDef>> {
    off_main(app, move |state| {
        let all = gall_core::util::actions::load_actions(Some(&state.dirs.config));
        let converted: Vec<gall_core::util::actions::ActionTarget> =
            targets.into_iter().map(Into::into).collect();
        Ok(gall_core::util::actions::filter_actions(&all, &converted))
    })
    .await
}

#[tauri::command]
pub async fn action_run(
    app: AppHandle,
    action_id: String,
    paths: Vec<PathBuf>,
    dir: PathBuf,
) -> R<()> {
    off_main(app, move |state| {
        let all = gall_core::util::actions::load_actions(Some(&state.dirs.config));
        let action = all
            .into_iter()
            .find(|a| a.id == action_id)
            .ok_or_else(|| format!("action '{action_id}' not found"))?;
        let terminal = state.settings.read().unwrap().terminal.clone();
        let real_paths = if paths.is_empty() {
            Vec::new()
        } else {
            archive_view::materialize(&paths).map_err(e)?
        };
        gall_core::util::actions::run_action(&action, &real_paths, &dir, &terminal).map_err(e)
    })
    .await
}

#[tauri::command]
pub async fn zoxide_dirs() -> R<Vec<String>> {
    tauri::async_runtime::spawn_blocking(|| {
        gall_core::util::actions::zoxide_dirs()
            .map(|dirs| dirs.into_iter().map(lossy).collect())
            .map_err(e)
    })
    .await
    .map_err(e)?
}

#[tauri::command]
pub async fn omarchy_terminal_cwd() -> R<Option<String>> {
    tauri::async_runtime::spawn_blocking(|| {
        Ok(gall_core::util::actions::omarchy_terminal_cwd().map(lossy))
    })
    .await
    .map_err(e)?
}

#[tauri::command]
pub fn omarchy_state_get(state: State<'_, AppState>) -> crate::omarchy::OmarchyState {
    state.omarchy.current()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileManagerStatus {
    folder_handler: Option<String>,
    reveal_handler: Option<String>,
    reveal_available: bool,
}

#[tauri::command]
pub async fn file_manager_status() -> R<FileManagerStatus> {
    #[cfg(target_os = "linux")]
    {
        use gtk::gio::prelude::AppInfoExt;
        use zbus::fdo::{DBusProxy, Error};

        let folder = tauri::async_runtime::spawn_blocking(|| {
            gtk::gio::AppInfo::default_for_type("inode/directory", false)
                .map(|app| app.display_name().to_string())
        });
        let reveal = async {
            let connection = zbus::Connection::session().await.map_err(e)?;
            let proxy = DBusProxy::new(&connection).await.map_err(e)?;
            let name = "org.freedesktop.FileManager1".try_into().map_err(e)?;
            let owner = match proxy.get_name_owner(name).await {
                Ok(owner) => owner,
                Err(Error::NameHasNoOwner(_)) => return Ok(None),
                Err(err) => return Err(e(err)),
            };
            let pid = proxy
                .get_connection_unix_process_id(owner.as_ref().into())
                .await
                .map_err(e)?;
            let app = if pid == std::process::id() {
                "Gall".into()
            } else {
                std::fs::read_to_string(format!("/proc/{pid}/comm"))
                    .map(|name| name.trim().to_owned())
                    .unwrap_or_else(|_| owner.to_string())
            };
            Ok(Some(app))
        };
        let reveal = reveal.await;
        Ok(FileManagerStatus {
            folder_handler: folder.await.map_err(e)?,
            reveal_available: reveal.is_ok(),
            reveal_handler: reveal.unwrap_or_default(),
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("File manager status is unavailable on this platform".into())
    }
}

/// Cross-platform default file manager status (Linux, Windows, macOS).
/// Never called at startup: the Welcome setup and *Settings › Open with* call it when opened.
#[tauri::command]
pub async fn default_file_manager_status(
    state: State<'_, AppState>,
) -> R<gall_core::util::default_fm::DefaultFileManagerStatus> {
    let data_dir = state.dirs.data.clone();
    tauri::async_runtime::spawn_blocking(move || gall_core::util::default_fm::status(&data_dir))
        .await
        .map_err(e)
}

/// Sets Gall as the operating system's default file manager for the current user.
#[tauri::command]
pub async fn default_file_manager_set(
    state: State<'_, AppState>,
) -> R<gall_core::util::default_fm::DefaultFileManagerStatus> {
    let data_dir = state.dirs.data.clone();
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::util::default_fm::set_default(&data_dir).map_err(e)
    })
    .await
    .map_err(e)?
}

/// Restores the previous (or built-in system) default file manager for the current user.
#[tauri::command]
pub async fn default_file_manager_restore(
    state: State<'_, AppState>,
) -> R<gall_core::util::default_fm::DefaultFileManagerStatus> {
    let data_dir = state.dirs.data.clone();
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::util::default_fm::restore_default(&data_dir).map_err(e)
    })
    .await
    .map_err(e)?
}

#[cfg(test)]
mod tests {
    use super::{
        classify_clipboard_text, hyprland_resize_lua, hyprland_window_geometry, ActionTargetInput,
        ClipContent, SelftestCaptureMode,
    };
    use gall_core::nav::path as npath;
    use gall_core::util::actions::{filter_actions_with, ActionDef};
    use std::path::PathBuf;

    #[test]
    fn hyprland_resize_lua_targets_exact_pid_and_dimensions() {
        let lua = hyprland_resize_lua(4242, 800, 600);
        assert!(lua.contains("w.pid == 4242"));
        assert!(lua.contains("x = 800, y = 600, exact = true"));
        assert!(lua.contains("hl.dsp.window.float"));
    }

    #[test]
    fn selftest_capture_mode_parses_known_and_rejects_unknown() {
        assert_eq!(
            SelftestCaptureMode::parse(None).unwrap(),
            SelftestCaptureMode::Auto
        );
        assert_eq!(
            SelftestCaptureMode::parse(Some("  ")).unwrap(),
            SelftestCaptureMode::Auto
        );
        assert_eq!(
            SelftestCaptureMode::parse(Some("auto")).unwrap(),
            SelftestCaptureMode::Auto
        );
        assert_eq!(
            SelftestCaptureMode::parse(Some("webview")).unwrap(),
            SelftestCaptureMode::Webview
        );
        assert_eq!(
            SelftestCaptureMode::parse(Some("spectacle")).unwrap(),
            SelftestCaptureMode::Spectacle
        );
        assert_eq!(
            SelftestCaptureMode::parse(Some("grim")).unwrap(),
            SelftestCaptureMode::Grim
        );
        assert!(SelftestCaptureMode::parse(Some("scrot")).is_err());
    }

    #[test]
    fn hyprland_window_geometry_formats_valid_bounds_and_rejects_invalid() {
        assert_eq!(
            hyprland_window_geometry(r#"{"at":[120,80],"size":[1100,800]}"#).as_deref(),
            Some("120,80 1100x800")
        );
        assert_eq!(
            hyprland_window_geometry(r#"{"at":[-10,0],"size":[640,480]}"#).as_deref(),
            Some("-10,0 640x480")
        );
        assert_eq!(
            hyprland_window_geometry(r#"{"at":[0,0],"size":[0,800]}"#),
            None
        );
        assert_eq!(hyprland_window_geometry("{}"), None);
        assert_eq!(hyprland_window_geometry("not json"), None);
    }

    #[test]
    fn a_running_git_job_blocks_updates_but_not_other_mutations() {
        use std::sync::mpsc;
        use std::time::Duration;
        let (job, _cancel) = super::JobGuard::register("test-gate-job", "busy").unwrap();
        job.started();
        // The updater sees the job for as long as it runs.
        assert!(super::git_jobs_active());
        assert!(super::JobGuard::register("test-gate-job", "busy").is_err());
        // A rename, delete or undo started meanwhile gets the gate at once.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _gate = super::mutation_guard();
            let _ = tx.send(());
        });
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok());
        drop(job);
        assert!(super::JobGuard::register("test-gate-job", "busy").is_ok());
    }

    #[test]
    fn patch_save_waits_until_application_replacement_finishes() {
        use gall_core::fs::diff::DiffSource;
        use std::sync::mpsc;
        use std::time::{Duration, SystemTime, UNIX_EPOCH};
        let dest = std::env::temp_dir().join(format!(
            "gall-update-gate-{}-{}.patch",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let gate = super::mutation_guard();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let output = dest.clone();
        let worker = std::thread::spawn(move || {
            let text = |text: &str| DiffSource::Text {
                label: "test.txt".into(),
                text: text.into(),
            };
            started_tx.send(()).unwrap();
            let result = tauri::async_runtime::block_on(super::diff_save_patch(
                text("before\n"),
                text("after\n"),
                output,
                None,
            ));
            done_tx.send(result).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
        assert!(
            !dest.exists(),
            "A mutation must not write during replacement"
        );
        drop(gate);
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        let patch = std::fs::read_to_string(&dest).unwrap();
        std::fs::remove_file(dest).unwrap();
        assert!(patch.contains("-before"));
        assert!(patch.contains("+after"));
    }

    #[test]
    fn windows_file_uris_become_native_paths() {
        assert_eq!(
            npath::decode_file_uri_for("file:///C:/Users/u/My%20File.txt", true),
            Some(PathBuf::from(r"C:\Users\u\My File.txt"))
        );
        assert_eq!(
            npath::decode_file_uri_for("file://server/share/a.txt", true),
            Some(PathBuf::from(r"\\server\share\a.txt"))
        );
        assert_eq!(npath::file_uri_or_raw(r"C:\plain\path"), r"C:\plain\path");
    }

    #[cfg(not(windows))]
    #[test]
    fn file_uris_become_paths() {
        assert_eq!(
            npath::file_uri_or_raw("file:///home/u/My%20File.txt"),
            "/home/u/My File.txt"
        );
        assert_eq!(
            npath::file_uri_or_raw("file://localhost/tmp/a%25b"),
            "/tmp/a%b"
        );
        assert_eq!(
            npath::file_uri_or_raw("file:///home/u/%C4%8Crka.txt"),
            "/home/u/Črka.txt"
        );
        assert_eq!(npath::file_uri_or_raw("/plain/path"), "/plain/path");
        assert_eq!(npath::file_uri_or_raw("file:///bad%2"), "/bad%2");
    }

    #[test]
    fn action_target_input_deserializes_camel_case_and_filters() {
        let raw = r#"[{"path":"/tmp/pic.png","isDir":false},{"path":"/tmp/dir","isDir":true}]"#;
        let inputs: Vec<ActionTargetInput> = serde_json::from_str(raw).unwrap();
        let targets: Vec<gall_core::util::actions::ActionTarget> =
            inputs.into_iter().map(Into::into).collect();
        assert_eq!(targets.len(), 2);
        assert!(!targets[0].is_dir);
        assert!(targets[1].is_dir);

        let any_action: ActionDef = serde_json::from_str(
            r#"{"id":"test.any","label":"Send","command":["echo","{paths}"],"selection":"any"}"#,
        )
        .unwrap();
        let files_only: ActionDef = serde_json::from_str(
            r#"{"id":"test.files","label":"Files","command":["echo","{path}"],"selection":"files"}"#,
        )
        .unwrap();
        let matched = filter_actions_with(&[any_action, files_only], &targets, |_| true);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "test.any");
    }

    #[test]
    fn clip_content_paths_omits_none_cut_and_serializes_present_cut() {
        let plain = ClipContent::Paths {
            paths: vec!["/tmp/a".into()],
            cut: None,
        };
        let plain_json = serde_json::to_value(&plain).unwrap();
        assert_eq!(plain_json["kind"], "paths");
        assert!(plain_json.get("cut").is_none());

        let cut = ClipContent::Paths {
            paths: vec!["/tmp/a".into()],
            cut: Some(true),
        };
        let cut_json = serde_json::to_value(&cut).unwrap();
        assert_eq!(cut_json["cut"], true);

        assert_eq!(
            classify_clipboard_text("https://example.com".into()),
            ClipContent::Url {
                url: "https://example.com".into()
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn gvfs_slow_mount_guards_skip_probes_and_excerpts() {
        use super::{excerpts_guarded, video_probe_guarded};
        use std::path::Path;

        let gvfs_file = Path::new("/run/user/1000/gvfs/smb-share:server=nas,share=media/video.mp4");
        assert!(gall_core::fs::gvfs::is_slow_or_gvfs_mount(gvfs_file));

        let probe = video_probe_guarded(gvfs_file, Path::new("/tmp"));
        assert!(!probe.ffmpeg);
        assert!(probe.codec.is_none());

        let ex = excerpts_guarded(&[gvfs_file.to_string_lossy().into_owned()], 5);
        assert_eq!(ex, vec![None]);

        let scope = super::check_index_scope(
            &[PathBuf::from("/run/user/1000")],
            Path::new("/run/user/1000/gvfs/smb-share:server=nas,share=media"),
        );
        assert_eq!(
            scope,
            Some(super::ScopeInfo {
                covered: false,
                scanning: false,
                entries: 0,
            })
        );
    }
}

#[tauri::command]
pub async fn git_graph(
    path: PathBuf,
    comparison: Option<bool>,
    reflog: Option<bool>,
    limit: Option<usize>,
    reference: Option<String>,
) -> R<gall_core::fs::gitgraph::Graph> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitgraph::query_history(
            &path,
            comparison.unwrap_or(false),
            reflog.unwrap_or(false),
            limit.unwrap_or(200),
            reference.as_deref(),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
/// History and repository state in one call, with the work tree status read once.
/// Read-only: it must not wait on the mutation gate.
#[tauri::command]
pub async fn git_graph_view(
    path: PathBuf,
    comparison: Option<bool>,
    reflog: Option<bool>,
    limit: Option<usize>,
    reference: Option<String>,
) -> R<gall_core::fs::gitrepository::GraphView> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitrepository::graph_view(
            &path,
            comparison.unwrap_or(false),
            reflog.unwrap_or(false),
            limit.unwrap_or(200),
            reference.as_deref().filter(|s| !s.is_empty()),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[derive(serde::Serialize)]
pub struct ScopedGitHistory {
    history: gall_core::fs::gitinspect::FileHistoryPage,
    repository: gall_core::fs::gitrepository::Repository,
}

#[tauri::command]
pub async fn git_scoped_history(path: PathBuf, file: PathBuf, limit: usize) -> R<ScopedGitHistory> {
    tauri::async_runtime::spawn_blocking(move || {
        let repository = gall_core::fs::gitrepository::snapshot(&path).map_err(e)?;
        let history = gall_core::fs::gitinspect::scoped_history(
            std::path::Path::new(&repository.root),
            &file,
            limit,
            Some(repository.head.as_str()).filter(|head| !head.is_empty()),
        )
        .map_err(e)?;
        Ok(ScopedGitHistory {
            history,
            repository,
        })
    })
    .await
    .map_err(e)?
}

/// Where HEAD and the branches have been. Reads files only.
#[tauri::command]
pub async fn git_reflog(path: PathBuf) -> R<gall_core::fs::gitreflog::Reflog> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitreflog::reflog(&path))
        .await
        .map_err(e)?
        .map_err(e)
}
/// The operations of this work tree that can be undone and redone. Reads files only.
#[tauri::command]
pub async fn git_journal(path: PathBuf) -> R<gall_core::fs::gitjournal::View> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitjournal::view(&path))
        .await
        .map_err(e)?
        .map_err(e)
}
/// Runs a blocking Git operation with structured throttled `"git-progress"` events when
/// `progress_id` is supplied. When `None`, executes `f` directly with zero overhead.
fn run_with_git_progress<R>(
    app: &AppHandle,
    progress_id: Option<String>,
    operation: &str,
    phase: &str,
    pipeline: gall_core::fs::gitprogress::Pipeline,
    cancellable: bool,
    f: impl FnOnce() -> R,
) -> R {
    let Some(id) = progress_id.filter(|s| !s.is_empty()) else {
        return f();
    };
    let app = app.clone();
    let tracker = Arc::new(std::sync::Mutex::new(
        gall_core::fs::gitprogress::ProgressTracker::new(
            id,
            operation,
            pipeline,
            cancellable,
            move |progress| {
                let _ = app.emit("git-progress", progress);
            },
        ),
    ));
    if let Ok(mut guard) = tracker.lock() {
        guard.start(phase);
    }
    gall_core::fs::gitprogress::with_tracker(tracker, f)
}

fn repository_action_meta(
    request: &gall_core::fs::gitrepository::Request,
) -> (&'static str, &'static str) {
    use gall_core::fs::gitrepository::Request;
    match request {
        Request::Merge { .. } | Request::MergeWithMode { .. } => ("merge", "Merging"),
        Request::Rebase { .. } => ("rebase", "Rebasing"),
        Request::CherryPick { .. } | Request::CherryPickMany { .. } => {
            ("cherryPick", "Cherry-picking")
        }
        Request::Revert { .. } => ("revert", "Reverting"),
        Request::IntegrateMerge { revert: true, .. } => ("revert", "Reverting"),
        Request::IntegrateMerge { revert: false, .. } => ("cherryPick", "Cherry-picking"),
        Request::Reset { .. } => ("reset", "Resetting working tree"),
        Request::StashSave { .. } => ("stash", "Saving stash"),
        Request::StashApply { .. } => ("stash", "Applying stash"),
        Request::StashPop { .. } => ("stash", "Popping stash"),
        Request::StashBranch { .. } => ("stash", "Creating branch from stash"),
        Request::Continue => ("continue", "Continuing operation"),
        Request::Skip => ("skip", "Skipping step"),
        Request::Abort => ("abort", "Aborting operation"),
        _ => ("repository", "Running Git operation"),
    }
}

fn advanced_action_meta(
    request: &gall_core::fs::gitadvanced::Request,
) -> (
    &'static str,
    &'static str,
    gall_core::fs::gitprogress::Pipeline,
) {
    use gall_core::fs::gitadvanced::Request;
    use gall_core::fs::gitprogress::Pipeline;
    match request {
        Request::Clone { .. } => ("clone", "Cloning repository", Pipeline::Clone),
        Request::CreateWorktree { .. } | Request::QuickWorktree(_) => {
            ("worktree", "Creating worktree", Pipeline::Worktree)
        }
        Request::UndoWorktree { .. }
        | Request::RemoveWorktree { .. }
        | Request::RemoveWorktreeWithIgnored { .. }
        | Request::RemoveWorktreeWithSnapshot { .. } => {
            ("worktree", "Removing worktree", Pipeline::Single)
        }
        Request::MoveWorktree { .. } => ("worktree", "Moving worktree", Pipeline::Single),
        Request::RepairWorktree { .. } => ("worktree", "Repairing worktree", Pipeline::Single),
        Request::PruneWorktrees => ("worktree", "Pruning worktrees", Pipeline::Single),
        Request::InitSubmodule { .. } | Request::UpdateSubmodule { .. } => {
            ("submodule", "Updating submodule", Pipeline::Fetch)
        }
        Request::DeinitSubmodule { .. } => {
            ("submodule", "Deinitializing submodule", Pipeline::Single)
        }
        Request::BisectStart { .. }
        | Request::BisectGood
        | Request::BisectBad
        | Request::BisectSkip
        | Request::BisectReset
        | Request::BisectCheckout => ("bisect", "Running bisect", Pipeline::Single),
        Request::ExportPatch { .. } | Request::ExportPatches { .. } => {
            ("patch", "Exporting patches", Pipeline::Single)
        }
        Request::ApplyPatch { .. } | Request::ApplyPatchText { .. } => {
            ("patch", "Applying patch", Pipeline::Single)
        }
        _ => ("advanced", "Running Git operation", Pipeline::Single),
    }
}

/// Undoes or redoes until `applied` operations are applied; `expected` is the number shown.
#[tauri::command]
pub async fn git_journal_move(
    app: AppHandle,
    path: PathBuf,
    applied: usize,
    expected: usize,
    progress_id: Option<String>,
) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        run_with_git_progress(
            &app,
            progress_id,
            "journal",
            "Restoring Git state",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || gall_core::fs::gitjournal::move_to(&path, applied, expected),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
/// The number of the latest commit search of each window; an older one stops.
static GIT_SEARCHES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
    std::sync::LazyLock::new(Default::default);

fn git_search_latest(window: &str, ticket: u64) -> u64 {
    let mut all = GIT_SEARCHES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let latest = all.entry(window.to_owned()).or_default();
    *latest = (*latest).max(ticket);
    *latest
}

#[derive(Clone, serde::Serialize)]
struct GitSearchHits {
    ticket: u64,
    lines: String,
}

/// Searches the whole history. Matches arrive as `git-search-hits` events with the ticket of
/// the search; a newer ticket of the same window (or [`git_search_cancel`]) stops it.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn git_search(
    app: AppHandle,
    window: tauri::WebviewWindow,
    path: PathBuf,
    query: gall_core::fs::gitsearch::CommitQuery,
    reflog: bool,
    reference: Option<String>,
    ticket: u64,
    progress_id: Option<String>,
) -> R<gall_core::fs::gitsearch::SearchEnd> {
    let label = window.label().to_owned();
    git_search_latest(&label, ticket);
    tauri::async_runtime::spawn_blocking(move || {
        run_with_git_progress(
            &app,
            progress_id,
            "search",
            "Searching history",
            gall_core::fs::gitprogress::Pipeline::Single,
            true,
            || {
                gall_core::fs::gitsearch::search(
                    &path,
                    &query,
                    reflog,
                    reference.as_deref().filter(|name| !name.is_empty()),
                    &|| git_search_latest(&label, 0) != ticket,
                    |lines| {
                        let _ = window.emit_to(
                            tauri::EventTarget::webview_window(&label),
                            "git-search-hits",
                            GitSearchHits { ticket, lines },
                        );
                    },
                )
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Stops every search of the window up to `ticket`.
#[tauri::command]
pub fn git_search_cancel(window: tauri::WebviewWindow, ticket: u64) {
    git_search_latest(window.label(), ticket + 1);
}
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn git_switch(
    app: AppHandle,
    path: PathBuf,
    target: String,
    branch: bool,
    action: String,
    message: String,
    expected: Option<String>,
    progress_id: Option<String>,
) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        run_with_git_progress(
            &app,
            progress_id,
            "checkout",
            if branch {
                "Switching branch"
            } else {
                "Checking out commit"
            },
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || {
                if let Some(expected) = expected {
                    if gall_core::fs::gitrepository::snapshot(&path)?.token != expected {
                        return Err(gall_core::CoreError::Other(
                            "Repository changed. Refresh and review checkout again.".into(),
                        ));
                    }
                }
                gall_core::fs::gitgraph::switch(&path, &target, branch, &action, &message)
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}

#[tauri::command]
pub async fn git_wip_list(path: PathBuf) -> R<Vec<gall_core::fs::gitwip::Wip>> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitwip::list(&path))
        .await
        .map_err(e)?
        .map_err(e)
}
#[tauri::command]
pub async fn git_wip_discard(path: PathBuf, branch: String) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        gall_core::fs::gitwip::discard(&path, &branch)
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_wip_stash(path: PathBuf, branch: String) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        gall_core::fs::gitwip::to_stash(&path, &branch)
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_workspace(path: PathBuf) -> R<gall_core::fs::gitworktree::Workspace> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitworktree::workspace(&path))
        .await
        .map_err(e)?
        .map_err(e)
}
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn git_index_action(
    app: AppHandle,
    path: PathBuf,
    action: gall_core::fs::gitworktree::IndexAction,
    file: Option<String>,
    message: String,
    expected: String,
    progress_id: Option<String>,
) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        run_with_git_progress(
            &app,
            progress_id,
            "index",
            "Updating Git index",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || {
                gall_core::fs::gitworktree::index_action(
                    &path,
                    action,
                    file.as_deref(),
                    &message,
                    &expected,
                )
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_file_patch(path: PathBuf, file: String, staged: bool) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitworktree::patch(&path, &file, staged)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

#[tauri::command]
pub async fn git_checkout_commit_file(path: PathBuf, commit: String, file: String) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitrepository::checkout_commit_file(&path, &commit, &file)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

#[tauri::command]
pub async fn git_commit_details(
    path: PathBuf,
    commit: String,
    base: Option<String>,
) -> R<gall_core::fs::gitinspect::Details> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitinspect::details(&path, &commit, base.as_deref())
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_commit_diff(path: PathBuf, commit: String) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitinspect::commit_diff(&path, &commit)
    })
    .await
    .map_err(e)?
    .map_err(e)
}
/// The commit as an applicable email-style patch, for the clipboard.
#[tauri::command]
pub async fn git_commit_patch_text(path: PathBuf, commit: String) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitinspect::commit_patch_text(&path, &commit)
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_commit_patch(
    path: PathBuf,
    commit: String,
    base: Option<String>,
    file: String,
) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitinspect::patch(&path, &commit, base.as_deref(), &file)
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_worktree_comparison(
    path: PathBuf,
) -> R<gall_core::fs::gitinspect::WorktreeComparison> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitinspect::worktree_comparison(&path)
    })
    .await
    .map_err(e)?
    .map_err(e)
}

#[tauri::command]
pub async fn git_file_history(
    app: AppHandle,
    path: PathBuf,
    file: String,
    limit: Option<usize>,
    commit: Option<String>,
    progress_id: Option<String>,
) -> R<Vec<gall_core::fs::gitinspect::HistoryEntry>> {
    tauri::async_runtime::spawn_blocking(move || {
        run_with_git_progress(
            &app,
            progress_id,
            "fileHistory",
            "Loading file history",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || {
                gall_core::fs::gitinspect::file_history(
                    &path,
                    &file,
                    limit.unwrap_or(200),
                    commit.as_deref(),
                )
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
/// The history of lines of a file. A newer ticket of the same window (or
/// [`git_line_history_cancel`]) stops it.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn git_line_history(
    app: AppHandle,
    window: tauri::WebviewWindow,
    path: PathBuf,
    commit: String,
    file: String,
    start: u32,
    end: u32,
    ticket: u64,
    progress_id: Option<String>,
) -> R<gall_core::fs::gitinspect::LineHistory> {
    let label = format!("lines:{}", window.label());
    git_search_latest(&label, ticket);
    tauri::async_runtime::spawn_blocking(move || {
        run_with_git_progress(
            &app,
            progress_id,
            "lineHistory",
            "Loading line history",
            gall_core::fs::gitprogress::Pipeline::Single,
            true,
            || {
                gall_core::fs::gitinspect::line_history(
                    &path,
                    &commit,
                    &file,
                    (start, end),
                    200,
                    &|| git_search_latest(&label, 0) != ticket,
                )
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
/// Stops the line history of the window up to `ticket`.
#[tauri::command]
pub fn git_line_history_cancel(window: tauri::WebviewWindow, ticket: u64) {
    git_search_latest(&format!("lines:{}", window.label()), ticket + 1);
}
#[tauri::command]
pub async fn git_commit_content(path: PathBuf, commit: String, file: String) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitinspect::content(&path, &commit, &file)
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_conflict(
    path: PathBuf,
    file: String,
) -> R<gall_core::fs::gitrepository::Conflict> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitrepository::conflict(&path, &file)
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[derive(Clone, serde::Serialize)]
struct GitBlamePart {
    ticket: u64,
    part: gall_core::fs::gitblame::BlamePart,
}

/// Blame of a file. Lines arrive as `git-blame-part` events with the ticket; a second blame of the
/// window does not stop the first, [`git_blame_cancel`] does. With `absolute` the file is a path on
/// disk, else a name in the repository of `path`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn git_blame_lines(
    app: AppHandle,
    window: tauri::WebviewWindow,
    path: PathBuf,
    commit: Option<String>,
    file: String,
    absolute: bool,
    ticket: u64,
    progress_id: Option<String>,
) -> R<gall_core::fs::gitblame::BlameEnd> {
    let label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        run_with_git_progress(
            &app,
            progress_id,
            "blame",
            "Blaming file",
            gall_core::fs::gitprogress::Pipeline::Single,
            true,
            || {
                let (root, name) = if absolute {
                    gall_core::fs::gitblame::locate(Path::new(&file))?
                } else {
                    (path, file)
                };
                let end = gall_core::fs::gitblame::blame(
                    &root,
                    commit.as_deref(),
                    &name,
                    &|| blame_cancelled(&label, ticket, false),
                    |part| {
                        let _ = window.emit_to(
                            tauri::EventTarget::webview_window(&label),
                            "git-blame-part",
                            GitBlamePart { ticket, part },
                        );
                    },
                );
                blame_cancelled(&label, ticket, true);
                end
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Which commits the staged changes belong to. Changes nothing.
#[tauri::command]
pub async fn git_absorb_propose(
    app: AppHandle,
    window: tauri::WebviewWindow,
    path: PathBuf,
    ticket: u64,
    progress_id: Option<String>,
) -> R<gall_core::fs::gitabsorb::Proposal> {
    let label = format!("absorb:{}", window.label());
    tauri::async_runtime::spawn_blocking(move || {
        run_with_git_progress(
            &app,
            progress_id,
            "absorb",
            "Analyzing staged changes",
            gall_core::fs::gitprogress::Pipeline::Single,
            true,
            || {
                let result = gall_core::fs::gitabsorb::propose_cancellable(&path, &|| {
                    blame_cancelled(&label, ticket, false)
                });
                blame_cancelled(&label, ticket, true);
                result
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}

#[tauri::command]
pub fn git_absorb_cancel(window: tauri::WebviewWindow, ticket: u64) {
    cancel_blame(format!("absorb:{}", window.label()), ticket);
}
/// Puts staged changes into commits; returns how many were rewritten.
#[tauri::command]
pub async fn git_absorb_apply(
    app: AppHandle,
    path: PathBuf,
    assignments: Vec<gall_core::fs::gitabsorb::Assignment>,
    expected: String,
    progress_id: Option<String>,
) -> R<usize> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        run_with_git_progress(
            &app,
            progress_id,
            "absorb",
            "Absorbing changes",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || gall_core::fs::gitabsorb::apply(&path, &assignments, &expected),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
/// A version of an image of a repository as a file to show.
#[tauri::command]
pub async fn git_image(
    path: PathBuf,
    side: gall_core::fs::gitimages::Side,
) -> R<gall_core::fs::gitimages::Image> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitimages::image(&path, &side))
        .await
        .map_err(e)?
        .map_err(e)
}
/// What merging one commit into another would do; `git_forecast_cancel` stops it.
#[tauri::command]
pub async fn git_forecast(
    window: tauri::WebviewWindow,
    path: PathBuf,
    ours: String,
    theirs: String,
    ticket: u64,
) -> R<gall_core::fs::gitforecast::Forecast> {
    let label = format!("forecast:{}", window.label());
    tauri::async_runtime::spawn_blocking(move || {
        let found = gall_core::fs::gitforecast::forecast(&path, &ours, &theirs, &|| {
            blame_cancelled(&label, ticket, false)
        });
        blame_cancelled(&label, ticket, true);
        found
    })
    .await
    .map_err(e)?
    .map_err(e)
}

/// Where one of the three inputs of a merge comes from.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MergeSource {
    Text {
        text: String,
    },
    /// A file on disk; one that does not exist is a missing stage, not an error.
    File {
        path: PathBuf,
    },
    /// A stage of an unresolved file: 1 the common ancestor, 2 ours, 3 theirs.
    GitStage {
        path: PathBuf,
        file: String,
        stage: u8,
    },
}

/// The three inputs of a merge; `None`: the stage does not exist.
#[derive(Deserialize)]
pub struct MergeRequest {
    base: Option<MergeSource>,
    left: Option<MergeSource>,
    right: Option<MergeSource>,
}

fn merge_bytes(source: &Option<MergeSource>) -> R<Option<Vec<u8>>> {
    use std::io::Read;
    let limit = gall_core::merge::MAX_STAGE_BYTES;
    match source {
        None => Ok(None),
        Some(MergeSource::Text { text }) => Ok(Some(text.clone().into_bytes())),
        Some(MergeSource::File { path }) => match std::fs::File::open(path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                // One byte over the limit tells the merge that the file is too large.
                file.take(limit as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(e)?;
                Ok(Some(bytes))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(e(error)),
        },
        Some(MergeSource::GitStage { path, file, stage }) => {
            gall_core::fs::gitinspect::conflict_stage(path, file, *stage, limit).map_err(e)
        }
    }
}

/// Runs `work` on the inputs of a request off the main thread; `merge_cancel` with the ticket stops it.
async fn merge_job<T: Send + 'static>(
    window: tauri::WebviewWindow,
    request: MergeRequest,
    ticket: u64,
    work: impl FnOnce(&gall_core::merge::Inputs, &dyn Fn() -> bool) -> gall_core::Result<T>
        + Send
        + 'static,
) -> R<T> {
    let label = format!("merge:{}", window.label());
    tauri::async_runtime::spawn_blocking(move || {
        let found = (|| {
            let (base, left, right) = (
                merge_bytes(&request.base)?,
                merge_bytes(&request.left)?,
                merge_bytes(&request.right)?,
            );
            let inputs = gall_core::merge::Inputs {
                base: base.as_deref(),
                left: left.as_deref(),
                right: right.as_deref(),
            };
            work(&inputs, &|| blame_cancelled(&label, ticket, false)).map_err(e)
        })();
        blame_cancelled(&label, ticket, true);
        found
    })
    .await
    .map_err(e)?
}

/// The chunks of a three-way merge, or why there is none (binary, a missing or too large stage). Reads only.
#[tauri::command]
pub async fn merge_three_way(
    window: tauri::WebviewWindow,
    request: MergeRequest,
    ticket: u64,
) -> R<gall_core::merge::Outcome> {
    merge_job(window, request, ticket, |inputs, cancel| {
        gall_core::merge::merge(inputs, cancel)
    })
    .await
}

/// The result of the merge with a choice per conflict, only when every conflict has one. Nothing is written.
#[tauri::command]
pub async fn merge_resolve(
    window: tauri::WebviewWindow,
    request: MergeRequest,
    choices: std::collections::BTreeMap<String, gall_core::merge::Choice>,
    ticket: u64,
) -> R<gall_core::merge::Resolution> {
    merge_job(window, request, ticket, move |inputs, cancel| {
        gall_core::merge::resolve(inputs, &choices, cancel)
    })
    .await
}

/// The unfinished merge with markers for the conflicts that have no choice yet; `None` when the inputs are not text.
#[tauri::command]
pub async fn merge_preview(
    window: tauri::WebviewWindow,
    request: MergeRequest,
    choices: std::collections::BTreeMap<String, gall_core::merge::Choice>,
    ticket: u64,
) -> R<Option<gall_core::merge::Preview>> {
    merge_job(window, request, ticket, move |inputs, cancel| {
        gall_core::merge::preview(inputs, &choices, cancel)
    })
    .await
}

/// Stops a merge of the window.
#[tauri::command]
pub fn merge_cancel(window: tauri::WebviewWindow, ticket: u64) {
    cancel_blame(format!("merge:{}", window.label()), ticket);
}

/// Runs PDF extraction in an isolated child process with memory and wall-clock limits so a crafted
/// PDF cannot retain memory in the main app process and cancellation kills it immediately.
fn extract_pdf_isolated(
    path: &std::path::Path,
    cancelled: &dyn Fn() -> bool,
) -> std::result::Result<gall_core::doc_compare::Extracted, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    if cancelled() {
        return Err("Cancelled".into());
    }
    let meta = std::fs::metadata(path).map_err(e)?;
    if meta.len() > gall_core::doc_compare::MAX_PDF_BYTES as u64 {
        return Err(format!(
            "The PDF is larger than {} MiB, the most that is read for a comparison",
            gall_core::doc_compare::MAX_PDF_BYTES / (1024 * 1024)
        ));
    }
    let Ok(exe) = std::env::current_exe() else {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(e)?
            .take((gall_core::doc_compare::MAX_PDF_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(e)?;
        return gall_core::doc_compare::extract_pdf(&bytes, cancelled).map_err(e);
    };
    let mut child = Command::new(exe)
        .arg(gall_core::doc_compare::PDF_WORKER_ARG)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(e)?;
    let stdout = child.stdout.take().ok_or("pdf worker stdout missing")?;
    let max_json = (gall_core::doc_compare::MAX_TEXT_BYTES * 4) as u64;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.take(max_json).read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("Cancelled".into());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("Reading the PDF timed out".into());
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(err.to_string());
            }
        }
    };
    let payload = reader.join().unwrap_or_default();
    if !status.success() || payload.is_empty() {
        return Err("The PDF could not be read safely (memory or format limit exceeded)".into());
    }
    serde_json::from_slice::<std::result::Result<gall_core::doc_compare::Extracted, String>>(
        &payload,
    )
    .map_err(|err| format!("The PDF worker returned invalid output: {err}"))?
}

/// The text and positioned layout blocks of a document (`.docx` or `.pdf`) for comparison.
/// Reads the file up to a limit, never follows an external link, and `doc_extract_cancel` stops it.
/// PDF extraction runs in an isolated child process with memory and wall-clock limits.
#[tauri::command]
pub async fn doc_extract(
    window: tauri::WebviewWindow,
    path: PathBuf,
    ticket: u64,
) -> R<gall_core::doc_compare::Extracted> {
    let label = format!("doc:{}", window.label());
    tauri::async_runtime::spawn_blocking(move || {
        let found = (|| {
            let extension = path
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if extension != "docx" && extension != "pdf" {
                return Err(
                    "Only Word documents (.docx) and PDF files can be compared as text".to_string(),
                );
            }
            let cancelled = || blame_cancelled(&label, ticket, false);
            if extension == "pdf" {
                extract_pdf_isolated(&path, &cancelled)
            } else {
                use std::io::Read;
                let limit = gall_core::doc_compare::MAX_FILE_BYTES as u64;
                let mut bytes = Vec::new();
                std::fs::File::open(&path)
                    .map_err(e)?
                    .take(limit + 1)
                    .read_to_end(&mut bytes)
                    .map_err(e)?;
                gall_core::doc_compare::extract_docx(&bytes, &cancelled).map_err(e)
            }
        })();
        blame_cancelled(&label, ticket, true);
        found
    })
    .await
    .map_err(e)?
}

/// Stops the extraction of the window.
#[tauri::command]
pub fn doc_extract_cancel(window: tauri::WebviewWindow, ticket: u64) {
    cancel_blame(format!("doc:{}", window.label()), ticket);
}

/// Stops a forecast of the window.
#[tauri::command]
pub fn git_forecast_cancel(window: tauri::WebviewWindow, ticket: u64) {
    cancel_blame(format!("forecast:{}", window.label()), ticket);
}

/// Cancelled blames and forecasts that may still be running, by window and ticket.
static CANCELLED_BLAMES: std::sync::Mutex<std::collections::BTreeSet<(String, u64)>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Whether a blame was cancelled; `ended` forgets it.
fn blame_cancelled(label: &str, ticket: u64, ended: bool) -> bool {
    let Ok(mut all) = CANCELLED_BLAMES.lock() else {
        return false;
    };
    let key = (label.to_owned(), ticket);
    if ended {
        all.remove(&key)
    } else {
        all.contains(&key)
    }
}

/// Stops a blame of the window.
#[tauri::command]
pub fn git_blame_cancel(window: tauri::WebviewWindow, ticket: u64) {
    cancel_blame(window.label().to_owned(), ticket);
}

fn cancel_blame(label: String, ticket: u64) {
    if let Ok(mut all) = CANCELLED_BLAMES.lock() {
        // One that had ended before is never asked for again: the oldest are dropped.
        while all.len() >= 256 {
            all.pop_first();
        }
        all.insert((label, ticket));
    }
}
#[tauri::command]
pub async fn git_repository(path: PathBuf) -> R<gall_core::fs::gitrepository::Repository> {
    // Read-only: must not wait on the update/mutation gate, which long
    // network jobs hold for their whole duration.
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitrepository::snapshot(&path))
        .await
        .map_err(e)?
        .map_err(e)
}
#[tauri::command]
pub async fn git_repository_action(
    app: AppHandle,
    path: PathBuf,
    request: gall_core::fs::gitrepository::Request,
    expected: String,
    progress_id: Option<String>,
) -> R<String> {
    let (operation, phase) = repository_action_meta(&request);
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        run_with_git_progress(
            &app,
            progress_id,
            operation,
            phase,
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || gall_core::fs::gitrepository::execute(&path, request, &expected),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_patch_selection(
    path: PathBuf,
    file: String,
    staged: bool,
) -> R<gall_core::fs::gitworktree::SelectionPatch> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitworktree::selection_patch(&path, &file, staged)
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_apply_selection(
    app: AppHandle,
    path: PathBuf,
    file: String,
    staged: bool,
    selected: Vec<usize>,
    expected: String,
    progress_id: Option<String>,
) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        run_with_git_progress(
            &app,
            progress_id,
            "index",
            if staged {
                "Unstaging changes"
            } else {
                "Staging changes"
            },
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || {
                gall_core::fs::gitworktree::apply_selection(
                    &path, &file, staged, &selected, &expected,
                )
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn git_worktree_transfer(
    app: AppHandle,
    path: PathBuf,
    target: PathBuf,
    file: String,
    staged: bool,
    selected: Option<Vec<usize>>,
    expected: Option<String>,
    apply: bool,
    progress_id: Option<String>,
) -> R<gall_core::fs::gitworktree::Transfer> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = apply.then(mutation_guard);
        run_with_git_progress(
            &app,
            progress_id,
            "transfer",
            "Transferring changes",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || {
                gall_core::fs::gitworktree::transfer(
                    &path,
                    &target,
                    &file,
                    staged,
                    selected.as_deref(),
                    expected.as_deref(),
                    apply,
                )
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
/// Files and age of every stash.
#[tauri::command]
pub async fn git_stash_list(path: PathBuf) -> R<Vec<gall_core::fs::gitstash::StashInfo>> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitstash::list(&path))
        .await
        .map_err(e)?
        .map_err(e)
}
/// The changes of a stash.
#[tauri::command]
pub async fn git_stash_preview(
    path: PathBuf,
    id: String,
) -> R<gall_core::fs::gitstash::StashPreview> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitstash::preview(&path, &id))
        .await
        .map_err(e)?
        .map_err(e)
}
/// Stashes chosen lines of a file.
#[tauri::command]
pub async fn git_stash_selection(
    app: AppHandle,
    path: PathBuf,
    file: String,
    selected: Vec<usize>,
    message: String,
    expected: String,
    progress_id: Option<String>,
) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        run_with_git_progress(
            &app,
            progress_id,
            "stash",
            "Saving stash",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || {
                gall_core::fs::gitworktree::stash_selection(
                    &path, &file, &selected, &message, &expected,
                )
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_discard_selection(
    app: AppHandle,
    path: PathBuf,
    file: String,
    selected: Vec<usize>,
    expected: String,
    progress_id: Option<String>,
) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        run_with_git_progress(
            &app,
            progress_id,
            "index",
            "Discarding changes",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || gall_core::fs::gitworktree::discard_selection(&path, &file, &selected, &expected),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}

type NetworkJobs = std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>;
static GIT_NETWORK_JOBS: std::sync::LazyLock<NetworkJobs> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Is a Git network or advanced job running? An update must not replace the
/// application under it (`updates::update_ready`).
pub fn git_jobs_active() -> bool {
    !GIT_NETWORK_JOBS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_empty()
}

/// Registers a cancellable job and unregisters it on drop. The job's worker owns the
/// guard, so the entry lasts exactly as long as the work, also when the command future
/// is dropped mid-await.
struct JobGuard {
    id: String,
}

impl JobGuard {
    fn register(id: &str, busy: &str) -> Result<(Self, Arc<AtomicBool>), String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut jobs = GIT_NETWORK_JOBS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if jobs.contains_key(id) {
            return Err(busy.into());
        }
        jobs.insert(id.to_owned(), cancel.clone());
        Ok((Self { id: id.to_owned() }, cancel))
    }
}

impl JobGuard {
    /// Called by the worker before it starts. These jobs run for minutes (a slow remote),
    /// so they do not hold the mutation gate, which would make every rename, delete, undo
    /// and copy wait for them. The update installer holds the gate while it checks for
    /// registered jobs: passing it once here means that either the installer saw this job
    /// and refused, or it is replacing the application and the job never starts.
    fn started(&self) {
        drop(mutation_guard());
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        GIT_NETWORK_JOBS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn git_network(
    app: AppHandle,
    path: PathBuf,
    id: String,
    remote: String,
    action: gall_core::fs::gitnetwork::NetworkAction,
    expected: String,
    lease: Option<String>,
    branch_name: Option<String>,
) -> R<String> {
    let (registration, cancel) = JobGuard::register(&id, "Remote operation ID is already active")?;
    let job = id.clone();
    let (operation, phase, pipeline) = match action {
        gall_core::fs::gitnetwork::NetworkAction::Fetch => (
            "fetch",
            "Fetching from remote",
            gall_core::fs::gitprogress::Pipeline::Fetch,
        ),
        gall_core::fs::gitnetwork::NetworkAction::Pull
        | gall_core::fs::gitnetwork::NetworkAction::PullMerge
        | gall_core::fs::gitnetwork::NetworkAction::PullRebase => (
            "pull",
            "Pulling from remote",
            gall_core::fs::gitprogress::Pipeline::Pull,
        ),
        gall_core::fs::gitnetwork::NetworkAction::Push
        | gall_core::fs::gitnetwork::NetworkAction::ForcePush
        | gall_core::fs::gitnetwork::NetworkAction::PushTag
        | gall_core::fs::gitnetwork::NetworkAction::DeleteTag
        | gall_core::fs::gitnetwork::NetworkAction::DeleteBranch => (
            "push",
            "Pushing to remote",
            gall_core::fs::gitprogress::Pipeline::Push,
        ),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let registration = registration;
        registration.started();
        run_with_git_progress(&app, Some(id), operation, phase, pipeline, true, || {
            gall_core::fs::gitnetwork::run(
                &path,
                &remote,
                action,
                &expected,
                lease.as_deref(),
                branch_name.as_deref(),
                &cancel,
                |text| {
                    let _ = app.emit(
                        "git-network-progress",
                        gall_core::fs::gitnetwork::Progress {
                            id: job.clone(),
                            text,
                        },
                    );
                },
            )
        })
    })
    .await
    .map_err(e)
    .and_then(|v| v.map_err(e))
}
// Reservations close the gap between requesting a remote tag lookup and registering
// its cancellation token. The client releases every successful reservation on cleanup.
type RemoteTagReservations =
    std::sync::Mutex<std::collections::HashMap<String, (JobGuard, Arc<AtomicBool>)>>;
static REMOTE_TAG_RESERVATIONS: std::sync::LazyLock<RemoteTagReservations> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[tauri::command]
pub fn git_remote_tags_register(id: String) -> R<()> {
    let mut reservations = REMOTE_TAG_RESERVATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let job = JobGuard::register(&id, "Remote operation ID is already active")?;
    reservations.insert(id, job);
    Ok(())
}

#[tauri::command]
pub async fn git_remote_tags(
    app: AppHandle,
    path: PathBuf,
    remote: String,
    id: String,
) -> R<Vec<gall_core::fs::gitnetwork::RemoteTag>> {
    let (registration, cancel) = REMOTE_TAG_RESERVATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id)
        .ok_or_else(|| "Remote tag lookup was cancelled or not registered".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let registration = registration;
        registration.started();
        run_with_git_progress(
            &app,
            Some(id),
            "remoteTags",
            "Listing remote tags",
            gall_core::fs::gitprogress::Pipeline::Single,
            true,
            || gall_core::fs::gitnetwork::remote_tags(&path, &remote, &cancel),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}

#[tauri::command]
pub fn git_remote_tags_release(id: String) {
    let reservation = REMOTE_TAG_RESERVATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id);
    if reservation.is_none() {
        git_network_cancel(id);
    }
}

#[cfg(test)]
mod remote_tag_job_tests {
    use super::*;

    #[test]
    fn release_removes_an_unstarted_remote_tag_job() {
        let id = "remote-tag-test-unstarted";
        git_remote_tags_register(id.into()).unwrap();
        assert!(git_remote_tags_register(id.into()).is_err());
        git_remote_tags_release(id.into());
        assert!(!REMOTE_TAG_RESERVATIONS.lock().unwrap().contains_key(id));
        assert!(!GIT_NETWORK_JOBS.lock().unwrap().contains_key(id));
    }

    #[test]
    fn release_cancels_a_started_remote_tag_job() {
        let id = "remote-tag-test-started";
        git_remote_tags_register(id.into()).unwrap();
        let (guard, cancel) = REMOTE_TAG_RESERVATIONS.lock().unwrap().remove(id).unwrap();
        git_remote_tags_release(id.into());
        assert!(cancel.load(Ordering::Relaxed));
        drop(guard);
        assert!(!GIT_NETWORK_JOBS.lock().unwrap().contains_key(id));
    }
}

#[tauri::command]
pub fn git_network_cancel(id: String) {
    if let Some(cancel) = GIT_NETWORK_JOBS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&id)
    {
        cancel.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub async fn git_advanced(
    path: PathBuf,
    commit: Option<String>,
) -> R<gall_core::fs::gitadvanced::AdvancedSnapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitadvanced::snapshot(&path, commit.as_deref())
    })
    .await
    .map_err(e)?
    .map_err(e)
}
/// The worktrees of a repository, the most recently used first. Reads files only.
#[tauri::command]
pub async fn git_worktree_places(path: PathBuf) -> R<Vec<gall_core::fs::gitworktrees::Place>> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitworktrees::places(&path))
        .await
        .map_err(e)?
        .map_err(e)
}
/// The state of one worktree: asked for rows on screen.
#[tauri::command]
pub async fn git_worktree_state(path: PathBuf) -> R<gall_core::fs::gitworktrees::State> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitworktrees::state(&path))
        .await
        .map_err(e)?
        .map_err(e)
}
/// Worktrees that look finished. Removes nothing.
#[tauri::command]
pub async fn git_worktree_cleanup(path: PathBuf) -> R<Vec<gall_core::fs::gitworktrees::Candidate>> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitworktrees::cleanup(&path))
        .await
        .map_err(e)?
        .map_err(e)
}
/// Where a new worktree would go: the live preview of a location template.
#[tauri::command]
pub async fn git_custom_run(
    app: AppHandle,
    path: PathBuf,
    id: String,
    argv: Vec<String>,
) -> R<gall_core::fs::gitcustom::Outcome> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        run_with_git_progress(
            &app,
            Some(id.clone()),
            "custom",
            "Running command",
            gall_core::fs::gitprogress::Pipeline::Single,
            true,
            || gall_core::fs::gitcustom::run(&path, &id, &argv),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub fn git_custom_cancel(id: String) -> bool {
    gall_core::fs::gitcustom::cancel(&id)
}
#[tauri::command]
pub async fn git_worktree_location(
    path: PathBuf,
    template: String,
    templates: std::collections::BTreeMap<String, String>,
    branch: String,
) -> R<gall_core::fs::gitadvanced::WorktreeLocation> {
    tauri::async_runtime::spawn_blocking(move || {
        gall_core::fs::gitadvanced::worktree_location(&path, &template, &templates, &branch)
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_submodule_tree(path: PathBuf) -> R<gall_core::fs::gitadvanced::SubmoduleTree> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitadvanced::submodule_tree(&path))
        .await
        .map_err(e)?
        .map_err(e)
}
#[tauri::command]
pub async fn git_advanced_action(
    app: AppHandle,
    path: PathBuf,
    id: String,
    request: gall_core::fs::gitadvanced::Request,
    expected: String,
) -> R<String> {
    let (registration, cancel) = JobGuard::register(&id, "Git operation ID is already active")?;
    let job = id.clone();
    let (operation, phase, pipeline) = advanced_action_meta(&request);
    tauri::async_runtime::spawn_blocking(move || {
        let registration = registration;
        registration.started();
        run_with_git_progress(&app, Some(id), operation, phase, pipeline, true, || {
            gall_core::fs::gitadvanced::execute(&path, request, &expected, &cancel, |text| {
                let _ = app.emit(
                    "git-advanced-progress",
                    gall_core::fs::gitnetwork::Progress {
                        id: job.clone(),
                        text,
                    },
                );
            })
        })
    })
    .await
    .map_err(e)
    .and_then(|v| v.map_err(e))
}
#[tauri::command]
pub fn git_advanced_cancel(id: String) {
    git_network_cancel(id);
}
#[tauri::command]
pub async fn git_rebase_plan(
    path: PathBuf,
    base: String,
) -> R<gall_core::fs::gitrewrite::RewritePlan> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitrewrite::plan(&path, &base))
        .await
        .map_err(e)?
        .map_err(e)
}
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn git_rebase_conflicts(
    app: AppHandle,
    path: PathBuf,
    entries: Vec<gall_core::fs::gitrewrite::RewriteEntry>,
    base: String,
    window: tauri::WebviewWindow,
    ticket: u64,
    progress_id: Option<String>,
) -> R<Vec<gall_core::fs::gitrewrite::ConflictPrediction>> {
    // A newer check of the window stops this one.
    let label = format!("rebase:{}", window.label());
    git_search_latest(&label, ticket);
    tauri::async_runtime::spawn_blocking(move || {
        run_with_git_progress(
            &app,
            progress_id,
            "rebaseConflicts",
            "Checking rebase conflicts",
            gall_core::fs::gitprogress::Pipeline::Single,
            true,
            || {
                gall_core::fs::gitrewrite::predict_conflicts(&path, &base, &entries, &|| {
                    git_search_latest(&label, 0) != ticket
                })
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
/// Stops the conflict check of the window up to `ticket`.
#[tauri::command]
pub fn git_rebase_conflicts_cancel(window: tauri::WebviewWindow, ticket: u64) {
    git_search_latest(&format!("rebase:{}", window.label()), ticket + 1);
}
/// The messages of commits, asked for when an editor needs them.
#[tauri::command]
pub async fn git_commit_messages(
    path: PathBuf,
    ids: Vec<String>,
) -> R<Vec<gall_core::fs::gitrewrite::CommitMessage>> {
    tauri::async_runtime::spawn_blocking(move || gall_core::fs::gitrewrite::messages(&path, &ids))
        .await
        .map_err(e)?
        .map_err(e)
}
#[tauri::command]
pub async fn git_rebase_execute(
    app: AppHandle,
    path: PathBuf,
    entries: Vec<gall_core::fs::gitrewrite::RewriteEntry>,
    base: String,
    expected: String,
    progress_id: Option<String>,
) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        let executable =
            std::env::current_exe().map_err(|e| gall_core::CoreError::Other(e.to_string()))?;
        run_with_git_progress(
            &app,
            progress_id,
            "rebase",
            "Rebasing",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || gall_core::fs::gitrewrite::execute(&path, entries, &base, &expected, &executable),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
#[tauri::command]
pub async fn git_reword(
    app: AppHandle,
    path: PathBuf,
    target: String,
    message: String,
    expected: String,
    progress_id: Option<String>,
) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        let executable =
            std::env::current_exe().map_err(|e| gall_core::CoreError::Other(e.to_string()))?;
        run_with_git_progress(
            &app,
            progress_id,
            "reword",
            "Rewording commit",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || gall_core::fs::gitrewrite::reword(&path, &target, &message, &expected, &executable),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}

#[tauri::command]
pub async fn git_amend_staged(
    app: AppHandle,
    path: PathBuf,
    target: String,
    expected: String,
    progress_id: Option<String>,
) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        run_with_git_progress(
            &app,
            progress_id,
            "amend",
            "Amending commit",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || gall_core::fs::gitrewrite::amend_staged(&path, &target, &expected),
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}

#[tauri::command]
pub async fn git_commit_edit(
    app: AppHandle,
    path: PathBuf,
    target: String,
    action: gall_core::fs::gitrewrite::QuickAction,
    expected: String,
    progress_id: Option<String>,
) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = mutation_guard();
        let executable =
            std::env::current_exe().map_err(|e| gall_core::CoreError::Other(e.to_string()))?;
        run_with_git_progress(
            &app,
            progress_id,
            "rewrite",
            "Rewriting history",
            gall_core::fs::gitprogress::Pipeline::Single,
            false,
            || {
                gall_core::fs::gitrewrite::quick_edit(
                    &path,
                    &target,
                    action,
                    &expected,
                    &executable,
                )
            },
        )
    })
    .await
    .map_err(e)?
    .map_err(e)
}
