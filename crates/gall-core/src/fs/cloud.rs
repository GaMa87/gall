//! Cloud storage folders (OneDrive, Dropbox, Google Drive, iCloud, …) found on this
//! computer, for the sidebar's Cloud section. Nothing to configure: the folders are
//! looked up where each client keeps them.
//!
//! - Windows: the sync roots Explorer knows (registry `SyncRootManager`, covers
//!   OneDrive, Dropbox, iCloud, Box and other Cloud Files clients), the OneDrive
//!   environment variables, Dropbox's `info.json`, and Google Drive's virtual drive.
//! - macOS: `~/Library/CloudStorage/*` and iCloud Drive.
//! - Linux: well-known folders in the home folder and GNOME (gvfs) online accounts.

use serde::Serialize;
use std::path::{Path, PathBuf};

use super::drives::Drive;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudFolder {
    /// "onedrive", "dropbox", "gdrive", "icloud", "box", "mega", "pcloud",
    /// "nextcloud", "proton" or "other".
    pub provider: String,
    /// Display name, e.g. "OneDrive - Contoso" or "Google Drive".
    pub name: String,
    pub path: String,
}

/// Cloud folders on this computer, without duplicates. `drives` is the current drive
/// list (Google Drive for desktop appears as a drive labelled "Google Drive").
pub fn detect(drives: &[Drive]) -> Vec<CloudFolder> {
    let mut found = Vec::new();
    let home = dirs::home_dir().unwrap_or_default();
    platform_folders(&home, &mut found);
    for d in drives {
        if d.label.eq_ignore_ascii_case("Google Drive") {
            let my = Path::new(&d.mount).join("My Drive");
            let path = if my.is_dir() {
                my
            } else {
                PathBuf::from(&d.mount)
            };
            push(&mut found, "gdrive", "Google Drive", path);
        }
    }
    // Well-known folder names in the home folder (Linux relies on these). Empty folders
    // are skipped: clients leave them behind when an account is unlinked. On Windows the
    // big clients are found reliably above, so a plain "OneDrive" folder there is a leftover.
    if let Ok(rd) = std::fs::read_dir(&home) {
        let mut names: Vec<_> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for n in names {
            let Some((provider, label)) = classify_home_folder(&n) else {
                continue;
            };
            if cfg!(windows) && REGISTERED_ON_WINDOWS.contains(&provider) {
                continue;
            }
            let path = home.join(&n);
            if is_empty_dir(&path) {
                continue;
            }
            push(&mut found, provider, &label, path);
        }
    }
    dedupe(found)
}

/// Providers whose Windows clients register sync roots, environment variables or
/// info files, so their folders never need guessing from names.
const REGISTERED_ON_WINDOWS: &[&str] = &["onedrive", "dropbox", "icloud", "box", "gdrive"];

fn is_empty_dir(p: &Path) -> bool {
    std::fs::read_dir(p)
        .map(|mut rd| rd.next().is_none())
        .unwrap_or(true)
}

/// Adds a folder if it exists.
fn push(out: &mut Vec<CloudFolder>, provider: &str, name: &str, path: PathBuf) {
    if path.is_dir() {
        out.push(CloudFolder {
            provider: provider.into(),
            name: name.into(),
            path: path.to_string_lossy().into_owned(),
        });
    }
}

/// Keeps the first entry per folder (sources are added best-first).
fn dedupe(found: Vec<CloudFolder>) -> Vec<CloudFolder> {
    let key = |p: &str| {
        let p = p.trim_end_matches(['/', '\\']);
        if cfg!(windows) {
            p.to_lowercase()
        } else {
            p.to_string()
        }
    };
    let mut out: Vec<CloudFolder> = Vec::new();
    for f in found {
        if !out.iter().any(|o| key(&o.path) == key(&f.path)) {
            out.push(f);
        }
    }
    out
}

/// Provider and display name for a folder directly in the home folder.
pub fn classify_home_folder(name: &str) -> Option<(&'static str, String)> {
    let n = name.to_lowercase();
    let p = match n.as_str() {
        "dropbox" => "dropbox",
        "onedrive" => "onedrive",
        "google drive" | "googledrive" => "gdrive",
        "icloud drive" | "iclouddrive" => return Some(("icloud", "iCloud Drive".into())),
        "box" => "box",
        "mega" | "megasync" => "mega",
        "pclouddrive" | "pcloud drive" => "pcloud",
        "nextcloud" | "owncloud" => "nextcloud",
        "proton drive" => "proton",
        _ if n.starts_with("onedrive - ") => "onedrive",
        _ if n.starts_with("dropbox (") => "dropbox",
        _ => return None,
    };
    Some((p, name.to_string()))
}

/// Provider for a Windows sync-root id such as `OneDrive!S-1-5-…!Business1|…`.
pub fn sync_root_provider(id: &str) -> &'static str {
    let vendor = id.split('!').next().unwrap_or("").to_lowercase();
    match vendor.as_str() {
        v if v.starts_with("onedrive") => "onedrive",
        v if v.starts_with("dropbox") => "dropbox",
        v if v.contains("icloud") => "icloud",
        v if v.starts_with("googledrive") || v.starts_with("drivefs") => "gdrive",
        v if v.starts_with("box") => "box",
        v if v.starts_with("mega") => "mega",
        v if v.starts_with("pcloud") => "pcloud",
        v if v.starts_with("nextcloud") || v.starts_with("owncloud") => "nextcloud",
        v if v.starts_with("proton") => "proton",
        _ => "other",
    }
}

/// macOS `~/Library/CloudStorage` entry such as `OneDrive-Contoso` or
/// `GoogleDrive-me@gmail.com`: provider and display name.
pub fn classify_cloud_storage(entry: &str) -> (&'static str, String) {
    let (vendor, account) = entry.split_once('-').unwrap_or((entry, ""));
    let (provider, label) = match vendor.to_lowercase().as_str() {
        "onedrive" => ("onedrive", "OneDrive"),
        "dropbox" => ("dropbox", "Dropbox"),
        "googledrive" => ("gdrive", "Google Drive"),
        "box" => ("box", "Box"),
        "protondrive" => ("proton", "Proton Drive"),
        "pcloud" => ("pcloud", "pCloud"),
        "nextcloud" => ("nextcloud", "Nextcloud"),
        _ => ("other", vendor),
    };
    let name = match account {
        "" | "Personal" | "Box" => label.to_string(),
        a => format!("{label} - {a}"),
    };
    (provider, name)
}

/// GNOME online-account mount (`$XDG_RUNTIME_DIR/gvfs/<entry>`) such as
/// `google-drive:host=gmail.com,user=me`.
pub fn classify_gvfs(entry: &str) -> Option<(&'static str, String)> {
    let (scheme, rest) = entry.split_once(':')?;
    let (provider, label) = match scheme {
        "google-drive" => ("gdrive", "Google Drive"),
        "onedrive" => ("onedrive", "OneDrive"),
        _ => return None,
    };
    let user = rest.split(',').find_map(|kv| kv.strip_prefix("user="));
    let host = rest.split(',').find_map(|kv| kv.strip_prefix("host="));
    Some(match (user, host) {
        (Some(u), Some(h)) => (provider, format!("{label} ({u}@{h})")),
        (Some(u), None) => (provider, format!("{label} ({u})")),
        _ => (provider, label.to_string()),
    })
}

#[cfg(windows)]
fn platform_folders(home: &Path, out: &mut Vec<CloudFolder>) {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    // 1. Sync roots registered with Explorer: name, provider and path per account.
    let roots = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\SyncRootManager",
        KEY_READ,
    );
    if let Ok(roots) = roots {
        // Only this user's roots: other accounts' folders sit in other profiles.
        let users = home.parent().map(Path::to_path_buf);
        for id in roots.enum_keys().flatten() {
            let Ok(key) = roots.open_subkey_with_flags(&id, KEY_READ) else {
                continue;
            };
            let label: String = key.get_value("DisplayNameResource").unwrap_or_default();
            let Ok(user_roots) = key.open_subkey_with_flags("UserSyncRoots", KEY_READ) else {
                continue;
            };
            for (_, value) in user_roots.enum_values().flatten() {
                let path = PathBuf::from(value.to_string().trim_matches('"'));
                let other_profile =
                    users.as_ref().is_some_and(|u| path.starts_with(u)) && !path.starts_with(home);
                if other_profile {
                    continue;
                }
                // Resource references ("@dll,-123") are not readable names; use the folder's.
                let name = if label.is_empty() || label.starts_with('@') {
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                } else {
                    label.clone()
                };
                let name = if name.eq_ignore_ascii_case("iCloudDrive") {
                    "iCloud Drive".into()
                } else {
                    name
                };
                push(out, sync_root_provider(&id), &name, path);
            }
        }
    }
    // 2. OneDrive sets these even when the sync-root key is missing.
    for var in ["OneDriveCommercial", "OneDriveConsumer", "OneDrive"] {
        if let Some(p) = std::env::var_os(var) {
            let p = PathBuf::from(p);
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "OneDrive".into());
            push(out, "onedrive", &name, p);
        }
    }
    // 3. Dropbox writes its folders to info.json.
    for base in [dirs::config_dir(), dirs::data_local_dir()]
        .into_iter()
        .flatten()
    {
        dropbox_info(&base.join("Dropbox").join("info.json"), out);
    }
}

#[cfg(target_os = "macos")]
fn platform_folders(home: &Path, out: &mut Vec<CloudFolder>) {
    push(
        out,
        "icloud",
        "iCloud Drive",
        home.join("Library/Mobile Documents/com~apple~CloudDocs"),
    );
    if let Ok(rd) = std::fs::read_dir(home.join("Library/CloudStorage")) {
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let (provider, name) = classify_cloud_storage(&e.file_name().to_string_lossy());
            push(out, provider, &name, e.path());
        }
    }
    dropbox_info(&home.join(".dropbox/info.json"), out);
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_folders(home: &Path, out: &mut Vec<CloudFolder>) {
    dropbox_info(&home.join(".dropbox/info.json"), out);
    // GNOME online accounts mounted by gvfs.
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    if let Some(Ok(rd)) = runtime.map(|r| std::fs::read_dir(r.join("gvfs"))) {
        for e in rd.flatten() {
            if let Some((provider, name)) = classify_gvfs(&e.file_name().to_string_lossy()) {
                push(out, provider, &name, e.path());
            }
        }
    }
}

/// Dropbox's `info.json`: `{"personal": {"path": …}, "business": {"path": …}}`.
fn dropbox_info(file: &Path, out: &mut Vec<CloudFolder>) {
    let Some(v) = std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return;
    };
    for (kind, label) in [("personal", "Dropbox"), ("business", "Dropbox (Business)")] {
        if let Some(p) = v
            .get(kind)
            .and_then(|a| a.get("path"))
            .and_then(|p| p.as_str())
        {
            push(out, "dropbox", label, PathBuf::from(p));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_folder_names() {
        assert_eq!(
            classify_home_folder("Dropbox"),
            Some(("dropbox", "Dropbox".into()))
        );
        assert_eq!(
            classify_home_folder("OneDrive - Contoso"),
            Some(("onedrive", "OneDrive - Contoso".into()))
        );
        assert_eq!(
            classify_home_folder("iCloudDrive"),
            Some(("icloud", "iCloud Drive".into()))
        );
        assert_eq!(classify_home_folder("Documents"), None);
        assert_eq!(classify_home_folder("Boxes"), None);
    }

    #[test]
    fn windows_sync_root_ids() {
        assert_eq!(
            sync_root_provider("OneDrive!S-1-5-21-1!Business1|abc"),
            "onedrive"
        );
        assert_eq!(
            sync_root_provider("Dropbox!S-1-5-21-1!Personal|1"),
            "dropbox"
        );
        assert_eq!(
            sync_root_provider("iCloudDrive!S-1-5-21-1!Account"),
            "icloud"
        );
        assert_eq!(sync_root_provider("SomethingElse!S-1"), "other");
    }

    #[test]
    fn macos_cloud_storage_names() {
        assert_eq!(
            classify_cloud_storage("OneDrive-Personal"),
            ("onedrive", "OneDrive".into())
        );
        assert_eq!(
            classify_cloud_storage("OneDrive-Contoso"),
            ("onedrive", "OneDrive - Contoso".into())
        );
        assert_eq!(
            classify_cloud_storage("GoogleDrive-me@gmail.com"),
            ("gdrive", "Google Drive - me@gmail.com".into())
        );
        assert_eq!(
            classify_cloud_storage("Dropbox"),
            ("dropbox", "Dropbox".into())
        );
    }

    #[test]
    fn gvfs_mounts() {
        assert_eq!(
            classify_gvfs("google-drive:host=gmail.com,user=me"),
            Some(("gdrive", "Google Drive (me@gmail.com)".into()))
        );
        assert_eq!(classify_gvfs("sftp:host=example.org"), None);
    }

    #[test]
    fn duplicates_are_dropped_first_wins() {
        let f = |p: &str, n: &str| CloudFolder {
            provider: "onedrive".into(),
            name: n.into(),
            path: p.into(),
        };
        let out = dedupe(vec![f("/h/OneDrive", "registry"), f("/h/OneDrive/", "env")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "registry");
    }

    #[test]
    fn dropbox_info_json() {
        let t = tempfile::tempdir().unwrap();
        let folder = t.path().join("Dropbox");
        std::fs::create_dir(&folder).unwrap();
        let info = t.path().join("info.json");
        let json = serde_json::json!({ "personal": { "path": folder.to_string_lossy() } });
        std::fs::write(&info, json.to_string()).unwrap();
        let mut out = vec![];
        dropbox_info(&info, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Dropbox");
    }
}
