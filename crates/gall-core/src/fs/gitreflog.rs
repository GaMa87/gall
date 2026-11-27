//! The reflog as a list: where HEAD and the branches have been. Read from Git's log files, so it
//! starts no process; Git is asked only where there are no such files (reftable).
use super::gitadvanced::{common_dir, worktree_git_dir};
use super::gitgraph::{git, root};
use crate::Result;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// More than anyone reads; the newest are kept.
pub const MAX_ENTRIES: usize = 5_000;
/// A log file is read from its end; older entries of a huge one are left out.
const MAX_LOG: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// `HEAD` or the branch.
    pub reference: String,
    /// Its position in the log of that reference (`HEAD@{3}`).
    pub index: usize,
    /// Where it was before; empty for a new reference.
    pub old: String,
    pub id: String,
    /// Seconds since 1970.
    pub time: i64,
    /// `commit (amend)`, `rebase (finish)`, `checkout`…
    pub action: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reflog {
    /// Newest first.
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

fn zero(id: &str) -> bool {
    id.bytes().all(|b| b == b'0')
}

/// `<old> <new> <name> <mail> <time> <zone>\t<action>: <message>`
fn parse_line(reference: &str, line: &str) -> Option<Entry> {
    let (facts, text) = line.split_once('\t').unwrap_or((line, ""));
    let mut ids = facts.splitn(3, ' ');
    let (old, id, rest) = (ids.next()?, ids.next()?, ids.next()?);
    let whole = |id: &str| matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit());
    if !whole(old) || !whole(id) {
        return None;
    }
    // The name may hold anything, so the time is looked for from the end.
    let time = rest.rsplit(' ').nth(1)?.parse().ok()?;
    let (action, message) = match text.split_once(": ") {
        Some((action, message)) if !action.contains('\n') => (action, message),
        _ => ("", text),
    };
    Some(Entry {
        reference: reference.to_owned(),
        index: 0,
        old: if zero(old) {
            String::new()
        } else {
            old.to_owned()
        },
        id: id.to_owned(),
        time,
        action: action.to_owned(),
        message: message.trim_end().to_owned(),
    })
}

/// The entries of a log, newest first, and whether older ones were left out.
fn read_log(reference: &str, file: &Path) -> Option<(Vec<Entry>, bool)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut open = std::fs::File::open(file).ok()?;
    let size = open.metadata().ok()?.len();
    let cut = size > MAX_LOG;
    if cut {
        open.seek(SeekFrom::End(-(MAX_LOG as i64))).ok()?;
    }
    let mut bytes = Vec::new();
    open.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut entries: Vec<Entry> = text
        .lines()
        // The first line of a cut log may be the end of one.
        .skip(usize::from(cut))
        .filter_map(|line| parse_line(reference, line))
        .collect();
    entries.reverse();
    for (index, entry) in entries.iter_mut().enumerate() {
        entry.index = index;
    }
    Some((entries, cut))
}

/// The log files of the branches below `folder`, with the names of the branches.
fn logs(folder: &Path, prefix: &str, found: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let full = format!("{prefix}{name}");
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => logs(&entry.path(), &format!("{full}/"), found),
            Ok(kind) if kind.is_file() => found.push((full, entry.path())),
            _ => {}
        }
    }
}

/// Asks Git, for a repository without log files.
fn ask(root: &Path) -> Result<Vec<Entry>> {
    let count = format!("-{}", MAX_ENTRIES + 1);
    let output = git(
        root,
        &[
            "log",
            "--walk-reflogs",
            "--no-show-signature",
            &count,
            "--date=unix",
            "--format=%H%x00%gd%x00%gs%x1e",
            "HEAD",
        ],
    )?;
    let mut entries: Vec<Entry> = output
        .split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').split('\0');
            let (id, selector, text) = (fields.next()?, fields.next()?, fields.next()?);
            let time = selector
                .rsplit_once("@{")?
                .1
                .trim_end_matches('}')
                .parse()
                .ok()?;
            let (action, message) = text.split_once(": ").unwrap_or(("", text));
            Some(Entry {
                reference: "HEAD".into(),
                index: 0,
                old: String::new(),
                id: id.to_owned(),
                time,
                action: action.to_owned(),
                message: message.to_owned(),
            })
        })
        .collect();
    for index in 0..entries.len() {
        entries[index].index = index;
        if let Some(older) = entries.get(index + 1) {
            entries[index].old = older.id.clone();
        }
    }
    Ok(entries)
}

/// The reflogs of HEAD and of the local branches as one list, newest first. A step that HEAD and
/// its branch made together is one entry.
pub fn reflog(path: &Path) -> Result<Reflog> {
    let root = root(path)?;
    let head = worktree_git_dir(&root).map(|dir| dir.join("logs").join("HEAD"));
    let mut truncated = false;
    let mut entries = match head.as_deref().and_then(|file| read_log("HEAD", file)) {
        Some((entries, cut)) => {
            truncated |= cut;
            entries
        }
        None => ask(&root).unwrap_or_default(),
    };
    // A step of HEAD and of its branch has the same text in both logs. Without it a branch created in
    // the second of the first commit (empty `old`, same id and time) would be taken for that commit.
    let key = |entry: &Entry| {
        (
            entry.old.clone(),
            entry.id.clone(),
            entry.time,
            entry.action.clone(),
            entry.message.clone(),
        )
    };
    let mut known: HashSet<_> = entries.iter().map(key).collect();
    let mut files = Vec::new();
    if let Ok(common) = common_dir(&root) {
        logs(
            &common.join("logs").join("refs").join("heads"),
            "",
            &mut files,
        );
    }
    for (name, file) in files {
        let Some((own, cut)) = read_log(&name, &file) else {
            continue;
        };
        truncated |= cut;
        entries.extend(own.into_iter().filter(|entry| known.insert(key(entry))));
    }
    // Stable: entries of the same second stay in the order of their log.
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.time));
    truncated |= entries.len() > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);
    Ok(Reflog { entries, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "Ana > Test 12 <x>"],
            &["config", "user.email", "test@example.com"],
            &["config", "core.autocrlf", "false"],
        ] {
            git(d.path(), args).unwrap();
        }
        d
    }

    fn commit(p: &Path, text: &str) -> String {
        std::fs::write(p.join("f.txt"), text).unwrap();
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", &format!("add: {text}")]).unwrap();
        git(p, &["rev-parse", "HEAD"]).unwrap().trim().into()
    }

    #[test]
    fn entries_agree_with_git_and_lost_commits_are_in_it() {
        let d = repository();
        let p = d.path();
        let one = commit(p, "one");
        let two = commit(p, "two");
        git(p, &["reset", "-q", "--hard", &one]).unwrap();
        git(p, &["switch", "-qc", "feature/x"]).unwrap();
        let three = commit(p, "three");
        let log = reflog(p).unwrap();
        assert!(!log.truncated);
        let head: Vec<&Entry> = log
            .entries
            .iter()
            .filter(|entry| entry.reference == "HEAD")
            .collect();
        let expected = git(p, &["log", "-g", "--format=%H %gs", "HEAD"]).unwrap();
        // Entries of one second keep the order of the log.
        let mut by_index = head.clone();
        by_index.sort_by_key(|entry| entry.index);
        let found: Vec<String> = by_index
            .iter()
            .map(|entry| format!("{} {}: {}", entry.id, entry.action, entry.message))
            .collect();
        assert_eq!(found, expected.lines().collect::<Vec<_>>());
        // The commit that the reset took away, and what each step came from.
        assert!(head.iter().any(|entry| entry.id == two && entry.old == one));
        assert!(head
            .iter()
            .any(|entry| entry.id == three && entry.message == "add: three"));
        // A step of HEAD and its branch is listed once; the creation of the branch is its own.
        assert!(log
            .entries
            .iter()
            .any(|entry| entry.reference == "feature/x" && entry.action == "branch"));
        assert_eq!(
            log.entries.iter().filter(|entry| entry.id == three).count(),
            1
        );
        // From a linked worktree: its own HEAD, the branches of all.
        let other = d.path().join("linked");
        git(
            p,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &other.to_string_lossy(),
            ],
        )
        .unwrap();
        let linked = reflog(&other).unwrap();
        assert!(linked
            .entries
            .iter()
            .any(|entry| entry.reference == "feature/x"));
        assert!(linked
            .entries
            .iter()
            .filter(|entry| entry.reference == "HEAD")
            .all(|entry| entry.id == three));
    }

    #[test]
    fn lines_are_read_whatever_the_name_and_message_hold() {
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        let entry = parse_line(
            "HEAD",
            &format!("{a} {b} Ana 1 2 <m@x> 1700000000 +0100\tcommit (amend): fix: x\ty"),
        )
        .unwrap();
        assert_eq!(
            (entry.time, entry.action.as_str(), entry.message.as_str()),
            (1_700_000_000, "commit (amend)", "fix: x\ty")
        );
        let zero = "0".repeat(40);
        assert_eq!(
            parse_line("x", &format!("{zero} {b} A <m> 5 +0000"))
                .unwrap()
                .old,
            ""
        );
        assert!(parse_line("x", "nonsense").is_none());
        assert!(parse_line("x", &format!("{a} short A <m> 5 +0000\tx")).is_none());
    }

    #[test]
    fn an_empty_repository_has_an_empty_reflog() {
        let d = repository();
        assert_eq!(reflog(d.path()).unwrap().entries, []);
    }
}
