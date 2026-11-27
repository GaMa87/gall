//! macOS file clipboard (`public.file-url` via `NSPasteboard`) and Finder drag-in
//! pasteboard fallback (`NSDragPboard`).
//!
//! Finder puts one `NSPasteboardItem` per file on `NSPasteboard.generalPasteboard()`,
//! carrying `public.file-url` (often as a file-reference URL `file:///.file/id=…`) plus
//! `public.utf8-plain-text` with only the file name. Reading `public.file-url` before plain
//! text lets Cmd+V paste files copied in Finder, and writing per-item `public.file-url`
//! objects lets Finder paste files copied in Gall.
//!
//! macOS has no standard clipboard flag for cut vs copy (Finder uses Cmd+Opt+V at paste
//! time). Gall writes a private marker type (`org.gall.cut-files`) alongside the file URLs
//! when cutting so Gall-to-Gall cut/paste preserves the cut state, while any subsequent copy
//! in Finder calls `clearContents()` and clears the marker automatically.

use objc2::runtime::ProtocolObject;
use objc2::ClassType;
#[allow(deprecated)]
use objc2_app_kit::NSFilenamesPboardType;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardNameDrag, NSPasteboardTypeFileURL,
    NSPasteboardTypeString, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSString, NSURL};
use std::path::PathBuf;

const CUT_MARKER_TYPE: &str = "org.gall.cut-files";

/// Copies or cuts `paths` onto the macOS system clipboard (`public.file-url` + plain text).
pub fn set_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    let pb = NSPasteboard::generalPasteboard();
    set_files_on(&pb, paths, cut)
}

/// Reads file paths from the macOS system clipboard and whether Gall marked them as cut.
pub fn read_files() -> Option<(Vec<PathBuf>, bool)> {
    let pb = NSPasteboard::generalPasteboard();
    read_files_from(&pb)
}

/// Reads file paths currently on the macOS drag pasteboard (`NSDragPboard`), used as a
/// fallback when WKWebView omits `text/uri-list` during a drag from Finder.
pub fn read_drag_files() -> Vec<PathBuf> {
    // SAFETY: `NSPasteboardNameDrag` is a static AppKit pasteboard name constant.
    let pb = NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag });
    read_files_from(&pb)
        .map(|(paths, _)| paths)
        .unwrap_or_default()
}

/// Diagnostic dump of file clipboard representations for self-tests.
pub fn dump() -> Vec<(String, String)> {
    let pb = NSPasteboard::generalPasteboard();
    dump_from(&pb)
}

fn set_files_on(pb: &NSPasteboard, paths: &[PathBuf], cut: bool) -> Result<(), String> {
    if paths.is_empty() {
        pb.clearContents();
        return Ok(());
    }
    let joined_text = paths
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    let cut_type = NSString::from_str(CUT_MARKER_TYPE);
    let cut_val = NSString::from_str("1");

    let mut items: Vec<objc2::rc::Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
        Vec::with_capacity(paths.len());
    for (idx, path) in paths.iter().enumerate() {
        let Some(s) = path.to_str() else {
            return Err(format!(
                "non-UTF-8 path cannot be copied: {}",
                path.display()
            ));
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(s));
        let Some(abs) = url.absoluteString() else {
            return Err(format!("cannot build file URL for {}", path.display()));
        };
        let item = NSPasteboardItem::new();
        // SAFETY: `NSPasteboardTypeFileURL` / `NSPasteboardTypeString` are static AppKit constants.
        unsafe {
            item.setString_forType(&abs, NSPasteboardTypeFileURL);
            if idx == 0 {
                item.setString_forType(&NSString::from_str(&joined_text), NSPasteboardTypeString);
                if cut {
                    item.setString_forType(&cut_val, &cut_type);
                }
            }
        }
        items.push(ProtocolObject::from_retained(item));
    }

    let arr = NSArray::from_retained_slice(&items);
    pb.clearContents();
    if pb.writeObjects(&arr) {
        Ok(())
    } else {
        Err("macOS pasteboard refused file items".into())
    }
}

fn resolve_file_url(url: &NSURL) -> Option<PathBuf> {
    if !url.isFileURL() {
        return None;
    }
    // Finder frequently writes file-reference URLs (`file:///.file/id=…`).
    // `filePathURL()` resolves them to a POSIX file path URL; fall back to `url` itself
    // when the target does not exist (e.g. deleted path or synthetic test URL).
    let path = url
        .filePathURL()
        .and_then(|u| u.path())
        .or_else(|| url.path())?;
    let s = path.to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

fn read_files_from(pb: &NSPasteboard) -> Option<(Vec<PathBuf>, bool)> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut cut = false;
    let cut_type = NSString::from_str(CUT_MARKER_TYPE);

    if let Some(items) = pb.pasteboardItems() {
        for item in items.iter() {
            if item.stringForType(&cut_type).is_some() {
                cut = true;
            }
            // SAFETY: `NSPasteboardTypeFileURL` is a static AppKit constant.
            if let Some(raw) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) {
                if let Some(url) = NSURL::URLWithString(&raw) {
                    if let Some(p) = resolve_file_url(&url) {
                        paths.push(p);
                    }
                }
            }
        }
    }

    if paths.is_empty() {
        // Fallback: `readObjectsForClasses:options:` handles legacy `NSFilenamesPboardType`
        // and Cocoa object writers that did not populate string-typed `public.file-url`.
        let classes = NSArray::from_slice(&[NSURL::class()]);
        // SAFETY: `NSURL` implements `NSPasteboardReading`; `options` is `None`.
        if let Some(objs) = unsafe { pb.readObjectsForClasses_options(&classes, None) } {
            for obj in objs.iter() {
                if let Some(url) = obj.downcast_ref::<NSURL>() {
                    if let Some(p) = resolve_file_url(url) {
                        paths.push(p);
                    }
                }
            }
        }
    }

    if paths.is_empty() {
        #[allow(deprecated)]
        // SAFETY: `NSFilenamesPboardType` is a static AppKit constant; property list is checked via downcast.
        unsafe {
            let types = NSArray::arrayWithObject(NSFilenamesPboardType);
            if pb.availableTypeFromArray(&types).is_some() {
                if let Some(plist) = pb.propertyListForType(NSFilenamesPboardType) {
                    if let Ok(arr) = plist.downcast::<NSArray>() {
                        for item in arr {
                            if let Ok(s) = item.downcast::<NSString>() {
                                let text = s.to_string();
                                if !text.is_empty() {
                                    paths.push(PathBuf::from(text));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if paths.is_empty() {
        None
    } else {
        Some((paths, cut))
    }
}

fn dump_from(pb: &NSPasteboard) -> Vec<(String, String)> {
    let mut file_urls = Vec::new();
    let mut plain = Vec::new();
    let mut cut_flag = false;
    let cut_type = NSString::from_str(CUT_MARKER_TYPE);

    if let Some(items) = pb.pasteboardItems() {
        for item in items.iter() {
            // SAFETY: static AppKit pasteboard type constants.
            unsafe {
                if let Some(u) = item.stringForType(NSPasteboardTypeFileURL) {
                    file_urls.push(u.to_string());
                }
                if let Some(t) = item.stringForType(NSPasteboardTypeString) {
                    plain.push(t.to_string());
                }
            }
            if item.stringForType(&cut_type).is_some() {
                cut_flag = true;
            }
        }
    }
    let mut out = Vec::new();
    if !file_urls.is_empty() {
        out.push(("public.file-url".into(), file_urls.join("\n")));
    }
    if cut_flag {
        out.push((CUT_MARKER_TYPE.into(), "1".into()));
    }
    if !plain.is_empty() {
        out.push(("public.utf8-plain-text".into(), plain.join("\n")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_pasteboard_round_trip_copy_cut_and_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("hello world.txt");
        let b = dir.path().join("Čšž dokument 日本語 #1.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();

        let pb = NSPasteboard::pasteboardWithUniqueName();
        let paths = vec![a.clone(), b.clone()];

        for cut in [false, true] {
            set_files_on(&pb, &paths, cut).unwrap();
            let got = read_files_from(&pb).expect("files on pasteboard");
            assert_eq!(got.1, cut);
            let got_canon: Vec<PathBuf> = got
                .0
                .iter()
                .map(|p| std::fs::canonicalize(p).unwrap())
                .collect();
            let want_canon: Vec<PathBuf> = paths
                .iter()
                .map(|p| std::fs::canonicalize(p).unwrap())
                .collect();
            assert_eq!(got_canon, want_canon);

            let dumped = dump_from(&pb);
            assert!(dumped.iter().any(|(k, _)| k == "public.file-url"));
            assert!(dumped.iter().any(|(k, _)| k == "public.utf8-plain-text"));
            assert_eq!(dumped.iter().any(|(k, _)| k == CUT_MARKER_TYPE), cut);
        }
    }

    #[test]
    fn resolves_finder_file_reference_urls_and_clears_cut_flag() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("finder file.txt");
        std::fs::write(&f, "hello").unwrap();

        let pb = NSPasteboard::pasteboardWithUniqueName();
        // Start with a Gall cut, then simulate Finder overwriting the pasteboard with a
        // file-reference URL (`file:///.file/id=…`) and bare filename plain text.
        set_files_on(&pb, std::slice::from_ref(&f), true).unwrap();
        assert_eq!(read_files_from(&pb).map(|(_, c)| c), Some(true));

        let url = NSURL::fileURLWithPath(&NSString::from_str(f.to_str().unwrap()));
        let ref_url = url.fileReferenceURL().expect("fileReferenceURL");
        let ref_str = ref_url.absoluteString().expect("absoluteString");
        assert!(
            ref_str.to_string().contains("/.file/id="),
            "expected Finder-style file reference URL, got {}",
            ref_str
        );

        let item = NSPasteboardItem::new();
        // SAFETY: static AppKit pasteboard type constants.
        unsafe {
            item.setString_forType(&ref_str, NSPasteboardTypeFileURL);
            item.setString_forType(
                &NSString::from_str("finder file.txt"),
                NSPasteboardTypeString,
            );
        }
        let arr = NSArray::from_retained_slice(&[ProtocolObject::from_retained(item)]);
        pb.clearContents();
        assert!(pb.writeObjects(&arr));

        let (got_paths, got_cut) = read_files_from(&pb).expect("resolved Finder URL");
        assert!(!got_cut, "Finder copy must clear Gall's cut marker");
        assert_eq!(got_paths.len(), 1);
        assert_eq!(
            std::fs::canonicalize(&got_paths[0]).unwrap(),
            std::fs::canonicalize(&f).unwrap()
        );
    }

    #[test]
    fn plain_text_only_returns_none() {
        let pb = NSPasteboard::pasteboardWithUniqueName();
        let item = NSPasteboardItem::new();
        // SAFETY: static AppKit pasteboard type constant.
        unsafe {
            item.setString_forType(&NSString::from_str("just text"), NSPasteboardTypeString);
        }
        let arr = NSArray::from_retained_slice(&[ProtocolObject::from_retained(item)]);
        pb.clearContents();
        assert!(pb.writeObjects(&arr));
        assert_eq!(read_files_from(&pb), None);
    }

    /// Uses the real system clipboard (`generalPasteboard`), so it only runs on request:
    /// `cargo test -p gall macos_clipboard::tests::general_pasteboard_round_trip -- --ignored`.
    #[test]
    #[ignore]
    fn general_pasteboard_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("Čšž general.txt");
        std::fs::write(&f, "ok").unwrap();
        for cut in [false, true] {
            set_files(std::slice::from_ref(&f), cut).unwrap();
            let (got, got_cut) = read_files().expect("generalPasteboard files");
            assert_eq!(got_cut, cut);
            assert_eq!(
                std::fs::canonicalize(&got[0]).unwrap(),
                std::fs::canonicalize(&f).unwrap()
            );
        }
    }
}
