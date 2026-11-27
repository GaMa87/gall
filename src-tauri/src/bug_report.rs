//! Local report preparation; publication remains an explicit browser action.
use gall_core::diagnostics::{sanitize, Diagnostics};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use tauri::Manager;
static DIAGNOSTICS: OnceLock<Mutex<Diagnostics>> = OnceLock::new();

/// How Gall was installed, as far as it is known without asking anything: None when a
/// package manager has to be asked (`packaged`).
fn installation(dirs: &gall_core::util::appdirs::AppDirs) -> Option<&'static str> {
    if dirs.portable {
        return Some("portable");
    }
    if std::env::var_os("APPIMAGE").is_some() {
        return Some("AppImage");
    }
    if cfg!(target_os = "linux") {
        return None;
    }
    Some(unpackaged())
}

fn unpackaged() -> &'static str {
    if cfg!(debug_assertions) {
        "local (debug)"
    } else {
        "installed/local"
    }
}

/// Asks the package managers which of them owns the program. Each is a process that
/// reads its database (tens of milliseconds, seconds on a cold disk), so this runs
/// beside the start, not in it.
fn packaged() -> &'static str {
    #[cfg(target_os = "linux")]
    if let Some(exe) = std::env::var_os(gall_core::updater::BASE_EXE_ENV)
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
    {
        for (command, query, kind) in [("dpkg-query", "-S", "deb"), ("rpm", "-qf", "rpm")] {
            if std::process::Command::new(command)
                .arg(query)
                .arg(&exe)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
            {
                return kind;
            }
        }
    }
    unpackaged()
}

fn format_installation(base: &str) -> String {
    if let Ok(base_ver) = std::env::var(gall_core::updater::BASE_VERSION_ENV) {
        let trimmed = base_ver.trim();
        if !trimmed.is_empty() {
            return format!(
                "{base} (overlay {} over {trimmed})",
                crate::updates::running_version()
            );
        }
    }
    base.to_string()
}

fn identity(
    dirs: &gall_core::util::appdirs::AppDirs,
) -> std::collections::BTreeMap<String, String> {
    [
        ("version", crate::updates::running_version()),
        (
            "revision",
            option_env!("GALL_BUILD_REVISION")
                .unwrap_or("unknown")
                .into(),
        ),
        ("os", std::env::consts::OS.into()),
        ("arch", std::env::consts::ARCH.into()),
        (
            "installation",
            installation(dirs)
                .map(format_installation)
                .unwrap_or_else(|| "not known yet".into()),
        ),
        (
            "desktop",
            sanitize(&format!(
                "{} {}",
                std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".into()),
                std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into())
            )),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}
pub fn init(dirs: &gall_core::util::appdirs::AppDirs) {
    let _ = DIAGNOSTICS.set(Mutex::new(Diagnostics::open_with_identity(
        dirs.data.join(
            if std::env::var_os("GALL_MULTI_INSTANCE").is_some()
                && !(cfg!(debug_assertions)
                    && matches!(
                        std::env::var("GALL_SELFTEST").as_deref(),
                        Ok("bug-report" | "bug-report-text")
                    ))
            {
                format!("diagnostics-test-{}.json", std::process::id())
            } else {
                "diagnostics.json".into()
            },
        ),
        identity(dirs),
    )));
    if installation(dirs).is_none() {
        std::thread::spawn(|| {
            let kind = format_installation(packaged());
            if let Some(d) = DIAGNOSTICS.get() {
                d.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_identity("installation", &kind);
            }
        });
    }
    // Ordinary events (one per UI command) stay in memory; this thread writes them out
    // periodically when dirty. A hard crash loses at most the last 30 seconds of ordinary
    // events: errors and panics are written at once and a normal exit writes everything
    // (`clean`). The file is written after the lock is released, so recording never waits
    // for the disk.
    let _ = std::thread::Builder::new()
        .name("diagnostics-flush".into())
        .spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let pending = DIAGNOSTICS
                .get()
                .map(|d| d.lock().unwrap_or_else(|e| e.into_inner()))
                .and_then(|mut d| d.pending());
            if let Some(pending) = pending {
                pending.write();
            }
        });
    let old = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        record("panic", &location);
        log::error!(
            "panic: {info}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        old(info);
    }));
}
fn short_error_message(message: &str) -> String {
    message
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Unknown error")
        .chars()
        .take(1000)
        .collect()
}

pub fn record(kind: &str, message: &str) {
    if let Some(lock) = DIAGNOSTICS.get() {
        // A panic may come from a thread that holds the lock: never wait there. Everything
        // else waits (the lock is held for microseconds), so no event is dropped because the
        // flush thread happened to be reading the session.
        let guard = if kind == "panic" {
            lock.try_lock().ok()
        } else {
            // A panic elsewhere must not silence the record for good.
            Some(lock.lock().unwrap_or_else(|e| e.into_inner()))
        };
        if let Some(mut d) = guard {
            // Keep successful actions categorical; only the latest error needs a short message.
            let safe = if kind == "command" || kind == "action" {
                message
                    .split_whitespace()
                    .filter(|w| {
                        w.len() <= 64
                            && w.chars()
                                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                    })
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" ")
            } else if matches!(kind, "error" | "panic") {
                short_error_message(message)
            } else {
                let frames = message
                    .lines()
                    .filter(|line| {
                        line.contains(".tsx:")
                            || line.contains(".ts:")
                            || line.contains(".rs:")
                            || line.contains(".js:")
                    })
                    .take(20)
                    .map(|line| {
                        line.split_whitespace()
                            .filter(|word| {
                                word.contains(".tsx:")
                                    || word.contains(".ts:")
                                    || word.contains(".rs:")
                                    || word.contains(".js:")
                            })
                            .map(|word| {
                                let tail = word.rsplit(['/', '\\']).next().unwrap_or("unknown");
                                tail.chars()
                                    .filter(|c| {
                                        c.is_ascii_alphanumeric()
                                            || matches!(c, '.' | ':' | '_' | '-')
                                    })
                                    .take(100)
                                    .collect::<String>()
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("diagnostic event {frames}; see optional log")
            };
            d.record(
                if matches!(kind, "command" | "action" | "error" | "warn" | "panic") {
                    kind
                } else {
                    "event"
                },
                &format!("{}ms {safe}", crate::startup_ms()),
            );
        }
    }
}
pub fn clean() {
    if let Ok(mut clipboard) = REPORT_CLIPBOARD.lock() {
        clipboard.take();
    }
    if let Some(d) = DIAGNOSTICS.get() {
        if let Ok(mut d) = d.lock() {
            d.clean();
        }
    }
}
/// Undoes [`clean`] when the process was not actually left (a failed update install).
pub fn reopen() {
    if let Some(d) = DIAGNOSTICS.get() {
        if let Ok(mut d) = d.lock() {
            d.reopen();
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub view_mode: Option<String>,
    pub active_dialog: Option<String>,
    pub pane_count: Option<u32>,
    pub tab_count: Option<u32>,
    pub selection_count: Option<u32>,
    pub task_count: Option<u32>,
    pub trigger_error: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    title: String,
    body: String,
    full_body: String,
    log: String,
    previous_unclean: bool,
    description: String,
    environment: String,
    context: String,
    events: Vec<String>,
}
#[tauri::command]
pub fn bug_report_record(kind: String, message: String) {
    record(&kind, &message);
}
/// Several `(kind, message)` records in one call: the UI notes every command it runs,
/// and one message per note doubled the traffic between the UI and the backend.
#[tauri::command]
pub fn bug_report_record_many(entries: Vec<(String, String)>) {
    for (kind, message) in &entries {
        record(kind, message);
    }
}
#[tauri::command]
pub fn bug_report_status() -> bool {
    DIAGNOSTICS
        .get()
        .and_then(|d| d.lock().ok())
        .is_some_and(|d| d.previous.unclean)
}
#[tauri::command]
pub fn bug_report_dismiss() {
    if let Some(d) = DIAGNOSTICS.get() {
        if let Ok(mut d) = d.lock() {
            d.previous.unclean = false;
        }
    }
}
#[tauri::command]
pub fn bug_report_prepare(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, crate::state::AppState>,
    description: String,
    include_log: bool,
    snapshot: Snapshot,
    previous: bool,
) -> Result<Report, String> {
    let d = DIAGNOSTICS
        .get()
        .ok_or("Diagnostics unavailable")?
        .lock()
        .map_err(|_| "Diagnostics unavailable")?;
    let session = if previous { &d.previous } else { &d.current };
    let previous_unclean = d.previous.unclean;
    let metadata = session.identity.clone();
    let meta = |key: &str| metadata.get(key).map(String::as_str).unwrap_or("unknown");
    let events = session.events.iter().cloned().collect::<Vec<_>>();
    let last_error = snapshot
        .trigger_error
        .filter(|_| !previous)
        .map(|error| format!("error: {}", sanitize(&short_error_message(&error))))
        .or_else(|| session.latest_error().map(str::to_owned));
    drop(d);
    let known = |v: Option<String>, allowed: &[&str]| {
        v.filter(|s| allowed.contains(&s.as_str()))
            .unwrap_or_else(|| "unknown".into())
    };
    let view = known(
        snapshot.view_mode,
        &[
            "standard", "columns", "classic", "miller", "list", "grid", "details",
        ],
    );
    let dialog = known(
        snapshot.active_dialog,
        &[
            "none",
            "settings",
            "about",
            "search",
            "rename",
            "preview",
            "bugReport",
        ],
    );
    let description = description.chars().take(10000).collect::<String>();
    let resolution = window
        .inner_size()
        .map(|size| format!("{}x{}", size.width, size.height))
        .unwrap_or_else(|_| "unknown".into());
    let environment = format!("- Gall: {}\n- Build: {}\n- OS: {}\n- Architecture: {}\n- Installation: {}\n- Resolution: {}\n- Report time: {}\n- Session: {}", meta("version"), meta("revision"), meta("os"), meta("arch"), meta("installation"), resolution, chrono::Utc::now().to_rfc3339(), if previous { "previous irregular exit" } else { "current" });
    let context = format!(
        "- View: {}\n- Dialog: {}\n- Panes: {:?}; tabs: {:?}; selected: {:?}; tasks: {:?}",
        view,
        dialog,
        snapshot.pane_count,
        snapshot.tab_count,
        snapshot.selection_count,
        snapshot.task_count
    );
    let body = assemble_body(&description, &environment, last_error.as_deref());
    let log = if include_log {
        let folder = if state.dirs.portable {
            Some(state.dirs.logs())
        } else {
            app.path().app_log_dir().ok()
        };
        folder
            .and_then(|p| std::fs::read_dir(p).ok())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
            .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
            .and_then(|e| {
                use std::io::{Read, Seek, SeekFrom};
                let mut f = std::fs::File::open(e.path()).ok()?;
                let len = f.metadata().ok()?.len();
                f.seek(SeekFrom::Start(len.saturating_sub(32768))).ok()?;
                let mut bytes = Vec::new();
                f.take(32768).read_to_end(&mut bytes).ok()?;
                Some(
                    String::from_utf8_lossy(&bytes)
                        .lines()
                        .map(sanitize)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            })
            .unwrap_or_else(|| "No log excerpt available".into())
    } else {
        String::new()
    };
    let full_body = if include_log {
        let environment = format!("{environment}\n- Desktop/display: {}", meta("desktop"));
        format!(
            "{}\n## Additional sanitized diagnostic log\n```text\n{}\n```\n",
            assemble_detailed_body(
                &description,
                &environment,
                &context,
                &events,
                last_error.as_deref()
            ),
            log.replace("```", "'''")
        )
    } else {
        body.clone()
    };
    Ok(Report {
        title: format!("Bug report: Gall {} on {}", meta("version"), meta("os")),
        body,
        full_body,
        log,
        previous_unclean,
        description,
        environment,
        context,
        events,
    })
}
fn assemble_body(description: &str, environment: &str, last_error: Option<&str>) -> String {
    format!(
        "## What happened?\n{description}\n\n## Environment\n{environment}\n\n## Recent actions and errors\n```text\n{}\n```\n",
        last_error.unwrap_or("No error recorded.").replace("```", "'''")
    )
}
fn assemble_detailed_body(
    description: &str,
    environment: &str,
    context: &str,
    events: &[String],
    last_error: Option<&str>,
) -> String {
    format!("{}\n## Current UI context (may differ from previous session)\n{context}\n\n## Recent action history\n```text\n{}\n```\n", assemble_body(description, environment, last_error), events.join("\n").replace("```", "'''"))
}

pub use gall_core::report_bundle::ReportAttachment;

/// Export only to a new ZIP chosen by the user; never replace existing data.
fn export_report(
    path: &std::path::Path,
    body: &str,
    attachments: &[ReportAttachment],
) -> Result<(), String> {
    gall_core::report_bundle::export_report_bundle(path, body, attachments)
}
#[tauri::command]
pub async fn bug_report_export(
    path: String,
    body: String,
    attachments: Vec<ReportAttachment>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        export_report(std::path::Path::new(&path), &body, &attachments)
    })
    .await
    .map_err(|error| format!("Cannot export report: {error}"))?
}

#[cfg(test)]
mod export_tests {
    use super::{assemble_body, assemble_detailed_body, export_report};
    #[test]
    fn report_sections_keep_user_markdown_separate() {
        let description =
            "## Environment\nuser text\n## Recent actions and errors\n```\ninput\n```";
        let body = assemble_body(description, "- Gall: old", Some("error: File not found"));
        assert!(body.starts_with(&format!("## What happened?\n{description}\n\n")));
        assert!(body.ends_with("error: File not found\n```\n"));
    }
    #[test]
    fn copied_report_excludes_history_and_ui_context() {
        let events: Vec<String> = vec!["command: list_dir ok".into()];
        let last_error = Some("error: File not found");
        let compact = assemble_body("Cannot open file", "- Resolution: 1280x720", last_error);
        assert!(compact.contains("error: File not found"));
        assert!(compact.contains("- Resolution: 1280x720"));
        assert!(!compact.contains("list_dir ok"));
        assert!(!compact.contains("Current UI context"));
        let detailed = assemble_detailed_body(
            "Cannot open file",
            "- Resolution: 1280x720",
            "- View: columns",
            &events,
            last_error,
        );
        assert!(detailed.contains("list_dir ok"));
        assert!(detailed.contains("Current UI context"));
        assert!(detailed.contains("error: File not found"));
    }
    #[test]
    fn copied_report_handles_no_error_and_escapes_error_fences() {
        assert!(assemble_body("", "", None).contains("No error recorded."));
        assert!(assemble_body("", "", Some("error: ```")).ends_with("error: '''\n```\n"));
    }
    struct TestDir(std::path::PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::current_dir().unwrap().join(format!(
                "gall-report-export-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn export_preserves_existing_file() {
        let dir = TestDir::new();
        let path = dir.0.join("report.zip");
        std::fs::write(&path, "existing user data").unwrap();
        assert!(export_report(&path, "new report", &[])
            .unwrap_err()
            .contains("Choose another name"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "existing user data");
    }
    #[test]
    fn export_rejects_relative_and_oversized_reports() {
        assert!(
            export_report(std::path::Path::new("report.zip"), "report", &[])
                .unwrap_err()
                .contains("absolute")
        );
        let dir = TestDir::new();
        let path = dir.0.join("report.zip");
        assert!(export_report(&path, &"a".repeat(1_000_001), &[])
            .unwrap_err()
            .contains("limit"));
        assert!(!path.exists());
    }
}

#[cfg(test)]
const MAX_SCREENSHOT_BYTES: usize = gall_core::report_bundle::MAX_MEDIA_BYTES;
static REPORT_CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

#[derive(Serialize)]
pub struct ScreenshotSize {
    width: u32,
    height: u32,
}

pub(super) fn decode_screenshot(png: &[u8]) -> Result<image::RgbaImage, String> {
    gall_core::report_bundle::decode_report_png(png)
}

/// Keep report text available while the user switches to GitHub, including X11.
#[tauri::command]
pub async fn bug_report_copy_text(text: String) -> Result<(), String> {
    if text.len() > 1_000_000 {
        return Err("Report exceeds clipboard limit".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut clipboard = REPORT_CLIPBOARD
            .lock()
            .map_err(|_| "Report clipboard unavailable")?;
        if clipboard.is_none() {
            *clipboard =
                Some(arboard::Clipboard::new().map_err(|e| format!("Clipboard unavailable: {e}"))?);
        }
        clipboard
            .as_mut()
            .unwrap()
            .set_text(text)
            .map_err(|e| format!("Cannot copy report: {e}"))
    })
    .await
    .map_err(|e| format!("Cannot copy report: {e}"))?
}

/// Retain the clipboard owner for X11/Wayland while the user pastes in a browser.
#[tauri::command]
pub async fn bug_report_copy_screenshot(png: Vec<u8>) -> Result<ScreenshotSize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let image = decode_screenshot(&png)?;
        let size = ScreenshotSize {
            width: image.width(),
            height: image.height(),
        };
        let mut clipboard = REPORT_CLIPBOARD
            .lock()
            .map_err(|_| "Report clipboard unavailable")?;
        if clipboard.is_none() {
            *clipboard =
                Some(arboard::Clipboard::new().map_err(|e| format!("Clipboard unavailable: {e}"))?);
        }
        clipboard
            .as_mut()
            .unwrap()
            .set_image(arboard::ImageData {
                width: size.width as usize,
                height: size.height as usize,
                bytes: std::borrow::Cow::Owned(image.into_raw()),
            })
            .map_err(|e| format!("Cannot copy screenshot: {e}"))?;
        Ok(size)
    })
    .await
    .map_err(|e| format!("Cannot copy screenshot: {e}"))?
}

#[cfg(test)]
mod screenshot_tests {
    use super::*;
    #[test]
    fn screenshot_decodes_exact_pixels() {
        let pixels =
            image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 128, 255, 255]).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels.clone())
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(decode_screenshot(png.get_ref()).unwrap(), pixels);
    }
    #[test]
    fn screenshot_rejects_malformed_and_oversized_inputs() {
        assert!(decode_screenshot(b"not a png").is_err());
        assert!(decode_screenshot(&vec![0; MAX_SCREENSHOT_BYTES + 1])
            .unwrap_err()
            .contains("8 MiB"));
        let mut header = vec![0; 24];
        header[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        header[12..16].copy_from_slice(b"IHDR");
        for (width, height) in [(0u32, 1u32), (8193, 1), (8192, 8192)] {
            header[16..20].copy_from_slice(&width.to_be_bytes());
            header[20..24].copy_from_slice(&height.to_be_bytes());
            assert!(decode_screenshot(&header)
                .unwrap_err()
                .contains("dimension"));
        }
        header[16..20].copy_from_slice(&1u32.to_be_bytes());
        header[20..24].copy_from_slice(&1u32.to_be_bytes());
        assert!(decode_screenshot(&header)
            .unwrap_err()
            .contains("valid PNG"));
    }
}
