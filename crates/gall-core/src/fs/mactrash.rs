//! The macOS trash. The `trash` crate can move items to the Trash on macOS but can
//! neither list nor restore them, and `~/.Trash` itself cannot be listed without Full
//! Disk Access. Gall therefore trashes through `NSFileManager.trashItemAtURL`, which
//! reports the exact path the item received, and remembers each item (original path,
//! trash path, time) in an index. Undo, the Trash window and Restore work from that
//! index, by path, which needs no extra permission. Items other apps put in the Trash
//! are not listed.

use crate::error::{CoreError, Result};
use objc2_foundation::{NSFileManager, NSString, NSURL};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub original: PathBuf,
    pub trashed: PathBuf,
    /// Unix milliseconds.
    pub time_ms: i64,
}

struct Index {
    items: Vec<Item>,
    loaded: bool,
}

static INDEX: Mutex<Index> = Mutex::new(Index {
    items: Vec::new(),
    loaded: false,
});
static INDEX_FILE: OnceLock<PathBuf> = OnceLock::new();

/// Where the index is kept (the app's data folder). Without it the index lives in memory
/// only, which is what unit tests use.
pub fn set_index_file(path: PathBuf) {
    if INDEX_FILE.set(path).is_err() {
        return;
    }
    // Items trashed before the path was known stay; the saved ones are added to them.
    let early = {
        let mut g = INDEX.lock().unwrap_or_else(|p| p.into_inner());
        g.loaded = false;
        std::mem::take(&mut g.items)
    };
    let mut g = lock();
    for it in early {
        if !g.items.contains(&it) {
            g.items.push(it);
        }
    }
    save(&g);
}

fn lock() -> std::sync::MutexGuard<'static, Index> {
    let mut g = INDEX.lock().unwrap_or_else(|p| p.into_inner());
    if !g.loaded {
        g.loaded = true;
        if let Some(f) = INDEX_FILE.get() {
            if let Some(items) = std::fs::read(f)
                .ok()
                .and_then(|b| serde_json::from_slice::<Vec<Item>>(&b).ok())
            {
                g.items = items;
            }
        }
    }
    g
}

fn save(g: &Index) {
    let Some(f) = INDEX_FILE.get() else { return };
    let r = serde_json::to_vec(&g.items)
        .map_err(|e| e.to_string())
        .and_then(|b| {
            let tmp = f.with_extension("tmp");
            std::fs::write(&tmp, b)
                .and_then(|_| std::fs::rename(&tmp, f))
                .map_err(|e| e.to_string())
        });
    if let Err(e) = r {
        log::warn!("could not save the trash index: {e}");
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// Returns `true` when `~/.Trash` can be listed directly (i.e. macOS Full Disk Access is
/// granted). Reading `~/.Trash` returns `PermissionDenied` immediately without triggering a
/// TCC permission prompt when Full Disk Access is absent.
pub fn trash_folder_accessible() -> bool {
    dirs::home_dir()
        .map(|h| h.join(".Trash"))
        .is_some_and(|p| std::fs::read_dir(p).is_ok())
}

/// Moves the items to the Trash and records where each one went.
pub fn trash(paths: &[PathBuf]) -> Result<()> {
    let mgr = NSFileManager::defaultManager();
    for p in paths {
        let Some(s) = p.to_str() else {
            return Err(CoreError::Trash(format!(
                "“{}” has a name that cannot be moved to the Trash",
                p.display()
            )));
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(s));
        let mut result = None;
        mgr.trashItemAtURL_resultingItemURL_error(&url, Some(&mut result))
            .map_err(|e| {
                CoreError::Trash(format!(
                    "“{}” cannot be moved to the Trash ({})",
                    p.display(),
                    e.localizedDescription()
                ))
            })?;
        // Normally the system says where the item went; if it does not, the usual place.
        let trashed = result
            .and_then(|u| u.path())
            .map(|s| PathBuf::from(s.to_string()))
            .or_else(|| {
                let guess = dirs::home_dir()?.join(".Trash").join(p.file_name()?);
                exists(&guess).then_some(guess)
            });
        if trashed.is_none() {
            log::warn!(
                "{} was trashed but its place in the Trash is unknown",
                p.display()
            );
        }
        if let Some(trashed) = trashed {
            let mut g = lock();
            g.items.retain(|i| i.trashed != trashed);
            g.items.push(Item {
                original: p.clone(),
                trashed,
                time_ms: now_ms(),
            });
            save(&g);
        }
    }
    Ok(())
}

/// Items still in the Trash, most recently deleted last. Items that were emptied or
/// restored by other means are dropped from the index, except while their Trash folder
/// cannot be reached (an unmounted volume's `.Trashes`): they may still be recoverable.
pub fn items() -> Vec<Item> {
    let mut g = lock();
    let before = g.items.len();
    g.items
        .retain(|i| exists(&i.trashed) || i.trashed.parent().is_some_and(|d| !exists(d)));
    if g.items.len() != before {
        save(&g);
    }
    g.items.clone()
}

fn forget(items: &[Item]) {
    let mut g = lock();
    g.items.retain(|i| !items.contains(i));
    save(&g);
}

/// Puts the items back where they were deleted from. Never overwrites: an item whose
/// place is taken stays in the Trash and is named in the error.
pub fn restore(items: Vec<Item>) -> Result<()> {
    let mut first_err = None;
    for it in items {
        let r = (|| {
            if exists(&it.original) {
                return Err(CoreError::Trash(format!(
                    "“{}” already exists in {}; rename or move it first",
                    it.original
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    it.original
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                )));
            }
            if let Some(parent) = it.original.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CoreError::Trash(format!("{}: {e}", parent.display())))?;
            }
            crate::fs::ops::rename_noreplace(&it.trashed, &it.original)
                .map_err(|e| CoreError::Trash(format!("{}: {e}", it.original.display())))
        })();
        match r {
            Ok(()) => forget(std::slice::from_ref(&it)),
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// Deletes the items for good.
pub fn purge(items: &[Item]) -> Result<()> {
    let mut first_err = None;
    for it in items {
        let r = match std::fs::symlink_metadata(&it.trashed) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&it.trashed),
            Ok(_) => std::fs::remove_file(&it.trashed),
            Err(_) => Ok(()),
        };
        match r {
            Ok(()) => forget(std::slice::from_ref(it)),
            Err(e) => {
                first_err.get_or_insert(CoreError::Trash(format!("{}: {e}", it.trashed.display())));
            }
        }
    }
    first_err.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_list_restore_and_purge_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("gall-mactrash-a.txt");
        let b = d.path().join("gall-mactrash-b.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        trash(&[a.clone(), b.clone()]).unwrap();
        assert!(!a.exists() && !b.exists());

        let mine: Vec<Item> = items()
            .into_iter()
            .filter(|i| i.original == a || i.original == b)
            .collect();
        assert_eq!(mine.len(), 2, "both trashed items are listed");
        assert!(mine.iter().all(|i| i.trashed.exists()));

        let (ia, ib) = (
            mine.iter().find(|i| i.original == a).unwrap().clone(),
            mine.iter().find(|i| i.original == b).unwrap().clone(),
        );
        restore(vec![ia.clone()]).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
        assert!(!items().contains(&ia), "a restored item leaves the index");

        // The place is taken again: restoring must not overwrite and keeps the item.
        std::fs::write(&b, "new").unwrap();
        assert!(restore(vec![ib.clone()]).is_err());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "new");
        assert!(ib.trashed.exists() && items().contains(&ib));

        purge(std::slice::from_ref(&ib)).unwrap();
        assert!(!ib.trashed.exists() && !items().contains(&ib));
    }
}
