//! Windows file clipboard: `CF_HDROP` plus `Preferred DropEffect`, the formats Explorer uses,
//! so files copied or cut in Explorer paste into Gall and the other way round. The byte
//! layout lives in `gall_core::util::hdrop`; this file only talks to the clipboard.

use gall_core::util::hdrop;
use std::path::PathBuf;
use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;

/// Closes the clipboard when dropped.
struct Open;

impl Open {
    /// Another program can hold the clipboard for a moment: retry briefly.
    fn new() -> Result<Open, String> {
        let mut last = String::new();
        for _ in 0..10 {
            // SAFETY: no window handle: the clipboard is opened for the current task.
            match unsafe { OpenClipboard(Some(HWND::default())) } {
                Ok(()) => return Ok(Open),
                Err(err) => last = err.to_string(),
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Err(format!("clipboard unavailable: {last}"))
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: only created after a successful OpenClipboard.
        let _ = unsafe { CloseClipboard() };
    }
}

fn effect_format() -> u32 {
    // SAFETY: a static NUL-terminated wide string.
    unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) }
}

/// A movable global block holding `bytes`, ready for `SetClipboardData`.
fn global_from(bytes: &[u8]) -> Result<HGLOBAL, String> {
    // SAFETY: the block is locked only while it is filled, with exactly its allocated size.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).map_err(|e| e.to_string())?;
        let p = GlobalLock(mem);
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err("clipboard memory is not available".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(mem);
        Ok(mem)
    }
}

/// Puts `bytes` on the (open, emptied) clipboard under `format`. On success the system owns the block.
fn put(format: u32, bytes: &[u8]) -> Result<(), String> {
    let mem = global_from(bytes)?;
    // SAFETY: `mem` is a valid movable block; ownership passes to the clipboard on success.
    match unsafe { SetClipboardData(format, Some(HANDLE(mem.0))) } {
        Ok(_) => Ok(()),
        Err(err) => {
            // SAFETY: the clipboard did not take the block.
            let _ = unsafe { GlobalFree(Some(mem)) };
            Err(err.to_string())
        }
    }
}

/// The bytes of one clipboard format, `None` when absent. The clipboard must be open.
fn get(format: u32) -> Option<Vec<u8>> {
    // SAFETY: the handle belongs to the clipboard; it is only read while locked.
    unsafe {
        IsClipboardFormatAvailable(format).ok()?;
        let h = GetClipboardData(format).ok()?;
        let mem = HGLOBAL(h.0);
        let size = GlobalSize(mem);
        let p = GlobalLock(mem);
        if p.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(p.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(mem);
        Some(bytes)
    }
}

/// Copies or cuts `paths` for Explorer: the drop list, the effect, and the paths as text.
pub fn set_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    let list: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let mut text: Vec<u8> = Vec::new();
    for u in list.join("\r\n").encode_utf16().chain(std::iter::once(0)) {
        text.extend_from_slice(&u.to_le_bytes());
    }
    let _open = Open::new()?;
    // SAFETY: the clipboard is open.
    unsafe { EmptyClipboard() }.map_err(|e| e.to_string())?;
    put(CF_HDROP, &hdrop::build_hdrop(&list))?;
    put(effect_format(), &hdrop::drop_effect(cut))?;
    put(CF_UNICODETEXT, &text)
}

/// The files on the clipboard (from Explorer or Gall) and whether they were cut.
pub fn read_files() -> Option<(Vec<PathBuf>, bool)> {
    let _open = Open::new().ok()?;
    let paths = hdrop::parse_hdrop(&get(CF_HDROP)?);
    if paths.is_empty() {
        return None;
    }
    let cut = hdrop::is_cut(get(effect_format()).as_deref());
    Some((paths.into_iter().map(PathBuf::from).collect(), cut))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the real clipboard (it replaces its content), so it only runs on request:
    /// `cargo test -p gall windows_clipboard -- --ignored`.
    #[test]
    #[ignore]
    fn files_round_trip_through_the_system_clipboard() {
        let paths = vec![
            PathBuf::from(r"C:\Users\Public\Čšž dokument 日本語.txt"),
            PathBuf::from(format!(r"C:\{}\long.txt", "d".repeat(280))),
        ];
        for cut in [false, true] {
            set_files(&paths, cut).unwrap();
            assert_eq!(read_files(), Some((paths.clone(), cut)));
        }
    }
}

#[cfg(test)]
mod interop {
    use super::*;

    /// Writes a real file to the clipboard, for a script to check what Explorer-style readers see.
    #[test]
    #[ignore]
    fn write_for_external_reader() {
        let p = std::env::var("GALL_CLIP_FILE").expect("GALL_CLIP_FILE");
        set_files(&[PathBuf::from(p)], std::env::var("GALL_CLIP_CUT").is_ok()).unwrap();
    }

    /// Prints what is on the clipboard (a file list put there by another program).
    #[test]
    #[ignore]
    fn print_clipboard_files() {
        println!("CLIPFILES {:?}", read_files());
    }
}
