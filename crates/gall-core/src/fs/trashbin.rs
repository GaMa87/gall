//! The trash as Gall shows it (sidebar › Trash): what is in it, where it came from,
//! and Restore / Delete permanently / Empty. The Windows Recycle Bin is not a folder
//! Gall can list, so this goes through the `trash` crate on every platform.

use crate::error::{CoreError, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    /// Opaque id for restore / delete (the item's path in the trash).
    pub id: String,
    pub name: String,
    /// Where it was deleted from (the folder it goes back to).
    pub original_parent: String,
    /// Unix seconds.
    pub deleted: i64,
    pub is_dir: bool,
    /// Bytes; for a folder everything inside it.
    pub size: u64,
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
mod imp {
    use super::*;
    use crate::fs::ops::with_trash;
    use trash::os_limited;

    /// Where the item's content lies: Windows `C:\$Recycle.Bin\<SID>\$R…` is the id
    /// itself; freedesktop ids are `Trash/info/<x>.trashinfo`, content `Trash/files/<x>`.
    fn stored_path(item: &trash::TrashItem) -> PathBuf {
        let id = PathBuf::from(&item.id);
        if cfg!(windows) {
            return id;
        }
        match (id.parent().and_then(Path::parent), id.file_stem()) {
            (Some(trash_dir), Some(stem)) => trash_dir.join("files").join(stem),
            _ => id,
        }
    }

    fn entry(item: &trash::TrashItem) -> TrashEntry {
        let stored = stored_path(item);
        let meta = std::fs::symlink_metadata(&stored).ok();
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
        let size = if is_dir {
            walkdir::WalkDir::new(&stored)
                .follow_links(false)
                .into_iter()
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        } else {
            meta.map(|m| m.len()).unwrap_or(0)
        };
        TrashEntry {
            id: item.id.to_string_lossy().into_owned(),
            name: item.name.to_string_lossy().into_owned(),
            original_parent: item.original_parent.to_string_lossy().into_owned(),
            deleted: item.time_deleted,
            is_dir,
            size,
        }
    }

    pub fn list() -> Result<Vec<TrashEntry>> {
        let items = with_trash(os_limited::list)?;
        let mut out: Vec<TrashEntry> = items.iter().map(entry).collect();
        out.sort_by(|a, b| b.deleted.cmp(&a.deleted).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    /// The trash items with these ids (still in the trash).
    fn pick(ids: &[String]) -> Result<Vec<trash::TrashItem>> {
        let items = with_trash(os_limited::list)?;
        let picked: Vec<_> = items
            .into_iter()
            .filter(|i| ids.iter().any(|id| *id == i.id.to_string_lossy()))
            .collect();
        if picked.is_empty() && !ids.is_empty() {
            return Err(CoreError::Trash(
                "the items are no longer in the trash".into(),
            ));
        }
        Ok(picked)
    }

    pub fn restore(ids: &[String]) -> Result<()> {
        let items = pick(ids)?;
        // Say which item could not go back, not the crate's debug text.
        let r = std::thread::Builder::new()
            .name("trash".into())
            .spawn(move || os_limited::restore_all(items))
            .map_err(|e| CoreError::Trash(e.to_string()))?
            .join()
            .map_err(|_| CoreError::Trash("restoring failed".into()))?;
        match r {
            Ok(()) => Ok(()),
            Err(trash::Error::RestoreCollision { path, .. }) => Err(CoreError::Trash(format!(
                "“{}” already exists in {}; rename or move it first",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path.parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ))),
            Err(trash::Error::RestoreTwins { path, .. }) => Err(CoreError::Trash(format!(
                "several selected items would go back to {}; restore them one at a time",
                path.display()
            ))),
            Err(e) => Err(CoreError::Trash(e.to_string())),
        }
    }

    pub fn purge(ids: &[String]) -> Result<()> {
        let items = pick(ids)?;
        with_trash(move || os_limited::purge_all(items))
    }

    pub fn empty() -> Result<()> {
        with_trash(|| os_limited::list().and_then(os_limited::purge_all))
    }
}

#[cfg(not(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
)))]
mod imp {
    use super::*;
    fn unsupported<T>() -> Result<T> {
        Err(CoreError::Trash(
            "the trash can't be listed on this system".into(),
        ))
    }
    pub fn list() -> Result<Vec<TrashEntry>> {
        unsupported()
    }
    pub fn restore(_ids: &[String]) -> Result<()> {
        unsupported()
    }
    pub fn purge(_ids: &[String]) -> Result<()> {
        unsupported()
    }
    pub fn empty() -> Result<()> {
        unsupported()
    }
}

/// Everything in the trash, most recently deleted first.
pub fn list() -> Result<Vec<TrashEntry>> {
    imp::list()
}

/// Puts the items back where they were deleted from.
pub fn restore(ids: &[String]) -> Result<()> {
    imp::restore(ids)
}

/// Deletes the items for good.
pub fn purge(ids: &[String]) -> Result<()> {
    imp::purge(ids)
}

/// Deletes everything in the trash for good.
pub fn empty() -> Result<()> {
    imp::empty()
}

#[cfg(test)]
mod tests {
    /// Trash a file, find it, restore it, trash it again and delete it for good.
    /// Uses the real trash, so it is ignored by default:
    /// `cargo test -p gall-core trash_round_trip -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn trash_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir
            .path()
            .join(format!("gall-trash-test-{}.txt", std::process::id()));
        std::fs::write(&f, "hello").unwrap();
        trash::delete(&f).unwrap();
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let find = || super::list().unwrap().into_iter().find(|e| e.name == name);
        let e = find().expect("in the trash");
        println!("{e:?}");
        assert_eq!(e.size, 5);
        assert!(!e.is_dir);
        super::restore(std::slice::from_ref(&e.id)).unwrap();
        assert!(f.is_file(), "restored");
        trash::delete(&f).unwrap();
        let e = find().expect("in the trash again");
        super::purge(&[e.id]).unwrap();
        assert!(find().is_none(), "deleted for good");
    }
}
