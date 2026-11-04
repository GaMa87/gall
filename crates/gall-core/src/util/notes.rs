//! Notes / To-Do items (O-09): plain text files with `.2do` (open) and
//! `.2dx` (done) extensions, a format chosen for interoperability.

use crate::error::{CoreError, IoContext, Result};
use crate::fs::ops::{create_file, UndoJournal, UndoRecord};
use std::path::{Path, PathBuf};

pub const OPEN_EXT: &str = "2do";
pub const DONE_EXT: &str = "2dx";

/// Derives a file name from the first line of the note text.
pub fn note_file_name(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Note");
    let clean: String = first
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|\0".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut name: String = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() > 60 {
        name = name
            .chars()
            .take(60)
            .collect::<String>()
            .trim_end()
            .to_string();
    }
    if name.is_empty() || name.starts_with('.') {
        name = format!("Note{name}");
    }
    format!("{name}.{OPEN_EXT}")
}

pub fn create_note(dir: &Path, text: &str, journal: &UndoJournal) -> Result<PathBuf> {
    let name = note_file_name(text);
    let target = crate::fs::ops::unique_path(dir, &name);
    let fname = target.file_name().unwrap().to_string_lossy().into_owned();
    create_file(dir, &fname, None, Some(text.as_bytes()), journal)
}

/// Toggles done state by swapping the extension; returns the new path.
pub fn toggle_done(path: &Path, journal: &UndoJournal) -> Result<PathBuf> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);
    let new_ext = match ext.as_deref() {
        Some(OPEN_EXT) => DONE_EXT,
        Some(DONE_EXT) => OPEN_EXT,
        _ => return Err(CoreError::Other("not a note".into())),
    };
    let dest = path.with_extension(new_ext);
    if dest.exists() {
        return Err(CoreError::AlreadyExists(dest));
    }
    std::fs::rename(path, &dest).ctx("rename", path)?;
    crate::fs::ops::notify_moved(path, &dest);
    journal.push(UndoRecord::Moved {
        pairs: vec![(path.to_path_buf(), dest.clone())],
    });
    Ok(dest)
}

/// Saves an edited note: written to a temporary file first and then put in place, so a
/// crash or a full disk never leaves the note half written.
pub fn update_note(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    // A symlinked note: save the file it points to, keep the link.
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = real.as_path();
    let tmp = path.with_file_name(format!(
        ".{}.gall-save-{}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
    ));
    let res = (|| {
        let mut f = std::fs::File::create(&tmp).ctx("write", &tmp)?;
        f.write_all(text.as_bytes()).ctx("write", &tmp)?;
        f.sync_all().ctx("write", &tmp)?;
        if let Ok(m) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, m.permissions());
        }
        std::fs::rename(&tmp, path).ctx("write", path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming() {
        assert_eq!(
            note_file_name("  Buy milk / eggs\nmore"),
            "Buy milk eggs.2do"
        );
        assert_eq!(note_file_name(""), "Note.2do");
        assert_eq!(note_file_name(".env"), "Note.env.2do");
    }

    #[test]
    fn update_is_atomic_and_keeps_links() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real.2do");
        std::fs::write(&real, "old").unwrap();
        #[cfg(unix)]
        {
            let link = d.path().join("link.2do");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            update_note(&link, "new").unwrap();
            assert!(std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
        }
        update_note(&real, "newer").unwrap();
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "newer");
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            names
                .iter()
                .all(|n| !n.to_string_lossy().contains("gall-save")),
            "{names:?}"
        );
    }

    #[test]
    fn create_and_toggle() {
        let d = tempfile::tempdir().unwrap();
        let j = UndoJournal::default();
        let p = create_note(d.path(), "Call Ana", &j).unwrap();
        let p2 = create_note(d.path(), "Call Ana", &j).unwrap();
        assert_ne!(p, p2);
        let done = toggle_done(&p, &j).unwrap();
        assert_eq!(done.extension().unwrap(), "2dx");
        let open = toggle_done(&done, &j).unwrap();
        assert_eq!(std::fs::read_to_string(open).unwrap(), "Call Ana");
    }
}
