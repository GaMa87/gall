//! Update availability depends on the installed distribution and release signing key.
use crate::state::AppState;

#[tauri::command]
pub fn update_support(state: tauri::State<'_, AppState>) -> String {
    if state.dirs.portable {
        return "Portable installations must download and extract the new ZIP manually.".into();
    }
    if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_none() {
        return "Update this installation using your package manager or download the new package."
            .into();
    }
    if option_env!("GALL_UPDATER_PUBLIC_KEY")
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return "This build has no update signing key configured. Download the latest release manually.".into();
    }
    String::new()
}

#[tauri::command]
pub fn update_ready(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let reason = update_support(state.clone());
    if !reason.is_empty() {
        return Err(reason);
    }
    if !state.tasks.running_ids().is_empty() {
        return Err("Wait for file operations to finish before installing an update.".into());
    }
    Ok(())
}

/// Serializes task starts with the final installer and application restart.
pub static INSTALL_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
static INSTALLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    use tauri::{Emitter, Manager};
    use tauri_plugin_updater::UpdaterExt;
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("An update is already being installed.".into());
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            INSTALLING.store(false, Ordering::SeqCst);
        }
    }
    let _reset = Reset;
    update_ready(app.state())?;
    let update = app
        .updater_builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("No update is available.")?;
    if update.version.split('+').next().unwrap_or("").contains('-') {
        return Err("Prerelease updates are not enabled.".into());
    }
    let mut downloaded = 0u64;
    let bytes = update
        .download(
            |count, _| {
                downloaded += count as u64;
                let _ = app.emit("update-progress", downloaded);
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    // No task can start between the active-task check and process replacement.
    let _gate = INSTALL_GATE.lock().map_err(|e| e.to_string())?;
    update_ready(app.state())?;
    update.install(bytes).map_err(|e| e.to_string())?;
    app.restart();
}
