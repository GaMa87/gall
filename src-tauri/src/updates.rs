//! In-app update checking, staging, installation, and startup status reporting.

use crate::state::AppState;
use gall_core::updater::{
    bootstrap, detect_capability_with, install, take_startup_notice, ArchiveKind, CapabilityProbe,
    InstallStrategy, StartupNotice, UpdateCapability, UpdateStage, BASE_EXE_ENV, BASE_VERSION_ENV,
    WAIT_PID_FLAG,
};
use gall_core::util::appdirs::AppDirs;
use serde::Serialize;
use std::ffi::OsString;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

/// Serializes task starts with the final update swap and application restart.
pub static INSTALL_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
static UPDATING: AtomicBool = AtomicBool::new(false);

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        UPDATING.store(false, Ordering::SeqCst);
    }
}

fn acquire_busy_guard() -> Result<BusyGuard, String> {
    if UPDATING.swap(true, Ordering::SeqCst) {
        return Err("An update is already in progress.".into());
    }
    Ok(BusyGuard)
}

/// Returns the version string this build reports as its running version.
/// Debug builds allow `GALL_UPDATER_TEST_VERSION` so real self-update restarts can be tested.
pub fn running_version() -> String {
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var("GALL_UPDATER_TEST_VERSION") {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}

pub fn effective_pubkey() -> String {
    let compiled = option_env!("GALL_UPDATER_PUBLIC_KEY")
        .unwrap_or("")
        .trim()
        .to_string();
    #[cfg(debug_assertions)]
    if compiled.is_empty() {
        if let Ok(v) = std::env::var("GALL_UPDATER_TEST_PUBKEY") {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    compiled
}

fn test_endpoints() -> Option<Vec<tauri::Url>> {
    #[cfg(debug_assertions)]
    if let Ok(raw) = std::env::var("GALL_UPDATER_TEST_ENDPOINT") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            if let Ok(url) = tauri::Url::parse(trimmed) {
                return Some(vec![url]);
            }
        }
    }
    None
}

fn force_overlay_in_debug() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("GALL_UPDATER_TEST_FORCE_OVERLAY").is_some_and(|v| !v.is_empty())
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

pub fn build_capability(dirs: &AppDirs) -> UpdateCapability {
    let mut probe = CapabilityProbe::from_env(effective_pubkey(), running_version(), dirs.clone());
    if force_overlay_in_debug() {
        probe.force_overlay = true;
    }
    detect_capability_with(&probe)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCapabilityDto {
    pub strategy: String,
    pub install_kind: String,
    pub target: String,
    pub reason: String,
    pub install_dir: String,
    pub updates_dir: String,
    pub base_exe_path: String,
    pub base_version: String,
    pub running_overlay: bool,
    pub active_overlay_version: Option<String>,
}

impl From<&UpdateCapability> for UpdateCapabilityDto {
    fn from(cap: &UpdateCapability) -> Self {
        Self {
            strategy: cap.strategy.as_str().into(),
            install_kind: cap.install_kind.as_str().into(),
            target: cap.target_key.clone(),
            reason: cap.reason.clone(),
            install_dir: cap.install_dir.to_string_lossy().into_owned(),
            updates_dir: cap.updates_dir.to_string_lossy().into_owned(),
            base_exe_path: cap.base_exe_path.to_string_lossy().into_owned(),
            base_version: cap.base_version.clone(),
            running_overlay: cap.running_overlay,
            active_overlay_version: cap.active_overlay_version.clone(),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckDto {
    pub available: bool,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub staged: bool,
    #[serde(flatten)]
    pub capability: UpdateCapabilityDto,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedDto {
    pub version: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgressEvent {
    pub stage: UpdateStage,
    pub downloaded: u64,
    pub total: Option<u64>,
}

#[tauri::command]
pub fn update_capability(state: tauri::State<'_, AppState>) -> UpdateCapabilityDto {
    let cap = build_capability(&state.dirs);
    UpdateCapabilityDto::from(&cap)
}

#[tauri::command]
pub fn update_support(state: tauri::State<'_, AppState>) -> String {
    build_capability(&state.dirs).reason
}

fn ensure_ready(state: &AppState) -> Result<(), String> {
    let cap = build_capability(&state.dirs);
    if !cap.reason.is_empty() {
        return Err(cap.reason);
    }
    if !state.tasks.running_ids().is_empty() {
        return Err("Wait for file operations to finish before installing an update.".into());
    }
    if crate::commands::git_jobs_active() {
        return Err("Wait for Git operations to finish before installing an update.".into());
    }
    Ok(())
}

#[tauri::command]
pub fn update_ready(state: tauri::State<'_, AppState>) -> Result<(), String> {
    ensure_ready(&state)
}

#[tauri::command]
pub fn update_startup_notice(state: tauri::State<'_, AppState>) -> Option<StartupNotice> {
    take_startup_notice(&state.dirs.updates())
}

fn build_updater(
    app: &AppHandle,
    target_key: &str,
    timeout: Duration,
) -> Result<tauri_plugin_updater::Updater, String> {
    let current = running_version();
    let mut builder = app
        .updater_builder()
        .pubkey(effective_pubkey())
        .target(target_key)
        .timeout(timeout)
        .version_comparator(move |_pkg_ver, remote| {
            gall_core::updater::is_version_newer(&remote.version.to_string(), &current)
        });
    if let Some(urls) = test_endpoints() {
        builder = builder.endpoints(urls).map_err(|e| e.to_string())?;
    }
    builder.build().map_err(|e| e.to_string())
}

async fn check_remote(
    app: &AppHandle,
    cap: &UpdateCapability,
    timeout: Duration,
) -> Result<Option<tauri_plugin_updater::Update>, String> {
    let updater = build_updater(app, &cap.target_key, timeout)?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
    let Some(update) = update else {
        return Ok(None);
    };
    if update.version.split('+').next().unwrap_or("").contains('-') {
        return Ok(None);
    }
    Ok(Some(update))
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<UpdateCheckDto, String> {
    let state = app.state::<AppState>();
    let cap = build_capability(&state.dirs);
    let cap_dto = UpdateCapabilityDto::from(&cap);
    let pubkey = effective_pubkey();

    let update = check_remote(&app, &cap, Duration::from_secs(15)).await?;
    let Some(update) = update else {
        return Ok(UpdateCheckDto {
            available: false,
            version: None,
            notes: None,
            staged: false,
            capability: cap_dto,
        });
    };

    let require_signed_version = !matches!(cap.archive_kind, ArchiveKind::LinuxAppImage);
    let staged = !pubkey.is_empty()
        && install::verified_staged_version(
            &cap.updates_dir,
            &update.version,
            &cap.target_key,
            &pubkey,
            require_signed_version,
        )
        .is_some();

    Ok(UpdateCheckDto {
        available: true,
        version: Some(update.version),
        notes: update.body,
        staged,
        capability: cap_dto,
    })
}

async fn download_and_stage_inner(app: &AppHandle) -> Result<StagedDto, String> {
    let state = app.state::<AppState>();
    let cap = build_capability(&state.dirs);
    if !cap.reason.is_empty() {
        return Err(cap.reason);
    }
    let pubkey = effective_pubkey();
    let update = check_remote(app, &cap, Duration::from_secs(120))
        .await?
        .ok_or_else(|| "No update is available.".to_string())?;

    let require_signed_version = !matches!(cap.archive_kind, ArchiveKind::LinuxAppImage);
    if install::verified_staged_version(
        &cap.updates_dir,
        &update.version,
        &cap.target_key,
        &pubkey,
        require_signed_version,
    )
    .is_some()
    {
        return Ok(StagedDto {
            version: update.version,
        });
    }

    let mut downloaded = 0u64;
    let bytes = update
        .download(
            |count, total| {
                downloaded += count as u64;
                let _ = app.emit(
                    "update-progress",
                    UpdateProgressEvent {
                        stage: UpdateStage::Downloading,
                        downloaded,
                        total,
                    },
                );
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;

    let total_bytes = Some(bytes.len() as u64);
    let outer_sig = update.signature.clone();
    let expected_version = update.version.clone();
    let app_handle = app.clone();
    let cap_clone = cap.clone();

    let staged = tauri::async_runtime::spawn_blocking(move || {
        install::stage_update(
            &bytes,
            &outer_sig,
            &pubkey,
            &expected_version,
            &cap_clone,
            require_signed_version,
            &[],
            |stage| {
                let _ = app_handle.emit(
                    "update-progress",
                    UpdateProgressEvent {
                        stage,
                        downloaded,
                        total: total_bytes,
                    },
                );
            },
        )
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(StagedDto {
        version: staged.version,
    })
}

fn spawn_updated_process(
    launch_exe: &std::path::Path,
    cap: &UpdateCapability,
    raw_args: &[OsString],
) -> Result<(), String> {
    let user_args = bootstrap::strip_internal_args(raw_args);
    let mut cmd = Command::new(launch_exe);
    cmd.args(&user_args)
        .arg(WAIT_PID_FLAG)
        .arg(std::process::id().to_string());

    if cap.strategy == InstallStrategy::Overlay {
        cmd.env(BASE_EXE_ENV, &cap.base_exe_path)
            .env(BASE_VERSION_ENV, &cap.base_version);
    }
    if cap.archive_kind == ArchiveKind::LinuxAppImage {
        for key in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
            cmd.env_remove(key);
        }
    }
    #[cfg(debug_assertions)]
    cmd.env_remove("GALL_UPDATER_TEST_VERSION");

    cmd.spawn().map(|_| ()).map_err(|e| {
        format!(
            "Cannot restart updated Gall ({}): {e}",
            launch_exe.display()
        )
    })
}

async fn apply_staged_inner(app: &AppHandle) -> Result<(), String> {
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let _gate = INSTALL_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = app_handle.state::<AppState>();
        ensure_ready(&state)?;

        let cap = build_capability(&state.dirs);
        let pubkey = effective_pubkey();
        let from_version = running_version();
        let require_signed_version = !matches!(cap.archive_kind, ArchiveKind::LinuxAppImage);

        let _ = app_handle.emit(
            "update-progress",
            UpdateProgressEvent {
                stage: UpdateStage::Applying,
                downloaded: 0,
                total: None,
            },
        );

        // Close the crash marker and flush session synchronously before exiting so the next
        // launch never reports an irregular shutdown.
        state.flush_session();
        state.db.checkpoint();
        crate::bug_report::clean();

        let active_path = gall_core::updater::active_overlay_path(&cap.updates_dir);
        let prior_active = std::fs::read(&active_path).ok();

        let (launch_exe, _to_version) =
            match install::apply_staged(&cap, &from_version, &pubkey, require_signed_version) {
                Ok(res) => res,
                Err(err) => {
                    crate::bug_report::reopen();
                    return Err(err);
                }
            };

        let _ = app_handle.emit(
            "update-progress",
            UpdateProgressEvent {
                stage: UpdateStage::Restarting,
                downloaded: 0,
                total: None,
            },
        );

        let raw_args: Vec<OsString> = std::env::args_os().collect();
        if let Err(err) = spawn_updated_process(&launch_exe, &cap, &raw_args) {
            match cap.strategy {
                InstallStrategy::Overlay => {
                    if let Some(prev) = prior_active {
                        let _ = std::fs::write(&active_path, prev);
                    } else {
                        let _ = std::fs::remove_file(&active_path);
                    }
                }
                InstallStrategy::InPlace => {
                    let target = cap
                        .target_bundle_path
                        .as_deref()
                        .unwrap_or(&cap.target_exe_path);
                    let mut old_os = target.as_os_str().to_os_string();
                    old_os.push(gall_core::updater::OLD_SUFFIX);
                    let old_path = std::path::PathBuf::from(old_os);
                    if old_path.exists() {
                        let mut bad_os = target.as_os_str().to_os_string();
                        bad_os.push(".gall-failed");
                        let bad_path = std::path::PathBuf::from(bad_os);
                        let _ = std::fs::rename(target, &bad_path);
                        if std::fs::rename(&old_path, target).is_ok() {
                            let _ = std::fs::remove_file(&bad_path)
                                .or_else(|_| std::fs::remove_dir_all(&bad_path));
                        }
                    }
                }
                _ => {}
            }
            let _ = std::fs::remove_file(gall_core::updater::pending_notice_path(&cap.updates_dir));
            crate::bug_report::reopen();
            return Err(err);
        }

        std::process::exit(0);
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn download_update(app: AppHandle) -> Result<StagedDto, String> {
    let _busy = acquire_busy_guard()?;
    download_and_stage_inner(&app).await
}

#[tauri::command]
pub async fn apply_update(app: AppHandle) -> Result<(), String> {
    let _busy = acquire_busy_guard()?;
    apply_staged_inner(&app).await
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let _busy = acquire_busy_guard()?;
    download_and_stage_inner(&app).await?;
    apply_staged_inner(&app).await
}

#[tauri::command]
pub async fn revert_overlay_update(app: AppHandle) -> Result<(), String> {
    let _busy = acquire_busy_guard()?;
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let _gate = INSTALL_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = app_handle.state::<AppState>();
        ensure_ready(&state)?;

        let cap = build_capability(&state.dirs);
        if !cap.running_overlay {
            return Err("Gall is already running from its base installation.".into());
        }

        state.flush_session();
        state.db.checkpoint();
        crate::bug_report::clean();

        let active_path = gall_core::updater::active_overlay_path(&cap.updates_dir);
        let prior_active = std::fs::read(&active_path).ok();
        let from_version = running_version();
        if let Err(err) =
            install::revert_overlay(&cap.updates_dir, &from_version, &cap.base_version)
        {
            crate::bug_report::reopen();
            return Err(err);
        }

        let raw_args: Vec<OsString> = std::env::args_os().collect();
        let user_args = bootstrap::strip_internal_args(&raw_args);
        let mut cmd = Command::new(&cap.base_exe_path);
        cmd.args(&user_args)
            .arg(WAIT_PID_FLAG)
            .arg(std::process::id().to_string())
            .env_remove(BASE_EXE_ENV)
            .env_remove(BASE_VERSION_ENV);
        if cap.archive_kind == ArchiveKind::LinuxAppImage {
            for key in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
                cmd.env_remove(key);
            }
        }
        if let Err(err) = cmd.spawn() {
            if let Some(prev) = prior_active {
                let _ = std::fs::write(&active_path, prev);
            }
            let _ = std::fs::remove_file(gall_core::updater::pending_notice_path(&cap.updates_dir));
            crate::bug_report::reopen();
            return Err(format!(
                "Cannot start base Gall ({}): {err}",
                cap.base_exe_path.display()
            ));
        }
        std::process::exit(0);
    })
    .await
    .map_err(|e| e.to_string())?
}
