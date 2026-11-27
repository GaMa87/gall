//! The journal of history-changing operations, with undo and redo.
//!
//! Recording starts no Git process: where HEAD and its branch are is read from Git's files
//! before and after an operation, and the journal is a file in the Git directory of the work
//! tree. What an operation left behind is kept reachable by the references the operations
//! write anyway (`refs/gall/recovery`, and `refs/gall/journal` for what a rewrite replaced).
//!
//! Undo never forces: it refuses when HEAD or its branch is not where the operation left it,
//! commits and soft resets are taken back without touching files, and everything else goes
//! through `git reset --keep` or `git switch`, which refuse to overwrite local changes. Local
//! changes are saved as a snapshot first all the same.
use super::gitadvanced::{common_dir, worktree_git_dir};
use super::gitgraph::{git, root};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

const FILE: &str = "gall-journal.json";
/// Entries kept, and for how long: Git itself keeps unreachable reflog entries for 30 days.
const MAX_ENTRIES: usize = 100;
/// Pruning deletes references with one process, so it waits until several are due.
const PRUNE_SLACK: usize = 20;
const MAX_AGE_SECS: i64 = 28 * 24 * 60 * 60;

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// Where HEAD is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    /// `refs/heads/…`; None for a detached HEAD.
    pub reference: Option<String>,
    /// Empty before the first commit.
    pub id: String,
}

/// How an operation is taken back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Commit,
    Amend,
    Merge,
    Rebase,
    CherryPick,
    Revert,
    ResetSoft,
    ResetMixed,
    ResetHard,
    /// Reword, squash, fixup, drop, move and the interactive rebase.
    Rewrite,
    Checkout,
    Pull,
    /// Staged hunks put into their commits; undone, they are staged again.
    Absorb,
}

impl Kind {
    /// Only HEAD moves; the files and the index stay as they are.
    fn keeps_files(self) -> bool {
        matches!(
            self,
            Kind::Commit | Kind::Amend | Kind::ResetSoft | Kind::Absorb
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: u64,
    pub kind: Kind,
    /// What it was done with: a subject, a branch, a commit.
    pub label: String,
    /// Seconds since 1970.
    pub time: i64,
    pub before: Point,
    pub after: Point,
    /// A stash commit with the local changes the operation removed.
    pub snapshot: Option<String>,
}

/// An operation that stopped (a conflict): recorded once it is finished.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    kind: Kind,
    label: String,
    before: Point,
    snapshot: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Journal {
    /// Oldest first; those from `applied` on are undone.
    entries: Vec<Entry>,
    applied: usize,
    next: u64,
    pending: Option<Pending>,
}

/// The journal as the timeline shows it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    /// Oldest first.
    pub entries: Vec<Entry>,
    /// How many are applied; the rest can be redone.
    pub applied: usize,
    pub current: Option<Point>,
    /// Why the next undo, or the next redo, is not possible now.
    pub undo_blocked: Option<String>,
    pub redo_blocked: Option<String>,
}

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static SNAPSHOT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Remembers the snapshot the running operation saved (`retain_recovery` calls this).
pub(super) fn saved_snapshot(id: &str) {
    if DEPTH.get() > 0 {
        SNAPSHOT.with(|slot| *slot.borrow_mut() = Some(id.to_owned()));
    }
}

fn journal_path(root: &Path) -> Option<PathBuf> {
    Some(worktree_git_dir(root)?.join(FILE))
}

fn load(root: &Path) -> Journal {
    let mut journal: Journal = journal_path(root)
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    journal.applied = journal.applied.min(journal.entries.len());
    journal
}

/// Written beside and renamed, so a crash leaves the old journal.
fn store(root: &Path, journal: &Journal) -> Result<()> {
    let path = journal_path(root).ok_or_else(|| fail("The Git directory is not readable"))?;
    let bytes = serde_json::to_vec(journal).map_err(|e| fail(e.to_string()))?;
    let temporary = path.with_extension(format!("json.{}", std::process::id()));
    let written = std::fs::write(&temporary, bytes)
        .and_then(|_| std::fs::rename(&temporary, &path))
        .map_err(|e| fail(format!("Operation journal: {e}")));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

fn object_id(text: &str) -> Option<String> {
    let text = text.trim();
    (matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| text.to_owned())
}

/// Where HEAD is, from Git's files; Git is asked only where they cannot answer (reftable, a
/// symbolic branch).
fn point(root: &Path) -> Option<Point> {
    read_point(root).or_else(|| {
        let id = git(root, &["rev-parse", "--verify", "-q", "HEAD"]).unwrap_or_default();
        let reference = git(root, &["symbolic-ref", "-q", "HEAD"]).ok();
        Some(Point {
            reference: reference.map(|name| name.trim().to_owned()),
            id: id.trim().to_owned(),
        })
    })
}

fn read_point(root: &Path) -> Option<Point> {
    let git_dir = worktree_git_dir(root)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let Some(name) = head.trim().strip_prefix("ref: ") else {
        return Some(Point {
            reference: None,
            id: object_id(&head)?,
        });
    };
    // A reftable repository has the fixed placeholder here.
    if !name.starts_with("refs/heads/") || name == "refs/heads/.invalid" {
        return None;
    }
    // An ordinary repository has its references right there. Looking for a `commondir` that
    // does not exist costs about 10 ms through the file server of WSL.
    let common = if git_dir == root.join(".git") {
        git_dir
    } else {
        common_dir(root).ok()?
    };
    let id = match std::fs::read_to_string(common.join(name)) {
        Ok(text) => object_id(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let packed = std::fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
            packed
                .lines()
                .find_map(|line| line.strip_suffix(name)?.strip_suffix(' '))
                .map_or(Some(String::new()), object_id)?
        }
        Err(_) => return None,
    };
    Some(Point {
        reference: Some(name.to_owned()),
        id,
    })
}

/// Whether an operation waits to be finished.
fn in_progress(root: &Path) -> bool {
    const MARKERS: [&str; 6] = [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ];
    // One listing: looking up missing files is slow through WSL's file server.
    worktree_git_dir(root)
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .is_some_and(|entries| {
            entries
                .flatten()
                .any(|entry| MARKERS.iter().any(|name| entry.file_name() == *name))
        })
}

/// Runs an operation and records it when it moved HEAD or its branch. Never changes the result
/// of the operation. A nested call belongs to the outer operation.
pub(super) fn around<T>(
    path: &Path,
    operation: Option<(Kind, String)>,
    run: impl FnOnce() -> Result<T>,
) -> Result<T> {
    struct Nested;
    impl Drop for Nested {
        fn drop(&mut self) {
            DEPTH.set(DEPTH.get() - 1);
        }
    }
    let Some((kind, label)) = operation else {
        return run();
    };
    if DEPTH.get() > 0 {
        return run();
    }
    let Some((root, before)) = root(path)
        .ok()
        .and_then(|root| point(&root).map(|p| (root, p)))
    else {
        return run();
    };
    DEPTH.set(1);
    let nested = Nested;
    SNAPSHOT.with(|slot| slot.borrow_mut().take());
    let result = run();
    let snapshot = SNAPSHOT.with(|slot| slot.borrow_mut().take());
    drop(nested);
    let pending = Pending {
        kind,
        label,
        before,
        snapshot,
    };
    // A journal that cannot be written must not make a finished operation fail.
    let _ = record(&root, pending);
    result
}

/// Finishes or drops the operation that stopped: after continue, skip or abort.
pub(super) fn around_control<T>(path: &Path, run: impl FnOnce() -> Result<T>) -> Result<T> {
    let root = root(path)?;
    let result = run();
    let mut journal = load(&root);
    if let Some(pending) = journal.pending.clone() {
        if !in_progress(&root) {
            journal.pending = None;
            if point(&root).is_some_and(|now| now != pending.before) {
                let _ = record_in(&root, journal, pending);
            } else {
                let _ = store(&root, &journal);
            }
        }
    }
    result
}

fn record(root: &Path, operation: Pending) -> Result<()> {
    let mut journal = load(root);
    if in_progress(root) {
        // A step inside a stopped operation (amending at an edit stop) belongs to that one.
        if journal.pending.is_none() {
            journal.pending = Some(operation);
            return store(root, &journal);
        }
        return Ok(());
    }
    // What stopped before was finished or dropped without Gall.
    let stale = journal.pending.take().is_some();
    let moved = point(root).is_some_and(|now| now != operation.before);
    // A hard reset to the same commit still removes local changes.
    if !moved && operation.snapshot.is_none() {
        return if stale { store(root, &journal) } else { Ok(()) };
    }
    record_in(root, journal, operation)
}

fn record_in(root: &Path, mut journal: Journal, operation: Pending) -> Result<()> {
    let after = point(root).ok_or_else(|| fail("HEAD is not readable"))?;
    // A new operation ends what could be redone, as in an editor.
    journal.entries.truncate(journal.applied);
    journal.entries.push(Entry {
        id: journal.next,
        kind: operation.kind,
        label: operation.label,
        time: chrono::Utc::now().timestamp(),
        before: operation.before,
        after,
        snapshot: operation.snapshot,
    });
    journal.next += 1;
    journal.applied = journal.entries.len();
    let dropped = prune(&mut journal);
    store(root, &journal)?;
    release(root, &journal, &dropped);
    Ok(())
}

/// Takes out what is too old or too many, once enough has gathered; returns it.
fn prune(journal: &mut Journal) -> Vec<Entry> {
    let oldest = chrono::Utc::now().timestamp() - MAX_AGE_SECS;
    let expired = journal
        .entries
        .iter()
        .take_while(|e| e.time < oldest)
        .count();
    let surplus = journal.entries.len().saturating_sub(MAX_ENTRIES);
    if surplus < PRUNE_SLACK && expired < PRUNE_SLACK {
        return Vec::new();
    }
    let count = surplus.max(expired).min(journal.applied);
    journal.applied -= count;
    journal.entries.drain(..count).collect()
}

/// Deletes the journal references of entries that left, unless another entry needs them.
fn release(root: &Path, journal: &Journal, dropped: &[Entry]) {
    let ids = |entries: &[Entry]| -> std::collections::BTreeSet<String> {
        entries
            .iter()
            .flat_map(|e| [e.before.id.clone(), e.after.id.clone()])
            .filter(|id| !id.is_empty())
            .collect()
    };
    let needed = ids(&journal.entries);
    let lines: String = ids(dropped)
        .difference(&needed)
        .map(|id| format!("delete refs/gall/journal/{id}\n"))
        .collect();
    if !lines.is_empty() {
        let _ = update_refs(root, &lines);
    }
}

/// `git update-ref --stdin`: several references with one process, all or none.
fn update_refs(root: &Path, commands: &str) -> Result<()> {
    let mut command = super::gitexec::command(root, super::gitstatus::program());
    command
        .args(["-c", "core.hooksPath="])
        .arg("-C")
        .arg(super::gitexec::dir_arg(root))
        .args(["update-ref", "--no-deref", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    super::gitexec::forward_env(&mut command);
    let mut child = command.spawn().map_err(|e| fail(e.to_string()))?;
    let mut input = child.stdin.take().ok_or_else(|| fail("No Git input"))?;
    let written = input.write_all(commands.as_bytes());
    drop(input);
    let output = child.wait_with_output().map_err(|e| fail(e.to_string()))?;
    written.map_err(|e| fail(e.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(fail(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

/// Replaces the recovery reference of a finished rewrite by one of the journal, which is out of
/// every graph: the replaced commits stay for an undo.
pub(super) fn keep_for_undo(root: &Path, id: &str) {
    let _ = update_refs(
        root,
        &format!("update refs/gall/journal/{id} {id}\ndelete refs/gall/recovery/{id}\n"),
    );
}

/// Why `expected` is not where HEAD is, for the user.
fn moved(expected: &Point, now: &Point) -> Option<String> {
    let name = |p: &Point| {
        p.reference
            .as_deref()
            .map_or("a detached HEAD", |r| {
                r.strip_prefix("refs/heads/").unwrap_or(r)
            })
            .to_owned()
    };
    let short = |id: &str| id.chars().take(8).collect::<String>();
    if expected.reference != now.reference {
        return Some(format!(
            "The journal left {} checked out, now it is {}. Switch back first.",
            name(expected),
            name(now)
        ));
    }
    (expected.id != now.id).then(|| {
        format!(
            "{} moved outside Gall since then (from {} to {}). Undoing would lose that; use the reflog instead.",
            name(now),
            short(&expected.id),
            short(&now.id)
        )
    })
}

/// Why this entry cannot be undone (or redone) with HEAD at `now`.
fn blocked(root: &Path, entry: &Entry, redo: bool, now: &Point) -> Option<String> {
    let (expected, target) = if redo {
        (&entry.before, &entry.after)
    } else {
        (&entry.after, &entry.before)
    };
    if in_progress(root) {
        return Some("Finish or abort the current Git operation first.".into());
    }
    if target.id.is_empty() {
        return Some("The first commit of a repository cannot be undone here.".into());
    }
    // Switching leaves the branch where it is, so its commits cannot be lost; those of a detached
    // HEAD can.
    if entry.kind == Kind::Checkout
        && expected.reference.is_some()
        && expected.reference == now.reference
    {
        return None;
    }
    moved(expected, now)
}

/// The journal for the timeline. Starts no Git process.
pub fn view(path: &Path) -> Result<View> {
    let root = root(path)?;
    let journal = load(&root);
    let current = point(&root);
    let check = |entry: Option<&Entry>, redo: bool| {
        entry.and_then(|entry| match &current {
            Some(now) => blocked(&root, entry, redo, now),
            None => Some("HEAD is not readable.".into()),
        })
    };
    Ok(View {
        undo_blocked: check(
            journal
                .applied
                .checked_sub(1)
                .and_then(|at| journal.entries.get(at)),
            false,
        ),
        redo_blocked: check(journal.entries.get(journal.applied), true),
        entries: journal.entries,
        applied: journal.applied,
        current,
    })
}

/// Saves local changes of tracked files as a stash commit before they could be touched.
fn snapshot_changes(root: &Path, why: &str) -> Result<()> {
    let id = git(root, &["stash", "create", why])?;
    let id = id.trim();
    if !id.is_empty() {
        super::gitgraph::retain_recovery(root, id, why)?;
    }
    Ok(())
}

/// Whether the local changes of tracked files are the ones in the snapshot of `entry`, staged
/// and unstaged alike: removing them again loses nothing.
fn has_only(root: &Path, entry: &Entry) -> Result<bool> {
    let Some(snapshot) = &entry.snapshot else {
        return Ok(false);
    };
    let now = git(root, &["stash", "create", "Gall comparison before redo"])?;
    let now = now.trim();
    if now.is_empty() {
        return Ok(false);
    }
    // The work tree and the index, each as a tree.
    let trees = |id: &str| {
        git(
            root,
            &[
                "rev-parse",
                &format!("{id}^{{tree}}"),
                &format!("{id}^2^{{tree}}"),
            ],
        )
    };
    Ok(trees(now)? == trees(snapshot)?)
}

fn rewrite_keeps_tree(root: &Path, entry: &Entry) -> Result<bool> {
    if entry.kind != Kind::Rewrite
        || entry
            .snapshot
            .as_ref()
            .is_some_and(|id| id != &entry.before.id && id != &entry.after.id)
    {
        return Ok(false);
    }
    let listing = git(
        root,
        &[
            "rev-parse",
            &format!("{}^{{tree}}", entry.before.id),
            &format!("{}^{{tree}}", entry.after.id),
        ],
    )?;
    let mut trees = listing.lines();
    Ok(
        matches!((trees.next(), trees.next(), trees.next()), (Some(before), Some(after), None) if before == after),
    )
}

/// Moves HEAD from where it is to `target`. Returns a note about what could not be restored.
fn go(root: &Path, entry: &Entry, redo: bool) -> Result<Option<String>> {
    let (from, target) = if redo {
        (&entry.before, &entry.after)
    } else {
        (&entry.after, &entry.before)
    };
    // It may have been collected, or the journal may come from another copy.
    if git(
        root,
        &["cat-file", "-e", &format!("{}^{{commit}}", target.id)],
    )
    .is_err()
    {
        return Err(fail(format!(
            "Commit {} no longer exists in this repository",
            &target.id[..8.min(target.id.len())]
        )));
    }
    let plain = ["-c", "submodule.recurse=false"];
    let run = |args: &[&str]| {
        let all: Vec<&str> = plain.iter().chain(args).copied().collect();
        git(root, &all)
    };
    // What is left behind stays reachable, so that it can be done again.
    if !from.id.is_empty() {
        update_refs(
            root,
            &format!("update refs/gall/journal/{0} {0}\n", from.id),
        )?;
    }
    if from.reference != target.reference {
        snapshot_changes(root, "Gall recovery before undo of a checkout")?;
        match &target.reference {
            // Git refuses when local changes or untracked files are in the way.
            Some(name) => {
                let branch = name.strip_prefix("refs/heads/").unwrap_or(name);
                run(&["switch", "--no-overwrite-ignore", "--", branch])?
            }
            None => run(&["switch", "--no-overwrite-ignore", "--detach", &target.id])?,
        };
    } else if entry.kind.keeps_files() || rewrite_keeps_tree(root, entry)? {
        run(&["reset", "--soft", &target.id])?;
    } else if entry.kind == Kind::ResetMixed {
        // Files stay as they are; the index is the one from before (or the commit's, on redo).
        snapshot_changes(root, "Gall recovery before undo of a mixed reset")?;
        if redo {
            run(&["reset", "--mixed", "-q", &target.id])?;
        } else {
            run(&["reset", "--soft", &target.id])?;
            match &entry.snapshot {
                Some(snapshot) => run(&["read-tree", &format!("{snapshot}^2")])?,
                None => run(&["reset", "--mixed", "-q", &target.id])?,
            };
        }
        // The index has other file times now.
        let _ = run(&["update-index", "-q", "--refresh"]);
    } else if redo && entry.kind == Kind::ResetHard && has_only(root, entry)? {
        // The changes to remove are exactly those the entry has kept.
        super::gitrepository::protect_untracked(root, &[&target.id], None)?;
        run(&["reset", "--hard", "-q", &target.id])?;
    } else {
        super::gitrepository::protect_reset_keep_paths(root, &from.id, &target.id)?;
        snapshot_changes(root, "Gall recovery before undo")?;
        // `--keep` moves the branch and the files, and refuses to touch a locally changed one.
        run(&["reset", "--keep", &target.id]).map_err(|e| {
            fail(format!(
                "Git could not move HEAD safely. Review the affected paths in Working Changes, then commit or stash the overlapping edits and try again.\n{e}"
            ))
        })?;
    }
    if redo || entry.kind == Kind::ResetMixed {
        return Ok(None);
    }
    let Some(snapshot) = &entry.snapshot else {
        return Ok(None);
    };
    // Older history rewrites could record their original HEAD as a stash snapshot.
    if snapshot == &entry.before.id || snapshot == &entry.after.id {
        return Ok(None);
    }
    // The local changes the operation removed. `--index` brings the staged ones back as staged.
    if run(&["stash", "apply", "--index", "-q", snapshot]).is_err()
        && run(&["stash", "apply", "-q", snapshot]).is_err()
    {
        return Ok(Some(format!(
            "The local changes from before could not be applied; they are kept in {}",
            &snapshot[..12.min(snapshot.len())]
        )));
    }
    Ok(None)
}

/// Undoes or redoes one after the other until `applied` operations are applied. `expected` is
/// the number the user saw. Stops at the first step that is refused; earlier steps stay.
pub fn move_to(path: &Path, applied: usize, expected: usize) -> Result<String> {
    let _busy = super::gitfacts::mutating();
    let root = root(path)?;
    // Content filters do not matter here: HEAD, the index and the files move only through `reset`,
    // `stash create` and `stash apply`, and the index trees come from Git itself, so Git runs the
    // filters as it does for lazygit.
    let mut journal = load(&root);
    if journal.applied != expected || applied > journal.entries.len() {
        return Err(fail("The operation journal changed. Refresh it first"));
    }
    let mut notes = Vec::new();
    while journal.applied != applied {
        let redo = journal.applied < applied;
        let at = if redo {
            journal.applied
        } else {
            journal.applied - 1
        };
        let entry = journal.entries[at].clone();
        let now = point(&root).ok_or_else(|| fail("HEAD is not readable"))?;
        if let Some(reason) = blocked(&root, &entry, redo, &now) {
            return Err(fail(reason));
        }
        notes.extend(go(&root, &entry, redo)?);
        journal.applied = if redo { at + 1 } else { at };
        // After every step: a later one that is refused leaves the journal in step with HEAD.
        store(&root, &journal)?;
    }
    Ok(notes.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::super::gitrepository::{self, Request, ResetMode};
    use super::*;

    fn repository() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(p, &["config", key, value]).unwrap();
        }
        commit(p, "file.txt", "one\n", "one");
        d
    }

    fn commit(p: &Path, file: &str, text: &str, message: &str) -> String {
        write(p, file, text);
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", message]).unwrap();
        head(p)
    }

    fn write(p: &Path, file: &str, text: &str) {
        std::fs::write(p.join(file), text).unwrap();
    }

    fn text(p: &Path, file: &str) -> String {
        std::fs::read_to_string(p.join(file)).unwrap_or_default()
    }

    fn head(p: &Path) -> String {
        git(p, &["rev-parse", "HEAD"]).unwrap().trim().into()
    }

    fn status(p: &Path) -> String {
        git(p, &["status", "--porcelain"]).unwrap()
    }

    fn act(p: &Path, request: Request) -> Result<String> {
        let token = gitrepository::snapshot(p)?.token;
        gitrepository::execute(p, request, &token)
    }

    fn index(p: &Path, action: super::super::gitworktree::IndexAction, message: &str) {
        use super::super::gitworktree::{index_action, workspace};
        let token = workspace(p).unwrap().token;
        index_action(p, action, None, message, &token).unwrap();
    }

    fn undo(p: &Path) -> Result<String> {
        let at = view(p)?.applied;
        move_to(p, at - 1, at)
    }

    fn redo(p: &Path) -> Result<String> {
        let at = view(p)?.applied;
        move_to(p, at + 1, at)
    }

    #[test]
    fn head_is_read_as_git_reports_it() {
        let d = repository();
        let p = d.path();
        let check = || {
            let found = read_point(p).unwrap();
            assert_eq!(found.id, head(p));
            assert_eq!(
                found.reference,
                git(p, &["symbolic-ref", "-q", "HEAD"])
                    .ok()
                    .map(|r| r.trim().to_owned())
            );
        };
        check();
        git(p, &["pack-refs", "--all"]).unwrap();
        check();
        git(p, &["switch", "-qc", "feature/a"]).unwrap();
        check();
        git(p, &["pack-refs", "--all"]).unwrap();
        // A name that only ends in the name of the branch is another one.
        git(p, &["branch", "x/refs/heads/feature/a"]).unwrap();
        check();
        git(p, &["switch", "-q", "--detach"]).unwrap();
        check();
        git(p, &["worktree", "add", "-q", "-b", "wt", "linked"]).unwrap();
        let linked = p.join("linked");
        assert_eq!(
            read_point(&linked).unwrap().reference.as_deref(),
            Some("refs/heads/wt")
        );
        git(p, &["switch", "-q", "--orphan", "empty"]).unwrap();
        assert_eq!(read_point(p).unwrap().id, "");
    }

    #[test]
    fn undo_and_redo_run_in_a_repository_with_a_content_filter() {
        use super::super::gitworktree::IndexAction;
        let d = repository();
        let p = d.path();
        let one = head(p);
        write(p, "file.txt", "two\n");
        git(p, &["add", "--all"]).unwrap();
        // Index operations still refuse a filter, so the commit is made before it is configured.
        index(p, IndexAction::Commit, "two");
        let two = head(p);
        git(p, &["config", "filter.keep.clean", "cat"]).unwrap();
        git(p, &["config", "filter.keep.smudge", "cat"]).unwrap();
        std::fs::write(p.join(".git/info/attributes"), "*.txt filter=keep\n").unwrap();
        write(p, "local.txt", "mine\n");
        write(p, "file.txt", "edited\n");
        undo(p).unwrap_or_else(|e| panic!("undo: {e}"));
        assert_eq!(head(p), one);
        assert_eq!(text(p, "file.txt"), "edited\n");
        assert_eq!(text(p, "local.txt"), "mine\n");
        redo(p).unwrap_or_else(|e| panic!("redo: {e}"));
        assert_eq!(head(p), two);
        assert_eq!(text(p, "local.txt"), "mine\n");
    }

    #[test]
    fn undo_of_an_old_rewrite_does_not_apply_the_original_head_as_a_stash() {
        let d = repository();
        let p = d.path();
        let before = head(p);
        around(p, Some((Kind::Rewrite, "legacy rewrite".into())), || {
            saved_snapshot(&before);
            git(p, &["commit", "--amend", "-qm", "replacement"])
        })
        .unwrap();
        assert_eq!(
            view(p).unwrap().entries[0].snapshot.as_deref(),
            Some(before.as_str())
        );
        write(p, "file.txt", "stage this\n");
        git(p, &["add", "--", "file.txt"]).unwrap();
        write(p, "file.txt", "working edit\n");
        let after = head(p);
        let expected = status(p);
        let result = undo(p).unwrap();
        assert!(!result.contains("could not be applied"));
        assert_eq!(head(p), before);
        assert_eq!(status(p), expected);
        assert_eq!(git(p, &["show", ":file.txt"]).unwrap(), "stage this\n");
        redo(p).unwrap();
        assert_eq!(head(p), after);
        assert_eq!(status(p), expected);
        assert_eq!(text(p, "file.txt"), "working edit\n");
    }

    #[test]
    fn commit_and_amend_are_undone_without_touching_files() {
        use super::super::gitworktree::IndexAction;
        let d = repository();
        let p = d.path();
        let one = head(p);
        write(p, "file.txt", "two\n");
        git(p, &["add", "--all"]).unwrap();
        index(p, IndexAction::Commit, "two");
        let two = head(p);
        write(p, "new.txt", "new\n");
        git(p, &["add", "--all"]).unwrap();
        index(p, IndexAction::Amend, "amended");
        let amended = head(p);
        // Unrelated local work stays through all of it.
        write(p, "local.txt", "mine\n");
        write(p, "file.txt", "edited\n");
        assert_eq!(
            view(p)
                .unwrap()
                .entries
                .iter()
                .map(|e| e.kind)
                .collect::<Vec<_>>(),
            [Kind::Commit, Kind::Amend]
        );
        undo(p).unwrap();
        assert_eq!(head(p), two);
        assert_eq!(status(p), " M file.txt\nA  new.txt\n?? local.txt\n");
        undo(p).unwrap();
        assert_eq!(head(p), one);
        assert_eq!(text(p, "file.txt"), "edited\n");
        redo(p).unwrap();
        redo(p).unwrap();
        assert_eq!(head(p), amended);
        assert_eq!(status(p), " M file.txt\n?? local.txt\n");
        assert!(redo(p).is_err());
        // The replaced commit survives a garbage collection.
        git(p, &["reflog", "expire", "--expire=now", "--all"]).unwrap();
        git(p, &["gc", "-q", "--prune=now"]).unwrap();
        undo(p).unwrap();
        assert_eq!(head(p), two);
    }

    #[test]
    fn merge_cherry_pick_revert_and_rebase_are_undone_and_redone() {
        let d = repository();
        let p = d.path();
        git(p, &["switch", "-qc", "side"]).unwrap();
        let side = commit(p, "side.txt", "side\n", "side");
        git(p, &["switch", "-q", "main"]).unwrap();
        commit(p, "main.txt", "main\n", "main");
        let requests: Vec<Box<dyn Fn() -> Request>> = vec![
            Box::new(|| Request::Merge {
                target: side.clone(),
            }),
            Box::new(|| Request::CherryPick {
                target: side.clone(),
            }),
            Box::new(|| Request::CherryPickMany {
                targets: vec![side.clone()],
            }),
            Box::new(|| Request::Rebase {
                target: side.clone(),
            }),
            Box::new(|| Request::Revert { target: head(p) }),
        ];
        for request in requests {
            let before = head(p);
            act(p, request()).unwrap();
            let after = head(p);
            assert_ne!(before, after);
            let tree = git(p, &["ls-files"]).unwrap();
            undo(p).unwrap();
            assert_eq!((head(p), status(p)), (before.clone(), String::new()));
            redo(p).unwrap();
            assert_eq!((head(p), status(p)), (after, String::new()));
            assert_eq!(git(p, &["ls-files"]).unwrap(), tree);
            undo(p).unwrap();
            assert_eq!(head(p), before);
            assert_eq!(
                git(p, &["symbolic-ref", "HEAD"]).unwrap().trim(),
                "refs/heads/main"
            );
        }
    }

    #[test]
    fn local_changes_are_never_overwritten() {
        let d = repository();
        let p = d.path();
        git(p, &["switch", "-qc", "side"]).unwrap();
        let side = commit(p, "side.txt", "side\n", "side");
        git(p, &["switch", "-q", "main"]).unwrap();
        commit(p, "main.txt", "main\n", "main");
        act(p, Request::Merge { target: side }).unwrap();
        let merged = head(p);
        // Changed here, removed by the undo: refused, nothing moves.
        write(p, "side.txt", "edited\n");
        assert!(undo(p).is_err());
        assert_eq!(head(p), merged);
        assert_eq!(text(p, "side.txt"), "edited\n");
        assert_eq!(view(p).unwrap().applied, 1);
        git(p, &["checkout", "-q", "--", "side.txt"]).unwrap();
        // Changed elsewhere: kept.
        write(p, "file.txt", "edited\n");
        undo(p).unwrap();
        assert_eq!(text(p, "file.txt"), "edited\n");
        // An untracked and an ignored file where the redo would write.
        for ignored in [false, true] {
            write(p, "side.txt", "mine\n");
            if ignored {
                std::fs::write(p.join(".git/info/exclude"), "side.txt\n").unwrap();
            }
            let error = redo(p).unwrap_err();
            assert!(error.to_string().contains("side.txt"));
            assert_eq!(text(p, "side.txt"), "mine\n");
            assert_eq!(view(p).unwrap().applied, 0);
        }
    }

    #[test]
    fn undo_keeps_untracked_bytes_at_an_unchanged_path() {
        let d = repository();
        let p = d.path();
        git(p, &["switch", "-qc", "side"]).unwrap();
        let side = commit(p, "side.txt", "side\n", "side");
        git(p, &["switch", "-q", "main"]).unwrap();
        let before = commit(p, "main.txt", "main\n", "main");
        act(p, Request::Merge { target: side }).unwrap();

        git(p, &["rm", "--cached", "--", "file.txt"]).unwrap();
        write(p, "file.txt", "local replacement\n");
        undo(p).unwrap();
        assert_eq!(head(p), before);
        assert_eq!(text(p, "file.txt"), "local replacement\n");
        assert!(!p.join("side.txt").exists());
    }

    #[test]
    fn resets_bring_back_the_index_and_the_removed_changes() {
        let d = repository();
        let p = d.path();
        let one = head(p);
        let two = commit(p, "file.txt", "two\n", "two");
        let prepare = || {
            write(p, "file.txt", "staged\n");
            git(p, &["add", "file.txt"]).unwrap();
            write(p, "file.txt", "unstaged\n");
        };
        let reset = |mode| {
            act(
                p,
                Request::Reset {
                    target: one.clone(),
                    mode,
                },
            )
            .unwrap();
            assert_eq!(head(p), one);
        };
        let restored = || {
            assert_eq!(head(p), two);
            assert_eq!(text(p, "file.txt"), "unstaged\n");
            assert_eq!(git(p, &["show", ":file.txt"]).unwrap(), "staged\n");
        };
        for mode in [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard] {
            let hard = matches!(mode, ResetMode::Hard);
            prepare();
            reset(mode);
            undo(p).unwrap();
            restored();
            // Something more than the snapshot has: a hard reset does not remove it.
            if hard {
                write(p, "file.txt", "more\n");
                assert!(redo(p).is_err());
                assert_eq!(text(p, "file.txt"), "more\n");
                write(p, "file.txt", "unstaged\n");
            }
            redo(p).unwrap();
            assert_eq!(head(p), one);
            undo(p).unwrap();
            restored();
            git(p, &["reset", "-q", "--hard"]).unwrap();
        }
    }

    #[test]
    fn a_checkout_with_local_changes_is_undone_with_them() {
        use super::super::gitgraph::switch;
        let d = repository();
        let p = d.path();
        git(p, &["branch", "side"]).unwrap();
        write(p, "file.txt", "edited\n");
        write(p, "new.txt", "new\n");
        switch(p, "side", true, "discard", "").unwrap();
        assert_eq!(status(p), "");
        undo(p).unwrap();
        let on = || git(p, &["symbolic-ref", "HEAD"]).unwrap();
        assert_eq!(on().trim(), "refs/heads/main");
        assert_eq!(text(p, "file.txt"), "edited\n");
        assert_eq!(text(p, "new.txt"), "new\n");
        // To a commit, with a commit of the changes first.
        let before = head(p);
        switch(p, &before, false, "commit", "saved").unwrap();
        assert!(read_point(p).unwrap().reference.is_none());
        undo(p).unwrap();
        assert_eq!(on().trim(), "refs/heads/main");
        assert_eq!(
            git(p, &["log", "-1", "--format=%s"]).unwrap().trim(),
            "saved"
        );
        redo(p).unwrap();
        assert!(read_point(p).unwrap().reference.is_none());
        assert_eq!(head(p), before);
    }

    #[test]
    fn a_stopped_operation_is_one_entry_once_it_is_finished() {
        let d = repository();
        let p = d.path();
        git(p, &["switch", "-qc", "side"]).unwrap();
        commit(p, "side.txt", "side\n", "side one");
        let side = commit(p, "file.txt", "side\n", "side two");
        git(p, &["switch", "-q", "main"]).unwrap();
        let main = commit(p, "file.txt", "main\n", "main");
        let start = |p: &Path| {
            assert!(act(
                p,
                Request::Rebase {
                    target: side.clone()
                }
            )
            .is_err());
        };
        start(p);
        assert!(view(p).unwrap().entries.is_empty());
        act(p, Request::Abort).unwrap();
        assert!(view(p).unwrap().entries.is_empty() && head(p) == main);
        start(p);
        act(
            p,
            Request::ResolveContent {
                file: "file.txt".into(),
                content: "both\n".into(),
            },
        )
        .unwrap();
        act(p, Request::Continue).unwrap();
        let seen = view(p).unwrap();
        assert_eq!(
            (seen.entries.len(), seen.entries[0].kind),
            (1, Kind::Rebase)
        );
        assert_eq!(seen.entries[0].before.id, main);
        undo(p).unwrap();
        assert_eq!((head(p), text(p, "file.txt")), (main, "main\n".into()));
    }

    #[test]
    fn external_changes_are_detected_and_refused() {
        let d = repository();
        let p = d.path();
        let one = head(p);
        commit(p, "file.txt", "two\n", "two");
        act(
            p,
            Request::Reset {
                target: one.clone(),
                mode: ResetMode::Soft,
            },
        )
        .unwrap();
        // Somebody commits in a terminal.
        let outside = commit(p, "file.txt", "outside\n", "outside");
        let seen = view(p).unwrap();
        assert!(seen.undo_blocked.unwrap().contains("moved outside Gall"));
        assert!(undo(p).unwrap_err().to_string().contains("outside Gall"));
        assert_eq!(head(p), outside);
        git(p, &["reset", "-q", "--hard", &one]).unwrap();
        // Another branch at the same commit.
        git(p, &["switch", "-qc", "other"]).unwrap();
        assert!(undo(p).unwrap_err().to_string().contains("Switch back"));
        git(p, &["switch", "-q", "main"]).unwrap();
        // A stale number of the timeline.
        assert!(move_to(p, 0, 7).is_err() && move_to(p, 9, 1).is_err());
        assert!(view(p).unwrap().undo_blocked.is_none());
        undo(p).unwrap();
        // In the middle of an operation.
        git(p, &["switch", "-qc", "side", &one]).unwrap();
        let side = commit(p, "file.txt", "side\n", "side");
        git(p, &["switch", "-q", "main"]).unwrap();
        assert!(git(p, &["merge", "-q", &side]).is_err());
        assert!(view(p).unwrap().redo_blocked.unwrap().contains("Finish"));
        assert!(redo(p).is_err());
    }

    #[test]
    fn jumps_over_several_and_a_new_operation_ends_the_redo() {
        use super::super::gitworktree::IndexAction;
        let d = repository();
        let p = d.path();
        let mut ids = vec![head(p)];
        for i in 0..4 {
            write(p, "file.txt", &format!("{i}\n"));
            git(p, &["add", "--all"]).unwrap();
            index(p, IndexAction::Commit, &format!("c{i}"));
            ids.push(head(p));
        }
        move_to(p, 1, 4).unwrap();
        assert_eq!(head(p), ids[1]);
        move_to(p, 3, 1).unwrap();
        assert_eq!(head(p), ids[3]);
        index(p, IndexAction::Amend, "other");
        let seen = view(p).unwrap();
        assert_eq!((seen.entries.len(), seen.applied), (4, 4));
        assert_eq!(seen.entries[3].kind, Kind::Amend);
        assert!(redo(p).is_err());
    }

    #[test]
    fn the_first_commit_and_a_broken_journal_are_harmless() {
        use super::super::gitworktree::IndexAction;
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [("user.name", "T"), ("user.email", "t@example.com")] {
            git(p, &["config", key, value]).unwrap();
        }
        write(p, "a.txt", "a\n");
        git(p, &["add", "--all"]).unwrap();
        index(p, IndexAction::Commit, "first");
        assert!(undo(p).unwrap_err().to_string().contains("first commit"));
        std::fs::write(journal_path(p).unwrap(), "{ not json").unwrap();
        assert!(view(p).unwrap().entries.is_empty());
        write(p, "a.txt", "b\n");
        git(p, &["add", "--all"]).unwrap();
        index(p, IndexAction::Commit, "second");
        assert_eq!(view(p).unwrap().entries.len(), 1);
    }

    #[test]
    fn old_entries_leave_with_their_references() {
        let d = repository();
        let p = d.path();
        let id = head(p);
        update_refs(p, &format!("update refs/gall/journal/{id} {id}\n")).unwrap();
        let point = |id: &str| Point {
            reference: Some("refs/heads/main".into()),
            id: id.into(),
        };
        let mut journal = Journal::default();
        for i in 0..MAX_ENTRIES + PRUNE_SLACK {
            journal.entries.push(Entry {
                id: i as u64,
                kind: Kind::Commit,
                label: String::new(),
                time: chrono::Utc::now().timestamp(),
                before: point(&id),
                after: point(&id),
                snapshot: None,
            });
        }
        journal.applied = journal.entries.len();
        let dropped = prune(&mut journal);
        assert_eq!(
            (dropped.len(), journal.entries.len(), journal.applied),
            (PRUNE_SLACK, MAX_ENTRIES, MAX_ENTRIES)
        );
        // Still needed by the entries that stay.
        release(p, &journal, &dropped);
        let exists = || {
            git(
                p,
                &["rev-parse", "--verify", &format!("refs/gall/journal/{id}")],
            )
            .is_ok()
        };
        assert!(exists());
        journal.entries.clear();
        release(p, &journal, &dropped);
        assert!(!exists());
        // Entries that could be redone are never pruned from under the user.
        let mut undone = Journal {
            entries: dropped
                .iter()
                .cloned()
                .map(|e| Entry { time: 0, ..e })
                .collect(),
            ..Journal::default()
        };
        assert!(prune(&mut undone).is_empty() && undone.entries.len() == PRUNE_SLACK);
    }
}
