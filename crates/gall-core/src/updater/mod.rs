//! Self-update capability detection, signature verification, installation, and startup bootstrapper.

pub mod bootstrap;
pub mod install;
pub mod verify;

use crate::util::appdirs::AppDirs;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// CLI flag passed to a candidate update binary before switching over.
pub const PROBE_FLAG: &str = "--gall-update-probe";
/// CLI flag passed to the newly launched version so it waits for the old PID to release locks.
pub const WAIT_PID_FLAG: &str = "--gall-wait-pid";
/// Prefix printed by `--gall-update-probe` on stdout.
pub const PROBE_PREFIX: &str = "GALL_PROBE_OK ";
/// Environment variable carrying the base binary path when running from a user-space overlay.
pub const BASE_EXE_ENV: &str = "GALL_BASE_EXE";
/// Environment variable carrying the base installed version when running from an overlay.
pub const BASE_VERSION_ENV: &str = "GALL_BASE_VERSION";
/// Escape-hatch environment variable to bypass overlay handoff and run the base binary directly.
pub const NO_OVERLAY_ENV: &str = "GALL_NO_OVERLAY";
/// Compile-time flag indicating the binary is managed by a distribution package manager.
pub const PACKAGE_MANAGED_ENV: Option<&str> = option_env!("GALL_PACKAGE_MANAGED");
/// Runtime marker file installed by Linux distro packages (e.g. Arch PKGBUILD).
pub const PACKAGE_MANAGED_MARKER: &str = "/usr/share/gall/package-managed";

/// Suffix placed on the displaced binary/bundle during an in-place swap; removed after restart.
pub const OLD_SUFFIX: &str = ".gall-old";
const NEW_SUFFIX: &str = ".gall-new";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallStrategy {
    InPlace,
    Overlay,
    StoreManaged,
    Unconfigured,
}

impl InstallStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InPlace => "inPlace",
            Self::Overlay => "overlay",
            Self::StoreManaged => "storeManaged",
            Self::Unconfigured => "unconfigured",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallKind {
    Portable,
    Nsis,
    Msi,
    Scoop,
    AppImage,
    SystemPackage,
    MacOsApp,
    Local,
    Flatpak,
    Snap,
    DistroPackage,
}

impl InstallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::Nsis => "nsis",
            Self::Msi => "msi",
            Self::Scoop => "scoop",
            Self::AppImage => "appimage",
            Self::SystemPackage => "system-package",
            Self::MacOsApp => "macos-app",
            Self::Local => "local",
            Self::Flatpak => "flatpak",
            Self::Snap => "snap",
            Self::DistroPackage => "distro-package",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveKind {
    LinuxTarGz,
    WindowsZip,
    MacOsTarGz,
    LinuxAppImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateStage {
    Downloading,
    Verifying,
    Extracting,
    Probing,
    Applying,
    Restarting,
}

#[derive(Debug, Clone)]
pub struct UpdateCapability {
    pub strategy: InstallStrategy,
    pub install_kind: InstallKind,
    pub target_key: String,
    pub archive_kind: ArchiveKind,
    /// Non-empty when in-app installation is unavailable (store-managed, unconfigured, or read-only portable).
    pub reason: String,
    /// Folder containing the primary installation (or base installation when running as an overlay).
    pub install_dir: PathBuf,
    /// Folder storing user-space update stages, verified versions, and overlay metadata.
    pub updates_dir: PathBuf,
    /// Executable replaced by in-place updates (`$APPIMAGE` for AppImage, `gall.exe` / `gall` otherwise).
    pub target_exe_path: PathBuf,
    /// macOS `.app` bundle path when running inside a macOS application bundle.
    pub target_bundle_path: Option<PathBuf>,
    /// Underlying base executable when running as an overlay (otherwise equals `target_exe_path`).
    pub base_exe_path: PathBuf,
    /// Version of the underlying base executable.
    pub base_version: String,
    /// True when the current process was handed off from a base binary into `updates/versions/`.
    pub running_overlay: bool,
    /// Active overlay version recorded in `active.json`, if any.
    pub active_overlay_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousOverlay {
    pub version: String,
    pub binary_rel: String,
    pub sig_rel: String,
    #[serde(default)]
    pub is_appimage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveOverlay {
    pub version: String,
    pub binary_rel: String,
    pub sig_rel: String,
    #[serde(default)]
    pub is_appimage: bool,
    pub verified: bool,
    pub boot_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_verified: Option<PreviousOverlay>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedUpdate {
    pub version: String,
    pub target_key: String,
    pub archive_kind: ArchiveKind,
    pub binary_rel: String,
    pub sig_rel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_rel: Option<String>,
    #[serde(default)]
    pub is_appimage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StartupNotice {
    Updated {
        version: String,
        from_version: Option<String>,
    },
    RolledBack {
        failed_version: String,
        restored_version: String,
        reason: String,
    },
    Reverted {
        from_version: String,
        to_version: String,
    },
}

fn is_truthy_flag(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Pure, injectable probe for distro-managed package detection. The compile-time flag belongs to this
/// binary. The marker file only says that *a* package is installed: it counts only when the running
/// binary lives in a system folder, so an AppImage, a portable folder or a `~/.local` build next to the
/// package still updates itself.
pub fn detect_package_managed_with(
    os: &str,
    compile_env: Option<&str>,
    marker_path: &Path,
    exe: Option<&Path>,
) -> bool {
    is_truthy_flag(compile_env)
        || (os == "linux" && marker_path.exists() && exe.is_some_and(is_unix_system_prefix))
}

/// Returns true when Gall was built or packaged as a distro-managed package.
pub fn is_package_managed() -> bool {
    detect_package_managed_with(
        std::env::consts::OS,
        PACKAGE_MANAGED_ENV,
        Path::new(PACKAGE_MANAGED_MARKER),
        std::env::current_exe().ok().as_deref(),
    )
}

/// Inputs describing the host installation environment (abstracted for thorough unit testing).
#[derive(Debug, Clone)]
pub struct CapabilityProbe {
    pub os: &'static str,
    pub arch: &'static str,
    pub pubkey: String,
    pub current_version: String,
    pub current_exe: PathBuf,
    pub dirs: AppDirs,
    pub appimage: Option<PathBuf>,
    pub is_flatpak: bool,
    pub is_snap: bool,
    pub is_package_managed: bool,
    pub base_exe_env: Option<PathBuf>,
    pub base_version_env: Option<String>,
    pub nsis_user_install_dir: Option<PathBuf>,
    pub force_overlay: bool,
}

impl CapabilityProbe {
    pub fn from_env(pubkey: String, current_version: String, dirs: AppDirs) -> Self {
        let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gall"));
        let appimage = std::env::var_os("APPIMAGE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let base_exe_env = std::env::var_os(BASE_EXE_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let base_version_env = std::env::var(BASE_VERSION_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty());
        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            pubkey,
            current_version,
            current_exe,
            dirs,
            appimage,
            is_flatpak: cfg!(target_os = "linux") && Path::new("/.flatpak-info").exists(),
            is_snap: cfg!(target_os = "linux")
                && std::env::var_os("SNAP").is_some_and(|v| !v.is_empty()),
            is_package_managed: is_package_managed(),
            base_exe_env,
            base_version_env,
            nsis_user_install_dir: read_nsis_user_install_dir(),
            force_overlay: false,
        }
    }
}

#[cfg(windows)]
fn read_nsis_user_install_dir() -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Gall",
            KEY_READ,
        )
        .ok()?;
    let loc: String = key.get_value("InstallLocation").ok()?;
    let trimmed = loc.trim().trim_matches('"');
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

#[cfg(not(windows))]
fn read_nsis_user_install_dir() -> Option<PathBuf> {
    None
}

pub(crate) fn canonical_or_self(path: &Path) -> PathBuf {
    crate::nav::path::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn paths_equal(a: &Path, b: &Path) -> bool {
    let ca = canonical_or_self(a);
    let cb = canonical_or_self(b);
    if cfg!(windows) {
        ca.to_string_lossy()
            .eq_ignore_ascii_case(&cb.to_string_lossy())
    } else {
        ca == cb
    }
}

pub(crate) fn path_starts_with(child: &Path, parent: &Path) -> bool {
    let c_child = canonical_or_self(child);
    let c_parent = canonical_or_self(parent);
    if cfg!(windows) {
        let mut p = c_parent.to_string_lossy().to_lowercase();
        let c = c_child.to_string_lossy().to_lowercase();
        if !p.ends_with('\\') && !p.ends_with('/') {
            p.push('\\');
        }
        c == p.trim_end_matches(['\\', '/']) || c.starts_with(&p)
    } else {
        c_child.starts_with(&c_parent)
    }
}

/// Returns the `.app` root if `exe` points to `<Name>.app/Contents/MacOS/<bin>`.
pub fn macos_bundle_root(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name().and_then(|n| n.to_str()) != Some("MacOS") {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name().and_then(|n| n.to_str()) != Some("Contents") {
        return None;
    }
    let app_dir = contents_dir.parent()?;
    if app_dir.extension().and_then(|e| e.to_str()) == Some("app") {
        Some(app_dir.to_path_buf())
    } else {
        None
    }
}

fn dir_is_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    tempfile::NamedTempFile::new_in(dir).is_ok()
}

fn file_is_replaceable(dir: &Path, file: &Path) -> bool {
    if !dir_is_writable(dir) {
        return false;
    }
    if cfg!(windows) {
        if let Ok(meta) = std::fs::metadata(file) {
            if meta.permissions().readonly() {
                return false;
            }
        }
    }
    true
}

fn is_unix_system_prefix(path: &Path) -> bool {
    path.starts_with("/usr")
        || path.starts_with("/bin")
        || path.starts_with("/sbin")
        || path.starts_with("/lib")
        || path.starts_with("/opt")
}

fn is_scoop_path(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_lowercase().replace('/', "\\");
    lower.contains("\\scoop\\apps\\gall\\")
}

fn target_for_probe(probe: &CapabilityProbe, is_appimage: bool) -> (String, ArchiveKind) {
    let arch = if probe.arch == "aarch64" {
        "aarch64"
    } else {
        "x86_64"
    };
    match probe.os {
        "windows" => (format!("windows-{arch}-bin"), ArchiveKind::WindowsZip),
        "macos" => (format!("darwin-{arch}-app"), ArchiveKind::MacOsTarGz),
        _ if is_appimage => (format!("linux-{arch}"), ArchiveKind::LinuxAppImage),
        _ => (format!("linux-{arch}-bin"), ArchiveKind::LinuxTarGz),
    }
}

pub fn detect_capability(pubkey: &str, current_version: &str, dirs: &AppDirs) -> UpdateCapability {
    let probe = CapabilityProbe::from_env(
        pubkey.to_string(),
        current_version.to_string(),
        dirs.clone(),
    );
    detect_capability_with(&probe)
}

pub fn detect_capability_with(probe: &CapabilityProbe) -> UpdateCapability {
    let updates_dir = probe.dirs.updates();
    let versions_dir = updates_dir.join("versions");
    let active: Option<ActiveOverlay> = read_json_opt(&active_overlay_path(&updates_dir));

    let running_appimage_path = probe.appimage.as_ref().filter(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| !n.is_empty())
    });

    let effective_exe = running_appimage_path
        .cloned()
        .unwrap_or_else(|| probe.current_exe.clone());
    let running_overlay = probe.base_exe_env.is_some()
        && (path_starts_with(&probe.current_exe, &versions_dir)
            || running_appimage_path.is_some_and(|p| path_starts_with(p, &versions_dir)));

    let is_appimage = probe.os == "linux"
        && (running_appimage_path.is_some()
            || (running_overlay && active.as_ref().is_some_and(|a| a.is_appimage)));

    let (target_key, archive_kind) = target_for_probe(probe, is_appimage);

    let base_exe_path = if running_overlay {
        probe
            .base_exe_env
            .clone()
            .unwrap_or_else(|| effective_exe.clone())
    } else {
        effective_exe.clone()
    };
    let base_version = if running_overlay {
        probe
            .base_version_env
            .clone()
            .unwrap_or_else(|| probe.current_version.clone())
    } else {
        probe.current_version.clone()
    };

    let target_bundle_path = if probe.os == "macos" {
        macos_bundle_root(&effective_exe)
    } else {
        None
    };

    let base_bundle_path = if probe.os == "macos" {
        macos_bundle_root(&base_exe_path)
    } else {
        None
    };

    let install_dir = base_bundle_path
        .as_deref()
        .and_then(Path::parent)
        .or_else(|| base_exe_path.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let classify_kind = |strategy: InstallStrategy| -> InstallKind {
        if probe.is_flatpak {
            return InstallKind::Flatpak;
        }
        if probe.is_snap {
            return InstallKind::Snap;
        }
        if probe.is_package_managed {
            return InstallKind::DistroPackage;
        }
        if probe.dirs.portable {
            return InstallKind::Portable;
        }
        match probe.os {
            "windows" => {
                if is_scoop_path(&base_exe_path) {
                    InstallKind::Scoop
                } else if probe
                    .nsis_user_install_dir
                    .as_deref()
                    .is_some_and(|d| paths_equal(d, &install_dir))
                {
                    InstallKind::Nsis
                } else if strategy == InstallStrategy::Overlay {
                    InstallKind::Msi
                } else {
                    InstallKind::Portable
                }
            }
            "macos" => {
                if base_bundle_path.is_some() {
                    InstallKind::MacOsApp
                } else {
                    InstallKind::Local
                }
            }
            _ => {
                if is_appimage {
                    InstallKind::AppImage
                } else if is_unix_system_prefix(&base_exe_path) {
                    InstallKind::SystemPackage
                } else {
                    InstallKind::Local
                }
            }
        }
    };

    let make = |strategy: InstallStrategy, reason: String| UpdateCapability {
        strategy,
        install_kind: classify_kind(strategy),
        target_key: target_key.clone(),
        archive_kind,
        reason,
        install_dir: install_dir.clone(),
        updates_dir: updates_dir.clone(),
        target_exe_path: effective_exe.clone(),
        target_bundle_path: target_bundle_path.clone(),
        base_exe_path: base_exe_path.clone(),
        base_version: base_version.clone(),
        running_overlay,
        active_overlay_version: active.as_ref().map(|a| a.version.clone()),
    };

    if probe.is_flatpak {
        return make(
            InstallStrategy::StoreManaged,
            "This installation is managed by Flatpak. Updates are installed through Flatpak or your software centre.".into(),
        );
    }
    if probe.is_snap {
        return make(
            InstallStrategy::StoreManaged,
            "This installation is managed by Snap. Updates are installed automatically by Snap."
                .into(),
        );
    }
    if probe.is_package_managed {
        return make(
            InstallStrategy::StoreManaged,
            "This installation is managed by your system package manager. Updates are installed through your package manager."
                .into(),
        );
    }
    if probe.pubkey.trim().is_empty() {
        return make(
            InstallStrategy::Unconfigured,
            "This build has no update signing key configured. Download the latest release manually.".into(),
        );
    }

    if probe.dirs.portable {
        if file_is_replaceable(&install_dir, &effective_exe) {
            return make(InstallStrategy::InPlace, String::new());
        }
        return make(
            InstallStrategy::Unconfigured,
            "This portable folder is read-only. Make it writable to install updates in place."
                .into(),
        );
    }

    if running_overlay || probe.force_overlay {
        return make(InstallStrategy::Overlay, String::new());
    }

    match probe.os {
        "windows" => {
            if is_scoop_path(&base_exe_path) {
                make(InstallStrategy::Overlay, String::new())
            } else if file_is_replaceable(&install_dir, &effective_exe) {
                make(InstallStrategy::InPlace, String::new())
            } else {
                make(InstallStrategy::Overlay, String::new())
            }
        }
        "macos" => {
            let translocated = effective_exe
                .to_string_lossy()
                .contains("/AppTranslocation/");
            if translocated {
                return make(InstallStrategy::Overlay, String::new());
            }
            if let Some(ref bundle) = target_bundle_path {
                let parent = bundle.parent().unwrap_or(Path::new("/"));
                let contents = bundle.join("Contents");
                if dir_is_writable(parent) && dir_is_writable(&contents) {
                    make(InstallStrategy::InPlace, String::new())
                } else {
                    make(InstallStrategy::Overlay, String::new())
                }
            } else if file_is_replaceable(&install_dir, &effective_exe)
                && !is_unix_system_prefix(&effective_exe)
            {
                make(InstallStrategy::InPlace, String::new())
            } else {
                make(InstallStrategy::Overlay, String::new())
            }
        }
        _ => {
            if is_appimage {
                if file_is_replaceable(&install_dir, &effective_exe) {
                    make(InstallStrategy::InPlace, String::new())
                } else {
                    make(InstallStrategy::Overlay, String::new())
                }
            } else if is_unix_system_prefix(&base_exe_path)
                || !file_is_replaceable(&install_dir, &effective_exe)
            {
                make(InstallStrategy::Overlay, String::new())
            } else {
                make(InstallStrategy::InPlace, String::new())
            }
        }
    }
}

pub fn active_overlay_path(updates_dir: &Path) -> PathBuf {
    updates_dir.join("active.json")
}

pub fn staged_dir(updates_dir: &Path) -> PathBuf {
    updates_dir.join("staging")
}

pub fn staged_meta_path(updates_dir: &Path) -> PathBuf {
    staged_dir(updates_dir).join("staged.json")
}

pub fn pending_notice_path(updates_dir: &Path) -> PathBuf {
    updates_dir.join("pending_notice.json")
}

pub fn versions_dir(updates_dir: &Path) -> PathBuf {
    updates_dir.join("versions")
}

pub(crate) fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid target path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("Cannot create temporary file in {}: {e}", parent.display()))?;
    serde_json::to_writer_pretty(&mut tmp, value)
        .map_err(|e| format!("Cannot serialize update metadata: {e}"))?;
    use std::io::Write;
    tmp.flush()
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| format!("Cannot flush {}: {e}", path.display()))?;
    tmp.persist(path)
        .map_err(|e| format!("Cannot save {}: {}", path.display(), e.error))?;
    Ok(())
}

pub(crate) fn read_json_opt<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let data = std::fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

pub fn write_startup_notice(updates_dir: &Path, notice: &StartupNotice) -> Result<(), String> {
    atomic_write_json(&pending_notice_path(updates_dir), notice)
}

pub fn take_startup_notice(updates_dir: &Path) -> Option<StartupNotice> {
    let path = pending_notice_path(updates_dir);
    let notice = read_json_opt(&path)?;
    let _ = std::fs::remove_file(&path);
    Some(notice)
}

/// Returns `true` when `candidate` parses as a strictly newer semver than `current`.
pub fn is_version_newer(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate.trim_start_matches('v')),
        semver::Version::parse(current.trim_start_matches('v')),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    }
}

/// Ensures an update version string is a single safe semver directory component.
pub(crate) fn validate_version_dir_name(version: &str) -> Result<(), String> {
    if version.is_empty()
        || version == "."
        || version == ".."
        || version.contains(['/', '\\'])
        || Path::new(version).components().count() != 1
    {
        return Err(format!("Refusing invalid update version string: {version}"));
    }
    semver::Version::parse(version.trim_start_matches('v'))
        .map_err(|e| format!("Invalid update version {version}: {e}"))?;
    Ok(())
}

/// Validates that a relative updater or archive path contains only normal path components.
pub(crate) fn validate_rel_path(rel: &Path) -> Result<PathBuf, String> {
    use std::path::Component;
    let mut clean = PathBuf::new();
    for comp in rel.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(part) => clean.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "Refusing unsafe archive entry path: {}",
                    rel.display()
                ));
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err("Refusing empty archive entry path.".into());
    }
    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_probe(dir: &Path) -> CapabilityProbe {
        CapabilityProbe {
            os: "linux",
            arch: "x86_64",
            pubkey: "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3".into(),
            current_version: "0.2.2".into(),
            current_exe: dir.join("gall"),
            dirs: AppDirs::at(&dir.join("data")),
            appimage: None,
            is_flatpak: false,
            is_snap: false,
            is_package_managed: false,
            base_exe_env: None,
            base_version_env: None,
            nsis_user_install_dir: None,
            force_overlay: false,
        }
    }

    #[test]
    fn capability_detects_flatpak_snap_and_keyless() {
        let t = tempfile::tempdir().unwrap();
        let mut p = base_probe(t.path());
        p.is_flatpak = true;
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::StoreManaged);
        assert_eq!(c.install_kind, InstallKind::Flatpak);
        assert!(!c.reason.is_empty());

        p.is_flatpak = false;
        p.is_snap = true;
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::StoreManaged);
        assert_eq!(c.install_kind, InstallKind::Snap);

        p.is_snap = false;
        p.pubkey.clear();
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::Unconfigured);
        assert!(!c.reason.is_empty());
    }

    #[test]
    fn capability_routes_system_packages_to_overlay_and_writable_appimage_in_place() {
        let t = tempfile::tempdir().unwrap();
        let mut p = base_probe(t.path());
        p.current_exe = PathBuf::from("/usr/bin/gall");
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::Overlay);
        assert_eq!(c.install_kind, InstallKind::SystemPackage);
        assert_eq!(c.target_key, "linux-x86_64-bin");
        assert!(c.reason.is_empty());

        let app_dir = t.path().join("apps");
        std::fs::create_dir_all(&app_dir).unwrap();
        let appimage = app_dir.join("Gall.AppImage");
        std::fs::write(&appimage, b"appimage").unwrap();
        p.appimage = Some(appimage);
        p.current_exe = t.path().join("mount/usr/bin/gall");
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::InPlace);
        assert_eq!(c.install_kind, InstallKind::AppImage);
        assert_eq!(c.target_key, "linux-x86_64");

        // Non-writable AppImage falls back to overlay.
        p.appimage = Some(PathBuf::from("/opt/gall-missing-ro/Gall.AppImage"));
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::Overlay);
        assert_eq!(c.install_kind, InstallKind::AppImage);
    }

    #[test]
    fn capability_handles_portable_scoop_nsis_and_readonly_windows() {
        let t = tempfile::tempdir().unwrap();
        let mut p = base_probe(t.path());
        p.os = "windows";
        p.dirs.portable = true;
        std::fs::write(t.path().join("gall.exe"), b"exe").unwrap();
        p.current_exe = t.path().join("gall.exe");
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::InPlace);
        assert_eq!(c.install_kind, InstallKind::Portable);
        assert_eq!(c.target_key, "windows-x86_64-bin");

        // Non-existent/read-only portable folder blocks instead of writing to LocalAppData.
        p.current_exe = t.path().join("readonly-missing/gall.exe");
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::Unconfigured);
        assert!(!c.reason.is_empty());

        // Portable ZIP WITHOUT `portable` marker still updates in place!
        p.dirs.portable = false;
        p.current_exe = t.path().join("gall.exe");
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::InPlace);
        assert_eq!(c.install_kind, InstallKind::Portable);

        // NSIS user-scope updates in place.
        p.nsis_user_install_dir = Some(t.path().to_path_buf());
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::InPlace);
        assert_eq!(c.install_kind, InstallKind::Nsis);

        // Scoop always uses overlay so Scoop's app folder stays intact.
        p.nsis_user_install_dir = None;
        let scoop_dir = t.path().join("scoop/apps/gall/current");
        std::fs::create_dir_all(&scoop_dir).unwrap();
        p.current_exe = scoop_dir.join("gall.exe");
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::Overlay);
        assert_eq!(c.install_kind, InstallKind::Scoop);

        // Read-only MSI location uses overlay.
        p.current_exe = PathBuf::from(r"Z:\nonexistent-program-files\Gall\gall.exe");
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::Overlay);
        assert_eq!(c.install_kind, InstallKind::Msi);
    }

    #[test]
    fn capability_handles_macos_app_and_translocation() {
        let t = tempfile::tempdir().unwrap();
        let app_dir = t.path().join("Apps");
        let exe = app_dir.join("Gall.app/Contents/MacOS/gall");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"macho").unwrap();

        let mut p = base_probe(t.path());
        p.os = "macos";
        p.arch = "aarch64";
        p.current_exe = exe;
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::InPlace);
        assert_eq!(c.install_kind, InstallKind::MacOsApp);
        assert_eq!(c.target_key, "darwin-aarch64-app");
        assert_eq!(c.target_bundle_path.unwrap(), app_dir.join("Gall.app"));

        p.current_exe = PathBuf::from(
            "/private/var/folders/xx/AppTranslocation/1234/d/Gall.app/Contents/MacOS/gall",
        );
        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::Overlay);
        assert_eq!(c.install_kind, InstallKind::MacOsApp);
    }

    #[test]
    fn capability_detects_running_overlay_and_preserves_base_info() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let dirs = AppDirs::at(&data);
        let overlay_exe = dirs.updates().join("versions/0.4.0/gall");
        std::fs::create_dir_all(overlay_exe.parent().unwrap()).unwrap();
        std::fs::write(&overlay_exe, b"bin").unwrap();

        let mut p = base_probe(t.path());
        p.dirs = dirs;
        p.current_version = "0.4.0".into();
        p.current_exe = overlay_exe;
        p.base_exe_env = Some(PathBuf::from("/usr/bin/gall"));
        p.base_version_env = Some("0.3.0".into());

        let c = detect_capability_with(&p);
        assert!(c.running_overlay);
        assert_eq!(c.strategy, InstallStrategy::Overlay);
        assert_eq!(c.install_kind, InstallKind::SystemPackage);
        assert_eq!(c.base_version, "0.3.0");
        assert_eq!(c.install_dir, PathBuf::from("/usr/bin"));
    }

    #[test]
    fn package_managed_returns_store_managed_and_distro_package() {
        let t = tempfile::tempdir().unwrap();
        let mut p = base_probe(t.path());
        p.is_package_managed = true;
        p.pubkey.clear();
        p.current_exe = PathBuf::from("/usr/bin/gall");
        p.force_overlay = true;

        let c = detect_capability_with(&p);
        assert_eq!(c.strategy, InstallStrategy::StoreManaged);
        assert_eq!(c.install_kind, InstallKind::DistroPackage);
        assert_eq!(c.install_kind.as_str(), "distro-package");
        assert!(!c.reason.is_empty());
    }

    #[test]
    fn detect_package_managed_with_handles_env_and_marker_per_os() {
        let t = tempfile::tempdir().unwrap();
        let missing = t.path().join("missing-marker");
        let marker = t.path().join("package-managed");
        let sys = Path::new("/usr/bin/gall");

        assert!(detect_package_managed_with(
            "linux",
            Some("1"),
            &missing,
            None
        ));
        assert!(detect_package_managed_with(
            "linux",
            Some("true"),
            &missing,
            None
        ));
        assert!(detect_package_managed_with(
            "windows",
            Some("yes"),
            &missing,
            None
        ));
        assert!(!detect_package_managed_with(
            "linux",
            Some("0"),
            &missing,
            Some(sys)
        ));
        assert!(!detect_package_managed_with(
            "linux",
            None,
            &missing,
            Some(sys)
        ));

        std::fs::write(&marker, b"").unwrap();
        assert!(detect_package_managed_with(
            "linux",
            None,
            &marker,
            Some(sys)
        ));
        assert!(!detect_package_managed_with(
            "windows",
            None,
            &marker,
            Some(sys)
        ));
        assert!(!detect_package_managed_with(
            "macos",
            None,
            &marker,
            Some(sys)
        ));
        // The marker of an installed package does not make another copy of Gall package-managed.
        for other in [
            "/tmp/.mount_GallXYZ/usr/bin/gall",
            "/home/u/.local/bin/gall",
            "/home/u/Portable/gall",
        ] {
            assert!(
                !detect_package_managed_with("linux", None, &marker, Some(Path::new(other))),
                "{other}"
            );
        }
        assert!(!detect_package_managed_with("linux", None, &marker, None));
    }
}
