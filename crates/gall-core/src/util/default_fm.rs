//! Set and restore Gall as the operating system's default file manager.
//!
//! Nothing here runs at startup: detection and updates happen only when the user opens the
//! Welcome setup or *Settings › Open with*.
//!
//! Platform mechanisms:
//! - **Linux**: Freedesktop `inode/directory` default in `$XDG_CONFIG_HOME/mimeapps.list` (and any
//!   existing desktop-specific `$XDG_CONFIG_HOME/<de>-mimeapps.list` that overrides
//!   `inode/directory`). When Gall is not installed via a package that provides `gall.desktop`, a
//!   user-local `~/.local/share/applications/gall.desktop` pointing to the stable executable is
//!   written first. Inside Flatpak, uses `{FLATPAK_ID}.desktop` without writing a local desktop
//!   file.
//! - **Windows**: per-user `HKCU\Software\Classes\Directory\shell` and `Drive\shell` default verb
//!   `org.gall.app.open` (no administrator rights needed). Real filesystem folders and drives open
//!   in Gall while virtual Shell namespaces (Control Panel, Recycle Bin, This PC) stay in File
//!   Explorer. Notifies the Shell via `SHChangeNotify`.
//! - **macOS**: user global preference `NSFileViewer` (`defaults write -g NSFileViewer -string
//!   org.gall.app`) plus best-effort LaunchServices `public.folder` role handler. Requires an
//!   installed `Gall.app` bundle with identifier `org.gall.app` so bare developer binaries cannot
//!   break *Reveal in Finder*.
//!
//! Before modifying the OS setting, Gall stores the previous value in
//! `<data_dir>/default-file-manager.json` so *Restore* cleanly reverts to what was active before.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// Backup file stored inside Gall's data directory.
const BACKUP_FILE_NAME: &str = "default-file-manager.json";

/// MIME type for directories on Freedesktop/Linux.
pub const LINUX_DIRECTORY_MIME: &str = "inode/directory";
/// Standard desktop file id shipped by Linux packages.
pub const LINUX_GALL_DESKTOP_ID: &str = "gall.desktop";
/// Alternate reverse-DNS desktop id.
pub const LINUX_GALL_RDNS_DESKTOP_ID: &str = "org.gall.app.desktop";
/// Flatpak desktop id (matches `packaging/flatpak/io.github.gama87.Gall.yml`).
pub const LINUX_GALL_FLATPAK_DESKTOP_ID: &str = "io.github.gama87.Gall.desktop";

/// Windows HKCU verb registered under `Directory\shell` and `Drive\shell`.
pub const WINDOWS_VERB_NAME: &str = "org.gall.app.open";
/// Display label of the Windows shell verb.
pub const WINDOWS_VERB_LABEL: &str = "Open in Gall";
#[cfg(windows)]
const WINDOWS_PREVIOUS_VERB_SAVED: &str = "GallPreviousDefaultVerbSaved";
#[cfg(windows)]
const WINDOWS_PREVIOUS_VERB: &str = "GallPreviousDefaultVerb";

/// macOS bundle identifier (`src-tauri/tauri.conf.json`).
pub const MACOS_BUNDLE_ID: &str = "org.gall.app";
/// Apple Finder bundle identifier.
pub const MACOS_FINDER_BUNDLE_ID: &str = "com.apple.finder";

/// Machine-readable reason why setting Gall as the default file manager is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnavailableReason {
    /// macOS: Gall is not running from (nor installed as) a `.app` bundle with `org.gall.app`.
    MacosAppBundleRequired,
    /// Current executable path could not be resolved.
    ExecutableNotFound,
    /// Current operating system is not supported.
    UnsupportedPlatform,
}

/// Snapshot of the operating system's current default file manager state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultFileManagerStatus {
    /// `"linux"`, `"windows"`, or `"macos"`.
    pub platform: String,
    /// True when Gall is currently configured as the system's default folder handler.
    pub is_default: bool,
    /// Human-readable name of the current default handler (e.g. `"Gall"`, `"Finder"`,
    /// `"File Explorer"`, `"Dolphin"`), or `None` when unset/unknown.
    pub current_handler: Option<String>,
    /// Underlying identifier of the current default handler when known.
    pub current_id: Option<String>,
    /// True when Gall can set itself as the default in this environment.
    pub can_set: bool,
    /// Reason why `can_set` is false.
    pub unavailable_reason: Option<UnavailableReason>,
    /// True when Gall is currently the default and can restore the previous/system default.
    pub can_restore: bool,
    /// Human-readable name of what *Restore* will revert to (e.g. `"Finder"`, `"File Explorer"`,
    /// `"Dolphin"`), or `None` for the generic system default.
    pub restore_target: Option<String>,
}

/// Persisted backup of the previous OS setting before Gall became default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    linux: Option<LinuxBackup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    windows: Option<WindowsBackup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    macos: Option<MacosBackup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinuxMimeappsEntryBackup {
    path: PathBuf,
    previous: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinuxBackup {
    entries: Vec<LinuxMimeappsEntryBackup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_display_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsBackup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directory_prev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    drive_prev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_display_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MacosBackup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_folder_bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_display_name: Option<String>,
}

#[cfg(any(test, target_os = "macos"))]
impl MacosBackup {
    fn previous_folder_handler(&self) -> &str {
        self.previous_folder_bundle_id
            .as_deref()
            .or(self.previous_bundle_id.as_deref())
            .filter(|id| !id.eq_ignore_ascii_case(MACOS_BUNDLE_ID))
            .unwrap_or(MACOS_FINDER_BUNDLE_ID)
    }
}

fn backup_path(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUP_FILE_NAME)
}

fn read_backup(data_dir: &Path) -> BackupState {
    let path = backup_path(data_dir);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_backup(data_dir: &Path, state: &BackupState) -> Result<()> {
    std::fs::create_dir_all(data_dir).map_err(|e| {
        CoreError::Other(format!(
            "cannot create data directory {}: {e}",
            data_dir.display()
        ))
    })?;
    let path = backup_path(data_dir);
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| CoreError::Other(format!("cannot serialize backup state: {e}")))?;
    let tmp = path.with_extension("json.gall-tmp");
    if std::fs::write(&tmp, &json).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&path, json)
        .map_err(|e| CoreError::Other(format!("cannot write {}: {e}", path.display())))
}

fn remove_backup(data_dir: &Path) {
    let _ = std::fs::remove_file(backup_path(data_dir));
}

/// Resolves the stable Gall executable path, preferring the base launcher when running under an
/// updater overlay (`GALL_BASE_EXE`) or an AppImage (`APPIMAGE`).
pub fn stable_exe_path() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os(crate::updater::BASE_EXE_ENV) {
        let p = PathBuf::from(base);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    #[cfg(target_os = "linux")]
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        let p = PathBuf::from(appimage);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    std::env::current_exe().ok()
}

/// Queries the current operating system's default file manager status.
pub fn status(data_dir: &Path) -> DefaultFileManagerStatus {
    #[cfg(target_os = "linux")]
    {
        let env = LinuxEnv::from_host();
        linux_status_in(&env, data_dir)
    }
    #[cfg(windows)]
    {
        windows_impl::status(data_dir)
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::status(data_dir)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = data_dir;
        DefaultFileManagerStatus {
            platform: std::env::consts::OS.to_string(),
            is_default: false,
            current_handler: None,
            current_id: None,
            can_set: false,
            unavailable_reason: Some(UnavailableReason::UnsupportedPlatform),
            can_restore: false,
            restore_target: None,
        }
    }
}

/// Sets Gall as the operating system's default file manager for the current user and returns the
/// refreshed status.
pub fn set_default(data_dir: &Path) -> Result<DefaultFileManagerStatus> {
    #[cfg(target_os = "linux")]
    {
        let env = LinuxEnv::from_host();
        linux_set_default_in(&env, data_dir)
    }
    #[cfg(windows)]
    {
        windows_impl::set_default(data_dir)
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::set_default(data_dir)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = data_dir;
        Err(CoreError::Other(
            "setting the default file manager is not supported on this platform".into(),
        ))
    }
}

/// Restores the previous (or platform built-in) default file manager and returns the refreshed
/// status.
pub fn restore_default(data_dir: &Path) -> Result<DefaultFileManagerStatus> {
    #[cfg(target_os = "linux")]
    {
        let env = LinuxEnv::from_host();
        linux_restore_default_in(&env, data_dir)
    }
    #[cfg(windows)]
    {
        windows_impl::restore_default(data_dir)
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::restore_default(data_dir)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = data_dir;
        Err(CoreError::Other(
            "restoring the default file manager is not supported on this platform".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Linux / Freedesktop implementation (pure testable core + host adapter)
// ---------------------------------------------------------------------------

/// Freedesktop directory and desktop environment configuration used by Linux default-FM logic.
#[derive(Debug, Clone)]
pub struct LinuxEnv {
    /// `$XDG_CONFIG_HOME` (or `~/.config`).
    pub config_home: PathBuf,
    /// Additional system config dirs (`$XDG_CONFIG_DIRS`, default `/etc/xdg`).
    pub config_dirs: Vec<PathBuf>,
    /// `$XDG_DATA_HOME` (or `~/.local/share`).
    pub data_home: PathBuf,
    /// Additional system data dirs (`$XDG_DATA_DIRS`, default `/usr/local/share`, `/usr/share`).
    pub data_dirs: Vec<PathBuf>,
    /// Lowercased desktops from `XDG_CURRENT_DESKTOP`.
    pub desktops: Vec<String>,
    /// True when running inside a Flatpak sandbox.
    pub in_flatpak: bool,
    /// Flatpak app id when `in_flatpak` is true (e.g. `io.github.gama87.Gall`).
    pub flatpak_id: Option<String>,
    /// Stable Gall executable path used if we need to write `~/.local/share/applications/gall.desktop`.
    pub exe_path: Option<PathBuf>,
    /// Whether to invoke `update-desktop-database` after creating a local `.desktop` file.
    pub refresh_desktop_db: bool,
}

impl LinuxEnv {
    /// Builds [`LinuxEnv`] from the current host environment.
    pub fn from_host() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let config_dirs = split_colon_paths(
            std::env::var_os("XDG_CONFIG_DIRS"),
            &[PathBuf::from("/etc/xdg")],
        );
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let mut data_dirs = split_colon_paths(
            std::env::var_os("XDG_DATA_DIRS"),
            &[
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ],
        );
        for extra in [
            data_home.join("flatpak/exports/share"),
            PathBuf::from("/var/lib/flatpak/exports/share"),
        ] {
            if !data_dirs.contains(&extra) {
                data_dirs.push(extra);
            }
        }
        let desktops = parse_xdg_desktops(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref());
        let in_flatpak = crate::util::hostspawn::in_flatpak();
        let flatpak_id = std::env::var("FLATPAK_ID")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Self {
            config_home,
            config_dirs,
            data_home,
            data_dirs,
            desktops,
            in_flatpak,
            flatpak_id,
            exe_path: stable_exe_path(),
            refresh_desktop_db: true,
        }
    }

    fn all_config_dirs(&self) -> Vec<PathBuf> {
        let mut out = vec![self.config_home.clone()];
        for d in &self.config_dirs {
            if !out.contains(d) {
                out.push(d.clone());
            }
        }
        out
    }

    fn all_data_dirs(&self) -> Vec<PathBuf> {
        let mut out = vec![self.data_home.clone()];
        for d in &self.data_dirs {
            if !out.contains(d) {
                out.push(d.clone());
            }
        }
        out
    }

    /// Expected `.desktop` id for this Gall installation.
    pub fn gall_desktop_id(&self) -> String {
        if self.in_flatpak {
            let id = self
                .flatpak_id
                .as_deref()
                .unwrap_or("io.github.gama87.Gall");
            return format!("{id}.desktop");
        }
        if self.find_desktop_file(LINUX_GALL_DESKTOP_ID).is_some() {
            return LINUX_GALL_DESKTOP_ID.to_string();
        }
        if self.find_desktop_file(LINUX_GALL_RDNS_DESKTOP_ID).is_some() {
            return LINUX_GALL_RDNS_DESKTOP_ID.to_string();
        }
        LINUX_GALL_DESKTOP_ID.to_string()
    }

    /// Locates `<desktop_id>` across XDG `applications/` directories in precedence order.
    pub fn find_desktop_file(&self, desktop_id: &str) -> Option<PathBuf> {
        if desktop_id.is_empty() || desktop_id.contains(['/', '\\', '\0']) {
            return None;
        }
        for d in self.all_data_dirs() {
            let apps = d.join("applications");
            let direct = apps.join(desktop_id);
            if direct.is_file() {
                return Some(direct);
            }
            // Freedesktop vendor-prefix rule: `foo-bar.desktop` can live at `foo/bar.desktop`.
            if let Some((vendor, rest)) = desktop_id.split_once('-') {
                let nested = apps.join(vendor).join(rest);
                if nested.is_file() {
                    return Some(nested);
                }
            }
        }
        None
    }

    /// `mimeapps.list` paths in Freedesktop MIME Apps 1.0.1 precedence order.
    fn mimeapps_search_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for c in self.all_config_dirs() {
            for de in &self.desktops {
                out.push(c.join(format!("{de}-mimeapps.list")));
            }
            out.push(c.join("mimeapps.list"));
        }
        for d in self.all_data_dirs() {
            let apps = d.join("applications");
            for de in &self.desktops {
                out.push(apps.join(format!("{de}-mimeapps.list")));
            }
            out.push(apps.join("mimeapps.list"));
        }
        out
    }

    /// Resolves the current default desktop id for `inode/directory`.
    pub fn current_directory_desktop_id(&self) -> Option<String> {
        for path in self.mimeapps_search_paths() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Some(id) = read_default_from_mimeapps_text(&text, LINUX_DIRECTORY_MIME) {
                    return Some(id);
                }
            }
        }
        None
    }

    /// Human-readable display name for `desktop_id`.
    pub fn display_name_for_desktop_id(&self, desktop_id: &str) -> String {
        if let Some(file) = self.find_desktop_file(desktop_id) {
            if let Ok(text) = std::fs::read_to_string(file) {
                if let Some(name) = desktop_display_name_from_text(&text) {
                    return name;
                }
            }
        }
        fallback_desktop_name(desktop_id)
    }
}

fn split_colon_paths(raw: Option<std::ffi::OsString>, fallback: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(val) = raw {
        for part in val.to_string_lossy().split(':') {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                let p = PathBuf::from(trimmed);
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    if out.is_empty() {
        out.extend(fallback.iter().cloned());
    }
    out
}

/// Parses `XDG_CURRENT_DESKTOP` (colon-separated) into lowercased desktop identifiers.
pub fn parse_xdg_desktops(raw: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let Some(val) = raw else { return out };
    for part in val.split(':') {
        let lower = part.trim().to_ascii_lowercase();
        if !lower.is_empty() && !lower.contains(['/', '\\', '\0']) && !out.contains(&lower) {
            out.push(lower);
        }
    }
    out
}

/// True when `desktop_id` identifies Gall.
pub fn is_gall_desktop_id(desktop_id: &str, expected: &str) -> bool {
    let id = desktop_id.trim();
    id.eq_ignore_ascii_case(expected)
        || id.eq_ignore_ascii_case(LINUX_GALL_DESKTOP_ID)
        || id.eq_ignore_ascii_case(LINUX_GALL_RDNS_DESKTOP_ID)
        || id.eq_ignore_ascii_case(LINUX_GALL_FLATPAK_DESKTOP_ID)
}

/// Extracts the complete desktop-id list for `mime` under `[Default Applications]`.
pub fn read_default_associations_from_mimeapps_text(text: &str, mime: &str) -> Option<String> {
    let mut in_default = false;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_default = trimmed == "[Default Applications]";
            continue;
        }
        if !in_default {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            if k.trim() == mime {
                let value = v.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn first_mimeapps_desktop_id(value: &str) -> Option<&str> {
    value.split(';').map(str::trim).find(|s| !s.is_empty())
}

fn first_non_gall_mimeapps_desktop_id<'a>(value: &'a str, expected: &str) -> Option<&'a str> {
    value
        .split(';')
        .map(str::trim)
        .find(|id| !id.is_empty() && !is_gall_desktop_id(id, expected))
}

fn has_non_gall_mimeapps_association(associations: &str, expected: &str) -> bool {
    first_non_gall_mimeapps_desktop_id(associations, expected).is_some()
}

/// Extracts the first desktop id listed for `mime` under `[Default Applications]`.
pub fn read_default_from_mimeapps_text(text: &str, mime: &str) -> Option<String> {
    read_default_associations_from_mimeapps_text(text, mime)
        .and_then(|value| first_mimeapps_desktop_id(&value).map(str::to_string))
}

fn replace_default_associations_in_mimeapps_text(
    text: &str,
    mime: &str,
    associations: &str,
) -> String {
    let mut lines = Vec::new();
    let mut group = String::new();
    let mut found_default_section = false;
    let mut replaced_default = false;
    let value = associations.trim();
    let new_entry = if value.ends_with(';') {
        format!("{mime}={value}")
    } else {
        format!("{mime}={value};")
    };

    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if group == "[Default Applications]" && !replaced_default {
                lines.push(new_entry.clone());
                replaced_default = true;
            }
            group = trimmed.to_string();
            found_default_section |= group == "[Default Applications]";
            lines.push(raw.to_string());
            continue;
        }
        if group == "[Default Applications]" {
            if let Some((key, _)) = trimmed.split_once('=') {
                if key.trim() == mime {
                    if !replaced_default {
                        lines.push(new_entry.clone());
                        replaced_default = true;
                    }
                    continue;
                }
            }
        }
        lines.push(raw.to_string());
    }

    if !replaced_default {
        if !found_default_section {
            if !lines.is_empty() && !lines.last().is_some_and(|line| line.is_empty()) {
                lines.push(String::new());
            }
            lines.push("[Default Applications]".into());
        }
        lines.push(new_entry);
    }

    let mut out = lines.join("\n");
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Updates `mime=desktop_id;` under `[Default Applications]` and clears `desktop_id` from
/// `[Removed Associations]`, preserving all other sections and comments.
pub fn update_mimeapps_text(text: &str, mime: &str, desktop_id: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut group = String::new();
    let mut found_default_section = false;
    let mut replaced_default = false;
    let new_entry = format!("{mime}={desktop_id};");

    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if group == "[Default Applications]" && !replaced_default {
                lines.push(new_entry.clone());
                replaced_default = true;
            }
            group = trimmed.to_string();
            if group == "[Default Applications]" {
                found_default_section = true;
            }
            lines.push(raw.to_string());
            continue;
        }
        if group == "[Default Applications]" {
            if let Some((k, _)) = trimmed.split_once('=') {
                if k.trim() == mime {
                    if !replaced_default {
                        lines.push(new_entry.clone());
                        replaced_default = true;
                    }
                    continue;
                }
            }
        } else if group == "[Removed Associations]" {
            if let Some((k, v)) = trimmed.split_once('=') {
                if k.trim() == mime {
                    let kept: Vec<&str> = v
                        .split(';')
                        .map(str::trim)
                        .filter(|s| !s.is_empty() && *s != desktop_id)
                        .collect();
                    if !kept.is_empty() {
                        lines.push(format!("{mime}={};", kept.join(";")));
                    }
                    continue;
                }
            }
        }
        lines.push(raw.to_string());
    }

    if !replaced_default {
        if !found_default_section {
            if !lines.is_empty() && !lines.last().is_some_and(|l| l.is_empty()) {
                lines.push(String::new());
            }
            lines.push("[Default Applications]".into());
        }
        lines.push(new_entry);
    }

    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Removes `mime=...` from `[Default Applications]`, preserving all other lines and sections.
pub fn remove_default_from_mimeapps_text(text: &str, mime: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut group = String::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            group = trimmed.to_string();
            lines.push(raw.to_string());
            continue;
        }
        if group == "[Default Applications]" {
            if let Some((k, _)) = trimmed.split_once('=') {
                if k.trim() == mime {
                    continue;
                }
            }
        }
        lines.push(raw.to_string());
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Parses the unlocalized `Name=` key under `[Desktop Entry]`.
pub fn desktop_display_name_from_text(text: &str) -> Option<String> {
    let mut in_entry = false;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_entry = trimmed == "[Desktop Entry]";
            continue;
        }
        if in_entry {
            if let Some((k, v)) = trimmed.split_once('=') {
                if k.trim() == "Name" {
                    let name = v.trim();
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Best-effort human-readable fallback from a `.desktop` id (e.g. `org.kde.dolphin.desktop` ->
/// `"dolphin"`).
pub fn fallback_desktop_name(desktop_id: &str) -> String {
    let stem = desktop_id.strip_suffix(".desktop").unwrap_or(desktop_id);
    stem.rsplit('.').next().unwrap_or(stem).to_string()
}

/// Escapes an executable path for a Freedesktop Desktop Entry `Exec=` value.
pub fn linux_escape_exec_arg(exe: &Path) -> String {
    let raw = exe.to_string_lossy();
    let needs_quote = raw.is_empty()
        || raw.chars().any(|c| {
            matches!(
                c,
                ' ' | '\t'
                    | '\n'
                    | '"'
                    | '\''
                    | '\\'
                    | '>'
                    | '<'
                    | '~'
                    | '|'
                    | '&'
                    | ';'
                    | '$'
                    | '*'
                    | '?'
                    | '#'
                    | '('
                    | ')'
                    | '`'
                    | '%'
            )
        });
    if !needs_quote {
        return raw.into_owned();
    }
    let mut escaped = String::with_capacity(raw.len() + 4);
    escaped.push('"');
    for ch in raw.chars() {
        match ch {
            '"' | '`' | '$' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            '%' => escaped.push_str("%%"),
            _ => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

/// Generates a minimal, valid Freedesktop `.desktop` entry pointing at `exe`.
pub fn linux_desktop_entry_content(exe: &Path) -> String {
    let exec = linux_escape_exec_arg(exe);
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Gall\n\
         Comment=Keyboard-first file manager with instant search and compare\n\
         Exec={exec} %U\n\
         Icon=gall\n\
         Terminal=false\n\
         Categories=System;FileTools;FileManager;\n\
         MimeType=inode/directory;\n\
         StartupWMClass=gall\n"
    )
}

fn write_file_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Other(format!("cannot create {}: {e}", parent.display())))?;
    }
    let tmp = path.with_extension("gall-tmp");
    if std::fs::write(&tmp, content).is_ok() && std::fs::rename(&tmp, path).is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(path, content)
        .map_err(|e| CoreError::Other(format!("cannot write {}: {e}", path.display())))
}

/// Evaluates Linux default-FM status against an explicit [`LinuxEnv`].
pub fn linux_status_in(env: &LinuxEnv, data_dir: &Path) -> DefaultFileManagerStatus {
    let expected = env.gall_desktop_id();
    let current_id = env.current_directory_desktop_id();
    let is_default = current_id
        .as_deref()
        .is_some_and(|id| is_gall_desktop_id(id, &expected));
    let current_handler = current_id.as_deref().map(|id| {
        if is_gall_desktop_id(id, &expected) {
            "Gall".to_string()
        } else {
            env.display_name_for_desktop_id(id)
        }
    });
    let has_desktop = env.in_flatpak || env.find_desktop_file(&expected).is_some();
    let can_set = has_desktop || env.exe_path.is_some();
    let unavailable_reason = if can_set {
        None
    } else {
        Some(UnavailableReason::ExecutableNotFound)
    };
    let backup = read_backup(data_dir).linux;
    let restore_target = if is_default {
        backup.and_then(|b| {
            b.previous_display_name.or_else(|| {
                b.entries
                    .iter()
                    .find_map(|e| {
                        e.previous.as_deref().and_then(|associations| {
                            first_non_gall_mimeapps_desktop_id(associations, &expected)
                        })
                    })
                    .map(|id| env.display_name_for_desktop_id(id))
            })
        })
    } else {
        None
    };
    DefaultFileManagerStatus {
        platform: "linux".to_string(),
        is_default,
        current_handler,
        current_id,
        can_set,
        unavailable_reason,
        can_restore: is_default,
        restore_target,
    }
}

/// Sets Gall as the default file manager on Linux against an explicit [`LinuxEnv`].
pub fn linux_set_default_in(env: &LinuxEnv, data_dir: &Path) -> Result<DefaultFileManagerStatus> {
    let expected = env.gall_desktop_id();
    let current_before = linux_status_in(env, data_dir);

    // Ensure a `.desktop` entry exists when running from an AppImage or unpacked binary.
    if !env.in_flatpak && env.find_desktop_file(&expected).is_none() {
        let exe = env
            .exe_path
            .as_deref()
            .ok_or_else(|| CoreError::Other("cannot determine Gall executable path".into()))?;
        let local_apps = env.data_home.join("applications");
        let desktop_path = local_apps.join(&expected);
        write_file_atomic(&desktop_path, &linux_desktop_entry_content(exe))?;
        if env.refresh_desktop_db {
            let _ = std::process::Command::new("update-desktop-database")
                .arg(&local_apps)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    // Collect user `mimeapps.list` files to update: always `~/.config/mimeapps.list`, plus any
    // existing desktop-specific `$XDG_CONFIG_HOME/<de>-mimeapps.list` that explicitly overrides
    // `inode/directory`.
    let primary_mimeapps = env.config_home.join("mimeapps.list");
    let mut targets = vec![primary_mimeapps];
    for de in &env.desktops {
        let candidate = env.config_home.join(format!("{de}-mimeapps.list"));
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            if read_default_from_mimeapps_text(&text, LINUX_DIRECTORY_MIME).is_some()
                && !targets.contains(&candidate)
            {
                targets.push(candidate);
            }
        }
    }

    let mut backup = read_backup(data_dir);
    if !current_before.is_default || backup.linux.is_none() {
        let mut entries = Vec::with_capacity(targets.len());
        for path in &targets {
            let prev = std::fs::read_to_string(path)
                .ok()
                .and_then(|t| {
                    read_default_associations_from_mimeapps_text(&t, LINUX_DIRECTORY_MIME)
                })
                .filter(|associations| has_non_gall_mimeapps_association(associations, &expected));
            entries.push(LinuxMimeappsEntryBackup {
                path: path.clone(),
                previous: prev,
            });
        }
        backup.linux = Some(LinuxBackup {
            entries,
            previous_display_name: if current_before.is_default {
                None
            } else {
                current_before.current_handler.clone()
            },
        });
        write_backup(data_dir, &backup)?;
    }

    for path in &targets {
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        let updated = update_mimeapps_text(&existing, LINUX_DIRECTORY_MIME, &expected);
        write_file_atomic(path, &updated)?;
    }

    Ok(linux_status_in(env, data_dir))
}

/// Restores the previous default file manager on Linux against an explicit [`LinuxEnv`].
pub fn linux_restore_default_in(
    env: &LinuxEnv,
    data_dir: &Path,
) -> Result<DefaultFileManagerStatus> {
    let expected = env.gall_desktop_id();
    let mut backup = read_backup(data_dir);
    let mut handled: Vec<PathBuf> = Vec::new();

    if let Some(linux_backup) = backup.linux.take() {
        for entry in linux_backup.entries {
            let existing = std::fs::read_to_string(&entry.path).unwrap_or_default();
            let updated = match entry.previous.as_deref() {
                Some(prev) if has_non_gall_mimeapps_association(prev, &expected) => {
                    let associations = if first_mimeapps_desktop_id(prev)
                        .is_some_and(|id| is_gall_desktop_id(id, &expected))
                    {
                        prev.split(';')
                            .map(str::trim)
                            .filter(|id| !id.is_empty() && !is_gall_desktop_id(id, &expected))
                            .collect::<Vec<_>>()
                            .join(";")
                    } else {
                        prev.to_string()
                    };
                    replace_default_associations_in_mimeapps_text(
                        &existing,
                        LINUX_DIRECTORY_MIME,
                        &associations,
                    )
                }
                _ => remove_default_from_mimeapps_text(&existing, LINUX_DIRECTORY_MIME),
            };
            if entry.path.exists() || !updated.is_empty() {
                write_file_atomic(&entry.path, &updated)?;
            }
            handled.push(entry.path);
        }
    }

    // Also clear any user mimeapps.list that still points to Gall (for instance if Gall was set
    // outside this helper or from a previous install without a backup file).
    let mut check_paths = vec![env.config_home.join("mimeapps.list")];
    for de in &env.desktops {
        check_paths.push(env.config_home.join(format!("{de}-mimeapps.list")));
    }
    for path in check_paths {
        if handled.contains(&path) {
            continue;
        }
        if let Ok(existing) = std::fs::read_to_string(&path) {
            if let Some(associations) =
                read_default_associations_from_mimeapps_text(&existing, LINUX_DIRECTORY_MIME)
            {
                if first_mimeapps_desktop_id(&associations)
                    .is_some_and(|id| is_gall_desktop_id(id, &expected))
                {
                    let remaining: Vec<&str> = associations
                        .split(';')
                        .map(str::trim)
                        .filter(|id| !id.is_empty() && !is_gall_desktop_id(id, &expected))
                        .collect();
                    let updated = if remaining.is_empty() {
                        remove_default_from_mimeapps_text(&existing, LINUX_DIRECTORY_MIME)
                    } else {
                        replace_default_associations_in_mimeapps_text(
                            &existing,
                            LINUX_DIRECTORY_MIME,
                            &remaining.join(";"),
                        )
                    };
                    write_file_atomic(&path, &updated)?;
                }
            }
        }
    }

    if backup.linux.is_none() && backup.windows.is_none() && backup.macos.is_none() {
        remove_backup(data_dir);
    } else {
        write_backup(data_dir, &backup)?;
    }

    Ok(linux_status_in(env, data_dir))
}

// ---------------------------------------------------------------------------
// Windows helpers (cross-platform pure helpers + Windows Registry adapter)
// ---------------------------------------------------------------------------

/// Formats the HKCU shell command string for opening a folder or drive root in Gall.
///
/// Uses `-- "%1\."` so names starting with `-` are never treated as options and drive roots such
/// as `C:\` expand to `"C:\\."` instead of `"C:\"` (which breaks `CommandLineToArgvW` by escaping
/// the closing quote). `cli::normalize_arg` strips the trailing `\.`.
pub fn format_windows_open_command(exe: &Path) -> String {
    format!("\"{}\" -- \"%1\\.\"", exe.display())
}

/// Formats the HKCU `Icon` value for the Gall shell verb.
pub fn format_windows_icon_value(exe: &Path) -> String {
    format!("\"{}\",0", exe.display())
}

/// Interprets a Windows `Directory\shell` default verb value into a display name.
pub fn windows_verb_display_name(default_verb: Option<&str>, verb_label: Option<&str>) -> String {
    match default_verb.map(str::trim).filter(|s| !s.is_empty()) {
        None => "File Explorer".to_string(),
        Some(v) if v.eq_ignore_ascii_case(WINDOWS_VERB_NAME) => "Gall".to_string(),
        Some(v)
            if v.eq_ignore_ascii_case("none")
                || v.eq_ignore_ascii_case("open")
                || v.eq_ignore_ascii_case("explore") =>
        {
            "File Explorer".to_string()
        }
        Some(other) => verb_label
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.replace('&', ""))
            .unwrap_or_else(|| other.to_string()),
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::ffi::c_void;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    const DIRECTORY_SHELL_SUBKEY: &str = r"Software\Classes\Directory\shell";
    const DRIVE_SHELL_SUBKEY: &str = r"Software\Classes\Drive\shell";

    const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
    const SHCNF_IDLIST: u32 = 0x0000;

    #[link(name = "shell32")]
    extern "system" {
        fn SHChangeNotify(
            w_event_id: i32,
            u_flags: u32,
            dw_item1: *const c_void,
            dw_item2: *const c_void,
        );
    }

    fn notify_shell_assoc_changed() {
        // SAFETY: standard Win32 Shell notification with null item pointers.
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED,
                SHCNF_IDLIST,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
    }

    fn read_default_verb(subkey: &str) -> Option<String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = hkcu.open_subkey_with_flags(subkey, KEY_READ).ok()?;
        let val: String = key.get_value("").ok()?;
        let trimmed = val.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    fn write_previous_default_verb(subkey: &str, previous: Option<&str>) -> Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (verb_key, _) = hkcu
            .create_subkey(format!(r"{subkey}\{WINDOWS_VERB_NAME}"))
            .map_err(|e| CoreError::Other(format!("cannot open Gall verb key: {e}")))?;
        if let Err(err) = verb_key.delete_value(WINDOWS_PREVIOUS_VERB_SAVED) {
            if err.kind() != std::io::ErrorKind::NotFound {
                return Err(CoreError::Other(format!(
                    "cannot update saved Windows default verb: {err}"
                )));
            }
        }
        match previous {
            Some(value) => verb_key
                .set_value(WINDOWS_PREVIOUS_VERB, &value)
                .map_err(|e| CoreError::Other(format!("cannot save previous Windows verb: {e}")))?,
            None => {
                if let Err(err) = verb_key.delete_value(WINDOWS_PREVIOUS_VERB) {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        return Err(CoreError::Other(format!(
                            "cannot clear previous Windows verb: {err}"
                        )));
                    }
                }
            }
        }
        verb_key
            .set_value(WINDOWS_PREVIOUS_VERB_SAVED, &"1")
            .map_err(|e| CoreError::Other(format!("cannot mark previous Windows verb saved: {e}")))
    }

    fn read_verb_label(subkey: &str, verb: &str) -> Option<String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = hkcu
            .open_subkey_with_flags(format!(r"{subkey}\{verb}"), KEY_READ)
            .ok()?;
        let val: String = key.get_value("").ok()?;
        let trimmed = val.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    fn has_gall_command(subkey: &str) -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(key) =
            hkcu.open_subkey_with_flags(format!(r"{subkey}\{WINDOWS_VERB_NAME}\command"), KEY_READ)
        else {
            return false;
        };
        key.get_value::<String, _>("")
            .ok()
            .is_some_and(|s| !s.trim().is_empty())
    }

    fn write_gall_verb(subkey: &str, exe: &Path) -> Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (shell_key, _) = hkcu
            .create_subkey(subkey)
            .map_err(|e| CoreError::Other(format!("cannot open HKCU\\{subkey}: {e}")))?;
        let (verb_key, _) = shell_key.create_subkey(WINDOWS_VERB_NAME).map_err(|e| {
            CoreError::Other(format!(
                "cannot create HKCU\\{subkey}\\{WINDOWS_VERB_NAME}: {e}"
            ))
        })?;
        verb_key
            .set_value("", &WINDOWS_VERB_LABEL)
            .map_err(|e| CoreError::Other(format!("cannot set verb label: {e}")))?;
        verb_key
            .set_value("Icon", &format_windows_icon_value(exe))
            .map_err(|e| CoreError::Other(format!("cannot set verb icon: {e}")))?;
        let (cmd_key, _) = verb_key
            .create_subkey("command")
            .map_err(|e| CoreError::Other(format!("cannot create command key: {e}")))?;
        cmd_key
            .set_value("", &format_windows_open_command(exe))
            .map_err(|e| CoreError::Other(format!("cannot set command value: {e}")))?;
        shell_key
            .set_value("", &WINDOWS_VERB_NAME)
            .map_err(|e| CoreError::Other(format!("cannot set default verb: {e}")))?;
        Ok(())
    }

    fn restore_shell_key(subkey: &str, previous: Option<&str>) -> Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(shell_key) = hkcu.open_subkey_with_flags(subkey, KEY_READ | KEY_WRITE) else {
            return Ok(());
        };
        let current = shell_key.get_value::<String, _>("").ok();
        if current
            .as_deref()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case(WINDOWS_VERB_NAME))
        {
            match previous
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(WINDOWS_VERB_NAME))
            {
                Some(prev) => {
                    shell_key.set_value("", &prev).map_err(|e| {
                        CoreError::Other(format!("cannot restore HKCU\\{subkey}: {e}"))
                    })?;
                }
                None => {
                    if let Err(err) = shell_key.delete_value("") {
                        if err.kind() != std::io::ErrorKind::NotFound {
                            return Err(CoreError::Other(format!(
                                "cannot clear HKCU\\{subkey}: {err}"
                            )));
                        }
                    }
                }
            }
        }
        let _ = shell_key.delete_subkey_all(WINDOWS_VERB_NAME);
        Ok(())
    }

    pub fn status(data_dir: &Path) -> DefaultFileManagerStatus {
        let dir_verb = read_default_verb(DIRECTORY_SHELL_SUBKEY);
        let is_default = dir_verb
            .as_deref()
            .is_some_and(|v| v.eq_ignore_ascii_case(WINDOWS_VERB_NAME))
            && has_gall_command(DIRECTORY_SHELL_SUBKEY);
        let label = dir_verb
            .as_deref()
            .and_then(|v| read_verb_label(DIRECTORY_SHELL_SUBKEY, v));
        let current_handler = Some(if is_default {
            "Gall".to_string()
        } else {
            windows_verb_display_name(dir_verb.as_deref(), label.as_deref())
        });
        let can_set = stable_exe_path().is_some();
        let unavailable_reason = if can_set {
            None
        } else {
            Some(UnavailableReason::ExecutableNotFound)
        };
        let restore_target = if is_default {
            let saved = read_backup(data_dir)
                .windows
                .and_then(|w| w.previous_display_name);
            Some(saved.unwrap_or_else(|| "File Explorer".to_string()))
        } else {
            None
        };
        DefaultFileManagerStatus {
            platform: "windows".to_string(),
            is_default,
            current_handler,
            current_id: dir_verb,
            can_set,
            unavailable_reason,
            can_restore: is_default,
            restore_target,
        }
    }

    pub fn set_default(data_dir: &Path) -> Result<DefaultFileManagerStatus> {
        let exe = stable_exe_path()
            .ok_or_else(|| CoreError::Other("cannot determine Gall executable path".into()))?;
        let before = status(data_dir);
        let mut saved_backup = read_backup(data_dir).windows;
        if !before.is_default {
            let previous = WindowsBackup {
                directory_prev: read_default_verb(DIRECTORY_SHELL_SUBKEY),
                drive_prev: read_default_verb(DRIVE_SHELL_SUBKEY),
                previous_display_name: before.current_handler.clone(),
            };
            let mut backup = read_backup(data_dir);
            backup.windows = Some(previous.clone());
            write_backup(data_dir, &backup)?;
            saved_backup = Some(previous);
        }
        if let Some(previous) = saved_backup.as_ref() {
            write_previous_default_verb(
                DIRECTORY_SHELL_SUBKEY,
                previous.directory_prev.as_deref(),
            )?;
            write_previous_default_verb(DRIVE_SHELL_SUBKEY, previous.drive_prev.as_deref())?;
        }
        write_gall_verb(DIRECTORY_SHELL_SUBKEY, &exe)?;
        write_gall_verb(DRIVE_SHELL_SUBKEY, &exe)?;
        notify_shell_assoc_changed();
        Ok(status(data_dir))
    }

    pub fn restore_default(data_dir: &Path) -> Result<DefaultFileManagerStatus> {
        let mut backup = read_backup(data_dir);
        let win_backup = backup.windows.take();
        restore_shell_key(
            DIRECTORY_SHELL_SUBKEY,
            win_backup
                .as_ref()
                .and_then(|w| w.directory_prev.as_deref()),
        )?;
        restore_shell_key(
            DRIVE_SHELL_SUBKEY,
            win_backup.as_ref().and_then(|w| w.drive_prev.as_deref()),
        )?;
        notify_shell_assoc_changed();
        if backup.linux.is_none() && backup.windows.is_none() && backup.macos.is_none() {
            remove_backup(data_dir);
        } else {
            write_backup(data_dir, &backup)?;
        }
        Ok(status(data_dir))
    }
}

// ---------------------------------------------------------------------------
// macOS helpers (cross-platform pure helpers + macOS defaults/LaunchServices)
// ---------------------------------------------------------------------------

/// Given an executable path such as `/Applications/Gall.app/Contents/MacOS/gall`, returns the
/// enclosing `.app` bundle directory (`/Applications/Gall.app`).
pub fn macos_app_bundle_from_exe(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let app_dir = contents_dir.parent()?;
    if app_dir
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
    {
        Some(app_dir.to_path_buf())
    } else {
        None
    }
}

/// Reads `(CFBundleIdentifier, display_name)` from `<bundle>/Contents/Info.plist` (XML or binary).
pub fn macos_read_bundle_id_and_name(bundle: &Path) -> Option<(String, Option<String>)> {
    let plist_path = bundle.join("Contents/Info.plist");
    let value = plist::Value::from_file(plist_path).ok()?;
    let dict = value.as_dictionary()?;
    let bundle_id = dict.get("CFBundleIdentifier")?.as_string()?.trim();
    if bundle_id.is_empty() {
        return None;
    }
    let name = dict
        .get("CFBundleDisplayName")
        .and_then(|v| v.as_string())
        .or_else(|| dict.get("CFBundleName").and_then(|v| v.as_string()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    Some((bundle_id.to_string(), name))
}

/// Human-readable fallback from a macOS bundle identifier (e.g. `com.apple.finder` -> `"Finder"`).
pub fn macos_fallback_bundle_name(bundle_id: &str) -> String {
    let id = bundle_id.trim();
    if id.eq_ignore_ascii_case(MACOS_FINDER_BUNDLE_ID) {
        return "Finder".to_string();
    }
    if id.eq_ignore_ascii_case(MACOS_BUNDLE_ID) {
        return "Gall".to_string();
    }
    id.rsplit('.').next().unwrap_or(id).to_string()
}

/// Searches candidate `.app` bundle paths for `org.gall.app`.
pub fn macos_find_gall_bundle_in(candidates: &[PathBuf]) -> Option<PathBuf> {
    for candidate in candidates {
        if let Some((id, _)) = macos_read_bundle_id_and_name(candidate) {
            if id.eq_ignore_ascii_case(MACOS_BUNDLE_ID) {
                return Some(candidate.clone());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::*;
    use std::process::Command;

    fn candidate_gall_bundles() -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Some(exe) = stable_exe_path() {
            if let Some(bundle) = macos_app_bundle_from_exe(&exe) {
                out.push(bundle);
            }
        }
        out.push(PathBuf::from("/Applications/Gall.app"));
        if let Some(home) = dirs::home_dir() {
            out.push(home.join("Applications/Gall.app"));
        }
        out
    }

    fn read_ns_file_viewer() -> Option<String> {
        let output = Command::new("/usr/bin/defaults")
            .args(["read", "-g", "NSFileViewer"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!val.is_empty()).then_some(val)
    }

    fn display_name_for_bundle_id(bundle_id: &str) -> String {
        if bundle_id.eq_ignore_ascii_case(MACOS_FINDER_BUNDLE_ID) {
            return "Finder".to_string();
        }
        if bundle_id.eq_ignore_ascii_case(MACOS_BUNDLE_ID) {
            return "Gall".to_string();
        }
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        for root in roots {
            let Ok(entries) = std::fs::read_dir(root) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) {
                    if let Some((id, name)) = macos_read_bundle_id_and_name(&p) {
                        if id.eq_ignore_ascii_case(bundle_id) {
                            return name.unwrap_or_else(|| macos_fallback_bundle_name(bundle_id));
                        }
                    }
                }
            }
        }
        macos_fallback_bundle_name(bundle_id)
    }

    pub fn status(data_dir: &Path) -> DefaultFileManagerStatus {
        let viewer = read_ns_file_viewer();
        let effective_id = viewer
            .clone()
            .unwrap_or_else(|| MACOS_FINDER_BUNDLE_ID.to_string());
        let is_default = effective_id.eq_ignore_ascii_case(MACOS_BUNDLE_ID);
        let current_handler = Some(display_name_for_bundle_id(&effective_id));
        let bundle_present = macos_find_gall_bundle_in(&candidate_gall_bundles()).is_some();
        let can_set = bundle_present;
        let unavailable_reason = if can_set {
            None
        } else {
            Some(UnavailableReason::MacosAppBundleRequired)
        };
        let restore_target = if is_default {
            let saved = read_backup(data_dir)
                .macos
                .and_then(|m| m.previous_display_name);
            Some(saved.unwrap_or_else(|| "Finder".to_string()))
        } else {
            None
        };
        DefaultFileManagerStatus {
            platform: "macos".to_string(),
            is_default,
            current_handler,
            current_id: Some(effective_id),
            can_set,
            unavailable_reason,
            can_restore: is_default,
            restore_target,
        }
    }

    pub fn set_default(data_dir: &Path) -> Result<DefaultFileManagerStatus> {
        if macos_find_gall_bundle_in(&candidate_gall_bundles()).is_none() {
            return Err(CoreError::Other(
                "Install Gall.app in Applications to set it as the default file manager".into(),
            ));
        }
        let before = status(data_dir);
        let mut backup = read_backup(data_dir);
        if !before.is_default || backup.macos.is_none() {
            let prev_id =
                read_ns_file_viewer().filter(|id| !id.eq_ignore_ascii_case(MACOS_BUNDLE_ID));
            backup.macos = Some(MacosBackup {
                previous_bundle_id: prev_id,
                previous_folder_bundle_id: crate::util::openwith::macos_default_uti_handler(
                    "public.folder",
                )
                .filter(|id| !id.eq_ignore_ascii_case(MACOS_BUNDLE_ID)),
                previous_display_name: if before.is_default {
                    None
                } else {
                    before.current_handler.clone()
                },
            });
            write_backup(data_dir, &backup)?;
        }
        let out = Command::new("/usr/bin/defaults")
            .args(["write", "-g", "NSFileViewer", "-string", MACOS_BUNDLE_ID])
            .output()
            .map_err(|e| CoreError::Other(format!("cannot run defaults: {e}")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(CoreError::Other(if stderr.is_empty() {
                format!("defaults write failed with {}", out.status)
            } else {
                stderr
            }));
        }
        let ls_status =
            crate::util::openwith::macos_set_uti_handler("public.folder", MACOS_BUNDLE_ID);
        if ls_status != 0 {
            return Err(CoreError::Other(format!(
                "cannot set macOS default folder handler (LaunchServices status {ls_status})"
            )));
        }
        Ok(status(data_dir))
    }

    pub fn restore_default(data_dir: &Path) -> Result<DefaultFileManagerStatus> {
        let mut backup = read_backup(data_dir);
        let macos_backup = backup.macos.take().unwrap_or_default();
        let previous_folder_handler = macos_backup.previous_folder_handler();
        let ls_status =
            crate::util::openwith::macos_set_uti_handler("public.folder", previous_folder_handler);
        if ls_status != 0 {
            return Err(CoreError::Other(format!(
                "cannot restore macOS default folder handler (LaunchServices status {ls_status})"
            )));
        }
        let prev = macos_backup.previous_bundle_id.filter(|id| {
            !id.eq_ignore_ascii_case(MACOS_BUNDLE_ID)
                && !id.eq_ignore_ascii_case(MACOS_FINDER_BUNDLE_ID)
        });
        match prev.as_deref() {
            Some(other) => {
                let out = Command::new("/usr/bin/defaults")
                    .args(["write", "-g", "NSFileViewer", "-string", other])
                    .output()
                    .map_err(|e| CoreError::Other(format!("cannot run defaults: {e}")))?;
                if !out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    return Err(CoreError::Other(if stderr.is_empty() {
                        format!("defaults write failed with {}", out.status)
                    } else {
                        stderr
                    }));
                }
            }
            None => {
                let out = Command::new("/usr/bin/defaults")
                    .args(["delete", "-g", "NSFileViewer"])
                    .output()
                    .map_err(|e| CoreError::Other(format!("cannot run defaults: {e}")))?;
                if !out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if !stderr.to_ascii_lowercase().contains("does not exist") {
                        return Err(CoreError::Other(stderr.trim().to_string()));
                    }
                }
            }
        }
        if backup.linux.is_none() && backup.windows.is_none() && backup.macos.is_none() {
            remove_backup(data_dir);
        } else {
            write_backup(data_dir, &backup)?;
        }
        Ok(status(data_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_linux_env(root: &Path, desktops: &[&str]) -> LinuxEnv {
        LinuxEnv {
            config_home: root.join("config"),
            config_dirs: vec![root.join("etc/xdg")],
            data_home: root.join("data"),
            data_dirs: vec![root.join("usr/share")],
            desktops: desktops.iter().map(|s| s.to_string()).collect(),
            in_flatpak: false,
            flatpak_id: None,
            exe_path: Some(PathBuf::from("/opt/Gall File Manager/gall")),
            refresh_desktop_db: false,
        }
    }

    fn write_file(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn older_macos_backup_loads_without_a_saved_folder_handler() {
        let backup: BackupState = serde_json::from_str(
            r#"{"macos":{"previousBundleId":"com.example.filemanager","previousDisplayName":"Files"}}"#,
        )
        .unwrap();
        let macos = backup.macos.unwrap();
        assert_eq!(
            macos.previous_bundle_id.as_deref(),
            Some("com.example.filemanager")
        );
        assert_eq!(macos.previous_folder_bundle_id, None);
        assert_eq!(macos.previous_folder_handler(), "com.example.filemanager");
    }

    #[test]
    fn linux_set_and_restore_preserves_other_entries_and_restores_previous_manager() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("gall-data");
        let env = temp_linux_env(tmp.path(), &["kde"]);

        write_file(
            &env.data_dirs[0].join("applications/org.kde.dolphin.desktop"),
            "[Desktop Entry]\nType=Application\nName=Dolphin\nExec=dolphin %u\n",
        );
        write_file(
            &env.data_dirs[0].join("applications/gall.desktop"),
            "[Desktop Entry]\nType=Application\nName=Gall\nExec=gall %U\nMimeType=inode/directory;\n",
        );
        write_file(
            &env.data_dirs[0].join("applications/org.gnome.nautilus.desktop"),
            "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus %U\n",
        );
        let mimeapps = env.config_home.join("mimeapps.list");
        write_file(
            &mimeapps,
            "# user defaults\n[Default Applications]\ntext/plain=nvim.desktop;\ninode/directory=org.kde.dolphin.desktop;org.gnome.nautilus.desktop;\n\n[Removed Associations]\ninode/directory=gall.desktop;thunar.desktop;\n",
        );

        let before = linux_status_in(&env, &data_dir);
        assert!(!before.is_default);
        assert_eq!(before.current_handler.as_deref(), Some("Dolphin"));
        assert!(before.can_set);
        assert!(!before.can_restore);

        let after_set = linux_set_default_in(&env, &data_dir).unwrap();
        assert!(after_set.is_default);
        assert_eq!(after_set.current_handler.as_deref(), Some("Gall"));
        assert!(after_set.can_restore);
        assert_eq!(after_set.restore_target.as_deref(), Some("Dolphin"));

        let written = std::fs::read_to_string(&mimeapps).unwrap();
        assert!(written.contains("text/plain=nvim.desktop;"));
        assert!(written.contains("inode/directory=gall.desktop;"));
        assert!(written.contains("inode/directory=thunar.desktop;"));
        assert!(!written.contains("gall.desktop;thunar.desktop"));

        // Calling set_default again must not clobber the saved Dolphin backup.
        let after_second = linux_set_default_in(&env, &data_dir).unwrap();
        assert_eq!(after_second.restore_target.as_deref(), Some("Dolphin"));

        let after_restore = linux_restore_default_in(&env, &data_dir).unwrap();
        assert!(!after_restore.is_default);
        assert_eq!(after_restore.current_handler.as_deref(), Some("Dolphin"));
        assert_eq!(
            read_default_associations_from_mimeapps_text(
                &std::fs::read_to_string(&mimeapps).unwrap(),
                LINUX_DIRECTORY_MIME
            )
            .as_deref(),
            Some("org.kde.dolphin.desktop;org.gnome.nautilus.desktop;")
        );
        assert!(!backup_path(&data_dir).exists());
    }

    #[test]
    fn linux_set_and_restore_preserves_fallbacks_when_gall_is_already_default() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("gall-data");
        let env = temp_linux_env(tmp.path(), &[]);

        write_file(
            &env.data_dirs[0].join("applications/gall.desktop"),
            "[Desktop Entry]\nType=Application\nName=Gall\nExec=gall %U\nMimeType=inode/directory;\n",
        );
        write_file(
            &env.data_dirs[0].join("applications/org.gnome.nautilus.desktop"),
            "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus %U\n",
        );
        let mimeapps = env.config_home.join("mimeapps.list");
        write_file(
            &mimeapps,
            "[Default Applications]\ninode/directory=gall.desktop;org.gnome.nautilus.desktop;\n",
        );

        assert!(linux_status_in(&env, &data_dir).is_default);
        let after_set = linux_set_default_in(&env, &data_dir).unwrap();
        assert_eq!(after_set.restore_target.as_deref(), Some("Files"));
        assert!(backup_path(&data_dir).exists());

        let after_restore = linux_restore_default_in(&env, &data_dir).unwrap();
        assert!(!after_restore.is_default);
        assert_eq!(after_restore.current_handler.as_deref(), Some("Files"));
        assert_eq!(
            read_default_associations_from_mimeapps_text(
                &std::fs::read_to_string(&mimeapps).unwrap(),
                LINUX_DIRECTORY_MIME,
            )
            .as_deref(),
            Some("org.gnome.nautilus.desktop;")
        );
    }

    #[test]
    fn linux_restore_without_backup_preserves_fallback_associations() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("gall-data");
        let env = temp_linux_env(tmp.path(), &[]);

        write_file(
            &env.data_dirs[0].join("applications/gall.desktop"),
            "[Desktop Entry]\nType=Application\nName=Gall\nExec=gall %U\nMimeType=inode/directory;\n",
        );
        write_file(
            &env.data_dirs[0].join("applications/org.kde.dolphin.desktop"),
            "[Desktop Entry]\nType=Application\nName=Dolphin\nExec=dolphin %u\n",
        );
        write_file(
            &env.data_dirs[0].join("applications/org.gnome.nautilus.desktop"),
            "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus %U\n",
        );
        let mimeapps = env.config_home.join("mimeapps.list");
        write_file(
            &mimeapps,
            "[Default Applications]\ninode/directory=gall.desktop;org.kde.dolphin.desktop;org.gnome.nautilus.desktop;\n",
        );

        let after_restore = linux_restore_default_in(&env, &data_dir).unwrap();
        assert!(!after_restore.is_default);
        assert_eq!(after_restore.current_handler.as_deref(), Some("Dolphin"));
        assert_eq!(
            read_default_associations_from_mimeapps_text(
                &std::fs::read_to_string(&mimeapps).unwrap(),
                LINUX_DIRECTORY_MIME,
            )
            .as_deref(),
            Some("org.kde.dolphin.desktop;org.gnome.nautilus.desktop;")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_multi_handler_restore_agrees_with_xdg_mime_and_gio() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("gall-data");
        let env = temp_linux_env(tmp.path(), &["hyprland"]);

        let dolphin_desktop = env.data_dirs[0].join("applications/org.kde.dolphin.desktop");
        write_file(
            &dolphin_desktop,
            "[Desktop Entry]\nType=Application\nName=Dolphin\nExec=/usr/bin/true %u\nMimeType=inode/directory;\n",
        );
        write_file(
            &env.data_dirs[0].join("applications/org.gnome.nautilus.desktop"),
            "[Desktop Entry]\nType=Application\nName=Files\nExec=/usr/bin/true %U\nMimeType=inode/directory;\n",
        );
        write_file(
            &env.data_dirs[0].join("applications/gall.desktop"),
            "[Desktop Entry]\nType=Application\nName=Gall\nExec=/usr/bin/true %U\nMimeType=inode/directory;\n",
        );
        let mimeapps = env.config_home.join("mimeapps.list");
        write_file(
            &mimeapps,
            "[Default Applications]\ninode/directory=org.kde.dolphin.desktop;org.gnome.nautilus.desktop;\n",
        );

        let query_xdg = |e: &LinuxEnv| -> Option<String> {
            let out = std::process::Command::new("xdg-mime")
                .args(["query", "default", LINUX_DIRECTORY_MIME])
                .env("XDG_CONFIG_HOME", &e.config_home)
                .env("XDG_CONFIG_DIRS", &e.config_dirs[0])
                .env("XDG_DATA_HOME", &e.data_home)
                .env("XDG_DATA_DIRS", &e.data_dirs[0])
                .env("XDG_CURRENT_DESKTOP", "Hyprland")
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        let query_gio = |e: &LinuxEnv| -> Option<String> {
            let out = std::process::Command::new("gio")
                .args(["mime", LINUX_DIRECTORY_MIME])
                .env("XDG_CONFIG_HOME", &e.config_home)
                .env("XDG_CONFIG_DIRS", &e.config_dirs[0])
                .env("XDG_DATA_HOME", &e.data_home)
                .env("XDG_DATA_DIRS", &e.data_dirs[0])
                .env("XDG_CURRENT_DESKTOP", "Hyprland")
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).to_string())
        };

        if let Some(before) = query_xdg(&env) {
            assert_eq!(before, "org.kde.dolphin.desktop");
        }
        if let Some(before_gio) = query_gio(&env) {
            assert!(before_gio.contains("org.kde.dolphin.desktop"));
        }

        let after_set = linux_set_default_in(&env, &data_dir).unwrap();
        assert!(after_set.is_default);
        if let Some(during) = query_xdg(&env) {
            assert_eq!(during, "gall.desktop");
        }
        if let Some(during_gio) = query_gio(&env) {
            assert!(during_gio.contains("gall.desktop"));
        }

        let after_restore = linux_restore_default_in(&env, &data_dir).unwrap();
        assert!(!after_restore.is_default);
        if let Some(restored) = query_xdg(&env) {
            assert_eq!(restored, "org.kde.dolphin.desktop");
        }

        // Removing the primary handler proves Freedesktop still resolves the retained fallback.
        std::fs::remove_file(&dolphin_desktop).unwrap();
        if let Some(fallback) = query_xdg(&env) {
            assert_eq!(fallback, "org.gnome.nautilus.desktop");
        }
        if let Some(fallback_gio) = query_gio(&env) {
            assert!(fallback_gio.contains("org.gnome.nautilus.desktop"));
        }
    }

    #[test]
    fn linux_updates_desktop_specific_override_and_restores_it() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("gall-data");
        let env = temp_linux_env(tmp.path(), &["hyprland"]);

        write_file(
            &env.data_dirs[0].join("applications/org.gnome.Nautilus.desktop"),
            "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus %U\n",
        );
        write_file(
            &env.data_dirs[0].join("applications/gall.desktop"),
            "[Desktop Entry]\nType=Application\nName=Gall\nExec=gall %U\n",
        );
        let de_mimeapps = env.config_home.join("hyprland-mimeapps.list");
        write_file(
            &de_mimeapps,
            "[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop;\n",
        );

        let after_set = linux_set_default_in(&env, &data_dir).unwrap();
        assert!(after_set.is_default);
        assert_eq!(after_set.restore_target.as_deref(), Some("Files"));
        assert_eq!(
            read_default_from_mimeapps_text(
                &std::fs::read_to_string(&de_mimeapps).unwrap(),
                LINUX_DIRECTORY_MIME
            )
            .as_deref(),
            Some("gall.desktop")
        );

        let after_restore = linux_restore_default_in(&env, &data_dir).unwrap();
        assert!(!after_restore.is_default);
        assert_eq!(after_restore.current_handler.as_deref(), Some("Files"));
    }

    #[test]
    fn linux_creates_local_desktop_entry_for_unpacked_binary_and_escapes_spaces() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("gall-data");
        let env = temp_linux_env(tmp.path(), &[]);

        let after_set = linux_set_default_in(&env, &data_dir).unwrap();
        assert!(after_set.is_default);
        let desktop_file = env.data_home.join("applications/gall.desktop");
        assert!(desktop_file.is_file());
        let text = std::fs::read_to_string(&desktop_file).unwrap();
        assert!(text.contains("Exec=\"/opt/Gall File Manager/gall\" %U"));
        assert!(text.contains("MimeType=inode/directory;"));

        // Restoring with no previous user entry removes the line cleanly.
        let after_restore = linux_restore_default_in(&env, &data_dir).unwrap();
        assert!(!after_restore.is_default);
        assert!(after_restore.current_handler.is_none());
    }

    #[test]
    fn linux_flatpak_uses_flatpak_desktop_id_without_writing_local_desktop_file() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("gall-data");
        let mut env = temp_linux_env(tmp.path(), &[]);
        env.in_flatpak = true;
        env.flatpak_id = Some("io.github.gama87.Gall".into());

        let after_set = linux_set_default_in(&env, &data_dir).unwrap();
        assert!(after_set.is_default);
        assert_eq!(
            after_set.current_id.as_deref(),
            Some("io.github.gama87.Gall.desktop")
        );
        assert!(!env
            .data_home
            .join("applications/io.github.gama87.Gall.desktop")
            .exists());
    }

    #[test]
    fn windows_command_quotes_exe_and_protects_drive_root_backslash() {
        let exe = Path::new(r"C:\Program Files\Gall\gall.exe");
        assert_eq!(
            format_windows_open_command(exe),
            r#""C:\Program Files\Gall\gall.exe" -- "%1\.""#
        );
        assert_eq!(
            format_windows_icon_value(exe),
            r#""C:\Program Files\Gall\gall.exe",0"#
        );
        assert_eq!(windows_verb_display_name(None, None), "File Explorer");
        assert_eq!(
            windows_verb_display_name(Some("none"), None),
            "File Explorer"
        );
        assert_eq!(
            windows_verb_display_name(Some(WINDOWS_VERB_NAME), Some("Open in Gall")),
            "Gall"
        );
        assert_eq!(
            windows_verb_display_name(Some("open_in_dopus"), Some("&Open in Directory Opus")),
            "Open in Directory Opus"
        );
    }

    #[test]
    fn macos_bundle_detection_and_info_plist_parsing() {
        assert_eq!(
            macos_app_bundle_from_exe(Path::new("/Applications/Gall.app/Contents/MacOS/gall")),
            Some(PathBuf::from("/Applications/Gall.app"))
        );
        assert_eq!(
            macos_app_bundle_from_exe(Path::new("/Users/dev/colonnade/target/debug/gall")),
            None
        );

        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("Gall.app");
        write_file(
            &bundle.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>org.gall.app</string>
  <key>CFBundleDisplayName</key>
  <string>Gall</string>
</dict>
</plist>"#,
        );
        assert_eq!(
            macos_read_bundle_id_and_name(&bundle),
            Some(("org.gall.app".into(), Some("Gall".into())))
        );
        assert_eq!(
            macos_find_gall_bundle_in(&[tmp.path().join("Missing.app"), bundle.clone()]),
            Some(bundle)
        );
        assert_eq!(macos_fallback_bundle_name("com.apple.finder"), "Finder");
        assert_eq!(macos_fallback_bundle_name("org.gall.app"), "Gall");
        assert_eq!(
            macos_fallback_bundle_name("com.cocoatech.PathFinder"),
            "PathFinder"
        );
    }
}
