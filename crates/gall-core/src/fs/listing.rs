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
    // First the names, which the folder gives in one pass; entries that the options
    // drop by name or type alone never get further.
    let mut items = Vec::new();
    for item in rd {
        let item = match item {
            Ok(i) => i,
            Err(e) => {
                log::warn!("skipping entry in {}: {e}", dir.display());
                continue;
            }
        };
        // Folders only (the Miller columns): a plain file is skipped before anything is
        // read about it. The type comes with the name on every platform; a link stays,
        // because only its target tells whether it behaves like a folder.
        if opts.dirs_only {
            if let Ok(t) = item.file_type() {
                if !t.is_dir() && !t.is_symlink() {
                    continue;
                }
            }
        }
        // A name that is hidden by itself (a leading dot) is dropped before anything is
        // read about it: a home folder holds hundreds of them, and each would cost a stat
        // (a round trip of its own on a network share or inside WSL).
        if !opts.show_hidden && super::entry::is_hidden_name(&item.file_name().to_string_lossy()) {
            continue;
        }
        items.push(item);
    }
    // Then what has to be read per entry (on Unix a stat each, for links and Windows
    // junctions their target too). In a folder of tens of thousands of entries that is
    // most of the time, and the entries do not depend on each other: the work is shared
    // between threads, and the order of the folder is kept.
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(MAX_WORKERS)
        .min(items.len() / PER_WORKER)
        .max(1);
    let chunk = items.len().div_ceil(workers).max(1);
    let parts: Vec<Vec<FileEntry>> = if workers == 1 {
        vec![read_entries(dir, &items, opts)]
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = items
                .chunks(chunk)
                .map(|part| scope.spawn(move || read_entries(dir, part, opts)))
                .collect();
            handles
                .into_iter()
                // A listing with a part missing would look complete: a panic in a
                // worker goes on to the caller, as it did on one thread.
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                })
                .collect()
        })
    };
    Ok(parts.into_iter().flatten().collect())
}

/// Threads that read entries of one folder at most, and how many entries make one
/// worth starting.
const MAX_WORKERS: usize = 8;
const PER_WORKER: usize = 512;

/// The entries of `items` (of the folder `dir`) that the options keep.
fn read_entries(dir: &Path, items: &[std::fs::DirEntry], opts: ListOptions) -> Vec<FileEntry> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let path = item.path();
        // The metadata of the entry itself, links not followed. On Windows the folder
        // listing already carries it, so nothing more is asked per item (asking by path
        // opens every file again and made large folders slow). If that fails, ask by path;
        // inside WSL a name may end with a dot, which the plain path would lose.
        let meta = match item
            .metadata()
            .or_else(|_| std::fs::symlink_metadata(super::wsl::io_path(&path)))
        {
            Ok(m) => m,
            Err(e) => {
                log::warn!("cannot stat {} in {}: {e}", path.display(), dir.display());
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
    out
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

    /// A large folder is read on several threads: the entries and their order have to
    /// be the ones a single thread gives, with every combination of the options.
    #[test]
    fn large_folder_lists_as_on_one_thread() {
        let d = tempfile::tempdir().unwrap();
        for n in 0..3000 {
            match n % 10 {
                0 => fs::create_dir(d.path().join(format!("folder {n}"))).unwrap(),
                1 => fs::write(d.path().join(format!(".hidden {n}")), "x").unwrap(),
                2 => fs::write(d.path().join(format!("draft {n}.tmp")), "x").unwrap(),
                _ => fs::write(d.path().join(format!("file {n}.txt")), vec![1u8; n % 7]).unwrap(),
            }
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path().join("folder 0"), d.path().join("to folder"))
                .unwrap();
            std::os::unix::fs::symlink(d.path().join("file 3.txt"), d.path().join("to file"))
                .unwrap();
        }
        let key = |v: Vec<FileEntry>| -> Vec<_> {
            v.into_iter()
                .map(|e| (e.name, e.is_dir, e.is_hidden, e.size))
                .collect()
        };
        for bits in 0..8 {
            let opts = ListOptions {
                show_hidden: bits & 1 != 0,
                hide_temp: bits & 2 != 0,
                dirs_only: bits & 4 != 0,
            };
            let items: Vec<_> = fs::read_dir(d.path()).unwrap().flatten().collect();
            assert!(items.len() / PER_WORKER >= 2, "the folder is read in parts");
            let plain = key(read_entries(d.path(), &items, opts));
            let listed = key(list_dir(d.path(), opts).unwrap());
            assert!(!listed.is_empty());
            assert_eq!(listed, plain, "{opts:?}");
        }
    }

    #[test]
    fn listed_metadata_matches_metadata_asked_by_path() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("data.bin"), vec![7u8; 4321]).unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();
        let mut listed = list_dir(d.path(), ListOptions::default()).unwrap();
        listed.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(listed.len(), 2);
        for e in &listed {
            let by_path = FileEntry::from_path(Path::new(&e.path)).unwrap();
            assert_eq!(e.name, by_path.name);
            assert_eq!(e.is_dir, by_path.is_dir);
            assert_eq!(e.is_symlink, by_path.is_symlink);
            assert_eq!(e.size, by_path.size);
            assert_eq!(e.modified, by_path.modified);
            assert_eq!(e.readonly, by_path.readonly);
            assert_eq!(e.is_hidden, by_path.is_hidden);
            assert_eq!(e.attributes, by_path.attributes);
        }
        assert_eq!(listed[0].size, 4321);
        assert!(listed[1].is_dir);
    }

    #[cfg(unix)]
    #[test]
    fn dirs_only_keeps_links_to_folders_and_drops_links_to_files() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("file.txt"), "x").unwrap();
        fs::create_dir(d.path().join("real")).unwrap();
        std::os::unix::fs::symlink(d.path().join("real"), d.path().join("to-dir")).unwrap();
        std::os::unix::fs::symlink(d.path().join("file.txt"), d.path().join("to-file")).unwrap();
        std::os::unix::fs::symlink(d.path().join("gone"), d.path().join("broken")).unwrap();
        let mut names: Vec<_> = list_dir(
            d.path(),
            ListOptions {
                dirs_only: true,
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
        names.sort();
        assert_eq!(names, ["real", "to-dir"]);
    }

    #[test]
    fn missing_dir_is_error() {
        assert!(list_dir(Path::new("/definitely/not/here"), ListOptions::default()).is_err());
    }
}
