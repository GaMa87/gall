//! Linux `org.freedesktop.FileManager1` D-Bus service and `reveal_path` loop guard.

#[cfg(target_os = "linux")]
use crate::cli::CliArgs;
use serde::Serialize;
#[cfg(any(target_os = "linux", test))]
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;

#[cfg(target_os = "linux")]
static OWNS_FILE_MANAGER1: AtomicBool = AtomicBool::new(false);

/// True if this Gall instance currently owns `org.freedesktop.FileManager1` on the session bus.
#[cfg(target_os = "linux")]
pub fn owns_name() -> bool {
    OWNS_FILE_MANAGER1.load(Ordering::Acquire)
}

/// Pure loop-guard predicate: when Gall owns `org.freedesktop.FileManager1` on Linux,
/// `reveal_path` must navigate internally instead of calling `reveal_item_in_dir`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn should_reveal_internally(owns: bool) -> bool {
    cfg!(target_os = "linux") && owns
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::enum_variant_names)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum FileManagerAction {
    ShowFolders {
        folders: Vec<String>,
        startup_id: String,
    },
    ShowItems {
        folders: Vec<String>,
        select_paths: Vec<String>,
        startup_id: String,
    },
    ShowItemProperties {
        folder: String,
        target: String,
        startup_id: String,
    },
}

/// Decodes a `file://` URI (or raw absolute path when no URI scheme is present) into a local `PathBuf`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn decode_dbus_uri(raw: &str) -> Option<PathBuf> {
    if let Some(p) = gall_core::nav::path::decode_file_uri(raw) {
        return Some(p);
    }
    if !raw.contains("://") && Path::new(raw).is_absolute() {
        return Some(PathBuf::from(raw));
    }
    None
}

/// Most URIs one D-Bus request may open (each folder becomes a tab, each item a selection).
#[cfg(any(target_os = "linux", test))]
pub(crate) const MAX_DBUS_URIS: usize = 50;

#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_dbus_uris(uris: &[String]) -> Vec<PathBuf> {
    uris.iter()
        .filter_map(|u| decode_dbus_uri(u))
        .take(MAX_DBUS_URIS)
        .collect()
}

/// Runs the (blocking, lock-taking) window routing off the D-Bus executor thread.
#[cfg(target_os = "linux")]
async fn dispatch_blocking(app: &AppHandle, action: FileManagerAction) -> zbus::fdo::Result<()> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || dispatch_action(&app, None, action))
        .await
        .map_err(|err| zbus::fdo::Error::Failed(err.to_string()))?
        .map_err(zbus::fdo::Error::Failed)
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn group_show_items(paths: &[PathBuf]) -> (Vec<String>, Vec<String>) {
    let mut folders = Vec::new();
    let mut select_paths = Vec::new();
    for p in paths {
        let folder = p
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned();
        if !folders.contains(&folder) {
            folders.push(folder);
        }
        let item = p.to_string_lossy().into_owned();
        if !select_paths.contains(&item) {
            select_paths.push(item);
        }
    }
    (folders, select_paths)
}

#[cfg(target_os = "linux")]
pub(crate) fn action_to_cli_args(action: &FileManagerAction) -> CliArgs {
    match action {
        FileManagerAction::ShowFolders { folders, .. } => CliArgs {
            path1: folders.first().cloned(),
            paths: folders.clone(),
            ..Default::default()
        },
        FileManagerAction::ShowItems {
            folders,
            select_paths,
            ..
        } => CliArgs {
            path1: folders.first().cloned(),
            paths: folders.clone(),
            select_paths: select_paths.clone(),
            ..Default::default()
        },
        FileManagerAction::ShowItemProperties { folder, target, .. } => CliArgs {
            path1: Some(folder.clone()),
            paths: vec![folder.clone()],
            select_paths: vec![target.clone()],
            ..Default::default()
        },
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn dispatch_action(
    app: &AppHandle,
    target_override: Option<&str>,
    action: FileManagerAction,
) -> Result<(), String> {
    let cli_args = action_to_cli_args(&action);
    crate::route_fm_to_window(app, target_override, cli_args, action)
}

/// Reveals `path` internally in `label` (or an active/new window) without invoking D-Bus.
#[cfg(target_os = "linux")]
pub(crate) fn reveal_in_window(app: &AppHandle, label: &str, path: PathBuf) -> Result<(), String> {
    let (folders, select_paths) = group_show_items(&[path]);
    dispatch_action(
        app,
        Some(label),
        FileManagerAction::ShowItems {
            folders,
            select_paths,
            startup_id: String::new(),
        },
    )
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::sync::OnceLock;

    static DBUS_CONNECTION: OnceLock<zbus::Connection> = OnceLock::new();

    struct FileManager1Service {
        app: AppHandle,
    }

    #[zbus::interface(name = "org.freedesktop.FileManager1")]
    impl FileManager1Service {
        #[zbus(name = "ShowFolders")]
        async fn show_folders(
            &self,
            uris: Vec<String>,
            startup_id: String,
        ) -> zbus::fdo::Result<()> {
            if uris.is_empty() {
                return Ok(());
            }
            let paths = parse_dbus_uris(&uris);
            if paths.is_empty() {
                // Not an error for the caller (browsers may pass http:// or trash:// items):
                // there is simply nothing local to show.
                log::warn!(
                    "FileManager1: ignoring request without valid local file URIs: {uris:?}"
                );
                return Ok(());
            }
            // `is_dir`/`exists` may wait on a slow mount: not on the D-Bus executor thread.
            let folders = tauri::async_runtime::spawn_blocking(move || {
                let mut folders: Vec<String> = Vec::new();
                for p in paths {
                    let folder = if p.is_dir() || !p.exists() {
                        p.to_string_lossy().into_owned()
                    } else {
                        p.parent()
                            .filter(|d| !d.as_os_str().is_empty())
                            .unwrap_or(&p)
                            .to_string_lossy()
                            .into_owned()
                    };
                    if !folders.contains(&folder) {
                        folders.push(folder);
                    }
                }
                folders
            })
            .await
            .map_err(|err| zbus::fdo::Error::Failed(err.to_string()))?;
            dispatch_blocking(
                &self.app,
                FileManagerAction::ShowFolders {
                    folders,
                    startup_id,
                },
            )
            .await
        }

        #[zbus(name = "ShowItems")]
        async fn show_items(&self, uris: Vec<String>, startup_id: String) -> zbus::fdo::Result<()> {
            if uris.is_empty() {
                return Ok(());
            }
            let paths = parse_dbus_uris(&uris);
            if paths.is_empty() {
                // Not an error for the caller (browsers may pass http:// or trash:// items):
                // there is simply nothing local to show.
                log::warn!(
                    "FileManager1: ignoring request without valid local file URIs: {uris:?}"
                );
                return Ok(());
            }
            let (folders, select_paths) = group_show_items(&paths);
            dispatch_blocking(
                &self.app,
                FileManagerAction::ShowItems {
                    folders,
                    select_paths,
                    startup_id,
                },
            )
            .await
        }

        #[zbus(name = "ShowItemProperties")]
        async fn show_item_properties(
            &self,
            uris: Vec<String>,
            startup_id: String,
        ) -> zbus::fdo::Result<()> {
            if uris.is_empty() {
                return Ok(());
            }
            let paths = parse_dbus_uris(&uris);
            let Some(target_path) = paths.into_iter().next() else {
                // Not an error for the caller (browsers may pass http:// or trash:// items):
                // there is simply nothing local to show.
                log::warn!(
                    "FileManager1: ignoring request without valid local file URIs: {uris:?}"
                );
                return Ok(());
            };
            let folder = target_path
                .parent()
                .filter(|d| !d.as_os_str().is_empty())
                .unwrap_or(&target_path)
                .to_string_lossy()
                .into_owned();
            let target = target_path.to_string_lossy().into_owned();
            dispatch_blocking(
                &self.app,
                FileManagerAction::ShowItemProperties {
                    folder,
                    target,
                    startup_id,
                },
            )
            .await
        }
    }

    pub(super) fn start_service(app: AppHandle) {
        tauri::async_runtime::spawn(async move {
            let service = FileManager1Service { app };
            let builder = match zbus::connection::Builder::session() {
                Ok(b) => b,
                Err(err) => {
                    log::warn!("FileManager1 D-Bus session bus unavailable: {err}");
                    return;
                }
            };
            let builder = match builder.name("org.freedesktop.FileManager1") {
                Ok(b) => b
                    .replace_existing_names(false)
                    .allow_name_replacements(false),
                Err(err) => {
                    log::warn!("FileManager1 invalid bus name: {err}");
                    return;
                }
            };
            let builder = match builder.serve_at("/org/freedesktop/FileManager1", service) {
                Ok(b) => b,
                Err(err) => {
                    log::warn!("FileManager1 serve_at failed: {err}");
                    return;
                }
            };
            match builder.build().await {
                Ok(conn) => {
                    let _ = DBUS_CONNECTION.set(conn);
                    OWNS_FILE_MANAGER1.store(true, Ordering::Release);
                    log::info!("registered org.freedesktop.FileManager1 on session D-Bus");
                }
                Err(zbus::Error::NameTaken) => {
                    OWNS_FILE_MANAGER1.store(false, Ordering::Release);
                    log::info!("org.freedesktop.FileManager1 already owned; skipping registration");
                }
                Err(err) => {
                    OWNS_FILE_MANAGER1.store(false, Ordering::Release);
                    log::warn!("FileManager1 D-Bus registration skipped: {err}");
                }
            }
        });
    }
}

/// Registers `org.freedesktop.FileManager1` on the Linux session D-Bus without blocking startup.
pub fn start(app: AppHandle) {
    #[cfg(target_os = "linux")]
    {
        linux::start_service(app);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_capped_at_max_uris() {
        let uris: Vec<String> = (0..MAX_DBUS_URIS + 30)
            .map(|i| format!("file:///tmp/d{i}"))
            .collect();
        assert_eq!(parse_dbus_uris(&uris).len(), MAX_DBUS_URIS);
    }

    #[test]
    fn decode_dbus_uri_handles_file_uris_raw_paths_and_rejects_other_schemes() {
        assert_eq!(
            decode_dbus_uri("file:///home/u/My%20File.txt"),
            Some(PathBuf::from("/home/u/My File.txt"))
        );
        assert_eq!(
            decode_dbus_uri("file://localhost/tmp/%C4%8Crka.txt"),
            Some(PathBuf::from("/tmp/Črka.txt"))
        );
        // A raw path must be absolute on the host (`/x` is not on Windows).
        let abs = if cfg!(windows) {
            r"C:\plain\abs\path"
        } else {
            "/plain/abs/path"
        };
        assert_eq!(decode_dbus_uri(abs), Some(PathBuf::from(abs)));
        assert_eq!(decode_dbus_uri("https://example.com/file.txt"), None);
        assert_eq!(decode_dbus_uri("file:///tmp/bad%00"), None);
        assert_eq!(decode_dbus_uri("relative/path"), None);
    }

    #[test]
    fn group_show_items_deduplicates_folders_and_preserves_order() {
        let (folders, items) = group_show_items(&[
            PathBuf::from("/"),
            PathBuf::from("/home/u/a.txt"),
            PathBuf::from("/home/u/b.txt"),
            PathBuf::from("/var/log/syslog"),
        ]);
        assert_eq!(folders, vec!["/", "/home/u", "/var/log"]);
        assert_eq!(
            items,
            vec!["/", "/home/u/a.txt", "/home/u/b.txt", "/var/log/syslog"]
        );
    }

    #[test]
    fn loop_guard_predicate_respects_platform_and_ownership() {
        assert!(!should_reveal_internally(false));
        assert_eq!(should_reveal_internally(true), cfg!(target_os = "linux"));
    }
}
