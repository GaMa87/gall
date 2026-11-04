//! Archives: create (zip, tar.gz, 7z) and extract (zip, tar with gz/xz/bz2/zst,
//! 7z, and single-file gz/xz/bz2/zst). Everything is pure Rust, so it behaves the
//! same on Linux, Windows and macOS.
//!
//! Both directions write to a hidden staging name first and move the result into
//! place at the end, so a cancelled or failed task leaves nothing half-written.
//! Entry names that would escape the destination (`..`, absolute paths) are skipped.

use crate::error::{CoreError, IoContext, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::ops::unique_path;

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
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
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

fn staging_name(dir: &Path, hint: &str) -> PathBuf {
    let pid = std::process::id();
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    dir.join(format!(".gall-{hint}-{pid}-{n:x}.part"))
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

/// Packs `sources` into a new archive at `target` (a unique name is chosen if it
/// exists). Returns the path written.
pub fn compress(
    sources: &[PathBuf],
    target: &Path,
    format: ArchiveFormat,
    cancel: &AtomicBool,
    tick: &mut Tick,
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
    let tmp = staging_name(dir, "pack");
    let guard = Staging(Some(tmp.clone()));
    match format {
        ArchiveFormat::Zip => write_zip(&tmp, &entries, cancel, tick)?,
        ArchiveFormat::TarGz => write_tar_gz(&tmp, &entries, cancel, tick)?,
        ArchiveFormat::SevenZ => write_7z(&tmp, &entries, cancel, tick)?,
        _ => unreachable!(),
    }
    let dest = unique_path(dir, &name);
    fs::rename(&tmp, &dest).ctx("rename", &dest)?;
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
    tmp: &Path,
    entries: &[(PathBuf, String, fs::Metadata)],
    cancel: &AtomicBool,
    tick: &mut Tick,
) -> Result<()> {
    use zip::write::SimpleFileOptions;
    let mut w = zip::ZipWriter::new(fs::File::create(tmp).ctx("create", tmp)?);
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
    check(w.finish(), cancel, "zip")?;
    Ok(())
}

fn write_tar_gz(
    tmp: &Path,
    entries: &[(PathBuf, String, fs::Metadata)],
    cancel: &AtomicBool,
    tick: &mut Tick,
) -> Result<()> {
    let gz = flate2::write::GzEncoder::new(
        fs::File::create(tmp).ctx("create", tmp)?,
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
    check(gz.finish(), cancel, "gzip")?;
    Ok(())
}

fn write_7z(
    tmp: &Path,
    entries: &[(PathBuf, String, fs::Metadata)],
    cancel: &AtomicBool,
    tick: &mut Tick,
) -> Result<()> {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    let mut w = check(ArchiveWriter::create(tmp), cancel, "7z")?;
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
    let (format, stem) = detect(archive).ok_or_else(|| {
        CoreError::Other(format!("{} is not a supported archive", archive.display()))
    })?;
    if !dest_dir.is_dir() {
        return Err(CoreError::NotFound(dest_dir.into()));
    }
    let stage = staging_name(dest_dir, "extract");
    fs::create_dir(&stage).ctx("create", &stage)?;
    let guard = Staging(Some(stage.clone()));
    {
        let r = open_counting(archive, cancel, tick)?;
        match format {
            ArchiveFormat::Zip => read_zip(r, &stage, cancel, archive)?,
            ArchiveFormat::SevenZ => read_7z(r, &stage, cancel, archive)?,
            ArchiveFormat::Tar => read_tar(r, &stage, cancel)?,
            ArchiveFormat::TarGz => read_tar(
                flate2::read::MultiGzDecoder::new(io::BufReader::new(r)),
                &stage,
                cancel,
            )?,
            ArchiveFormat::TarXz => read_tar(
                lzma_rust2::XzReader::new(io::BufReader::new(r), true),
                &stage,
                cancel,
            )?,
            ArchiveFormat::TarBz2 => read_tar(
                bzip2_rs::DecoderReader::new(io::BufReader::new(r)),
                &stage,
                cancel,
            )?,
            ArchiveFormat::TarZst => read_tar(zstd_reader(r, cancel)?, &stage, cancel)?,
            ArchiveFormat::Gz => read_single(
                flate2::read::MultiGzDecoder::new(io::BufReader::new(r)),
                &stage.join(&stem),
                cancel,
            )?,
            ArchiveFormat::Xz => read_single(
                lzma_rust2::XzReader::new(io::BufReader::new(r), true),
                &stage.join(&stem),
                cancel,
            )?,
            ArchiveFormat::Bz2 => read_single(
                bzip2_rs::DecoderReader::new(io::BufReader::new(r)),
                &stage.join(&stem),
                cancel,
            )?,
            ArchiveFormat::Zst => read_single(zstd_reader(r, cancel)?, &stage.join(&stem), cancel)?,
        }
    }
    let top: Vec<PathBuf> = fs::read_dir(&stage)
        .ctx("read", &stage)?
        .flatten()
        .map(|e| e.path())
        .collect();
    let placed = if mode == ExtractMode::Smart && top.len() == 1 {
        let item = &top[0];
        let name = item.file_name().unwrap().to_string_lossy().into_owned();
        let dest = unique_path(dest_dir, &name);
        fs::rename(item, &dest).ctx("move", &dest)?;
        dest
    } else {
        let dest = unique_path(dest_dir, &stem);
        fs::rename(&stage, &dest).ctx("move", &dest)?;
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

fn read_tar<R: Read>(r: R, stage: &Path, cancel: &AtomicBool) -> Result<()> {
    let mut ar = tar::Archive::new(r);
    ar.set_preserve_mtime(true);
    ar.set_unpack_xattrs(false);
    for e in check(ar.entries(), cancel, "tar")? {
        let mut e = check(e, cancel, "tar")?;
        // unpack_in refuses entries that would land outside `stage`.
        check(e.unpack_in(stage), cancel, "tar")?;
    }
    Ok(())
}

fn read_zip<R: Read + Seek>(r: R, stage: &Path, cancel: &AtomicBool, archive: &Path) -> Result<()> {
    let mut z = check(zip::ZipArchive::new(r), cancel, "zip")?;
    for i in 0..z.len() {
        // Encrypted entries use the password typed while browsing the archive.
        let mut f = super::archive_view::zip_entry(&mut z, i, archive)?;
        let Some(rel) = f.enclosed_name() else {
            continue;
        };
        let out = stage.join(local_name(rel));
        if f.is_dir() {
            fs::create_dir_all(&out).ctx("create", &out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            fs::create_dir_all(p).ctx("create", p)?;
        }
        if f.is_symlink() {
            let mut target = String::new();
            check(f.read_to_string(&mut target), cancel, "zip")?;
            make_symlink(&target, &out, stage);
            continue;
        }
        let mut w = fs::File::create(&out).ctx("create", &out)?;
        check(io::copy(&mut f, &mut w), cancel, "zip")?;
        #[cfg(unix)]
        if let Some(mode) = f.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o7777));
        }
    }
    Ok(())
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

/// Recreates a symlink from a zip entry if it stays inside the extracted tree.
fn make_symlink(target: &str, link: &Path, stage: &Path) {
    let resolved = link.parent().unwrap_or(stage).join(target);
    let inside =
        safe_relative(target).is_some() || resolved.starts_with(stage) && !target.contains("..");
    if !inside {
        return;
    }
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(target, link);
    // Windows wants `\` in the target and a directory link for a folder. Creating links
    // needs Developer Mode or admin rights; without them the link is left out.
    #[cfg(windows)]
    {
        let target = target.replace('/', "\\");
        let _ = if resolved.is_dir() {
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
