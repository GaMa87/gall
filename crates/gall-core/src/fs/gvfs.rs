//! GVFS and KDE KIO-Fuse mount discovery, classification (Devices vs. Network),
//! connect/disconnect/eject, and non-blocking slow-mount guards.
//!
//! - `mtp:` / `gphoto2:` mounts are surfaced in `Drives` via [`gvfs_drives`].
//! - `smb-share:` / `nfs:` / `sftp:` / `dav:` / `ftp:` / `afp:` GVFS mounts and active
//!   KDE `kio-fuse` mounts are surfaced in `Network` via [`list_network_mounts`].
//! - `google-drive:` / `onedrive:` remain handled exclusively by [`super::cloud::classify_gvfs`].

use super::drives::Drive;
use crate::error::{CoreError, Result};
use serde::Serialize;
#[cfg(target_os = "linux")]
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GvfsKind {
    Device,
    Network,
    Cloud,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkMount {
    pub name: String,
    pub scheme: String,
    pub uri: String,
    pub mount_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedGvfsEntry {
    pub dir_name: String,
    pub raw_scheme: String,
    pub kind: GvfsKind,
    pub scheme: String,
    pub params: Vec<(String, String)>,
    pub mount_path: String,
}

impl ParsedGvfsEntry {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GioMountInfo {
    pub name: String,
    pub uri: String,
    pub mount_type: String,
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Percent-decodes `%XX` byte sequences into UTF-8 (lossy on invalid sequences).
/// Does not treat `+` as space and preserves malformed `%` literals without panicking.
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses a single GVFS directory entry name (under `$XDG_RUNTIME_DIR/gvfs/`) into a
/// structured [`ParsedGvfsEntry`].
pub fn parse_gvfs_entry(dir_name: &str, mount_path: &Path) -> Option<ParsedGvfsEntry> {
    let (raw_scheme, rest) = dir_name.split_once(':')?;
    if raw_scheme.is_empty() {
        return None;
    }
    let mut params = Vec::new();
    for token in rest.split(',').filter(|t| !t.is_empty()) {
        if let Some((k, v)) = token.split_once('=') {
            params.push((percent_decode(k), percent_decode(v)));
        }
    }
    let param_val = |key: &str| -> Option<&str> {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };

    let (kind, scheme) = if super::cloud::classify_gvfs(dir_name).is_some() {
        (GvfsKind::Cloud, raw_scheme.to_string())
    } else {
        match raw_scheme {
            "mtp" | "gphoto2" => (GvfsKind::Device, raw_scheme.to_string()),
            "smb-share" | "smb" => (GvfsKind::Network, "smb".to_string()),
            "nfs" => (GvfsKind::Network, "nfs".to_string()),
            "sftp" => (GvfsKind::Network, "sftp".to_string()),
            "dav" => {
                let s = if param_val("ssl") == Some("true") {
                    "davs"
                } else {
                    "dav"
                };
                (GvfsKind::Network, s.to_string())
            }
            "davs" | "ftp" | "ftps" | "ftpes" | "afp" => {
                (GvfsKind::Network, raw_scheme.to_string())
            }
            _ => (GvfsKind::Other, raw_scheme.to_string()),
        }
    };

    Some(ParsedGvfsEntry {
        dir_name: dir_name.to_string(),
        raw_scheme: raw_scheme.to_string(),
        kind,
        scheme,
        params,
        mount_path: mount_path.to_string_lossy().into_owned(),
    })
}

/// Parses `LC_ALL=C gio mount -l` output into [`GioMountInfo`] records.
pub fn parse_gio_mount_list(output: &str) -> Vec<GioMountInfo> {
    let mut mounts: Vec<GioMountInfo> = Vec::new();
    let mut expecting_type = false;

    for raw in output.lines() {
        let line = raw.trim_start();
        if line.starts_with("Drive(") || line.starts_with("Volume(") {
            expecting_type = false;
            continue;
        }
        if line.starts_with("Mount(") {
            expecting_type = false;
            if let Some(idx) = line.find("): ") {
                let body = &line[idx + 3..];
                if let Some((name, uri)) = body.rsplit_once(" -> ") {
                    let uri = uri.trim();
                    if uri.contains("://") && !uri.chars().any(char::is_whitespace) {
                        mounts.push(GioMountInfo {
                            name: name.trim().to_string(),
                            uri: uri.to_string(),
                            mount_type: String::new(),
                        });
                        expecting_type = true;
                    }
                }
            }
            continue;
        }
        if expecting_type {
            if let Some(t) = line.strip_prefix("Type: ") {
                if let Some(last) = mounts.last_mut() {
                    last.mount_type = t.trim().to_string();
                }
                expecting_type = false;
            }
        }
    }

    mounts
}

#[derive(Debug)]
struct UriParts {
    scheme: String,
    user: Option<String>,
    host: Option<String>,
    port: Option<String>,
    path: String,
}

fn parse_uri_parts(uri: &str) -> Option<UriParts> {
    let (scheme, rest) = uri.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let (auth, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    };
    let (user, host_port) = match auth.rsplit_once('@') {
        Some((u, hp)) => (Some(percent_decode(u)), hp),
        None => (None, auth),
    };
    let (host, port) = if let Some(inner) = host_port.strip_prefix('[') {
        if let Some((h, after)) = inner.split_once(']') {
            let p = after.strip_prefix(':').map(str::to_string);
            (Some(percent_decode(h)), p)
        } else {
            (Some(percent_decode(host_port)), None)
        }
    } else if let Some((h, p)) = host_port.rsplit_once(':') {
        if p.bytes().all(|b| b.is_ascii_digit()) && !p.is_empty() {
            (Some(percent_decode(h)), Some(p.to_string()))
        } else {
            (Some(percent_decode(host_port)), None)
        }
    } else if host_port.is_empty() {
        (None, None)
    } else {
        (Some(percent_decode(host_port)), None)
    };
    Some(UriParts {
        scheme,
        user,
        host,
        port,
        path: percent_decode(path),
    })
}

fn scheme_compatible(entry_scheme: &str, uri_scheme: &str) -> Option<u32> {
    if entry_scheme == uri_scheme {
        return Some(2);
    }
    let compat = matches!(
        (entry_scheme, uri_scheme),
        ("sftp", "ssh")
            | ("dav", "davs")
            | ("davs", "dav")
            | ("ftp", "ftps" | "ftpes")
            | ("ftps" | "ftpes", "ftp")
    );
    compat.then_some(1)
}

fn match_score(entry: &ParsedGvfsEntry, uri: &UriParts) -> Option<u32> {
    let scheme_pts = scheme_compatible(&entry.scheme, &uri.scheme)?;
    let entry_host = entry.param("server").or_else(|| entry.param("host"))?;
    let uri_host = uri.host.as_deref()?;
    let entry_host_norm = entry_host.trim_start_matches('[').trim_end_matches(']');
    let uri_host_norm = uri_host.trim_start_matches('[').trim_end_matches(']');
    if !entry_host_norm.eq_ignore_ascii_case(uri_host_norm) {
        return None;
    }
    let mut score = scheme_pts + 10;

    if let Some(share) = entry.param("share") {
        let first_seg = uri
            .path
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("");
        if !first_seg.is_empty() && !first_seg.eq_ignore_ascii_case(share) {
            return None;
        }
        if first_seg.eq_ignore_ascii_case(share) {
            score += 8;
        }
    }
    if let Some(prefix) = entry.param("prefix").or_else(|| entry.param("path")) {
        let norm_prefix = prefix.trim_end_matches('/');
        let norm_uri_path = uri.path.trim_end_matches('/');
        if !norm_prefix.is_empty() {
            if norm_uri_path == norm_prefix {
                score += 6;
            } else if !norm_uri_path
                .strip_prefix(norm_prefix)
                .is_some_and(|rest| rest.starts_with('/'))
            {
                // `/remote` is a prefix of `/remote/x`, not of `/remote2/x`.
                return None;
            }
        }
    }
    if let (Some(eu), Some(uu)) = (entry.param("user"), uri.user.as_deref()) {
        if eu == uu {
            score += 3;
        } else {
            return None;
        }
    }
    if let (Some(ep), Some(up)) = (entry.param("port"), uri.port.as_deref()) {
        if ep == up {
            score += 2;
        } else {
            return None;
        }
    }
    Some(score)
}

fn fallback_device_label(entry: &ParsedGvfsEntry) -> String {
    if let Some(host) = entry.param("host").filter(|h| !h.is_empty()) {
        let is_usb = host.starts_with("[usb:") || host.starts_with("usb:");
        if !is_usb {
            return host.replace('_', " ");
        }
        return if entry.raw_scheme == "gphoto2" {
            format!("Camera ({host})")
        } else {
            format!("MTP Device ({host})")
        };
    }
    if entry.raw_scheme == "gphoto2" {
        "Camera".into()
    } else {
        "MTP Device".into()
    }
}

fn fallback_network_name(entry: &ParsedGvfsEntry) -> String {
    let host = entry
        .param("server")
        .or_else(|| entry.param("host"))
        .unwrap_or("");
    match entry.scheme.as_str() {
        "smb" => match (entry.param("share"), host) {
            (Some(s), h) if !h.is_empty() => format!("{s} on {h}"),
            (Some(s), _) => s.to_string(),
            (None, h) if !h.is_empty() => h.to_string(),
            _ => entry.dir_name.clone(),
        },
        "nfs" => match (host, entry.param("path")) {
            (h, Some(p)) if !h.is_empty() => format!("{h}:{p}"),
            (h, _) if !h.is_empty() => h.to_string(),
            _ => entry.dir_name.clone(),
        },
        _ => match (entry.param("user"), host) {
            (Some(u), h) if !h.is_empty() => format!("{u}@{h}"),
            (_, h) if !h.is_empty() => h.to_string(),
            _ => entry.dir_name.clone(),
        },
    }
}

fn fallback_network_uri(entry: &ParsedGvfsEntry) -> String {
    let host = entry
        .param("server")
        .or_else(|| entry.param("host"))
        .unwrap_or("");
    let user = entry
        .param("user")
        .filter(|u| !u.is_empty())
        .map(|u| format!("{u}@"))
        .unwrap_or_default();
    let port = entry
        .param("port")
        .filter(|p| !p.is_empty())
        .map(|p| format!(":{p}"))
        .unwrap_or_default();
    let tail = entry
        .param("share")
        .or_else(|| entry.param("path"))
        .or_else(|| entry.param("prefix"))
        .filter(|t| !t.is_empty())
        .map(|t| {
            if t.starts_with('/') {
                t.to_string()
            } else {
                format!("/{t}")
            }
        })
        .unwrap_or_default();
    format!("{}://{user}{host}{port}{tail}", entry.scheme)
}

/// Classifies parsed GVFS entries (enriched with optional `gio mount -l` metadata) into
/// removable device drives (`mtp`/`gphoto2`) and [`NetworkMount`] entries (`smb`/`nfs`/`sftp`/`dav`/`ftp`/`afp`).
/// Cloud entries (`google-drive`/`onedrive`) and `Other` schemes are excluded.
pub fn resolve_gvfs_mounts(
    entries: &[ParsedGvfsEntry],
    gio_mounts: &[GioMountInfo],
) -> (Vec<Drive>, Vec<NetworkMount>) {
    let relevant: Vec<&ParsedGvfsEntry> = entries
        .iter()
        .filter(|e| matches!(e.kind, GvfsKind::Device | GvfsKind::Network))
        .collect();

    let parsed_gio: Vec<(usize, &GioMountInfo, UriParts)> = gio_mounts
        .iter()
        .enumerate()
        .filter_map(|(idx, m)| {
            let parts = parse_uri_parts(&m.uri)?;
            if matches!(parts.scheme.as_str(), "file" | "google-drive" | "onedrive") {
                return None;
            }
            Some((idx, m, parts))
        })
        .collect();

    let mut used = vec![false; gio_mounts.len()];
    let mut matched_idx: Vec<Option<usize>> = vec![None; relevant.len()];

    // Pass 1: primary host/scheme/share match.
    for (ei, entry) in relevant.iter().enumerate() {
        let best = parsed_gio
            .iter()
            .filter(|(gi, _, _)| !used[*gi])
            .filter_map(|(gi, _, parts)| match_score(entry, parts).map(|s| (s, *gi)))
            .max_by_key(|(s, _)| *s);
        if let Some((_, gi)) = best {
            used[gi] = true;
            matched_idx[ei] = Some(gi);
        }
    }

    // Pass 2: unique-scheme fallback for unmatched entries (e.g. gphoto2:host=usb:... <-> gphoto2://Camera/).
    for (ei, entry) in relevant.iter().enumerate() {
        if matched_idx[ei].is_some() {
            continue;
        }
        let candidates: Vec<usize> = parsed_gio
            .iter()
            .filter(|(gi, _, parts)| !used[*gi] && parts.scheme == entry.scheme)
            .map(|(gi, _, _)| *gi)
            .collect();
        if candidates.len() == 1 {
            let gi = candidates[0];
            used[gi] = true;
            matched_idx[ei] = Some(gi);
        }
    }

    let mut drives = Vec::new();
    let mut network = Vec::new();

    for (ei, entry) in relevant.iter().enumerate() {
        let gio = matched_idx[ei].map(|gi| &gio_mounts[gi]);
        match entry.kind {
            GvfsKind::Device => {
                let short = gio
                    .map(|g| g.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| fallback_device_label(entry));
                drives.push(Drive {
                    short,
                    label: String::new(),
                    mount: entry.mount_path.clone(),
                    fs: entry.scheme.clone(),
                    total: 0,
                    available: 0,
                    removable: true,
                    lockable: false,
                    space_shown_by: String::new(),
                    device: String::new(),
                });
            }
            GvfsKind::Network => {
                let name = gio
                    .map(|g| g.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| fallback_network_name(entry));
                let uri = gio
                    .map(|g| g.uri.clone())
                    .filter(|u| !u.is_empty())
                    .unwrap_or_else(|| fallback_network_uri(entry));
                network.push(NetworkMount {
                    name,
                    scheme: entry.scheme.clone(),
                    uri,
                    mount_path: entry.mount_path.clone(),
                });
            }
            GvfsKind::Cloud | GvfsKind::Other => {}
        }
    }

    drives.sort_by(|a, b| a.mount.cmp(&b.mount));
    network.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.mount_path.cmp(&b.mount_path))
    });

    (drives, network)
}

fn is_run_user_gvfs_components(parts: &[Component<'_>], exact_root: bool) -> bool {
    let valid_uid = |s: &std::ffi::OsStr| {
        s.to_str()
            .is_some_and(|u| !u.is_empty() && u.bytes().all(|b| b.is_ascii_digit()))
    };
    if exact_root {
        matches!(
            parts,
            [
                Component::RootDir,
                Component::Normal(r),
                Component::Normal(u),
                Component::Normal(uid),
                Component::Normal(g),
            ] if *r == "run" && *u == "user" && valid_uid(uid) && *g == "gvfs"
        )
    } else {
        matches!(
            parts,
            [
                Component::RootDir,
                Component::Normal(r),
                Component::Normal(u),
                Component::Normal(uid),
                Component::Normal(g),
                ..
            ] if *r == "run" && *u == "user" && valid_uid(uid) && *g == "gvfs"
        )
    }
}

/// Pure, cross-platform helper checking whether `path` is a GVFS root or inside a GVFS directory.
pub fn is_gvfs_path_with(path: &Path, runtime_dir: Option<&Path>, home_dir: Option<&Path>) -> bool {
    if let Some(rt) = runtime_dir.filter(|p| !p.as_os_str().is_empty()) {
        if path.starts_with(rt.join("gvfs")) {
            return true;
        }
    }
    if let Some(home) = home_dir.filter(|p| !p.as_os_str().is_empty()) {
        if path.starts_with(home.join(".gvfs")) {
            return true;
        }
    }
    let parts: Vec<Component<'_>> = path.components().collect();
    is_run_user_gvfs_components(&parts, false)
}

/// Pure, cross-platform helper checking whether `mount` is an individual GVFS submount
/// directly under `$XDG_RUNTIME_DIR/gvfs`, `/run/user/<uid>/gvfs`, or `~/.gvfs`.
pub fn is_gvfs_mount_path_with(
    mount: &Path,
    runtime_dir: Option<&Path>,
    home_dir: Option<&Path>,
) -> bool {
    if mount.file_name().is_none() {
        return false;
    }
    let Some(parent) = mount.parent() else {
        return false;
    };
    if let Some(rt) = runtime_dir.filter(|p| !p.as_os_str().is_empty()) {
        if parent == rt.join("gvfs") {
            return true;
        }
    }
    if let Some(home) = home_dir.filter(|p| !p.as_os_str().is_empty()) {
        if parent == home.join(".gvfs") {
            return true;
        }
    }
    let parts: Vec<Component<'_>> = parent.components().collect();
    is_run_user_gvfs_components(&parts, true)
}

/// Validates a remote server URI before passing it to `kio-fuse` or `gio mount`.
pub fn validate_connect_uri(uri: &str) -> Result<String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') || trimmed.contains(['\0', '\r', '\n']) {
        return Err(CoreError::Other("invalid server URI".into()));
    }
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err(CoreError::Other(
            "server URI must include a scheme (e.g. smb://host/share or sftp://user@host)".into(),
        ));
    };
    let scheme_lower = scheme.to_ascii_lowercase();
    if !matches!(
        scheme_lower.as_str(),
        "smb"
            | "sftp"
            | "ssh"
            | "nfs"
            | "dav"
            | "davs"
            | "webdav"
            | "webdavs"
            | "ftp"
            | "ftps"
            | "ftpes"
            | "afp"
            | "mtp"
            | "gphoto2"
    ) {
        return Err(CoreError::Other(format!(
            "unsupported server URI scheme '{scheme}'"
        )));
    }
    let authority = rest.split('/').next().unwrap_or("");
    // A password in the address would show in the process list and in error messages: gio/KIO asks for it.
    if authority
        .rsplit_once('@')
        .is_some_and(|(user, _)| user.contains(':'))
    {
        return Err(CoreError::Other(
            "do not put the password in the address (smb://user@host/share): you are asked for it"
                .into(),
        ));
    }
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(inner) = host_port.strip_prefix('[') {
        let Some((bracketed, after)) = inner.split_once(']') else {
            return Err(CoreError::Other(
                "server URI has an unclosed IPv6 host bracket".into(),
            ));
        };
        if !after.is_empty() && !after.starts_with(':') {
            return Err(CoreError::Other(
                "server URI has an invalid IPv6 host".into(),
            ));
        }
        bracketed
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return Err(CoreError::Other("server URI is missing a host name".into()));
    }
    Ok(trimmed.to_string())
}

/// Which Linux desktop VFS backend should be tried first when connecting to a server URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectBackend {
    Gvfs,
    KioFuse,
}

/// True when `XDG_CURRENT_DESKTOP` (colon-separated) identifies KDE Plasma.
pub fn is_kde_desktop(xdg_current_desktop: &str) -> bool {
    xdg_current_desktop
        .split(':')
        .any(|part| part.trim().eq_ignore_ascii_case("KDE"))
}

/// Selects the primary backend for "Connect to server…": KDE Plasma prefers `kio-fuse` so
/// mounts share Dolphin's KIO workers and KWallet credentials (including host-only `nfs://host`);
/// other desktops prefer GVFS (`gio mount`) and fall back to `kio-fuse` when GVFS is unavailable.
pub fn preferred_connect_backend(xdg_current_desktop: &str) -> ConnectBackend {
    if is_kde_desktop(xdg_current_desktop) {
        ConnectBackend::KioFuse
    } else {
        ConnectBackend::Gvfs
    }
}

/// Translates a validated server URI into the scheme expected by KDE KIO / `kio-fuse`.
pub fn kio_fuse_uri_for(uri: &str) -> String {
    let Some((scheme, rest)) = uri.split_once("://") else {
        return uri.to_string();
    };
    let mapped = match scheme.to_ascii_lowercase().as_str() {
        "dav" => "webdav",
        "davs" => "webdavs",
        "ssh" => "sftp",
        other => return format!("{other}://{rest}"),
    };
    format!("{mapped}://{rest}")
}

/// Translates a validated server URI into the scheme expected by GVFS (`gio mount`).
pub fn gvfs_uri_for(uri: &str) -> String {
    let Some((scheme, rest)) = uri.split_once("://") else {
        return uri.to_string();
    };
    let mapped = match scheme.to_ascii_lowercase().as_str() {
        "webdav" => "dav",
        "webdavs" => "davs",
        other => return format!("{other}://{rest}"),
    };
    format!("{mapped}://{rest}")
}

/// True when `uri` is an `nfs://host` URI without an export path (only `/` or empty).
pub fn is_host_only_nfs_uri(uri: &str) -> bool {
    parse_uri_parts(uri).is_some_and(|p| p.scheme == "nfs" && p.path.trim_matches('/').is_empty())
}

/// Environment variables removed when spawning system `gio` from inside an AppImage so `gio`
/// loads the host's GLib and GVFS modules instead of failing with "volume doesn't implement mount".
pub fn appimage_gio_env_removals(in_appimage: bool) -> &'static [&'static str] {
    if in_appimage {
        &[
            "APPIMAGE",
            "APPDIR",
            "ARGV0",
            "OWD",
            "LD_LIBRARY_PATH",
            "GIO_MODULE_DIR",
            "GIO_EXTRA_MODULES",
            "GSETTINGS_SCHEMA_DIR",
        ]
    } else {
        &[]
    }
}

fn is_kio_fuse_component(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|s| s == "kio-fuse" || s.starts_with("kio-fuse-"))
}

/// Pure, cross-platform helper checking whether `path` lies inside a KDE `kio-fuse` mount root.
pub fn is_kio_fuse_path_with(path: &Path, runtime_dir: Option<&Path>) -> bool {
    if let Some(rt) = runtime_dir.filter(|p| !p.as_os_str().is_empty()) {
        if let Ok(rel) = path.strip_prefix(rt) {
            if let Some(Component::Normal(first)) = rel.components().next() {
                if is_kio_fuse_component(first) {
                    return true;
                }
            }
        }
    }
    let parts: Vec<Component<'_>> = path.components().collect();
    matches!(
        parts.as_slice(),
        [
            Component::RootDir,
            Component::Normal(r),
            Component::Normal(u),
            Component::Normal(uid),
            Component::Normal(kf),
            ..
        ] if *r == "run"
            && *u == "user"
            && uid.to_str().is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            && is_kio_fuse_component(kf)
    )
}

/// True when `path` lies inside a KDE `kio-fuse` mount on Linux.
pub fn is_kio_fuse_path(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let rt = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        if is_kio_fuse_path_with(path, rt.as_deref()) {
            return true;
        }
        kio_fuse_roots().iter().any(|root| path.starts_with(root))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

/// Extracts active `fuse.kio-fuse` mount roots from `/proc/self/mounts` text.
pub fn parse_kio_fuse_roots(mounts_text: &str) -> Vec<PathBuf> {
    let unescape = |s: &str| -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            let octal = b
                .get(i + 1..i + 4)
                .filter(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)));
            if let (b'\\', Some(d)) = (b[i], octal) {
                out.push(
                    d.iter()
                        .fold(0u8, |v, c| v.wrapping_mul(8).wrapping_add(c - b'0')),
                );
                i += 4;
                continue;
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    let mut roots = Vec::new();
    for line in mounts_text.lines() {
        let mut fields = line.split(' ');
        let Some(_dev) = fields.next() else { continue };
        let Some(point) = fields.next() else { continue };
        let Some(kind) = fields.next() else { continue };
        if kind == "fuse.kio-fuse" {
            let path = PathBuf::from(unescape(point));
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
    }
    roots
}

fn normalize_display_scheme(raw: &str) -> Option<&'static str> {
    match raw.to_ascii_lowercase().as_str() {
        "smb" => Some("smb"),
        "nfs" => Some("nfs"),
        "sftp" | "ssh" | "fish" => Some("sftp"),
        "dav" | "webdav" => Some("dav"),
        "davs" | "webdavs" => Some("davs"),
        "ftp" => Some("ftp"),
        "ftps" => Some("ftps"),
        "ftpes" => Some("ftpes"),
        "afp" => Some("afp"),
        _ => None,
    }
}

/// Builds a [`NetworkMount`] from a depth-2 `kio-fuse` directory (`<root>/<scheme>/<authority>`).
pub fn parse_kio_fuse_entry(
    scheme_dir: &str,
    authority: &str,
    mount_path: &Path,
) -> Option<NetworkMount> {
    let scheme = normalize_display_scheme(scheme_dir)?;
    if authority.is_empty() {
        return None;
    }
    Some(NetworkMount {
        name: authority.to_string(),
        scheme: scheme.to_string(),
        uri: format!("{scheme}://{authority}"),
        mount_path: mount_path.to_string_lossy().into_owned(),
    })
}

/// Builds a [`NetworkMount`] from a user-supplied URI and the local path returned by `kio-fuse`.
pub fn network_mount_from_kio_uri(uri: &str, mount_path: &Path) -> NetworkMount {
    let mount_path_str = mount_path.to_string_lossy().into_owned();
    let Some(parts) = parse_uri_parts(uri) else {
        return NetworkMount {
            name: uri.to_string(),
            scheme: "network".to_string(),
            uri: uri.to_string(),
            mount_path: mount_path_str,
        };
    };
    let scheme = normalize_display_scheme(&parts.scheme)
        .unwrap_or("network")
        .to_string();
    let host = parts.host.as_deref().unwrap_or("");
    let trimmed_path = parts.path.trim_end_matches('/');
    let name = match scheme.as_str() {
        "smb" => {
            let share = trimmed_path
                .trim_start_matches('/')
                .split('/')
                .next()
                .unwrap_or("");
            match (share.is_empty(), host.is_empty()) {
                (false, false) => format!("{share} on {host}"),
                (false, true) => share.to_string(),
                (true, false) => host.to_string(),
                (true, true) => uri.to_string(),
            }
        }
        "nfs" => match (host.is_empty(), trimmed_path.is_empty()) {
            (false, false) => format!("{host}:{trimmed_path}"),
            (false, true) => host.to_string(),
            _ => uri.to_string(),
        },
        _ => match (parts.user.as_deref(), host.is_empty()) {
            (Some(u), false) if !u.is_empty() => format!("{u}@{host}"),
            (_, false) => host.to_string(),
            _ => uri.to_string(),
        },
    };
    NetworkMount {
        name,
        scheme,
        uri: uri.to_string(),
        mount_path: mount_path_str,
    }
}

/// Merges depth-2 discovered `kio-fuse` mounts with explicitly registered `kio-fuse` mounts,
/// suppressing dismissed paths and parent authority entries already covered by a deeper mount.
pub fn merge_kio_fuse_mounts(
    discovered: Vec<NetworkMount>,
    registered: &[NetworkMount],
    dismissed: &std::collections::HashSet<String>,
) -> Vec<NetworkMount> {
    let mut out: Vec<NetworkMount> = registered
        .iter()
        .filter(|m| !dismissed.contains(&m.mount_path))
        .cloned()
        .collect();
    for item in discovered {
        if dismissed.contains(&item.mount_path) {
            continue;
        }
        let covered = out.iter().any(|existing| {
            existing.mount_path == item.mount_path
                || existing
                    .mount_path
                    .strip_prefix(&item.mount_path)
                    .is_some_and(|rest| rest.starts_with('/'))
        });
        if !covered {
            out.push(item);
        }
    }
    out
}

/// Structured failure from `gio mount` so callers know whether to fall back to `kio-fuse`
/// and how to explain the failure without raw GLib jargon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GioConnectError {
    /// GVFS daemon/module is missing, `gio` is unavailable, or no D-Bus session is reachable.
    Unavailable(String),
    /// GVFS is active, but the backend package for this URI scheme (e.g. `gvfs-nfs`) is missing.
    SchemeUnsupported(String),
    /// GVFS NFS rejected a host-only `nfs://host` URI that lacks an export path.
    NfsMissingExport(String),
    /// Remote host, credentials, timeout, or other real connection failure.
    Failed(String),
}

fn strip_gio_prefix(uri: &str, stderr: &str) -> String {
    let trimmed = stderr.trim();
    if let Some(rest) = trimmed.strip_prefix("gio:") {
        let rest = rest.trim_start();
        if let Some(after_uri) = rest.strip_prefix(uri) {
            if let Some(msg) = after_uri
                .trim_start_matches('/')
                .strip_prefix(':')
                .map(str::trim)
            {
                if !msg.is_empty() {
                    return msg.to_string();
                }
            }
        }
        if let Some(idx) = rest.find(": ") {
            let after = rest[idx + 2..].trim();
            if !after.is_empty() {
                return after.to_string();
            }
        }
        return rest.to_string();
    }
    trimmed.to_string()
}

/// Classifies `gio mount` stderr (running under `LC_ALL=C`) into a structured [`GioConnectError`].
pub fn classify_gio_connect_stderr(uri: &str, stderr: &str) -> GioConnectError {
    let normalized = stderr.replace('\u{2019}', "'");
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("volume doesn't implement mount")
        || lower.contains("cannot run gio")
        || lower.contains("no such file or directory") && lower.contains("gio")
    {
        return GioConnectError::Unavailable(strip_gio_prefix(uri, &normalized));
    }
    if lower.contains("location is not mountable") || lower.contains("operation not supported") {
        return GioConnectError::SchemeUnsupported(strip_gio_prefix(uri, &normalized));
    }
    if is_host_only_nfs_uri(uri)
        && (lower.contains("mount point does not exist") || lower.contains("permission denied"))
    {
        return GioConnectError::NfsMissingExport(strip_gio_prefix(uri, &normalized));
    }
    GioConnectError::Failed(strip_gio_prefix(uri, &normalized))
}

fn gvfs_package_hint(scheme: &str) -> &'static str {
    match scheme {
        "nfs" => "gvfs-nfs (or gvfs-backends)",
        "smb" => "gvfs-smb (or gvfs-backends)",
        "afp" => "gvfs-afp (or gvfs-backends)",
        _ => "gvfs (or gvfs-backends)",
    }
}

/// Formats an actionable, user-facing connection error combining GVFS and KIO-Fuse outcomes.
pub fn format_connect_error(
    uri: &str,
    gio_err: Option<&GioConnectError>,
    kio_err: Option<&str>,
    is_kde: bool,
) -> String {
    let parts = parse_uri_parts(uri);
    let scheme = parts
        .as_ref()
        .map(|p| p.scheme.as_str())
        .unwrap_or("network");
    let host = parts
        .as_ref()
        .and_then(|p| p.host.as_deref())
        .unwrap_or("host");
    let pkg = gvfs_package_hint(scheme);

    if let Some(kio) = kio_err.map(str::trim).filter(|s| !s.is_empty()) {
        if matches!(
            gio_err,
            None | Some(
                GioConnectError::Unavailable(_)
                    | GioConnectError::SchemeUnsupported(_)
                    | GioConnectError::NfsMissingExport(_)
            )
        ) {
            return format!("Cannot connect to {uri}: {kio}");
        }
    }

    match gio_err {
        Some(GioConnectError::Unavailable(_)) => {
            if is_kde {
                format!(
                    "Cannot connect to {uri}: neither KDE kio-fuse nor GVFS is available. Install kio-fuse or {pkg}."
                )
            } else {
                format!(
                    "Cannot connect to {uri}: GVFS is not available on this system. Install {pkg} (or kio-fuse) and ensure a D-Bus user session is running."
                )
            }
        }
        Some(GioConnectError::SchemeUnsupported(_)) => {
            format!(
                "Cannot connect to {uri}: the {scheme}:// mount backend is not installed. Install {pkg} (or kio-fuse) and try again."
            )
        }
        Some(GioConnectError::NfsMissingExport(_)) => {
            format!(
                "Cannot connect to {uri}: GVFS NFS requires the full export path (for example nfs://{host}/export), or install kio-fuse to browse server exports."
            )
        }
        Some(GioConnectError::Failed(detail)) if !detail.is_empty() => {
            format!("Cannot connect to {uri}: {detail}")
        }
        _ => {
            if let Some(kio) = kio_err.map(str::trim).filter(|s| !s.is_empty()) {
                format!("Cannot connect to {uri}: {kio}")
            } else {
                format!("Cannot connect to {uri}")
            }
        }
    }
}

/// Finds the best matching mounted local path in `mounts` for a recently connected `uri`.
pub fn find_mount_path_for_uri(mounts: &[NetworkMount], uri: &str) -> Option<String> {
    let target = parse_uri_parts(uri)?;
    let target_host = target
        .host
        .as_deref()?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let target_path = target.path.trim_end_matches('/');

    mounts
        .iter()
        .filter_map(|m| {
            let mp = parse_uri_parts(&m.uri)?;
            let scheme_pts = scheme_compatible(&mp.scheme, &target.scheme)?;
            let mh = mp
                .host
                .as_deref()?
                .trim_start_matches('[')
                .trim_end_matches(']');
            if !mh.eq_ignore_ascii_case(target_host) {
                return None;
            }
            let mut score = scheme_pts + 10;
            let mp_path = mp.path.trim_end_matches('/');
            if !target_path.is_empty() {
                if mp_path.eq_ignore_ascii_case(target_path) {
                    score += 8;
                } else if !mp_path.is_empty()
                    && !target_path
                        .strip_prefix(mp_path)
                        .is_some_and(|rest| rest.starts_with('/'))
                {
                    return None;
                }
            }
            if let (Some(mu), Some(tu)) = (mp.user.as_deref(), target.user.as_deref()) {
                if mu == tu {
                    score += 3;
                } else {
                    return None;
                }
            }
            Some((score, m.mount_path.clone()))
        })
        .max_by_key(|(s, _)| *s)
        .map(|(_, path)| path)
}

/// Candidate GVFS daemon directories on Linux (empty on other platforms).
pub fn gvfs_mount_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let mut candidates = Vec::new();
        if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
            candidates.push(PathBuf::from(rt).join("gvfs"));
        }
        // SAFETY: geteuid() is always safe and cannot fail.
        let uid = unsafe { libc::geteuid() };
        candidates.push(PathBuf::from(format!("/run/user/{uid}/gvfs")));
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".gvfs"));
        }
        let mut seen = HashSet::new();
        candidates
            .into_iter()
            .filter(|p| seen.insert(p.clone()) && p.is_dir())
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// True when `path` lies inside a GVFS mount directory on Linux.
pub fn is_gvfs_path(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let rt = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let home = dirs::home_dir();
        is_gvfs_path_with(path, rt.as_deref(), home.as_deref())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

/// True when `mount` is a direct GVFS submount path on Linux.
pub fn is_gvfs_mount_path(mount: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        let rt = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let home = dirs::home_dir();
        is_gvfs_mount_path_with(Path::new(mount), rt.as_deref(), home.as_deref())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = mount;
        false
    }
}

/// Non-blocking guard: returns true if `path` is on a GVFS, KIO-Fuse, or remote mount that must not
/// trigger file watchers, recursive index scans, or deleted-folder cleanup.
/// Performs zero blocking syscalls on `path` itself.
pub fn is_slow_or_gvfs_mount(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        is_gvfs_path(path)
            || is_kio_fuse_path(path)
            || super::index::remote_mounts()
                .iter()
                .any(|m| path.starts_with(m))
    }
    #[cfg(target_os = "macos")]
    {
        super::index::remote_mounts()
            .iter()
            .any(|m| path.starts_with(m))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = path;
        false
    }
}

#[cfg(target_os = "linux")]
fn gio_command(args: &[&str]) -> std::process::Command {
    let argv: Vec<String> = std::iter::once("gio".to_string())
        .chain(args.iter().map(|s| (*s).to_string()))
        .collect();
    let wrapped = crate::util::hostspawn::wrap_argv(argv, None, &[("LC_ALL", "C")]);
    let mut cmd = std::process::Command::new(&wrapped[0]);
    cmd.args(&wrapped[1..]);
    if !crate::util::hostspawn::in_flatpak() {
        cmd.env("LC_ALL", "C");
        for var in appimage_gio_env_removals(std::env::var_os("APPIMAGE").is_some()) {
            cmd.env_remove(var);
        }
    }
    cmd
}

#[cfg(target_os = "linux")]
fn kio_fuse_roots() -> Vec<PathBuf> {
    std::fs::read_to_string("/proc/self/mounts")
        .map(|text| parse_kio_fuse_roots(&text))
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
static REGISTERED_KIO_MOUNTS: std::sync::Mutex<Vec<NetworkMount>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(target_os = "linux")]
static DISMISSED_KIO_MOUNTS: std::sync::Mutex<Option<HashSet<String>>> =
    std::sync::Mutex::new(None);

/// Records a KIO-Fuse mount returned by `org.kde.KIOFuse.VFS.mountUrl` so it appears in the
/// Network sidebar with its exact URI and mount path (even when mounted below `<root>/<scheme>/<authority>`).
pub fn register_kio_fuse_mount(uri: &str, mount_path: &str) -> NetworkMount {
    let mount = network_mount_from_kio_uri(uri, Path::new(mount_path));
    #[cfg(target_os = "linux")]
    {
        if let Ok(mut dismissed) = DISMISSED_KIO_MOUNTS.lock() {
            if let Some(set) = dismissed.as_mut() {
                set.remove(mount_path);
            }
        }
        let mut reg = REGISTERED_KIO_MOUNTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reg.retain(|m| m.mount_path != mount.mount_path);
        reg.push(mount.clone());
    }
    mount
}

#[cfg(target_os = "linux")]
fn read_kio_fuse_mounts() -> Vec<NetworkMount> {
    let roots = kio_fuse_roots();
    if roots.is_empty() {
        if let Ok(mut reg) = REGISTERED_KIO_MOUNTS.lock() {
            reg.clear();
        }
        if let Ok(mut dismissed) = DISMISSED_KIO_MOUNTS.lock() {
            *dismissed = None;
        }
        return Vec::new();
    }

    // Only read depth 1 (<root>/<scheme>) and depth 2 (<root>/<scheme>/<authority>), which
    // kio-fuse serves strictly from its in-memory tree of already-mounted origins without touching
    // the remote server.
    let mut discovered = Vec::new();
    for root in &roots {
        let Ok(schemes) = std::fs::read_dir(root) else {
            continue;
        };
        for scheme_entry in schemes.flatten() {
            let scheme_name = scheme_entry.file_name().to_string_lossy().into_owned();
            if normalize_display_scheme(&scheme_name).is_none() {
                continue;
            }
            let Ok(authorities) = std::fs::read_dir(scheme_entry.path()) else {
                continue;
            };
            for auth_entry in authorities.flatten() {
                let auth_name = auth_entry.file_name().to_string_lossy().into_owned();
                let mount_path = auth_entry.path();
                if let Some(m) = parse_kio_fuse_entry(&scheme_name, &auth_name, &mount_path) {
                    discovered.push(m);
                }
            }
        }
    }

    let registered = {
        let mut reg = REGISTERED_KIO_MOUNTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reg.retain(|m| {
            let p = Path::new(&m.mount_path);
            roots.iter().any(|r| p.starts_with(r))
        });
        reg.clone()
    };
    let dismissed = DISMISSED_KIO_MOUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or_default();

    merge_kio_fuse_mounts(discovered, &registered, &dismissed)
}

#[cfg(target_os = "linux")]
fn read_gvfs_entries() -> Vec<ParsedGvfsEntry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for root in gvfs_mount_roots() {
        let Ok(rd) = std::fs::read_dir(&root) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let path_key = path.to_string_lossy().into_owned();
            if !seen.insert(path_key) {
                continue;
            }
            if let Some(parsed) = parse_gvfs_entry(&name, &path) {
                entries.push(parsed);
            }
        }
    }
    entries
}

#[cfg(target_os = "linux")]
fn query_gio_mount_list() -> Vec<GioMountInfo> {
    use std::io::Read;
    use std::process::Stdio;
    let Ok(mut child) = gio_command(&["mount", "-l"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Vec::new();
    };
    // Read while the child runs: a long list fills the pipe, and a child waiting to write would
    // otherwise look hung and be killed with the list lost.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    match rx.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(out) => {
            let _ = child.wait();
            parse_gio_mount_list(&out)
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Vec::new()
        }
    }
}

#[cfg(target_os = "linux")]
static GVFS_GIO_CACHE: std::sync::Mutex<Option<(Vec<String>, Vec<GioMountInfo>)>> =
    std::sync::Mutex::new(None);

#[cfg(target_os = "linux")]
fn invalidate_gio_cache() {
    if let Ok(mut guard) = GVFS_GIO_CACHE.lock() {
        *guard = None;
    }
}

#[cfg(target_os = "linux")]
fn resolve_live_gvfs() -> (Vec<Drive>, Vec<NetworkMount>) {
    let entries = read_gvfs_entries();
    let mut key: Vec<String> = entries
        .iter()
        .filter(|e| matches!(e.kind, GvfsKind::Device | GvfsKind::Network))
        .map(|e| e.mount_path.clone())
        .collect();
    if key.is_empty() {
        invalidate_gio_cache();
        return (Vec::new(), Vec::new());
    }
    key.sort();
    let gio = {
        let mut guard = GVFS_GIO_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((cached_key, cached_gio)) = guard.as_ref() {
            if *cached_key == key {
                cached_gio.clone()
            } else {
                let fresh = query_gio_mount_list();
                *guard = Some((key, fresh.clone()));
                fresh
            }
        } else {
            let fresh = query_gio_mount_list();
            *guard = Some((key, fresh.clone()));
            fresh
        }
    };
    resolve_gvfs_mounts(&entries, &gio)
}

/// Lists GVFS and KDE KIO-Fuse network mounts (`smb`, `nfs`, `sftp`, `dav`, `ftp`, `afp`).
pub fn list_network_mounts() -> Vec<NetworkMount> {
    #[cfg(target_os = "linux")]
    {
        let mut mounts = resolve_live_gvfs().1;
        let mut seen: HashSet<String> = mounts.iter().map(|m| m.mount_path.clone()).collect();
        for m in read_kio_fuse_mounts() {
            if seen.insert(m.mount_path.clone()) {
                mounts.push(m);
            }
        }
        mounts.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.mount_path.cmp(&b.mount_path))
        });
        mounts
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// Lists GVFS device mounts (`mtp`, `gphoto2`) formatted as [`Drive`] entries for the sidebar.
pub fn gvfs_drives() -> Vec<Drive> {
    #[cfg(target_os = "linux")]
    {
        resolve_live_gvfs().0
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// Runs `gio <args>` and waits at most `timeout`: an unreachable host or a credentials prompt must not
/// hold a worker for ever. Returns whether it succeeded and what it printed on stderr.
#[cfg(target_os = "linux")]
fn run_gio(args: &[&str], timeout: std::time::Duration) -> Result<(bool, String)> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = gio_command(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CoreError::Other(format!("cannot run gio: {e}")))?;
    let mut stderr = child.stderr.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(se) = stderr.as_mut() {
            let _ = se.read_to_string(&mut text);
        }
        let _ = tx.send(text);
    });
    match rx.recv_timeout(timeout) {
        Ok(text) => {
            let ok = child.wait().map(|s| s.success()).unwrap_or(false);
            Ok((ok, text.trim().to_string()))
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(CoreError::Other(format!(
                "gio did not answer within {} s",
                timeout.as_secs()
            )))
        }
    }
}

/// Mounts a remote GVFS location via `gio mount -- <uri>`, returning the local GVFS mount path
/// when discovered and a structured [`GioConnectError`] on failure.
pub fn connect_server_gvfs(uri: &str) -> std::result::Result<Option<String>, GioConnectError> {
    let valid = validate_connect_uri(uri).map_err(|e| GioConnectError::Failed(e.to_string()))?;
    let gvfs_uri = gvfs_uri_for(&valid);
    #[cfg(target_os = "linux")]
    {
        let (ok, stderr) = match run_gio(
            &["mount", "--", &gvfs_uri],
            std::time::Duration::from_secs(60),
        ) {
            Ok(out) => out,
            Err(err) => return Err(classify_gio_connect_stderr(&gvfs_uri, &err.to_string())),
        };
        invalidate_gio_cache();
        if ok || stderr.to_ascii_lowercase().contains("already mounted") {
            let mounts = resolve_live_gvfs().1;
            return Ok(find_mount_path_for_uri(&mounts, &gvfs_uri));
        }
        Err(classify_gio_connect_stderr(&gvfs_uri, &stderr))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = gvfs_uri;
        Err(GioConnectError::Failed(
            "GVFS server mounts are only supported on Linux".into(),
        ))
    }
}

/// Mounts a remote GVFS location via `gio mount -- <uri>`.
pub fn connect_server(uri: &str) -> Result<Option<String>> {
    let valid = validate_connect_uri(uri)?;
    let is_kde = std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| is_kde_desktop(&d))
        .unwrap_or(false);
    connect_server_gvfs(&valid)
        .map_err(|err| CoreError::Other(format_connect_error(&valid, Some(&err), None, is_kde)))
}

/// Disconnects a GVFS or KDE KIO-Fuse network mount.
pub fn disconnect_mount(mount_path_or_uri: &str) -> Result<()> {
    let target = mount_path_or_uri.trim();
    if target.is_empty() || target.starts_with('-') || target.contains('\0') {
        return Err(CoreError::Other("invalid mount target".into()));
    }
    #[cfg(target_os = "linux")]
    {
        if is_kio_fuse_path(Path::new(target)) {
            let mut reg = REGISTERED_KIO_MOUNTS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            reg.retain(|m| m.mount_path != target);
            let mut dismissed = DISMISSED_KIO_MOUNTS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            dismissed
                .get_or_insert_with(HashSet::new)
                .insert(target.to_string());
            return Ok(());
        }
        let (ok, stderr) = run_gio(
            &["mount", "-u", "--", target],
            std::time::Duration::from_secs(30),
        )?;
        invalidate_gio_cache();
        if ok {
            return Ok(());
        }
        Err(CoreError::Other(if stderr.is_empty() {
            format!("failed to disconnect {target}")
        } else {
            stderr
        }))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = target;
        Err(CoreError::Other(
            "GVFS mounts are only supported on Linux".into(),
        ))
    }
}

/// Ejects a GVFS device mount (`gio mount -e`), falling back to `gio mount -u` if eject
/// fails while the mount path is still present.
pub fn eject_gvfs(mount_path: &str) -> Result<()> {
    let target = mount_path.trim();
    if target.is_empty() || target.starts_with('-') || target.contains('\0') {
        return Err(CoreError::Other("invalid mount path".into()));
    }
    #[cfg(target_os = "linux")]
    {
        use std::process::Stdio;
        let eject_out = gio_command(&["mount", "-e", "--", target])
            .stdin(Stdio::null())
            .output();
        invalidate_gio_cache();
        if let Ok(o) = &eject_out {
            if o.status.success() || !Path::new(target).exists() {
                return Ok(());
            }
        }
        let unmount_out = gio_command(&["mount", "-u", "--", target])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| CoreError::Other(format!("eject failed: {e}")))?;
        if unmount_out.status.success() || !Path::new(target).exists() {
            return Ok(());
        }
        let msg = String::from_utf8_lossy(&unmount_out.stderr)
            .trim()
            .to_string();
        let fallback = eject_out
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stderr).trim().to_string())
            .unwrap_or_default();
        Err(CoreError::Other(format!(
            "eject failed: {}",
            if msg.is_empty() { fallback } else { msg }
        )))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = target;
        Err(CoreError::Other(
            "GVFS eject is only supported on Linux".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_path_prefix_matches_whole_folders_only() {
        let entry = ParsedGvfsEntry {
            dir_name: "dav:host=nas,prefix=%2Fremote".into(),
            raw_scheme: "dav".into(),
            kind: GvfsKind::Network,
            scheme: "dav".into(),
            params: vec![
                ("host".into(), "nas".into()),
                ("prefix".into(), "/remote".into()),
            ],
            mount_path: "/run/user/1000/gvfs/dav:host=nas,prefix=%2Fremote".into(),
        };
        let score = |uri: &str| match_score(&entry, &parse_uri_parts(uri).unwrap());
        assert!(score("dav://nas/remote").is_some());
        assert!(score("dav://nas/remote/x").is_some());
        assert!(
            score("dav://nas/remote2/x").is_none(),
            "/remote is not a prefix of /remote2"
        );
        assert!(score("dav://nas/other").is_none());
    }

    use super::*;

    #[test]
    fn percent_decode_preserves_plus_and_invalid_escapes() {
        assert_eq!(percent_decode("My%20Docs%2BC%23"), "My Docs+C#");
        assert_eq!(percent_decode("%C4%8Crka"), "Črka");
        assert_eq!(percent_decode("bad%2g%zz%2"), "bad%2g%zz%2");
    }

    #[test]
    fn parses_gvfs_entries_across_devices_network_and_cloud() {
        let mtp = parse_gvfs_entry(
            "mtp:host=Google_Pixel_9_123",
            Path::new("/run/user/1000/gvfs/mtp:host=Google_Pixel_9_123"),
        )
        .unwrap();
        assert_eq!(mtp.kind, GvfsKind::Device);
        assert_eq!(mtp.scheme, "mtp");
        assert_eq!(mtp.param("host"), Some("Google_Pixel_9_123"));

        let cam = parse_gvfs_entry(
            "gphoto2:host=%5Busb%3A001%2C004%5D",
            Path::new("/run/user/1000/gvfs/gphoto2:host=%5Busb%3A001%2C004%5D"),
        )
        .unwrap();
        assert_eq!(cam.kind, GvfsKind::Device);
        assert_eq!(cam.param("host"), Some("[usb:001,004]"));

        // Encoded comma and equals inside share name stay intact.
        let smb = parse_gvfs_entry(
            "smb-share:server=nas.local,share=a%2Cb%3Dc,user=matej",
            Path::new("/run/user/1000/gvfs/smb-share:server=nas.local,share=a%2Cb%3Dc,user=matej"),
        )
        .unwrap();
        assert_eq!(smb.kind, GvfsKind::Network);
        assert_eq!(smb.scheme, "smb");
        assert_eq!(smb.param("server"), Some("nas.local"));
        assert_eq!(smb.param("share"), Some("a,b=c"));
        assert_eq!(smb.param("user"), Some("matej"));

        let davs = parse_gvfs_entry(
            "dav:host=cloud.example.com,ssl=true,prefix=%2Fremote.php%2Fdav",
            Path::new("/run/user/1000/gvfs/dav:host=cloud.example.com,ssl=true,prefix=%2Fremote.php%2Fdav"),
        )
        .unwrap();
        assert_eq!(davs.kind, GvfsKind::Network);
        assert_eq!(davs.scheme, "davs");
        assert_eq!(davs.param("prefix"), Some("/remote.php/dav"));

        let gdrive = parse_gvfs_entry(
            "google-drive:host=gmail.com,user=me",
            Path::new("/run/user/1000/gvfs/google-drive:host=gmail.com,user=me"),
        )
        .unwrap();
        assert_eq!(gdrive.kind, GvfsKind::Cloud);
    }

    #[test]
    fn parses_gio_mount_list_and_resolves_mounts() {
        let gio_out = "\
Drive(0): WDC WD40EFRX
  Type: GProxyDrive (GProxyVolumeMonitorUDisks2)
  Volume(0): Pixel 9
    Type: GProxyVolume (GProxyVolumeMonitorMTP)
    Mount(0): Pixel 9 -> mtp://Google_Pixel_9_123/
      Type: GDaemonMount
Mount(1): My -> Docs on nas.local -> smb://matej@nas.local/My%20Docs/
  Type: GDaemonMount
Mount(2): Canon EOS R5 -> gphoto2://Canon_EOS_R5/
  Type: GDaemonMount
Mount(3): me@gmail.com -> google-drive://me@gmail.com/
  Type: GDaemonMount
Mount(4): boot -> file:///boot
  Type: GUnixMount
";
        let gio = parse_gio_mount_list(gio_out);
        assert_eq!(gio.len(), 5);
        assert_eq!(gio[0].name, "Pixel 9");
        assert_eq!(gio[0].uri, "mtp://Google_Pixel_9_123/");
        assert_eq!(gio[0].mount_type, "GDaemonMount");
        assert_eq!(gio[1].name, "My -> Docs on nas.local");
        assert_eq!(gio[1].uri, "smb://matej@nas.local/My%20Docs/");

        let entries = vec![
            parse_gvfs_entry(
                "mtp:host=Google_Pixel_9_123",
                Path::new("/run/user/1000/gvfs/mtp:host=Google_Pixel_9_123"),
            )
            .unwrap(),
            parse_gvfs_entry(
                "gphoto2:host=%5Busb%3A001%2C004%5D",
                Path::new("/run/user/1000/gvfs/gphoto2:host=%5Busb%3A001%2C004%5D"),
            )
            .unwrap(),
            parse_gvfs_entry(
                "smb-share:server=nas.local,share=My%20Docs,user=matej",
                Path::new(
                    "/run/user/1000/gvfs/smb-share:server=nas.local,share=My%20Docs,user=matej",
                ),
            )
            .unwrap(),
            parse_gvfs_entry(
                "sftp:host=srv.example,user=alice,port=2222",
                Path::new("/run/user/1000/gvfs/sftp:host=srv.example,user=alice,port=2222"),
            )
            .unwrap(),
            parse_gvfs_entry(
                "google-drive:host=gmail.com,user=me",
                Path::new("/run/user/1000/gvfs/google-drive:host=gmail.com,user=me"),
            )
            .unwrap(),
        ];

        let (drives, network) = resolve_gvfs_mounts(&entries, &gio);
        assert_eq!(drives.len(), 2);
        assert_eq!(drives[0].short, "Canon EOS R5");
        assert!(drives[0].removable);
        assert_eq!((drives[0].total, drives[0].available), (0, 0));
        assert_eq!(drives[0].label, "");
        assert_eq!(drives[1].short, "Pixel 9");

        assert_eq!(network.len(), 2);
        assert_eq!(network[0].name, "alice@srv.example");
        assert_eq!(network[0].scheme, "sftp");
        assert_eq!(network[0].uri, "sftp://alice@srv.example:2222");
        assert_eq!(network[1].name, "My -> Docs on nas.local");
        assert_eq!(network[1].scheme, "smb");
        assert_eq!(network[1].uri, "smb://matej@nas.local/My%20Docs/");
    }

    #[test]
    fn gvfs_path_and_mount_path_detection() {
        let rt = Some(Path::new("/tmp/custom-rt"));
        let home = Some(Path::new("/home/u"));

        assert!(is_gvfs_path_with(
            Path::new("/tmp/custom-rt/gvfs/smb-share:server=nas,share=x"),
            rt,
            home
        ));
        assert!(is_gvfs_path_with(
            Path::new("/run/user/1000/gvfs/mtp:host=Phone/DCIM"),
            None,
            None
        ));
        assert!(is_gvfs_path_with(
            Path::new("/home/u/.gvfs/sftp:host=a"),
            rt,
            home
        ));
        assert!(!is_gvfs_path_with(
            Path::new("/run/user/1000/gvfs2/x"),
            None,
            None
        ));
        assert!(!is_gvfs_path_with(
            Path::new("/run/user/abc/gvfs/x"),
            None,
            None
        ));

        assert!(is_gvfs_mount_path_with(
            Path::new("/run/user/1000/gvfs/mtp:host=Phone"),
            None,
            None
        ));
        assert!(!is_gvfs_mount_path_with(
            Path::new("/run/user/1000/gvfs/mtp:host=Phone/DCIM"),
            None,
            None
        ));
        assert!(!is_gvfs_mount_path_with(
            Path::new("/run/user/1000/gvfs"),
            None,
            None
        ));
    }

    #[test]
    fn validate_connect_uri_checks_scheme_host_and_safety() {
        assert_eq!(
            validate_connect_uri("  smb://nas.local/share  ").unwrap(),
            "smb://nas.local/share"
        );
        assert_eq!(
            validate_connect_uri("sftp://user@host:2222/home").unwrap(),
            "sftp://user@host:2222/home"
        );
        assert!(
            validate_connect_uri("smb://user:secret@nas/share").is_err(),
            "no password in the address"
        );
        assert!(validate_connect_uri("").is_err());
        assert!(validate_connect_uri("-u smb://nas/share").is_err());
        assert!(validate_connect_uri("file:///etc/passwd").is_err());
        assert!(validate_connect_uri("nas.local/share").is_err());
        assert!(validate_connect_uri("smb:///share").is_err());
        assert!(validate_connect_uri("smb://:445/share").is_err());
        assert!(validate_connect_uri("sftp://[fe80::1]:2222/home").is_ok());
        assert!(validate_connect_uri("smb://[fe80::1/share").is_err());
        assert!(validate_connect_uri("smb://[]/share").is_err());
        assert!(validate_connect_uri("smb://[fe80::1]junk/share").is_err());
        assert!(validate_connect_uri("smb://nas/share\nrm").is_err());
    }

    #[test]
    fn resolves_ipv6_gvfs_mount_with_gio_name() {
        let entry = parse_gvfs_entry(
            "sftp:host=%5Bfe80%3A%3A1%5D,port=2222",
            Path::new("/run/user/1000/gvfs/sftp:host=%5Bfe80%3A%3A1%5D,port=2222"),
        )
        .unwrap();
        let gio = parse_gio_mount_list(
            "Mount(0): Lab Box -> sftp://[fe80::1]:2222/\n  Type: GDaemonMount\n",
        );
        let (_drives, network) = resolve_gvfs_mounts(&[entry], &gio);
        assert_eq!(network.len(), 1);
        assert_eq!(network[0].name, "Lab Box");
        assert_eq!(network[0].uri, "sftp://[fe80::1]:2222/");
    }

    #[test]
    fn backend_preference_and_uri_scheme_mapping() {
        assert!(is_kde_desktop("KDE"));
        assert!(is_kde_desktop("fedora:KDE"));
        assert!(!is_kde_desktop("GNOME"));
        assert!(!is_kde_desktop("Hyprland"));
        assert_eq!(preferred_connect_backend("KDE"), ConnectBackend::KioFuse);
        assert_eq!(
            preferred_connect_backend("fedora:KDE"),
            ConnectBackend::KioFuse
        );
        assert_eq!(preferred_connect_backend("Hyprland"), ConnectBackend::Gvfs);
        assert_eq!(preferred_connect_backend(""), ConnectBackend::Gvfs);

        assert_eq!(
            kio_fuse_uri_for("nfs://192.168.0.101"),
            "nfs://192.168.0.101"
        );
        assert_eq!(
            kio_fuse_uri_for("davs://cloud.example/remote.php/dav"),
            "webdavs://cloud.example/remote.php/dav"
        );
        assert_eq!(kio_fuse_uri_for("dav://nas/dav"), "webdav://nas/dav");
        assert_eq!(kio_fuse_uri_for("ssh://alice@srv"), "sftp://alice@srv");

        assert_eq!(
            gvfs_uri_for("webdavs://cloud.example/dav"),
            "davs://cloud.example/dav"
        );
        assert_eq!(gvfs_uri_for("webdav://nas/dav"), "dav://nas/dav");
        assert_eq!(gvfs_uri_for("nfs://192.168.0.101"), "nfs://192.168.0.101");

        assert!(is_host_only_nfs_uri("nfs://192.168.0.101"));
        assert!(is_host_only_nfs_uri("nfs://192.168.0.101/"));
        assert!(!is_host_only_nfs_uri("nfs://192.168.0.101/mnt/mx500"));
        assert!(!is_host_only_nfs_uri("smb://192.168.0.101"));

        assert!(validate_connect_uri("webdavs://cloud.example/dav").is_ok());
        assert!(validate_connect_uri("webdav://nas/dav").is_ok());
        assert!(validate_connect_uri("nfs://192.168.0.101").is_ok());
    }

    #[test]
    fn kio_fuse_paths_roots_entries_and_merging() {
        let rt = Some(Path::new("/tmp/rt"));
        assert!(is_kio_fuse_path_with(
            Path::new("/tmp/rt/kio-fuse-AbCdEf/nfs/192.168.0.101"),
            rt
        ));
        assert!(is_kio_fuse_path_with(
            Path::new("/run/user/1000/kio-fuse-xyz123/nfs/192.168.0.101/mnt/mx500"),
            None
        ));
        assert!(!is_kio_fuse_path_with(
            Path::new("/run/user/1000/gvfs/nfs:host=192.168.0.101"),
            None
        ));
        assert!(!is_kio_fuse_path_with(
            Path::new("/run/user/1000/kio-fuse2/nfs/x"),
            None
        ));

        let mounts_txt = "\
kio-fuse /run/user/1000/kio-fuse-AbCdEf fuse.kio-fuse rw,nosuid,nodev 0 0\n\
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0\n\
kio-fuse /run/user/1000/kio-fuse\\040test fuse.kio-fuse rw 0 0\n";
        let roots = parse_kio_fuse_roots(mounts_txt);
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/run/user/1000/kio-fuse-AbCdEf"),
                PathBuf::from("/run/user/1000/kio-fuse test"),
            ]
        );

        let nfs_host = parse_kio_fuse_entry(
            "nfs",
            "192.168.0.101",
            Path::new("/run/user/1000/kio-fuse-AbCdEf/nfs/192.168.0.101"),
        )
        .unwrap();
        assert_eq!(nfs_host.name, "192.168.0.101");
        assert_eq!(nfs_host.scheme, "nfs");
        assert_eq!(nfs_host.uri, "nfs://192.168.0.101");

        let webdav = parse_kio_fuse_entry(
            "webdavs",
            "cloud.example",
            Path::new("/run/user/1000/kio-fuse-AbCdEf/webdavs/cloud.example"),
        )
        .unwrap();
        assert_eq!(webdav.scheme, "davs");
        assert!(parse_kio_fuse_entry("trash", "foo", Path::new("/x")).is_none());

        let registered_smb = network_mount_from_kio_uri(
            "smb://nas.local/Media",
            Path::new("/run/user/1000/kio-fuse-AbCdEf/smb/nas.local/Media"),
        );
        assert_eq!(registered_smb.name, "Media on nas.local");
        assert_eq!(registered_smb.scheme, "smb");

        let bare_smb = parse_kio_fuse_entry(
            "smb",
            "nas.local",
            Path::new("/run/user/1000/kio-fuse-AbCdEf/smb/nas.local"),
        )
        .unwrap();
        let dismissed: std::collections::HashSet<String> =
            [webdav.mount_path.clone()].into_iter().collect();
        let merged = merge_kio_fuse_mounts(
            vec![nfs_host.clone(), bare_smb, webdav],
            std::slice::from_ref(&registered_smb),
            &dismissed,
        );
        assert_eq!(merged.len(), 2);
        assert!(merged.contains(&registered_smb));
        assert!(merged.contains(&nfs_host));
    }

    #[test]
    fn gio_error_classification_formatting_and_appimage_env_hygiene() {
        assert!(appimage_gio_env_removals(false).is_empty());
        let removed = appimage_gio_env_removals(true);
        assert!(removed.contains(&"LD_LIBRARY_PATH"));
        assert!(removed.contains(&"GIO_MODULE_DIR"));
        assert!(removed.contains(&"APPIMAGE"));

        let unavail_ascii = classify_gio_connect_stderr(
            "nfs://192.168.0.101",
            "gio: nfs://192.168.0.101: volume doesn't implement mount",
        );
        assert!(matches!(unavail_ascii, GioConnectError::Unavailable(_)));

        let unavail_curly = classify_gio_connect_stderr(
            "nfs://192.168.0.101",
            "gio: nfs://192.168.0.101: volume doesn\u{2019}t implement mount",
        );
        assert!(matches!(unavail_curly, GioConnectError::Unavailable(_)));

        let unsupported = classify_gio_connect_stderr(
            "davs://127.0.0.1/dav",
            "gio: davs://127.0.0.1/dav: Location is not mountable",
        );
        assert!(matches!(unsupported, GioConnectError::SchemeUnsupported(_)));

        let missing_export = classify_gio_connect_stderr(
            "nfs://192.168.0.101",
            "gio: nfs://192.168.0.101/: Mount point does not exist",
        );
        assert!(matches!(
            missing_export,
            GioConnectError::NfsMissingExport(_)
        ));

        let with_export = classify_gio_connect_stderr(
            "nfs://192.168.0.101/mnt/mx500",
            "gio: nfs://192.168.0.101/mnt/mx500: Mount point does not exist",
        );
        assert_eq!(
            with_export,
            GioConnectError::Failed("Mount point does not exist".into())
        );

        let kde_msg = format_connect_error("nfs://192.168.0.101", Some(&unavail_ascii), None, true);
        assert!(kde_msg.contains("kio-fuse"));
        assert!(kde_msg.contains("gvfs-nfs"));

        let export_msg =
            format_connect_error("nfs://192.168.0.101", Some(&missing_export), None, false);
        assert!(export_msg.contains("nfs://192.168.0.101/export"));

        let kio_real_err = format_connect_error(
            "nfs://192.168.0.101",
            Some(&unavail_ascii),
            Some("Could not connect to host 192.168.0.101"),
            true,
        );
        assert_eq!(
            kio_real_err,
            "Cannot connect to nfs://192.168.0.101: Could not connect to host 192.168.0.101"
        );

        let mounts = vec![NetworkMount {
            name: "192.168.0.101:/mnt/mx500".into(),
            scheme: "nfs".into(),
            uri: "nfs://192.168.0.101/mnt/mx500".into(),
            mount_path: "/run/user/1000/gvfs/nfs:host=192.168.0.101,path=%2Fmnt%2Fmx500".into(),
        }];
        assert_eq!(
            find_mount_path_for_uri(&mounts, "nfs://192.168.0.101/mnt/mx500").as_deref(),
            Some("/run/user/1000/gvfs/nfs:host=192.168.0.101,path=%2Fmnt%2Fmx500")
        );
    }
}
