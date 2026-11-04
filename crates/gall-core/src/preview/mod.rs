//! Preview engine (P-01…P-08): classify a file and extract what the details
//! pane needs. Heavy rendering (images, video, PDF) happens in the webview;
//! this module supplies metadata, text and archive listings.

use crate::error::{IoContext, Result};
use crate::fs::FileEntry;
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};

pub mod video;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewKind {
    Folder,
    Image,
    Video,
    Audio,
    Pdf,
    Text,
    Archive,
    Note,
    None,
}

const IMAGE: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "avif", "apng",
];
const VIDEO: &[&str] = &["mp4", "webm", "mkv", "mov", "m4v", "ogv", "avi"];
const AUDIO: &[&str] = &["mp3", "ogg", "oga", "wav", "flac", "m4a", "aac", "opus"];
const ARCHIVE: &[&str] = &[
    "zip", "jar", "apk", "docx", "xlsx", "pptx", "odt", "ods", "epub",
];
const TEXT: &[&str] = &[
    "txt",
    "md",
    "markdown",
    "log",
    "csv",
    "tsv",
    "json",
    "jsonc",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "xml",
    "html",
    "htm",
    "css",
    "scss",
    "js",
    "mjs",
    "cjs",
    "ts",
    "tsx",
    "jsx",
    "rs",
    "py",
    "rb",
    "go",
    "java",
    "kt",
    "c",
    "h",
    "cpp",
    "hpp",
    "cc",
    "cs",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "cmd",
    "sql",
    "lua",
    "vue",
    "svelte",
    "gitignore",
    "env",
    "properties",
    "gradle",
    "cmake",
    "makefile",
    "dockerfile",
    "srt",
    "vtt",
    "tex",
    "nfo",
    "reg",
    "url",
    "desktop",
    "service",
];

pub fn classify(entry: &FileEntry) -> PreviewKind {
    if entry.is_dir {
        return PreviewKind::Folder;
    }
    let ext = entry.ext.as_str();
    let lname = entry.name.to_lowercase();
    match ext {
        "2do" | "2dx" => PreviewKind::Note,
        "pdf" => PreviewKind::Pdf,
        e if IMAGE.contains(&e) => PreviewKind::Image,
        e if VIDEO.contains(&e) => PreviewKind::Video,
        e if AUDIO.contains(&e) => PreviewKind::Audio,
        e if ARCHIVE.contains(&e) => PreviewKind::Archive,
        _ if crate::fs::archive::detect(Path::new(&entry.name))
            .is_some_and(|(f, _)| crate::fs::archive_view::browsable(f)) =>
        {
            PreviewKind::Archive
        }
        e if TEXT.contains(&e) => PreviewKind::Text,
        "" if ["makefile", "dockerfile", "readme", "license", "changelog"]
            .contains(&lname.as_str()) =>
        {
            PreviewKind::Text
        }
        // Any other source file the syntax colouring knows (psm1, php, swift, …).
        // read_text still refuses binary content.
        _ if crate::fs::syntax::knows(&entry.name) => PreviewKind::Text,
        _ => PreviewKind::None,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveItem {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub kind: Option<PreviewKind>,
    pub entry: Option<FileEntry>,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub text: Option<String>,
    pub text_truncated: bool,
    /// True if the file looked binary although its extension suggested text.
    pub binary: bool,
    pub archive: Vec<ArchiveItem>,
    pub archive_total: usize,
    /// Folder summary: (items, bytes of direct children).
    pub folder_items: Option<usize>,
    pub folder_bytes: Option<u64>,
    /// For an item inside an archive: its extracted copy, which the webview loads
    /// (images, video, PDF) instead of the virtual path.
    pub local_path: Option<String>,
}

/// Items inside archives larger than this are not extracted just to preview them.
const ARCHIVE_PREVIEW_MAX: u64 = 64 << 20;

fn preview_in_archive(archive: &Path, inner: &str, text_limit: usize) -> Result<PreviewInfo> {
    use crate::fs::archive_view as av;
    let entry = av::stat(archive, inner)?
        .ok_or_else(|| crate::CoreError::NotFound(av::virtual_path(archive, inner).into()))?;
    if entry.is_dir {
        let (n, bytes) = av::folder_totals(archive, inner)?;
        return Ok(PreviewInfo {
            kind: Some(PreviewKind::Folder),
            folder_items: Some(n),
            folder_bytes: Some(bytes),
            entry: Some(entry),
            ..Default::default()
        });
    }
    if entry.size > ARCHIVE_PREVIEW_MAX {
        return Ok(PreviewInfo {
            kind: Some(PreviewKind::None),
            entry: Some(entry),
            ..Default::default()
        });
    }
    let real = av::materialize(&[PathBuf::from(&entry.path)])?.remove(0);
    let mut info = preview_info(&real, text_limit)?;
    info.local_path = Some(real.to_string_lossy().into_owned());
    info.entry = Some(entry);
    Ok(info)
}

pub const ARCHIVE_LIMIT: usize = 200;

/// Reads up to `max` bytes and decodes them as UTF-8 (lossy). Returns
/// (text, truncated, looks_binary).
pub fn read_text(path: &Path, max: usize) -> Result<(String, bool, bool)> {
    let f = crate::fs::open_regular(path).ctx("open", path)?;
    let mut buf = Vec::with_capacity(max.min(1 << 20));
    f.take(max as u64 + 1)
        .read_to_end(&mut buf)
        .ctx("read", path)?;
    let truncated = buf.len() > max;
    buf.truncate(max);
    let sample = &buf[..buf.len().min(8192)];
    let binary = sample.contains(&0) || {
        let ctrl = sample
            .iter()
            .filter(|&&b| b < 9 || (b > 13 && b < 32))
            .count();
        !sample.is_empty() && ctrl * 10 > sample.len()
    };
    // Strip UTF-8 BOM.
    let start = if buf.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    let mut text = String::from_utf8_lossy(&buf[start..]).into_owned();
    if truncated {
        // Don't end on a half-decoded replacement char.
        while text.ends_with('\u{FFFD}') {
            text.pop();
        }
    }
    Ok((text, truncated, binary))
}

pub fn list_archive(path: &Path) -> Result<(Vec<ArchiveItem>, usize)> {
    let f = crate::fs::open_regular(path).ctx("open", path)?;
    let mut z =
        zip::ZipArchive::new(f).map_err(|e| crate::CoreError::Other(format!("archive: {e}")))?;
    let total = z.len();
    let mut items = Vec::new();
    for i in 0..total.min(ARCHIVE_LIMIT) {
        if let Ok(e) = z.by_index_raw(i) {
            items.push(ArchiveItem {
                name: e.name().map(|n| n.to_string()).unwrap_or_default(),
                size: e.size(),
                is_dir: e.is_dir(),
            });
        }
    }
    Ok((items, total))
}

/// Gathers preview data for one path. Never fails for unsupported types: it
/// returns `kind = None` and the bare entry so the properties pane still works.
pub fn preview_info(path: &Path, text_limit: usize) -> Result<PreviewInfo> {
    if let Some((archive, inner)) = crate::fs::archive_view::split(path) {
        if !inner.is_empty() {
            return preview_in_archive(&archive, &inner, text_limit);
        }
    }
    let entry = FileEntry::from_path(path).ctx("stat", path)?;
    // Pipes, sockets and devices: nothing to read (opening a pipe can wait forever).
    if std::fs::metadata(path).is_ok_and(|m| !m.is_file() && !m.is_dir()) {
        return Ok(PreviewInfo {
            kind: Some(PreviewKind::None),
            entry: Some(entry),
            ..Default::default()
        });
    }
    let kind = classify(&entry);
    let mut info = PreviewInfo {
        kind: Some(kind),
        ..Default::default()
    };
    match kind {
        PreviewKind::Image => {
            if let Ok(dim) = imagesize::size(path) {
                info.width = Some(dim.width);
                info.height = Some(dim.height);
            }
        }
        PreviewKind::Text | PreviewKind::Note => {
            let (t, trunc, bin) = read_text(path, text_limit)?;
            info.binary = bin;
            if !bin {
                info.text = Some(t);
                info.text_truncated = trunc;
            }
        }
        PreviewKind::Archive if crate::fs::archive_view::is_browsable_file(path) => {
            // Top level of zip / 7z / tar.*: what pressing Enter will show.
            if let Ok(items) = crate::fs::archive_view::index(path) {
                let top: Vec<_> = items.iter().filter(|i| !i.name.contains('/')).collect();
                info.archive_total = top.len();
                info.archive = top
                    .iter()
                    .take(ARCHIVE_LIMIT)
                    .map(|i| ArchiveItem {
                        name: i.name.clone(),
                        size: i.size,
                        is_dir: i.is_dir,
                    })
                    .collect();
            }
        }
        PreviewKind::Archive => {
            if let Ok((items, total)) = list_archive(path) {
                info.archive = items;
                info.archive_total = total;
            }
        }
        PreviewKind::Folder => {
            if let Ok(rd) = std::fs::read_dir(path) {
                let (mut n, mut bytes) = (0usize, 0u64);
                for e in rd.flatten() {
                    n += 1;
                    if let Ok(m) = e.metadata() {
                        if m.is_file() {
                            bytes += m.len();
                        }
                    }
                }
                info.folder_items = Some(n);
                info.folder_bytes = Some(bytes);
            }
        }
        PreviewKind::None
            // Unknown extension: sniff a little; show as text if it looks like text.
            if entry.size > 0 && entry.size < 4 << 20 => {
                if let Ok((t, trunc, bin)) = read_text(path, text_limit) {
                    if !bin {
                        info.kind = Some(PreviewKind::Text);
                        info.text = Some(t);
                        info.text_truncated = trunc;
                    }
                }
            }
        _ => {}
    }
    info.entry = Some(entry);
    Ok(info)
}

/// First `lines` non-empty lines of a text file, for Thumbs+Details excerpts.
pub fn excerpt(path: &Path, lines: usize) -> Option<String> {
    let (t, _, bin) = read_text(path, 4096).ok()?;
    if bin {
        return None;
    }
    let s: Vec<&str> = t
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(lines)
        .collect();
    (!s.is_empty()).then(|| s.join(" ⏎ "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn classify_and_text() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.md");
        std::fs::write(&p, "\u{FEFF}# Title\n\nbody").unwrap();
        let i = preview_info(&p, 1000).unwrap();
        assert_eq!(i.kind, Some(PreviewKind::Text));
        assert_eq!(i.text.as_deref(), Some("# Title\n\nbody"));
        assert_eq!(excerpt(&p, 2).as_deref(), Some("# Title ⏎ body"));

        let p = d.path().join("blob.txt");
        std::fs::write(&p, [0u8, 1, 2, 3, 0, 0]).unwrap();
        let i = preview_info(&p, 1000).unwrap();
        assert!(i.binary && i.text.is_none());
    }

    #[test]
    fn truncation() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("big.log");
        std::fs::write(&p, "x".repeat(5000)).unwrap();
        let i = preview_info(&p, 100).unwrap();
        assert!(i.text_truncated);
        assert_eq!(i.text.unwrap().len(), 100);
    }

    #[test]
    fn archive_listing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.zip");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        z.start_file::<_, ()>("inner.txt", Default::default())
            .unwrap();
        z.write_all(b"hello").unwrap();
        z.finish().unwrap();
        let i = preview_info(&p, 10).unwrap();
        assert_eq!(i.kind, Some(PreviewKind::Archive));
        assert_eq!(i.archive_total, 1);
        assert_eq!(i.archive[0].name, "inner.txt");
    }

    #[test]
    fn folder_summary() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), "123").unwrap();
        std::fs::create_dir(d.path().join("s")).unwrap();
        let i = preview_info(d.path(), 10).unwrap();
        assert_eq!((i.folder_items, i.folder_bytes), (Some(2), Some(3)));
    }
}
