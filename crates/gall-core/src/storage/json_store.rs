//! Crash-safe JSON persistence (C-02).
//!
//! Save: serialise → write `<name>.tmp` → fsync → copy the previous good file
//! to `<name>.backup.json` → atomic rename over the original.
//! Load: original → backup → `Default`, logging which one was used.

use crate::error::{IoContext, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct JsonStore {
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadedFrom {
    Primary,
    Backup,
    Default,
}

impl JsonStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn backup_path(&self) -> PathBuf {
        let stem = self.path.file_stem().unwrap_or_default().to_string_lossy();
        self.path.with_file_name(format!("{stem}.backup.json"))
    }

    fn read<T: DeserializeOwned>(p: &Path) -> std::result::Result<Option<T>, serde_json::Error> {
        let Ok(contents) = std::fs::read(p) else {
            return Ok(None);
        };
        serde_json::from_slice(&contents)
            .map(Some)
            .inspect_err(|e| {
                log::info!("corrupt json {}: {e}", p.display());
            })
    }

    pub fn load<T: DeserializeOwned + Default>(&self) -> (T, LoadedFrom) {
        let primary = Self::read(&self.path);
        if let Ok(Some(v)) = primary {
            return (v, LoadedFrom::Primary);
        }
        let name = self
            .path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy();
        let backup = Self::read(&self.backup_path());
        if let Ok(Some(v)) = backup {
            log::warn!("Restored “{name}” from backup.");
            return (v, LoadedFrom::Backup);
        }
        if primary.is_err() || backup.is_err() {
            log::warn!("Could not read “{name}”. Using defaults.");
        }
        (T::default(), LoadedFrom::Default)
    }

    pub fn save<T: Serialize>(&self, value: &T) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).ctx("create folder", dir)?;
        }
        let data = serde_json::to_vec_pretty(value)?;
        let tmp = self.path.with_extension("tmp");
        {
            let mut f = std::fs::File::create(&tmp).ctx("write", &tmp)?;
            f.write_all(&data).ctx("write", &tmp)?;
            f.sync_all().ctx("sync", &tmp)?;
        }
        // Keep the last known-good version as backup (only if it parses).
        if matches!(Self::read::<serde_json::Value>(&self.path), Ok(Some(_))) {
            let _ = std::fs::copy(&self.path, self.backup_path());
        }
        std::fs::rename(&tmp, &self.path).ctx("replace", &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn roundtrip_and_backup_recovery() {
        let d = tempfile::tempdir().unwrap();
        let s = JsonStore::new(d.path().join("settings.json"));
        let (v, from): (BTreeMap<String, i32>, _) = s.load();
        assert!(v.is_empty() && from == LoadedFrom::Default);

        let mut m = BTreeMap::new();
        m.insert("a".to_string(), 1);
        s.save(&m).unwrap();
        m.insert("a".to_string(), 2);
        s.save(&m).unwrap(); // backup now holds a=1

        std::fs::write(s.path(), "{ corrupt").unwrap();
        let (v, from): (BTreeMap<String, i32>, _) = s.load();
        assert_eq!(from, LoadedFrom::Backup);
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn session_store_migrates_legacy_and_recovers_backup() {
        use crate::nav::{Session, StoredSession};

        let d = tempfile::tempdir().unwrap();
        let store = JsonStore::new(d.path().join("session.json"));
        let mut legacy = Session::new("/left", "/right");
        legacy.navigate(0, "/left/sub");
        store.save(&legacy).unwrap();

        let (loaded, from): (Option<StoredSession>, _) = store.load();
        assert_eq!(from, LoadedFrom::Primary);
        let mut ws = loaded.unwrap().into_store("/home");
        assert_eq!(ws.workspaces.len(), 1);
        assert_eq!(ws.active_session().active_tab(0).path, "/left/sub");

        ws.create("/two-l", "/two-r", Some("Docs".into()), None, true);
        store.save(&ws).unwrap();

        // Corrupt primary: recovers previous valid file from backup.
        std::fs::write(store.path(), "{ broken").unwrap();
        let (recovered, from): (Option<StoredSession>, _) = store.load();
        assert_eq!(from, LoadedFrom::Backup);
        assert_eq!(
            recovered
                .unwrap()
                .into_store("/home")
                .active_session()
                .active_tab(0)
                .path,
            "/left/sub"
        );
    }
}
