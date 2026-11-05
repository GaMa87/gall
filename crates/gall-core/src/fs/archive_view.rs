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
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
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

/// Archive path → (version, items).
type IndexCache = Mutex<HashMap<PathBuf, (Stamp, Arc<Vec<Item>>)>>;

fn cache() -> &'static IndexCache {
    static C: OnceLock<IndexCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// All items of `archive`, folders included, sorted by name. Cached until it changes.
pub fn index(archive: &Path) -> Result<Arc<Vec<Item>>> {
    let st = stamp(archive)?;
    if let Some((s, items)) = cache().lock().unwrap().get(archive) {
        if *s == st {
            return Ok(items.clone());
        }
    }
    let (format, _) = detect(archive).ok_or_else(|| not_archive(archive))?;
    let raw = read_index(archive, format)?;
    let items = Arc::new(with_folders(raw));
    let mut c = cache().lock().unwrap();
    if c.len() > 16 {
        c.clear();
    }
    c.insert(archive.to_path_buf(), (st, items.clone()));
    Ok(items)
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

fn not_archive(p: &Path) -> CoreError {
    CoreError::Other(format!("{} is not an archive Gall can browse", p.display()))
}

/// `a/b/c.txt` → the item plus folders `a` and `a/b`; duplicates merged, sorted.
fn with_folders(raw: Vec<Item>) -> Vec<Item> {
    let mut all: BTreeMap<String, Item> = BTreeMap::new();
    for it in raw {
        let mut parent = it.name.as_str();
        while let Some((p, _)) = parent.rsplit_once('/') {
            all.entry(p.to_string()).or_insert_with(|| Item {
                name: p.to_string(),
                is_dir: true,
                size: 0,
                modified: None,
            });
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
    all.into_values().collect()
}

/// `/`-joined normal components, or None for names that climb out or are absolute.
fn clean(name: &str) -> Option<String> {
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
            let mut z = zip::ZipArchive::new(file).map_err(|e| err("zip", &e))?;
            for i in 0..z.len() {
                let f = z.by_index_raw(i).map_err(|e| err("zip", &e))?;
                let Some(name) = f.enclosed_name().and_then(|p| clean(&p.to_string_lossy())) else {
                    continue;
                };
                out.push(Item {
                    name,
                    is_dir: f.is_dir(),
                    size: f.size(),
                    modified: f.last_modified().and_then(zip_time),
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

/// One cache root per archive version (see `cache_base`). Members live under
/// `entries/`, separate from the root's bookkeeping and staging files.
fn cache_root(archive: &Path) -> Result<PathBuf> {
    let st = stamp(archive)?;
    let mut h = DefaultHasher::new();
    ("member-cache-v2", archive, st.0, st.1).hash(&mut h);
    Ok(cache_base().join(format!("{:016x}", h.finish())))
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

/// Windows: the per-user temp folder. Elsewhere `/tmp` is shared by all users (another
/// user could own `gall-archives`) and often a small tmpfs, so the user's cache folder.
fn cache_base() -> PathBuf {
    if cfg!(windows) {
        return std::env::temp_dir().join("gall-archives");
    }
    dirs::cache_dir()
        .map(|d| d.join("gall").join("archives"))
        .unwrap_or_else(|| std::env::temp_dir().join("gall-archives"))
}

/// Removes extracted archive folders not used for a day (once per run).
fn prune_once() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let Ok(rd) = fs::read_dir(cache_base()) else {
            return;
        };
        let day = Duration::from_secs(24 * 3600);
        for e in rd.flatten() {
            let old = fs::metadata(e.path().join(".used"))
                .or_else(|_| e.metadata())
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > day);
            if old {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    });
}

/// Real files for virtual paths: each entry (a folder with everything in it) is
/// extracted into the cache once, and its cached path returned in the same order.
/// Ordinary paths are returned unchanged, so callers can pass a mixed selection.
pub fn materialize(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    prune_once();
    let mut out = paths.to_vec();
    // Group the virtual paths by archive: one pass over each archive.
    let mut by_archive: HashMap<PathBuf, Vec<(usize, String)>> = HashMap::new();
    for (i, p) in paths.iter().enumerate() {
        if let Some((ar, inner)) = split(p) {
            if inner.is_empty() {
                continue; // the archive file itself
            }
            by_archive.entry(ar).or_default().push((i, inner));
        }
    }
    for (archive, wanted) in by_archive {
        let root = cache_root(&archive)?;
        let lock = extraction_lock(&root);
        let _guard = lock.lock().unwrap();
        let items = index(&archive)?;
        let members = member_paths(&items);
        let mut need: Vec<String> = Vec::new();
        for (i, inner) in &wanted {
            let it = items
                .iter()
                .find(|it| &it.name == inner)
                .ok_or_else(|| CoreError::NotFound(PathBuf::from(virtual_path(&archive, inner))))?;
            let target = root.join("entries").join(&members[inner]);
            // A folder is re-extracted each time (cheap to check, hard to know if complete);
            // a file is reused when it is already there with the right size.
            let ready = !it.is_dir
                && fs::metadata(&target).is_ok_and(|m| m.is_file() && m.len() == it.size);
            if !ready {
                need.push(inner.clone());
            }
            out[*i] = target;
        }
        if !need.is_empty() {
            fs::create_dir_all(&root).ctx("create", &root)?;
            extract_entries(&archive, &root, &need, &members)?;
        }
        let _ = filetime_touch(&root);
    }
    Ok(out)
}

/// Marks a cache folder as used, outside the extracted members' namespace.
fn filetime_touch(dir: &Path) -> io::Result<()> {
    let f = dir.join(".used");
    fs::write(f, b"")
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
/// each file written to a temporary name first so readers never see half a file.
fn extract_entries(
    archive: &Path,
    root: &Path,
    wanted: &[String],
    members: &HashMap<String, PathBuf>,
) -> Result<()> {
    let (format, _) = detect(archive).ok_or_else(|| not_archive(archive))?;
    let file = fs::File::open(archive).ctx("open", archive)?;
    let err = |what: &str, e: &dyn std::fmt::Display| CoreError::Other(format!("{what}: {e}"));
    let write = |name: &str, is_dir: bool, data: &mut dyn Read| -> Result<()> {
        let out = root.join("entries").join(&members[name]);
        if is_dir {
            return fs::create_dir_all(&out).ctx("create", &out);
        }
        if let Some(p) = out.parent() {
            fs::create_dir_all(p).ctx("create", p)?;
        }
        // Writes are serialized per root; no archive member can alias this file.
        let tmp = root.join(".gall-part");
        {
            let mut w = fs::File::create(&tmp).ctx("create", &tmp)?;
            io::copy(data, &mut w).ctx("write", &tmp)?;
        }
        fs::rename(&tmp, &out).ctx("move", &out)
    };
    match format {
        ArchiveFormat::Zip => {
            let mut z = zip::ZipArchive::new(file).map_err(|e| err("zip", &e))?;
            for i in 0..z.len() {
                // Names come from the raw entry: only wanted ones are decrypted.
                let (name, skip) = {
                    let f = z.by_index_raw(i).map_err(|e| err("zip", &e))?;
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
                        // A wrong ZipCrypto password shows up as a bad checksum while reading.
                        return Err(if password_for(archive).is_some() {
                            need_password(archive)
                        } else {
                            e
                        });
                    }
                }
            }
        }
        ArchiveFormat::SevenZ => {
            let mut ar = sevenz_rust2::ArchiveReader::new(file, sevenz_password(archive))
                .map_err(|e| sevenz_err(archive, e))?;
            let mut failed: Option<CoreError> = None;
            ar.for_each_entries(|entry, data| {
                if let Some(name) = clean(entry.name()) {
                    if wanted_by(&name, wanted) {
                        if let Err(e) = write(&name, entry.is_directory(), data) {
                            failed = Some(e);
                            return Ok(false);
                        }
                    }
                }
                // Unwanted entries must still be read through in solid archives.
                io::copy(data, &mut io::sink())?;
                Ok(true)
            })
            .map_err(|e| sevenz_err(archive, e))?;
            if let Some(e) = failed {
                // With a wrong password decryption yields garbage the decoder rejects.
                return Err(if password_for(archive).is_some() {
                    need_password(archive)
                } else {
                    e
                });
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
                    write(&name, kind.is_dir(), &mut e)?;
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
}
