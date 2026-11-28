//! Reads the tracked files of a Git repository straight from `.git/index`: one file read
//! instead of walking and `stat`ing thousands of files (Quick Open's zero-walk fast path).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where the index file of the repository at `root` lives (`.git` may be a `gitdir:` file in a
/// worktree or submodule).
fn index_file(root: &Path) -> Option<PathBuf> {
    let dot = root.join(".git");
    let meta = std::fs::metadata(&dot).ok()?;
    if meta.is_dir() {
        return Some(dot.join("index"));
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let dir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let dir = Path::new(dir);
    let dir = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        root.join(dir)
    };
    Some(dir.join("index"))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The paths (relative, `/` separated) stored in a Git index file's bytes (versions 2, 3 and 4).
/// `None` for anything unexpected: the caller then falls back to walking the folder.
pub fn parse_index(data: &[u8]) -> Option<Vec<String>> {
    if data.get(..4)? != b"DIRC" {
        return None;
    }
    let version = be32(data, 4)?;
    if !(2..=4).contains(&version) {
        return None;
    }
    let count = be32(data, 8)? as usize;
    let mut out = Vec::with_capacity(count.min(1_000_000));
    let mut pos = 12;
    let mut prev: Vec<u8> = Vec::new();
    for _ in 0..count {
        let start = pos;
        let flags = u16::from_be_bytes(data.get(pos + 60..pos + 62)?.try_into().ok()?);
        pos += 62;
        if version >= 3 && flags & 0x4000 != 0 {
            pos += 2;
        }
        let path: Vec<u8> = if version == 4 {
            // Prefix compression: how many bytes to cut from the previous path, then the new tail.
            let mut cut = 0usize;
            let mut first = true;
            for _ in 0..9 {
                let b = *data.get(pos)?;
                pos += 1;
                cut = if first {
                    (b & 0x7f) as usize
                } else {
                    ((cut + 1) << 7) | (b & 0x7f) as usize
                };
                first = false;
                if b & 0x80 == 0 {
                    break;
                }
            }
            let nul = data.get(pos..)?.iter().position(|&b| b == 0)?;
            let keep = prev.len().checked_sub(cut)?;
            let mut p = prev[..keep].to_vec();
            p.extend_from_slice(&data[pos..pos + nul]);
            pos += nul + 1;
            p
        } else {
            let nul = data.get(pos..)?.iter().position(|&b| b == 0)?;
            let p = data[pos..pos + nul].to_vec();
            // Entries are NUL padded to a multiple of 8 bytes, counted from the entry's start.
            let len = pos + nul - start;
            pos = start + (len + 8) / 8 * 8;
            p
        };
        out.push(String::from_utf8_lossy(&path).into_owned());
        prev = path;
    }
    Some(out)
}

/// Every tracked file of the repository at `root` plus the folders that contain them, as absolute
/// `(path, is_dir)` pairs, or `None` when `root` is no repository or its index cannot be read.
pub fn tracked_paths(root: &Path) -> Option<Vec<(PathBuf, bool)>> {
    let data = std::fs::read(index_file(root)?).ok()?;
    let files = parse_index(&data)?;
    let mut dirs: BTreeSet<&str> = BTreeSet::new();
    for f in &files {
        let mut cur = f.as_str();
        while let Some(i) = cur.rfind('/') {
            cur = &cur[..i];
            if !dirs.insert(cur) {
                break;
            }
        }
    }
    let mut out: Vec<(PathBuf, bool)> = dirs.into_iter().map(|d| (root.join(d), true)).collect();
    out.extend(files.iter().map(|f| (root.join(f), false)));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn rejects_foreign_data() {
        assert!(parse_index(b"").is_none());
        assert!(parse_index(b"NOPE\0\0\0\x02\0\0\0\0").is_none());
        assert!(parse_index(b"DIRC\0\0\0\x09\0\0\0\0").is_none());
    }

    #[test]
    fn matches_git_ls_files_for_every_index_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        if !git(root, &["init", "-q"]) {
            return; // git is not installed
        }
        std::fs::create_dir_all(root.join("src/deep/er")).unwrap();
        std::fs::write(root.join("src/deep/er/a.rs"), "").unwrap();
        std::fs::write(root.join("src/deep/b.rs"), "").unwrap();
        std::fs::write(root.join("src/c d.rs"), "").unwrap();
        std::fs::write(root.join("top.txt"), "").unwrap();
        assert!(git(root, &["add", "-A"]));
        for version in ["2", "4"] {
            assert!(git(root, &["update-index", "--index-version", version]));
            let out = Command::new("git")
                .current_dir(root)
                .args(["ls-files", "-z"])
                .output()
                .unwrap();
            let mut expected: Vec<String> = String::from_utf8(out.stdout)
                .unwrap()
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            expected.sort();
            let mut got = parse_index(&std::fs::read(root.join(".git/index")).unwrap()).unwrap();
            got.sort();
            assert_eq!(got, expected, "index version {version}");
        }
        let all = tracked_paths(root).unwrap();
        assert!(all.contains(&(root.join("src/deep"), true)));
        assert!(all.contains(&(root.join("src/deep/er/a.rs"), false)));
        assert!(!all.iter().any(|(p, _)| p.starts_with(root.join(".git"))));
    }

    #[test]
    fn a_folder_without_a_repository_has_no_tracked_paths() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(tracked_paths(tmp.path()).is_none());
    }
}
