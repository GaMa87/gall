use serde::Serialize;
use std::fs::Metadata;
use std::path::Path;
use std::time::SystemTime;

/// Broad item category, used for icons, preview selection and sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

/// One item in a folder listing. Times are Unix epoch milliseconds so the
/// structure serialises compactly for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    /// True when this is a directory, or a symlink that resolves to one.
    pub is_dir: bool,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<i64>,
    pub created: Option<i64>,
    /// Lower-case extension without the dot; empty for folders / no extension.
    pub ext: String,
    pub readonly: bool,
    /// Kept only in the cloud (see [`is_cloud_only`]): no thumbnail or excerpt, which
    /// would download it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cloud_only: bool,
    /// Unix only: `st_mode` (file type and permission bits) of the item itself, not of a
    /// symlink's target, as `ls -l` shows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    /// Unix only: the owner's user name (the numeric uid when it has none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Where a symlink (Windows: also a junction) points, as written in the link
    /// (`usr/bin`, `C:\Users`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_target: Option<String>,
    /// Windows only: the item's own file attributes (archive, read-only, hidden, system,
    /// reparse point), shown like PowerShell's `Mode` column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<u32>,
}

pub fn to_millis(t: std::io::Result<SystemTime>) -> Option<i64> {
    let t = t.ok()?;
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => Some(d.as_millis() as i64),
        Err(e) => Some(-(e.duration().as_millis() as i64)),
    }
}

/// Lower-case extension of a file name. Dot-files such as `.bashrc` have no
/// extension; `archive.tar.gz` yields `gz`.
pub fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(0) | None => String::new(),
        Some(i) if i == name.len() - 1 => String::new(),
        Some(i) => name[i + 1..].to_lowercase(),
    }
}

#[cfg(windows)]
fn hidden_attr(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
fn hidden_attr(_meta: &Metadata) -> bool {
    false
}

/// A cloud file that is not on this computer: reading it downloads it. Windows: OneDrive
/// Files On-Demand, Dropbox, iCloud (recall attributes). macOS: iCloud Drive and File
/// Provider apps (OneDrive, Dropbox) mark such files "dataless". Linux cloud clients are
/// FUSE mounts, which searches skip as a whole (see `index::remote_mounts`).
pub fn is_cloud_only(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
        const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
        const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
        meta.file_attributes()
            & (FILE_ATTRIBUTE_OFFLINE
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
            != 0
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        const SF_DATALESS: u32 = 0x4000_0000;
        meta.st_flags() & SF_DATALESS != 0
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = meta;
        false
    }
}

/// User name for a uid, looked up once per uid (`getpwuid_r`, so LDAP / sssd users too).
#[cfg(unix)]
pub fn user_name(uid: u32) -> String {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(n) = cache.lock().unwrap().get(&uid) {
        return n.clone();
    }
    let name = (|| {
        let mut buf = vec![0u8; 4096];
        // SAFETY: zeroed passwd is a valid out-parameter; buf outlives the call and we copy
        // pw_name out of it before it is dropped.
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        let rc =
            unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut out) };
        if rc != 0 || out.is_null() || pw.pw_name.is_null() {
            return None;
        }
        let n = unsafe { std::ffi::CStr::from_ptr(pw.pw_name) };
        Some(n.to_string_lossy().into_owned())
    })()
    .unwrap_or_else(|| uid.to_string());
    cache.lock().unwrap().insert(uid, name.clone());
    name
}

/// Windows: where a symlink or junction points (`read_link` reads both), without the
/// `\\?\` prefix junctions are stored with. Other reparse points (OneDrive, dedup) have none.
#[cfg(windows)]
fn windows_link_target(path: &Path, link_meta: &Metadata) -> Option<String> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    if link_meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        return None;
    }
    let t = std::fs::read_link(path).ok()?;
    let t = t.to_string_lossy();
    Some(t.strip_prefix(r"\\?\").unwrap_or(&t).to_string())
}

pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}

impl FileEntry {
    /// Builds an entry from a path and its `symlink_metadata`. Symlinks are
    /// followed once to decide whether they behave like a directory.
    pub fn from_meta(path: &Path, link_meta: &Metadata) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let is_symlink = link_meta.file_type().is_symlink();
        let target_meta = if is_symlink {
            std::fs::metadata(path).ok()
        } else {
            None
        };
        let meta = target_meta.as_ref().unwrap_or(link_meta);
        let is_dir = meta.is_dir();
        let kind = if is_symlink {
            EntryKind::Symlink
        } else if is_dir {
            EntryKind::Dir
        } else if meta.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        #[cfg(unix)]
        let (mode, owner, link_target) = {
            use std::os::unix::fs::MetadataExt;
            (
                Some(link_meta.mode()),
                Some(user_name(link_meta.uid())),
                if is_symlink {
                    std::fs::read_link(path)
                        .ok()
                        .map(|t| t.to_string_lossy().into_owned())
                } else {
                    None
                },
            )
        };
        #[cfg(windows)]
        let (mode, owner, link_target) = (None, None, windows_link_target(path, link_meta));
        #[cfg(not(any(unix, windows)))]
        let (mode, owner, link_target) = (None, None, None);
        #[cfg(windows)]
        let attributes = {
            use std::os::windows::fs::MetadataExt;
            Some(link_meta.file_attributes())
        };
        #[cfg(not(windows))]
        let attributes = None;
        FileEntry {
            attributes,
            mode,
            owner,
            link_target,
            ext: if is_dir {
                String::new()
            } else {
                extension_of(&name)
            },
            is_hidden: is_hidden_name(&name) || hidden_attr(link_meta),
            size: if is_dir { 0 } else { meta.len() },
            modified: to_millis(meta.modified()),
            created: to_millis(meta.created()),
            readonly: meta.permissions().readonly(),
            cloud_only: !is_dir && is_cloud_only(meta),
            path: path.to_string_lossy().into_owned(),
            name,
            kind,
            is_dir,
            is_symlink,
        }
    }

    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let m = std::fs::symlink_metadata(path)?;
        Ok(Self::from_meta(path, &m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(extension_of("a.TXT"), "txt");
        assert_eq!(extension_of("archive.tar.gz"), "gz");
        assert_eq!(extension_of(".bashrc"), "");
        assert_eq!(extension_of("noext"), "");
        assert_eq!(extension_of("trailing."), "");
    }
}
