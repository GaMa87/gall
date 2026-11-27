//! What is in the stashes: files and age of all of them for the list, and the changes of one for
//! its preview. A stash is named by its commit, so an answer never changes and can be kept.
use super::gitgraph::{git, root};
use super::gitinspect::{commit_id, read_git, PATCH_LIMIT};
use crate::{CoreError, Result};
use serde::Serialize;
use std::path::Path;

/// One stash as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashInfo {
    pub id: String,
    /// Seconds since 1970.
    pub time: i64,
    /// Changed tracked files. Untracked ones are in the preview.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashPreview {
    pub patch: String,
    pub truncated: bool,
}

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// Files and age of every stash, with one process for all of them.
pub fn list(path: &Path) -> Result<Vec<StashInfo>> {
    let root = root(path)?;
    let output = git(
        &root,
        &[
            "stash",
            "list",
            "--no-show-signature",
            "--format=%x1e%H%x00%ct%x00",
            "--name-only",
            "-z",
        ],
    )?;
    Ok(parse_list(&output))
}

fn parse_list(output: &str) -> Vec<StashInfo> {
    output
        .split('\x1e')
        .filter_map(|record| {
            let mut fields = record.split('\0');
            let id = fields.next()?.trim();
            if !matches!(id.len(), 40 | 64) {
                return None;
            }
            Some(StashInfo {
                id: id.to_owned(),
                time: fields.next()?.trim().parse().ok()?,
                // Git puts a line break between the format and the names.
                files: fields
                    .map(|name| name.trim_start_matches('\n'))
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

/// The changes of a stash, new files included (`git stash show -p`).
pub fn preview(path: &Path, id: &str) -> Result<StashPreview> {
    if !matches!(id.len(), 40 | 64) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(fail("A full Git commit ID is required"));
    }
    let root = root(path)?;
    let (bytes, truncated) = read_git(
        &root,
        &[
            "stash",
            "show",
            "-p",
            "--include-untracked",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "-M",
            id,
        ],
        PATCH_LIMIT,
    )?;
    Ok(StashPreview {
        patch: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
    })
}

/// The commit a stash was made on.
pub(super) fn base(root: &Path, id: &str) -> Result<String> {
    commit_id(root, id)?;
    Ok(git(root, &["rev-parse", "--verify", &format!("{id}^1")])?
        .trim()
        .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "Test"],
            &["config", "user.email", "test@example.com"],
            &["config", "core.autocrlf", "false"],
        ] {
            git(p, args).unwrap();
        }
        std::fs::write(p.join("a.txt"), "one\n").unwrap();
        std::fs::write(p.join("ž b.txt"), "one\n").unwrap();
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "one"]).unwrap();
        d
    }

    #[test]
    fn the_list_has_files_and_age_and_the_preview_new_files_too() {
        let d = repository();
        let p = d.path();
        assert!(list(p).unwrap().is_empty());
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        std::fs::write(p.join("ž b.txt"), "two\n").unwrap();
        std::fs::write(p.join("new.txt"), "new\n").unwrap();
        git(p, &["stash", "push", "-q", "-u", "-m", "first"]).unwrap();
        std::fs::write(p.join("a.txt"), "three\n").unwrap();
        git(p, &["stash", "push", "-q", "-m", "second"]).unwrap();
        let found = list(p).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].files, ["a.txt"]);
        assert_eq!(found[1].files, ["a.txt", "ž b.txt"]);
        assert!(found.iter().all(|s| s.time > 1_700_000_000));
        let ids: Vec<_> = git(p, &["stash", "list", "--format=%H"])
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(found.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), ids);
        let shown = preview(p, &found[1].id).unwrap();
        assert!(!shown.truncated);
        for part in ["+two", "new.txt", "+new"] {
            assert!(shown.patch.contains(part), "{part}");
        }
        // By its commit, so also when it is no stash of the list any more.
        git(p, &["stash", "drop", "-q", "stash@{1}"]).unwrap();
        assert_eq!(preview(p, &found[1].id).unwrap(), shown);
        assert!(preview(p, "stash@{0}").is_err());
        assert!(preview(p, "--output=x").is_err());
    }

    #[test]
    fn records_are_read_however_git_breaks_them() {
        let id = "a".repeat(40);
        let text = format!("\x1e{id}\05\0\na.txt\0b c.txt\0\x1e{id}\06\0");
        let found = parse_list(&text);
        assert_eq!(found[0].files, ["a.txt", "b c.txt"]);
        assert_eq!((found[1].time, found[1].files.len()), (6, 0));
    }
}
