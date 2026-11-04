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

    fn read<T: DeserializeOwned>(p: &Path) -> Option<T> {
        let s = std::fs::read_to_string(p).ok()?;
        match serde_json::from_str(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                log::error!("corrupt json {}: {e}", p.display());
                None
            }
        }
    }

    pub fn load<T: DeserializeOwned + Default>(&self) -> (T, LoadedFrom) {
        if let Some(v) = Self::read(&self.path) {
            return (v, LoadedFrom::Primary);
        }
        if let Some(v) = Self::read(&self.backup_path()) {
            log::warn!("restored {} from backup", self.path.display());
            return (v, LoadedFrom::Backup);
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
        if Self::read::<serde_json::Value>(&self.path).is_some() {
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
}
