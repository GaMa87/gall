use super::entry::FileEntry;
use crate::error::{IoContext, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListOptions {
    pub show_hidden: bool,
    /// Hide common temporary/backup files (`~` suffix, `.tmp`, `.swp`…). (F-08)
    pub hide_temp: bool,
    pub dirs_only: bool,
}

pub fn is_temp_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with('~')
        || lower.starts_with("~$")
        || [
            ".tmp",
            ".temp",
            ".swp",
            ".swo",
            ".bak",
            ".part",
            ".crdownload",
        ]
        .iter()
        .any(|s| lower.ends_with(s))
}

/// Lists a directory. Unreadable individual entries are skipped (logged) so a
/// single broken item never hides the whole folder.
pub fn list_dir(dir: &Path, opts: ListOptions) -> Result<Vec<FileEntry>> {
    // Inside an archive (photos.zip\2024): list its index instead.
    if let Some((archive, inner)) = super::archive_view::split(dir) {
        let mut v = super::archive_view::list(&archive, &inner)?;
        v.retain(|e| {
            (opts.show_hidden || !e.is_hidden)
                && !(opts.hide_temp && !e.is_dir && is_temp_name(&e.name))
                && !(opts.dirs_only && !e.is_dir)
        });
        return Ok(v);
    }
    let rd = std::fs::read_dir(dir).ctx("list", dir)?;
    let mut out = Vec::new();
    for item in rd {
        let item = match item {
            Ok(i) => i,
            Err(e) => {
                log::warn!("skipping entry in {}: {e}", dir.display());
                continue;
            }
        };
        let path = item.path();
        // Inside WSL a name may end with a dot, which the plain path would lose.
        let meta = match std::fs::symlink_metadata(super::wsl::io_path(&path)) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("cannot stat {}: {e}", path.display());
                continue;
            }
        };
        let entry = FileEntry::from_meta(&path, &meta);
        if !opts.show_hidden && entry.is_hidden {
            continue;
        }
        if opts.hide_temp && !entry.is_dir && is_temp_name(&entry.name) {
            continue;
        }
        if opts.dirs_only && !entry.is_dir {
            continue;
        }
        out.push(entry);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn hidden_and_temp_filtering() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.txt"), "x").unwrap();
        fs::write(d.path().join(".hidden"), "x").unwrap();
        fs::write(d.path().join("b.txt~"), "x").unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();

        let names = |o| {
            let mut v: Vec<_> = list_dir(d.path(), o)
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect();
            v.sort();
            v
        };
        assert_eq!(names(ListOptions::default()), ["a.txt", "b.txt~", "sub"]);
        assert_eq!(
            names(ListOptions {
                show_hidden: true,
                ..Default::default()
            }),
            [".hidden", "a.txt", "b.txt~", "sub"]
        );
        assert_eq!(
            names(ListOptions {
                hide_temp: true,
                ..Default::default()
            }),
            ["a.txt", "sub"]
        );
        assert_eq!(
            names(ListOptions {
                dirs_only: true,
                ..Default::default()
            }),
            ["sub"]
        );
    }

    #[test]
    fn missing_dir_is_error() {
        assert!(list_dir(Path::new("/definitely/not/here"), ListOptions::default()).is_err());
    }
}
