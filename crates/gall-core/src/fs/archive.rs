//! Archives: create (zip, tar.gz, 7z) and extract (zip, tar with gz/xz/bz2/zst,
//! 7z, and single-file gz/xz/bz2/zst). Everything is pure Rust, so it behaves the
//! same on Linux, Windows and macOS.
//!
//! Both directions write to a hidden staging name first and move the result into
//! place at the end, so a cancelled or failed task leaves nothing half-written.
//! Entry names that would escape the destination (`..`, absolute paths) are skipped.

use crate::error::{CoreError, IoContext, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::ops::publish_unique;

/// Progress sink: bytes just processed.
pub type Tick<'a> = dyn FnMut(u64) + 'a;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    TarXz,
    TarBz2,
    TarZst,
    SevenZ,
    /// Single compressed file (not a tar).
    Gz,
    Xz,
    Bz2,
    Zst,
}

impl ArchiveFormat {
    /// Formats Gall can create.
    pub fn can_create(self) -> bool {
        matches!(self, Self::Zip | Self::TarGz | Self::SevenZ)
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::TarXz => "tar.xz",
            Self::TarBz2 => "tar.bz2",
            Self::TarZst => "tar.zst",
            Self::SevenZ => "7z",
            Self::Gz => "gz",
            Self::Xz => "xz",
            Self::Bz2 => "bz2",
            Self::Zst => "zst",
        }
    }
}

/// The last extensions of the names `detect` recognises.
pub(crate) const EXTENSIONS: &[&str] = &[
    "tgz", "txz", "tbz2", "tbz", "tzst", "tar", "zip", "7z", "gz", "xz", "bz2", "zst",
];

/// Recognises an archive by its file name. Returns the format and the name without
/// the archive extension (`photos.tar.gz` → `photos`).
pub fn detect(path: &Path) -> Option<(ArchiveFormat, String)> {
    use ArchiveFormat::*;
    let name = path.file_name()?.to_string_lossy().into_owned();
    const SUFFIXES: &[(&str, ArchiveFormat)] = &[
        (".tar.gz", TarGz),
        (".tgz", TarGz),
        (".tar.xz", TarXz),
        (".txz", TarXz),
        (".tar.bz2", TarBz2),
        (".tbz2", TarBz2),
        (".tbz", TarBz2),
        (".tar.zst", TarZst),
        (".tzst", TarZst),
        (".tar", Tar),
        (".zip", Zip),
        (".7z", SevenZ),
        (".gz", Gz),
        (".xz", Xz),
        (".bz2", Bz2),
        (".zst", Zst),
    ];
    SUFFIXES.iter().find_map(|(suf, f)| {
        let cut = name
            .len()
            .checked_sub(suf.len())
            .filter(|&c| c > 0 && name.is_char_boundary(c))?;
        name[cut..]
            .eq_ignore_ascii_case(suf)
            .then(|| (*f, name[..cut].to_string()))
    })
}

// ───────────────────────────── helpers ─────────────────────────────

/// Reader that reports bytes read and stops with an error once `cancel` is set.
struct Counting<'t, R> {
    inner: R,
    cancel: &'t AtomicBool,
    tick: &'t mut Tick<'t>,
}

impl<R: Read> Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("cancelled"));
        }
        let n = self.inner.read(buf)?;
        if n > 0 {
            (self.tick)(n as u64);
        }
        Ok(n)
    }
}

impl<R: Seek> Seek for Counting<'_, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Maps any error to `Cancelled` when the user cancelled (the readers abort with an I/O error).
fn check<T, E: std::fmt::Display>(
    r: std::result::Result<T, E>,
    cancel: &AtomicBool,
    what: &str,
) -> Result<T> {
    r.map_err(|e| {
        if cancel.load(Ordering::Relaxed) {
            CoreError::Cancelled
        } else {
            CoreError::Other(format!("{what}: {e}"))
        }
    })
}

/// A relative path inside an archive, or None if it is absolute or climbs out with `..`.
pub(crate) fn safe_relative(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// An archive path made valid on Windows, like 7-Zip does: characters Windows forbids
/// become `_` (`a:b` would otherwise write an alternate data stream), trailing dots and
/// spaces go, and device names (`aux.txt`, `CON`) get a `_` prefix. Elsewhere unchanged.
pub(crate) fn local_name(rel: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return rel;
    }
    rel.components()
        .map(|c| windows_component(&c.as_os_str().to_string_lossy()))
        .collect()
}

fn windows_component(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c < ' ' || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    while s.ends_with(['.', ' ']) {
        s.pop();
    }
    if s.is_empty() {
        return "_".into();
    }
    let stem = s
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    let device = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$" | "CONFIG$"
    ) || matches!(
        stem.strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT")),
        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
    );
    if device {
        s.insert(0, '_');
    }
    s
}

/// Removes a staging path when dropped, unless disarmed.
struct Staging(Option<PathBuf>);
impl Staging {
    fn disarm(mut self) {
        self.0 = None;
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = if p.is_dir() {
                fs::remove_dir_all(&p)
            } else {
                fs::remove_file(&p)
            };
        }
    }
}

static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);

fn staging_name(dir: &Path, hint: &str) -> PathBuf {
    let pid = std::process::id();
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    dir.join(format!(".gall-{hint}-{pid}-{n:x}-{seq:x}.part"))
}

fn create_staging_dir(dir: &Path, hint: &str) -> Result<PathBuf> {
    for _ in 0..1024 {
        let candidate = staging_name(dir, hint);
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(CoreError::io("create", &candidate, e)),
        }
    }
    Err(CoreError::AlreadyExists(staging_name(dir, hint)))
}

fn create_staging_file(dir: &Path, hint: &str) -> Result<(PathBuf, fs::File)> {
    for _ in 0..1024 {
        let candidate = staging_name(dir, hint);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(CoreError::io("create", &candidate, e)),
        }
    }
    Err(CoreError::AlreadyExists(staging_name(dir, hint)))
}

/// Ensures an archive's stem is a safe single filename component inside a directory.
fn safe_stem(stem: &str) -> String {
    let fallback = "archive";
    let Some(rel) = safe_relative(stem) else {
        return fallback.into();
    };
    let mut comps = rel.components();
    let Some(Component::Normal(first)) = comps.next() else {
        return fallback.into();
    };
    if comps.next().is_some() {
        return fallback.into();
    }
    let clean = local_name(PathBuf::from(first));
    let out = clean.to_string_lossy().into_owned();
    if out.is_empty() || out == "." || out == ".." {
        fallback.into()
    } else {
        out
    }
}

/// Files and folders under `sources`, each with its name inside the archive
/// (relative to the source's parent folder), parents before children.
fn collect(sources: &[PathBuf]) -> Result<Vec<(PathBuf, String, fs::Metadata)>> {
    let mut out = Vec::new();
    for src in sources {
        let base = src.parent().unwrap_or(Path::new(""));
        for e in walkdir::WalkDir::new(src)
            .follow_links(false)
            .sort_by_file_name()
        {
            let e = e.map_err(|e| CoreError::Other(e.to_string()))?;
            let rel = e.path().strip_prefix(base).unwrap_or(e.path());
            let name = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let meta = e.path().symlink_metadata().ctx("stat", e.path())?;
            out.push((e.path().to_path_buf(), name, meta));
        }
    }
    Ok(out)
}

/// Total bytes a compress task reads (regular files only).
pub fn source_bytes(sources: &[PathBuf]) -> u64 {
    collect(sources)
        .map(|v| {
            v.iter()
                .filter(|(_, _, m)| m.is_file())
                .map(|(_, _, m)| m.len())
                .sum()
        })
        .unwrap_or(0)
}

// ───────────────────────────── compress ─────────────────────────────

/// Size of the buffer between an archive writer and its file: writers emit many small pieces
/// (headers, deflate blocks), and each unbuffered piece is a system call.
const WRITE_BUFFER: usize = 256 * 1024;

/// Packs `sources` into a new archive at `target` (a unique name is chosen if it
/// exists). Returns the path written.
pub fn compress(
    sources: &[PathBuf],
    target: &Path,
    format: ArchiveFormat,
    cancel: &AtomicBool,
    tick: &mut Tick,
) -> Result<PathBuf> {
    compress_with_publisher(sources, target, format, cancel, tick, publish_unique)
}

fn compress_with_publisher(
    sources: &[PathBuf],
    target: &Path,
    format: ArchiveFormat,
    cancel: &AtomicBool,
    tick: &mut Tick,
    publish: impl FnOnce(&Path, &Path, &str) -> Result<PathBuf>,
) -> Result<PathBuf> {
    if !format.can_create() {
        return Err(CoreError::Other(format!(
            "can't create .{} archives",
            format.extension()
        )));
    }
    let dir = target
        .parent()
        .ok_or_else(|| CoreError::NotFound(target.into()))?;
    let name = target
        .file_name()
        .ok_or_else(|| CoreError::NotFound(target.into()))?
        .to_string_lossy()
        .into_owned();
    let entries = collect(sources)?;
    let (tmp, file) = create_staging_file(dir, "pack")?;
    let guard = Staging(Some(tmp.clone()));
    match format {
        ArchiveFormat::Zip => write_zip(file, &entries, cancel, tick)?,
        ArchiveFormat::TarGz => write_tar_gz(file, &entries, cancel, tick)?,
        ArchiveFormat::SevenZ => write_7z(file, &entries, cancel, tick)?,
        _ => unreachable!(),
    }
    let dest = publish(&tmp, dir, &name)?;
    guard.disarm();
    Ok(dest)
}

fn open_counting<'t>(
    path: &Path,
    cancel: &'t AtomicBool,
    tick: &'t mut Tick<'t>,
) -> Result<Counting<'t, fs::File>> {
    Ok(Counting {
        inner: fs::File::open(path).ctx("open", path)?,
        cancel,
        tick,
    })
}

fn write_zip(
    file: fs::File,
    entries: &[(PathBuf, String, fs::Metadata)],
    cancel: &AtomicBool,
    tick: &mut Tick,
) -> Result<()> {
    use zip::write::SimpleFileOptions;
    let mut w = zip::ZipWriter::new(io::BufWriter::with_capacity(WRITE_BUFFER, file));
    for (path, name, meta) in entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let mut opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            opts = opts.unix_permissions(meta.permissions().mode() & 0o7777);
        }
        if meta.is_dir() {
            check(w.add_directory(format!("{name}/"), opts), cancel, "zip")?;
        } else if meta.file_type().is_symlink() {
            let target = fs::read_link(path).ctx("read link", path)?;
            check(
                w.add_symlink(name.as_str(), target.to_string_lossy().as_ref(), opts),
                cancel,
                "zip",
            )?;
        } else if meta.is_file() {
            opts = opts.large_file(meta.len() >= u32::MAX as u64);
            check(w.start_file(name.as_str(), opts), cancel, "zip")?;
            let mut r = open_counting(path, cancel, tick)?;
            check(io::copy(&mut r, &mut w), cancel, "zip")?;
        }
    }
    let mut out = check(w.finish(), cancel, "zip")?;
    check(io::Write::flush(&mut out), cancel, "zip")?;
    Ok(())
}

fn write_tar_gz(
    file: fs::File,
    entries: &[(PathBuf, String, fs::Metadata)],
    cancel: &AtomicBool,
    tick: &mut Tick,
) -> Result<()> {
    let gz = flate2::write::GzEncoder::new(
        io::BufWriter::with_capacity(WRITE_BUFFER, file),
        flate2::Compression::default(),
    );
    let mut b = tar::Builder::new(gz);
    b.follow_symlinks(false);
    for (path, name, meta) in entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        if meta.is_file() {
            let mut h = tar::Header::new_gnu();
            h.set_metadata(meta);
            let r = open_counting(path, cancel, tick)?;
            check(b.append_data(&mut h, name, r), cancel, "tar")?;
        } else {
            check(b.append_path_with_name(path, name), cancel, "tar")?;
        }
    }
    let gz = check(b.into_inner(), cancel, "tar")?;
    let mut out = check(gz.finish(), cancel, "gzip")?;
    check(io::Write::flush(&mut out), cancel, "gzip")?;
    Ok(())
}

fn write_7z(
    file: fs::File,
    entries: &[(PathBuf, String, fs::Metadata)],
    cancel: &AtomicBool,
    tick: &mut Tick,
) -> Result<()> {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    let mut w = check(ArchiveWriter::new(file), cancel, "7z")?;
    for (path, name, meta) in entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let entry = ArchiveEntry::from_path(path, name.clone());
        if meta.is_file() {
            let r = open_counting(path, cancel, tick)?;
            check(w.push_archive_entry(entry, Some(r)), cancel, "7z")?;
        } else if meta.is_dir() {
            check(w.push_archive_entry::<fs::File>(entry, None), cancel, "7z")?;
        } // symlinks are not stored in 7z archives
    }
    check(w.finish(), cancel, "7z")?;
    Ok(())
}

// ───────────────────────────── extract ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ExtractMode {
    /// Into the destination folder; contents with more than one top-level item get
    /// their own folder named after the archive, so they never spill out.
    #[default]
    Smart,
    /// Always into a new folder named after the archive.
    Folder,
}

/// Extracts `archive` into `dest_dir`. Returns the paths created there (one folder
/// or one item), so the task can be undone.
pub fn extract(
    archive: &Path,
    dest_dir: &Path,
    mode: ExtractMode,
    cancel: &AtomicBool,
    tick: &mut Tick,
) -> Result<Vec<PathBuf>> {
    extract_with_publisher(archive, dest_dir, mode, cancel, tick, publish_unique)
}

fn extract_with_publisher(
    archive: &Path,
    dest_dir: &Path,
    mode: ExtractMode,
    cancel: &AtomicBool,
    tick: &mut Tick,
    publish: impl FnOnce(&Path, &Path, &str) -> Result<PathBuf>,
) -> Result<Vec<PathBuf>> {
    let (format, raw_stem) = detect(archive).ok_or_else(|| {
        CoreError::Other(format!("{} is not a supported archive", archive.display()))
    })?;
    let stem = safe_stem(&raw_stem);
    if !dest_dir.is_dir() {
        return Err(CoreError::NotFound(dest_dir.into()));
    }
    let stage = create_staging_dir(dest_dir, "extract")?;
    let guard = Staging(Some(stage.clone()));
    let allow_smart_unwrap = {
        let r = open_counting(archive, cancel, tick)?;
        match format {
            ArchiveFormat::Zip => read_zip(r, &stage, mode, cancel, archive)?,
            ArchiveFormat::SevenZ => {
                read_7z(r, &stage, cancel, archive)?;
                true
            }
            ArchiveFormat::Tar => read_tar(r, &stage, mode, cancel)?,
            ArchiveFormat::TarGz => read_tar(
                flate2::read::MultiGzDecoder::new(io::BufReader::new(r)),
                &stage,
                mode,
                cancel,
            )?,
            ArchiveFormat::TarXz => read_tar(
                lzma_rust2::XzReader::new(io::BufReader::new(r), true),
                &stage,
                mode,
                cancel,
            )?,
            ArchiveFormat::TarBz2 => read_tar(
                bzip2_rs::DecoderReader::new(io::BufReader::new(r)),
                &stage,
                mode,
                cancel,
            )?,
            ArchiveFormat::TarZst => read_tar(zstd_reader(r, cancel)?, &stage, mode, cancel)?,
            ArchiveFormat::Gz => {
                read_single(
                    flate2::read::MultiGzDecoder::new(io::BufReader::new(r)),
                    &stage.join(&stem),
                    cancel,
                )?;
                true
            }
            ArchiveFormat::Xz => {
                read_single(
                    lzma_rust2::XzReader::new(io::BufReader::new(r), true),
                    &stage.join(&stem),
                    cancel,
                )?;
                true
            }
            ArchiveFormat::Bz2 => {
                read_single(
                    bzip2_rs::DecoderReader::new(io::BufReader::new(r)),
                    &stage.join(&stem),
                    cancel,
                )?;
                true
            }
            ArchiveFormat::Zst => {
                read_single(zstd_reader(r, cancel)?, &stage.join(&stem), cancel)?;
                true
            }
        }
    };
    let top: Vec<PathBuf> = fs::read_dir(&stage)
        .ctx("read", &stage)?
        .flatten()
        .map(|e| e.path())
        .collect();
    let placed = if mode == ExtractMode::Smart
        && allow_smart_unwrap
        && top.len() == 1
        && !fs::symlink_metadata(&top[0])
            .ctx("stat", &top[0])?
            .file_type()
            .is_symlink()
    {
        let item = &top[0];
        let name = item.file_name().unwrap().to_string_lossy().into_owned();
        publish(item, dest_dir, &name)?
    } else {
        let dest = publish(&stage, dest_dir, &stem)?;
        guard.disarm();
        return Ok(vec![dest]);
    };
    drop(guard); // removes the now empty staging folder
    Ok(vec![placed])
}

fn zstd_reader<R: Read>(
    r: R,
    cancel: &AtomicBool,
) -> Result<ruzstd::decoding::StreamingDecoder<io::BufReader<R>, ruzstd::decoding::FrameDecoder>> {
    check(
        ruzstd::decoding::StreamingDecoder::new(io::BufReader::new(r)),
        cancel,
        "zstd",
    )
}

fn read_single<R: Read>(mut r: R, out: &Path, cancel: &AtomicBool) -> Result<()> {
    let mut f = fs::File::create(out).ctx("create", out)?;
    check(io::copy(&mut r, &mut f), cancel, "decompress")?;
    Ok(())
}

/// Deferred link entry captured while streaming an archive before installing links.
enum PendingLink {
    Hardlink(PathBuf),
    Symlink(PathBuf),
}

fn read_tar<R: Read>(r: R, stage: &Path, mode: ExtractMode, cancel: &AtomicBool) -> Result<bool> {
    let mut ar = tar::Archive::new(r);
    // Write files and create real directories before installing hardlinks or symlinks.
    let mut pending_links: BTreeMap<PathBuf, (usize, PendingLink)> = BTreeMap::new();
    let mut file_seq: HashMap<PathBuf, usize> = HashMap::new();
    let mut seq = 0usize;
    let mut made_dir = PathBuf::new();

    for e in check(ar.entries(), cancel, "tar")? {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        seq += 1;
        let mut e = check(e, cancel, "tar")?;
        let entry_type = e.header().entry_type();
        // Old V7/BSD tar records directories as Regular entries with a trailing `/`.
        let is_v7_dir = entry_type.is_file() && e.path_bytes().ends_with(b"/");
        let Ok(raw_path) = e.path() else {
            continue;
        };
        let Some(rel) = safe_relative(&raw_path.to_string_lossy()) else {
            continue;
        };
        let rel = local_name(rel);
        let out = stage.join(&rel);

        if entry_type == tar::EntryType::Directory || is_v7_dir {
            clear_superseded_links(&mut pending_links, &rel);
            if fs::symlink_metadata(&out).is_ok_and(|m| m.is_file()) {
                let _ = fs::remove_file(&out);
            }
            fs::create_dir_all(&out).ctx("create", &out)?;
            continue;
        }

        match entry_type {
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                clear_superseded_links(&mut pending_links, &rel);
                if let Some(p) = out.parent().filter(|p| *p != made_dir) {
                    if fs::symlink_metadata(p).is_ok_and(|m| m.is_file()) {
                        continue;
                    }
                    fs::create_dir_all(p).ctx("create", p)?;
                    made_dir = p.to_path_buf();
                }
                if fs::symlink_metadata(&out).is_ok_and(|m| m.is_dir()) {
                    continue;
                }
                let mut w = fs::File::create(&out).ctx("create", &out)?;
                check(io::copy(&mut e, &mut w), cancel, "tar")?;
                #[cfg(unix)]
                if let Ok(mode) = e.header().mode() {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = w.set_permissions(fs::Permissions::from_mode(mode & 0o777));
                }
                if let Ok(mtime) = e.header().mtime() {
                    if mtime > 0 {
                        let _ = w.set_modified(
                            std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime),
                        );
                    }
                }
                file_seq.insert(rel, seq);
            }
            tar::EntryType::Link => {
                if let Some(target) = check(e.link_name(), cancel, "tar")? {
                    pending_links.insert(rel, (seq, PendingLink::Hardlink(target.into_owned())));
                }
            }
            tar::EntryType::Symlink => {
                if let Some(target) = check(e.link_name(), cancel, "tar")? {
                    pending_links.insert(rel, (seq, PendingLink::Symlink(target.into_owned())));
                }
            }
            // Skip device nodes, FIFOs, and non-payload metadata headers.
            _ => {}
        }
    }

    let mut hardlinks: Vec<(usize, PathBuf, PathBuf)> = Vec::new();
    let mut symlinks = Vec::new();
    for (rel, (link_seq, kind)) in pending_links {
        if file_seq.get(&rel).is_some_and(|&s| s > link_seq) {
            continue;
        }
        match kind {
            PendingLink::Hardlink(target) => hardlinks.push((link_seq, rel, target)),
            PendingLink::Symlink(target) => symlinks.push((rel, target)),
        }
    }
    // Preserve archive order so hardlink chains resolve targets created earlier in the stream.
    hardlinks.sort_by_key(|(link_seq, _, _)| *link_seq);
    let ordered_hardlinks: Vec<(PathBuf, PathBuf)> = hardlinks
        .into_iter()
        .map(|(_, rel, target)| (rel, target))
        .collect();

    install_archive_links(stage, mode, &ordered_hardlinks, &symlinks, cancel)
}

/// Drops deferred links at `rel` or any of its ancestor directories once a real
/// file or directory claims that path.
fn clear_superseded_links(pending: &mut BTreeMap<PathBuf, (usize, PendingLink)>, rel: &Path) {
    let mut cur = rel;
    loop {
        pending.remove(cur);
        match cur.parent() {
            Some(p) if !p.as_os_str().is_empty() => cur = p,
            _ => break,
        }
    }
}

fn read_zip<R: Read + Seek>(
    r: R,
    stage: &Path,
    mode: ExtractMode,
    cancel: &AtomicBool,
    archive: &Path,
) -> Result<bool> {
    let mut z = check(zip::ZipArchive::new(r), cancel, "zip")?;
    // Write files and create real directories before installing archive links.
    let mut links = Vec::new();
    // Entries of a folder come one after another: create it once, not once per file.
    let mut made_dir = PathBuf::new();
    for i in 0..z.len() {
        // Encrypted entries use the password typed while browsing the archive.
        let mut f = super::archive_view::zip_entry(&mut z, i, archive)?;
        let Some(rel) = f.enclosed_name().filter(|p| !p.as_os_str().is_empty()) else {
            continue;
        };
        let rel = local_name(rel);
        let out = stage.join(&rel);
        if f.is_dir() {
            fs::create_dir_all(&out).ctx("create", &out)?;
            continue;
        }
        if f.is_symlink() {
            links.push((i, rel));
            continue;
        }
        if let Some(p) = out.parent().filter(|p| *p != made_dir) {
            fs::create_dir_all(p).ctx("create", p)?;
            made_dir = p.to_path_buf();
        }
        let mut w = fs::File::create(&out).ctx("create", &out)?;
        check(io::copy(&mut f, &mut w), cancel, "zip")?;
        #[cfg(unix)]
        if let Some(mode) = f.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = w.set_permissions(fs::Permissions::from_mode(mode & 0o777));
        }
    }
    // Bound the bytes read for each link independently of the archive's size hint.
    const MAX_LINK_BYTES: u64 = 16 * 1024;
    let mut symlinks = Vec::new();
    for (i, rel) in links {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let f = super::archive_view::zip_entry(&mut z, i, archive)?;
        if f.size() > MAX_LINK_BYTES {
            continue;
        }
        let mut target = String::new();
        check(
            f.take(MAX_LINK_BYTES + 1).read_to_string(&mut target),
            cancel,
            "zip",
        )?;
        if target.len() as u64 <= MAX_LINK_BYTES {
            symlinks.push((rel, PathBuf::from(target)));
        }
    }
    install_archive_links(stage, mode, &[], &symlinks, cancel)
}

fn read_7z<R: Read + Seek>(r: R, stage: &Path, cancel: &AtomicBool, archive: &Path) -> Result<()> {
    let mut ar = sevenz_rust2::ArchiveReader::new(r, super::archive_view::sevenz_password(archive))
        .map_err(|e| super::archive_view::sevenz_err(archive, e))?;
    let mut err: Option<CoreError> = None;
    let res = ar.for_each_entries(|entry, data| {
        let Some(rel) = safe_relative(entry.name()) else {
            return Ok(true);
        };
        let out = stage.join(local_name(rel));
        let step = (|| -> Result<()> {
            if entry.is_directory() {
                fs::create_dir_all(&out).ctx("create", &out)?;
                return Ok(());
            }
            if let Some(p) = out.parent() {
                fs::create_dir_all(p).ctx("create", p)?;
            }
            let mut w = fs::File::create(&out).ctx("create", &out)?;
            check(io::copy(data, &mut w), cancel, "7z")?;
            Ok(())
        })();
        match step {
            Ok(()) => Ok(true),
            Err(e) => {
                err = Some(e);
                Ok(false)
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    check(res, cancel, "7z")
}

/// Checks whether every existing ancestor of `rel_dir` under `root` is a real directory
/// without creating anything yet.
fn ancestors_are_real_dirs(root: &Path, rel_dir: &Path) -> Result<bool> {
    let mut cur = root.to_path_buf();
    for part in rel_dir.components() {
        let Component::Normal(name) = part else {
            return Ok(false);
        };
        cur.push(name);
        match fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(true),
            Err(e) => return Err(CoreError::io("stat", &cur, e)),
        }
    }
    Ok(true)
}

/// Ensures every component of `rel_dir` under `root` is a real directory (never traversing a symlink).
fn ensure_real_dir(root: &Path, rel_dir: &Path) -> Result<bool> {
    let mut cur = root.to_path_buf();
    for part in rel_dir.components() {
        let Component::Normal(name) = part else {
            return Ok(false);
        };
        cur.push(name);
        match fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&cur).ctx("create", &cur)?;
            }
            Err(e) => return Err(CoreError::io("stat", &cur, e)),
        }
    }
    Ok(true)
}

fn first_component(rel: &Path) -> Option<OsString> {
    rel.components().find_map(|c| match c {
        Component::Normal(name) => Some(name.to_os_string()),
        _ => None,
    })
}

/// Installs hardlinks first, then symlinks against the exact published extraction boundary.
///
/// Returns whether `ExtractMode::Smart` may safely unwrap a lone top-level item. Whenever
/// symlinks are validated against `stage` (instead of an unwrapped top-level directory),
/// Smart unwrapping is disabled so no validated symlink can ever have `stage` removed out
/// from under it.
fn install_archive_links(
    stage: &Path,
    mode: ExtractMode,
    hardlinks: &[(PathBuf, PathBuf)],
    symlinks: &[(PathBuf, PathBuf)],
    cancel: &AtomicBool,
) -> Result<bool> {
    // Pass 1: hardlinks (no symlinks exist in `stage` yet, so target paths cannot traverse symlinks).
    for (rel, target_raw) in hardlinks {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let Some(target_rel) = safe_relative(&target_raw.to_string_lossy()) else {
            continue;
        };
        let target = stage.join(local_name(target_rel));
        let Ok(target_meta) = fs::symlink_metadata(&target) else {
            continue;
        };
        if !target_meta.file_type().is_file() {
            continue;
        }
        let link = stage.join(rel);
        if fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            if !ancestors_are_real_dirs(stage, parent)? || !ensure_real_dir(stage, parent)? {
                continue;
            }
        }
        let _ = fs::hard_link(&target, &link);
    }

    if symlinks.is_empty() {
        return Ok(true);
    }

    // Pass 2: pre-filter symlinks that stay within `stage` and do not collide with existing
    // entries or non-directory ancestors, without creating any directories yet.
    let mut candidates = Vec::new();
    for (rel, target) in symlinks {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let link = stage.join(rel);
        if fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            if !ancestors_are_real_dirs(stage, parent)? {
                continue;
            }
        }
        if !safe_symlink_target(target, &link, stage) {
            continue;
        }
        candidates.push((rel, target));
    }

    if candidates.is_empty() {
        return Ok(true);
    }

    // Compute the exact published boundary from existing top-level entries plus all valid
    // candidate symlinks. If Smart mode will unwrap a single top-level real directory (either
    // already present in `stage` or created exclusively by nested symlinks), validate every
    // symlink against that directory; otherwise validate against `stage` and keep `stage`
    // wrapped so no symlink validated against `stage` can escape after unwrapping.
    let mut top_names: BTreeSet<OsString> = BTreeSet::new();
    let mut existing_top_is_dir: HashMap<OsString, bool> = HashMap::new();
    for entry in fs::read_dir(stage).ctx("read", stage)?.flatten() {
        let name = entry.file_name();
        let path = entry.path();
        let is_dir = fs::symlink_metadata(&path)
            .ctx("stat", &path)?
            .file_type()
            .is_dir();
        existing_top_is_dir.insert(name.clone(), is_dir);
        top_names.insert(name);
    }
    let mut top_level_symlinks: BTreeSet<OsString> = BTreeSet::new();
    for (rel, _) in &candidates {
        if let Some(first) = first_component(rel) {
            top_names.insert(first.clone());
            if rel.parent().is_none_or(|p| p.as_os_str().is_empty()) {
                top_level_symlinks.insert(first);
            }
        }
    }

    let (boundary, allow_smart_unwrap) = if mode == ExtractMode::Smart && top_names.len() == 1 {
        let only = top_names.iter().next().unwrap();
        let is_top_symlink = top_level_symlinks.contains(only);
        let is_existing_non_dir = existing_top_is_dir.get(only).is_some_and(|&d| !d);
        if !is_top_symlink && !is_existing_non_dir {
            (stage.join(only), true)
        } else {
            (stage.to_path_buf(), false)
        }
    } else {
        (stage.to_path_buf(), false)
    };

    // Pass 3: validate every symlink against `boundary` BEFORE creating parent directories.
    let mut validated_nested = Vec::new();
    for (rel, target) in candidates {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let link = stage.join(rel);
        if !safe_symlink_target(target, &link, &boundary) {
            continue;
        }
        if rel.parent().is_none_or(|p| p.as_os_str().is_empty()) {
            if fs::symlink_metadata(&link).is_err() {
                make_symlink(target, &link, &boundary);
            }
        } else {
            validated_nested.push((rel, target));
        }
    }

    for (rel, target) in validated_nested {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let link = stage.join(rel);
        if fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            if !ensure_real_dir(stage, parent)? {
                continue;
            }
        }
        make_symlink(target, &link, &boundary);
    }

    Ok(allow_smart_unwrap)
}

/// Checks whether a symlink at `link` pointing to `target` stays contained inside `boundary`.
///
/// Only relative targets consisting of optional leading `..` (never exceeding `link`'s depth
/// below `boundary`) followed by normal components are allowed. Forbidding `..` after any
/// normal component ensures chains of individually valid symlinks can never escape `boundary`.
fn safe_symlink_target(target: &Path, link: &Path, boundary: &Path) -> bool {
    if target.as_os_str().is_empty() {
        return false;
    }
    let Some(link_parent) = link.parent() else {
        return false;
    };
    let Ok(rel_parent) = link_parent.strip_prefix(boundary) else {
        return false;
    };
    let parent_depth = rel_parent
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count();

    let mut parent_ups = 0usize;
    let mut seen_normal = false;
    let mut has_component = false;
    for part in target.components() {
        match part {
            Component::CurDir => {
                has_component = true;
            }
            Component::ParentDir => {
                if seen_normal {
                    return false;
                }
                parent_ups += 1;
                if parent_ups > parent_depth {
                    return false;
                }
                has_component = true;
            }
            Component::Normal(name) => {
                if cfg!(windows) && !name.to_str().is_some_and(|n| windows_component(n) == n) {
                    return false;
                }
                seen_normal = true;
                has_component = true;
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    has_component
}

/// Recreates a symlink from an archive entry if it stays inside `boundary`.
fn make_symlink(target: &Path, link: &Path, boundary: &Path) {
    if !safe_symlink_target(target, link, boundary) {
        return;
    }
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(target, link);
    // Windows wants `\` in the target and a directory link for a folder. Creating links
    // needs Developer Mode or admin rights; without them the link is left out.
    #[cfg(windows)]
    {
        let target = target.to_string_lossy().replace('/', "\\");
        let is_dir = link
            .parent()
            .is_some_and(|parent| parent.join(&target).is_dir());
        let _ = if is_dir {
            std::os::windows::fs::symlink_dir(&target, link)
        } else {
            std::os::windows::fs::symlink_file(&target, link)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tree(root: &Path) -> PathBuf {
        let src = root.join("photos");
        fs::create_dir_all(src.join("iceland")).unwrap();
        fs::write(src.join("a.txt"), "alpha\n").unwrap();
        fs::write(src.join("iceland/b.txt"), "beta ".repeat(5000)).unwrap();
        fs::create_dir_all(src.join("empty")).unwrap();
        src
    }

    fn same_tree(a: &Path, b: &Path) {
        assert_eq!(
            fs::read(a.join("a.txt")).unwrap(),
            fs::read(b.join("a.txt")).unwrap()
        );
        assert_eq!(
            fs::read(a.join("iceland/b.txt")).unwrap(),
            fs::read(b.join("iceland/b.txt")).unwrap()
        );
        assert!(b.join("empty").is_dir());
    }

    #[test]
    fn detects_formats_and_stems() {
        assert_eq!(
            detect(Path::new("x/Photos.TAR.GZ")),
            Some((ArchiveFormat::TarGz, "Photos".into()))
        );
        assert_eq!(
            detect(Path::new("a.b.zip")),
            Some((ArchiveFormat::Zip, "a.b".into()))
        );
        assert_eq!(
            detect(Path::new("log.gz")),
            Some((ArchiveFormat::Gz, "log".into()))
        );
        assert_eq!(
            detect(Path::new("x.tzst")),
            Some((ArchiveFormat::TarZst, "x".into()))
        );
        assert_eq!(detect(Path::new(".zip")), None);
        assert_eq!(detect(Path::new("notes.txt")), None);
    }

    #[test]
    fn names_windows_cannot_store() {
        assert_eq!(windows_component("a:b?.txt"), "a_b_.txt");
        assert_eq!(windows_component("notes. "), "notes");
        assert_eq!(windows_component("aux.txt"), "_aux.txt");
        assert_eq!(windows_component("COM1"), "_COM1");
        assert_eq!(windows_component("com¹.txt"), "_com¹.txt");
        assert_eq!(windows_component("CONOUT$"), "_CONOUT$");
        assert_eq!(windows_component("com10.txt"), "com10.txt");
        assert_eq!(windows_component("console.log"), "console.log");
        assert_eq!(windows_component("..."), "_");
    }

    #[test]
    fn round_trip_every_writable_format() {
        for fmt in [
            ArchiveFormat::Zip,
            ArchiveFormat::TarGz,
            ArchiveFormat::SevenZ,
        ] {
            let t = tempfile::tempdir().unwrap();
            let src = tree(t.path());
            let out = t.path().join("out");
            fs::create_dir(&out).unwrap();
            let mut bytes = 0;
            let ar = compress(
                std::slice::from_ref(&src),
                &out.join(format!("photos.{}", fmt.extension())),
                fmt,
                &AtomicBool::new(false),
                &mut |n| bytes += n,
            )
            .unwrap();
            assert!(ar.is_file(), "{fmt:?}");
            assert_eq!(
                bytes,
                source_bytes(std::slice::from_ref(&src)),
                "{fmt:?} progress counts source bytes"
            );
            // One top-level folder: smart extraction puts it in place without a wrapper.
            let placed = extract(
                &ar,
                &out,
                ExtractMode::Smart,
                &AtomicBool::new(false),
                &mut |_| {},
            )
            .unwrap();
            assert_eq!(placed, vec![out.join("photos")], "{fmt:?}");
            same_tree(&src, &placed[0]);
            // Folder mode wraps it, with a unique name.
            let placed = extract(
                &ar,
                &out,
                ExtractMode::Folder,
                &AtomicBool::new(false),
                &mut |_| {},
            )
            .unwrap();
            assert_eq!(placed, vec![out.join("photos (2)")], "{fmt:?}");
            same_tree(&src, &placed[0].join("photos"));
            assert!(
                fs::read_dir(&out)
                    .unwrap()
                    .flatten()
                    .all(|e| !e.file_name().to_string_lossy().starts_with(".gall-")),
                "no staging left"
            );
        }
    }

    /// Every path below `dir` (relative, sorted) with the bytes of each file.
    fn snapshot(dir: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        fn walk(root: &Path, at: &Path, out: &mut Vec<(String, Option<Vec<u8>>)>) {
            for e in fs::read_dir(at).unwrap().flatten() {
                let rel = e
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if e.path().is_dir() {
                    out.push((rel, None));
                    walk(root, &e.path(), out);
                } else {
                    out.push((rel, Some(fs::read(e.path()).unwrap())));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    /// The writers are buffered and extraction creates a folder once for its entries: a payload that
    /// is bigger than the write buffer, an empty file, and many entries in a few folders must come
    /// back byte for byte, with the empty folders.
    #[test]
    fn buffered_writers_keep_every_byte() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut noise = |n: usize| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    (state >> 24) as u8
                })
                .collect()
        };
        for fmt in [ArchiveFormat::Zip, ArchiveFormat::TarGz] {
            let t = tempfile::tempdir().unwrap();
            let src = t.path().join("data");
            for d in ["a", "a/deep/er", "b", "empty/nested"] {
                fs::create_dir_all(src.join(d)).unwrap();
            }
            // Incompressible: the archive itself is far larger than WRITE_BUFFER.
            fs::write(src.join("a/big.bin"), noise(WRITE_BUFFER * 2 + 12_345)).unwrap();
            fs::write(src.join("b/exact.bin"), noise(WRITE_BUFFER)).unwrap();
            fs::write(src.join("zero.txt"), b"").unwrap();
            for i in 0..300 {
                fs::write(
                    src.join(format!("a/deep/er/f{i:03}.txt")),
                    format!("file {i}\n").repeat(i % 7 + 1),
                )
                .unwrap();
                fs::write(src.join(format!("b/g{i:03}.dat")), noise(i * 13 % 900)).unwrap();
            }
            let out = t.path().join("out");
            fs::create_dir(&out).unwrap();
            let ar = compress(
                std::slice::from_ref(&src),
                &out.join(format!("data.{}", fmt.extension())),
                fmt,
                &AtomicBool::new(false),
                &mut |_| {},
            )
            .unwrap();
            assert!(
                fs::metadata(&ar).unwrap().len() > WRITE_BUFFER as u64 * 2,
                "{fmt:?}"
            );
            let placed = extract(
                &ar,
                &out,
                ExtractMode::Smart,
                &AtomicBool::new(false),
                &mut |_| {},
            )
            .unwrap();
            assert_eq!(placed, vec![out.join("data")], "{fmt:?}");
            let (want, got) = (snapshot(&src), snapshot(&placed[0]));
            assert_eq!(want.len(), got.len(), "{fmt:?}: same paths");
            for (w, g) in want.iter().zip(&got) {
                assert_eq!(w.0, g.0, "{fmt:?}");
                assert!(w.1 == g.1, "{fmt:?}: {} differs", w.0);
            }
        }
    }

    #[test]
    fn several_top_level_items_get_a_folder() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("one.txt"), "1").unwrap();
        fs::write(t.path().join("two.txt"), "2").unwrap();
        let srcs = [t.path().join("one.txt"), t.path().join("two.txt")];
        let ar = compress(
            &srcs,
            &t.path().join("pair.zip"),
            ArchiveFormat::Zip,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let placed = extract(
            &ar,
            &out,
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(placed, vec![out.join("pair")]);
        assert_eq!(fs::read_to_string(out.join("pair/two.txt")).unwrap(), "2");
    }

    #[test]
    fn existing_archive_name_is_kept() {
        let t = tempfile::tempdir().unwrap();
        let src = tree(t.path());
        fs::write(t.path().join("photos.zip"), "old").unwrap();
        let ar = compress(
            &[src],
            &t.path().join("photos.zip"),
            ArchiveFormat::Zip,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(ar, t.path().join("photos (2).zip"));
        assert_eq!(
            fs::read_to_string(t.path().join("photos.zip")).unwrap(),
            "old"
        );
    }

    #[test]
    fn single_file_and_tar_variants_extract() {
        let t = tempfile::tempdir().unwrap();
        // .gz of one file
        let mut gz = flate2::write::GzEncoder::new(
            fs::File::create(t.path().join("log.txt.gz")).unwrap(),
            flate2::Compression::default(),
        );
        gz.write_all(b"hello log").unwrap();
        gz.finish().unwrap();
        let placed = extract(
            &t.path().join("log.txt.gz"),
            t.path(),
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&placed[0]).unwrap(), "hello log");
        assert_eq!(placed[0], t.path().join("log.txt"));
        // .tar.xz
        let src = tree(t.path());
        let xz = lzma_rust2::XzWriter::new(
            fs::File::create(t.path().join("p.tar.xz")).unwrap(),
            lzma_rust2::XzOptions::default(),
        )
        .unwrap();
        let mut b = tar::Builder::new(xz);
        b.append_dir_all("photos", &src).unwrap();
        b.into_inner().unwrap().finish().unwrap();
        let out = t.path().join("xz");
        fs::create_dir(&out).unwrap();
        let placed = extract(
            &t.path().join("p.tar.xz"),
            &out,
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        same_tree(&src, &placed[0]);
    }

    #[test]
    fn zip_entries_cannot_escape() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("evil.zip");
        let mut w = zip::ZipWriter::new(fs::File::create(&p).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        w.start_file("../escaped.txt", o).unwrap();
        w.write_all(b"x").unwrap();
        w.start_file("ok.txt", o).unwrap();
        w.write_all(b"y").unwrap();
        w.finish().unwrap();
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let placed = extract(
            &p,
            &out,
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert!(!t.path().join("escaped.txt").exists());
        assert_eq!(placed, vec![out.join("ok.txt")]);
    }

    #[cfg(unix)]
    #[test]
    fn zip_keeps_relative_links_and_executable_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let t = tempfile::tempdir().unwrap();
        let archive = t.path().join("tools.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o750);
        zip.add_symlink("shortcut", "./tools/", opts).unwrap();
        zip.start_file("tools/tool", opts).unwrap();
        zip.write_all(b"example tool").unwrap();
        zip.finish().unwrap();

        let placed = extract(
            &archive,
            t.path(),
            ExtractMode::Folder,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        let shortcut = placed[0].join("shortcut");
        assert_eq!(fs::read_link(&shortcut).unwrap().as_os_str(), "./tools/");
        let tool = shortcut.join("tool");
        assert_eq!(fs::read(&tool).unwrap(), b"example tool");
        assert_eq!(
            fs::metadata(tool).unwrap().permissions().mode() & 0o7777,
            0o750
        );
    }

    #[cfg(unix)]
    #[test]
    fn smart_extraction_keeps_a_lone_link_inside_its_archive_folder() {
        let t = tempfile::tempdir().unwrap();
        let archive = t.path().join("shortcuts.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.add_symlink(
            "shortcut",
            "notes.txt",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();

        let placed = extract(
            &archive,
            t.path(),
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(placed, vec![t.path().join("shortcuts")]);
        assert_eq!(
            fs::read_link(placed[0].join("shortcut")).unwrap(),
            Path::new("notes.txt")
        );
    }

    #[test]
    fn tar_preserves_safe_relative_links_and_hardlinks() {
        let t = tempfile::tempdir().unwrap();
        let archive = t.path().join("safe-links.tar");
        {
            let mut b = tar::Builder::new(fs::File::create(&archive).unwrap());
            let mut add_file = |name: &str, data: &[u8], mode: u32| {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(mode);
                b.append_data(&mut h, name, data).unwrap();
            };
            add_file("pkg/other.txt", b"world", 0o644);
            add_file("pkg/sub/file.txt", b"hello", 0o750);

            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            h.set_mode(0o777);
            b.append_link(&mut h, "pkg/sub/same_dir", "file.txt")
                .unwrap();

            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            h.set_mode(0o777);
            b.append_link(&mut h, "pkg/sub/up_link", "../other.txt")
                .unwrap();

            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Link);
            h.set_size(0);
            h.set_mode(0o644);
            b.append_link(&mut h, "pkg/hard.txt", "pkg/other.txt")
                .unwrap();
            b.finish().unwrap();
        }

        for (label, mode) in [
            ("smart", ExtractMode::Smart),
            ("folder", ExtractMode::Folder),
        ] {
            let out = t.path().join(label);
            fs::create_dir(&out).unwrap();
            let placed =
                extract(&archive, &out, mode, &AtomicBool::new(false), &mut |_| {}).unwrap();
            let pkg = if mode == ExtractMode::Smart {
                placed[0].clone()
            } else {
                placed[0].join("pkg")
            };
            assert_eq!(fs::read(pkg.join("hard.txt")).unwrap(), b"world", "{label}");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::read_link(pkg.join("sub/same_dir")).unwrap(),
                    Path::new("file.txt"),
                    "{label}"
                );
                assert_eq!(
                    fs::read_link(pkg.join("sub/up_link")).unwrap(),
                    Path::new("../other.txt"),
                    "{label}"
                );
                assert_eq!(
                    fs::read(pkg.join("sub/up_link")).unwrap(),
                    b"world",
                    "{label}"
                );
                assert_eq!(
                    fs::metadata(pkg.join("sub/file.txt"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o7777,
                    0o750,
                    "{label}"
                );
            }
        }
    }

    #[test]
    fn tar_rejects_escaping_links_symlink_chains_and_special_mode_bits() {
        let t = tempfile::tempdir().unwrap();
        let outside_file = t.path().join("outside.txt");
        let outside_dir = t.path().join("outside-dir");
        fs::write(&outside_file, "secret").unwrap();
        fs::create_dir(&outside_dir).unwrap();

        let archive = t.path().join("evil-links.tar");
        {
            let mut b = tar::Builder::new(fs::File::create(&archive).unwrap());
            let mut h = tar::Header::new_gnu();
            h.set_size(4);
            h.set_mode(0o4755);
            b.append_data(&mut h, "pkg/ok.txt", &b"safe"[..]).unwrap();

            // Symlink created before a file inside that directory path.
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, "pkg/via", &outside_dir).unwrap();

            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o644);
            b.append_data(&mut h, "pkg/via/pwn.txt", &b"pwn"[..])
                .unwrap();

            for (name, target) in [
                ("pkg/abs_sym", outside_file.to_string_lossy().into_owned()),
                ("pkg/rel_escape", "../../outside.txt".into()),
                ("pkg/smart_escape", "../outside.txt".into()),
                ("pkg/sub/chain_escape", "../chain_dir/..".into()),
            ] {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_size(0);
                b.append_link(&mut h, name, target).unwrap();
            }

            // A symlink pointing to `..` inside `pkg/sub`, paired with `chain_escape` above.
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, "pkg/sub/chain_dir", "..").unwrap();

            for (name, target) in [
                ("pkg/escape_hard", "../../outside.txt"),
                ("pkg/abs_hard", "/etc/passwd"),
            ] {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Link);
                h.set_size(0);
                b.append_link(&mut h, name, target).unwrap();
            }
            b.finish().unwrap();
        }

        let out = t.path().join("smart-out");
        fs::create_dir(&out).unwrap();
        let placed = extract(
            &archive,
            &out,
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(placed, vec![out.join("pkg")]);
        assert!(!outside_dir.join("pwn.txt").exists());
        assert_eq!(fs::read(placed[0].join("via/pwn.txt")).unwrap(), b"pwn");
        for rejected in [
            "abs_sym",
            "rel_escape",
            "smart_escape",
            "sub/chain_escape",
            "escape_hard",
            "abs_hard",
        ] {
            assert!(
                fs::symlink_metadata(placed[0].join(rejected)).is_err(),
                "{rejected} must not be published"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(placed[0].join("ok.txt"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o755,
                "setuid bit must be stripped"
            );
        }
        assert_eq!(fs::read_to_string(&outside_file).unwrap(), "secret");
    }

    #[cfg(unix)]
    #[test]
    fn zip_smart_extraction_keeps_nested_symlinks_inside_unwrapped_folder() {
        let t = tempfile::tempdir().unwrap();
        let archive = t.path().join("nested.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("pkg/ok.txt", opts).unwrap();
        zip.write_all(b"ok").unwrap();
        zip.add_symlink("pkg/sub/safe", "../ok.txt", opts).unwrap();
        zip.add_symlink("pkg/escape", "../outside.txt", opts)
            .unwrap();
        zip.finish().unwrap();

        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let placed = extract(
            &archive,
            &out,
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(placed, vec![out.join("pkg")]);
        assert_eq!(
            fs::read_link(placed[0].join("sub/safe")).unwrap(),
            Path::new("../ok.txt")
        );
        assert!(fs::symlink_metadata(placed[0].join("escape")).is_err());
    }

    #[test]
    fn compress_and_extract_preserve_concurrent_destinations() {
        let t = tempfile::tempdir().unwrap();
        let src = tree(t.path());
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();

        let mut raced = false;
        let archive = compress_with_publisher(
            std::slice::from_ref(&src),
            &out.join("photos.zip"),
            ArchiveFormat::Zip,
            &AtomicBool::new(false),
            &mut |_| {},
            |from, dir, name| {
                super::super::ops::publish_unique_with(from, dir, name, |a, b| {
                    if !raced {
                        raced = true;
                        fs::write(b, "concurrent archive").unwrap();
                    }
                    super::super::ops::rename_noreplace(a, b)
                })
            },
        )
        .unwrap();
        assert_eq!(archive, out.join("photos (2).zip"));
        assert_eq!(
            fs::read_to_string(out.join("photos.zip")).unwrap(),
            "concurrent archive"
        );

        let mut raced = false;
        let placed = extract_with_publisher(
            &archive,
            &out,
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
            |from, dir, name| {
                super::super::ops::publish_unique_with(from, dir, name, |a, b| {
                    if !raced {
                        raced = true;
                        fs::write(b, "concurrent target").unwrap();
                    }
                    super::super::ops::rename_noreplace(a, b)
                })
            },
        )
        .unwrap();
        assert_eq!(placed, vec![out.join("photos (2)")]);
        assert_eq!(
            fs::read_to_string(out.join("photos")).unwrap(),
            "concurrent target"
        );
        same_tree(&src, &placed[0]);
    }

    #[test]
    fn tar_nested_symlink_only_archive_cannot_escape_smart_boundary() {
        let t = tempfile::tempdir().unwrap();
        let archive = t.path().join("only-link.tar");
        {
            let mut b = tar::Builder::new(fs::File::create(&archive).unwrap());
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, "pkg/escape", "../outside.txt")
                .unwrap();
            b.finish().unwrap();
        }
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let placed = extract(
            &archive,
            &out,
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(placed, vec![out.join("only-link")]);
        assert!(!out.join("pkg").exists());
        assert!(fs::symlink_metadata(placed[0].join("pkg/escape")).is_err());
    }

    #[test]
    fn tar_keeps_cross_folder_symlinks_and_skips_empty_dirs_for_rejected_links() {
        let t = tempfile::tempdir().unwrap();
        // (a) Valid cross-folder relative symlink without an explicit `b/` directory entry.
        let cross = t.path().join("cross.tar");
        {
            let mut b = tar::Builder::new(fs::File::create(&cross).unwrap());
            let mut h = tar::Header::new_gnu();
            h.set_size(2);
            h.set_mode(0o644);
            b.append_data(&mut h, "a/ok.txt", &b"ok"[..]).unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, "b/link", "../a/ok.txt").unwrap();
            b.finish().unwrap();
        }
        let out = t.path().join("cross-out");
        fs::create_dir(&out).unwrap();
        let placed = extract(
            &cross,
            &out,
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(placed, vec![out.join("cross")]);
        #[cfg(unix)]
        {
            assert_eq!(
                fs::read_link(placed[0].join("b/link")).unwrap(),
                Path::new("../a/ok.txt")
            );
            assert_eq!(fs::read(placed[0].join("b/link")).unwrap(), b"ok");
        }

        // (b) Rejected escaping symlink in another folder must not create stray empty dirs
        //     or block Smart unwrapping of `pkg/`.
        let stray = t.path().join("stray.tar");
        {
            let mut b = tar::Builder::new(fs::File::create(&stray).unwrap());
            let mut h = tar::Header::new_gnu();
            h.set_size(2);
            h.set_mode(0o644);
            b.append_data(&mut h, "pkg/ok.txt", &b"ok"[..]).unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, "evil/bad", "/etc/passwd").unwrap();
            b.finish().unwrap();
        }
        let out2 = t.path().join("stray-out");
        fs::create_dir(&out2).unwrap();
        let placed2 = extract(
            &stray,
            &out2,
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(placed2, vec![out2.join("pkg")]);
        assert!(!out2.join("evil").exists());
    }

    #[test]
    fn tar_handles_v7_trailing_slash_dirs_and_hardlink_chains() {
        let t = tempfile::tempdir().unwrap();
        let archive = t.path().join("v7-and-chain.tar");
        {
            let mut b = tar::Builder::new(fs::File::create(&archive).unwrap());
            // V7-style directory: Regular entry whose path ends with `/`.
            let mut h = tar::Header::new_old();
            h.set_entry_type(tar::EntryType::Regular);
            h.set_size(0);
            h.set_mode(0o755);
            b.append_data(&mut h, "pkg/", io::empty()).unwrap();

            let mut h = tar::Header::new_gnu();
            h.set_size(5);
            h.set_mode(0o644);
            b.append_data(&mut h, "pkg/z.txt", &b"chain"[..]).unwrap();

            // Hardlink chain where `pkg/a.txt` sorts before `pkg/m.txt`.
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Link);
            h.set_size(0);
            b.append_link(&mut h, "pkg/m.txt", "pkg/z.txt").unwrap();

            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Link);
            h.set_size(0);
            b.append_link(&mut h, "pkg/a.txt", "pkg/m.txt").unwrap();
            b.finish().unwrap();
        }
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let placed = extract(
            &archive,
            &out,
            ExtractMode::Smart,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(placed, vec![out.join("pkg")]);
        assert_eq!(fs::read(placed[0].join("a.txt")).unwrap(), b"chain");
        assert_eq!(fs::read(placed[0].join("m.txt")).unwrap(), b"chain");
        assert_eq!(fs::read(placed[0].join("z.txt")).unwrap(), b"chain");
    }

    #[test]
    fn dot_and_dotdot_archive_stems_stay_contained() {
        assert_eq!(safe_stem(".."), "archive");
        assert_eq!(safe_stem("."), "archive");
        assert_eq!(safe_stem("photos"), "photos");
    }

    #[test]
    fn cancel_leaves_nothing_behind() {
        let t = tempfile::tempdir().unwrap();
        let src = tree(t.path());
        let cancel = AtomicBool::new(true);
        let r = compress(
            &[src],
            &t.path().join("c.zip"),
            ArchiveFormat::Zip,
            &cancel,
            &mut |_| {},
        );
        assert!(matches!(r, Err(CoreError::Cancelled)));
        let names: Vec<_> = fs::read_dir(t.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("photos")]);
    }
}
