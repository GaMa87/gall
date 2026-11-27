//! A branch that remembers its unfinished work (WN-49, opt-in).
//!
//! Switching with local changes puts them under `refs/gall/wip/<branch>` (the stash commit that `git stash push
//! --include-untracked` makes, kept only as a reference, never as an entry of the stash list) and the work tree is clean
//! for the other branch. Switching back applies them again, index included, when that works without conflicts; when it
//! does not, nothing is applied, the snapshot stays and the user can save it as an ordinary stash (`to_stash`) or discard
//! it. The snapshot is also kept under `refs/gall/recovery`, so no step loses the work.
use super::gitgraph::{git, push_recovery};
use crate::{CoreError, Result};
use serde::Serialize;
use std::path::Path;

const PREFIX: &str = "refs/gall/wip/";
const KEPT: &str = "refs/gall/wip-kept/";

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// A branch name as a single path part of a reference name: every byte but letters, digits, `_` and `-` as `%XX`, so
/// branches `a` and `a/b` cannot collide as a file and a folder, and no odd character reaches a reference name.
pub fn encode(branch: &str) -> String {
    let mut out = String::new();
    for byte in branch.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
/// The branch an encoded name stands for; None for a name that is not one of ours.
pub fn decode(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}
fn reference(branch: &str) -> String {
    format!("{PREFIX}{}", encode(branch))
}
fn existing(root: &Path, name: &str) -> Option<String> {
    git(root, &["rev-parse", "-q", "--verify", name])
        .ok()
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
}

/// The branch HEAD is on; None when detached.
pub fn current_branch(root: &Path) -> Option<String> {
    if let Some((_, _, branch)) = super::gitfacts::head(root) {
        return branch;
    }
    git(root, &["symbolic-ref", "-q", "--short", "HEAD"])
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

/// Takes the local changes away and remembers them for `branch`. An older snapshot of the same branch (one that could not be
/// restored) is moved to `refs/gall/wip-kept/…` first, never overwritten.
pub fn save(root: &Path, branch: &str) -> Result<()> {
    let Some(id) = push_recovery(root, &[], &format!("Gall WIP of {branch}"))? else {
        return Ok(());
    };
    let name = reference(branch);
    if let Some(old) = existing(root, &name) {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        git(
            root,
            &[
                "update-ref",
                &format!("{KEPT}{}/{seconds}", encode(branch)),
                &old,
            ],
        )?;
    }
    git(root, &["update-ref", &name, &id])?;
    Ok(())
}

/// What bringing the snapshot back did.
#[derive(Debug, PartialEq, Eq)]
pub enum Restored {
    Nothing,
    Applied,
    /// Not applied, the snapshot is kept; the reason is in words.
    Kept(String),
}

/// Applies the snapshot of `branch`, if there is one, to the clean work tree after a switch to it.
pub fn restore(root: &Path, branch: &str) -> Result<Restored> {
    let name = reference(branch);
    let Some(id) = existing(root, &name) else {
        return Ok(Restored::Nothing);
    };
    let clean = git(
        root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
    )?
    .is_empty();
    if !clean {
        return Ok(Restored::Kept(
            "the working folder has changes of its own".into(),
        ));
    }
    if let Some(reason) = conflict(root, &id)? {
        return Ok(Restored::Kept(reason));
    }
    match git(root, &["stash", "apply", "--index", "-q", &id]) {
        Ok(_) => {
            git(root, &["update-ref", "-d", &name])?;
            Ok(Restored::Applied)
        }
        Err(error) => {
            // The tree was clean, so what is there now comes from this apply: take it back, then keep the snapshot.
            let _ = git(root, &["reset", "-q", "--hard"]);
            for file in untracked_files(root, &id) {
                let _ = std::fs::remove_file(root.join(file));
            }
            Ok(Restored::Kept(format!("Git could not apply it: {error}")))
        }
    }
}

fn untracked_files(root: &Path, id: &str) -> Vec<String> {
    let third = format!("{id}^3");
    if existing(root, &third).is_none() {
        return Vec::new();
    }
    git(root, &["ls-tree", "-r", "--name-only", "-z", &third])
        .map(|names| {
            names
                .split('\0')
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Why the snapshot would not apply cleanly on top of HEAD, in words; None when it would. Nothing is changed:
/// a file the snapshot brings that already exists, or a merge of its tracked changes that conflicts (Git 2.38+;
/// anything the check cannot decide counts as a conflict, since the snapshot is kept then).
fn conflict(root: &Path, id: &str) -> Result<Option<String>> {
    for file in untracked_files(root, id) {
        if root.join(&file).symlink_metadata().is_ok() {
            return Ok(Some(format!("{file} exists on this branch")));
        }
    }
    let merge = git(
        root,
        &[
            "merge-tree",
            "--write-tree",
            "--no-messages",
            &format!("--merge-base={id}^1"),
            "HEAD",
            id,
        ],
    );
    Ok(merge
        .is_err()
        .then(|| "its changes overlap what this branch changed".to_owned()))
}

/// A remembered snapshot, for the badge on the branch.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Wip {
    pub branch: String,
    pub id: String,
    /// Unix seconds.
    pub created: i64,
}

/// The remembered snapshots, one process (`for-each-ref`) that the caller runs lazily, after the first paint.
pub fn list(path: &Path) -> Result<Vec<Wip>> {
    let root = super::gitgraph::root(path)?;
    let raw = git(
        &root,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(creatordate:unix)",
            PREFIX,
        ],
    )?;
    let mut found: Vec<Wip> = raw
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\0');
            let (name, id, created) = (parts.next()?, parts.next()?, parts.next()?);
            Some(Wip {
                branch: decode(name.strip_prefix(PREFIX)?)?,
                id: id.to_owned(),
                created: created.trim().parse().unwrap_or(0),
            })
        })
        .collect();
    found.sort_by(|a, b| a.branch.cmp(&b.branch));
    Ok(found)
}

/// Forgets the snapshot of `branch`; the hidden recovery reference made when it was taken stays.
pub fn discard(path: &Path, branch: &str) -> Result<()> {
    let _busy = super::gitfacts::mutating();
    let root = super::gitgraph::root(path)?;
    let name = reference(branch);
    existing(&root, &name).ok_or_else(|| fail("This branch has no remembered changes"))?;
    git(&root, &["update-ref", "-d", &name])?;
    Ok(())
}

/// Turns the snapshot of `branch` into an ordinary stash entry (to apply and resolve by hand) and forgets it as a snapshot.
pub fn to_stash(path: &Path, branch: &str) -> Result<()> {
    let _busy = super::gitfacts::mutating();
    let root = super::gitgraph::root(path)?;
    let name = reference(branch);
    let id = existing(&root, &name).ok_or_else(|| fail("This branch has no remembered changes"))?;
    git(
        &root,
        &[
            "stash",
            "store",
            "-m",
            &format!("Gall WIP of {branch}"),
            &id,
        ],
    )?;
    git(&root, &["update-ref", "-d", &name])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "T"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "core.autocrlf", "false"],
        ] {
            git(root, &args).unwrap();
        }
        fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        fs::write(root.join("b.txt"), "b\n").unwrap();
        git(root, &["add", "."]).unwrap();
        git(root, &["commit", "-qm", "initial"]).unwrap();
        git(root, &["branch", "other"]).unwrap();
        temp
    }
    fn read(root: &Path, file: &str) -> String {
        fs::read_to_string(root.join(file)).unwrap()
    }
    fn clean(root: &Path) -> bool {
        git(root, &["status", "--porcelain", "--untracked-files=all"])
            .unwrap()
            .is_empty()
    }

    #[test]
    fn branch_names_encode_to_one_safe_part_and_back() {
        for name in [
            "main",
            "feature/x",
            "a",
            "a/b",
            "ünï/çode",
            "with space",
            "dots..and.lock",
            "100%",
            "x%2Fy",
        ] {
            let encoded = encode(name);
            assert!(
                !encoded.contains('/') && !encoded.contains(' ') && !encoded.contains('.'),
                "{encoded}"
            );
            assert_eq!(decode(&encoded).as_deref(), Some(name));
        }
        assert_ne!(encode("a/b"), encode("a%2Fb"));
        assert_eq!(decode("%zz"), None);
        assert_eq!(decode("%4"), None);
    }

    #[test]
    fn changes_go_with_the_branch_and_come_back_exactly_with_the_index() {
        let temp = repo();
        let root = temp.path();
        fs::write(root.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        fs::write(root.join("b.txt"), "b\nstaged\n").unwrap();
        git(root, &["add", "b.txt"]).unwrap();
        fs::write(root.join("new.txt"), "untracked\n").unwrap();
        crate::fs::gitgraph::switch(root, "other", true, "wip", "").unwrap();
        // The other branch starts clean and the snapshot is remembered, not listed as a stash.
        assert!(clean(root));
        assert_eq!(read(root, "a.txt"), "one\ntwo\nthree\n");
        assert!(!root.join("new.txt").exists());
        let listed = list(root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].branch, "main");
        assert!(git(root, &["stash", "list"]).unwrap().is_empty());
        // Back on main: everything is as it was, the staged part still staged, and nothing is remembered any more.
        crate::fs::gitgraph::switch(root, "main", true, "wip", "").unwrap();
        assert_eq!(read(root, "a.txt"), "one\nTWO\nthree\n");
        assert_eq!(read(root, "new.txt"), "untracked\n");
        assert!(
            git(root, &["diff", "--cached", "--name-only"])
                .unwrap()
                .trim()
                == "b.txt"
        );
        assert!(list(root).unwrap().is_empty());
        // The hidden recovery reference of the snapshot is still there.
        assert!(git(root, &["for-each-ref", "refs/gall/recovery"])
            .unwrap()
            .contains("recovery"));
    }

    #[test]
    fn a_clean_switch_changes_nothing_and_without_the_action_nothing_is_remembered() {
        let temp = repo();
        let root = temp.path();
        crate::fs::gitgraph::switch(root, "other", true, "wip", "").unwrap();
        assert!(list(root).unwrap().is_empty());
        fs::write(root.join("a.txt"), "dirty\n").unwrap();
        // Git decides (Golden rules for Git modules): a file that both branches hold the same is carried along, so the
        // switch is allowed and the change is kept, as in lazygit. Nothing is remembered without the wip action.
        crate::fs::gitgraph::switch(root, "main", true, "clean", "").unwrap();
        assert_eq!(read(root, "a.txt"), "dirty\n");
        assert!(list(root).unwrap().is_empty());
        // A detached HEAD has no branch to remember them for.
        git(root, &["checkout", "-q", "--detach"]).unwrap();
        let main_id = git(root, &["rev-parse", "main"]).unwrap();
        assert!(crate::fs::gitgraph::switch(root, "main", true, "wip", "").is_err());
        assert_eq!(read(root, "a.txt"), "dirty\n");
        let _ = main_id;
    }

    #[test]
    fn a_conflicting_restore_applies_nothing_and_keeps_the_snapshot_until_it_is_stashed_or_discarded(
    ) {
        let temp = repo();
        let root = temp.path();
        // On `other` the same line is changed in a commit, and main leaves work on that line behind.
        git(root, &["checkout", "-q", "other"]).unwrap();
        fs::write(root.join("a.txt"), "one\nOTHER\nthree\n").unwrap();
        git(root, &["commit", "-qam", "other changes two"]).unwrap();
        git(root, &["checkout", "-q", "main"]).unwrap();
        fs::write(root.join("a.txt"), "one\nMINE\nthree\n").unwrap();
        fs::write(root.join("mine.txt"), "mine\n").unwrap();
        crate::fs::gitgraph::switch(root, "other", true, "wip", "").unwrap();
        // Now `other` leaves work on the same line, and main gets a commit that conflicts with what it left.
        git(root, &["checkout", "-q", "main"]).unwrap();
        assert!(list(root).unwrap().iter().any(|wip| wip.branch == "main"));
        // Restoring main's snapshot onto a main that changed that line the same way must not apply it halfway.
        fs::write(root.join("a.txt"), "one\nCOMMITTED\nthree\n").unwrap();
        git(root, &["commit", "-qam", "main changes two"]).unwrap();
        git(root, &["checkout", "-q", "other"]).unwrap();
        crate::fs::gitgraph::switch(root, "main", true, "wip", "").unwrap();
        assert!(
            clean(root),
            "{}",
            git(root, &["status", "--porcelain"]).unwrap()
        );
        assert_eq!(read(root, "a.txt"), "one\nCOMMITTED\nthree\n");
        assert!(!root.join("mine.txt").exists());
        assert_eq!(
            list(root)
                .unwrap()
                .iter()
                .filter(|wip| wip.branch == "main")
                .count(),
            1
        );
        // Saved as a stash: the entry exists and the snapshot is gone.
        to_stash(root, "main").unwrap();
        assert!(git(root, &["stash", "list"])
            .unwrap()
            .contains("Gall WIP of main"));
        assert!(list(root).unwrap().is_empty());
        assert!(to_stash(root, "main").is_err());
        assert!(discard(root, "main").is_err());
    }

    #[test]
    fn an_untracked_file_in_the_way_keeps_the_snapshot_and_discarding_forgets_only_the_reference() {
        let temp = repo();
        let root = temp.path();
        fs::write(root.join("note.txt"), "from main\n").unwrap();
        crate::fs::gitgraph::switch(root, "other", true, "wip", "").unwrap();
        // Main later gets a committed note.txt (plain Git, so nothing is restored on the way).
        git(root, &["checkout", "-q", "main"]).unwrap();
        fs::write(root.join("note.txt"), "committed on main\n").unwrap();
        git(root, &["add", "note.txt"]).unwrap();
        git(root, &["commit", "-qm", "note on main"]).unwrap();
        git(root, &["checkout", "-q", "other"]).unwrap();
        // The snapshot brings an untracked note.txt that main now tracks: nothing is applied, nothing is lost.
        crate::fs::gitgraph::switch(root, "main", true, "wip", "").unwrap();
        assert!(clean(root));
        assert_eq!(read(root, "note.txt"), "committed on main\n");
        assert_eq!(list(root).unwrap().len(), 1);
        discard(root, "main").unwrap();
        assert!(list(root).unwrap().is_empty());
        // Discarding left the hidden recovery reference of the snapshot.
        assert!(git(root, &["for-each-ref", "refs/gall/recovery"])
            .unwrap()
            .contains("recovery"));
        // Taking a second snapshot of the same branch keeps the older one under wip-kept.
        fs::write(root.join("a.txt"), "first\n").unwrap();
        crate::fs::gitgraph::switch(root, "other", true, "wip", "").unwrap();
        git(root, &["checkout", "-q", "main"]).unwrap();
        fs::write(root.join("a.txt"), "second\n").unwrap();
        crate::fs::gitgraph::switch(root, "other", true, "wip", "").unwrap();
        assert!(git(root, &["for-each-ref", "refs/gall/wip-kept"])
            .unwrap()
            .contains("refs/gall/wip-kept/main/"));
    }
}
