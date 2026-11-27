//! Gall desktop application (Tauri shell).

mod bug_report;
mod cli;
mod commands;
mod compare_wait;
mod filemanager_dbus;
mod headless;
#[cfg(any(target_os = "linux", test))]
mod linux_clipboard;
#[cfg(target_os = "macos")]
mod macos_clipboard;
#[cfg(target_os = "linux")]
mod media_server;
mod omarchy;
mod pick;
mod report_capture;
mod state;
mod updates;
#[cfg(windows)]
mod winapp;
#[cfg(gall_wayland_presentation)]
mod window_presentation;
#[cfg(windows)]
mod windows_clipboard;
#[cfg(windows)]
mod windrop;

use gall_core::fs::ops::{TaskManager, UndoJournal};
use gall_core::fs::watcher::FolderWatcher;
use gall_core::handoff::Outcome;
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
/// URL prefix of the loopback media server (Linux only, see `media_server`).
pub(crate) static MEDIA_BASE: OnceLock<String> = OnceLock::new();
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
            bug_report::record(level, &msg);
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

/// How often the index refresh thread looks at the interval setting while it waits.
const INDEX_REFRESH_POLL: Duration = Duration::from_secs(20);

/// Is a periodic rebuild of the Quick Open index due, `waited` after the last one?
/// An interval of 0 minutes means "only on demand".
fn index_refresh_due(minutes: u32, waited: Duration) -> bool {
    minutes != 0 && waited >= Duration::from_secs(u64::from(minutes) * 60)
}

/// Early bootstrapper step run at the start of `main()` before initializing Tauri or
/// single-instance locking: handles `--gall-update-probe`, Git editor helper mode,
/// `--gall-wait-pid` synchronization, `.gall-old` cleanup, and signed overlay handoff.
pub fn pre_run(raw_args: &[std::ffi::OsString]) -> Option<i32> {
    if let Some(code) = gall_core::doc_compare::handle_pdf_worker_arg(raw_args) {
        return Some(code);
    }
    let text: Vec<String> = raw_args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    // Window-less subcommands (search, grep, selection, skill) never start the app.
    if headless::is_headless(&text) {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
        let dirs = AppDirs::resolve(exe_dir.as_deref());
        if let Some(code) = headless::run(&text, &dirs) {
            return Some(code);
        }
    }
    if let Some(msg) = cli::usage_error(&text) {
        eprintln!("{msg}");
        return Some(gall_core::handoff::EXIT_ERROR);
    }
    // On Windows the updater spawns and detaches the overlay process. Keep the
    // process that Git started alive and let only its GUI child do that handoff.
    #[cfg(windows)]
    if let Some(code) = compare_wait::run_caller(raw_args, &text) {
        return Some(code);
    }
    // Probes must report the binary's compiled-in version, ignoring debug test overrides.
    if let Some(code) =
        gall_core::updater::bootstrap::handle_probe_arg(raw_args, env!("CARGO_PKG_VERSION"))
    {
        return Some(code);
    }
    let ver = updates::running_version();
    if let Some(code) = gall_core::fs::gitrewrite::handle_editor_args(raw_args) {
        return Some(code);
    }
    let current_exe = std::env::current_exe().ok();
    let exe_dir = current_exe
        .as_deref()
        .and_then(|p| p.parent())
        .map(std::path::Path::to_path_buf);
    let dirs = AppDirs::resolve(exe_dir.as_deref());
    let appimage = std::env::var_os("APPIMAGE")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from);
    gall_core::updater::bootstrap::handle_wait_and_cleanup(
        raw_args,
        current_exe.as_deref(),
        appimage.as_deref(),
    );
    gall_core::updater::bootstrap::maybe_handoff_to_overlay(
        &updates::effective_pubkey(),
        &ver,
        &dirs,
        current_exe.as_deref(),
        appimage.as_deref(),
        raw_args,
    );
    if cfg!(windows) {
        None
    } else {
        compare_wait::run_caller(raw_args, &text)
    }
}

fn allocate_window_for(
    app: &AppHandle,
    label_override: Option<&str>,
    args: cli::CliArgs,
) -> String {
    let state = app.state::<state::AppState>();
    // Pickers and waiting comparisons keep an independent session even on a cold start.
    let waiting = args.handoff.is_some();
    let label = if args.pick.is_some() {
        state.allocate_pick_label()
    } else if waiting {
        state.allocate_window_label()
    } else {
        state.choose_window_label(
            label_override,
            |l| app.get_webview_window(l).is_some(),
            || {
                app.webview_windows()
                    .keys()
                    .any(|l| state.compare_wait_window(l).is_none())
            },
        )
    };
    state.begin_window(&label);
    state.ensure_session(&label, Some(&args));
    state.set_window_initial_cli(&label, args);
    if !waiting {
        state.set_active_window(&label);
    }
    label
}

fn finish_interrupted_compare(wait: &state::CompareWaitWindow, ready: bool) {
    if wait.withdraw() {
        return;
    }
    let written = wait.finish(|| {
        if ready || wait.output.is_some() {
            Outcome::Cancelled
        } else {
            Outcome::Error {
                message: "The comparison window closed before it was ready".into(),
            }
        }
    });
    if let Err(err) = written {
        log::warn!("could not report closed waiting comparison: {err}");
    }
}

fn report_compare_launch_failure(wait: &state::CompareWaitWindow, message: String) {
    if !wait.withdraw() {
        if let Err(err) = wait.finish(|| Outcome::Error { message }) {
            log::warn!("could not report waiting comparison launch failure: {err}");
        }
    }
}

#[cfg(gall_wayland_presentation)]
const STARTUP_STAGE_TITLE: &str = "Gall (preparing window)";

fn parse_hex_rgb(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.trim().strip_prefix('#')?;
    match hex.len() {
        3 => {
            let expand = |i| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|n| n * 17);
            Some((expand(0)?, expand(1)?, expand(2)?))
        }
        6 | 8 => Some((
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        )),
        _ => None,
    }
}

/// Initial opaque background color for a non-transparent window before the UI paints and syncs
/// the exact computed CSS `--bg` via `set_window_background_color`.
fn initial_window_background(
    settings: &gall_core::storage::Settings,
    omarchy: &omarchy::OmarchyState,
) -> tauri::window::Color {
    use gall_core::storage::settings::ThemeMode;
    const DARK_BG: (u8, u8, u8) = (0x0d, 0x0f, 0x13);
    const LIGHT_BG: (u8, u8, u8) = (0xec, 0xee, 0xf2);

    let (r, g, b) = if settings.theme == ThemeMode::Omarchy && omarchy.active {
        omarchy
            .palette_tokens
            .get("background")
            .and_then(|v| parse_hex_rgb(v))
            .unwrap_or(if omarchy.theme_mode == "light" {
                LIGHT_BG
            } else {
                DARK_BG
            })
    } else {
        let light = settings.theme == ThemeMode::Light;
        match settings.custom_theme.trim().to_ascii_lowercase().as_str() {
            "gruvbox" => {
                if light {
                    (0xf2, 0xe5, 0xbc)
                } else {
                    (0x1d, 0x20, 0x21)
                }
            }
            "nightowl" => {
                if light {
                    (0xe4, 0xe6, 0xea)
                } else {
                    (0x01, 0x12, 0x20)
                }
            }
            _ => {
                if light {
                    LIGHT_BG
                } else {
                    DARK_BG
                }
            }
        }
    };
    tauri::window::Color(r, g, b, 255)
}

fn finish_open_window(app: &AppHandle, label: &str) -> Result<String, String> {
    let state = app.state::<state::AppState>();
    let boot = state.settings.read().unwrap().clone();
    let Some(cfg) = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
    else {
        state.remove_window(label);
        return Err("missing main window config".to_string());
    };
    let mut win_cfg = cfg;
    win_cfg.label = label.to_string();
    #[cfg(gall_wayland_presentation)]
    if omarchy::prepare_window_reveal() {
        win_cfg.title = STARTUP_STAGE_TITLE.into();
    }

    let maximize = omarchy::start_maximized(boot.start_maximized, omarchy::is_tiling_wm());
    let omarchy_state = state.omarchy.current();
    let transparent = cfg!(target_os = "linux") && omarchy_state.active && boot.omarchy_glassy;
    let mut wb = match tauri::WebviewWindowBuilder::from_config(app, &win_cfg) {
        Ok(b) => {
            let b = b
                .visible(false)
                .maximized(maximize)
                .transparent(transparent);
            if transparent {
                b
            } else {
                b.background_color(initial_window_background(&boot, &omarchy_state))
            }
        }
        Err(err) => {
            state.remove_window(label);
            return Err(err.to_string());
        }
    };
    if state.dirs.portable {
        wb = wb.data_directory(state.dirs.webview());
    }
    let win = match wb.build() {
        Ok(w) => w,
        Err(err) => {
            state.remove_window(label);
            return Err(err.to_string());
        }
    };
    state.set_window_transparent(label, transparent);
    #[cfg(windows)]
    {
        windrop::install_for_window(app, &win);
        winapp::configure_taskbar_identity_for_window(&win);
    }

    let app_ev = app.clone();
    let label_ev = label.to_string();
    let generation = state.window_generation(label).unwrap_or(0);
    win.on_window_event(move |ev| match ev {
        tauri::WindowEvent::Focused(true) => {
            if let Some(st) = app_ev.try_state::<state::AppState>() {
                if st.compare_wait_window(&label_ev).is_none() {
                    st.set_active_window(&label_ev);
                }
            }
        }
        tauri::WindowEvent::CloseRequested { api, .. } => {
            if let Some(st) = app_ev.try_state::<state::AppState>() {
                if st.window_generation(&label_ev) == Some(generation)
                    && st.compare_wait_window(&label_ev).is_some()
                    && st.window_is_ready(&label_ev)
                    && app_ev.emit_to(&label_ev, "compare-wait-close", ()).is_ok()
                {
                    // The webview handles the same dirty-merge confirmation as
                    // its own close button and then reports the final outcome.
                    api.prevent_close();
                }
            }
        }
        tauri::WindowEvent::Destroyed => {
            if let Some(st) = app_ev.try_state::<state::AppState>() {
                // A window opened under this label since then owns the state: leave it alone.
                if st.window_generation(&label_ev) != Some(generation) {
                    return;
                }
                // A picker closed without an answer (window closed, killed by the compositor) is a cancel.
                if let Some(pick) = st.window_pick(&label_ev) {
                    if st.mark_pick_done(&label_ev) {
                        let _ = pick::write_result(
                            &pick.result,
                            &pick::PickResult {
                                status: 1,
                                paths: &[],
                                filter: None,
                                choices: &[],
                            },
                        );
                    }
                }
                if let Some(wait) = st.compare_wait_window(&label_ev) {
                    finish_interrupted_compare(&wait, st.window_is_ready(&label_ev));
                }
                st.remove_window_generation(&label_ev, generation);
            }
        }
        _ => {}
    });

    // A picker window closes itself when the caller gives up (the portal drops `<result>.cancel`).
    if let Some(pick) = state.window_pick(label) {
        let (h, lbl) = (app.clone(), label.to_string());
        std::thread::spawn(move || {
            let marker = pick::cancel_marker(&pick.result);
            loop {
                std::thread::sleep(Duration::from_millis(300));
                if h.get_webview_window(&lbl).is_none() {
                    break;
                }
                if marker.exists() {
                    let _ = std::fs::remove_file(&marker);
                    cancel_pick_window(&h, &pick.result);
                    break;
                }
            }
        });
    }
    if let Some(wait) = state.compare_wait_window(label) {
        let window = win.clone();
        let handle = app.clone();
        let label = label.to_string();
        std::thread::spawn(move || loop {
            if wait.withdraw() {
                let _ = window.destroy();
                break;
            }
            if handle.get_webview_window(&label).is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        });
    }

    log::info!("startup: window '{label}' created at {} ms", startup_ms());
    let h = app.clone();
    let reveal_lbl = label.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        commands::reveal_window(&h, &reveal_lbl);
    });
    Ok(label.to_string())
}

fn open_window_locked(
    app: &AppHandle,
    label_override: Option<&str>,
    args: cli::CliArgs,
) -> Result<String, String> {
    let label = allocate_window_for(app, label_override, args);
    finish_open_window(app, &label)
}

/// Creates and tracks a `WebviewWindow` (`"main"` or `"win-<N>"`), hidden until `window_ready`
/// (with a 4-second per-window fallback reveal).
pub(crate) fn open_window(
    app: &AppHandle,
    label_override: Option<&str>,
    args: cli::CliArgs,
) -> Result<String, String> {
    let state = app
        .try_state::<state::AppState>()
        .ok_or_else(|| "application state not initialized".to_string())?;
    let _open_guard = state.lock_window_open();
    open_window_locked(app, label_override, args)
}

enum CliRouteOutcome {
    Opened,
    Existing {
        target: String,
        is_ready: bool,
        args: Box<cli::CliArgs>,
    },
}

/// Closes the picker window whose answer goes to `result_file` (as a cancel). True when one was found.
pub(crate) fn cancel_pick_window(app: &AppHandle, result_file: &str) -> bool {
    let Some(state) = app.try_state::<state::AppState>() else {
        return false;
    };
    for (label, win) in app.webview_windows() {
        let Some(pick) = state.window_pick(&label) else {
            continue;
        };
        if pick.result != result_file {
            continue;
        }
        if state.mark_pick_done(&label) {
            let _ = pick::write_result(
                &pick.result,
                &pick::PickResult {
                    status: 1,
                    paths: &[],
                    filter: None,
                    choices: &[],
                },
            );
        }
        let _ = win.close();
        return true;
    }
    false
}

/// Atomically routes incoming CLI arguments (`single-instance` or macOS `RunEvent::Opened`)
/// to an existing window or opens a new window under `window_open_lock`.
pub(crate) fn route_cli_to_window(
    app: &AppHandle,
    args: cli::CliArgs,
    focus_existing: bool,
) -> Result<(), String> {
    let outcome = {
        let state = app
            .try_state::<state::AppState>()
            .ok_or_else(|| "application state not initialized".to_string())?;
        let _open_guard = state.lock_window_open();
        if args.new_window || args.handoff.is_some() {
            open_window_locked(app, None, args)?;
            CliRouteOutcome::Opened
        } else {
            match state.resolve_target_window(app) {
                Some(target) => {
                    let is_ready = state.dispatch_cli_or_enqueue(&target, args.clone());
                    CliRouteOutcome::Existing {
                        target,
                        is_ready,
                        args: Box::new(args),
                    }
                }
                None => {
                    open_window_locked(app, None, args)?;
                    CliRouteOutcome::Opened
                }
            }
        }
    };
    if let CliRouteOutcome::Existing {
        target,
        is_ready,
        args,
    } = outcome
    {
        if focus_existing {
            if let Some(w) = app.get_webview_window(&target) {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
        if is_ready {
            let _ = app.emit_to(&target, "second-instance", &*args);
            let _ = app.emit_to(&target, "cli-args", &*args);
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
enum FmRouteOutcome {
    Opened,
    Existing {
        target: String,
        is_ready: bool,
        cli_args: Box<cli::CliArgs>,
        action: Box<filemanager_dbus::FileManagerAction>,
    },
}

/// Atomically routes a Linux `org.freedesktop.FileManager1` (or internal `reveal_path`) action
/// to an existing window or opens a new window with seeded pending action under `window_open_lock`.
#[cfg(target_os = "linux")]
pub(crate) fn route_fm_to_window(
    app: &AppHandle,
    target_override: Option<&str>,
    cli_args: cli::CliArgs,
    action: filemanager_dbus::FileManagerAction,
) -> Result<(), String> {
    let outcome = {
        let state = app
            .try_state::<state::AppState>()
            .ok_or_else(|| "application state not initialized".to_string())?;
        let _open_guard = state.lock_window_open();
        let target_label = target_override
            .filter(|lbl| app.get_webview_window(lbl).is_some())
            .map(str::to_string)
            .or_else(|| state.resolve_target_window(app));
        match target_label {
            Some(label) => {
                let is_ready =
                    state.dispatch_fm_or_enqueue(&label, cli_args.clone(), action.clone());
                FmRouteOutcome::Existing {
                    target: label,
                    is_ready,
                    cli_args: Box::new(cli_args),
                    action: Box::new(action),
                }
            }
            None => {
                let label = allocate_window_for(app, None, cli_args);
                state.seed_pending_fm(&label, action);
                finish_open_window(app, &label)?;
                FmRouteOutcome::Opened
            }
        }
    };
    if let FmRouteOutcome::Existing {
        target,
        is_ready,
        cli_args,
        action,
    } = outcome
    {
        if let Some(win) = app.get_webview_window(&target) {
            let _ = win.unminimize();
            let _ = win.show();
            let _ = win.set_focus();
        }
        if is_ready {
            let _ = app.emit_to(&target, "cli-args", &*cli_args);
            let _ = app.emit_to(&target, "second-instance", &*cli_args);
            let _ = app.emit_to(&target, "filemanager-action", &*action);
        }
    }
    Ok(())
}

pub fn run() {
    startup_ms();
    #[cfg(target_os = "macos")]
    gall_core::util::macpath::fix_path();
    let args = cli::parse(std::env::args());
    let startup_args = args.clone();
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
    // A self-test run (GALL_SELFTEST=<mode>) is never handed over either: it must run its own
    // checks even while another Gall is open (its report would otherwise never be written).
    let selftest_run = std::env::var("GALL_SELFTEST").is_ok_and(|v| !matches!(v.trim(), "" | "0"));
    let multi = cfg!(debug_assertions)
        && (std::env::var_os("GALL_MULTI_INSTANCE").is_some() || selftest_run);
    if !multi {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            // A second launch forwards its paths to the running instance (X-01); relative
            // ones belong to the folder it was started in, not to ours.
            let a = cli::parse_in(argv, Some(std::path::Path::new(&cwd)));
            if (a.no_window || a.daemon) && !a.has_launch_action() && !a.new_window {
                return;
            }
            let pick = a.pick.clone();
            let wait = state::CompareWaitWindow::from_args(&a);
            if let Err(err) = route_cli_to_window(app, a, true) {
                log::warn!("single-instance route failed: {err}");
                if let Some(wait) = wait {
                    report_compare_launch_failure(&wait, err);
                }
                // Nobody else will answer a picker that could not open: the caller must not wait for ever.
                if let Some(p) = pick {
                    let _ = pick::write_result(
                        &p.result,
                        &pick::PickResult {
                            status: 2,
                            paths: &[],
                            filter: None,
                            choices: &[],
                        },
                    );
                }
            }
        }));
    }
    builder
        .plugin(log.build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey(updates::effective_pubkey())
                .build(),
        )
        .manage(args)
        .setup(move |app| {
            bug_report::init(&dirs);
            log::info!("startup: setup at {} ms", startup_ms());
            #[cfg(gall_wayland_presentation)]
            window_presentation::discover_on_main_thread();
            // Video and audio previews play from a loopback HTTP server: WebKitGTK cannot
            // play media from the asset:// scheme.
            #[cfg(target_os = "linux")]
            match media_server::start() {
                Ok(base) => {
                    let _ = MEDIA_BASE.set(base);
                }
                Err(e) => log::warn!("media server not started: {e}"),
            }
            let handle = app.handle().clone();
            let _ = APP.set(handle.clone());
            let (boot, settings_source) = gall_core::storage::Settings::load(
                &gall_core::storage::JsonStore::new(dirs.settings()),
            );
            let omarchy = omarchy::OmarchyBridge::start(handle.clone());
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
                    // A .gallignore created, edited or deleted here: Quick Open follows it.
                    if gall_core::fs::ignorefile::gallignore_changed_in(p) {
                        if let Some(st) = h3.try_state::<state::AppState>() {
                            commands::index_rebuild_bg(&h3, &st, 0);
                        }
                    }
                    let _ = h3.emit(
                        "fs-changed",
                        FsChanged {
                            path: p.to_string_lossy().into_owned(),
                            storm,
                        },
                    );
                }),
            );
            let app_state =
                state::AppState::load(dirs.clone(), tasks, watcher, boot, settings_source, omarchy);
            app_state
                .daemon_mode
                .store(startup_args.daemon, Ordering::Release);
            // The initial window is created here ("create": false in tauri.conf.json) unless
            // started headless via `--nowindow` / `--daemon`. It starts hidden and appears once
            // the UI has painted (`window_ready`), so it never shows growing or blank.
            // Acquire `window_open_lock` before `app.manage(app_state)` so a concurrent
            // `single_instance` callback cannot take the lock between `manage` and
            // `open_window_locked` and deadlock waiting on the not-yet-running event loop.
            // Also create the initial `"main"` window before registering
            // `org.freedesktop.FileManager1` so D-Bus calls cannot race initial startup.
            if startup_args.handoff.is_some() || !(startup_args.no_window || startup_args.daemon) {
                let open_lock = app_state.window_open_lock.clone();
                let startup_open_guard = open_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                app.manage(app_state);
                let wait = state::CompareWaitWindow::from_args(&startup_args);
                if let Err(err) = open_window_locked(&handle, Some("main"), startup_args) {
                    if let Some(wait) = wait {
                        report_compare_launch_failure(&wait, err.clone());
                    }
                    return Err(err.into());
                }
                drop(startup_open_guard);
            } else {
                app.manage(app_state);
                log::info!(
                    "startup: headless/daemon mode active (daemon={})",
                    startup_args.daemon
                );
            }
            filemanager_dbus::start(handle.clone());
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
            #[cfg(target_os = "macos")]
            gall_core::fs::mactrash::set_index_file(
                handle
                    .state::<state::AppState>()
                    .dirs
                    .data
                    .join("macos-trash.json"),
            );
            // Build the Quick Open index shortly after start (unless set to 0 = "only on demand"),
            // then keep it fresh at the configured interval.
            let h4 = handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(2));
                loop {
                    let st = h4.state::<state::AppState>();
                    if st.settings.read().unwrap().index_refresh_minutes > 0 {
                        commands::index_rebuild_bg(&h4, &st, 30);
                    }
                    // The interval is read again while waiting: 0 ("only on demand")
                    // pauses the refresh instead of ending it, and a new or shorter
                    // interval applies without a restart.
                    let since = std::time::Instant::now();
                    loop {
                        std::thread::sleep(INDEX_REFRESH_POLL);
                        let mins = st.settings.read().unwrap().index_refresh_minutes;
                        if index_refresh_due(mins, since.elapsed()) {
                            break;
                        }
                    }
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
            updates::update_capability,
            updates::update_support,
            updates::update_ready,
            updates::check_update,
            updates::download_update,
            updates::apply_update,
            updates::install_update,
            updates::update_startup_notice,
            updates::revert_overlay_update,
            commands::app_info,
            commands::window_ready,
            commands::window_frame_ready,
            commands::window_minimize,
            commands::window_is_maximized,
            commands::window_toggle_maximize,
            commands::set_window_background_color,
            commands::log_backlog,
            commands::get_settings,
            commands::set_settings,
            commands::reset_settings,
            commands::clear_prefs,
            commands::list_dir,
            commands::list_drives,
            commands::list_cloud_folders,
            commands::open_with_apps,
            commands::open_with_all_apps,
            commands::open_with_set_default,
            commands::open_with_app,
            commands::open_with_dialog,
            commands::open_with_command,
            commands::system_stats,
            commands::diff_compare,
            commands::diff_page_open,
            commands::diff_page_range,
            commands::diff_page_close,
            commands::git_commits,
            commands::git_graph,
            commands::git_graph_view,
            commands::git_scoped_history,
            commands::git_switch,
            commands::git_workspace,
            commands::git_index_action,
            commands::git_file_patch,
            commands::git_checkout_commit_file,
            commands::git_commit_details,
            commands::git_commit_diff,
            commands::git_commit_patch,
            commands::git_commit_patch_text,
            commands::git_file_history,
            commands::git_worktree_comparison,
            commands::git_line_history,
            commands::git_line_history_cancel,
            commands::git_blame_lines,
            commands::git_blame_cancel,
            commands::git_forecast,
            commands::git_image,
            commands::git_absorb_propose,
            commands::git_absorb_cancel,
            commands::git_absorb_apply,
            commands::git_forecast_cancel,
            commands::merge_three_way,
            commands::merge_resolve,
            commands::merge_preview,
            commands::merge_cancel,
            commands::patch_sniff,
            commands::patch_preview,
            commands::patch_apply,
            commands::doc_extract,
            commands::doc_extract_cancel,
            commands::git_commit_content,
            commands::git_conflict,
            commands::git_repository,
            commands::git_repository_action,
            commands::git_patch_selection,
            commands::git_apply_selection,
            commands::git_discard_selection,
            commands::git_stash_list,
            commands::git_stash_preview,
            commands::git_stash_selection,
            commands::git_worktree_transfer,
            commands::git_network,
            commands::git_network_cancel,
            commands::git_remote_tags,
            commands::git_remote_tags_register,
            commands::git_remote_tags_release,
            commands::git_advanced,
            commands::git_submodule_tree,
            commands::git_advanced_action,
            commands::git_search,
            commands::git_journal,
            commands::git_reflog,
            commands::git_journal_move,
            commands::git_search_cancel,
            commands::git_wip_list,
            commands::git_wip_discard,
            commands::git_wip_stash,
            commands::git_custom_run,
            commands::git_custom_cancel,
            commands::git_worktree_location,
            commands::git_worktree_places,
            commands::git_worktree_state,
            commands::git_worktree_cleanup,
            commands::git_advanced_cancel,
            commands::git_rebase_plan,
            commands::git_rebase_conflicts,
            commands::git_rebase_conflicts_cancel,
            commands::git_commit_messages,
            commands::git_rebase_execute,
            commands::git_reword,
            commands::git_amend_staged,
            commands::git_commit_edit,
            commands::diff_patch,
            commands::diff_patch_marks,
            commands::diff_highlight,
            commands::diff_save_patch,
            commands::compare_sessions_load,
            commands::compare_sessions_save,
            commands::compare_sessions_relink,
            commands::compare_report_html,
            commands::compare_report_pdf,
            commands::compare_report_cancel,
            commands::semantic_compare,
            commands::semantic_compare_cancel,
            commands::semantic_outline,
            commands::compare_ai_explain,
            commands::compare_ai_config_get,
            commands::compare_ai_config_save,
            commands::compare_ai_cancel,
            commands::merge_ai_revision,
            commands::merge_ai_propose,
            commands::generated_classify,
            commands::git_root,
            commands::dircmp_compare,
            commands::dircmp_cancel,
            commands::sync_apply,
            commands::sync_tree_fingerprints,
            commands::sync_file_fingerprints,
            commands::save_text,
            commands::save_text_with_undo,
            commands::read_text_file,
            commands::text_file_version,
            commands::image_diff,
            commands::similarity_matrix,
            commands::table_compare,
            commands::eject,
            commands::other_volumes,
            commands::volume_mount,
            commands::volume_power_off,
            commands::volume_repair_mount,
            commands::volume_unlock,
            commands::drive_lock,
            commands::resolve_prefs,
            commands::set_prefs,
            commands::session_get,
            commands::session_cmd,
            commands::workspaces_get,
            commands::workspace_cmd,
            commands::gravity_suggest,
            commands::gravity_record,
            commands::gravity_dismiss,
            commands::favorites_get,
            commands::favorites_cmd,
            commands::favorites_import_detect,
            commands::favorites_import_read,
            commands::favorites_import_apply,
            commands::favorites_import_undo,
            commands::marks_get,
            commands::marks_cmd,
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
            commands::git_untrusted,
            commands::git_trust,
            commands::folder_size,
            commands::folder_sizes_cancel,
            commands::rename,
            commands::batch_rename_with_undo,
            commands::create_dir,
            commands::ensure_dir,
            commands::create_dir_with_undo,
            commands::create_file,
            commands::create_file_with_undo,
            commands::duplicate,
            commands::trash,
            commands::symlink,
            commands::undo,
            commands::undo_by_id,
            commands::undo_peek,
            commands::undo_in,
            commands::undo_peek_in,
            commands::shortcut_resolve,
            commands::open_path,
            commands::open_paths,
            commands::open_elevated,
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
            commands::hide_from_search,
            commands::show_in_search,
            commands::search_hidden,
            commands::index_query,
            commands::index_hidden_count,
            commands::index_hidden_cancel,
            commands::index_covers,
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
            commands::macos_drag_paths,
            commands::open_full_disk_access_settings,
            commands::startup_args,
            commands::cli_initial_args,
            commands::cli_take_pending,
            commands::filemanager_take_pending,
            commands::new_window,
            commands::gvfs_network_mounts,
            commands::connect_server,
            commands::gvfs_unmount,
            commands::actions_list,
            commands::actions_for_selection,
            commands::action_run,
            commands::zoxide_dirs,
            commands::omarchy_terminal_cwd,
            commands::omarchy_state_get,
            commands::file_manager_status,
            commands::default_file_manager_status,
            commands::default_file_manager_set,
            commands::default_file_manager_restore,
            commands::ui_log,
            commands::modifiers,
            commands::font_families,
            commands::start_native_drag,
            commands::hex_diff,
            commands::hex_diff_cancel,
            commands::read_bytes,
            bug_report::bug_report_prepare,
            bug_report::bug_report_record,
            bug_report::bug_report_record_many,
            bug_report::bug_report_status,
            bug_report::bug_report_dismiss,
            bug_report::bug_report_export,
            bug_report::bug_report_copy_screenshot,
            bug_report::bug_report_copy_text,
            report_capture::bug_report_capture,
            report_capture::bug_report_save_media,
            commands::selftest_shot,
            commands::selftest_resize,
            commands::selftest_frame_trace,
            commands::selftest_done,
            commands::selftest_simulate_uwsm,
            commands::selftest_clip_dump,
            commands::selection_publish,
            commands::pick_finish,
            commands::pick_cancel,
            commands::compare_wait_finish,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Gall")
        .run(|app, event| {
            match event {
                tauri::RunEvent::ExitRequested {
                    code: None, api, ..
                } => {
                    if let Some(state) = app.try_state::<state::AppState>() {
                        if state.daemon_mode.load(Ordering::Acquire) {
                            api.prevent_exit();
                        }
                    }
                }
                tauri::RunEvent::Exit => {
                    // The session is written in the background; make sure the last
                    // state is on disk.
                    if let Some(state) = app.try_state::<state::AppState>() {
                        for (label, wait) in state.compare_wait_windows() {
                            finish_interrupted_compare(&wait, state.window_is_ready(&label));
                        }
                        state.flush_session();
                        state.db.checkpoint();
                        // Only our own selection file: another instance may have written it.
                        if headless::read_selection(&state.dirs)
                            .is_ok_and(|s| s.pid == std::process::id())
                        {
                            headless::remove_selection(&state.dirs);
                        }
                    }
                    bug_report::clean()
                }
                // macOS delivers "Open With Gall", a folder dropped on the Dock icon and
                // `open -a Gall <path>` as an event, not as command-line arguments.
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Opened { urls } => {
                    let mut paths = Vec::new();
                    let mut select_paths = Vec::new();
                    for u in urls {
                        if let Ok(p) = u.to_file_path() {
                            if p.is_dir() {
                                let s = p.to_string_lossy().into_owned();
                                if !paths.contains(&s) {
                                    paths.push(s);
                                }
                            } else {
                                let item = p.to_string_lossy().into_owned();
                                if !select_paths.contains(&item) {
                                    select_paths.push(item);
                                }
                                let parent = p
                                    .parent()
                                    .map(|d| d.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| p.to_string_lossy().into_owned());
                                if !paths.contains(&parent) {
                                    paths.push(parent);
                                }
                            }
                        }
                    }
                    if !paths.is_empty() || !select_paths.is_empty() {
                        let args = cli::CliArgs {
                            path1: paths.first().cloned(),
                            paths,
                            select_paths,
                            ..Default::default()
                        };
                        let handle = app.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            // On a cold launch via Finder/Dock/`open -a`, AppKit can deliver
                            // `RunEvent::Opened` before `.setup()` calls `app.manage(AppState)`.
                            let deadline = std::time::Instant::now() + Duration::from_secs(5);
                            while handle.try_state::<state::AppState>().is_none()
                                && std::time::Instant::now() < deadline
                            {
                                std::thread::sleep(Duration::from_millis(25));
                            }
                            if let Err(err) = route_cli_to_window(&handle, args, false) {
                                log::warn!("macos Opened route failed: {err}");
                            }
                        });
                    }
                }
                _ => {}
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_refresh_follows_the_interval_and_pauses_at_zero() {
        let min = |m: u64| Duration::from_secs(m * 60);
        assert!(!index_refresh_due(15, min(14)));
        assert!(index_refresh_due(15, min(15)));
        // "Only on demand" never becomes due, however long the wait.
        assert!(!index_refresh_due(0, min(100_000)));
        // Back to 15 minutes after a long pause: due at once.
        assert!(index_refresh_due(15, min(100_000)));
        // A shortened interval counts from the last rebuild, not from a new sleep.
        assert!(index_refresh_due(5, min(6)));
    }

    #[test]
    fn parse_hex_rgb_supports_short_long_and_alpha_forms() {
        assert_eq!(parse_hex_rgb("#0d0f13"), Some((0x0d, 0x0f, 0x13)));
        assert_eq!(parse_hex_rgb(" #1D2021ff "), Some((0x1d, 0x20, 0x21)));
        assert_eq!(parse_hex_rgb("#abc"), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_hex_rgb("123456"), None);
        assert_eq!(parse_hex_rgb("#zzzzzz"), None);
    }

    #[test]
    fn initial_window_background_matches_theme_and_omarchy_palette() {
        use gall_core::storage::settings::ThemeMode;
        use gall_core::storage::Settings;

        let mut settings = Settings::default();
        let mut omarchy = omarchy::OmarchyState::default();

        settings.theme = ThemeMode::Dark;
        assert_eq!(
            initial_window_background(&settings, &omarchy),
            tauri::window::Color(0x0d, 0x0f, 0x13, 255)
        );

        settings.theme = ThemeMode::Light;
        assert_eq!(
            initial_window_background(&settings, &omarchy),
            tauri::window::Color(0xec, 0xee, 0xf2, 255)
        );

        settings.custom_theme = "gruvbox".into();
        assert_eq!(
            initial_window_background(&settings, &omarchy),
            tauri::window::Color(0xf2, 0xe5, 0xbc, 255)
        );
        settings.theme = ThemeMode::Dark;
        assert_eq!(
            initial_window_background(&settings, &omarchy),
            tauri::window::Color(0x1d, 0x20, 0x21, 255)
        );

        settings.custom_theme = "nightowl".into();
        assert_eq!(
            initial_window_background(&settings, &omarchy),
            tauri::window::Color(0x01, 0x12, 0x20, 255)
        );

        settings.theme = ThemeMode::Omarchy;
        omarchy.active = true;
        omarchy
            .palette_tokens
            .insert("background".into(), "#1e222a".into());
        assert_eq!(
            initial_window_background(&settings, &omarchy),
            tauri::window::Color(0x1e, 0x22, 0x2a, 255)
        );
    }
}
