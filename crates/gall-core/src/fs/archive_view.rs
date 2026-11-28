//! Archives as read-only folders: `C:\x\photos.zip\2024\a.jpg` lists and previews
//! without extracting the whole archive. Listings come from the archive's index
//! (zip and 7z keep one; tar streams are read once), cached per archive version.
//! Copying, opening and previewing an entry extract just that entry (or folder) into
//! a cache folder, whose real paths then go through the normal file operations.

use super::archive::{detect, local_name, safe_relative, ArchiveFormat};
use super::entry::{extension_of, is_hidden_name, EntryKind, FileEntry};
use crate::error::{CoreError, IoContext, Result};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf, MAIN_SEPARATOR};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime};

/// One item of an archive (folders implied by file paths included).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Path inside the archive, `/`-separated, without a trailing slash.
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Epoch milliseconds.
    pub modified: Option<i64>,
}

/// Formats that can be browsed (single-file .gz/.xz/… cannot: they hold one stream).
pub fn browsable(format: ArchiveFormat) -> bool {
    use ArchiveFormat::*;
    matches!(format, Zip | SevenZ | Tar | TarGz | TarXz | TarBz2 | TarZst)
}

/// Splits a path that leads into an archive into (archive file, inner path). The inner
/// path is empty for the archive itself. None for ordinary paths (an existing folder is
/// never inside an archive, so ordinary listings pay one `is_dir` only).
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    if path.is_dir() {
        return None;
    }
    for anc in path.ancestors() {
        if anc.as_os_str().is_empty() {
            break;
        }
        if anc.is_dir() {
            return None; // reached real folders without passing an archive
        }
        if anc.is_file() {
            let (f, _) = detect(anc)?;
            if !browsable(f) {
                return None;
            }
            let rest = path.strip_prefix(anc).ok()?;
            let inner = rest
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            return Some((anc.to_path_buf(), inner));
        }
    }
    None
}

/// Is `path` an archive Gall can browse (by name)?
pub fn is_browsable_file(path: &Path) -> bool {
    detect(path).is_some_and(|(f, _)| browsable(f)) && path.is_file()
}

// ───────────────────────────── index ─────────────────────────────

type Stamp = (u64, Option<SystemTime>);

fn stamp(archive: &Path) -> Result<Stamp> {
    let m = fs::metadata(archive).ctx("stat", archive)?;
    Ok((m.len(), m.modified().ok()))
}

/// What is known about one version of an archive.
struct Indexed {
    stamp: Stamp,
    items: Arc<Vec<Item>>,
    /// No file name occurs twice in the archive. Then the first entry of a name is the
    /// only one, and reading can stop once every wanted file was seen; with a repeated
    /// name the last entry is the one that stays when the archive is extracted.
    unique_files: bool,
    /// When it was last asked for (a counter), to drop the one unused longest.
    used: u64,
}

/// How many archives keep their list in memory. The list of a compressed tar archive
/// costs a pass over the whole file, so going back to an archive must not read it again.
const INDEX_CACHE: usize = 32;

type IndexCache = Mutex<HashMap<PathBuf, Indexed>>;

fn cache() -> &'static IndexCache {
    static C: OnceLock<IndexCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn next_use() -> u64 {
    static USED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    USED.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// All items of `archive`, folders included, sorted by name. Cached until it changes.
pub fn index(archive: &Path) -> Result<Arc<Vec<Item>>> {
    Ok(index_info(archive)?.0)
}

/// [`index`] with whether every file name of the archive occurs once.
fn index_info(archive: &Path) -> Result<(Arc<Vec<Item>>, bool)> {
    let st = stamp(archive)?;
    if let Some(known) = cache().lock().unwrap().get_mut(archive) {
        if known.stamp == st {
            known.used = next_use();
            return Ok((known.items.clone(), known.unique_files));
        }
    }
    let (format, _) = detect(archive).ok_or_else(|| not_archive(archive))?;
    let raw = read_index(archive, format)?;
    let unique_files = {
        let mut names = HashSet::with_capacity(raw.len());
        raw.iter()
            .filter(|it| !it.is_dir)
            .all(|it| names.insert(it.name.as_str()))
    };
    let items = Arc::new(with_folders(raw));
    let mut c = cache().lock().unwrap();
    c.insert(
        archive.to_path_buf(),
        Indexed {
            stamp: st,
            items: items.clone(),
            unique_files,
            used: next_use(),
        },
    );
    while c.len() > INDEX_CACHE {
        let Some(oldest) = c
            .iter()
            .min_by_key(|(_, known)| known.used)
            .map(|(path, _)| path.clone())
        else {
            break;
        };
        c.remove(&oldest);
    }
    Ok((items, unique_files))
}

// ───────────────────────────── passwords ─────────────────────────────

/// Start of the error for an archive that needs a password; the UI then asks for it.
/// Full form: `archive-password:needed:<archive>` or `archive-password:wrong:<archive>`.
pub const PASSWORD_PREFIX: &str = "archive-password:";

/// Passwords typed for encrypted archives this run (memory only, never saved).
fn passwords() -> &'static Mutex<HashMap<PathBuf, String>> {
    static P: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// Remembers the password for `archive` (until Gall exits) and reads it again.
pub fn set_password(archive: &Path, password: &str) {
    passwords()
        .lock()
        .unwrap()
        .insert(archive.to_path_buf(), password.to_string());
    cache().lock().unwrap().remove(archive);
}

pub(crate) fn password_for(archive: &Path) -> Option<String> {
    passwords().lock().unwrap().get(archive).cloned()
}

pub(crate) fn sevenz_password(archive: &Path) -> sevenz_rust2::Password {
    password_for(archive)
        .map(|p| sevenz_rust2::Password::new(&p))
        .unwrap_or_else(sevenz_rust2::Password::empty)
}

fn need_password(archive: &Path) -> CoreError {
    let kind = if password_for(archive).is_some() {
        "wrong"
    } else {
        "needed"
    };
    CoreError::Other(format!("{PASSWORD_PREFIX}{kind}:{}", archive.display()))
}

/// A 7z error, as "ask for the password" where that is the cause.
pub(crate) fn sevenz_err(archive: &Path, e: sevenz_rust2::Error) -> CoreError {
    match e {
        sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_) => {
            need_password(archive)
        }
        e => CoreError::Other(format!("7z: {e}")),
    }
}

/// A zip error, as "ask for the password" where that is the cause.
pub(crate) fn zip_err(archive: &Path, e: zip::result::ZipError) -> CoreError {
    use zip::result::ZipError;
    match e {
        ZipError::InvalidPassword => need_password(archive),
        ZipError::UnsupportedArchive(m) if m == ZipError::PASSWORD_REQUIRED => {
            need_password(archive)
        }
        e => CoreError::Other(format!("zip: {e}")),
    }
}

/// An entry of a zip archive, decrypted with the remembered password when it is encrypted.
pub(crate) fn zip_entry<'a, R: Read + io::Seek>(
    z: &'a mut zip::ZipArchive<R>,
    i: usize,
    archive: &Path,
) -> Result<zip::read::ZipFile<'a, R>> {
    match password_for(archive) {
        Some(p) => z.by_index_decrypt(i, p.as_bytes()),
        None => z.by_index(i),
    }
    .map_err(|e| zip_err(archive, e))
}

/// A buffered file that keeps its buffer when a seek lands inside it. The zip reader
/// seeks before nearly every read (once per entry of the directory), and a plain
/// `BufReader` throws its buffer away on each seek, which leaves one system call per read.
pub(crate) struct SeekBuffered {
    inner: io::BufReader<fs::File>,
    at: u64,
}

impl SeekBuffered {
    pub(crate) fn new(file: fs::File) -> Self {
        Self {
            inner: io::BufReader::with_capacity(64 * 1024, file),
            at: 0,
        }
    }
}

impl Read for SeekBuffered {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.at += n as u64;
        Ok(n)
    }
}

impl io::Seek for SeekBuffered {
    fn seek(&mut self, to: io::SeekFrom) -> io::Result<u64> {
        let by = match to {
            io::SeekFrom::Start(n) => i64::try_from(n as i128 - self.at as i128).ok(),
            io::SeekFrom::Current(by) => Some(by),
            io::SeekFrom::End(_) => None,
        };
        match by.and_then(|by| Some((by, self.at.checked_add_signed(by)?))) {
            Some((by, at)) => {
                self.inner.seek_relative(by)?;
                self.at = at;
            }
            // From the end, or out of range: the file answers (and reports the error).
            None => self.at = self.inner.seek(to)?,
        }
        Ok(self.at)
    }
}

fn not_archive(p: &Path) -> CoreError {
    CoreError::Other(format!("{} is not an archive Gall can browse", p.display()))
}

/// `a/b/c.txt` → the item plus folders `a` and `a/b`; duplicates merged, sorted.
fn with_folders(raw: Vec<Item>) -> Vec<Item> {
    let mut all: HashMap<String, Item> = HashMap::with_capacity(raw.len() + raw.len() / 8);
    for it in raw {
        let mut parent = it.name.as_str();
        while let Some((p, _)) = parent.rsplit_once('/') {
            // A folder that is known came with all the folders above it.
            if all.contains_key(p) {
                break;
            }
            all.insert(
                p.to_string(),
                Item {
                    name: p.to_string(),
                    is_dir: true,
                    size: 0,
                    modified: None,
                },
            );
            parent = p;
        }
        match all.get_mut(&it.name) {
            // An explicit folder entry brings its date to an implied one.
            Some(e) if it.is_dir => e.modified = it.modified.or(e.modified),
            Some(_) => {}
            None => {
                all.insert(it.name.clone(), it);
            }
        }
    }
    let mut items: Vec<Item> = all.into_values().collect();
    items.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    items
}

/// `/`-joined normal components, or None for names that climb out or are absolute.
fn clean(name: &str) -> Option<String> {
    let name = name.trim_end_matches('/');
    // Nearly every name is already in this form; an archive has one per entry.
    let plain = !name.is_empty()
        && !name.contains(['\\', ':'])
        && name.split('/').all(|part| !matches!(part, "" | "." | ".."));
    if plain {
        return Some(name.to_string());
    }
    clean_checked(name)
}

/// [`clean`] for any name: through the path components of this platform.
fn clean_checked(name: &str) -> Option<String> {
    let rel = safe_relative(name.trim_end_matches('/'))?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn read_index(archive: &Path, format: ArchiveFormat) -> Result<Vec<Item>> {
    let file = fs::File::open(archive).ctx("open", archive)?;
    let err = |what: &str, e: &dyn std::fmt::Display| CoreError::Other(format!("{what}: {e}"));
    let mut out = Vec::new();
    match format {
        ArchiveFormat::Zip => {
            // The list comes from the directory at the end of the file alone: asking for
            // an entry itself would seek to it and read its header, once per entry.
            let z = zip::ZipArchive::new(SeekBuffered::new(file)).map_err(|e| err("zip", &e))?;
            // Files packed together often carry the same date; converting one to local
            // time is the dearest step per entry.
            let mut last_time: Option<(zip::DateTime, Option<i64>)> = None;
            let mut time_of = |d: zip::DateTime| match last_time {
                Some((known, millis)) if known == d => millis,
                _ => {
                    let millis = zip_time(d);
                    last_time = Some((d, millis));
                    millis
                }
            };
            for i in 0..z.len() {
                let f = z.by_index_data(i).map_err(|e| err("zip", &e))?;
                let Some(name) = f.enclosed_name().and_then(|p| clean(&p.to_string_lossy())) else {
                    continue;
                };
                out.push(Item {
                    name,
                    is_dir: f.is_dir(),
                    size: f.size(),
                    modified: f.last_modified().and_then(&mut time_of),
                });
            }
        }
        ArchiveFormat::SevenZ => {
            // Archives with encrypted headers need the password just to list them.
            let ar = sevenz_rust2::ArchiveReader::new(file, sevenz_password(archive))
                .map_err(|e| sevenz_err(archive, e))?;
            for f in &ar.archive().files {
                let Some(name) = clean(f.name()) else {
                    continue;
                };
                let modified = f
                    .has_last_modified_date
                    .then(|| super::entry::to_millis(Ok(SystemTime::from(f.last_modified_date()))));
                out.push(Item {
                    name,
                    is_dir: f.is_directory(),
                    size: f.size(),
                    modified: modified.flatten(),
                });
            }
        }
        _ => {
            let mut ar = tar::Archive::new(tar_stream(file, format)?);
            for e in ar.entries().map_err(|e| err("tar", &e))? {
                let e = e.map_err(|e| err("tar", &e))?;
                let Ok(p) = e.path() else { continue };
                let Some(name) = clean(&p.to_string_lossy()) else {
                    continue;
                };
                let h = e.header();
                let kind = h.entry_type();
                if !(kind.is_file() || kind.is_dir()) {
                    continue; // links, devices: not offered
                }
                out.push(Item {
                    name,
                    is_dir: kind.is_dir(),
                    size: h.size().unwrap_or(0),
                    modified: h.mtime().ok().map(|s| s as i64 * 1000),
                });
            }
        }
    }
    Ok(out)
}

fn zip_time(d: zip::DateTime) -> Option<i64> {
    use chrono::{Local, NaiveDate, TimeZone};
    let n = NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32)?
        .and_hms_opt(d.hour() as u32, d.minute() as u32, d.second() as u32)?;
    Some(Local.from_local_datetime(&n).earliest()?.timestamp_millis())
}

/// The decompressed tar stream of a (compressed) tar archive.
fn tar_stream(file: fs::File, format: ArchiveFormat) -> Result<Box<dyn Read>> {
    let r = io::BufReader::new(file);
    Ok(match format {
        ArchiveFormat::Tar => Box::new(r),
        ArchiveFormat::TarGz => Box::new(flate2::read::MultiGzDecoder::new(r)),
        ArchiveFormat::TarXz => Box::new(lzma_rust2::XzReader::new(r, true)),
        ArchiveFormat::TarBz2 => Box::new(bzip2_rs::DecoderReader::new(r)),
        ArchiveFormat::TarZst => Box::new(
            ruzstd::decoding::StreamingDecoder::new(r)
                .map_err(|e| CoreError::Other(format!("zstd: {e}")))?,
        ),
        _ => return Err(CoreError::Other("not a tar archive".into())),
    })
}

// ───────────────────────────── listing ─────────────────────────────

/// The virtual path of an item: the archive path, then the inner path with the
/// system's separator.
pub fn virtual_path(archive: &Path, inner: &str) -> String {
    let mut s = archive.to_string_lossy().into_owned();
    if !inner.is_empty() {
        s.push(MAIN_SEPARATOR);
        s.push_str(&inner.replace('/', std::path::MAIN_SEPARATOR_STR));
    }
    s
}

fn entry(archive: &Path, it: &Item) -> FileEntry {
    let name = it.name.rsplit('/').next().unwrap_or(&it.name).to_string();
    FileEntry {
        ext: if it.is_dir {
            String::new()
        } else {
            extension_of(&name)
        },
        is_hidden: is_hidden_name(&name),
        size: it.size,
        modified: it.modified,
        created: None,
        readonly: true,
        cloud_only: false,
        mode: None,
        owner: None,
        link_target: None,
        attributes: None,
        path: virtual_path(archive, &it.name),
        kind: if it.is_dir {
            EntryKind::Dir
        } else {
            EntryKind::File
        },
        is_dir: it.is_dir,
        is_symlink: false,
        name,
    }
}

/// The items directly inside `inner` ("" = the archive's top level).
pub fn list(archive: &Path, inner: &str) -> Result<Vec<FileEntry>> {
    let items = index(archive)?;
    if !inner.is_empty() && !items.iter().any(|i| i.is_dir && i.name == inner) {
        return Err(CoreError::NotFound(PathBuf::from(virtual_path(
            archive, inner,
        ))));
    }
    Ok(items
        .iter()
        .filter(|i| parent_of(&i.name) == inner)
        .map(|i| entry(archive, i))
        .collect())
}

fn parent_of(name: &str) -> &str {
    name.rsplit_once('/').map_or("", |(p, _)| p)
}

/// The entry for one virtual path (None if the archive has no such item).
pub fn stat(archive: &Path, inner: &str) -> Result<Option<FileEntry>> {
    Ok(index(archive)?
        .iter()
        .find(|i| i.name == inner)
        .map(|i| entry(archive, i)))
}

/// How many items a folder inside the archive holds, and their total size.
pub fn folder_totals(archive: &Path, inner: &str) -> Result<(usize, u64)> {
    let items = index(archive)?;
    let pre = format!("{inner}/");
    let mut n = 0;
    let mut bytes = 0;
    for i in items.iter() {
        if inner.is_empty() || i.name.starts_with(&pre) {
            if parent_of(&i.name) == inner {
                n += 1;
            }
            bytes += i.size;
        }
    }
    Ok((n, bytes))
}

// ───────────────────────────── extraction ─────────────────────────────

const PRIVATE_CACHE_DIR_NAME: &str = "archives-private";
const LEGACY_CACHE_DIR_NAME: &str = "archives";
const WINDOWS_PRIVATE_CACHE_DIR_NAME: &str = "gall-archives-private";
const WINDOWS_LEGACY_CACHE_DIR_NAME: &str = "gall-archives";

static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);

/// Removes an exclusive staging file on drop unless disarmed after publication.
struct StagingFile(Option<PathBuf>);

impl StagingFile {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for StagingFile {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::remove_file(&p);
        }
    }
}

#[cfg(unix)]
fn current_euid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Normalizes a path by stripping trailing slashes and `.` segments so `lstat`
/// (`fs::symlink_metadata`) never follows a symlink due to a trailing `/`.
fn strip_trailing_separators(path: &Path) -> PathBuf {
    path.components().collect()
}

/// Validates a parent directory that may be shared with other user caches (`~/.cache`,
/// `~/.cache/gall`): it must be a real directory (not a symlink), owned by the current
/// user on Unix, and not world-writable.
fn validate_parent_dir(path: &Path) -> Result<()> {
    let path = strip_trailing_separators(path);
    let meta = fs::symlink_metadata(&path).ctx("stat", &path)?;
    if meta.file_type().is_symlink() {
        return Err(CoreError::Other(format!(
            "unsafe archive cache root: {} is a symbolic link",
            path.display()
        )));
    }
    if !meta.file_type().is_dir() {
        return Err(CoreError::Other(format!(
            "unsafe archive cache root: {} is not a directory",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let euid = current_euid();
        if meta.uid() != euid {
            return Err(CoreError::Other(format!(
                "unsafe archive cache root: {} is not owned by the current user",
                path.display()
            )));
        }
        if meta.mode() & 0o002 != 0 {
            return Err(CoreError::Other(format!(
                "unsafe archive cache root: {} is world-writable",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Validates a system/temporary base directory (`TMPDIR`, `/tmp`, `%TEMP%`):
/// strips trailing separators so symlinks are detected, rejects symlinks and non-dirs,
/// and on Unix requires ownership by the current user or root plus the sticky bit
/// whenever the directory is world-writable.
fn validate_temp_dir(raw_temp: &Path) -> Result<PathBuf> {
    let temp = strip_trailing_separators(raw_temp);
    if temp.as_os_str().is_empty() {
        return Err(CoreError::Other(
            "unsafe archive cache root: temporary directory path is empty".into(),
        ));
    }
    let meta = fs::symlink_metadata(&temp).ctx("stat", &temp)?;
    if meta.file_type().is_symlink() {
        return Err(CoreError::Other(format!(
            "unsafe archive cache root: {} is a symbolic link",
            temp.display()
        )));
    }
    if !meta.file_type().is_dir() {
        return Err(CoreError::Other(format!(
            "unsafe archive cache root: {} is not a directory",
            temp.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let euid = current_euid();
        let uid = meta.uid();
        if uid != euid && uid != 0 {
            return Err(CoreError::Other(format!(
                "unsafe archive cache root: {} is not owned by the current user or root",
                temp.display()
            )));
        }
        let mode = meta.mode();
        if (mode & 0o002 != 0) && (mode & 0o1000 == 0) {
            return Err(CoreError::Other(format!(
                "unsafe archive cache root: {} is world-writable without the sticky bit",
                temp.display()
            )));
        }
    }
    Ok(temp)
}

/// Validates a private archive cache directory (`archives-private`, per-archive buckets,
/// `entries`, and member subdirectories): must be a real directory (not a symlink) and,
/// on Unix, owned by the current user with no group or world permissions (`0o077 == 0`).
fn validate_private_dir(path: &Path) -> Result<()> {
    let path = strip_trailing_separators(path);
    let meta = fs::symlink_metadata(&path).ctx("stat", &path)?;
    if meta.file_type().is_symlink() {
        return Err(CoreError::Other(format!(
            "unsafe archive cache directory: {} is a symbolic link",
            path.display()
        )));
    }
    if !meta.file_type().is_dir() {
        return Err(CoreError::Other(format!(
            "unsafe archive cache directory: {} is not a directory",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let euid = current_euid();
        if meta.uid() != euid {
            return Err(CoreError::Other(format!(
                "unsafe archive cache directory: {} is not owned by the current user",
                path.display()
            )));
        }
        if meta.mode() & 0o077 != 0 {
            return Err(CoreError::Other(format!(
                "unsafe archive cache directory: {} has non-private permissions ({:04o})",
                path.display(),
                meta.mode() & 0o7777
            )));
        }
    }
    Ok(())
}

/// Validates a cached regular file (not a symlink, and on Unix owned by the current user
/// with no group or world permissions).
fn validate_private_file(path: &Path) -> Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path).ctx("stat", path)?;
    if meta.file_type().is_symlink() {
        return Err(CoreError::Other(format!(
            "unsafe archive cache file: {} is a symbolic link",
            path.display()
        )));
    }
    if !meta.file_type().is_file() {
        return Err(CoreError::Other(format!(
            "unsafe archive cache file: {} is not a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let euid = current_euid();
        if meta.uid() != euid {
            return Err(CoreError::Other(format!(
                "unsafe archive cache file: {} is not owned by the current user",
                path.display()
            )));
        }
        if meta.mode() & 0o077 != 0 {
            return Err(CoreError::Other(format!(
                "unsafe archive cache file: {} has non-private permissions ({:04o})",
                path.display(),
                meta.mode() & 0o7777
            )));
        }
    }
    Ok(meta)
}

fn create_dir_mode_700(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        fs::DirBuilder::new().create(path)
    }
}

/// Ensures `path` exists as a validated private (`0o700`) directory without following symlinks.
fn ensure_private_dir(path: &Path) -> Result<()> {
    let path = strip_trailing_separators(path);
    match create_dir_mode_700(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(CoreError::io("create", &path, e)),
    }
    validate_private_dir(&path)
}

/// Ensures the user's cache root exists component-by-component, rejecting non-OS
/// symbolic link ancestors and rejecting any symlink at the leaf `cache_root` itself.
fn ensure_cache_root(raw_root: &Path) -> Result<PathBuf> {
    let root = strip_trailing_separators(raw_root);
    if root.as_os_str().is_empty() {
        return Err(CoreError::Other(
            "unsafe archive cache root: cache directory path is empty".into(),
        ));
    }
    let comps: Vec<Component<'_>> = root.components().collect();
    let mut cur = PathBuf::new();
    for (idx, comp) in comps.iter().enumerate() {
        cur.push(comp.as_os_str());
        if matches!(comp, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        let check_meta = |meta: &fs::Metadata| -> Result<()> {
            if meta.file_type().is_symlink() {
                // Allow only top-level root-owned OS layout symlinks (e.g. macOS `/var` ->
                // `/private/var`, immutable Linux `/home` -> `/var/home`), never deeper or leaf links.
                #[cfg(unix)]
                let allowed_os_ancestor = {
                    use std::os::unix::fs::MetadataExt;
                    let is_leaf = idx + 1 == comps.len();
                    !is_leaf
                        && idx == 1
                        && matches!(comps.first(), Some(Component::RootDir))
                        && meta.uid() == 0
                };
                #[cfg(not(unix))]
                let allowed_os_ancestor = {
                    let _ = idx;
                    false
                };
                if !allowed_os_ancestor {
                    return Err(CoreError::Other(format!(
                        "unsafe archive cache root: {} is a symbolic link",
                        cur.display()
                    )));
                }
            } else if !meta.file_type().is_dir() {
                return Err(CoreError::Other(format!(
                    "unsafe archive cache root: {} is not a directory",
                    cur.display()
                )));
            }
            Ok(())
        };
        match fs::symlink_metadata(&cur) {
            Ok(meta) => check_meta(&meta)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => match create_dir_mode_700(&cur) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let meta = fs::symlink_metadata(&cur).ctx("stat", &cur)?;
                    check_meta(&meta)?;
                }
                Err(e) => return Err(CoreError::io("create", &cur, e)),
            },
            Err(e) => return Err(CoreError::io("stat", &cur, e)),
        }
    }
    validate_parent_dir(&root)?;
    Ok(root)
}

/// Ensures a direct child parent directory (`<cache_root>/gall`) exists without following symlinks.
fn ensure_parent_dir(path: &Path) -> Result<()> {
    let path = strip_trailing_separators(path);
    match fs::symlink_metadata(&path) {
        Ok(_) => validate_parent_dir(&path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match create_dir_mode_700(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(CoreError::io("create", &path, e)),
            }
            validate_parent_dir(&path)
        }
        Err(e) => Err(CoreError::io("stat", &path, e)),
    }
}

/// Ensures every directory component of `rel` under validated private directory `base`
/// exists as a validated private (`0o700`) directory, never traversing symlinks.
fn ensure_private_dir_chain(base: &Path, rel: &Path) -> Result<()> {
    validate_private_dir(base)?;
    let mut cur = base.to_path_buf();
    for part in rel.components() {
        let Component::Normal(name) = part else {
            return Err(CoreError::Other(format!(
                "unsafe archive cache path component in {}",
                rel.display()
            )));
        };
        cur.push(name);
        ensure_private_dir(&cur)?;
    }
    Ok(())
}

/// Validates every existing ancestor directory of `rel` under `base` without creating anything.
fn validate_existing_dir_chain(base: &Path, rel: &Path) -> Result<()> {
    validate_private_dir(base)?;
    let mut cur = base.to_path_buf();
    for part in rel.components() {
        let Component::Normal(name) = part else {
            return Err(CoreError::Other(format!(
                "unsafe archive cache path component in {}",
                rel.display()
            )));
        };
        cur.push(name);
        match fs::symlink_metadata(&cur) {
            Ok(_) => validate_private_dir(&cur)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(CoreError::io("stat", &cur, e)),
        }
    }
    Ok(())
}

/// Creates a uniquely named, exclusive (`O_CREAT | O_EXCL`), private (`0o600`) staging
/// file directly inside validated private directory `root`.
fn create_private_staging_file(root: &Path) -> Result<(PathBuf, fs::File)> {
    validate_private_dir(root)?;
    let pid = std::process::id();
    for _ in 0..1024 {
        let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let candidate = root.join(format!(".gall-part-{pid}-{seq:x}-{nanos:x}"));
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(CoreError::io("create", &candidate, e)),
        }
    }
    Err(CoreError::AlreadyExists(root.join(".gall-part")))
}

/// One cache root per archive version (see `cache_base`). Members live under
/// `entries/`, separate from the root's bookkeeping and staging files.
fn cache_root_in(base: &Path, archive: &Path) -> Result<PathBuf> {
    let st = stamp(archive)?;
    let mut h = DefaultHasher::new();
    ("member-cache-v2", archive, st.0, st.1).hash(&mut h);
    Ok(base.join(format!("{:016x}", h.finish())))
}

#[cfg(test)]
fn cache_root(archive: &Path) -> Result<PathBuf> {
    let base = cache_base()?;
    cache_root_in(&base, archive)
}

fn extraction_lock(root: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap();
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(root).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(root.to_path_buf(), Arc::downgrade(&lock));
    lock
}

/// Assign sibling names together, reserving every preferred name before adding
/// suffixes. Parents use the same mapping whether extracted alone or recursively.
fn member_paths(items: &[Item]) -> HashMap<String, PathBuf> {
    let mut children: BTreeMap<&str, Vec<(&Item, String)>> = BTreeMap::new();
    for item in items {
        let name = item.name.rsplit('/').next().unwrap();
        let local = local_name(PathBuf::from(name))
            .to_string_lossy()
            .into_owned();
        children
            .entry(parent_of(&item.name))
            .or_default()
            .push((item, local));
    }
    let mut paths: HashMap<String, PathBuf> = HashMap::new();
    for (parent, mut siblings) in children {
        // Prefer unchanged names, so sanitizing `a:b` never renames a real `a_b`.
        siblings.sort_by_key(|(item, local)| {
            (item.name.rsplit('/').next().unwrap() != local, &item.name)
        });
        let mut reserved: HashSet<String> = siblings
            .iter()
            .map(|(_, local)| local.to_uppercase())
            .collect();
        let mut used = HashSet::new();
        for (item, mut local) in siblings {
            if !used.insert(local.to_uppercase()) {
                let path = Path::new(&local);
                let stem = path.file_stem().unwrap().to_string_lossy();
                let ext = path
                    .extension()
                    .map(|ext| format!(".{}", ext.to_string_lossy()))
                    .unwrap_or_default();
                let mut suffix = 1;
                loop {
                    let candidate = format!("{stem}~{suffix}{ext}");
                    let key = candidate.to_uppercase();
                    if reserved.insert(key.clone()) {
                        used.insert(key);
                        local = candidate;
                        break;
                    }
                    suffix += 1;
                }
            }
            let path = if parent.is_empty() {
                PathBuf::from(local)
            } else {
                paths[parent].join(local)
            };
            paths.insert(item.name.clone(), path);
        }
    }
    paths
}

struct CachePaths {
    base: PathBuf,
    legacy: Option<PathBuf>,
    fallback_parent: Option<PathBuf>,
}

/// Creates a process-private random directory (`0o700` on Unix) under validated `temp_dir`
/// for Unix hosts where `dirs::cache_dir()` is unavailable, avoiding predictable names in `/tmp`.
fn create_fallback_cache_dir(temp_dir: &Path) -> Result<PathBuf> {
    let temp = validate_temp_dir(temp_dir)?;
    let mut builder = tempfile::Builder::new();
    builder.prefix("gall-archives-").rand_bytes(12);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let dir = builder
        .tempdir_in(&temp)
        .map_err(|e| CoreError::io("create", &temp, e))?;
    let path = dir.keep();
    validate_private_dir(&path)?;
    Ok(path)
}

fn process_fallback_cache_base(temp_dir: &Path) -> Result<PathBuf> {
    static FALLBACK: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    let mut slot = FALLBACK.get_or_init(Default::default).lock().unwrap();
    if let Some(existing) = slot.as_ref() {
        if validate_private_dir(existing).is_ok() {
            return Ok(existing.clone());
        }
    }
    let fresh = create_fallback_cache_dir(temp_dir)?;
    *slot = Some(fresh.clone());
    Ok(fresh)
}

/// Resolves and validates the private archive cache base (creating it with `0o700` on Unix)
/// and returns any legacy cache path that should be safely retired.
fn resolve_cache_paths(
    user_cache_dir: Option<PathBuf>,
    temp_dir: &Path,
    fallback_dir: impl FnOnce(&Path) -> Result<PathBuf>,
) -> Result<CachePaths> {
    if cfg!(windows) {
        let temp = validate_temp_dir(temp_dir)?;
        let base = temp.join(WINDOWS_PRIVATE_CACHE_DIR_NAME);
        ensure_private_dir(&base)?;
        return Ok(CachePaths {
            base,
            legacy: Some(temp.join(WINDOWS_LEGACY_CACHE_DIR_NAME)),
            fallback_parent: None,
        });
    }
    match user_cache_dir {
        Some(cache_root) => {
            let cache_root = ensure_cache_root(&cache_root)?;
            let gall_dir = cache_root.join("gall");
            ensure_parent_dir(&gall_dir)?;
            let base = gall_dir.join(PRIVATE_CACHE_DIR_NAME);
            ensure_private_dir(&base)?;
            Ok(CachePaths {
                base,
                legacy: Some(gall_dir.join(LEGACY_CACHE_DIR_NAME)),
                fallback_parent: None,
            })
        }
        None => {
            let temp = validate_temp_dir(temp_dir)?;
            Ok(CachePaths {
                base: fallback_dir(&temp)?,
                legacy: None,
                fallback_parent: Some(temp),
            })
        }
    }
}

#[cfg(test)]
fn cache_base() -> Result<PathBuf> {
    let temp = std::env::temp_dir();
    Ok(resolve_cache_paths(dirs::cache_dir(), &temp, process_fallback_cache_base)?.base)
}

fn is_cache_bucket_name(name: &std::ffi::OsStr) -> bool {
    let Some(s) = name.to_str() else {
        return false;
    };
    s.len() == 16 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(unix)]
mod unix_dir {
    use super::current_euid;
    use std::ffi::{CStr, CString, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const OPEN_DIR_FLAGS: libc::c_int =
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    fn fstat_owned_dir(fd: libc::c_int, euid: u32) -> bool {
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `fd` is an open descriptor and `st` is valid for writes.
        if unsafe { libc::fstat(fd, st.as_mut_ptr()) } != 0 {
            return false;
        }
        // SAFETY: initialized by successful `fstat`.
        let st = unsafe { st.assume_init() };
        (st.st_mode & libc::S_IFMT) == libc::S_IFDIR && st.st_uid == euid
    }

    fn open_root_dir(path: &Path, euid: u32) -> Option<libc::c_int> {
        let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c_path` is NUL-terminated; `O_DIRECTORY | O_NOFOLLOW` never follows symlinks.
        let fd = unsafe { libc::open(c_path.as_ptr(), OPEN_DIR_FLAGS) };
        if fd < 0 {
            return None;
        }
        if !fstat_owned_dir(fd, euid) {
            // SAFETY: closes owned fd.
            unsafe { libc::close(fd) };
            return None;
        }
        Some(fd)
    }

    fn open_child_dir(parent_fd: libc::c_int, name: &CStr, euid: u32) -> Option<libc::c_int> {
        // SAFETY: `name` is NUL-terminated; `openat` pins lookup to `parent_fd` with `O_NOFOLLOW`.
        let fd = unsafe { libc::openat(parent_fd, name.as_ptr(), OPEN_DIR_FLAGS) };
        if fd < 0 {
            return None;
        }
        if !fstat_owned_dir(fd, euid) {
            // SAFETY: closes owned fd.
            unsafe { libc::close(fd) };
            return None;
        }
        Some(fd)
    }

    fn fstatat_child(parent_fd: libc::c_int, name: &CStr) -> Option<libc::stat> {
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `parent_fd` is open, `name` is NUL-terminated, `AT_SYMLINK_NOFOLLOW` never follows symlinks.
        if unsafe {
            libc::fstatat(
                parent_fd,
                name.as_ptr(),
                st.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return None;
        }
        // SAFETY: initialized by successful `fstatat`.
        Some(unsafe { st.assume_init() })
    }

    /// Reads entry names from `dir_fd` (consuming `dir_fd` via `fdopendir` / `closedir`).
    fn read_names_from_fd(dir_fd: libc::c_int) -> Vec<CString> {
        // SAFETY: `fdopendir` takes ownership of `dir_fd` on success.
        let dir = unsafe { libc::fdopendir(dir_fd) };
        if dir.is_null() {
            // SAFETY: `fdopendir` failed, so caller still owns `dir_fd`.
            unsafe { libc::close(dir_fd) };
            return Vec::new();
        }
        let mut names = Vec::new();
        loop {
            // SAFETY: `dir` is a valid open `DIR*`.
            let entry = unsafe { libc::readdir(dir) };
            if entry.is_null() {
                break;
            }
            // SAFETY: `d_name` is a NUL-terminated string inside the directory entry.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() != b"." && name.to_bytes() != b".." {
                names.push(name.to_owned());
            }
        }
        // SAFETY: closes `dir` and its underlying `dir_fd`.
        unsafe { libc::closedir(dir) };
        names
    }

    /// Recursively removes owned regular files and real directories inside pinned `dir_fd`
    /// without ever re-resolving paths or following symlinks.
    fn clear_dir_fd(dir_fd: libc::c_int, euid: u32) {
        // Duplicate the fd for `fdopendir` so `dir_fd` stays valid for `fstatat`/`openat`/`unlinkat`.
        // SAFETY: `F_DUPFD_CLOEXEC` duplicates an open descriptor.
        let iter_fd = unsafe { libc::fcntl(dir_fd, libc::F_DUPFD_CLOEXEC, 0) };
        if iter_fd < 0 {
            return;
        }
        for name in read_names_from_fd(iter_fd) {
            let Some(st) = fstatat_child(dir_fd, &name) else {
                continue;
            };
            if st.st_uid != euid {
                continue;
            }
            match st.st_mode & libc::S_IFMT {
                libc::S_IFLNK => continue,
                libc::S_IFREG => {
                    // SAFETY: unlinks regular file directly inside pinned `dir_fd`.
                    let _ = unsafe { libc::unlinkat(dir_fd, name.as_ptr(), 0) };
                }
                libc::S_IFDIR => {
                    if let Some(child_fd) = open_child_dir(dir_fd, &name, euid) {
                        clear_dir_fd(child_fd, euid);
                        // SAFETY: closes owned child fd.
                        unsafe { libc::close(child_fd) };
                        // SAFETY: removes child dir only if empty, directly inside pinned `dir_fd`.
                        let _ =
                            unsafe { libc::unlinkat(dir_fd, name.as_ptr(), libc::AT_REMOVEDIR) };
                    }
                }
                _ => continue,
            }
        }
    }

    fn has_bucket_signature_fd(bucket_fd: libc::c_int) -> bool {
        let check = |name: &CStr, kind: libc::mode_t| {
            fstatat_child(bucket_fd, name).is_some_and(|st| (st.st_mode & libc::S_IFMT) == kind)
        };
        check(c".used", libc::S_IFREG)
            || check(c"entries", libc::S_IFDIR)
            || check(c".gall-part", libc::S_IFREG)
    }

    pub(super) fn remove_real_tree_no_follow(dir: &Path) {
        let euid = current_euid();
        let Some(fd) = open_root_dir(dir, euid) else {
            return;
        };
        clear_dir_fd(fd, euid);
        // SAFETY: closes owned fd.
        unsafe { libc::close(fd) };
        let _ = std::fs::remove_dir(dir);
    }

    pub(super) fn retire_legacy_cache(old_base: &Path) {
        let euid = current_euid();
        let Some(base_fd) = open_root_dir(old_base, euid) else {
            return;
        };
        // SAFETY: duplicate fd for directory iteration while keeping `base_fd` pinned.
        let iter_fd = unsafe { libc::fcntl(base_fd, libc::F_DUPFD_CLOEXEC, 0) };
        if iter_fd >= 0 {
            for name in read_names_from_fd(iter_fd) {
                if !super::is_cache_bucket_name(OsStr::from_bytes(name.as_bytes())) {
                    continue;
                }
                let Some(bucket_fd) = open_child_dir(base_fd, &name, euid) else {
                    continue;
                };
                let is_bucket = has_bucket_signature_fd(bucket_fd);
                if is_bucket {
                    clear_dir_fd(bucket_fd, euid);
                }
                // SAFETY: closes owned fd.
                unsafe { libc::close(bucket_fd) };
                if is_bucket {
                    // SAFETY: removes empty bucket directly inside pinned `base_fd`.
                    let _ = unsafe { libc::unlinkat(base_fd, name.as_ptr(), libc::AT_REMOVEDIR) };
                }
            }
        }
        // SAFETY: closes owned fd.
        unsafe { libc::close(base_fd) };
        // Only succeeds when every item inside `old_base` was a recognized Gall cache bucket.
        let _ = std::fs::remove_dir(old_base);
    }
}

#[cfg(not(unix))]
fn has_legacy_bucket_signature(bucket: &Path) -> bool {
    let is_real_file = |name: &str| {
        fs::symlink_metadata(bucket.join(name))
            .is_ok_and(|m| !m.file_type().is_symlink() && m.file_type().is_file())
    };
    let is_real_dir = |name: &str| {
        fs::symlink_metadata(bucket.join(name))
            .is_ok_and(|m| !m.file_type().is_symlink() && m.file_type().is_dir())
    };
    is_real_file(".used") || is_real_dir("entries") || is_real_file(".gall-part")
}

/// Recursively removes regular files and real subdirectories inside `dir` without ever
/// following symbolic links. Symlinks (and directories containing them) are left untouched.
fn remove_real_tree_no_follow(dir: &Path) {
    let dir = strip_trailing_separators(dir);
    #[cfg(unix)]
    {
        unix_dir::remove_real_tree_no_follow(&dir);
    }
    #[cfg(not(unix))]
    {
        let Ok(meta) = fs::symlink_metadata(&dir) else {
            return;
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
            return;
        }
        let Ok(rd) = fs::read_dir(&dir) else {
            return;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(child_meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if child_meta.file_type().is_symlink() {
                continue;
            }
            if child_meta.file_type().is_dir() {
                remove_real_tree_no_follow(&path);
            } else if child_meta.file_type().is_file() {
                let _ = fs::remove_file(&path);
            }
        }
        let _ = fs::remove_dir(&dir);
    }
}

/// Safely retires the legacy (`archives` / `gall-archives`) cache directory without
/// following symlinks or deleting non-cache user data.
fn retire_legacy_cache(old_base: &Path) {
    let old_base = strip_trailing_separators(old_base);
    #[cfg(unix)]
    {
        unix_dir::retire_legacy_cache(&old_base);
    }
    #[cfg(not(unix))]
    {
        let Ok(meta) = fs::symlink_metadata(&old_base) else {
            return;
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
            return;
        }
        let Ok(rd) = fs::read_dir(&old_base) else {
            return;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if !is_cache_bucket_name(&entry.file_name()) {
                continue;
            }
            let Ok(bucket_meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if bucket_meta.file_type().is_symlink() || !bucket_meta.file_type().is_dir() {
                continue;
            }
            if has_legacy_bucket_signature(&path) {
                remove_real_tree_no_follow(&path);
            }
        }
        let _ = fs::remove_dir(&old_base);
    }
}

/// Removes extracted archive folders not used for a day without following symlinks.
fn prune_cache_base(base: &Path) {
    if validate_private_dir(base).is_err() {
        return;
    }
    let Ok(rd) = fs::read_dir(base) else {
        return;
    };
    let day = Duration::from_secs(24 * 3600);
    for e in rd.flatten() {
        let path = e.path();
        if !is_cache_bucket_name(&e.file_name()) {
            continue;
        }
        let Ok(bucket_meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if bucket_meta.file_type().is_symlink() || !bucket_meta.file_type().is_dir() {
            continue;
        }
        let used_meta = fs::symlink_metadata(path.join(".used"))
            .ok()
            .filter(|m| !m.file_type().is_symlink() && m.file_type().is_file());
        let old = used_meta
            .as_ref()
            .unwrap_or(&bucket_meta)
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        if old {
            remove_real_tree_no_follow(&path);
        }
    }
}

/// Prunes expired private fallback cache directories (`gall-archives-*`) created by
/// earlier processes when no user cache directory was available.
fn prune_stale_fallback_dirs(temp_dir: &Path, current_base: &Path) {
    let Ok(rd) = fs::read_dir(temp_dir) else {
        return;
    };
    let day = Duration::from_secs(24 * 3600);
    for entry in rd.flatten() {
        let path = entry.path();
        if path == current_base {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !name.starts_with("gall-archives-") {
            continue;
        }
        if validate_private_dir(&path).is_err() {
            continue;
        }
        // Read `mtime` before pruning children, because removing a child updates the parent's `mtime`.
        let was_old = fs::symlink_metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        prune_cache_base(&path);
        if was_old {
            let _ = fs::remove_dir(&path);
        }
    }
}

/// Removes extracted archive folders not used for a day and retires the legacy cache (once per run).
fn prune_once(paths: &CachePaths) {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        if let Some(legacy) = paths.legacy.as_deref() {
            retire_legacy_cache(legacy);
        }
        if let Some(parent) = paths.fallback_parent.as_deref() {
            prune_stale_fallback_dirs(parent, &paths.base);
        }
        prune_cache_base(&paths.base);
    });
}

/// Real files for virtual paths: each entry (a folder with everything in it) is
/// extracted into the cache once, and its cached path returned in the same order.
/// Ordinary paths are returned unchanged, so callers can pass a mixed selection.
pub fn materialize(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut by_archive: HashMap<PathBuf, Vec<(usize, String)>> = HashMap::new();
    for (i, p) in paths.iter().enumerate() {
        if let Some((ar, inner)) = split(p) {
            if inner.is_empty() {
                continue; // the archive file itself
            }
            by_archive.entry(ar).or_default().push((i, inner));
        }
    }
    if by_archive.is_empty() {
        return Ok(paths.to_vec());
    }
    let temp = std::env::temp_dir();
    let cache = resolve_cache_paths(dirs::cache_dir(), &temp, process_fallback_cache_base)?;
    prune_once(&cache);
    materialize_grouped(paths, by_archive, &cache.base)
}

#[cfg(test)]
fn materialize_in(paths: &[PathBuf], base: &Path) -> Result<Vec<PathBuf>> {
    let mut by_archive: HashMap<PathBuf, Vec<(usize, String)>> = HashMap::new();
    for (i, p) in paths.iter().enumerate() {
        if let Some((ar, inner)) = split(p) {
            if inner.is_empty() {
                continue;
            }
            by_archive.entry(ar).or_default().push((i, inner));
        }
    }
    if by_archive.is_empty() {
        return Ok(paths.to_vec());
    }
    materialize_grouped(paths, by_archive, base)
}

fn materialize_grouped(
    paths: &[PathBuf],
    by_archive: HashMap<PathBuf, Vec<(usize, String)>>,
    base: &Path,
) -> Result<Vec<PathBuf>> {
    validate_private_dir(base)?;
    let mut out = paths.to_vec();
    for (archive, wanted) in by_archive {
        let root = cache_root_in(base, &archive)?;
        let lock = extraction_lock(&root);
        let _guard = lock.lock().unwrap();
        // Validate existing cache roots before reusing or writing into them.
        if fs::symlink_metadata(&root).is_ok() {
            validate_private_dir(&root)?;
        }
        let entries_dir = root.join("entries");
        if fs::symlink_metadata(&entries_dir).is_ok() {
            validate_private_dir(&entries_dir)?;
        }
        let (items, unique_files) = index_info(&archive)?;
        let members = member_paths(&items);
        let mut need: Vec<String> = Vec::new();
        for (i, inner) in &wanted {
            let it = items
                .iter()
                .find(|it| &it.name == inner)
                .ok_or_else(|| CoreError::NotFound(PathBuf::from(virtual_path(&archive, inner))))?;
            let member_rel = &members[inner];
            let target = entries_dir.join(member_rel);
            if fs::symlink_metadata(&entries_dir).is_ok() {
                if let Some(parent_rel) = member_rel.parent().filter(|p| !p.as_os_str().is_empty())
                {
                    validate_existing_dir_chain(&entries_dir, parent_rel)?;
                }
            }
            // A folder is re-extracted each time (cheap to check, hard to know if complete);
            // a file is reused only when it is a validated private regular file of the right size.
            let ready = if it.is_dir {
                if fs::symlink_metadata(&target).is_ok() {
                    validate_private_dir(&target)?;
                }
                false
            } else {
                match fs::symlink_metadata(&target) {
                    Ok(_) => validate_private_file(&target)?.len() == it.size,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                    Err(e) => return Err(CoreError::io("stat", &target, e)),
                }
            };
            if !ready {
                need.push(inner.clone());
            }
            out[*i] = target;
        }
        if !need.is_empty() {
            ensure_private_dir(&root)?;
            ensure_private_dir(&entries_dir)?;
            // Tar and 7z archives are read from the start: when every name occurs once,
            // reading stops after the last wanted file instead of at the end.
            let files = unique_files.then(|| {
                items
                    .iter()
                    .filter(|it| !it.is_dir && wanted_by(&it.name, &need))
                    .count()
            });
            if files.is_some() {
                // Folders may come after their files in the archive (or not at all).
                for it in items
                    .iter()
                    .filter(|it| it.is_dir && wanted_by(&it.name, &need))
                {
                    ensure_private_dir_chain(&entries_dir, &members[&it.name])?;
                }
            }
            extract_entries(&archive, &root, &need, &members, files)?;
        }
        if fs::symlink_metadata(&root).is_ok() {
            let _ = filetime_touch(&root);
        }
    }
    Ok(out)
}

/// Marks a cache folder as used via an exclusive private file, outside the members' namespace.
fn filetime_touch(dir: &Path) -> Result<()> {
    let used = dir.join(".used");
    if fs::symlink_metadata(&used).is_ok() {
        validate_private_file(&used)?;
    }
    let (tmp, file) = create_private_staging_file(dir)?;
    let guard = StagingFile(Some(tmp.clone()));
    drop(file);
    fs::rename(&tmp, &used).ctx("move", &used)?;
    guard.disarm();
    Ok(())
}

/// Does `name` belong to one of `wanted` (the item itself or inside a wanted folder)?
fn wanted_by(name: &str, wanted: &[String]) -> bool {
    wanted.iter().any(|w| {
        name == w
            || (name.len() > w.len()
                && name.starts_with(w.as_str())
                && name.as_bytes()[w.len()] == b'/')
    })
}

/// Extracts the wanted entries (and folders' contents) of `archive` under `root`,
/// each file written to an exclusive private staging file first so readers never see
/// half a file or non-private permissions.
fn extract_entries(
    archive: &Path,
    root: &Path,
    wanted: &[String],
    members: &HashMap<String, PathBuf>,
    // The number of wanted files, when reading may stop after that many (see `materialize`).
    files: Option<usize>,
) -> Result<()> {
    validate_private_dir(root)?;
    let entries_dir = root.join("entries");
    ensure_private_dir(&entries_dir)?;
    let mut left = files;
    // One wanted file was written; true when it was the last one.
    let mut last_written = |is_dir: bool| match left.as_mut() {
        Some(n) if !is_dir => {
            *n = n.saturating_sub(1);
            *n == 0
        }
        _ => false,
    };
    let (format, _) = detect(archive).ok_or_else(|| not_archive(archive))?;
    let file = fs::File::open(archive).ctx("open", archive)?;
    let err = |what: &str, e: &dyn std::fmt::Display| CoreError::Other(format!("{what}: {e}"));
    enum WriteErr {
        Stream(CoreError),
        Cache(CoreError),
    }
    impl WriteErr {
        fn into_core(self) -> CoreError {
            match self {
                Self::Stream(e) | Self::Cache(e) => e,
            }
        }
        fn or_wrong_password(self, archive: &Path) -> CoreError {
            match self {
                Self::Cache(e) => e,
                Self::Stream(e) => {
                    if password_for(archive).is_some() {
                        need_password(archive)
                    } else {
                        e
                    }
                }
            }
        }
    }
    let write =
        |name: &str, is_dir: bool, data: &mut dyn Read| -> std::result::Result<(), WriteErr> {
            let member_rel = &members[name];
            let out = entries_dir.join(member_rel);
            if is_dir {
                return ensure_private_dir_chain(&entries_dir, member_rel).map_err(WriteErr::Cache);
            }
            if let Some(parent_rel) = member_rel.parent().filter(|p| !p.as_os_str().is_empty()) {
                ensure_private_dir_chain(&entries_dir, parent_rel).map_err(WriteErr::Cache)?;
            }
            if fs::symlink_metadata(&out).is_ok() {
                validate_private_file(&out).map_err(WriteErr::Cache)?;
            }
            let (tmp, mut w) = create_private_staging_file(root).map_err(WriteErr::Cache)?;
            let guard = StagingFile(Some(tmp.clone()));
            io::copy(data, &mut w)
                .ctx("write", &tmp)
                .map_err(WriteErr::Stream)?;
            drop(w);
            fs::rename(&tmp, &out)
                .ctx("move", &out)
                .map_err(WriteErr::Cache)?;
            guard.disarm();
            Ok(())
        };
    match format {
        ArchiveFormat::Zip => {
            let mut z =
                zip::ZipArchive::new(SeekBuffered::new(file)).map_err(|e| err("zip", &e))?;
            for i in 0..z.len() {
                // Names come from the directory: only wanted entries are read and decrypted.
                let (name, skip) = {
                    let f = z.by_index_data(i).map_err(|e| err("zip", &e))?;
                    (
                        f.enclosed_name().and_then(|p| clean(&p.to_string_lossy())),
                        f.is_symlink(),
                    )
                };
                let Some(name) = name else { continue };
                if wanted_by(&name, wanted) && !skip {
                    let mut f = zip_entry(&mut z, i, archive)?;
                    let is_dir = f.is_dir();
                    if let Err(e) = write(&name, is_dir, &mut f) {
                        // A wrong ZipCrypto password shows up as a bad checksum while reading,
                        // whereas cache validation errors must always surface directly.
                        return Err(e.or_wrong_password(archive));
                    }
                }
            }
        }
        ArchiveFormat::SevenZ => {
            let mut file = file;
            let password = sevenz_password(archive);
            let ar = sevenz_rust2::Archive::read(&mut file, &password)
                .map_err(|e| sevenz_err(archive, e))?;
            // The wanted entries by their place in the archive. Data is packed in blocks
            // (one for all files, a few large ones, or one per file): only blocks that
            // hold a wanted file are decoded, each up to its last wanted file. A repeated
            // name is written again in archive order, so its last entry stays.
            let wanted_at: Vec<Option<String>> = ar
                .files
                .iter()
                .map(|f| clean(f.name()).filter(|name| wanted_by(name, wanted)))
                .collect();
            let block_of = &ar.stream_map.file_block_index;
            let mut last_in_block: BTreeMap<usize, usize> = BTreeMap::new();
            for (at, name) in wanted_at.iter().enumerate() {
                if let (Some(_), Some(block)) = (name, block_of.get(at).copied().flatten()) {
                    last_in_block.insert(block, at);
                }
            }
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
            let mut failed: Option<WriteErr> = None;
            for (&block, &last) in &last_in_block {
                let mut at = ar.stream_map.block_first_file_index[block];
                sevenz_rust2::BlockDecoder::new(threads, block, &ar, &password, &mut file)
                    .for_each_entries(&mut |entry, data| {
                        let this = at;
                        at += 1;
                        if let Some(Some(name)) = wanted_at.get(this) {
                            if let Err(e) = write(name, entry.is_directory(), data) {
                                failed = Some(e);
                                return Ok(false);
                            }
                        }
                        // Entries before a wanted one are read through by the decoder.
                        Ok(this < last)
                    })
                    .map_err(|e| sevenz_err(archive, e))?;
                if failed.is_some() {
                    break;
                }
            }
            if let Some(e) = failed {
                // With a wrong password decryption yields garbage the decoder rejects,
                // whereas cache validation errors must always surface directly.
                return Err(e.or_wrong_password(archive));
            }
            // Folders and empty files have no data and belong to no block. They are
            // written last, so one whose name comes again later in the archive is left
            // out: the later entry is the one that stays.
            let mut last_of: HashMap<&str, usize> = HashMap::new();
            for (at, name) in wanted_at.iter().enumerate() {
                if let Some(name) = name {
                    last_of.insert(name, at);
                }
            }
            for (at, name) in wanted_at.iter().enumerate() {
                if let (Some(name), None) = (name, block_of.get(at).copied().flatten()) {
                    if last_of[name.as_str()] == at {
                        write(name, ar.files[at].is_directory(), &mut io::empty())
                            .map_err(WriteErr::into_core)?;
                    }
                }
            }
        }
        _ => {
            let mut ar = tar::Archive::new(tar_stream(file, format)?);
            for e in ar.entries().map_err(|e| err("tar", &e))? {
                let mut e = e.map_err(|e| err("tar", &e))?;
                let Some(name) = e.path().ok().and_then(|p| clean(&p.to_string_lossy())) else {
                    continue;
                };
                let kind = e.header().entry_type();
                if (kind.is_file() || kind.is_dir()) && wanted_by(&name, wanted) {
                    write(&name, kind.is_dir(), &mut e).map_err(WriteErr::into_core)?;
                    if last_written(kind.is_dir()) {
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with_entries(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("members.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    #[test]
    fn plain_names_are_cleaned_like_any_other() {
        for name in [
            "a",
            "a/b.txt",
            "a/b/",
            "a//b",
            "./a",
            "a/./b",
            "a/../b",
            "../a",
            "/a",
            "//a",
            "",
            "/",
            ".",
            "..",
            "a/..",
            "a\\b",
            "..\\a",
            "C:\\a",
            "C:a",
            "a:b",
            "a b/ć č.txt",
            "a/b/c/d/e",
            "a/.hidden",
            "a/..b",
            "a/b..",
            "...",
            " ",
            "a/ /b",
        ] {
            assert_eq!(clean(name), clean_checked(name), "{name:?}");
        }
        assert_eq!(clean("a/b/").as_deref(), Some("a/b"));
        assert_eq!(clean("../a"), None);
    }

    #[test]
    fn folders_are_implied_once_and_the_list_is_sorted() {
        let item = |name: &str, is_dir: bool, size: u64, modified: Option<i64>| Item {
            name: name.into(),
            is_dir,
            size,
            modified,
        };
        let all = with_folders(vec![
            item("b/c/two.txt", false, 2, Some(20)),
            item("b/one.txt", false, 1, Some(10)),
            item("a.txt", false, 3, None),
            // A second entry of the same name: the first one stays.
            item("b/one.txt", false, 99, Some(99)),
            // The folder itself, after its files: only its date is taken.
            item("b/c", true, 0, Some(5)),
            item("b!", true, 0, None),
        ]);
        let seen: Vec<_> = all
            .iter()
            .map(|i| (i.name.as_str(), i.is_dir, i.size, i.modified))
            .collect();
        assert_eq!(
            seen,
            [
                ("a.txt", false, 3, None),
                ("b", true, 0, None),
                ("b!", true, 0, None),
                ("b/c", true, 0, Some(5)),
                ("b/c/two.txt", false, 2, Some(20)),
                ("b/one.txt", false, 1, Some(10)),
            ]
        );
    }

    /// A tar archive of the entries given: `(name, Some(data))` a file, `(name, None)`
    /// a folder.
    fn tar_of(path: &Path, entries: &[(&str, Option<&[u8]>)]) {
        let mut writer = tar::Builder::new(fs::File::create(path).unwrap());
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            match data {
                Some(data) => {
                    header.set_size(data.len() as u64);
                    writer.append_data(&mut header, name, *data).unwrap();
                }
                None => {
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_size(0);
                    writer.append_data(&mut header, name, io::empty()).unwrap();
                }
            }
        }
        writer.finish().unwrap();
    }

    #[test]
    fn a_repeated_name_still_gives_its_last_entry() {
        let t = tempfile::tempdir().unwrap();
        let ar = t.path().join("twice.tar");
        tar_of(
            &ar,
            &[
                ("a/one.txt", Some(b"first")),
                ("a/two.txt", Some(b"two")),
                ("a/one.txt", Some(b"second")),
            ],
        );
        assert!(!index_info(&ar).unwrap().1);
        let got = materialize(&[ar.join("a/one.txt")]).unwrap().remove(0);
        assert_eq!(fs::read(&got).unwrap(), b"second");
        fs::remove_dir_all(cache_root(&ar).unwrap()).unwrap();
    }

    #[test]
    fn a_folder_comes_out_whole_when_reading_stops_early() {
        let t = tempfile::tempdir().unwrap();
        let ar = t.path().join("early.tar");
        tar_of(
            &ar,
            &[
                ("a/one.txt", Some(b"one")),
                ("a/sub/two.txt", Some(b"two")),
                ("b/other.txt", Some(b"other")),
                // Folders listed after their files, one of them empty.
                ("a/sub", None),
                ("a/empty", None),
            ],
        );
        assert!(index_info(&ar).unwrap().1);
        let folder = materialize(&[ar.join("a")]).unwrap().remove(0);
        assert_eq!(fs::read(folder.join("one.txt")).unwrap(), b"one");
        assert_eq!(
            fs::read(folder.join("sub").join("two.txt")).unwrap(),
            b"two"
        );
        assert!(folder.join("empty").is_dir());
        assert!(!folder.parent().unwrap().join("b").exists());
        fs::remove_dir_all(cache_root(&ar).unwrap()).unwrap();
    }

    #[test]
    fn reading_stops_after_the_last_wanted_file() {
        let t = tempfile::tempdir().unwrap();
        let ar = t.path().join("cut.tar");
        tar_of(
            &ar,
            &[
                ("first.txt", Some(b"first")),
                ("second.txt", Some(&[b'x'; 4096])),
            ],
        );
        let items = index(&ar).unwrap();
        let members = member_paths(&items);
        // The second entry loses its end: reading past the first one now fails.
        let whole = fs::read(&ar).unwrap();
        fs::write(&ar, &whole[..512 + 512 + 512 + 100]).unwrap();
        let root = t.path().join("out");
        ensure_private_dir(&root).unwrap();
        let wanted = ["first.txt".to_string()];
        extract_entries(&ar, &root, &wanted, &members, Some(1)).unwrap();
        assert_eq!(
            fs::read(root.join("entries").join("first.txt")).unwrap(),
            b"first"
        );
        // Without the count the whole archive is read, as before.
        assert!(extract_entries(&ar, &root, &wanted, &members, None).is_err());
    }

    #[test]
    fn the_index_cache_drops_the_archive_unused_longest() {
        let t = tempfile::tempdir().unwrap();
        let archive = |n: usize| t.path().join(format!("cache{n:02}.tar"));
        for n in 0..=INDEX_CACHE {
            tar_of(&archive(n), &[("file.txt", Some(b"x"))]);
            index(&archive(n)).unwrap();
            // The first one stays in use.
            index(&archive(0)).unwrap();
        }
        let cached = cache().lock().unwrap();
        assert!(cached.len() <= INDEX_CACHE);
        assert!(cached.contains_key(&archive(0)));
        assert!(cached.contains_key(&archive(INDEX_CACHE)));
    }

    #[test]
    fn seven_z_members_come_from_their_blocks_only() {
        use sevenz_rust2::{ArchiveEntry, ArchiveWriter, SourceReader};
        let t = tempfile::tempdir().unwrap();
        let ar = t.path().join("blocks.7z");
        let mut w = ArchiveWriter::create(&ar).unwrap();
        // One block of three files, then a file of its own, a folder and an empty file.
        let solid: [(&str, &[u8]); 3] = [
            ("a/one.txt", b"one"),
            ("a/two.txt", b"two two"),
            ("b/three.txt", b"three"),
        ];
        w.push_archive_entries(
            solid
                .iter()
                .map(|(n, _)| ArchiveEntry::new_file(n))
                .collect(),
            solid.iter().map(|(_, d)| SourceReader::from(*d)).collect(),
        )
        .unwrap();
        w.push_archive_entry(ArchiveEntry::new_file("a/four.txt"), Some(&b"four"[..]))
            .unwrap();
        w.push_archive_entry::<&[u8]>(ArchiveEntry::new_directory("a/empty"), None)
            .unwrap();
        w.push_archive_entry(ArchiveEntry::new_file("a/nothing.txt"), Some(&b""[..]))
            .unwrap();
        w.finish().unwrap();

        // A file in the middle of the shared block, alone.
        let two = materialize(&[ar.join("a/two.txt")]).unwrap().remove(0);
        assert_eq!(fs::read(&two).unwrap(), b"two two");
        assert!(!two.with_file_name("one.txt").exists());
        // An empty file has no block.
        let nothing = materialize(&[ar.join("a/nothing.txt")]).unwrap().remove(0);
        assert_eq!(fs::read(&nothing).unwrap(), b"");
        // A folder with files in two blocks, an empty file and an empty folder.
        let folder = materialize(&[ar.join("a")]).unwrap().remove(0);
        for (name, data) in [
            ("one.txt", &b"one"[..]),
            ("two.txt", b"two two"),
            ("four.txt", b"four"),
            ("nothing.txt", b""),
        ] {
            assert_eq!(fs::read(folder.join(name)).unwrap(), data, "{name}");
        }
        assert!(folder.join("empty").is_dir());
        assert!(!folder.with_file_name("b").exists());
        fs::remove_dir_all(cache_root(&ar).unwrap()).unwrap();
    }

    #[test]
    fn seven_z_repeated_name_gives_its_last_entry_even_after_an_empty_one() {
        use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
        let t = tempfile::tempdir().unwrap();
        let ar = t.path().join("twice.7z");
        let mut w = ArchiveWriter::create(&ar).unwrap();
        w.push_archive_entry(ArchiveEntry::new_file("x.txt"), Some(&b""[..]))
            .unwrap();
        w.push_archive_entry(ArchiveEntry::new_file("x.txt"), Some(&b"later"[..]))
            .unwrap();
        w.push_archive_entry(ArchiveEntry::new_file("y.txt"), Some(&b"data"[..]))
            .unwrap();
        w.push_archive_entry(ArchiveEntry::new_file("y.txt"), Some(&b""[..]))
            .unwrap();
        w.finish().unwrap();
        let items = index(&ar).unwrap();
        let members = member_paths(&items);
        let root = t.path().join("out");
        ensure_private_dir(&root).unwrap();
        let wanted = ["x.txt".to_string(), "y.txt".to_string()];
        extract_entries(&ar, &root, &wanted, &members, None).unwrap();
        let read = |name: &str| fs::read(root.join("entries").join(name)).unwrap();
        assert_eq!(read("x.txt"), b"later");
        assert_eq!(read("y.txt"), b"");
    }

    fn sample_zip(dir: &Path) -> PathBuf {
        let p = dir.join("photos.zip");
        let mut w = zip::ZipWriter::new(fs::File::create(&p).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        w.start_file("readme.txt", o).unwrap();
        w.write_all(b"hello").unwrap();
        w.start_file("2024/iceland/a.jpg", o).unwrap(); // folders only implied
        w.write_all(b"jpeg-bytes").unwrap();
        w.start_file("2024/b.txt", o).unwrap();
        w.write_all(b"bee").unwrap();
        w.start_file("../evil.txt", o).unwrap();
        w.write_all(b"x").unwrap();
        w.finish().unwrap();
        p
    }

    #[test]
    fn splits_paths_into_archives() {
        let t = tempfile::tempdir().unwrap();
        let ar = sample_zip(t.path());
        assert_eq!(split(&ar), Some((ar.clone(), String::new())));
        assert_eq!(
            split(&ar.join("2024").join("b.txt")),
            Some((ar.clone(), "2024/b.txt".into()))
        );
        assert_eq!(split(t.path()), None, "a real folder");
        assert_eq!(split(&t.path().join("missing").join("x")), None);
        fs::write(t.path().join("log.gz"), b"x").unwrap();
        assert_eq!(
            split(&t.path().join("log.gz").join("x")),
            None,
            "single-file .gz is not browsable"
        );
    }

    #[test]
    fn lists_levels_with_implied_folders() {
        let t = tempfile::tempdir().unwrap();
        let ar = sample_zip(t.path());
        let top = list(&ar, "").unwrap();
        let names: Vec<_> = top.iter().map(|e| (e.name.as_str(), e.is_dir)).collect();
        assert_eq!(
            names,
            vec![("2024", true), ("readme.txt", false)],
            "no ../evil.txt"
        );
        assert!(top.iter().all(|e| e.readonly));
        assert_eq!(top[0].path, virtual_path(&ar, "2024"));
        let y = list(&ar, "2024").unwrap();
        assert_eq!(
            y.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["b.txt", "iceland"]
        );
        assert_eq!(y[0].size, 3);
        assert!(list(&ar, "nope").is_err());
        assert_eq!(folder_totals(&ar, "2024").unwrap(), (2, 13));
    }

    #[test]
    fn materializes_files_and_folders() {
        let t = tempfile::tempdir().unwrap();
        let ar = sample_zip(t.path());
        let plain = t.path().join("plain.txt");
        let got = materialize(&[ar.join("readme.txt"), plain.clone(), ar.join("2024")]).unwrap();
        assert_eq!(fs::read_to_string(&got[0]).unwrap(), "hello");
        assert_eq!(
            got[0].file_name().unwrap(),
            "readme.txt",
            "names are kept for copying"
        );
        assert_eq!(got[1], plain, "ordinary paths pass through");
        assert_eq!(fs::read_to_string(got[2].join("b.txt")).unwrap(), "bee");
        assert_eq!(
            fs::read(got[2].join("iceland/a.jpg")).unwrap(),
            b"jpeg-bytes"
        );
        // Second time: served from the cache.
        let again = materialize(&[ar.join("readme.txt")]).unwrap();
        assert_eq!(again[0], got[0]);
        let _ = fs::remove_dir_all(cache_root(&ar).unwrap());
    }

    #[test]
    fn materializes_colliding_names_without_aliasing_files_or_folders() {
        let t = tempfile::tempdir().unwrap();
        let names = [
            "contents/report:old.txt",
            "contents/report_old.txt",
            "contents/report_old~1.txt",
            "contents/folder:old/item.txt",
            "contents/folder_old/item.txt",
            "contents/folder_old~1/item.txt",
            "contents/Case.txt",
            "contents/case.txt",
            "contents/case~1.txt",
            "contents/aux.txt",
            "contents/_aux.txt",
            "contents/ends.",
            "contents/ends",
            "contents/ends ",
            "contents/...",
            "contents/_",
            "contents/Σ.txt",
            "contents/ς.txt",
            "contents/σ.txt",
        ];
        let contents: Vec<_> = (0..names.len()).map(|i| format!("{i:04}")).collect();
        let entries: Vec<_> = names
            .iter()
            .zip(&contents)
            .map(|(name, data)| (*name, data.as_bytes()))
            .collect();
        // zip's enclosed_name rejects some Windows-invalid names before indexing;
        // tar keeps them, so it exercises the cache's sanitization itself.
        let ar = t.path().join("members.tar");
        let mut writer = tar::Builder::new(fs::File::create(&ar).unwrap());
        for (name, data) in &entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            writer.append_data(&mut header, name, *data).unwrap();
        }
        writer.finish().unwrap();
        drop(writer);
        let mut cached = Vec::new();
        // Separate calls exercise readiness checks for equal-size colliding entries.
        for (name, data) in &entries {
            let got = materialize(&[ar.join(name)]).unwrap().remove(0);
            assert_eq!(fs::read(&got).unwrap(), *data, "{name}");
            cached.push(got);
        }
        assert_eq!(cached.iter().collect::<HashSet<_>>().len(), entries.len());
        let folder = materialize(&[ar.join("contents")]).unwrap().remove(0);
        for ((name, data), path) in entries.iter().zip(&cached) {
            assert!(path.starts_with(&folder), "{name}");
            assert_eq!(fs::read(path).unwrap(), *data, "{name}");
            assert_eq!(
                materialize(&[ar.join(name)]).unwrap()[0],
                *path,
                "folder and individual extraction must share the mapping"
            );
        }
        let safe = &cached[1];
        assert_eq!(safe.file_name().unwrap(), "report_old.txt");
        assert_eq!(
            cached[4].parent().unwrap().file_name().unwrap(),
            "folder_old"
        );
        fs::remove_dir_all(cache_root(&ar).unwrap()).unwrap();
    }

    #[test]
    fn cache_bookkeeping_and_staging_do_not_overwrite_members() {
        let t = tempfile::tempdir().unwrap();
        let entries: &[(&str, &[u8])] = &[
            (".used", b"archive bookkeeping name"),
            (".gall-part", b"archive staging name"),
            (".file.txt.gall-part", b"old staging name"),
            ("file.txt", b"file data"),
            ("entries/.used", b"nested bookkeeping name"),
        ];
        let ar = zip_with_entries(t.path(), entries);
        for _ in 0..2 {
            for (name, data) in entries {
                let got = materialize(&[ar.join(name)]).unwrap();
                assert_eq!(fs::read(&got[0]).unwrap(), *data, "{name}");
            }
        }
        fs::remove_dir_all(cache_root(&ar).unwrap()).unwrap();
    }

    #[test]
    fn concurrent_materializations_share_a_complete_cache() {
        let t = tempfile::tempdir().unwrap();
        let data = vec![b'x'; 2 * 1024 * 1024];
        let ar = zip_with_entries(t.path(), &[("folder/file.txt", &data)]);
        let barrier = std::sync::Barrier::new(8);
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        // Folder requests force extraction even after the first success.
                        for _ in 0..3 {
                            let paths =
                                materialize(&[ar.join("folder"), ar.join("folder/file.txt")])?;
                            assert_eq!(paths[0].join("file.txt"), paths[1]);
                            assert_eq!(fs::read(&paths[1]).unwrap(), data);
                        }
                        Ok::<_, CoreError>(())
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        for result in results {
            result.unwrap();
        }
        let path = materialize(&[ar.join("folder/file.txt")]).unwrap();
        assert_eq!(fs::read(&path[0]).unwrap(), data);
        fs::remove_dir_all(cache_root(&ar).unwrap()).unwrap();
    }

    #[test]
    fn encrypted_7z_asks_for_the_password() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("secret");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("plan.txt"), "top secret").unwrap();
        let ar = t.path().join("s.7z");
        sevenz_rust2::compress_to_path_encrypted(&src, &ar, "hunter2".into()).unwrap();
        let first = list(&ar, "");
        let msg = first
            .map(|_| String::new())
            .unwrap_or_else(|e| e.to_string());
        // Either the listing (encrypted headers) or the extraction asks for the password.
        let needs = msg.contains(PASSWORD_PREFIX)
            || materialize(&[ar.join("plan.txt")])
                .unwrap_err()
                .to_string()
                .contains(&format!("{PASSWORD_PREFIX}needed:"));
        assert!(needs, "{msg}");
        set_password(&ar, "wrong");
        let e = materialize(&[ar.join("plan.txt")]).unwrap_err().to_string();
        assert!(e.contains(&format!("{PASSWORD_PREFIX}wrong:")), "{e}");
        set_password(&ar, "hunter2");
        assert!(list(&ar, "").unwrap().iter().any(|e| e.name == "plan.txt"));
        let got = materialize(&[ar.join("plan.txt")]).unwrap();
        assert_eq!(fs::read_to_string(&got[0]).unwrap(), "top secret");
        let _ = fs::remove_dir_all(cache_root(&ar).unwrap());
    }

    #[test]
    fn tar_gz_and_7z_browse_too() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        fs::create_dir_all(src.join("docs")).unwrap();
        fs::write(src.join("docs/n.md"), "# hi").unwrap();
        for ext in ["tar.gz", "7z"] {
            let fmt = if ext == "7z" {
                ArchiveFormat::SevenZ
            } else {
                ArchiveFormat::TarGz
            };
            let ar = super::super::archive::compress(
                std::slice::from_ref(&src),
                &t.path().join(format!("a.{ext}")),
                fmt,
                &std::sync::atomic::AtomicBool::new(false),
                &mut |_| {},
            )
            .unwrap();
            let l = list(&ar, "src/docs").unwrap();
            assert_eq!(l.len(), 1, "{ext}");
            assert_eq!(l[0].name, "n.md");
            let got = materialize(&[ar.join("src").join("docs").join("n.md")]).unwrap();
            assert_eq!(fs::read_to_string(&got[0]).unwrap(), "# hi", "{ext}");
            let _ = fs::remove_dir_all(cache_root(&ar).unwrap());
        }
    }

    #[cfg(unix)]
    #[test]
    fn materialized_cache_and_staging_stay_private_under_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;

        // Isolate `umask(0o000)` in a child process so it never races with parallel tests.
        if std::env::var_os("GALL_TEST_UMASK_CHILD").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "fs::archive_view::tests::materialized_cache_and_staging_stay_private_under_permissive_umask",
                    "--nocapture",
                ])
                .env("GALL_TEST_UMASK_CHILD", "1")
                .status()
                .unwrap();
            assert!(status.success(), "isolated umask child test failed");
            return;
        }

        // SAFETY: runs inside a single-test isolated child process.
        unsafe {
            libc::umask(0o000);
        }

        let t = tempfile::tempdir().unwrap();
        let user_cache = t.path().join("cache");
        let ar = zip_with_entries(t.path(), &[("nested/secret.txt", b"confidential")]);
        let resolved =
            resolve_cache_paths(Some(user_cache), t.path(), create_fallback_cache_dir).unwrap();

        let got = materialize_in(&[ar.join("nested/secret.txt")], &resolved.base).unwrap();
        let cached_file = &got[0];
        assert_eq!(fs::read(cached_file).unwrap(), b"confidential");

        let root = cache_root_in(&resolved.base, &ar).unwrap();
        for dir in [
            resolved.base.as_path(),
            root.as_path(),
            &root.join("entries"),
            &root.join("entries/nested"),
        ] {
            let mode = fs::symlink_metadata(dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "dir {} must be 0700", dir.display());
        }
        for file in [cached_file.as_path(), &root.join(".used")] {
            let mode = fs::symlink_metadata(file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "file {} must be 0600", file.display());
        }

        // Exclusive staging file creation also stays 0600 and cleans up on drop.
        let (tmp_path, tmp_file) = create_private_staging_file(&root).unwrap();
        let guard = StagingFile(Some(tmp_path.clone()));
        assert_eq!(
            fs::symlink_metadata(&tmp_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(tmp_file);
        drop(guard);
        assert!(!tmp_path.exists());

        // Missing user cache fallback also stays 0700/0600 under umask 0000.
        let shared_tmp = t.path().join("sticky-tmp");
        fs::create_dir(&shared_tmp).unwrap();
        fs::set_permissions(&shared_tmp, fs::Permissions::from_mode(0o1777)).unwrap();
        let fallback = resolve_cache_paths(None, &shared_tmp, create_fallback_cache_dir).unwrap();
        assert_eq!(
            fs::symlink_metadata(&fallback.base)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let fb_got = materialize_in(&[ar.join("nested/secret.txt")], &fallback.base).unwrap();
        assert_eq!(
            fs::symlink_metadata(&fb_got[0])
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        // Second materialization reuses the cached file without re-extracting.
        let reused = materialize_in(&[ar.join("nested/secret.txt")], &resolved.base).unwrap();
        assert_eq!(reused, got);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_and_non_private_cache_roots_and_entries() {
        use std::os::unix::fs::PermissionsExt;

        let t = tempfile::tempdir().unwrap();
        let ar = zip_with_entries(t.path(), &[("secret.txt", b"top-secret")]);

        // 1. Symlinked user cache root (with and without trailing slash) and symlinked ancestor are rejected.
        let real_target = t.path().join("real-target");
        fs::create_dir(&real_target).unwrap();
        let sym_cache = t.path().join("sym-cache");
        std::os::unix::fs::symlink(&real_target, &sym_cache).unwrap();
        assert!(
            resolve_cache_paths(Some(sym_cache.clone()), t.path(), create_fallback_cache_dir)
                .is_err()
        );
        let sym_with_slash = PathBuf::from(format!("{}/", sym_cache.display()));
        assert!(
            resolve_cache_paths(Some(sym_with_slash), t.path(), create_fallback_cache_dir).is_err()
        );
        assert!(resolve_cache_paths(
            Some(sym_cache.join("subcache")),
            t.path(),
            create_fallback_cache_dir
        )
        .is_err());
        assert!(!real_target.join("subcache").exists());

        // 2. World-writable user cache root is rejected.
        let ww_cache = t.path().join("ww-cache");
        fs::create_dir(&ww_cache).unwrap();
        fs::set_permissions(&ww_cache, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(resolve_cache_paths(Some(ww_cache), t.path(), create_fallback_cache_dir).is_err());

        // 3. Symlinked `archives-private` directory is rejected.
        let cache_a = t.path().join("cache-a");
        fs::create_dir_all(cache_a.join("gall")).unwrap();
        std::os::unix::fs::symlink(
            &real_target,
            cache_a.join("gall").join(PRIVATE_CACHE_DIR_NAME),
        )
        .unwrap();
        assert!(resolve_cache_paths(Some(cache_a), t.path(), create_fallback_cache_dir).is_err());

        // 4. Group/world-readable `archives-private` directory is rejected.
        let cache_b = t.path().join("cache-b");
        let unsafe_base = cache_b.join("gall").join(PRIVATE_CACHE_DIR_NAME);
        fs::create_dir_all(&unsafe_base).unwrap();
        fs::set_permissions(&unsafe_base, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(resolve_cache_paths(Some(cache_b), t.path(), create_fallback_cache_dir).is_err());

        // 5. Symlinked or world-writable non-sticky temp_dir is rejected on Unix fallback.
        let sym_tmp = t.path().join("sym-tmp");
        std::os::unix::fs::symlink(&real_target, &sym_tmp).unwrap();
        let sym_tmp_slash = PathBuf::from(format!("{}/", sym_tmp.display()));
        assert!(resolve_cache_paths(None, &sym_tmp_slash, create_fallback_cache_dir).is_err());
        let non_sticky_tmp = t.path().join("non-sticky-tmp");
        fs::create_dir(&non_sticky_tmp).unwrap();
        fs::set_permissions(&non_sticky_tmp, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(resolve_cache_paths(None, &non_sticky_tmp, create_fallback_cache_dir).is_err());

        // 6. Symlinked cached member inside an otherwise valid root is rejected on reuse.
        let cache_c = t.path().join("cache-c");
        let resolved =
            resolve_cache_paths(Some(cache_c), t.path(), create_fallback_cache_dir).unwrap();
        let cached = materialize_in(&[ar.join("secret.txt")], &resolved.base).unwrap()[0].clone();
        let decoy = t.path().join("decoy.txt");
        fs::write(&decoy, "0123456789").unwrap();
        fs::remove_file(&cached).unwrap();
        std::os::unix::fs::symlink(&decoy, &cached).unwrap();
        assert!(materialize_in(&[ar.join("secret.txt")], &resolved.base).is_err());
        assert_eq!(fs::read_to_string(&decoy).unwrap(), "0123456789");
    }

    #[test]
    fn materialize_passes_ordinary_paths_through_without_touching_cache() {
        let t = tempfile::tempdir().unwrap();
        let plain = t.path().join("plain.txt");
        fs::write(&plain, "ordinary").unwrap();
        let missing_base = t.path().join("does-not-exist");
        let got = materialize_in(std::slice::from_ref(&plain), &missing_base).unwrap();
        assert_eq!(got, vec![plain]);
        assert!(!missing_base.exists());
    }

    #[cfg(unix)]
    #[test]
    fn missing_user_cache_dir_uses_private_random_fallback_and_reuses_entries() {
        use std::os::unix::fs::PermissionsExt;

        let t = tempfile::tempdir().unwrap();
        let shared_tmp = t.path().join("shared-tmp");
        fs::create_dir(&shared_tmp).unwrap();
        // Plant an untrusted entry at the old predictable shared-temp path.
        let predictable = shared_tmp.join("gall-archives");
        fs::write(&predictable, "attacker-owned").unwrap();

        let ar = zip_with_entries(t.path(), &[("notes.txt", b"hello fallback")]);
        let resolved = resolve_cache_paths(None, &shared_tmp, create_fallback_cache_dir).unwrap();
        assert_ne!(resolved.base, predictable);
        assert!(resolved.base.starts_with(&shared_tmp));
        assert_eq!(
            fs::symlink_metadata(&resolved.base)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(resolved.legacy.is_none());

        let first = materialize_in(&[ar.join("notes.txt")], &resolved.base).unwrap();
        assert_eq!(fs::read(&first[0]).unwrap(), b"hello fallback");
        let second = materialize_in(&[ar.join("notes.txt")], &resolved.base).unwrap();
        assert_eq!(first, second);
        assert_eq!(fs::read_to_string(&predictable).unwrap(), "attacker-owned");
    }

    #[test]
    fn retire_legacy_cache_removes_buckets_and_preserves_user_data_and_links() {
        let t = tempfile::tempdir().unwrap();
        let legacy = t.path().join("archives");
        fs::create_dir_all(&legacy).unwrap();

        // Recognized legacy cache bucket.
        let bucket = legacy.join("0123456789abcdef");
        fs::create_dir_all(bucket.join("entries")).unwrap();
        fs::write(bucket.join(".used"), b"").unwrap();
        fs::write(bucket.join("entries/old.txt"), b"old cached data").unwrap();

        // Non-cache user data inside legacy dir.
        let user_file = legacy.join("my-notes.txt");
        fs::write(&user_file, "keep me").unwrap();
        let user_dir = legacy.join("user-folder");
        fs::create_dir(&user_dir).unwrap();
        fs::write(user_dir.join("keep.txt"), "still here").unwrap();

        // Untrusted symlinks must never be followed or have their targets deleted.
        let external = t.path().join("external-user-dir");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("precious.txt"), "do not delete").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&external, legacy.join("fedcba9876543210")).unwrap();
            let linked_bucket = legacy.join("1111222233334444");
            fs::create_dir_all(linked_bucket.join("entries")).unwrap();
            fs::write(linked_bucket.join(".used"), b"").unwrap();
            std::os::unix::fs::symlink(&external, linked_bucket.join("entries/escape")).unwrap();
        }

        retire_legacy_cache(&legacy);

        assert!(!bucket.exists(), "genuine legacy cache bucket is retired");
        assert_eq!(fs::read_to_string(&user_file).unwrap(), "keep me");
        assert_eq!(
            fs::read_to_string(user_dir.join("keep.txt")).unwrap(),
            "still here"
        );
        assert_eq!(
            fs::read_to_string(external.join("precious.txt")).unwrap(),
            "do not delete"
        );

        // A clean legacy cache containing only buckets is removed completely.
        let clean_legacy = t.path().join("clean-archives");
        let clean_bucket = clean_legacy.join("abcdef0123456789");
        fs::create_dir_all(clean_bucket.join("entries")).unwrap();
        fs::write(clean_bucket.join(".used"), b"").unwrap();
        fs::write(clean_bucket.join("entries/a.txt"), b"a").unwrap();
        retire_legacy_cache(&clean_legacy);
        assert!(!clean_legacy.exists());

        // A symlinked legacy root itself is never followed or deleted.
        #[cfg(unix)]
        {
            let sym_legacy = t.path().join("sym-archives");
            std::os::unix::fs::symlink(&external, &sym_legacy).unwrap();
            retire_legacy_cache(&sym_legacy);
            assert!(fs::symlink_metadata(&sym_legacy).is_ok());
            assert_eq!(
                fs::read_to_string(external.join("precious.txt")).unwrap(),
                "do not delete"
            );
        }
    }
}
