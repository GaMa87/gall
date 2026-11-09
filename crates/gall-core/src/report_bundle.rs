//! Bounded, local-only report bundles with validated root-level entry names.

use std::io::{Cursor, Write};
use std::path::Path;

pub const MAX_MEDIA_BYTES: usize = 8 * 1024 * 1024;
const MAX_REPORT_BYTES: usize = 1_000_000;
const MAX_ATTACHMENTS: usize = 3;

#[derive(Debug, serde::Deserialize)]
pub struct ReportAttachment {
    pub name: String,
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Check the PNG header before asking the decoder to allocate pixel storage.
pub fn decode_report_png(png: &[u8]) -> Result<image::RgbaImage, String> {
    if png.len() > MAX_MEDIA_BYTES {
        return Err("Screenshot exceeds the 8 MiB limit.".into());
    }
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return Err("Screenshot must be a valid PNG image.".into());
    }
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > 16_777_216
    {
        return Err("Screenshot exceeds the image dimension limit.".into());
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map(|img| img.to_rgba8())
        .map_err(|_| "Screenshot must be a valid PNG image.".into())
}

fn ebml_size(bytes: &[u8]) -> Option<(usize, usize)> {
    let first = *bytes.first()?;
    let count = first.leading_zeros() as usize + 1;
    if count > 8 || bytes.len() < count {
        return None;
    }
    let mut value = u64::from(first) & (0xffu64 >> count);
    for byte in &bytes[1..count] {
        value = (value << 8) | u64::from(*byte);
    }
    if value == (1u64 << (7 * count)) - 1 {
        return None;
    }
    Some((count, usize::try_from(value).ok()?))
}

fn is_webm(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"\x1a\x45\xdf\xa3") {
        return false;
    }
    let Some((size_len, header_len)) = ebml_size(&bytes[4..]) else {
        return false;
    };
    let start = 4 + size_len;
    let Some(end) = start
        .checked_add(header_len)
        .filter(|end| *end <= bytes.len())
    else {
        return false;
    };
    let mut cursor = start;
    while cursor < end {
        let id_len = bytes[cursor].leading_zeros() as usize + 1;
        if id_len > 4 || cursor + id_len >= end {
            return false;
        }
        let id = &bytes[cursor..cursor + id_len];
        cursor += id_len;
        let Some((size_len, size)) = ebml_size(&bytes[cursor..end]) else {
            return false;
        };
        cursor += size_len;
        let Some(next) = cursor.checked_add(size).filter(|next| *next <= end) else {
            return false;
        };
        if id == b"\x42\x82" {
            return &bytes[cursor..next] == b"webm";
        }
        cursor = next;
    }
    false
}

fn is_mp4(bytes: &[u8]) -> bool {
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
    if size < 16 || size > bytes.len() || !size.is_multiple_of(4) {
        return false;
    }
    let supported = |brand: &[u8]| {
        matches!(
            brand,
            b"isom"
                | b"iso2"
                | b"iso3"
                | b"iso4"
                | b"iso5"
                | b"iso6"
                | b"mp41"
                | b"mp42"
                | b"avc1"
                | b"dash"
                | b"M4V "
                | b"MSNV"
        )
    };
    supported(&bytes[8..12])
        || bytes[16..size]
            .as_chunks::<4>()
            .0
            .iter()
            .any(|brand| supported(brand))
}

/// Validate media using the same limits and signatures for individual and ZIP exports.
pub fn validate_report_media(bytes: &[u8], mime: &str) -> Result<(), String> {
    if bytes.len() > MAX_MEDIA_BYTES {
        return Err("Media exceeds the 8 MiB limit.".into());
    }
    match mime {
        "image/png" => {
            decode_report_png(bytes)?;
        }
        "video/webm" if is_webm(bytes) => {}
        "video/mp4" if is_mp4(bytes) => {}
        _ => {
            return Err("Media must be a valid PNG, WebM, or MP4 with a matching MIME type.".into())
        }
    }
    Ok(())
}

fn validate_attachment_name(name: &str, mime: &str) -> Result<(), String> {
    let extension = match mime {
        "image/png" => "png",
        "video/webm" => "webm",
        "video/mp4" => "mp4",
        _ => return Err("Attachment MIME type is unsupported.".into()),
    };
    if name.is_empty()
        || name.len() > 180
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || r#"/\:"<>|?*"#.contains(c))
    {
        return Err("Attachment names must be safe file basenames.".into());
    }
    let Some((stem, suffix)) = name.rsplit_once('.') else {
        return Err("Attachment filename extension must match its MIME type.".into());
    };
    if stem.is_empty() || !suffix.eq_ignore_ascii_case(extension) {
        return Err("Attachment filename extension must match its MIME type.".into());
    }
    let device = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    if matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            device.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
    {
        return Err("Attachment names must not use reserved device names.".into());
    }
    Ok(())
}

fn prepare_bundle(body: &str, attachments: &[ReportAttachment]) -> Result<Vec<u8>, String> {
    if body.len() > MAX_REPORT_BYTES {
        return Err("Report exceeds export limit".into());
    }
    if attachments.len() > MAX_ATTACHMENTS {
        return Err("A report can contain at most 3 attachments.".into());
    }
    let mut names = std::collections::HashSet::from(["report.md".to_string()]);
    for attachment in attachments {
        validate_report_media(&attachment.bytes, &attachment.mime)?;
        validate_attachment_name(&attachment.name, &attachment.mime)?;
        if !names.insert(attachment.name.to_lowercase()) {
            return Err("Attachment filenames must be unique, ignoring case.".into());
        }
    }
    let capacity = body.len()
        + attachments
            .iter()
            .map(|item| item.bytes.len())
            .sum::<usize>()
        + 4096;
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::with_capacity(capacity)));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    archive
        .start_file("report.md", options)
        .map_err(|e| format!("Cannot prepare report ZIP: {e}"))?;
    archive
        .write_all(body.as_bytes())
        .map_err(|e| format!("Cannot prepare report ZIP: {e}"))?;
    for attachment in attachments {
        archive
            .start_file(&attachment.name, options)
            .map_err(|e| format!("Cannot prepare report ZIP: {e}"))?;
        archive
            .write_all(&attachment.bytes)
            .map_err(|e| format!("Cannot prepare report ZIP: {e}"))?;
    }
    archive
        .finish()
        .map(|cursor| cursor.into_inner())
        .map_err(|e| format!("Cannot finish report ZIP: {e}"))
}

fn write_new_report(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "A file already exists at this location. Choose another name.".into()
            } else {
                format!("Cannot create report: {error}")
            }
        })?;
    if let Err(error) = write(&mut file) {
        drop(file);
        std::fs::remove_file(path).map_err(|cleanup| {
            format!("Cannot write report: {error}. Cannot remove incomplete report: {cleanup}")
        })?;
        return Err(format!("Cannot write report: {error}"));
    }
    Ok(())
}

/// Build and validate everything in memory before exclusively creating the destination.
pub fn export_report_bundle(
    path: &Path,
    body: &str,
    attachments: &[ReportAttachment],
) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Choose an absolute path for the report.".into());
    }
    let bytes = prepare_bundle(body, attachments)?;
    write_new_report(path, |file| {
        file.write_all(&bytes).and_then(|()| file.sync_all())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    struct TestDir(std::path::PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::current_dir().unwrap().join(format!(
                ".report-bundle-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn png() -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(2, 1))
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }
    fn attachment(name: &str, mime: &str, bytes: Vec<u8>) -> ReportAttachment {
        ReportAttachment {
            name: name.into(),
            mime: mime.into(),
            bytes,
        }
    }
    fn entry(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    }
    #[test]
    fn bundle_preserves_exact_report_and_all_media_with_supplied_safe_names() {
        let dir = TestDir::new();
        let path = dir.0.join("report.zip");
        let body = "# Report\nUnicode: ž 日本語\n## Diagnostics\nExact opted-in text\n";
        let attachments = [
            attachment("gall-screenshot-1.png", "image/png", png()),
            attachment(
                "gall-recording-1.webm",
                "video/webm",
                b"\x1a\x45\xdf\xa3\x87\x42\x82\x84webm".to_vec(),
            ),
            attachment(
                "gall-recording-2.mp4",
                "video/mp4",
                b"\0\0\0\x18ftypisom\0\0\0\0isommp42".to_vec(),
            ),
        ];
        export_report_bundle(&path, body, &attachments).unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(std::fs::read(path).unwrap())).unwrap();
        assert_eq!(archive.len(), 4);
        assert_eq!(entry(&mut archive, "report.md"), body.as_bytes());
        for attachment in &attachments {
            assert_eq!(entry(&mut archive, &attachment.name), attachment.bytes);
        }
    }
    #[test]
    fn empty_attachment_list_still_exports_zip_and_preserves_existing_destination() {
        let dir = TestDir::new();
        let path = dir.0.join("report.zip");
        export_report_bundle(&path, "compact text\n", &[]).unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(before.clone())).unwrap();
        assert_eq!(archive.len(), 1);
        assert_eq!(entry(&mut archive, "report.md"), b"compact text\n");
        assert!(export_report_bundle(&path, "replacement", &[])
            .unwrap_err()
            .contains("Choose another name"));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
    #[test]
    fn malformed_or_oversized_input_leaves_no_destination() {
        let dir = TestDir::new();
        let path = dir.0.join("report.zip");
        assert!(export_report_bundle(Path::new("relative.zip"), "", &[])
            .unwrap_err()
            .contains("absolute"));
        assert!(export_report_bundle(&path, &"a".repeat(MAX_REPORT_BYTES + 1), &[]).is_err());
        let too_many = (0..4)
            .map(|_| attachment("a.png", "image/png", png()))
            .collect::<Vec<_>>();
        assert!(export_report_bundle(&path, "", &too_many).is_err());
        for item in [
            attachment("a.png", "image/png", vec![0; MAX_MEDIA_BYTES + 1]),
            attachment("a.png", "image/png", b"not a PNG".to_vec()),
            attachment("a.webm", "video/webm", b"\x1a\x45\xdf\xa3".to_vec()),
            attachment("a.mp4", "video/mp4", b"\0\0\0\x10ftypjpeg\0\0\0\0".to_vec()),
            attachment("a.webm", "video/webm", png()),
            attachment("a.txt", "text/plain", b"metadata".to_vec()),
        ] {
            assert!(export_report_bundle(&path, "", &[item]).is_err());
            assert!(!path.exists());
        }
    }
    #[test]
    fn failed_write_removes_partially_written_destination() {
        let dir = TestDir::new();
        let path = dir.0.join("report.zip");
        let result = write_new_report(&path, |file| {
            file.write_all(b"PK partial")?;
            Err(std::io::Error::other("simulated full disk"))
        });
        assert!(result.unwrap_err().contains("simulated full disk"));
        assert!(!path.exists());
    }
    #[test]
    fn unsafe_duplicate_and_misleading_names_leave_no_destination() {
        let dir = TestDir::new();
        let path = dir.0.join("report.zip");
        for name in [
            "../escape.png",
            r"..\escape.png",
            "/absolute.png",
            r"C:\capture.png",
            "capture.png:stream",
            "capture.png.",
            "capture.png ",
            "bad\n.png",
            "CON.png",
            "COM1.png",
            "LPT².png",
            ".png",
            "image.webm",
            "",
        ] {
            assert!(
                export_report_bundle(&path, "", &[attachment(name, "image/png", png())]).is_err(),
                "{name:?}"
            );
            assert!(!path.exists());
        }
        assert!(export_report_bundle(
            &path,
            "",
            &[
                attachment("capture.png", "image/png", png()),
                attachment("CAPTURE.PNG", "image/png", png()),
            ]
        )
        .unwrap_err()
        .contains("unique"));
        assert!(!path.exists());
    }
    #[test]
    fn recording_magic_rejects_malformed_and_mismatched_containers() {
        assert!(!is_webm(b"\x1a\x45\xdf\xa3\x8b\x42\x82\x88matroska"));
        assert!(!is_webm(b"\x1a\x45\xdf\xa3\xff\x42\x82\x84webm"));
        assert!(is_webm(
            b"\x1a\x45\xdf\xa3\x01\0\0\0\0\0\0\x07\x42\x82\x84webm"
        ));
        assert!(!is_mp4(b"\0\0\x01\0ftypisom\0\0\0\0"));
    }
}
