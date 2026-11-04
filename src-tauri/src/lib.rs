//! Gall desktop application (Tauri shell).

mod cli;
mod commands;
mod state;
#[cfg(windows)]
mod windrop;

use gall_core::fs::ops::{TaskManager, UndoJournal};
use gall_core::fs::watcher::FolderWatcher;
use gall_core::util::appdirs::AppDirs;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsChanged {
    path: String,
    storm: bool,
}

/// A backend warning or error shown to the user as a notification.
#[derive(Clone, Serialize)]
pub struct UiLogLine {
    level: &'static str,
    msg: String,
}

static APP: OnceLock<AppHandle> = OnceLock::new();
/// Set once the UI listens for "backend-log"; until then lines wait in `BACKLOG`.
static UI_READY: AtomicBool = AtomicBool::new(false);
static BACKLOG: Mutex<Vec<UiLogLine>> = Mutex::new(Vec::new());

/// Log target that sends our own warnings and errors to the UI, so nothing that went
/// wrong is visible only in the log file. The UI's own messages (target "ui") are
/// already shown, and per-entry listing warnings would be noise.
fn ui_log_target() -> tauri_plugin_log::fern::Dispatch {
    tauri_plugin_log::fern::Dispatch::new()
        .level(log::LevelFilter::Warn)
        .filter(|m| {
            m.target().starts_with("gall") && !m.target().starts_with("gall_core::fs::listing")
        })
        .chain(tauri_plugin_log::fern::Output::call(|record| {
            let level = if record.level() == log::Level::Error {
                "error"
            } else {
                "warn"
            };
            // The plugin has already formatted the line as "[date][time][target][LEVEL] msg".
            let full = record.args().to_string();
            let tag = format!("[{}] ", record.level());
            let msg = full
                .split_once(&tag)
                .map_or(full.as_str(), |(_, m)| m)
                .to_string();
            let line = UiLogLine { level, msg };
            match APP.get() {
                Some(app) if UI_READY.load(Ordering::Acquire) => {
                    let _ = app.emit("backend-log", line);
                }
                _ => BACKLOG.lock().unwrap().push(line),
            }
        }))
}

/// Called by the UI once it listens for "backend-log": returns what was logged before.
pub(crate) fn take_log_backlog() -> Vec<UiLogLine> {
    let mut b = BACKLOG.lock().unwrap();
    UI_READY.store(true, Ordering::Release);
    std::mem::take(&mut *b)
}

/// Milliseconds since the process started (Linux: from /proc, so it includes
/// loading the libraries before `main`; elsewhere since `run`).
pub fn startup_ms() -> u64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let since_run = T0
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64;
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
        let start = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(19))
            .and_then(|v| v.parse::<f64>().ok());
        let uptime = std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|u| u.split_whitespace().next()?.parse::<f64>().ok());
        if let (Some(start), Some(up)) = (start, uptime) {
            // starttime is in clock ticks; 100 per second on Linux.
            return ((up - start / 100.0) * 1000.0).max(0.0) as u64;
        }
    }
    since_run
}

pub fn run() {
    startup_ms();
    let args = cli::parse(std::env::args());
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let dirs = AppDirs::resolve(exe_dir.as_deref());

    use tauri_plugin_log::{Target, TargetKind};
    let mut log = tauri_plugin_log::Builder::new()
        .level(log::LevelFilter::Info)
        .max_file_size(2_000_000);
    if dirs.portable {
        // Portable: the log file goes to UserData/logs, not the user profile.
        log = log.clear_targets().targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::Folder {
                path: dirs.logs(),
                file_name: None,
            }),
        ]);
    }
    let log = log.target(Target::new(TargetKind::Dispatch(ui_log_target())));

    let mut builder = tauri::Builder::default();
    // Debug builds: GALL_MULTI_INSTANCE=1 runs beside an open Gall (tests with their own
    // portable data); otherwise a second launch hands over to the running one.
    let multi = cfg!(debug_assertions) && std::env::var_os("GALL_MULTI_INSTANCE").is_some();
    if !multi {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // A second launch forwards its paths to the running instance (X-01).
            let a = cli::parse(argv);
            let _ = app.emit("second-instance", a);
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }));
    }
    builder
        .plugin(log.build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(args)
        .setup(move |app| {
            log::info!("startup: setup at {} ms", startup_ms());
            let handle = app.handle().clone();
            let _ = APP.set(handle.clone());
            let (boot, _): (gall_core::storage::Settings, _) =
                gall_core::storage::JsonStore::new(dirs.settings()).load();
            // The main window is created here ("create": false in tauri.conf.json) so a
            // portable copy can keep the webview's data in UserData too. It starts hidden
            // (already maximized if wanted) and appears once the UI has painted
            // (`window_ready`), so it never shows growing or blank.
            if let Some(cfg) = app.config().app.windows.iter().find(|w| w.label == "main") {
                let mut wb = tauri::WebviewWindowBuilder::from_config(&handle, cfg)?
                    .visible(false)
                    .maximized(boot.start_maximized);
                if dirs.portable {
                    wb = wb.data_directory(dirs.webview());
                }
                wb.build()?;
                // Windows: files dropped from Explorer need the host to tell their paths.
                #[cfg(windows)]
                windrop::install(&handle);
                log::info!("startup: window created at {} ms", startup_ms());
                // Safety net: show the window even if the UI never reports ready.
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(4));
                    commands::reveal_main(&h);
                });
            }
            let h2 = handle.clone();
            let journal = Arc::new(UndoJournal::default());
            let tasks = Arc::new(TaskManager::new(
                journal,
                Arc::new(move |ev| {
                    let _ = h2.emit("task", ev);
                }),
            ));
            let h3 = handle.clone();
            let debounce = Duration::from_millis(boot.watcher_debounce_ms.max(50) as u64);
            let watcher = FolderWatcher::new(
                debounce,
                Arc::new(move |p, storm| {
                    let _ = h3.emit(
                        "fs-changed",
                        FsChanged {
                            path: p.to_string_lossy().into_owned(),
                            storm,
                        },
                    );
                }),
            );
            app.manage(state::AppState::load(dirs.clone(), tasks, watcher));
            // Colour tags and folder settings follow every move and rename, undo included.
            let h4 = handle.clone();
            gall_core::fs::ops::set_move_hook(move |from, to| {
                if let Some(st) = h4.try_state::<state::AppState>() {
                    let _ = st
                        .db
                        .move_tag(&from.to_string_lossy(), &to.to_string_lossy());
                }
            });
            // Watching pauses inside a folder that is renamed, moved or deleted. Windows
            // refuses those while a folder inside is watched (the other browser shows a
            // subfolder); on every OS the resume tells a browser left in a folder that went
            // away (Linux's inotify says nothing when a folder above the watched one moves).
            {
                let h5 = handle.clone();
                gall_core::fs::ops::set_release_hook(move |p| {
                    let Some(st) = h5.try_state::<state::AppState>() else {
                        return Box::new(|| {});
                    };
                    let released = st.watcher.release_under(p);
                    let h = h5.clone();
                    Box::new(move || {
                        if let Some(st) = h.try_state::<state::AppState>() {
                            st.watcher.resume(&released);
                        }
                    })
                });
            }
            log::info!("startup: state loaded at {} ms", startup_ms());
            // Build the Quick Open index shortly after start, then keep it fresh.
            let h4 = handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(2));
                loop {
                    let st = h4.state::<state::AppState>();
                    commands::index_rebuild_bg(&h4, &st, 30);
                    let mins = st.settings.read().unwrap().index_refresh_minutes;
                    if mins == 0 {
                        break;
                    }
                    std::thread::sleep(Duration::from_secs(mins as u64 * 60));
                }
            });
            log::info!(
                "Gall {} started; config at {}",
                env!("CARGO_PKG_VERSION"),
                dirs.config.display()
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::window_ready,
            commands::log_backlog,
            commands::get_settings,
            commands::set_settings,
            commands::reset_settings,
            commands::clear_prefs,
            commands::list_dir,
            commands::list_drives,
            commands::list_cloud_folders,
            commands::open_with_apps,
            commands::open_with_app,
            commands::open_with_dialog,
            commands::open_with_command,
            commands::system_stats,
            commands::diff_compare,
            commands::diff_patch,
            commands::diff_highlight,
            commands::diff_save_patch,
            commands::git_root,
            commands::dircmp_compare,
            commands::dircmp_cancel,
            commands::sync_apply,
            commands::save_text,
            commands::image_diff,
            commands::similarity_matrix,
            commands::table_compare,
            commands::eject,
            commands::other_volumes,
            commands::volume_mount,
            commands::volume_unlock,
            commands::drive_lock,
            commands::resolve_prefs,
            commands::set_prefs,
            commands::session_get,
            commands::session_cmd,
            commands::favorites_get,
            commands::favorites_cmd,
            commands::history_get,
            commands::history_remove,
            commands::preview_info,
            commands::video_probe,
            commands::excerpts,
            commands::watch,
            commands::unwatch,
            commands::start_task,
            commands::cancel_task,
            commands::find_conflicts,
            commands::find_existing,
            commands::conflict_info,
            commands::batch_rename,
            commands::git_status,
            commands::folder_size,
            commands::folder_sizes_cancel,
            commands::rename,
            commands::create_dir,
            commands::create_file,
            commands::duplicate,
            commands::trash,
            commands::symlink,
            commands::undo,
            commands::undo_peek,
            commands::undo_in,
            commands::undo_peek_in,
            commands::open_path,
            commands::archive_materialize,
            commands::archive_password,
            commands::reveal_path,
            commands::trash_list,
            commands::trash_restore,
            commands::trash_purge,
            commands::open_terminal,
            commands::open_terminal_with,
            commands::terminal_apps,
            commands::editor_apps,
            commands::open_with_run,
            commands::create_note,
            commands::toggle_note,
            commands::update_note,
            commands::templates_list,
            commands::template_add,
            commands::path_complete,
            commands::path_resolve,
            commands::path_format,
            commands::search_start,
            commands::search_cancel,
            commands::index_status,
            commands::index_refresh,
            commands::index_query,
            commands::grep_start,
            commands::grep_cancel,
            commands::grep_context,
            commands::tags_in,
            commands::set_tag,
            commands::tagged,
            commands::destinations,
            commands::clear_destinations,
            commands::clip_set_files,
            commands::clip_get_files,
            commands::clip_clear_files,
            commands::clip_set_text,
            commands::clip_inspect,
            commands::clip_paste_as_file,
            commands::clip_paste_files,
            commands::startup_args,
            commands::ui_log,
            commands::modifiers,
            commands::font_families,
            commands::start_native_drag,
            commands::hex_diff,
            commands::hex_diff_cancel,
            commands::read_bytes,
            commands::selftest_shot,
            commands::selftest_done,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Gall");
}
