//! Windows taskbar AppUserModelID and relaunch command for overlay updates.

use std::path::Path;
use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_AppUserModel_ID, PKEY_AppUserModel_RelaunchCommand,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

const APP_ID: &str = "org.gall.app";

/// Pins taskbar shortcuts to the stable base executable rather than an ephemeral overlay path.
pub fn configure_taskbar_identity_for_window(win: &tauri::WebviewWindow) {
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(w!("org.gall.app"));
    }
    let Some(base_exe) = std::env::var_os(gall_core::updater::BASE_EXE_ENV) else {
        return;
    };
    let Ok(raw_hwnd) = win.hwnd() else {
        return;
    };
    let hwnd = HWND(raw_hwnd.0);
    let base_str = Path::new(&base_exe).to_string_lossy().into_owned();
    let relaunch_cmd = format!("\"{base_str}\"");
    unsafe {
        if let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(hwnd) {
            let id_val = PROPVARIANT::from(APP_ID);
            let cmd_val = PROPVARIANT::from(relaunch_cmd.as_str());
            let _ = store.SetValue(&PKEY_AppUserModel_ID, &id_val);
            let _ = store.SetValue(&PKEY_AppUserModel_RelaunchCommand, &cmd_val);
            let _ = store.Commit();
        }
    }
}
