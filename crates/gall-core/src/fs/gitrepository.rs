//! Typed local repository operations used by the Git workspace.
//!
//! The caller serializes mutations and supplies a freshly reviewed snapshot token.
//! Git still checks its own locks; a token cannot lock out another Git application.
use super::gitgraph::{has_content_filters, joined, retain_detached, root};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub root: String,
    pub head: String,
    pub branch: Option<String>,
    pub refs: Vec<Reference>,
    pub stashes: Vec<Stash>,
    pub remotes: Vec<Remote>,
    pub operation: Option<String>,
    pub operation_commit: Option<OperationCommit>,
    pub edit_stop: bool,
    pub bisect_active: bool,
    pub conflicts: Vec<String>,
    pub token: String,
}

/// The commit an interrupted operation is currently applying (or merging).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationCommit {
    pub id: String,
    pub subject: String,
    pub step: Option<(u32, u32)>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub name: String,
    pub full_name: String,
    pub kind: String,
    pub id: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// The configured upstream no longer exists (for example deleted on the remote).
    pub upstream_gone: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stash {
    pub id: String,
    pub selector: String,
    pub message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub file: String,
    pub base: String,
    pub ours: String,
    pub theirs: String,
    pub result: String,
    pub binary: bool,
    pub token: String,
}

/// Resolve a repository-relative path inside the working tree, refusing symlinked components.
fn checked_work_path(root: &Path, file: &str) -> Result<std::path::PathBuf> {
    let relative = Path::new(file);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(fail("Invalid conflict path"));
    }
    let mut work = root.to_path_buf();
    for component in relative.components() {
        work.push(component);
        match work.symlink_metadata() {
            Ok(meta) if meta.is_symlink() => {
                return Err(fail("The text editor cannot follow symlinks. Choose a Git side or mark the working path resolved."))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(fail(e.to_string())),
        }
    }
    Ok(root.join(relative))
}

/// Inspect a current conflict without following working-tree symlinks.
/// Missing stages (add/delete conflicts) are represented as empty text.
pub fn conflict(path: &Path, file: &str) -> Result<Conflict> {
    use std::io::Read;
    const MAX_PREVIEW: u64 = 1024 * 1024;
    let p = root(path)?;
    if !conflicts(&p)?.iter().any(|name| name == file) {
        return Err(fail("This file is no longer an unresolved conflict"));
    }
    let before = token(&p)?;
    let relative = Path::new(file);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(fail("Invalid conflict path"));
    }
    let mut work = p.clone();
    for component in relative.components() {
        work.push(component);
        match work.symlink_metadata() {
            Ok(meta) if meta.is_symlink() => {
                return Err(fail("The text editor cannot follow symlinks. Choose a Git side or mark the working path resolved."))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(fail(e.to_string())),
        }
    }
    let read_stage = |stage| -> Result<String> {
        let spec = format!(":{stage}:{file}");
        let size = match git(&p, &["cat-file", "-s", &spec]) {
            Ok(size) => size
                .trim()
                .parse::<u64>()
                .map_err(|_| fail("Invalid conflict object size"))?,
            Err(_) => return Ok(String::new()),
        };
        if size > MAX_PREVIEW {
            return Err(fail(
                "The text preview is limited to 1 MiB. Choose a Git side or mark the working file resolved.",
            ));
        }
        git(&p, &["cat-file", "blob", &spec])
    };
    let base = read_stage(1)?;
    let ours = read_stage(2)?;
    let theirs = read_stage(3)?;
    let mut bytes = Vec::new();
    match super::open_regular(&p.join(file)) {
        Ok(input) => {
            input
                .take(MAX_PREVIEW + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| fail(e.to_string()))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(fail(e.to_string())),
    }
    if bytes.len() as u64 > MAX_PREVIEW {
        return Err(fail(
            "The text preview is limited to 1 MiB. Choose a Git side or mark the working file resolved.",
        ));
    }
    let result = String::from_utf8_lossy(&bytes).into_owned();
    let binary = [&base, &ours, &theirs, &result]
        .iter()
        .any(|text| text.contains('\0') || text.contains('\u{fffd}'));
    let after = token(&p)?;
    if before != after {
        return Err(fail(
            "The conflict changed while loading. Refresh and review it again",
        ));
    }
    Ok(Conflict {
        file: file.into(),
        base,
        ours,
        theirs,
        result,
        binary,
        token: after,
    })
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ResolveSide {
    Ours,
    Theirs,
    Mark,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeMode {
    NoFastForward,
    NoCommit,
    FastForwardOnly,
    Squash,
}

#[derive(Deserialize, Debug)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Request {
    CreateBranch {
        name: String,
        target: String,
    },
    RenameBranch {
        name: String,
        new_name: String,
    },
    DeleteBranch {
        name: String,
        /// `git branch -D`: also deletes a branch with commits that are not merged.
        #[serde(default)]
        force: bool,
    },
    CreateTag {
        name: String,
        target: String,
        message: String,
    },
    DeleteTag {
        name: String,
    },
    SetUpstream {
        branch: String,
        upstream: String,
    },
    StashSave {
        message: String,
        /// Staged changes stay in the index and the working files.
        #[serde(default)]
        keep_index: bool,
        /// Only tracked files; new ones stay.
        #[serde(default)]
        tracked_only: bool,
        /// Only these files.
        #[serde(default)]
        files: Vec<String>,
    },
    /// A branch at the commit the stash was made on, with the stash applied and removed.
    StashBranch {
        id: String,
        name: String,
    },
    StashApply {
        id: String,
    },
    StashPop {
        id: String,
    },
    StashDrop {
        id: String,
    },
    StashRename {
        id: String,
        name: String,
    },
    Merge {
        target: String,
    },
    MergeWithMode {
        target: String,
        mode: MergeMode,
    },
    Rebase {
        target: String,
    },
    /// Several commits as one operation, in the given order (oldest first). Git stops at a
    /// conflict; continue goes on with the rest and abort takes all of them back.
    CherryPickMany {
        targets: Vec<String>,
    },
    CherryPick {
        target: String,
    },
    Revert {
        target: String,
    },
    IntegrateMerge {
        target: String,
        mainline: usize,
        revert: bool,
    },
    Reset {
        target: String,
        mode: ResetMode,
    },
    Continue,
    Skip,
    Abort,
    Resolve {
        file: String,
        side: ResolveSide,
    },
    ResolveContent {
        file: String,
        content: String,
    },
    AddRemote {
        name: String,
        url: String,
    },
    RenameRemote {
        name: String,
        new_name: String,
    },
    RemoveRemote {
        name: String,
    },
    SetRemoteUrl {
        name: String,
        url: String,
    },
}

/// What the token check of an action read about the repository: its guards (is the work
/// tree clean, is something unmerged, are there submodules, where is HEAD) ask for the
/// same things, and used to start the same Git processes again, one after another.
struct Seen {
    path: PathBuf,
    head: String,
    /// `ls-files --stage -z`.
    index: String,
    /// The work tree status; None in a repository with content filters.
    status: Option<String>,
    /// What the guards of this action will ask, read next to the token check.
    prefetched: Prefetched,
}

/// Read-only answers the guards of an action need, asked side by side with the token check
/// instead of one after another behind it. Each is what the guard would have asked Git a
/// moment later; a missing answer (not asked, or Git failed) makes the guard ask itself.
#[derive(Default)]
struct Prefetched {
    /// Commits `commit_id` would accept.
    commits: Vec<String>,
    /// `ls-files --others -z`.
    others: Option<String>,
    /// `ls-tree -r --name-only -z <target>` by target.
    trees: Vec<(String, String)>,
    /// `(target, log --raw … <target>..HEAD)` of a rebase.
    replayed: Option<(String, String)>,
}

/// Starts what [`Prefetched`] holds for `request` in `path`.
fn prefetch(path: &Path, request: &Request) -> Prefetched {
    let (target, untracked, tree, replay) = match request {
        Request::Merge { target } | Request::MergeWithMode { target, .. } => {
            (target, false, false, false)
        }
        Request::Rebase { target } => (target, true, true, true),
        Request::Revert { target } => (target, true, false, false),
        Request::Reset { target, .. }
        | Request::CreateBranch { target, .. }
        | Request::CreateTag { target, .. } => (target, false, false, false),
        _ => return Prefetched::default(),
    };
    if !matches!(target.len(), 40 | 64) || !target.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Prefetched::default();
    }
    let target = target.as_str();
    let read = |args: &[&str]| super::gitgraph::git(path, args).ok();
    std::thread::scope(|scope| {
        // HEAD itself needs no question (see `commit_id`).
        let commit = (super::gitfacts::head_id(path).as_deref() != Some(target)).then(|| {
            scope.spawn(move || {
                read(&["rev-parse", "--verify", &format!("{target}^{{commit}}")])
                    .filter(|id| id.trim() == target)
                    .map(|_| target.to_owned())
            })
        });
        let tree = tree
            .then(|| scope.spawn(move || read(&["ls-tree", "-r", "--name-only", "-z", target])));
        let replayed = replay.then(|| {
            scope.spawn(move || {
                read(&[
                    "log",
                    "--format=",
                    "--raw",
                    "-z",
                    "--no-renames",
                    "--no-abbrev",
                    &format!("{target}..HEAD"),
                ])
            })
        });
        let others = untracked
            .then(|| read(&["ls-files", "--others", "-z"]))
            .flatten();
        let done = |handle: Option<std::thread::ScopedJoinHandle<'_, Option<String>>>| {
            handle.and_then(|h| h.join().ok().flatten())
        };
        Prefetched {
            commits: done(commit).into_iter().collect(),
            others,
            trees: done(tree)
                .map(|listing| (target.to_owned(), listing))
                .into_iter()
                .collect(),
            replayed: done(replayed).map(|log| (target.to_owned(), log)),
        }
    })
}

thread_local! {
    /// The state read for the action running on this thread, until that action first
    /// changes the repository (see [`git`]).
    static SEEN: std::cell::RefCell<Option<Seen>> = const { std::cell::RefCell::new(None) };
}

/// Keeps [`SEEN`] for as long as it lives.
struct Armed;
impl Armed {
    fn new(seen: Seen) -> Self {
        SEEN.with(|slot| *slot.borrow_mut() = Some(seen));
        Armed
    }
}
impl Drop for Armed {
    fn drop(&mut self) {
        SEEN.with(|slot| *slot.borrow_mut() = None);
    }
}

/// What was read for the running action about `path`, if it is still current.
fn seen<T>(path: &Path, read: impl FnOnce(&Seen) -> Option<T>) -> Option<T> {
    SEEN.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|seen| seen.path == path)
            .and_then(read)
    })
}

/// Runs Git. The first command that can change the repository ends the use of what the
/// token check read: from then on every question is asked again.
fn git(path: &Path, args: &[&str]) -> Result<String> {
    if !super::gitgraph::reads_only(args) {
        SEEN.with(|slot| *slot.borrow_mut() = None);
    }
    super::gitgraph::git(path, args)
}

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

fn fields(value: &str) -> Vec<&str> {
    value.split('\0').collect()
}

fn checked_text(value: String) -> Result<String> {
    if value.contains('\u{fffd}') {
        Err(fail("Git names with invalid UTF-8 are not supported"))
    } else {
        Ok(value)
    }
}

/// Check out one historical file into the index and working tree.
pub fn checkout_commit_file(path: &Path, commit: &str, file: &str) -> Result<()> {
    use super::gitinspect::{commit_id, relative_file};

    relative_file(file)?;
    let _busy = super::gitfacts::mutating();
    let root = root(path)?;
    let commit = commit_id(&root, commit)?;
    let literal = format!(":(top,literal){file}");
    let entry = git(&root, &["ls-tree", "-r", "-z", &commit, "--", &literal])?;
    let historical_file = entry
        .strip_suffix('\0')
        .and_then(|entry| entry.split_once('\t'))
        .is_some_and(|(info, name)| name == file && info.contains(" blob "));
    if !historical_file {
        return Err(fail(format!("The selected revision has no file at {file}")));
    }

    let collision = |name: &Path| {
        fail(format!(
            "Checking out {file} would replace local data at {}. Commit or stash tracked changes in Working Changes, or move the item aside in the file manager.",
            name.display()
        ))
    };
    let relative = Path::new(file);
    let mut ancestors: Vec<_> = relative
        .ancestors()
        .take_while(|p| !p.as_os_str().is_empty())
        .collect();
    ancestors.reverse();
    let mut occupied = false;
    for name in ancestors {
        match root.join(name).symlink_metadata() {
            Ok(meta) if name == relative && !meta.is_dir() => occupied = true,
            Ok(meta) if name != relative && meta.is_dir() => {}
            Ok(_) => return Err(collision(name)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(fail(error.to_string())),
        }
    }

    let status = git(
        &root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--no-renames",
            "--untracked-files=all",
            "--ignored=matching",
            "--",
            &literal,
        ],
    )?;
    if !status.is_empty() {
        return Err(collision(relative));
    }
    // These index flags can cause status to omit working-tree edits.
    let index = git(&root, &["ls-files", "-v", "--stage", "-z", "--", &literal])?;
    if (occupied && index.is_empty())
        || fields(&index).into_iter().any(|entry| {
            entry.starts_with("S ") || entry.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        })
    {
        return Err(collision(relative));
    }
    super::gitadvanced::verify_selected_filtered_checkout_bytes(&root, &index, file)?;
    git(
        &root,
        &[
            "checkout",
            "--no-recurse-submodules",
            &commit,
            "--",
            &literal,
        ],
    )?;
    Ok(())
}

fn git_path(path: &Path, name: &str) -> Result<PathBuf> {
    // The operation markers live in the per-worktree Git directory, so no `rev-parse` is
    // needed. Each lookup used to be a process of its own.
    if let Some(found) = super::gitfacts::marker(path, name) {
        return Ok(found);
    }
    let value = git(path, &["rev-parse", "--git-path", name])?;
    Ok(path.join(super::gitexec::host_path(path, value.trim())))
}

/// Whether the operation marker `name` exists, from `listed` (one listing of the Git
/// directory, see `gitfacts::Markers`) where there is one.
pub(super) fn marker_exists(
    path: &Path,
    listed: Option<&super::gitfacts::Markers>,
    name: &str,
) -> Result<bool> {
    match listed {
        Some(markers) => Ok(markers.exists(name)),
        None => Ok(git_path(path, name)?.exists()),
    }
}

/// The content of the operation marker `name`, if it exists (see [`marker_exists`]).
pub(super) fn marker_content(
    path: &Path,
    listed: Option<&super::gitfacts::Markers>,
    name: &str,
) -> Result<Option<Vec<u8>>> {
    match listed {
        Some(markers) => Ok(markers.read(name)),
        None => Ok(std::fs::read(git_path(path, name)?).ok()),
    }
}

fn operation(path: &Path) -> Result<Option<String>> {
    operation_in(path, super::gitfacts::markers(path).as_ref())
}

fn operation_in(path: &Path, listed: Option<&super::gitfacts::Markers>) -> Result<Option<String>> {
    for (marker, name) in [
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
    ] {
        if marker_exists(path, listed, marker)? {
            return Ok(Some(name.into()));
        }
    }
    let todo = marker_content(path, listed, "sequencer/todo")?;
    if let Some(value) = todo.and_then(|bytes| String::from_utf8(bytes).ok()) {
        if value.lines().any(|l| l.starts_with("revert ")) {
            return Ok(Some("revert".into()));
        }
        if value.lines().any(|l| l.starts_with("pick ")) {
            return Ok(Some("cherry-pick".into()));
        }
    }
    Ok(None)
}

fn operation_commit(path: &Path) -> Option<OperationCommit> {
    let name = operation(path).ok()??;
    let marker = match name.as_str() {
        "rebase" => "REBASE_HEAD",
        "merge" => "MERGE_HEAD",
        "cherry-pick" => "CHERRY_PICK_HEAD",
        _ => "REVERT_HEAD",
    };
    let id = git(path, &["rev-parse", "--verify", "-q", marker])
        .ok()?
        .trim()
        .to_string();
    let subject = git(path, &["log", "-1", "--format=%s", &id])
        .ok()?
        .trim()
        .to_string();
    let number = |file: &str| {
        let dir = if git_path(path, "rebase-merge").ok()?.exists() {
            "rebase-merge"
        } else {
            "rebase-apply"
        };
        let name = if dir == "rebase-apply" && file == "msgnum" {
            "next"
        } else if dir == "rebase-apply" {
            "last"
        } else {
            file
        };
        std::fs::read_to_string(git_path(path, &format!("{dir}/{name}")).ok()?)
            .ok()?
            .trim()
            .parse::<u32>()
            .ok()
    };
    let step = if name == "rebase" {
        number("msgnum").zip(number("end"))
    } else {
        None
    };
    Some(OperationCommit { id, subject, step })
}

fn head(path: &Path) -> String {
    if let Some(id) = seen(path, |seen| Some(seen.head.clone())) {
        return id;
    }
    if let Some((id, ..)) = super::gitfacts::head(path) {
        return id;
    }
    git(path, &["rev-parse", "--verify", "HEAD"])
        .unwrap_or_default()
        .trim()
        .into()
}

/// Where HEAD is.
#[derive(Hash)]
struct Head {
    /// The commit; empty before the first commit.
    id: String,
    /// The reference HEAD points to (`refs/heads/main`); `HEAD` or empty when detached.
    reference: String,
    /// The current branch as Git shortens its name; `None` when detached.
    branch: Option<String>,
}

/// Reads HEAD from Git's files, or with one process where they cannot answer. A load needs it
/// in several places and reads it again for the final check, which used to be a `rev-parse` or
/// `symbolic-ref` for every one of them.
fn read_head(path: &Path) -> Head {
    if let Some((id, reference, branch)) = super::gitfacts::head(path) {
        return Head {
            id,
            reference,
            branch,
        };
    }
    let all = git(
        path,
        &[
            "rev-parse",
            "HEAD",
            "--symbolic-full-name",
            "HEAD",
            "--abbrev-ref",
            "HEAD",
        ],
    );
    if let Ok(all) = &all {
        if let [id, reference, branch] = all.lines().collect::<Vec<_>>()[..] {
            return Head {
                id: id.into(),
                reference: reference.into(),
                branch: (reference != "HEAD").then(|| branch.into()),
            };
        }
    }
    // Before the first commit that command fails as a whole: ask for each part.
    let symbolic = |args: &[&str]| git(path, args).ok().map(|s| s.trim().to_owned());
    Head {
        id: head(path),
        reference: symbolic(&["symbolic-ref", "-q", "HEAD"]).unwrap_or_default(),
        branch: symbolic(&["symbolic-ref", "--short", "-q", "HEAD"]),
    }
}

fn status(path: &Path) -> Result<String> {
    if let Some(status) = seen(path, |seen| seen.status.clone()) {
        return Ok(status);
    }
    checked_text(super::gitgraph::status_z(path)?)
}

fn conflicts(path: &Path) -> Result<Vec<String>> {
    match seen(path, |seen| Some(seen.index.clone())) {
        Some(index) => unmerged(&index),
        None => unmerged(&git(path, &["ls-files", "--unmerged", "-z"])?),
    }
}

/// The conflicted files of an index listing (`ls-files --stage -z` or `--unmerged -z`):
/// those with an entry of a stage other than 0.
fn unmerged(listing: &str) -> Result<Vec<String>> {
    let raw: String = listing
        .split_inclusive('\0')
        .filter(|record| {
            record
                .split_once('\t')
                .is_some_and(|(meta, _)| !meta.ends_with(" 0"))
        })
        .collect();
    let raw = checked_text(raw)?;
    let mut names: Vec<_> = raw
        .split('\0')
        .filter_map(|record| record.split_once('\t').map(|(_, name)| name.to_owned()))
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

fn has_filters(path: &Path) -> bool {
    has_content_filters(path)
}

fn refs(path: &Path) -> Result<Vec<Reference>> {
    let raw = checked_text(git(
        path,
        &[
            "for-each-ref",
            // Newest first (tag date, otherwise commit date), so the latest tags lead the list.
            "--sort=-creatordate",
            "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(upstream:short)%00%(upstream:track,nobracket)",
            "refs/heads/",
            "refs/remotes/",
            "refs/tags/",
        ],
    )?)?;
    Ok(raw
        .lines()
        .filter_map(|line| {
            let parts = fields(line);
            if parts.len() != 5 {
                return None;
            }
            let (kind, name) = if let Some(name) = parts[0].strip_prefix("refs/heads/") {
                ("branch", name)
            } else if let Some(name) = parts[0].strip_prefix("refs/remotes/") {
                ("remote", name)
            } else {
                ("tag", parts[0].strip_prefix("refs/tags/")?)
            };
            Some(Reference {
                name: name.into(),
                full_name: parts[0].into(),
                kind: kind.into(),
                id: if parts[2].is_empty() {
                    parts[1]
                } else {
                    parts[2]
                }
                .into(),
                upstream: (!parts[3].is_empty()).then(|| parts[3].into()),
                ahead: track_count(parts[4], "ahead"),
                behind: track_count(parts[4], "behind"),
                upstream_gone: parts[4].trim() == "gone",
            })
        })
        .collect())
}

fn track_count(track: &str, word: &str) -> u32 {
    track
        .split(',')
        .find_map(|part| part.trim().strip_prefix(word))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0)
}

fn stashes(path: &Path) -> Result<Vec<Stash>> {
    if let Some(list) = super::gitfacts::stashes(path) {
        return Ok(list
            .into_iter()
            .map(|(id, selector, message)| Stash {
                id,
                selector,
                message,
            })
            .collect());
    }
    let raw = git(path, &["stash", "list", "--format=%H%x00%gd%x00%gs"])?;
    Ok(raw
        .lines()
        .filter_map(|line| {
            let parts = fields(line);
            (parts.len() == 3).then(|| Stash {
                id: parts[0].into(),
                selector: parts[1].into(),
                message: parts[2].into(),
            })
        })
        .collect())
}

fn remotes(path: &Path) -> Result<Vec<Remote>> {
    Ok(parse_remotes(&checked_text(git(path, &["remote", "-v"])?)?))
}

/// Remotes from `git remote -v` (`name<TAB>url (fetch)`, `name<TAB>url (push)`, in newer Git
/// followed by a note such as ` [blob:none]`): one process for all of them, with the same
/// rewritten URLs `remote get-url` prints. A remote with several URLs shows its first one.
fn parse_remotes(listing: &str) -> Vec<Remote> {
    let mut remotes: Vec<Remote> = Vec::new();
    for line in listing.lines() {
        let (name, rest) = line.split_once('\t').unwrap_or((line, ""));
        let (url, push) = match (rest.rfind(" (fetch)"), rest.rfind(" (push)")) {
            (Some(fetch), push) if push.is_none_or(|push| fetch > push) => (&rest[..fetch], false),
            (_, Some(push)) => (&rest[..push], true),
            _ => (rest, false),
        };
        if remotes.last().is_none_or(|last| last.name != name) {
            remotes.push(Remote {
                name: name.into(),
                fetch_url: String::new(),
                push_url: String::new(),
            });
        }
        let Some(remote) = remotes.last_mut() else {
            continue;
        };
        let slot = if push {
            &mut remote.push_url
        } else {
            &mut remote.fetch_url
        };
        if slot.is_empty() {
            *slot = url.into();
        }
    }
    remotes
}

/// Hashes what describes the repository itself: HEAD, references, local configuration, stashes
/// and the state of a running operation. Shared by the light and the full token.
fn repository_parts(path: &Path) -> Result<(u64, Head)> {
    let mut hasher = DefaultHasher::new();
    let h = &mut hasher;
    // References, configuration and stashes are only compared, so where Git's files can be
    // read their content is hashed as it is, without a process.
    let head = if let Some(files) = super::gitfacts::repository_files(path) {
        let head = read_head(path);
        head.hash(h);
        files.hash(h);
        head
    } else {
        repository_parts_from_git(path, h)?
    };
    let listed = super::gitfacts::markers(path);
    let listed = listed.as_ref();
    operation_in(path, listed)?.hash(h);
    for marker in [
        "MERGE_HEAD",
        "MERGE_MSG",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "sequencer/todo",
        "rebase-merge/git-rebase-todo",
        "rebase-apply/next",
        "BISECT_START",
    ] {
        marker_content(path, listed, marker)?.hash(h);
    }
    Ok((hasher.finish(), head))
}

/// The parts of [`repository_parts`] as Git prints them, for repositories whose files are not
/// read directly. Every load computes this twice (before and after), so the queries run side
/// by side; the results are hashed in a fixed order.
fn repository_parts_from_git(path: &Path, h: &mut DefaultHasher) -> Result<Head> {
    let (head, refs, config, stashes) = std::thread::scope(|scope| {
        let head = scope.spawn(|| read_head(path));
        let refs =
            scope.spawn(|| git(path, &["for-each-ref", "--format=%(refname) %(objectname)"]));
        let config = scope.spawn(|| git(path, &["config", "--local", "--list", "-z"]));
        let stashes = git(path, &["stash", "list", "--format=%H%x00%gd"]);
        (joined(head), joined(refs), joined(config), stashes)
    });
    let head = head?;
    head.hash(h);
    refs??.hash(h);
    config??.hash(h);
    stashes?.hash(h);
    Ok(head)
}

/// Files up to this size are hashed by content, until `CONTENT_BUDGET` is used up.
const CONTENT_LIMIT: u64 = 256 * 1024;
const CONTENT_BUDGET: u64 = 16 * 1024 * 1024;

/// Fingerprint of a changed file: size, modification time and, on Unix, change time (which a
/// restored modification time cannot hide). Small files are also hashed by content, so two edits
/// of the same size within one timestamp tick still differ; the budget keeps the cost bounded
/// when thousands of files changed. The result depends only on the file and the order of the
/// calls, never on the current time: a token must stay the same until something changes.
fn hash_metadata(file: &Path, h: &mut DefaultHasher, budget: &mut u64) {
    let meta = file.symlink_metadata().ok();
    let content = content_plan(meta.as_ref(), budget);
    hash_file(file, meta.as_ref(), content, h);
}

/// How much of a changed file's content goes into its fingerprint.
#[derive(Clone, Copy)]
enum Content {
    Nothing,
    Whole,
    /// A large file is not read whole, but its two ends are: an edit of the same size with a
    /// restored modification time (Windows has no change time to expose it) that touches
    /// either end still changes the token.
    Ends,
}

/// Takes what reading the file costs from `budget`, in the order of the calls.
fn content_plan(meta: Option<&std::fs::Metadata>, budget: &mut u64) -> Content {
    let Some(meta) = meta.filter(|meta| meta.is_file()) else {
        return Content::Nothing;
    };
    if meta.len() <= CONTENT_LIMIT {
        if meta.len() <= *budget {
            *budget -= meta.len();
            return Content::Whole;
        }
    } else if 2 * SAMPLE <= *budget {
        *budget -= 2 * SAMPLE;
        return Content::Ends;
    }
    Content::Nothing
}

fn hash_file(
    file: &Path,
    meta: Option<&std::fs::Metadata>,
    content: Content,
    h: &mut DefaultHasher,
) {
    let Some(meta) = meta else {
        return;
    };
    meta.len().hash(h);
    meta.modified().ok().hash(h);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.ctime(), meta.ctime_nsec()).hash(h);
    }
    match content {
        Content::Nothing => {}
        Content::Whole => std::fs::read(file).ok().hash(h),
        Content::Ends => sample_ends(file, meta.len()).ok().hash(h),
    }
}

/// [`hash_metadata`] of every file of `names` in the work tree `path`, in that order. A status
/// can list thousands of files, and one after the other each costs a few dozen microseconds
/// on Windows, so they are looked at on several threads; the budget is still taken in order.
fn hash_changed(path: &Path, names: &[String], h: &mut DefaultHasher) {
    let files: Vec<PathBuf> = names.iter().map(|name| path.join(name)).collect();
    let metas = parallel_map(&files, |file| file.symlink_metadata().ok());
    let mut budget = CONTENT_BUDGET;
    let jobs: Vec<_> = files
        .iter()
        .zip(&metas)
        .map(|(file, meta)| (file, meta, content_plan(meta.as_ref(), &mut budget)))
        .collect();
    let prints = parallel_map(&jobs, |(file, meta, content)| {
        let mut h = DefaultHasher::new();
        hash_file(file, meta.as_ref(), *content, &mut h);
        h.finish()
    });
    for (name, print) in names.iter().zip(prints) {
        name.hash(h);
        print.hash(h);
    }
}

/// `items.iter().map(f).collect()`, on up to eight threads for long lists.
pub(super) fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    const PER_THREAD: usize = 64;
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get().min(8))
        .min(items.len().div_ceil(PER_THREAD));
    if threads <= 1 {
        return items.iter().map(f).collect();
    }
    let f = &f;
    std::thread::scope(|scope| {
        let workers: Vec<_> = items
            .chunks(items.len().div_ceil(threads))
            .map(|chunk| scope.spawn(move || chunk.iter().map(f).collect::<Vec<R>>()))
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

/// How much of the start and of the end of a large file goes into the token.
const SAMPLE: u64 = 64 * 1024;

fn sample_ends(file: &Path, len: u64) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(file)?;
    let mut read = |from: u64| -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        f.seek(SeekFrom::Start(from))?;
        (&mut f).take(SAMPLE).read_to_end(&mut buf)?;
        Ok(buf)
    };
    Ok((read(0)?, read(len.saturating_sub(SAMPLE))?))
}

/// Cheap token for the part of the state a snapshot reads (references, stashes, remotes,
/// operation, conflicts). It does not look at working files, so it costs a few processes and no
/// scan of the work tree. `snapshot` uses it to notice a change while it was loading.
pub(super) fn light_token(path: &Path) -> Result<String> {
    light_parts(path).map(|light| light.token)
}

/// The light token together with what was read for it, so a caller that needs those parts
/// does not start the Git processes behind them twice.
struct Light {
    token: String,
    /// The `repository_hash` inside the token.
    repository: u64,
    head: Head,
}

fn light_parts(path: &Path) -> Result<Light> {
    let (repository, head) = repository_parts(path)?;
    let mut h = DefaultHasher::new();
    repository.hash(&mut h);
    if let Some(dir) = super::gitfacts::git_dir(path) {
        hash_metadata(&dir.join("index"), &mut h, &mut 0);
    }
    Ok(Light {
        token: format!("{:016x}", h.finish()),
        repository,
        head,
    })
}

/// The part of the token that does not need the work tree status: the repository state and
/// the index (`ls-files --stage -z`). It can be computed while `status` runs. `repository` is
/// the `repository_hash` read at the start of a load; if the state moved since, the final
/// check of the load fails.
fn token_base(repository: u64, index: &str) -> u64 {
    let mut h = DefaultHasher::new();
    repository.hash(&mut h);
    index.hash(&mut h);
    h.finish()
}

/// The work tree status for the token, or `None` when a repository with content filters must
/// not run `status` (even status may execute a configured clean filter). Filter repositories
/// are browsable, but working-tree mutations are refused by worktree_guard.
fn current_status(path: &Path) -> Result<(Option<String>, Early)> {
    status_unless(path, has_filters(path))
}

/// In WSL: the fingerprint (`gitexec::fingerprint`) of the paths the last token of the work
/// tree listed, read while the status for the next token runs. [`token_finish`] uses it when
/// the status lists the same paths, so the token needs no round trip after the status (2 000
/// changed files took 100 ms). It is handed along with its own status, never shared, so a
/// token only uses what was read next to the status it hashes.
type Early = Option<(Vec<String>, Vec<u8>)>;

/// `current_status` for a caller that already knows whether the repository has filters.
fn status_unless(path: &Path, filtered: bool) -> Result<(Option<String>, Early)> {
    if filtered {
        return Ok((None, None));
    }
    let hint = super::gitexec::place(path).and(super::gitexec::last_changed(path));
    std::thread::scope(|scope| {
        let early = hint.map(|names| {
            scope.spawn(move || super::gitexec::fingerprint(path, &names).map(|stat| (names, stat)))
        });
        let status = status(path);
        let early = early.and_then(|early| early.join().ok().flatten());
        Ok((Some(status?), early))
    })
}

/// Completes a token from `token_base` and the status read at about the same time.
/// A changed file is recognised by its size and modification time; its content is not read, so
/// the cost does not grow with the size of the changes.
fn token_finish(path: &Path, base: u64, status: Option<&str>, early: Early) -> Result<String> {
    let mut h = DefaultHasher::new();
    base.hash(&mut h);
    let changed: Vec<String> = match status {
        // `status --untracked-files=all` lists tracked changes and untracked files in one pass.
        Some(status) => {
            status.hash(&mut h);
            super::gitgraph::status_entries(status)
                .into_iter()
                .map(|entry| entry.name.to_owned())
                .collect()
        }
        // Without status, list untracked files directly (this does not run content filters).
        None => checked_text(git(
            path,
            &["ls-files", "--others", "--exclude-standard", "-z"],
        )?)?
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect(),
    };
    // In WSL the times and sizes come from one `stat` in the distribution (see `gitexec`),
    // read next to the status when it lists the same paths as last time (see `Early`).
    let early = early
        .filter(|(names, _)| *names == changed)
        .map(|(_, stat)| stat);
    if let Some(stat) = early.or_else(|| super::gitexec::fingerprint(path, &changed)) {
        changed.hash(&mut h);
        stat.hash(&mut h);
        super::gitexec::remember_changed(path, changed);
    } else {
        hash_changed(path, &changed, &mut h);
    }
    Ok(format!("{:016x}", h.finish()))
}

/// Token covers references, index, tracked changes and untracked file metadata.
/// External Git clients must still be treated as concurrent writers after validation.
fn token(path: &Path) -> Result<String> {
    token_seen(path).map(|(token, _)| token)
}

/// The token together with what was read for it.
fn token_seen(path: &Path) -> Result<(String, Seen)> {
    let (repository, head) = repository_parts(path)?;
    let (index, status) = std::thread::scope(|scope| {
        let index = scope.spawn(|| git(path, &["ls-files", "--stage", "-z"]));
        let status = current_status(path);
        (joined(index), status)
    });
    let (index, (status, early)) = (index??, status?);
    let token = token_finish(
        path,
        token_base(repository, &index),
        status.as_deref(),
        early,
    )?;
    let seen = Seen {
        path: path.to_path_buf(),
        head: head.id,
        index,
        status,
        prefetched: Prefetched::default(),
    };
    Ok((token, seen))
}

/// Runs `read` with the light token taken before it (and what was read for it) and checks the
/// token again afterwards. A load that passes the check may show the HEAD read for the token. Another Git client rewriting the index or a reference while loading
/// is common, so one change is retried; a second one in a row is reported.
///
/// `prepare` reads what does not change between attempts (the effective configuration); it runs
/// once, alongside the first token.
fn load_consistently<P: Send, T>(
    p: &Path,
    prepare: impl FnOnce() -> P + Send,
    read: impl Fn(&Light, &P) -> Result<T>,
) -> Result<T> {
    let (first, prepared) = std::thread::scope(|scope| {
        let prepared = scope.spawn(prepare);
        (light_parts(p), joined(prepared))
    });
    let (prepared, mut first) = (prepared?, Some(first));
    for attempt in 0..2 {
        let before = match first.take() {
            Some(parts) => parts?,
            None => light_parts(p)?,
        };
        let value = read(&before, &prepared)?;
        if before.token == light_token(p)? {
            return Ok(value);
        }
        if attempt == 1 {
            break;
        }
    }
    Err(fail(
        "The repository changed while loading. Refresh to review its current state",
    ))
}

/// Reads the repository state without a token, and the index (`ls-files --stage -z`) the
/// token needs; the conflicts come from that listing. The Git queries run concurrently.
fn read_state(p: &Path, head: &Head) -> Result<(Repository, String)> {
    let listed = super::gitfacts::markers(p);
    let operation = operation_in(p, listed.as_ref())?;
    std::thread::scope(|scope| {
        let index = scope.spawn(|| git(p, &["ls-files", "--stage", "-z"]));
        let refs = scope.spawn(|| refs(p));
        let stashes = scope.spawn(|| stashes(p));
        let remotes = scope.spawn(|| remotes(p));
        // Only during an operation, which is rare.
        let operation_commit = operation
            .is_some()
            .then(|| scope.spawn(|| operation_commit(p)));
        let index = joined(index)??;
        let repository = Repository {
            root: p.to_string_lossy().into_owned(),
            head: head.id.clone(),
            branch: head.branch.clone(),
            refs: joined(refs)??,
            stashes: joined(stashes)??,
            remotes: joined(remotes)??,
            operation,
            operation_commit: operation_commit.map(joined).transpose()?.flatten(),
            edit_stop: super::gitrewrite::is_edit_stop(p),
            bisect_active: marker_exists(p, listed.as_ref(), "BISECT_START")?,
            conflicts: unmerged(&index)?,
            token: String::new(),
        };
        Ok((repository, index))
    })
}

/// Brings the commit-graph and the index of the work tree up to date and waits for it. The
/// Git views do this in the background when they open (see `gitfacts::keep_commit_graph`);
/// this is for a caller that wants the result first, such as the speed test.
pub fn prepare_history(path: &Path) -> Result<()> {
    super::gitfacts::write_commit_graph(&root(path)?);
    Ok(())
}

pub fn snapshot(path: &Path) -> Result<Repository> {
    let p = root(path)?;
    load_consistently(
        &p,
        || has_filters(&p),
        |light, &filtered| {
            let (state, status) = std::thread::scope(|scope| {
                let status = scope.spawn(|| status_unless(&p, filtered));
                let state = read_state(&p, &light.head);
                (state, joined(status))
            });
            let (mut state, index) = state?;
            let base = token_base(light.repository, &index);
            let (status, early) = status??;
            state.token = token_finish(&p, base, status.as_deref(), early)?;
            Ok(state)
        },
    )
}

/// What a remote operation checks before it starts.
pub(super) struct Checked {
    /// The same value [`snapshot`] returns as its token.
    pub token: String,
    pub remotes: Vec<Remote>,
    pub operation: Option<String>,
    pub head: String,
    pub branch: Option<String>,
    /// Empty unless asked for.
    pub refs: Vec<Reference>,
}

/// The parts of a [`snapshot`] a fetch or push looks at, with fewer processes: no stash
/// list, operation commit or conflicts, and the references only when `with_refs` (their
/// sorted listing with upstream counts is one of the slowest queries). Everything runs
/// side by side.
pub(super) fn checked(path: &Path, with_refs: bool) -> Result<Checked> {
    let p = root(path)?;
    // Another Git client rewriting the index or a reference meanwhile is common: one more
    // try, as `load_consistently` does for a snapshot.
    match checked_once(&p, with_refs) {
        Err(CoreError::Other(message)) if message == CHANGED_WHILE_LOADING => {
            checked_once(&p, with_refs)
        }
        result => result,
    }
}

const CHANGED_WHILE_LOADING: &str =
    "The repository changed while loading. Refresh to review its current state";

fn checked_once(p: &Path, with_refs: bool) -> Result<Checked> {
    let p = p.to_path_buf();
    let light = light_parts(&p)?;
    let (index, status, remotes, refs) = std::thread::scope(|scope| {
        let remotes = scope.spawn(|| remotes(&p));
        let refs = with_refs.then(|| scope.spawn(|| refs(&p)));
        let index = scope.spawn(|| git(&p, &["ls-files", "--stage", "-z"]));
        let status = current_status(&p);
        (
            joined(index),
            status,
            joined(remotes),
            refs.map(joined).transpose(),
        )
    });
    let (index, (status, early), remotes) = (index??, status?, remotes??);
    let refs = refs?.transpose()?.unwrap_or_default();
    // References read next to the token must belong to the state it describes.
    if with_refs && light.token != light_token(&p)? {
        return Err(fail(CHANGED_WHILE_LOADING));
    }
    Ok(Checked {
        token: token_finish(
            &p,
            token_base(light.repository, &index),
            status.as_deref(),
            early,
        )?,
        remotes,
        operation: operation(&p)?,
        head: light.head.id,
        branch: light.head.branch,
        refs,
    })
}

/// What the graph view shows: history and repository state, read in one pass. The work tree
/// status is read once and shared by the list of changes and the token.
#[derive(Serialize)]
pub struct GraphView {
    pub graph: super::gitgraph::Graph,
    pub repository: Repository,
}

pub fn graph_view(
    path: &Path,
    comparison: bool,
    reflog: bool,
    limit: usize,
    reference: Option<&str>,
) -> Result<GraphView> {
    let p = root(path)?;
    // The filter lookup runs next to the first token instead of before it.
    load_consistently(
        &p,
        || has_filters(&p),
        |light, &filtered| {
            let (graph, branches, state, status, escape) = std::thread::scope(|scope| {
                let status = scope.spawn(|| status_unless(&p, filtered));
                let state = scope.spawn(|| read_state(&p, &light.head));
                let branches = scope.spawn(|| super::gitgraph::branches(&p));
                // Only the list of changes needs it, and nothing waits for it.
                let escape = (!comparison && !filtered)
                    .then(|| scope.spawn(|| super::gitgraph::quote_non_ascii(&p)));
                let graph = super::gitgraph::history_part(
                    &p,
                    reflog,
                    limit,
                    reference,
                    Some(&light.head.id),
                );
                (
                    graph,
                    joined(branches),
                    joined(state),
                    joined(status),
                    escape.map(joined),
                )
            });
            let (mut graph, (mut state, index)) = (graph?, state??);
            graph.branches = branches??;
            graph.read_only_reason = super::gitgraph::read_only_reason(filtered);
            let (status, early) = status??;
            let base = token_base(light.repository, &index);
            state.token = token_finish(&p, base, status.as_deref(), early)?;
            if let (Some(status), Some(escape)) = (&status, escape) {
                // Git escapes non-ASCII names unless told otherwise; a failed lookup means that.
                graph.changes = super::gitgraph::changes_text(status, escape.unwrap_or(true));
            }
            Ok(GraphView {
                graph,
                repository: state,
            })
        },
    )
}

fn commit_id(path: &Path, value: &str) -> Result<()> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(fail("Select a concrete commit before this operation"));
    }
    // The HEAD the token check read is a commit: creating a branch or tag there is the
    // usual case, and needs no process.
    let known = |seen: &Seen| {
        (seen.head == value || seen.prefetched.commits.iter().any(|id| id == value)).then_some(())
    };
    if seen(path, known).is_some() {
        return Ok(());
    }
    let resolved = git(
        path,
        &["rev-parse", "--verify", &format!("{value}^{{commit}}")],
    )?;
    if resolved.trim() != value {
        return Err(fail("The selected object is not a commit"));
    }
    Ok(())
}

/// The most commits one cherry-pick takes; a longer list is a rebase or a merge.
const MAX_PICKS: usize = 200;

/// Checks the commits of one cherry-pick with one process for the commits and one for their
/// files, whatever their number: full IDs, no repeats, no merges (Git would stop in the middle
/// for a parent to choose), and no untracked file in the way of any of them.
// The legacy spelling also works with Git versions predating --empty=keep.
const KEEP_EMPTY_PICKS: &str = "--keep-redundant-commits";

fn check_picks(path: &Path, targets: &[String]) -> Result<()> {
    if targets.is_empty() || targets.len() > MAX_PICKS {
        return Err(fail(format!(
            "Select between 1 and {MAX_PICKS} commits to cherry-pick"
        )));
    }
    let unique: BTreeSet<&str> = targets.iter().map(String::as_str).collect();
    if unique.len() != targets.len() {
        return Err(fail("A commit is selected twice"));
    }
    if targets.iter().any(|id| {
        !matches!(id.len(), 40 | 64)
            || !id
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    }) {
        return Err(fail("Select concrete commits before this operation"));
    }
    let (listed, touched, others) = std::thread::scope(|scope| {
        let ids = || targets.iter().map(String::as_str);
        let listed = scope.spawn(move || {
            let mut args = vec!["rev-list", "--no-walk=unsorted", "--parents"];
            args.extend(ids());
            git(path, &args)
        });
        let touched = scope.spawn(move || {
            let mut args = vec![
                "log",
                "--no-walk=unsorted",
                "--format=",
                "--name-only",
                "--no-renames",
                "--root",
                "--ignore-submodules=none",
                "--diff-filter=AMT",
                "-z",
            ];
            args.extend(ids());
            git(path, &args).and_then(checked_text)
        });
        let others = git(path, &["ls-files", "--others", "-z"]).and_then(checked_text);
        (joined(listed), joined(touched), others)
    });
    let (listed, touched, others) = (listed??, touched??, others?);
    let commits: Vec<Vec<&str>> = listed
        .lines()
        .map(|line| line.split_whitespace().collect())
        .collect();
    if commits.len() != targets.len()
        || commits
            .iter()
            .zip(targets)
            .any(|(commit, id)| commit.first() != Some(&id.as_str()))
    {
        return Err(fail("A selected object is not a commit"));
    }
    if let Some(merge) = commits.iter().find(|commit| commit.len() > 2) {
        return Err(fail(format!(
            "Commit {} is a merge. Select it by itself and choose a mainline parent",
            &merge[0][..8]
        )));
    }
    // Names with the folders above them, so a file in place of a folder is found too.
    let fold = |name: &str| {
        if cfg!(any(windows, target_os = "macos")) {
            name.to_lowercase()
        } else {
            name.to_owned()
        }
    };
    let touched: BTreeSet<String> = touched
        .split_terminator('\0')
        .filter(|name| !name.is_empty())
        .map(fold)
        .collect();
    for other in others.split('\0').filter(|name| !name.is_empty()) {
        let name = fold(other.trim_end_matches('/'));
        let inside = format!("{name}/");
        let blocked = touched.contains(&name)
            || touched
                .range(inside.clone()..)
                .next()
                .is_some_and(|file| file.starts_with(&inside))
            || name
                .match_indices('/')
                .any(|(at, _)| touched.contains(&name[..at]));
        if blocked {
            return Err(fail(format!(
                "Untracked or ignored file {other} would be overwritten; move or rename it first"
            )));
        }
    }
    Ok(())
}

fn validate_name(path: &Path, name: &str, kind: &str) -> Result<()> {
    if let Some(problem) = name_problem(name) {
        return Err(fail(format!(
            "Invalid {kind} name “{name}”: {problem}. Example: feature/my-change"
        )));
    }
    // Validate the literal ref, rather than allowing --branch to expand @{-1}.
    let full = match kind {
        "branch" => format!("refs/heads/{name}"),
        "tag" => format!("refs/tags/{name}"),
        "remote" => format!("refs/remotes/{name}/check"),
        _ => return Err(fail("Invalid reference kind")),
    };
    // The rules of git-check-ref-format(1) accept it: Git would too (a test compares the
    // two). Anything else is left to Git, so its verdict stays the one that counts.
    if super::gitfacts::plain_ref_name(&full) {
        return Ok(());
    }
    git(path, &["check-ref-format", &full])
    .map_err(|_| {
        fail(format!(
            "Git rejected the {kind} name “{name}”. Use letters, digits, “-”, “_”, “.” and “/” only, without spaces or special characters. Example: feature/my-change"
        ))
    })?;
    Ok(())
}

fn name_problem(name: &str) -> Option<String> {
    let forbidden = ['~', '^', ':', '?', '*', '[', '\\'];
    Some(if name.is_empty() {
        "the name is empty".into()
    } else if name.starts_with('-') {
        "it cannot start with “-”".into()
    } else if name.chars().any(char::is_whitespace) {
        "it cannot contain spaces; use “-” or “_” instead".into()
    } else if name.chars().any(char::is_control) {
        "it cannot contain control characters".into()
    } else if let Some(c) = name.chars().find(|c| forbidden.contains(c)) {
        format!("it cannot contain “{c}”")
    } else if name.contains("..") || name.contains("@{") || name == "@" {
        "it cannot contain “..” or “@{”, or be just “@”".into()
    } else if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        "“/” cannot start or end the name or appear twice in a row".into()
    } else if name.ends_with('.')
        || name.ends_with(".lock")
        || name.split('/').any(|p| p.starts_with('.'))
    {
        "it cannot end with “.” or “.lock”, or have a part starting with “.”".into()
    } else {
        return None;
    })
}

/// Checks that the `kind` (branch or tag) `name` still exists.
fn reference(path: &Path, kind: &str, name: &str) -> Result<()> {
    validate_name(path, name, kind)?;
    // Branches read from Git's files list what `for-each-ref refs/heads/` lists, by exact
    // name; where the files cannot answer, Git is asked.
    let files = (kind == "branch").then(|| super::gitfacts::branches(path));
    let known = match files.flatten() {
        Some(names) => names.iter().any(|branch| branch == name),
        None => refs(path)?.iter().any(|r| r.kind == kind && r.name == name),
    };
    if !known {
        return Err(fail(format!("The selected {kind} no longer exists")));
    }
    Ok(())
}

fn remote_exists(path: &Path, name: &str) -> Result<()> {
    validate_name(path, name, "remote")?;
    if !remotes(path)?.iter().any(|r| r.name == name) {
        return Err(fail("The selected remote no longer exists"));
    }
    Ok(())
}

fn url(value: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('-')
        || value.chars().any(char::is_control)
        || value.starts_with("ext::")
    {
        return Err(fail(
            "A valid remote URL or local repository path is required",
        ));
    }
    Ok(())
}

fn require_head(path: &Path) -> Result<()> {
    if head(path).is_empty() {
        return Err(fail("Create the first commit before this operation"));
    }
    Ok(())
}

const HARD_RESET_RECOVERY: &str = "Gall recovery before hard reset";

/// Before a hard reset makes the submodules match the target: in every initialized submodule,
/// nested ones included, keep its HEAD and its index and tracked edits as recovery references,
/// then drop those edits so the checkout that follows cannot be refused by them. Untracked files
/// stay, and a checkout that would overwrite one fails instead. One Git process from Gall:
/// `foreach` visits the submodules itself.
fn reset_submodule_edits(path: &Path) -> Result<()> {
    let script = format!(
        "keep() {{ git update-ref --no-deref --create-reflog -m '{HARD_RESET_RECOVERY}' \"refs/gall/recovery/$1\" \"$1\"; }} && \
         head=$(git rev-parse -q --verify HEAD) && keep \"$head\" && \
         saved=$(git stash create '{HARD_RESET_RECOVERY}') && \
         {{ [ -z \"$saved\" ] || keep \"$saved\"; }} && \
         git -c submodule.recurse=false reset -q --hard"
    );
    git(
        path,
        &[
            "-c",
            "submodule.recurse=false",
            "submodule",
            "foreach",
            "--quiet",
            "--recursive",
            &script,
        ],
    )
    .map_err(|e| {
        fail(format!(
            "The local edits of a submodule could not be saved, so nothing was reset: {e}"
        ))
    })?;
    Ok(())
}

/// Check out in every initialized submodule, nested ones included, the commit the parent now
/// records. A commit missing there is fetched, without prompting for credentials.
fn update_submodules(path: &Path) -> Result<()> {
    git(
        path,
        &[
            "-c",
            "submodule.recurse=false",
            "submodule",
            "update",
            "--checkout",
            "--recursive",
        ],
    )
    .map_err(|e| {
        fail(format!(
            "The hard reset completed, but the submodules could not be checked out at its commits: {e}. Update them in Tools → Submodules"
        ))
    })?;
    Ok(())
}

/// A staged gitlink is the one change Git's autostash cannot reliably restore
/// without moving a submodule checkout. Check both index and HEAD modes to catch deletions.
pub(super) fn protect_staged_gitlinks(path: &Path) -> Result<()> {
    let changes = checked_text(git(
        path,
        &[
            "diff",
            "--cached",
            "--raw",
            "--no-renames",
            "--ignore-submodules=none",
            "-z",
            "--",
        ],
    )?)?;
    let mut records = changes.split_terminator('\0');
    while let (Some(metadata), Some(file)) = (records.next(), records.next()) {
        if metadata
            .trim_start_matches(':')
            .split_whitespace()
            .take(2)
            .any(|mode| mode == "160000")
        {
            return Err(fail(format!(
                "Rebase cannot recover the staged submodule pointer at {file}. Commit or unstage it in Working Changes first."
            )));
        }
    }
    Ok(())
}

pub(super) fn verify_filtered_autostash(path: &Path) -> Result<()> {
    if !has_filters(path) {
        return Ok(());
    }
    // Git will make its own autostash. This preflight also catches raw edits normalized
    // away by the filter, which Git would otherwise treat as a clean checkout.
    let recovery = git(path, &["stash", "create", "Gall rebase filter preflight"])?;
    let tree = match recovery.trim() {
        "" => "HEAD",
        id => id,
    };
    super::gitadvanced::verify_filtered_recovery(path, tree, "Rebase")
}

pub(super) fn protect_untracked(path: &Path, targets: &[&str], replay: Option<&str>) -> Result<()> {
    // What the token check already read (`SEEN` belongs to this thread, so it is looked up
    // before the workers start).
    let known_trees: Vec<Option<String>> = targets
        .iter()
        .map(|target| {
            seen(path, |seen| {
                let found = seen.prefetched.trees.iter().find(|(t, _)| t == target);
                found.map(|(_, listing)| listing.clone())
            })
        })
        .collect();
    let known_replay = replay.and_then(|target| {
        seen(path, |seen| {
            let found = seen.prefetched.replayed.as_ref();
            found
                .filter(|(t, _)| t == target)
                .map(|(_, log)| log.clone())
        })
    });
    let known_others = seen(path, |seen| seen.prefetched.others.clone());
    // The three listings do not depend on each other: asked side by side.
    let (other, trees, replayed) = std::thread::scope(|scope| {
        let trees = scope.spawn(|| -> Result<Vec<String>> {
            targets
                .iter()
                .zip(known_trees)
                .map(|(target, known)| match known {
                    Some(listing) => checked_text(listing),
                    None => {
                        checked_text(git(path, &["ls-tree", "-r", "--name-only", "-z", target])?)
                    }
                })
                .collect()
        });
        let replayed = scope.spawn(|| -> Result<Option<String>> {
            let Some(target) = replay else {
                return Ok(None);
            };
            if let Some(log) = known_replay {
                return checked_text(log).map(Some);
            }
            // Rebase can temporarily restore a path deleted again by a later commit.
            // Include every replayed path, not only the final HEAD tree.
            checked_text(git(
                path,
                &[
                    "log",
                    "--format=",
                    "--raw",
                    "-z",
                    "--no-renames",
                    "--no-abbrev",
                    &format!("{target}..HEAD"),
                ],
            )?)
            .map(Some)
        });
        let other = match known_others {
            Some(listing) => checked_text(listing),
            None => git(path, &["ls-files", "--others", "-z"]).and_then(checked_text),
        };
        (other, joined(trees), joined(replayed))
    });
    let other = other?;
    if other.is_empty() {
        return Ok(());
    }
    let mut touched = BTreeSet::new();
    for tree in trees?? {
        for name in tree.split('\0').filter(|name| !name.is_empty()) {
            touched.insert(name.to_owned());
        }
    }
    if let Some(raw) = replayed?? {
        let mut next_is_path = false;
        for record in raw.split('\0') {
            if next_is_path {
                touched.insert(record.to_owned());
                next_is_path = false;
            } else if record.trim_start_matches('\n').starts_with(':') {
                next_is_path = true;
            }
        }
    }
    protect_touched_paths(&other, &touched)
}

/// Protect only paths that the two-tree checkout phase of reset --keep can change.
pub(super) fn protect_reset_keep_paths(path: &Path, from: &str, target: &str) -> Result<()> {
    let other = checked_text(git(path, &["ls-files", "--others", "-z"])?)?;
    if other.is_empty() {
        return Ok(());
    }
    let changes = checked_text(git(
        path,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "--no-renames",
            "--ignore-submodules=none",
            "-r",
            "-z",
            from,
            target,
            "--",
        ],
    )?)?;
    let touched = changes.split_terminator('\0').map(str::to_owned).collect();
    protect_touched_paths(&other, &touched)
}

fn protect_integration_writes(path: &Path, before: &str, after: &str) -> Result<()> {
    let others = match seen(path, |seen| seen.prefetched.others.clone()) {
        Some(listing) => checked_text(listing)?,
        None => checked_text(git(path, &["ls-files", "--others", "-z"])?)?,
    };
    if others.is_empty() {
        return Ok(());
    }
    let names = checked_text(git(
        path,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--ignore-submodules=none",
            "--diff-filter=AMT",
            before,
            after,
            "--",
        ],
    )?)?;
    let touched = names.split_terminator('\0').map(str::to_owned).collect();
    protect_touched_paths(&others, &touched)
}

fn protect_touched_paths(other: &str, touched: &BTreeSet<String>) -> Result<()> {
    for name in other.split('\0').filter(|name| !name.is_empty()) {
        let mut parent = name;
        let ancestor = loop {
            if touched.contains(parent) {
                break true;
            }
            match parent.rsplit_once('/') {
                Some((value, _)) => parent = value,
                None => break false,
            }
        };
        let prefix = format!("{name}/");
        let descendant = touched
            .range(prefix.clone()..)
            .next()
            .is_some_and(|item| item.starts_with(&prefix));
        if ancestor || descendant {
            return Err(fail(format!("An ignored or untracked file would be overwritten: {name}. Move it before this operation")));
        }
    }
    Ok(())
}

fn no_operation(path: &Path) -> Result<()> {
    if operation(path)?.is_some() || !conflicts(path)?.is_empty() {
        return Err(fail("Finish or abort the current Git operation first"));
    }
    Ok(())
}

/// Keeps HEAD as a recovery reference and returns it (empty when there is none).
fn save_head(path: &Path) -> Result<String> {
    let id = head(path);
    if id.is_empty() {
        return Ok(id);
    }
    git(
        path,
        &[
            "update-ref",
            "--no-deref",
            &format!("refs/gall/recovery/{id}"),
            &id,
        ],
    )?;
    Ok(id)
}

fn save_index_and_worktree(path: &Path, message: &str, verify_filtered: bool) -> Result<()> {
    // `stash create` stores index and tracked working content without cleaning
    // the working tree or changing the user's stash stack.
    let id = git(path, &["stash", "create", message])?;
    let id = id.trim();
    if verify_filtered {
        let tree = if id.is_empty() { "HEAD" } else { id };
        super::gitadvanced::verify_filtered_recovery(path, tree, "Hard reset")?;
    }
    if !id.is_empty() {
        super::gitgraph::retain_recovery(path, id, message)?;
    }
    Ok(())
}

fn stash(path: &Path, id: &str) -> Result<Stash> {
    commit_id(path, id)?;
    stashes(path)?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| fail("The selected stash no longer exists; refresh the stash list"))
}

fn drop_stash(path: &Path, id: &str) -> Result<String> {
    let item = stash(path, id)?;
    // Retain the immutable stash object before removing its stack entry.
    git(
        path,
        &[
            "update-ref",
            "--no-deref",
            &format!("refs/gall/recovery/{id}"),
            id,
        ],
    )?;
    git(path, &["stash", "drop", &item.selector])
}

fn rename_stash(path: &Path, id: &str, name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(fail("Enter a nonempty, single-line stash name"));
    }
    let stack = stashes(path)?;
    let index = stack
        .iter()
        .position(|item| item.id == id)
        .ok_or_else(|| fail("The selected stash no longer exists; refresh the stash list"))?;
    let previous = &stack[index];
    let branch = previous.message.split_once(": ").and_then(|(prefix, _)| {
        prefix
            .strip_prefix("On ")
            .or_else(|| prefix.strip_prefix("WIP on "))
    });
    let label = branch.map_or_else(|| name.to_owned(), |branch| format!("On {branch}: {name}"));
    if previous.message == label {
        return Ok(String::new());
    }
    commit_id(path, id)?;
    let renamed = super::gitrewrite::rewrite_commit_object(
        path,
        id,
        &std::collections::HashMap::new(),
        Some(&label),
        None,
    )?;
    if renamed == id {
        return Err(fail("The stash already contains that commit message"));
    }
    super::gitgraph::retain_commit(path, id, "Gall recovery before renaming stash")?;
    git(path, &["stash", "store", "-m", &label, &renamed])?;
    let current = stashes(path)?;
    if current.len() != stack.len() + 1
        || current.first().is_none_or(|item| item.id != renamed)
        || current[1..]
            .iter()
            .zip(&stack)
            .any(|(now, before)| now.id != before.id || now.message != before.message)
    {
        return Err(fail("The stash list changed while renaming. Both versions are retained; refresh the stash list"));
    }
    git(path, &["stash", "drop", &current[index + 1].selector])
}

fn abort_merge(path: &Path) -> Result<String> {
    let others = checked_text(git(path, &["ls-files", "--others", "-z"])?)?;
    if !others.is_empty() {
        // Native merge abort resets the index to HEAD via reset --merge.
        let changes = checked_text(git(
            path,
            &[
                "diff",
                "--cached",
                "--name-only",
                "--no-renames",
                "--ignore-submodules=none",
                "-z",
                "HEAD",
                "--",
            ],
        )?)?;
        let touched = changes.split_terminator('\0').map(str::to_owned).collect();
        protect_touched_paths(&others, &touched)?;
        // An externally started merge may also reapply an autostash.
        if let Ok(stash) = git(path, &["rev-parse", "-q", "--verify", "MERGE_AUTOSTASH"]) {
            let stash = stash.trim();
            protect_integration_writes(path, &format!("{stash}^"), stash)?;
        }
    }
    git(path, &["-c", "submodule.recurse=false", "merge", "--abort"])
}

fn configured_git_filters(path: &Path) -> Option<bool> {
    let config = checked_text(git(path, &["config", "--null", "--list"]).ok()?).ok()?;
    Some(config.split_terminator('\0').any(|entry| {
        entry
            .split_once('\n')
            .map_or(entry, |(key, _)| key)
            .to_ascii_lowercase()
            .starts_with("filter.")
    }))
}

/// A newly staged attributes file can make byte-identical files appear modified. On the
/// final rebase step, with no autostash or remaining commands, Git only commits the index
/// and updates refs. Use the pre-commit attributes, but only after proving the
/// apparently modified regular files still match their staged blobs byte for byte.
fn attribute_only_rebase_source(path: &Path) -> Option<String> {
    let todo =
        std::fs::read_to_string(git_path(path, "rebase-merge/git-rebase-todo").ok()?).ok()?;
    if !todo.trim().is_empty()
        || git_path(path, "rebase-merge/autostash").ok()?.exists()
        || git_path(path, "rebase-merge/update-refs").ok()?.exists()
    {
        return None;
    }
    // GIT_ATTR_SOURCE affects every tracked path, including paths outside the diff.
    // An old attribute could otherwise run a configured clean/process filter there.
    if configured_git_filters(path)? {
        return None;
    }
    let staged = checked_text(
        git(
            path,
            &[
                "diff",
                "--cached",
                "--name-only",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "-z",
                "--",
            ],
        )
        .ok()?,
    )
    .ok()?;
    if !staged
        .split_terminator('\0')
        .any(|name| name == ".gitattributes" || name.ends_with("/.gitattributes"))
    {
        return None;
    }
    let source = git(path, &["rev-parse", "--verify", "HEAD^{commit}"])
        .ok()?
        .trim()
        .to_owned();
    let staged_before = git(
        path,
        &[
            "diff",
            "--cached",
            "--raw",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "-z",
            "--",
        ],
    )
    .ok()?;
    let names = checked_text(
        git(
            path,
            &[
                "diff",
                "--name-only",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--ignore-submodules=all",
                "--",
            ],
        )
        .ok()?,
    )
    .ok()?;
    let files: Vec<_> = names.split_terminator('\0').collect();
    if files.is_empty() || files.len() > 512 || names.len() > 100_000 {
        return None;
    }
    // Batch the Git reads, keeping command lines short enough for Windows. Raw object
    // hashes preserve binary bytes and work in both SHA-1 and SHA-256 repositories.
    for batch in files.chunks(16) {
        if batch.iter().any(|file| file.len() > 1000) {
            return None;
        }
        // Do not invoke filters, follow symlinks, or let a changed mode pass as clean.
        let mut attr_args = vec!["check-attr", "-z", "filter", "--"];
        attr_args.extend(batch.iter().copied());
        for attrs in [
            git(path, &attr_args),
            super::gitgraph::git_with_attr_source(path, &attr_args, &source),
        ] {
            let attrs = checked_text(attrs.ok()?).ok()?;
            let parts: Vec<_> = attrs.split_terminator('\0').collect();
            if parts.len() != batch.len() * 3
                || parts.as_chunks::<3>().0.iter().any(|entry| {
                    entry[1] != "filter" || !matches!(entry[2], "unspecified" | "unset")
                })
            {
                return None;
            }
        }
        for file in batch {
            let work = checked_work_path(path, file).ok()?;
            if !std::fs::symlink_metadata(work).ok()?.file_type().is_file() {
                return None;
            }
        }
        let mut hash_args = vec!["hash-object", "--no-filters", "--"];
        hash_args.extend(batch.iter().copied());
        let hashes = git(path, &hash_args).ok()?;
        let hashes: Vec<_> = hashes.lines().collect();
        if hashes.len() != batch.len() {
            return None;
        }
        let literals: Vec<_> = batch
            .iter()
            .map(|file| format!(":(literal){file}"))
            .collect();
        let mut index_args = vec!["ls-files", "--stage", "-z", "--"];
        index_args.extend(literals.iter().map(String::as_str));
        let entries = checked_text(git(path, &index_args).ok()?).ok()?;
        let mut indexed = std::collections::BTreeMap::new();
        for record in entries.split_terminator('\0') {
            let (meta, name) = record.split_once('\t')?;
            let mut fields = meta.split_whitespace();
            let mode = fields.next()?;
            let oid = fields.next()?;
            let stage = fields.next()?;
            if !matches!(mode, "100644" | "100755")
                || stage != "0"
                || fields.next().is_some()
                || indexed.insert(name, oid).is_some()
            {
                return None;
            }
        }
        if batch
            .iter()
            .zip(hashes)
            .any(|(file, hash)| indexed.get(file) != Some(&hash))
        {
            return None;
        }
    }
    // This also proves this Git supports GIT_ATTR_SOURCE: otherwise the same diff remains dirty.
    super::gitgraph::git_with_attr_source(
        path,
        &[
            "-c",
            "core.safecrlf=false",
            "diff",
            "--quiet",
            "--no-textconv",
            "--no-ext-diff",
            "--no-renames",
            "--ignore-submodules=dirty",
            "--",
        ],
        &source,
    )
    .ok()?;
    // An external Git client may have changed the operation while preflight ran.
    if git(path, &["rev-parse", "--verify", "HEAD^{commit}"])
        .ok()?
        .trim()
        != source
        || git(
            path,
            &[
                "diff",
                "--cached",
                "--raw",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "-z",
                "--",
            ],
        )
        .ok()?
            != staged_before
        || !std::fs::read_to_string(git_path(path, "rebase-merge/git-rebase-todo").ok()?)
            .ok()?
            .trim()
            .is_empty()
        || git_path(path, "rebase-merge/autostash").ok()?.exists()
        || git_path(path, "rebase-merge/update-refs").ok()?.exists()
    {
        return None;
    }
    Some(source)
}

/// Keep Git's reason visible first. The post-failure worktree can reflect partial progress,
/// so the current paths are context, not necessarily the cause of that failure.
fn rebase_failure_context(error: CoreError, files: &[&str]) -> CoreError {
    let paths = files
        .iter()
        .take(20)
        .map(|file| format!("{file:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let more = if files.len() > 20 {
        format!(" (+{} more)", files.len() - 20)
    } else {
        String::new()
    };
    fail(format!(
        "Git: {error}\n\nCurrent unstaged files: {paths}{more}. Review them in Working Changes."
    ))
}

fn control(path: &Path, command: &str) -> Result<String> {
    // Continue, skip and abort change the repository through other modules, which the
    // local `git` does not see: from here on every question is asked again.
    SEEN.with(|slot| *slot.borrow_mut() = None);
    let current = operation(path)?.ok_or_else(|| fail("There is no interrupted Git operation"))?;
    if command == "--continue" && !conflicts(path)?.is_empty() {
        return Err(fail("Resolve and stage all conflicts before continuing"));
    }
    // Finalizing a resolved merge commits the index without checking out any files.
    if current == "merge" {
        match command {
            "--continue" => return git(path, &["-c", "core.editor=true", "commit", "--no-edit"]),
            "--abort" => return abort_merge(path),
            _ => return Err(fail("A merge cannot be skipped; continue or abort it")),
        }
    }
    let mut targets = vec![head(path)];
    if command != "--abort" {
        targets.extend(super::gitrewrite::pending_commits(path)?);
    }
    if command == "--abort" {
        for marker in [
            "rebase-merge/orig-head",
            "rebase-apply/orig-head",
            "sequencer/head",
        ] {
            if let Ok(id) = std::fs::read_to_string(git_path(path, marker)?) {
                let id = id.trim();
                commit_id(path, id)?;
                targets.push(id.to_owned());
            }
        }
    } else {
        for marker in ["rebase-merge/git-rebase-todo", "sequencer/todo"] {
            if let Ok(todo) = std::fs::read_to_string(git_path(path, marker)?) {
                for line in todo.lines() {
                    let mut parts = line.split_whitespace();
                    if matches!(
                        parts.next(),
                        Some(
                            "pick"
                                | "p"
                                | "revert"
                                | "reword"
                                | "r"
                                | "edit"
                                | "e"
                                | "squash"
                                | "s"
                                | "fixup"
                                | "f"
                        )
                    ) {
                        if let Some(id) = parts
                            .next()
                            .filter(|id| id.len() >= 7 && id.bytes().all(|c| c.is_ascii_hexdigit()))
                        {
                            let full = git(
                                path,
                                &["rev-parse", "--verify", &format!("{id}^{{commit}}")],
                            )?;
                            targets.push(full.trim().into());
                        }
                    }
                }
            }
        }
    }
    let trees: Vec<_> = targets
        .iter()
        .filter(|id| !id.is_empty())
        .map(String::as_str)
        .collect();
    protect_untracked(path, &trees, None)?;
    if let Some(result) = super::gitrewrite::resume(
        path,
        command,
        &std::env::current_exe().map_err(|e| fail(e.to_string()))?,
    ) {
        return result;
    }
    let args = [
        "-c",
        "core.editor=true",
        "-c",
        "submodule.recurse=false",
        &current,
        command,
    ];
    // Decide before executing Git: retrying after a partial sequence could skip a new stop.
    let source = if current == "rebase" && command == "--continue" {
        attribute_only_rebase_source(path)
    } else {
        None
    };
    let result = match source {
        Some(source) => super::gitgraph::git_with_attr_source(path, &args, &source),
        None => git(path, &args),
    };
    match result {
        Err(error) if current == "rebase" && command == "--continue" => {
            // The operation may have stopped for another reason or after partial progress.
            // Report the current unstaged paths without claiming they caused the error.
            let names = git(
                path,
                &[
                    "diff",
                    "--name-only",
                    "-z",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--ignore-submodules=dirty",
                    "--",
                ],
            )
            .ok()
            .and_then(|value| checked_text(value).ok());
            let files: Vec<_> = names
                .as_deref()
                .unwrap_or("")
                .split_terminator('\0')
                .collect();
            if files.is_empty() {
                return Err(error);
            }
            Err(rebase_failure_context(error, &files))
        }
        result => result,
    }
}

/// Execute one explicitly selected local action. Commits are unsigned, with hooks disabled.
/// What the journal calls a request; None for one that does not move HEAD.
fn journaled(request: &Request) -> Option<(super::gitjournal::Kind, String)> {
    use super::gitjournal::Kind;
    let short = |id: &str| id.get(..8).unwrap_or(id).to_owned();
    Some(match request {
        Request::Merge { target } | Request::MergeWithMode { target, .. } => {
            (Kind::Merge, short(target))
        }
        Request::Rebase { target } => (Kind::Rebase, short(target)),
        Request::CherryPick { target } => (Kind::CherryPick, short(target)),
        Request::CherryPickMany { targets } => (
            Kind::CherryPick,
            match targets.as_slice() {
                [one] => short(one),
                all => format!("{} commits", all.len()),
            },
        ),
        Request::IntegrateMerge { target, revert, .. } => (
            if *revert {
                Kind::Revert
            } else {
                Kind::CherryPick
            },
            short(target),
        ),
        Request::Revert { target } => (Kind::Revert, short(target)),
        Request::Reset { target, mode } => (
            match mode {
                ResetMode::Soft => Kind::ResetSoft,
                ResetMode::Mixed => Kind::ResetMixed,
                ResetMode::Hard => Kind::ResetHard,
            },
            short(target),
        ),
        _ => return None,
    })
}

pub fn execute(path: &Path, request: Request, expected: &str) -> Result<String> {
    if matches!(request, Request::Continue | Request::Skip | Request::Abort) {
        return super::gitjournal::around_control(path, || execute_now(path, request, expected));
    }
    let operation = journaled(&request);
    super::gitjournal::around(path, operation, || execute_now(path, request, expected))
}

fn execute_now(path: &Path, request: Request, expected: &str) -> Result<String> {
    let merge_flags: &[&str] = match &request {
        Request::MergeWithMode { mode, .. } => match mode {
            MergeMode::NoFastForward => &["--no-ff"],
            MergeMode::NoCommit => &["--no-ff", "--no-commit"],
            MergeMode::FastForwardOnly => &["--ff-only"],
            MergeMode::Squash => &["--squash"],
        },
        _ => &[],
    };
    let _busy = super::gitfacts::mutating();
    let p = root(path)?;
    if marker_exists(&p, super::gitfacts::markers(&p).as_ref(), "BISECT_START")? {
        return Err(fail(
            "Finish bisect in Tools before other repository operations.",
        ));
    }
    let (checked, prefetched) = std::thread::scope(|scope| {
        let prefetched = scope.spawn(|| prefetch(&p, &request));
        (token_seen(&p), joined(prefetched))
    });
    let (token, mut seen) = checked?;
    seen.prefetched = prefetched.unwrap_or_default();
    if token != expected {
        return Err(fail(
            "The repository changed. Refresh and review the selected action again",
        ));
    }
    // The guards of the action reuse what the token was computed from.
    let _armed = Armed::new(seen);
    // These neither move HEAD nor detach it, so a detached commit stays where it was: HEAD.
    // Deleting a branch or tag still keeps it: that reference may have been the only one
    // to the detached commit besides HEAD.
    let keeps_head = matches!(
        request,
        Request::CreateBranch { .. }
            | Request::RenameBranch { .. }
            | Request::CreateTag { .. }
            | Request::SetUpstream { .. }
            | Request::AddRemote { .. }
            | Request::RenameRemote { .. }
            | Request::RemoveRemote { .. }
            | Request::SetRemoteUrl { .. }
            | Request::StashDrop { .. }
            | Request::StashRename { .. }
    );
    let result = match request {
        Request::CreateBranch { name, target } => {
            validate_name(&p, &name, "branch")?;
            commit_id(&p, &target)?;
            git(&p, &["branch", "--", &name, &target])
        }
        Request::RenameBranch { name, new_name } => {
            reference(&p, "branch", &name)?;
            validate_name(&p, &new_name, "branch")?;
            git(&p, &["branch", "--move", "--", &name, &new_name])
        }
        Request::DeleteBranch { name, force } => {
            reference(&p, "branch", &name)?;
            let flag = if force { "-D" } else { "--delete" };
            git(&p, &["branch", flag, "--", &name])
        }
        Request::CreateTag {
            name,
            target,
            message,
        } => {
            validate_name(&p, &name, "tag")?;
            commit_id(&p, &target)?;
            if message.trim().is_empty() {
                git(
                    &p,
                    &["-c", "tag.gpgSign=false", "tag", "--", &name, &target],
                )
            } else {
                git(
                    &p,
                    &[
                        "-c",
                        "tag.gpgSign=false",
                        "tag",
                        "-a",
                        "-m",
                        &message,
                        "--",
                        &name,
                        &target,
                    ],
                )
            }
        }
        Request::DeleteTag { name } => {
            reference(&p, "tag", &name)?;
            git(&p, &["tag", "--delete", "--", &name])
        }
        Request::SetUpstream { branch, upstream } => {
            reference(&p, "branch", &branch)?;
            let selected = refs(&p)?
                .into_iter()
                .find(|r| {
                    (r.kind == "branch" || r.kind == "remote")
                        && (r.name == upstream || r.full_name == upstream)
                })
                .ok_or_else(|| fail("Select an existing branch as upstream"))?;
            git(
                &p,
                &[
                    "branch",
                    &format!("--set-upstream-to={}", selected.full_name),
                    "--",
                    &branch,
                ],
            )
        }
        Request::AddRemote { name, url: value } => {
            validate_name(&p, &name, "remote")?;
            url(&value)?;
            git(&p, &["remote", "add", "--", &name, &value])
        }
        Request::RenameRemote { name, new_name } => {
            remote_exists(&p, &name)?;
            validate_name(&p, &new_name, "remote")?;
            git(&p, &["remote", "rename", "--", &name, &new_name])
        }
        Request::RemoveRemote { name } => {
            remote_exists(&p, &name)?;
            git(&p, &["remote", "remove", "--", &name])
        }
        Request::SetRemoteUrl { name, url: value } => {
            remote_exists(&p, &name)?;
            url(&value)?;
            git(&p, &["remote", "set-url", "--", &name, &value])
        }
        Request::StashDrop { id } => drop_stash(&p, &id),
        Request::StashRename { id, name } => rename_stash(&p, &id, &name),
        Request::StashSave {
            message,
            keep_index,
            tracked_only,
            files,
        } => {
            require_head(&p)?;
            no_operation(&p)?;
            let message = match message.trim() {
                "" => "Gall workspace stash",
                message => message,
            };
            let specs: Vec<String> = files
                .iter()
                .map(|file| {
                    super::gitinspect::relative_file(file)?;
                    Ok(format!(":(literal){file}"))
                })
                .collect::<Result<_>>()?;
            if has_filters(&p) {
                let paths: Vec<_> = specs.iter().map(String::as_str).collect();
                super::gitadvanced::verify_filtered_stash_paths(&p, &paths, !tracked_only)?;
            }
            let mut args = vec!["-c", "submodule.recurse=false", "stash", "push"];
            if !tracked_only {
                args.push("--include-untracked");
            }
            if keep_index {
                args.push("--keep-index");
            }
            args.extend(["-m", message]);
            if !specs.is_empty() {
                args.push("--");
                args.extend(specs.iter().map(String::as_str));
            }
            let output = git(&p, &args)?;
            // `stash push` prints the no-op result in the C locale even though it exits 0.
            if output.lines().next() == Some("No local changes to save") {
                return Err(fail("There are no such local changes to stash"));
            }
            Ok(output)
        }
        Request::StashBranch { id, name } => {
            require_head(&p)?;
            no_operation(&p)?;
            // Local changes do not block it, as with `git stash branch`: the switch refuses, without
            // changing anything, when it would overwrite one, and the apply refuses the same way.
            stash(&p, &id)?;
            validate_name(&p, &name, "branch")?;
            let base = super::gitstash::base(&p, &id)?;
            protect_untracked(&p, &[&base, &id], None)?;
            git(
                &p,
                &[
                    "-c",
                    "submodule.recurse=false",
                    "switch",
                    "--no-overwrite-ignore",
                    "-c",
                    &name,
                    &base,
                ],
            )?;
            // It was made on this commit, so it fits. The stash is removed only once it is applied.
            git(
                &p,
                &[
                    "-c",
                    "submodule.recurse=false",
                    "stash",
                    "apply",
                    "--index",
                    &id,
                ],
            )
            .map_err(|e| {
                fail(format!(
                    "The branch was created, but the stash could not be applied and is kept: {e}"
                ))
            })?;
            drop_stash(&p, &id).map_err(|e| {
                fail(format!(
                    "Stash applied, but its stack entry could not be removed: {e}"
                ))
            })
        }
        Request::StashApply { id } => {
            require_head(&p)?;
            no_operation(&p)?;
            // Local changes do not block it: `stash apply` refuses, without changing anything,
            // when it would overwrite one (lazygit lets Git decide as well).
            stash(&p, &id)?;
            git(
                &p,
                &[
                    "-c",
                    "submodule.recurse=false",
                    "stash",
                    "apply",
                    "--index",
                    &id,
                ],
            )
        }
        Request::StashPop { id } => {
            require_head(&p)?;
            no_operation(&p)?;
            stash(&p, &id)?;
            let output = git(
                &p,
                &[
                    "-c",
                    "submodule.recurse=false",
                    "stash",
                    "apply",
                    "--index",
                    &id,
                ],
            )?;
            drop_stash(&p, &id).map_err(|e| {
                fail(format!(
                    "Stash applied, but its stack entry could not be removed: {e}"
                ))
            })?;
            Ok(output)
        }
        Request::Continue => {
            require_head(&p)?;
            control(&p, "--continue")
        }
        Request::Skip => {
            require_head(&p)?;
            control(&p, "--skip")
        }
        Request::Abort => {
            require_head(&p)?;
            control(&p, "--abort")
        }
        Request::Resolve { file, side } => {
            require_head(&p)?;
            if !conflicts(&p)?.contains(&file) {
                return Err(fail("This file is no longer an unresolved conflict"));
            }
            let literal = format!(":(literal){file}");
            match side {
                ResolveSide::Ours | ResolveSide::Theirs => {
                    let (flag, chosen) = if matches!(side, ResolveSide::Ours) {
                        ("--ours", "2")
                    } else {
                        ("--theirs", "3")
                    };
                    let index = git(&p, &["ls-files", "--unmerged", "-z", "--", &literal])?;
                    let mut stages = index
                        .split_terminator('\0')
                        .filter_map(|record| {
                            let (metadata, path) = record.split_once('\t')?;
                            (path == file).then_some(metadata)
                        })
                        .peekable();
                    if stages.peek().is_none() {
                        return Err(fail("This file is no longer an unresolved conflict"));
                    }
                    let mut selected = None;
                    let mut has_gitlink = false;
                    for metadata in stages {
                        let mut fields = metadata.split_whitespace();
                        let mode = fields.next().unwrap_or_default();
                        let id = fields.next().unwrap_or_default();
                        has_gitlink |= mode == "160000";
                        if fields.next() == Some(chosen) {
                            selected = Some((mode, id));
                        }
                    }
                    match selected {
                        None if has_gitlink => {
                            // The parent can stop tracking the path without removing a nested checkout.
                            return git(&p, &["update-index", "--force-remove", "--", &file]);
                        }
                        None => return git(&p, &["rm", "-f", "--", &literal]),
                        Some(("160000", id)) => {
                            return git(
                                &p,
                                &[
                                    "update-index",
                                    "--add",
                                    "--cacheinfo",
                                    &format!("160000,{id},{file}"),
                                ],
                            );
                        }
                        Some(_) => {}
                    }
                    git(
                        &p,
                        &[
                            "-c",
                            "submodule.recurse=false",
                            "restore",
                            "--worktree",
                            flag,
                            "--",
                            &literal,
                        ],
                    )?;
                    git(&p, &["add", "--", &literal])
                }
                ResolveSide::Mark => git(&p, &["add", "--all", "--", &literal]),
            }
        }
        Request::ResolveContent { file, content } => {
            require_head(&p)?;
            if !conflicts(&p)?.contains(&file) {
                return Err(fail("This file is no longer an unresolved conflict"));
            }
            if content.len() > 4 * 1024 * 1024 {
                return Err(fail("The resolved file is too large"));
            }
            let target = checked_work_path(&p, &file)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| fail(format!("Cannot create the folder for {file}: {e}")))?;
            }
            std::fs::write(&target, content)
                .map_err(|e| fail(format!("Cannot write the resolved file {file}: {e}")))?;
            git(&p, &["add", "--", &format!(":(literal){file}")])
        }
        Request::Merge { target } | Request::MergeWithMode { target, .. } => {
            require_head(&p)?;
            no_operation(&p)?;
            // Local changes do not block a merge, as in Git and lazygit: Git itself refuses,
            // without changing anything, only when the merge would overwrite one of them.
            commit_id(&p, &target)?;
            save_head(&p)?;
            super::gitprogress::report_phase("Merging", None);
            let mut args = vec![
                "-c",
                "merge.autoStash=false",
                "-c",
                "submodule.recurse=false",
                "merge",
                "--no-edit",
                "--no-overwrite-ignore",
            ];
            if super::gitprogress::is_active() {
                args.push("--progress");
            }
            args.extend_from_slice(merge_flags);
            args.push(&target);
            git(&p, &args)
        }
        Request::Rebase { target } => {
            // Rebase rewrites the superproject's gitlinks without checking out submodules.
            // Reuse the token's status rather than recursively scanning their folders.
            require_head(&p)?;
            no_operation(&p)?;
            if super::gitgraph::status_entries(&status(&p)?)
                .iter()
                .any(|entry| entry.code != "?? ")
            {
                protect_staged_gitlinks(&p)?;
            }
            commit_id(&p, &target)?;
            protect_untracked(&p, &[&target], Some(&target))?;
            verify_filtered_autostash(&p)?;
            save_head(&p)?;
            super::gitprogress::report_phase("Rebasing", None);
            git(
                &p,
                &[
                    "-c",
                    "core.editor=true",
                    "-c",
                    "submodule.recurse=false",
                    // Local changes are kept aside by Git and put back afterwards (lazygit does the
                    // same); a conflict while putting them back leaves them in the stash, which
                    // Git names in its message. The staged gitlink check keeps pointers out of it.
                    "rebase",
                    "--autostash",
                    &target,
                ],
            )
        }
        Request::CherryPickMany { targets } => {
            require_head(&p)?;
            no_operation(&p)?;
            check_picks(&p, &targets)?;
            // retain_detached keeps HEAD where it is.
            let before = save_head(&p)?;
            retain_detached(&p)?;
            super::gitprogress::report_phase(
                "Cherry-picking",
                Some(format!("{} commits", targets.len())),
            );
            let mut args = vec![
                "-c",
                "submodule.recurse=false",
                "cherry-pick",
                KEEP_EMPTY_PICKS,
            ];
            args.extend(targets.iter().map(String::as_str));
            git(&p, &args).inspect_err(|_| {
                // Git refuses a list (a staged change, a file it would overwrite) after it has already
                // written its sequencer, which would show as an operation in progress that nothing
                // can continue. With no pick made and no conflict, only that record is dropped: the
                // index and the files stay as they were.
                if head(&p) == before
                    && !super::gitfacts::marker(&p, "CHERRY_PICK_HEAD")
                        .is_some_and(|marker| marker.exists())
                    && snapshot(&p).is_ok_and(|state| state.conflicts.is_empty())
                {
                    let _ = git(&p, &["cherry-pick", "--quit"]);
                }
            })
        }
        Request::CherryPick { target } => {
            require_head(&p)?;
            no_operation(&p)?;
            check_picks(&p, std::slice::from_ref(&target))?;
            retain_detached(&p)?;
            git(
                &p,
                &[
                    "-c",
                    "submodule.recurse=false",
                    "cherry-pick",
                    KEEP_EMPTY_PICKS,
                    &target,
                ],
            )
        }
        Request::Revert { target } => {
            require_head(&p)?;
            no_operation(&p)?;
            commit_id(&p, &target)?;
            let parents = git(&p, &["show", "-s", "--format=%P", &target])?;
            let parents: Vec<_> = parents.split_whitespace().collect();
            if let [parent] = parents.as_slice() {
                protect_integration_writes(&p, &target, parent)?;
            }
            retain_detached(&p)?;
            git(
                &p,
                &[
                    "-c",
                    "submodule.recurse=false",
                    "revert",
                    "--no-edit",
                    &target,
                ],
            )
        }
        Request::IntegrateMerge {
            target,
            mainline,
            revert,
        } => {
            require_head(&p)?;
            no_operation(&p)?;
            commit_id(&p, &target)?;
            let listing = git(&p, &["show", "-s", "--format=%P", &target])?;
            let parents: Vec<_> = listing.split_whitespace().collect();
            if parents.len() < 2 || mainline == 0 || mainline > parents.len() {
                return Err(fail(
                    "Select an existing mainline parent of the merge commit",
                ));
            }
            let parent = parents[mainline - 1];
            let (before, after) = if revert {
                (target.as_str(), parent)
            } else {
                (parent, target.as_str())
            };
            protect_integration_writes(&p, before, after)?;
            retain_detached(&p)?;
            let parent_number = mainline.to_string();
            let mut args = vec![
                "-c",
                "submodule.recurse=false",
                if revert { "revert" } else { "cherry-pick" },
                "--no-edit",
            ];
            if !revert {
                args.push(KEEP_EMPTY_PICKS);
            }
            args.extend(["-m", parent_number.as_str(), target.as_str()]);
            git(&p, &args)
        }
        Request::Reset { target, mode } => {
            require_head(&p)?;
            no_operation(&p)?;
            commit_id(&p, &target)?;
            save_head(&p)?;
            // Git itself never touches the submodules here, whatever `submodule.recurse` says:
            // a hard reset checks them out below, after saving their edits for recovery.
            let reset = |flag: &str| {
                git(
                    &p,
                    &["-c", "submodule.recurse=false", "reset", flag, &target],
                )
            };
            match mode {
                ResetMode::Soft => {
                    super::gitprogress::report_phase("Resetting branch", None);
                    reset("--soft")
                }
                ResetMode::Mixed => {
                    super::gitprogress::report_phase("Saving recovery snapshot", None);
                    save_index_and_worktree(&p, "Gall recovery before mixed reset", false)?;
                    super::gitprogress::report_phase("Resetting index", None);
                    reset("--mixed")
                }
                ResetMode::Hard => {
                    // A hard reset overwrites untracked or ignored files that the target
                    // commit tracks. Reject only those collisions: unrelated untracked and
                    // ignored files (build output, dependencies) are left alone by Git.
                    protect_untracked(&p, &[&target], None)?;
                    super::gitprogress::report_phase("Saving recovery snapshot", None);
                    save_index_and_worktree(&p, HARD_RESET_RECOVERY, true)?;
                    // The result matches the target commit, submodules included. Only those
                    // listed in `.gitmodules` are known to `git submodule`; an embedded
                    // repository without an entry keeps its folder as it is.
                    let submodules = p.join(".gitmodules").exists();
                    if submodules {
                        reset_submodule_edits(&p)?;
                    }
                    super::gitprogress::report_phase("Resetting working tree", None);
                    let output = reset("--hard")?;
                    if submodules || p.join(".gitmodules").exists() {
                        super::gitprogress::report_phase("Updating submodules", None);
                        update_submodules(&p)?;
                    }
                    Ok(output)
                }
            }
        }
    }?;
    if !keeps_head && !head(&p).is_empty() {
        retain_detached(&p).map_err(|e| fail(format!("The operation completed, but its detached commit recovery reference could not be saved: {e}")))?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        git(d.path(), &["init", "--initial-branch=main", "-q"]).unwrap();
        git(d.path(), &["config", "user.name", "Test"]).unwrap();
        git(d.path(), &["config", "user.email", "test@example.com"]).unwrap();
        // Windows runners default to autocrlf=true, which changes the bytes tests compare.
        git(d.path(), &["config", "core.autocrlf", "false"]).unwrap();
        d
    }

    fn commit(path: &Path, message: &str, value: &str) -> String {
        std::fs::write(path.join("file.txt"), value).unwrap();
        git(path, &["add", "--all"]).unwrap();
        git(path, &["commit", "-qm", message]).unwrap();
        head(path)
    }

    fn act(path: &Path, request: Request) -> Result<String> {
        execute(path, request, &snapshot(path)?.token)
    }

    #[test]
    fn checkout_commit_file_stages_only_the_selected_literal_path() {
        let d = repository();
        let p = d.path();
        let name = "nested/ž [draft].txt";
        std::fs::create_dir(p.join("nested")).unwrap();
        std::fs::write(p.join(name), "old\n").unwrap();
        let historical = commit(p, "historical", "original\n");
        std::fs::write(p.join(name), "new\n").unwrap();
        let current = commit(p, "current", "original\n");
        std::fs::write(p.join("file.txt"), "unrelated local edit\n").unwrap();
        std::fs::write(p.join("scratch"), "untracked\n").unwrap();

        checkout_commit_file(p, &historical, name).unwrap();

        assert_eq!(head(p), current);
        assert_eq!(std::fs::read_to_string(p.join(name)).unwrap(), "old\n");
        assert_eq!(git(p, &["show", &format!(":{name}")]).unwrap(), "old\n");
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "unrelated local edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("scratch")).unwrap(),
            "untracked\n"
        );
    }

    #[test]
    fn checkout_commit_file_preserves_staged_unstaged_and_hidden_edits() {
        let d = repository();
        let p = d.path();
        let historical = commit(p, "historical", "old\n");
        let current = commit(p, "current", "new\n");
        let staged = || git(p, &["show", ":file.txt"]).unwrap();

        std::fs::write(p.join("file.txt"), "local\n").unwrap();
        let error = checkout_commit_file(p, &historical, "file.txt").unwrap_err();
        assert!(error.to_string().contains("file.txt"));
        assert_eq!(staged(), "new\n");
        git(p, &["update-index", "--assume-unchanged", "file.txt"]).unwrap();
        assert!(checkout_commit_file(p, &historical, "file.txt").is_err());
        git(p, &["update-index", "--no-assume-unchanged", "file.txt"]).unwrap();
        git(p, &["add", "--", "file.txt"]).unwrap();
        assert!(checkout_commit_file(p, &historical, "file.txt").is_err());
        assert_eq!(staged(), "local\n");
        assert_eq!(head(p), current);
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "local\n"
        );
    }

    #[test]
    fn checkout_commit_file_preserves_untracked_and_ignored_replacements() {
        let d = repository();
        let p = d.path();
        let historical = commit(p, "historical", "old\n");
        git(p, &["rm", "--", "file.txt"]).unwrap();
        git(p, &["commit", "-qm", "delete file"]).unwrap();
        std::fs::write(p.join("file.txt"), "local\n").unwrap();
        assert!(checkout_commit_file(p, &historical, "file.txt").is_err());
        std::fs::write(p.join(".gitignore"), "file.txt\n").unwrap();
        assert!(checkout_commit_file(p, &historical, "file.txt").is_err());
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "local\n"
        );
        assert!(git(p, &["ls-files", "--", "file.txt"]).unwrap().is_empty());

        std::fs::remove_file(p.join("file.txt")).unwrap();
        checkout_commit_file(p, &historical, "file.txt").unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "old\n"
        );
    }

    #[test]
    fn cherry_pick_and_revert_merge_use_the_selected_mainline() {
        let d = repository();
        let p = d.path();
        let base = commit(p, "base", "base\n");
        git(p, &["switch", "-qc", "topic"]).unwrap();
        std::fs::write(p.join("topic.txt"), "topic\n").unwrap();
        git(p, &["add", "--", "topic.txt"]).unwrap();
        git(p, &["commit", "-qm", "Topic"]).unwrap();
        git(p, &["switch", "-q", "main"]).unwrap();
        git(p, &["merge", "--no-ff", "-qm", "Merge topic", "topic"]).unwrap();
        let merged = head(p);
        git(p, &["switch", "-qc", "pick", &base]).unwrap();
        let apply = |mainline, revert| {
            act(
                p,
                Request::IntegrateMerge {
                    target: merged.clone(),
                    mainline,
                    revert,
                },
            )
        };
        assert!(apply(3, false)
            .unwrap_err()
            .to_string()
            .contains("mainline"));
        assert_eq!(head(p), base);
        std::fs::write(p.join("topic.txt"), "keep local\n").unwrap();
        assert!(apply(1, false)
            .unwrap_err()
            .to_string()
            .contains("topic.txt"));
        assert_eq!(
            std::fs::read_to_string(p.join("topic.txt")).unwrap(),
            "keep local\n"
        );
        assert_eq!(head(p), base);
        std::fs::remove_file(p.join("topic.txt")).unwrap();
        apply(1, false).unwrap();
        assert_eq!(git(p, &["show", "HEAD:topic.txt"]).unwrap(), "topic\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        apply(1, true).unwrap();
        assert!(!p.join("topic.txt").exists());
        assert_eq!(git(p, &["rev-parse", "HEAD^"]).unwrap().trim(), merged);
    }

    #[test]
    fn merge_without_committing_stages_changes_and_accepts_a_custom_commit() {
        use super::super::gitworktree::{index_action, workspace, IndexAction};

        let d = repository();
        let p = d.path();
        let base = commit(p, "base", "base\n");
        git(p, &["switch", "-qc", "topic"]).unwrap();
        let target = commit(p, "topic change", "topic\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        let merge = || {
            act(
                p,
                Request::MergeWithMode {
                    target: target.clone(),
                    mode: MergeMode::NoCommit,
                },
            )
        };

        merge().unwrap();
        assert_eq!(head(p), base);
        assert_eq!(snapshot(p).unwrap().operation.as_deref(), Some("merge"));
        assert_eq!(git(p, &["rev-parse", "MERGE_HEAD"]).unwrap().trim(), target);
        assert_eq!(git(p, &["show", ":file.txt"]).unwrap(), "topic\n");
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "topic\n"
        );
        let prepared = workspace(p).unwrap();
        assert!(prepared.merging);

        // Quitting keeps HEAD and the index. An old merge token must still be refused.
        git(p, &["merge", "--quit"]).unwrap();
        assert!(
            index_action(p, IndexAction::Commit, None, "Stale merge", &prepared.token).is_err()
        );
        assert_eq!(head(p), base);
        git(p, &["reset", "--hard", "HEAD"]).unwrap();
        merge().unwrap();
        std::fs::write(p.join("extra.txt"), "extra\n").unwrap();
        std::fs::write(p.join("unstaged.txt"), "leave out\n").unwrap();
        let state = workspace(p).unwrap();
        assert!(index_action(p, IndexAction::Amend, None, "Amend", &state.token).is_err());
        index_action(p, IndexAction::Stage, Some("extra.txt"), "", &state.token).unwrap();
        let state = workspace(p).unwrap();
        index_action(
            p,
            IndexAction::Commit,
            None,
            "Custom merge\n\nExtra context",
            &state.token,
        )
        .unwrap();

        assert_eq!(snapshot(p).unwrap().operation, None);
        assert_eq!(
            git(p, &["show", "-s", "--format=%P", "HEAD"])
                .unwrap()
                .trim(),
            format!("{base} {target}")
        );
        assert_eq!(
            git(p, &["show", "-s", "--format=%B", "HEAD"])
                .unwrap()
                .trim(),
            "Custom merge\n\nExtra context"
        );
        assert_eq!(git(p, &["show", "HEAD:extra.txt"]).unwrap(), "extra\n");
        assert!(git(p, &["show", "HEAD:unstaged.txt"]).is_err());
        assert_eq!(
            std::fs::read_to_string(p.join("unstaged.txt")).unwrap(),
            "leave out\n"
        );
        let journal = super::super::gitjournal::view(p).unwrap();
        assert!(matches!(
            journal.entries.last().unwrap().kind,
            super::super::gitjournal::Kind::Merge
        ));
    }

    #[test]
    fn merge_without_committing_allows_a_custom_commit_for_an_identical_tree() {
        use super::super::gitworktree::{index_action, workspace, IndexAction};

        let d = repository();
        let p = d.path();
        commit(p, "base", "same\n");
        git(p, &["switch", "-qc", "topic"]).unwrap();
        git(p, &["commit", "--allow-empty", "-m", "topic"]).unwrap();
        let target = head(p);
        git(p, &["switch", "-q", "main"]).unwrap();
        act(
            p,
            Request::MergeWithMode {
                target: target.clone(),
                mode: MergeMode::NoCommit,
            },
        )
        .unwrap();
        let state = workspace(p).unwrap();
        assert!(state.merging);
        assert!(state.changes.is_empty());
        index_action(
            p,
            IndexAction::Commit,
            None,
            "Merge identical tree",
            &state.token,
        )
        .unwrap();
        assert_eq!(git(p, &["rev-parse", "HEAD^2"]).unwrap().trim(), target);
        act(
            p,
            Request::MergeWithMode {
                target,
                mode: MergeMode::NoCommit,
            },
        )
        .unwrap();
        assert!(!workspace(p).unwrap().merging);
    }

    #[test]
    fn explicit_merge_modes_keep_git_ancestry_and_index_semantics() {
        for mode in [
            MergeMode::NoFastForward,
            MergeMode::NoCommit,
            MergeMode::FastForwardOnly,
            MergeMode::Squash,
        ] {
            let d = repository();
            let p = d.path();
            let base = commit(p, "base", "base\n");
            git(p, &["switch", "-qc", "topic"]).unwrap();
            let target = commit(p, "topic change", "topic\n");
            git(p, &["switch", "-q", "main"]).unwrap();
            std::fs::write(p.join("build.log"), "keep\n").unwrap();
            act(
                p,
                Request::MergeWithMode {
                    target: target.clone(),
                    mode,
                },
            )
            .unwrap();
            assert_eq!(
                std::fs::read_to_string(p.join("file.txt")).unwrap(),
                "topic\n"
            );
            assert_eq!(
                std::fs::read_to_string(p.join("build.log")).unwrap(),
                "keep\n"
            );
            match mode {
                MergeMode::FastForwardOnly => assert_eq!(head(p), target),
                MergeMode::NoFastForward => {
                    assert_eq!(
                        git(p, &["show", "-s", "--format=%P", "HEAD"])
                            .unwrap()
                            .trim(),
                        format!("{base} {target}")
                    );
                }
                MergeMode::Squash | MergeMode::NoCommit => {
                    assert_eq!(head(p), base);
                    assert_eq!(git(p, &["show", ":file.txt"]).unwrap(), "topic\n");
                    assert_eq!(
                        p.join(".git/MERGE_HEAD").exists(),
                        matches!(mode, MergeMode::NoCommit)
                    );
                }
            }
        }
        let d = repository();
        let p = d.path();
        commit(p, "base", "base\n");
        git(p, &["switch", "-qc", "topic"]).unwrap();
        let target = commit(p, "topic", "topic\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        let original = commit(p, "different main", "main\n");
        assert!(act(
            p,
            Request::MergeWithMode {
                target,
                mode: MergeMode::FastForwardOnly
            }
        )
        .is_err());
        assert_eq!(head(p), original);
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "main\n"
        );
        assert!(!p.join(".git/MERGE_HEAD").exists());
    }

    #[test]
    fn the_check_before_a_remote_operation_agrees_with_the_snapshot() {
        let d = repository();
        let p = d.path();
        commit(p, "one", "1");
        git(
            p,
            &["remote", "add", "origin", "https://example.invalid/r.git"],
        )
        .unwrap();
        git(p, &["branch", "side"]).unwrap();
        git(p, &["branch", "--set-upstream-to=side", "main"]).unwrap();
        std::fs::write(p.join("file.txt"), "changed").unwrap();
        std::fs::write(p.join("new.txt"), "new").unwrap();
        for with_refs in [false, true] {
            let full = snapshot(p).unwrap();
            let light = checked(p, with_refs).unwrap();
            assert_eq!(light.token, full.token, "with_refs: {with_refs}");
            assert_eq!(light.head, full.head);
            assert_eq!(light.branch, full.branch);
            assert_eq!(light.operation, full.operation);
            let names = |remotes: &[Remote]| -> Vec<String> {
                remotes.iter().map(|r| r.name.clone()).collect()
            };
            assert_eq!(names(&light.remotes), names(&full.remotes));
            let upstreams = |refs: &[Reference]| -> Vec<(String, Option<String>)> {
                refs.iter()
                    .map(|r| (r.full_name.clone(), r.upstream.clone()))
                    .collect()
            };
            if with_refs {
                assert_eq!(upstreams(&light.refs), upstreams(&full.refs));
            } else {
                assert!(light.refs.is_empty());
            }
        }
    }

    #[test]
    fn branch_and_tag_actions_at_head_and_elsewhere_keep_their_checks() {
        let d = repository();
        let p = d.path();
        let first = commit(p, "one", "1");
        let second = commit(p, "two", "2");
        assert!(act(
            p,
            Request::CreateBranch {
                name: "at-head".into(),
                target: second.clone()
            }
        )
        .is_ok());
        assert!(act(
            p,
            Request::CreateBranch {
                name: "older".into(),
                target: first.clone()
            }
        )
        .is_ok());
        assert_eq!(git(p, &["rev-parse", "older"]).unwrap().trim(), first);
        // A tree is no commit, whichever way the check goes.
        let tree = git(p, &["rev-parse", "HEAD^{tree}"])
            .unwrap()
            .trim()
            .to_owned();
        assert!(act(
            p,
            Request::CreateBranch {
                name: "tree".into(),
                target: tree
            }
        )
        .is_err());
        // Names Git refuses are refused, whether the rules here or Git say so.
        for name in ["a.lock/b", "x..y", "-x", "a b"] {
            assert!(
                act(
                    p,
                    Request::CreateBranch {
                        name: name.into(),
                        target: second.clone()
                    }
                )
                .is_err(),
                "{name}"
            );
        }
        assert!(act(
            p,
            Request::DeleteBranch {
                name: "missing".into(),
                force: true
            }
        )
        .is_err());
        assert!(act(
            p,
            Request::DeleteBranch {
                name: "older".into(),
                force: true
            }
        )
        .is_ok());
        assert!(act(
            p,
            Request::DeleteBranch {
                name: "older".into(),
                force: true
            }
        )
        .is_err());
        // In a detached HEAD, creating a branch leaves no recovery reference: HEAD still
        // holds the commit, and every action that moves HEAD keeps it first.
        git(p, &["switch", "-q", "--detach", &first]).unwrap();
        assert!(act(
            p,
            Request::CreateBranch {
                name: "from-detached".into(),
                target: first.clone()
            }
        )
        .is_ok());
        assert_eq!(
            git(
                p,
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    &format!("refs/gall/recovery/{first}")
                ]
            )
            .ok(),
            None
        );
        // Deleting a branch there keeps it: that branch may have been its only other name.
        let recovery = format!("refs/gall/recovery/{first}");
        let deleted = Request::DeleteBranch {
            name: "from-detached".into(),
            force: true,
        };
        assert!(act(p, deleted).is_ok());
        assert!(git(p, &["rev-parse", "--verify", "-q", &recovery]).is_ok());
        // A symbolic branch makes the file reading give up; Git answers instead.
        git(p, &["switch", "-q", "main"]).unwrap();
        git(p, &["branch", "plain"]).unwrap();
        git(p, &["symbolic-ref", "refs/heads/alias", "refs/heads/main"]).unwrap();
        assert!(super::super::gitfacts::branches(p).is_none());
        let delete = |name: &str| Request::DeleteBranch {
            name: name.into(),
            force: true,
        };
        assert!(act(p, delete("missing")).is_err());
        assert!(act(p, delete("plain")).is_ok());
    }

    #[test]
    fn remotes_come_from_one_listing() {
        let listing = "origin\thttps://example.com/a (b).git (fetch)\n\
                       origin\tssh://push.example.com/a.git (push)\n\
                       origin\tssh://second.example.com/a.git (push)\n\
                       partial\thttps://example.com/p.git (fetch) [blob:none]\n\
                       partial\thttps://example.com/p.git (push)\n\
                       bare\n";
        let found: Vec<_> = parse_remotes(listing)
            .into_iter()
            .map(|r| (r.name, r.fetch_url, r.push_url))
            .collect();
        let row = |name: &str, fetch: &str, push: &str| (name.into(), fetch.into(), push.into());
        assert_eq!(
            found,
            [
                row(
                    "origin",
                    "https://example.com/a (b).git",
                    "ssh://push.example.com/a.git"
                ),
                row(
                    "partial",
                    "https://example.com/p.git",
                    "https://example.com/p.git"
                ),
                row("bare", "", ""),
            ]
        );

        // The listing agrees with `remote get-url`, including rewritten and push-only URLs.
        let d = repository();
        let p = d.path();
        git(p, &["remote", "add", "origin", "short:team/project.git"]).unwrap();
        git(p, &["remote", "add", "second", "https://example.com/s.git"]).unwrap();
        let rewrite = "url.https://example.com/.insteadOf";
        git(p, &["config", rewrite, "short:"]).unwrap();
        let push = "ssh://push.example.com/s.git";
        git(p, &["remote", "set-url", "--push", "second", push]).unwrap();
        let listed = remotes(p).unwrap();
        assert_eq!(listed.len(), 2);
        for remote in listed {
            let url = |args: &[&str]| git(p, args).unwrap().trim().to_owned();
            assert_eq!(
                remote.fetch_url,
                url(&["remote", "get-url", "--", &remote.name])
            );
            assert_eq!(
                remote.push_url,
                url(&["remote", "get-url", "--push", "--", &remote.name])
            );
        }
    }

    #[test]
    fn graph_view_matches_separate_queries() {
        let d = repository();
        let p = d.path();
        commit(p, "first", "first\n");
        std::fs::write(p.join("renamed.txt"), "to be renamed\n").unwrap();
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "second"]).unwrap();
        git(p, &["mv", "renamed.txt", "moved.txt"]).unwrap();
        std::fs::write(p.join("file.txt"), "edited\n").unwrap();
        std::fs::create_dir(p.join("dir")).unwrap();
        std::fs::write(p.join("dir/new.txt"), "new\n").unwrap();
        // Names that `status --porcelain` quotes: a space, non-ASCII, and on Unix also control
        // characters, a quote and a newline (not valid in Windows file names).
        std::fs::write(p.join("with space.txt"), "x\n").unwrap();
        std::fs::write(p.join("é.txt"), "x\n").unwrap();
        #[cfg(unix)]
        for name in [
            "tab\there.txt",
            "quote\"d.txt",
            "line\nbreak.txt",
            "back\\slash",
        ] {
            std::fs::write(p.join(name), "x\n").unwrap();
        }

        let view = graph_view(p, false, false, 200, None).unwrap();
        let expected = git(
            p,
            &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ],
        )
        .unwrap();
        assert!(expected.contains("R  renamed.txt -> moved.txt"));
        assert_eq!(view.graph.changes, expected);
        let separate = super::super::gitgraph::query_history(p, false, false, 200, None).unwrap();
        assert_eq!(view.graph.history, separate.history);
        assert_eq!(view.graph.branches, separate.branches);
        assert_eq!(view.graph.head, separate.head);
        assert_eq!(view.repository.token, snapshot(p).unwrap().token);
        assert_eq!(view.repository.head, view.graph.head);

        // The user's `core.quotePath` is honoured: non-ASCII names stay readable when it is off.
        git(p, &["config", "core.quotePath", "false"]).unwrap();
        let readable = graph_view(p, false, false, 200, None)
            .unwrap()
            .graph
            .changes;
        let expected = git(
            p,
            &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ],
        )
        .unwrap();
        // Compare with Git's own text, not a literal: macOS may normalize the name (NFD/NFC).
        assert!(!readable.contains("\\303"), "{readable}");
        assert_eq!(readable, expected);
        git(p, &["config", "--unset", "core.quotePath"]).unwrap();

        // A comparison view shows no local changes, as before.
        assert!(graph_view(p, true, false, 200, None)
            .unwrap()
            .graph
            .changes
            .is_empty());
    }

    #[test]
    fn token_follows_every_kind_of_change() {
        let d = repository();
        let p = d.path();
        commit(p, "first", "first\n");
        let clean = token(p).unwrap();
        assert_eq!(
            token(p).unwrap(),
            clean,
            "an unchanged repository keeps its token"
        );

        // A tracked file edited once, then edited again: status reads " M" both times.
        std::fs::write(p.join("file.txt"), "edit one\n").unwrap();
        let first_edit = token(p).unwrap();
        assert_ne!(first_edit, clean);
        std::fs::write(p.join("file.txt"), "a second, longer edit\n").unwrap();
        let second_edit = token(p).unwrap();
        assert_ne!(second_edit, first_edit);

        // Same size and a restored modification time: size and mtime cannot tell the edits
        // apart, the content (and on Unix the change time) does.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let set_old = |file: &Path| {
            let handle = std::fs::OpenOptions::new().write(true).open(file).unwrap();
            handle.set_modified(old).unwrap();
        };
        set_old(&p.join("file.txt"));
        let aged = token(p).unwrap();
        std::fs::write(p.join("file.txt"), "A SECOND, LONGER EDIT\n").unwrap();
        set_old(&p.join("file.txt"));
        assert_ne!(
            token(p).unwrap(),
            aged,
            "same-size edit with a restored mtime"
        );
        let second_edit = token(p).unwrap();
        // The token must not depend on the clock: a file edited moments ago keeps its token as
        // time passes, or a reviewed state would be rejected as changed a few seconds later.
        let moments_ago = std::time::SystemTime::now() - std::time::Duration::from_millis(2800);
        std::fs::OpenOptions::new()
            .write(true)
            .open(p.join("file.txt"))
            .unwrap()
            .set_modified(moments_ago)
            .unwrap();
        let fresh = token(p).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(
            token(p).unwrap(),
            fresh,
            "token changed only because time passed"
        );
        assert_ne!(fresh, second_edit);

        // Untracked files, including one that appears in a folder.
        std::fs::create_dir(p.join("dir")).unwrap();
        std::fs::write(p.join("dir/new.txt"), "new\n").unwrap();
        let untracked = token(p).unwrap();
        assert_ne!(untracked, second_edit);

        // A staged rename lists two records in `status -z`; both must parse without panicking.
        git(p, &["add", "--all"]).unwrap();
        git(p, &["mv", "dir/new.txt", "dir/moved.txt"]).unwrap();
        assert_ne!(token(p).unwrap(), untracked);

        // The light token reacts to a new reference and ignores work-tree edits.
        let light = light_token(p).unwrap();
        std::fs::write(p.join("file.txt"), "unrelated work tree edit\n").unwrap();
        assert_eq!(light_token(p).unwrap(), light);
        git(p, &["branch", "other"]).unwrap();
        assert_ne!(light_token(p).unwrap(), light);
    }

    /// A large tracked file edited to the same size with its modification time restored still
    /// changes the token, so a reviewed token cannot be reused for content nobody reviewed.
    #[test]
    fn token_sees_a_same_size_edit_of_a_large_file() {
        let d = repository();
        let p = d.path();
        let big = "a".repeat(400 * 1024);
        commit(p, "big", &big);
        let file = p.join("file.txt");
        std::fs::write(&file, big.replacen('a', "b", 1)).unwrap();
        let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
        let first = token(p).unwrap();
        std::fs::write(&file, big.replacen('a', "c", 1)).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_ne!(token(p).unwrap(), first);
    }

    #[test]
    fn empty_repository_and_reference_lifecycle() {
        let d = repository();
        let p = d.path();
        let initial = snapshot(p).unwrap();
        assert_eq!(initial.branch.as_deref(), Some("main"));
        assert!(initial.head.is_empty());
        assert!(initial.refs.is_empty());
        let first = commit(p, "first", "first\n");
        act(
            p,
            Request::CreateBranch {
                name: "feature/test".into(),
                target: first.clone(),
            },
        )
        .unwrap();
        act(
            p,
            Request::RenameBranch {
                name: "feature/test".into(),
                new_name: "feature/renamed".into(),
            },
        )
        .unwrap();
        act(
            p,
            Request::CreateTag {
                name: "v1".into(),
                target: first.clone(),
                message: "".into(),
            },
        )
        .unwrap();
        act(
            p,
            Request::CreateTag {
                name: "annotated".into(),
                target: first.clone(),
                message: "Release one".into(),
            },
        )
        .unwrap();
        let state = snapshot(p).unwrap();
        assert!(state
            .refs
            .iter()
            .any(|r| r.kind == "branch" && r.name == "feature/renamed"));
        assert!(state
            .refs
            .iter()
            .any(|r| r.kind == "tag" && r.name == "annotated" && r.id == first));
        act(
            p,
            Request::DeleteBranch {
                name: "feature/renamed".into(),
                force: false,
            },
        )
        .unwrap();
        act(p, Request::DeleteTag { name: "v1".into() }).unwrap();
        assert!(act(
            p,
            Request::DeleteBranch {
                name: "main".into(),
                force: true,
            }
        )
        .is_err());
        assert!(!snapshot(p).unwrap().refs.iter().any(|r| r.name == "v1"));
    }

    #[test]
    fn deleting_an_unmerged_branch_needs_force() {
        let d = repository();
        let p = d.path();
        commit(p, "first", "first\n");
        git(p, &["switch", "-q", "-c", "side"]).unwrap();
        commit(p, "second", "second\n");
        git(p, &["switch", "-q", "-"]).unwrap();
        let del = |force| {
            act(
                p,
                Request::DeleteBranch {
                    name: "side".into(),
                    force,
                },
            )
        };
        let error = del(false).unwrap_err().to_string();
        assert!(error.contains("not fully merged"), "{error}");
        del(true).unwrap();
        assert!(!snapshot(p).unwrap().refs.iter().any(|r| r.name == "side"));
    }

    #[test]
    fn rejects_option_injection_and_stale_head_index_and_worktree() {
        let d = repository();
        let p = d.path();
        let first = commit(p, "first", "first\n");
        for name in ["--delete", "@{-1}", "bad\nname", "../bad", "a.lock"] {
            assert!(
                act(
                    p,
                    Request::CreateBranch {
                        name: name.into(),
                        target: first.clone()
                    }
                )
                .is_err(),
                "accepted {name:?}"
            );
        }
        assert!(act(
            p,
            Request::CreateTag {
                name: "--delete".into(),
                target: first.clone(),
                message: "".into()
            }
        )
        .is_err());
        assert!(act(
            p,
            Request::CreateBranch {
                name: "valid".into(),
                target: "HEAD".into()
            }
        )
        .is_err());
        let before = snapshot(p).unwrap();
        std::fs::write(p.join("file.txt"), "second\n").unwrap();
        assert!(execute(
            p,
            Request::CreateBranch {
                name: "stale".into(),
                target: first.clone()
            },
            &before.token
        )
        .is_err());
        let before = snapshot(p).unwrap();
        git(p, &["add", "file.txt"]).unwrap();
        assert!(execute(
            p,
            Request::CreateBranch {
                name: "stale".into(),
                target: first.clone()
            },
            &before.token
        )
        .is_err());
        let before = snapshot(p).unwrap();
        git(p, &["commit", "-qm", "second"]).unwrap();
        assert!(execute(
            p,
            Request::CreateBranch {
                name: "stale".into(),
                target: first
            },
            &before.token
        )
        .is_err());
    }

    #[test]
    fn parses_upstream_tracking_counts() {
        assert_eq!(track_count("ahead 6, behind 8", "ahead"), 6);
        assert_eq!(track_count("ahead 6, behind 8", "behind"), 8);
        assert_eq!(track_count("behind 2", "ahead"), 0);
        assert_eq!(track_count("gone", "behind"), 0);
        assert_eq!(track_count("", "ahead"), 0);
    }

    #[test]
    fn invalid_reference_names_explain_the_problem() {
        let d = repository();
        let p = d.path();
        let first = commit(p, "first", "one\n");
        for (name, hint) in [
            ("my new branch", "spaces"),
            ("-x", "“-”"),
            ("a..b", "“..”"),
            ("bad~name", "“~”"),
        ] {
            let error = act(
                p,
                Request::CreateBranch {
                    name: name.into(),
                    target: first.clone(),
                },
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(hint) && error.contains("Example"), "{error}");
        }
    }

    #[test]
    fn remote_management_and_upstream() {
        let d = repository();
        let p = d.path();
        let first = commit(p, "first", "first\n");
        act(
            p,
            Request::AddRemote {
                name: "origin".into(),
                url: "/tmp/local-repository".into(),
            },
        )
        .unwrap();
        git(p, &["update-ref", "refs/remotes/origin/main", &first]).unwrap();
        act(
            p,
            Request::SetUpstream {
                branch: "main".into(),
                upstream: "origin/main".into(),
            },
        )
        .unwrap();
        assert_eq!(
            snapshot(p)
                .unwrap()
                .refs
                .iter()
                .find(|r| r.kind == "branch")
                .unwrap()
                .upstream
                .as_deref(),
            Some("origin/main")
        );
        act(
            p,
            Request::SetRemoteUrl {
                name: "origin".into(),
                url: "/tmp/changed-repository".into(),
            },
        )
        .unwrap();
        act(
            p,
            Request::RenameRemote {
                name: "origin".into(),
                new_name: "backup".into(),
            },
        )
        .unwrap();
        let state = snapshot(p).unwrap();
        assert_eq!(state.remotes[0].name, "backup");
        assert_eq!(state.remotes[0].fetch_url, "/tmp/changed-repository");
        assert!(act(
            p,
            Request::AddRemote {
                name: "--fetch".into(),
                url: "ext::bad".into()
            }
        )
        .is_err());
        act(
            p,
            Request::RemoveRemote {
                name: "backup".into(),
            },
        )
        .unwrap();
        assert!(snapshot(p).unwrap().remotes.is_empty());
    }

    #[test]
    fn a_branch_whose_upstream_was_deleted_is_reported_as_gone() {
        let d = repository();
        let p = d.path();
        commit(p, "first", "first\n");
        git(p, &["remote", "add", "origin", "."]).unwrap();
        git(p, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
        git(p, &["branch", "--set-upstream-to=origin/main", "main"]).unwrap();
        let main = |p: &Path| {
            refs(p)
                .unwrap()
                .into_iter()
                .find(|r| r.full_name == "refs/heads/main")
                .unwrap()
        };
        assert!(!main(p).upstream_gone);
        git(p, &["update-ref", "-d", "refs/remotes/origin/main"]).unwrap();
        assert!(main(p).upstream_gone);
    }

    #[test]
    fn references_list_the_newest_tags_first() {
        let d = repository();
        let p = d.path();
        commit(p, "first", "first\n");
        // Created oldest-last in name order so that alphabetical order would be wrong.
        for (name, date) in [
            ("v1", "2024-01-01T10:00:00Z"),
            ("v2", "2024-03-01T10:00:00Z"),
            ("v0", "2024-02-01T10:00:00Z"),
        ] {
            let status = std::process::Command::new("git")
                .current_dir(p)
                .env("GIT_COMMITTER_DATE", date)
                .args(["tag", "-a", "-m", name, name])
                .status()
                .unwrap();
            assert!(status.success());
        }
        let tags: Vec<_> = refs(p)
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == "tag")
            .map(|r| r.name)
            .collect();
        assert_eq!(tags, ["v2", "v0", "v1"]);
    }

    #[test]
    fn stash_save_apply_pop_and_drop_preserve_data() {
        let d = repository();
        let p = d.path();
        commit(p, "first", "first\n");
        std::fs::write(p.join("file.txt"), "modified\n").unwrap();
        std::fs::write(p.join("[literal].txt"), "untracked\n").unwrap();
        act(
            p,
            Request::StashSave {
                message: "my stash".into(),
                keep_index: false,
                tracked_only: false,
                files: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "first\n"
        );
        assert!(!p.join("[literal].txt").exists());
        let state = snapshot(p).unwrap();
        assert_eq!(state.stashes.len(), 1);
        let id = state.stashes[0].id.clone();
        act(p, Request::StashApply { id: id.clone() }).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "modified\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("[literal].txt")).unwrap(),
            "untracked\n"
        );
        assert_eq!(snapshot(p).unwrap().stashes.len(), 1);
        assert!(act(p, Request::StashPop { id: id.clone() }).is_err());
        assert_eq!(snapshot(p).unwrap().stashes.len(), 1);
        act(
            p,
            Request::StashSave {
                message: "second stash".into(),
                keep_index: false,
                tracked_only: false,
                files: Vec::new(),
            },
        )
        .unwrap();
        act(p, Request::StashPop { id: id.clone() }).unwrap();
        assert!(!snapshot(p).unwrap().stashes.iter().any(|s| s.id == id));
        assert!(git(
            p,
            &["show-ref", "--verify", &format!("refs/gall/recovery/{id}")]
        )
        .is_ok());
        let remaining = snapshot(p).unwrap().stashes[0].id.clone();
        act(
            p,
            Request::StashDrop {
                id: remaining.clone(),
            },
        )
        .unwrap();
        assert!(snapshot(p).unwrap().stashes.is_empty());
        assert!(git(
            p,
            &[
                "show-ref",
                "--verify",
                &format!("refs/gall/recovery/{remaining}")
            ]
        )
        .is_ok());
    }

    #[test]
    fn stash_rename_preserves_all_stash_trees_and_worktree_bytes() {
        let d = repository();
        let p = d.path();
        commit(p, "Base", "base\n");
        std::fs::write(p.join("file.txt"), "staged stash\n").unwrap();
        git(p, &["add", "--", "file.txt"]).unwrap();
        std::fs::write(p.join("file.txt"), "working stash\n").unwrap();
        std::fs::write(p.join("extra.txt"), "untracked stash\n").unwrap();
        git(
            p,
            &["stash", "push", "--include-untracked", "-m", "old name"],
        )
        .unwrap();
        let old = stashes(p).unwrap()[0].id.clone();
        std::fs::write(p.join("file.txt"), "second stash\n").unwrap();
        git(p, &["stash", "push", "-m", "second"]).unwrap();
        let second = stashes(p).unwrap()[0].id.clone();
        std::fs::write(p.join("file.txt"), "current unstaged\n").unwrap();

        let rename = |name: &str| {
            act(
                p,
                Request::StashRename {
                    id: old.clone(),
                    name: name.into(),
                },
            )
        };
        assert!(rename("bad\nname").is_err());
        assert_eq!(stashes(p).unwrap()[1].id, old);
        rename("new name").unwrap();
        let list = stashes(p).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].message.ends_with("new name"));
        assert_eq!(list[1].id, second);
        let new = &list[0].id;
        for tree in ["^{tree}", "^1^{tree}", "^2^{tree}", "^3^{tree}"] {
            assert_eq!(
                git(p, &["rev-parse", &format!("{new}{tree}")]).unwrap(),
                git(p, &["rev-parse", &format!("{old}{tree}")]).unwrap()
            );
        }
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "current unstaged\n"
        );
        assert!(git(
            p,
            &["show-ref", "--verify", &format!("refs/gall/recovery/{old}")]
        )
        .is_ok());
    }

    #[test]
    fn partial_stashes_leave_the_rest_and_a_stash_becomes_a_branch() {
        let d = repository();
        let p = d.path();
        commit(p, "initial", "original\n");
        let save = |keep_index, tracked_only, files: &[&str]| {
            act(
                p,
                Request::StashSave {
                    message: "part".into(),
                    keep_index,
                    tracked_only,
                    files: files.iter().map(|f| f.to_string()).collect(),
                },
            )
        };
        let text = |file: &str| std::fs::read_to_string(p.join(file)).unwrap_or_default();
        let write = |file: &str, text: &str| std::fs::write(p.join(file), text).unwrap();
        let original = text("file.txt");
        // Only the chosen files, also one whose name is a pattern and a new one.
        write("[a].txt", "new\n");
        write("a.txt", "other\n");
        write("file.txt", "changed\n");
        save(false, false, &["[a].txt"]).unwrap();
        assert!(!p.join("[a].txt").exists());
        assert_eq!(
            (text("a.txt"), text("file.txt")),
            ("other\n".into(), "changed\n".into())
        );
        // Nothing there any more, and nothing outside the repository.
        assert!(save(false, false, &["[a].txt"]).is_err());
        assert!(save(false, false, &["../x"]).is_err());
        // New files stay.
        save(false, true, &[]).unwrap();
        assert_eq!(
            (text("a.txt"), text("file.txt")),
            ("other\n".into(), original.clone())
        );
        assert!(save(false, true, &[]).is_err());
        assert_eq!(snapshot(p).unwrap().stashes.len(), 2);
        // Staged changes stay staged.
        write("file.txt", "staged\n");
        git(p, &["add", "file.txt"]).unwrap();
        write("file.txt", "unstaged\n");
        save(true, false, &[]).unwrap();
        assert_eq!(text("file.txt"), "staged\n");
        assert_eq!(git(p, &["show", ":file.txt"]).unwrap(), "staged\n");
        git(p, &["reset", "-q", "--hard"]).unwrap();
        // A branch where the stash was made, although the current branch went on.
        let id = snapshot(p).unwrap().stashes[0].id.clone();
        let base = git(p, &["rev-parse", &format!("{id}^1")]).unwrap();
        write("file.txt", "gone on\n");
        git(p, &["commit", "-qam", "gone on"]).unwrap();
        let branch = |name: &str| {
            act(
                p,
                Request::StashBranch {
                    id: id.clone(),
                    name: name.into(),
                },
            )
        };
        write("file.txt", "dirty\n");
        assert!(branch("from-stash").is_err());
        git(p, &["reset", "-q", "--hard"]).unwrap();
        assert!(branch("bad..name").is_err());
        branch("from-stash").unwrap();
        assert_eq!(git(p, &["rev-parse", "HEAD"]).unwrap(), base);
        assert_eq!(text("file.txt"), "unstaged\n");
        assert_eq!(git(p, &["show", ":file.txt"]).unwrap(), "staged\n");
        assert!(!snapshot(p).unwrap().stashes.iter().any(|s| s.id == id));
        assert!(git(p, &["rev-parse", &format!("refs/gall/recovery/{id}")]).is_ok());
    }

    #[test]
    fn a_stash_branch_is_made_with_unrelated_local_changes() {
        let d = repository();
        let p = d.path();
        let write = |file: &str, text: &str| std::fs::write(p.join(file), text).unwrap();
        let text = |file: &str| std::fs::read_to_string(p.join(file)).unwrap();
        write("a.txt", "a\n");
        write("b.txt", "b\n");
        commit(p, "one", "1\n");
        write("a.txt", "a stashed\n");
        git(p, &["stash", "push", "-q"]).unwrap();
        let id = snapshot(p).unwrap().stashes[0].id.clone();
        commit(p, "two", "2\n");
        write("b.txt", "b local\n");
        write("untracked.txt", "keep\n");
        act(
            p,
            Request::StashBranch {
                id: id.clone(),
                name: "from-stash".into(),
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            git(p, &["branch", "--show-current"]).unwrap().trim(),
            "from-stash"
        );
        assert_eq!(
            (text("a.txt"), text("b.txt"), text("untracked.txt")),
            ("a stashed\n".into(), "b local\n".into(), "keep\n".into())
        );
        assert!(snapshot(p).unwrap().stashes.is_empty());
    }

    #[test]
    fn conflicting_stash_pop_keeps_stack_entry() {
        let d = repository();
        let p = d.path();
        commit(p, "first", "first\n");
        std::fs::write(p.join("file.txt"), "stash contents\n").unwrap();
        act(
            p,
            Request::StashSave {
                message: "conflicting".into(),
                keep_index: false,
                tracked_only: false,
                files: Vec::new(),
            },
        )
        .unwrap();
        let id = snapshot(p).unwrap().stashes[0].id.clone();
        commit(p, "conflict", "branch contents\n");
        assert!(act(p, Request::StashPop { id: id.clone() }).is_err());
        let state = snapshot(p).unwrap();
        assert_eq!(state.stashes.len(), 1);
        assert_eq!(state.stashes[0].id, id);
        assert_eq!(state.conflicts, ["file.txt"]);
    }

    fn conflict(p: &Path) -> (String, String) {
        commit(p, "first", "first\n");
        git(p, &["switch", "-qc", "other"]).unwrap();
        let other = commit(p, "other", "other\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        let main = commit(p, "main", "main\n");
        (main, other)
    }

    /// A final conflict where the replay adds attributes after CRLF bytes were indexed.
    fn attribute_rebase() -> tempfile::TempDir {
        let d = repository();
        let p = d.path();
        let exclude = p.join(".git/excludes-empty");
        std::fs::write(&exclude, "").unwrap();
        git(
            p,
            &["config", "core.excludesFile", exclude.to_str().unwrap()],
        )
        .unwrap();
        std::fs::create_dir_all(p.join("nested")).unwrap();
        std::fs::write(p.join("nested/č file.data"), b"first\r\nsecond\r\n").unwrap();
        std::fs::write(p.join("file.txt"), "base\n").unwrap();
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "base"]).unwrap();
        git(p, &["switch", "-qc", "topic"]).unwrap();
        std::fs::write(p.join(".gitattributes"), "*.data text eol=lf\n").unwrap();
        std::fs::write(p.join("file.txt"), "topic\n").unwrap();
        git(p, &["add", "--", ".gitattributes", "file.txt"]).unwrap();
        git(p, &["commit", "-qm", "topic"]).unwrap();
        git(p, &["switch", "-q", "main"]).unwrap();
        std::fs::write(p.join("file.txt"), "main\n").unwrap();
        git(p, &["commit", "-qam", "main"]).unwrap();
        git(p, &["switch", "-q", "topic"]).unwrap();
        assert!(git(p, &["rebase", "main"]).is_err());
        std::fs::write(p.join("file.txt"), "resolved\n").unwrap();
        git(p, &["add", "--", "file.txt"]).unwrap();
        std::fs::write(p.join("nested/č file.data"), b"first\r\nsecond\r\n").unwrap();
        // Coarse clocks, such as Windows' default tick, can keep the old mtime and
        // let Git trust the index stat instead of rereading the file.
        std::fs::File::options()
            .write(true)
            .open(p.join("nested/č file.data"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        std::fs::write(p.join("unrelated untracked"), b"keep\n").unwrap();
        d
    }

    #[test]
    fn final_rebase_continues_when_staged_attributes_only_make_identical_bytes_look_dirty() {
        let d = attribute_rebase();
        let p = d.path();
        let source = head(p);
        if configured_git_filters(p).unwrap_or(true) {
            // A machine-wide Git LFS or other filter must disable this special path too.
            assert!(attribute_only_rebase_source(p).is_none());
            assert!(control(p, "--continue").is_err());
            assert_eq!(
                std::fs::read(p.join("nested/č file.data")).unwrap(),
                b"first\r\nsecond\r\n"
            );
            return;
        }
        if super::super::gitgraph::git_with_attr_source(
            p,
            &[
                "-c",
                "core.safecrlf=false",
                "diff",
                "--quiet",
                "--ignore-submodules=dirty",
                "--",
            ],
            &source,
        )
        .is_err()
        {
            // Git before 2.41 ignores GIT_ATTR_SOURCE. It must not take the special path.
            assert!(attribute_only_rebase_source(p).is_none());
            assert!(control(p, "--continue").is_err());
            assert_eq!(
                std::fs::read(p.join("nested/č file.data")).unwrap(),
                b"first\r\nsecond\r\n"
            );
            return;
        }
        assert!(attribute_only_rebase_source(p).is_some());
        control(p, "--continue").unwrap();
        assert_eq!(
            git(p, &["branch", "--show-current"]).unwrap().trim(),
            "topic"
        );
        assert_eq!(git(p, &["show", "HEAD:file.txt"]).unwrap(), "resolved\n");
        assert_eq!(
            std::fs::read(p.join("nested/č file.data")).unwrap(),
            b"first\r\nsecond\r\n"
        );
        assert_eq!(
            std::fs::read(p.join("unrelated untracked")).unwrap(),
            b"keep\n"
        );
        assert!(operation(p).unwrap().is_none());
    }

    #[test]
    fn rebase_error_keeps_the_actual_failure_and_describes_paths_as_context() {
        let error =
            rebase_failure_context(fail("Author identity is missing"), &["nested/č file.data"])
                .to_string();
        assert!(
            error.starts_with("Git: Author identity is missing"),
            "{error}"
        );
        assert!(
            error.contains("Current unstaged files") && error.contains("Working Changes"),
            "{error}"
        );
        assert!(
            !error.contains("prevent continuing") && !error.contains("stash"),
            "{error}"
        );
    }

    #[test]
    fn attribute_rebase_does_not_bypass_real_edits_or_pending_work() {
        let d = attribute_rebase();
        let p = d.path();
        let source = head(p);
        if configured_git_filters(p).unwrap_or(true) {
            // A machine-wide Git LFS or other filter must disable this special path too.
            assert!(attribute_only_rebase_source(p).is_none());
            assert!(control(p, "--continue").is_err());
            assert_eq!(
                std::fs::read(p.join("nested/č file.data")).unwrap(),
                b"first\r\nsecond\r\n"
            );
            return;
        }
        if super::super::gitgraph::git_with_attr_source(
            p,
            &[
                "-c",
                "core.safecrlf=false",
                "diff",
                "--quiet",
                "--ignore-submodules=dirty",
                "--",
            ],
            &source,
        )
        .is_err()
        {
            assert!(attribute_only_rebase_source(p).is_none());
            return;
        }
        assert!(attribute_only_rebase_source(p).is_some());
        let todo = p.join(".git/rebase-merge/git-rebase-todo");
        std::fs::write(&todo, "exec false\n").unwrap();
        assert!(attribute_only_rebase_source(p).is_none());
        std::fs::write(&todo, "").unwrap();
        for marker in ["autostash", "update-refs"] {
            let file = p.join(".git/rebase-merge").join(marker);
            std::fs::write(&file, "preserve\n").unwrap();
            assert!(attribute_only_rebase_source(p).is_none());
            std::fs::remove_file(file).unwrap();
        }
        git(p, &["config", "filter.example.clean", "cat"]).unwrap();
        assert!(attribute_only_rebase_source(p).is_none());
        git(p, &["config", "--unset", "filter.example.clean"]).unwrap();
        std::fs::write(p.join("nested/č file.data"), b"real local edit\r\n").unwrap();
        let staged = git(p, &["diff", "--cached", "--binary"]).unwrap();
        assert!(attribute_only_rebase_source(p).is_none());
        let error = control(p, "--continue").unwrap_err().to_string();
        assert!(
            error.contains("Working Changes") && error.contains("file.data"),
            "{error}"
        );
        assert_eq!(git(p, &["diff", "--cached", "--binary"]).unwrap(), staged);
        assert_eq!(
            std::fs::read(p.join("nested/č file.data")).unwrap(),
            b"real local edit\r\n"
        );
        assert!(operation(p).unwrap().is_some());
    }

    #[test]
    fn resolve_content_writes_and_stages_the_chosen_text() {
        let d = repository();
        let p = d.path();
        let (_, other) = conflict(p);
        assert!(act(p, Request::Merge { target: other }).is_err());
        assert!(act(
            p,
            Request::ResolveContent {
                file: "../file.txt".into(),
                content: "x".into()
            }
        )
        .is_err());
        act(
            p,
            Request::ResolveContent {
                file: "file.txt".into(),
                content: "both\nsides\n".into(),
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "both\nsides\n"
        );
        assert!(snapshot(p).unwrap().conflicts.is_empty());
        act(p, Request::Continue).unwrap();
        assert_eq!(git(p, &["show", "HEAD:file.txt"]).unwrap(), "both\nsides\n");
    }
    #[test]
    fn merge_abort_checks_only_paths_it_would_restore() {
        for staged_deletion in [false, true] {
            let d = repository();
            let p = d.path();
            std::fs::write(p.join("local"), "tracked before merge\n").unwrap();
            let (main, other) = conflict(p);
            assert!(act(p, Request::Merge { target: other }).is_err());
            std::fs::remove_file(p.join("local")).unwrap();
            if staged_deletion {
                git(p, &["rm", "--cached", "--", "local"]).unwrap();
            }
            std::fs::create_dir(p.join("local")).unwrap();
            std::fs::write(p.join(".git/info/exclude"), "*.tmp\n").unwrap();
            std::fs::write(p.join("local/keep.tmp"), "private\n").unwrap();

            let result = act(p, Request::Abort);
            if staged_deletion {
                assert!(result.unwrap_err().to_string().contains("local/keep.tmp"));
                assert_eq!(snapshot(p).unwrap().operation.as_deref(), Some("merge"));
            } else {
                result.unwrap();
                assert!(snapshot(p).unwrap().operation.is_none());
            }
            assert_eq!(head(p), main);
            assert_eq!(
                std::fs::read_to_string(p.join("local/keep.tmp")).unwrap(),
                "private\n"
            );
        }
    }

    #[test]
    fn merge_continue_preserves_an_ignored_replacement_for_a_staged_deletion() {
        let d = repository();
        let p = d.path();
        std::fs::write(p.join("local.tmp"), "old tracked bytes\n").unwrap();
        let (main, other) = conflict(p);
        assert!(act(
            p,
            Request::Merge {
                target: other.clone()
            }
        )
        .is_err());
        act(
            p,
            Request::Resolve {
                file: "file.txt".into(),
                side: ResolveSide::Ours,
            },
        )
        .unwrap();
        git(p, &["rm", "--cached", "--", "local.tmp"]).unwrap();
        std::fs::write(p.join(".git/info/exclude"), "*.tmp\n").unwrap();
        std::fs::write(p.join("local.tmp"), "keep local bytes\n").unwrap();
        act(p, Request::Continue).unwrap();

        assert_eq!(
            git(p, &["show", "-s", "--format=%P", "HEAD"])
                .unwrap()
                .trim(),
            format!("{main} {other}")
        );
        assert!(git(p, &["cat-file", "-e", "HEAD:local.tmp"]).is_err());
        assert_eq!(
            std::fs::read_to_string(p.join("local.tmp")).unwrap(),
            "keep local bytes\n"
        );
        assert!(snapshot(p).unwrap().operation.is_none());
    }

    #[test]
    fn merge_conflict_resolve_continue_and_abort() {
        let d = repository();
        let p = d.path();
        let (_, other) = conflict(p);
        assert!(act(
            p,
            Request::Merge {
                target: other.clone()
            }
        )
        .is_err());
        let state = snapshot(p).unwrap();
        assert_eq!(state.operation.as_deref(), Some("merge"));
        assert_eq!(state.conflicts, ["file.txt"]);
        let applying = state.operation_commit.expect("merged commit is reported");
        assert!(!applying.subject.is_empty() && applying.step.is_none());
        assert!(act(p, Request::Continue).is_err());
        assert!(act(p, Request::Skip).is_err());
        assert!(act(
            p,
            Request::Resolve {
                file: "../file.txt".into(),
                side: ResolveSide::Ours
            }
        )
        .is_err());
        act(
            p,
            Request::Resolve {
                file: "file.txt".into(),
                side: ResolveSide::Theirs,
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "other\n"
        );
        act(p, Request::Continue).unwrap();
        assert!(snapshot(p).unwrap().operation.is_none());
        assert_eq!(
            git(p, &["rev-list", "--parents", "-n", "1", "HEAD"])
                .unwrap()
                .split_whitespace()
                .count(),
            3
        );

        let d = repository();
        let p = d.path();
        let (main, other) = conflict(p);
        assert!(act(p, Request::Merge { target: other }).is_err());
        act(p, Request::Abort).unwrap();
        assert!(snapshot(p).unwrap().operation.is_none());
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "main\n"
        );
        // The pre-operation commit has a durable recovery ref even on a branch.
        assert!(git(
            d.path(),
            &[
                "show-ref",
                "--verify",
                &format!("refs/gall/recovery/{main}")
            ]
        )
        .is_ok());
    }

    #[test]
    fn reset_modes_retain_head_and_hard_reset_stashes_tracked_changes() {
        let d = repository();
        let p = d.path();
        let first = commit(p, "first", "first\n");
        let second = commit(p, "second", "second\n");
        act(
            p,
            Request::Reset {
                target: first.clone(),
                mode: ResetMode::Soft,
            },
        )
        .unwrap();
        assert_eq!(head(p), first);
        assert_eq!(git(p, &["show", ":file.txt"]).unwrap(), "second\n");
        assert!(git(
            p,
            &[
                "show-ref",
                "--verify",
                &format!("refs/gall/recovery/{second}")
            ]
        )
        .is_ok());
        act(
            p,
            Request::Reset {
                target: second.clone(),
                mode: ResetMode::Mixed,
            },
        )
        .unwrap();
        std::fs::write(p.join("file.txt"), "uncommitted\n").unwrap();
        act(
            p,
            Request::Reset {
                target: first.clone(),
                mode: ResetMode::Hard,
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "first\n"
        );
        // The recovery copy is kept as a reference, not as a visible stash entry.
        let state = snapshot(p).unwrap();
        assert!(state.stashes.is_empty());
        let saved = git(
            p,
            &[
                "for-each-ref",
                "--format=%(objectname)",
                "refs/gall/recovery",
            ],
        )
        .unwrap();
        assert!(saved.lines().any(|id| {
            git(p, &["show", &format!("{id}:file.txt")]).is_ok_and(|text| text == "uncommitted\n")
        }));
        // Unrelated untracked and ignored files must not block a hard reset.
        std::fs::write(p.join(".gitignore"), "build/\n").unwrap();
        git(p, &["add", ".gitignore"]).unwrap();
        git(p, &["commit", "-qm", "ignore"]).unwrap();
        std::fs::create_dir(p.join("build")).unwrap();
        std::fs::write(p.join("build").join("out.bin"), "ignored\n").unwrap();
        std::fs::write(p.join("notes.txt"), "keep me\n").unwrap();
        act(
            p,
            Request::Reset {
                target: second.clone(),
                mode: ResetMode::Hard,
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("notes.txt")).unwrap(),
            "keep me\n"
        );
        assert!(p.join("build").join("out.bin").exists());
        // An untracked file that the target commit would overwrite still blocks it.
        std::fs::write(p.join("extra.txt"), "tracked later\n").unwrap();
        git(p, &["add", "extra.txt"]).unwrap();
        git(p, &["commit", "-qm", "extra"]).unwrap();
        let third = head(p);
        act(
            p,
            Request::Reset {
                target: second,
                mode: ResetMode::Hard,
            },
        )
        .unwrap();
        std::fs::write(p.join("extra.txt"), "preserve me\n").unwrap();
        assert!(act(
            p,
            Request::Reset {
                target: third,
                mode: ResetMode::Hard
            }
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(p.join("extra.txt")).unwrap(),
            "preserve me\n"
        );
    }

    /// `main` and a branch `topic` with three commits, of which the second conflicts with `main`.
    fn picks(p: &Path) -> (String, Vec<String>) {
        commit(p, "first", "first\n");
        git(p, &["switch", "-qc", "topic"]).unwrap();
        let add = |name: &str| {
            std::fs::write(p.join(name), "new\n").unwrap();
            git(p, &["add", "--all"]).unwrap();
            git(p, &["commit", "-qm", name]).unwrap();
            head(p)
        };
        let one = add("one.txt");
        let two = commit(p, "two", "topic\n");
        let three = add("three.txt");
        git(p, &["switch", "-q", "main"]).unwrap();
        (commit(p, "main", "main\n"), vec![one, two, three])
    }

    #[test]
    fn many_picks_stop_at_a_conflict_and_continue_or_abort_as_one() {
        for finish in [true, false] {
            let d = repository();
            let p = d.path();
            let (main, targets) = picks(p);
            let request = || Request::CherryPickMany {
                targets: targets.clone(),
            };
            assert!(act(p, request()).is_err());
            let state = snapshot(p).unwrap();
            assert_eq!(state.operation.as_deref(), Some("cherry-pick"));
            assert_eq!(state.conflicts, vec!["file.txt"]);
            // The first commit is there, the third not yet.
            assert!(p.join("one.txt").exists() && !p.join("three.txt").exists());
            assert!(act(p, request()).is_err(), "no second operation meanwhile");
            if finish {
                act(
                    p,
                    Request::ResolveContent {
                        file: "file.txt".into(),
                        content: "both\n".into(),
                    },
                )
                .unwrap();
                act(p, Request::Continue).unwrap();
                assert!(p.join("three.txt").exists());
                let subjects = git(p, &["log", "--format=%s", "-4"]).unwrap();
                assert_eq!(subjects, "three.txt\ntwo\none.txt\nmain\n");
            } else {
                act(p, Request::Abort).unwrap();
                assert_eq!(head(p), main);
                assert!(!p.join("one.txt").exists());
            }
            assert_eq!(snapshot(p).unwrap().operation, None);
            // The state before the operation is kept for recovery.
            git(
                p,
                &[
                    "rev-parse",
                    "--verify",
                    &format!("refs/gall/recovery/{main}"),
                ],
            )
            .unwrap();
        }
    }

    #[test]
    fn many_picks_run_with_an_unrelated_unstaged_change_and_stop_cleanly_with_a_staged_one() {
        let d = repository();
        let p = d.path();
        let (main, targets) = picks(p);
        // `one.txt` and `three.txt` do not touch `file.txt`.
        let pair = vec![targets[0].clone(), targets[2].clone()];
        let write = |text: &str| std::fs::write(p.join("file.txt"), text).unwrap();
        let text = || std::fs::read_to_string(p.join("file.txt")).unwrap();
        // A staged change: Git itself refuses a several-commit pick, as it does for lazygit, and
        // nothing changes, no half-finished operation remains.
        write("staged\n");
        git(p, &["add", "file.txt"]).unwrap();
        let error = act(
            p,
            Request::CherryPickMany {
                targets: pair.clone(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("local changes"), "{error}");
        assert_eq!(head(p), main);
        assert_eq!(text(), "staged\n");
        assert_eq!(git(p, &["show", ":file.txt"]).unwrap(), "staged\n");
        assert_eq!(snapshot(p).unwrap().operation, None);
        // The same change, not staged, does not block it.
        git(p, &["reset", "-q"]).unwrap();
        act(p, Request::CherryPickMany { targets: pair }).unwrap_or_else(|e| panic!("{e}"));
        assert!(p.join("one.txt").exists() && p.join("three.txt").exists());
        assert_eq!(text(), "staged\n");
        assert_eq!(snapshot(p).unwrap().operation, None);
    }

    #[test]
    fn many_picks_keep_their_order_and_refuse_bad_lists_before_changing_anything() {
        let d = repository();
        let p = d.path();
        let (main, targets) = picks(p);
        let (one, three) = (targets[0].clone(), targets[2].clone());
        let refuse = |targets: Vec<String>, expected: &str| {
            let error = act(p, Request::CherryPickMany { targets })
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
            assert_eq!(head(p), main);
            assert_eq!(snapshot(p).unwrap().operation, None);
        };
        refuse(vec![], "Select between");
        refuse(vec![one.clone(), one.clone()], "twice");
        refuse(vec!["HEAD".into()], "concrete");
        refuse(vec!["0".repeat(40)], "");
        // A merge among them would stop Git halfway.
        git(p, &["switch", "-qc", "merged", &three]).unwrap();
        git(
            p,
            &[
                "merge", "-q", "--no-ff", "-m", "merge", "-s", "ours", "main",
            ],
        )
        .unwrap();
        let merge = head(p);
        git(p, &["switch", "-q", "main"]).unwrap();
        refuse(vec![one.clone(), merge], "is a merge");
        // An ignored file in the way of the last of them.
        std::fs::write(p.join(".git/info/exclude"), "three.txt\n").unwrap();
        std::fs::write(p.join("three.txt"), "mine\n").unwrap();
        refuse(vec![one.clone(), three.clone()], "three.txt");
        assert_eq!(
            std::fs::read_to_string(p.join("three.txt")).unwrap(),
            "mine\n"
        );
        std::fs::remove_file(p.join("three.txt")).unwrap();
        // A folder the user keeps there without knowing.
        std::fs::create_dir(p.join("three.txt")).unwrap();
        std::fs::write(p.join("three.txt").join("note"), "mine\n").unwrap();
        refuse(vec![one.clone(), three.clone()], "three.txt");
        std::fs::remove_dir_all(p.join("three.txt")).unwrap();

        act(
            p,
            Request::CherryPickMany {
                targets: vec![one, three],
            },
        )
        .unwrap();
        assert_eq!(
            git(p, &["log", "--format=%s", "-3"]).unwrap(),
            "three.txt\none.txt\nmain\n"
        );
    }

    #[test]
    fn cherry_pick_revert_and_rebase_leave_resumable_state() {
        let d = repository();
        let p = d.path();
        let (main, other) = conflict(p);
        assert!(act(
            p,
            Request::CherryPick {
                target: other.clone()
            }
        )
        .is_err());
        assert_eq!(
            snapshot(p).unwrap().operation.as_deref(),
            Some("cherry-pick")
        );
        act(p, Request::Abort).unwrap();
        assert_eq!(head(p), main);
        assert!(act(p, Request::Rebase { target: other }).is_err());
        let state = snapshot(p).unwrap();
        assert_eq!(state.operation.as_deref(), Some("rebase"));
        let applying = state.operation_commit.expect("replayed commit is reported");
        assert!(!applying.subject.is_empty());
        assert_eq!(applying.step, Some((1, 1)));
        act(p, Request::Abort).unwrap();
        assert_eq!(head(p), main);
        act(p, Request::Revert { target: main }).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "first\n"
        );
    }

    /// What lazygit allows and Gall must too: untracked files that are not in `.gitignore` and
    /// that an operation does not write never block it, and one it would overwrite is refused
    /// with its content kept.
    #[test]
    fn untracked_files_block_no_operation_unless_it_would_overwrite_them() {
        let d = repository();
        let p = d.path();
        commit(p, "one", "1\n");
        git(p, &["checkout", "-q", "-b", "topic"]).unwrap();
        std::fs::write(p.join("topic.txt"), "topic\n").unwrap();
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "topic"]).unwrap();
        let topic = head(p);
        git(p, &["checkout", "-q", "main"]).unwrap();
        std::fs::write(p.join("main.txt"), "main\n").unwrap();
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "main"]).unwrap();
        let main = head(p);
        git(p, &["checkout", "-q", "topic"]).unwrap();
        // Not ignored anywhere: a log, a build folder, a note with a space and a non-ASCII name.
        std::fs::write(p.join("scratch.log"), "local\n").unwrap();
        std::fs::create_dir(p.join("build")).unwrap();
        std::fs::write(p.join("build").join("out.bin"), "local\n").unwrap();
        std::fs::write(p.join("my notes č.txt"), "local\n").unwrap();
        let kept = |p: &Path| {
            assert_eq!(
                std::fs::read_to_string(p.join("scratch.log")).unwrap(),
                "local\n"
            );
            assert!(p.join("build").join("out.bin").exists());
            assert!(p.join("my notes č.txt").exists());
        };
        let back = |p: &Path| {
            git(p, &["reset", "-q", "--hard", &topic]).unwrap();
        };

        act(
            p,
            Request::Rebase {
                target: main.clone(),
            },
        )
        .unwrap();
        assert!(p.join("main.txt").exists() && p.join("topic.txt").exists());
        kept(p);
        back(p);
        act(
            p,
            Request::Merge {
                target: main.clone(),
            },
        )
        .unwrap();
        assert!(p.join("main.txt").exists());
        kept(p);
        back(p);
        act(
            p,
            Request::CherryPick {
                target: main.clone(),
            },
        )
        .unwrap();
        assert!(p.join("main.txt").exists());
        kept(p);
        back(p);
        act(
            p,
            Request::CherryPickMany {
                targets: vec![main.clone()],
            },
        )
        .unwrap();
        kept(p);
        back(p);
        act(
            p,
            Request::Revert {
                target: topic.clone(),
            },
        )
        .unwrap();
        assert!(!p.join("topic.txt").exists());
        kept(p);
        back(p);
        std::fs::write(p.join("file.txt"), "stashed\n").unwrap();
        git(p, &["stash", "push", "-q"]).unwrap();
        let stash = git(p, &["rev-parse", "stash@{0}"]).unwrap();
        act(
            p,
            Request::StashApply {
                id: stash.trim().into(),
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "stashed\n"
        );
        kept(p);
        git(p, &["reset", "-q", "--hard", &topic]).unwrap();

        // An untracked file at a path the operation writes is refused and keeps its content.
        std::fs::write(p.join("main.txt"), "mine\n").unwrap();
        for request in [
            Request::Rebase {
                target: main.clone(),
            },
            Request::Merge {
                target: main.clone(),
            },
            Request::CherryPick {
                target: main.clone(),
            },
            Request::CherryPickMany {
                targets: vec![main.clone()],
            },
        ] {
            assert!(act(p, request).is_err());
            assert_eq!(
                std::fs::read_to_string(p.join("main.txt")).unwrap(),
                "mine\n"
            );
            assert_eq!(head(p), topic);
            assert!(snapshot(p).unwrap().operation.is_none());
        }
    }

    /// Tracked local changes: Git and lazygit run merge, cherry-pick and revert when the change does
    /// not overlap, and lazygit rebases with `--autostash`. Gall must not refuse more than that, and
    /// an overlapping change must stay where it is when the operation is refused.
    #[test]
    fn tracked_local_changes_block_an_operation_only_where_git_would() {
        let d = repository();
        let p = d.path();
        let write = |file: &str, text: &str| std::fs::write(p.join(file), text).unwrap();
        let text = |file: &str| std::fs::read_to_string(p.join(file)).unwrap();
        write("mine.txt", "base\n");
        commit(p, "one", "1\n");
        git(p, &["checkout", "-q", "-b", "topic"]).unwrap();
        write("topic.txt", "topic\n");
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "topic"]).unwrap();
        let topic = head(p);
        git(p, &["checkout", "-q", "main"]).unwrap();
        write("main.txt", "main\n");
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "main"]).unwrap();
        let main = head(p);
        git(p, &["checkout", "-q", "topic"]).unwrap();
        let back = |p: &Path| git(p, &["reset", "-q", "--keep", &topic]).unwrap();
        // A change that no commit of the operation touches: staged in one run, unstaged in the other.
        for staged in [false, true] {
            write("mine.txt", "local edit\n");
            if staged {
                git(p, &["add", "mine.txt"]).unwrap();
            }
            for request in [
                Request::Rebase {
                    target: main.clone(),
                },
                Request::Merge {
                    target: main.clone(),
                },
                Request::CherryPick {
                    target: main.clone(),
                },
                Request::Revert {
                    target: topic.clone(),
                },
            ] {
                let name = format!("{request:?}");
                act(p, request).unwrap_or_else(|e| panic!("{name} (staged: {staged}): {e}"));
                assert_eq!(text("mine.txt"), "local edit\n", "{name}");
                assert!(snapshot(p).unwrap().operation.is_none(), "{name}");
                back(p);
                assert_eq!(text("mine.txt"), "local edit\n", "{name}");
            }
            git(p, &["checkout", "-q", "--", "mine.txt"]).unwrap();
            git(p, &["reset", "-q", "--", "mine.txt"]).unwrap();
            git(p, &["checkout", "-q", "--", "mine.txt"]).unwrap();
        }
        // A change that the operation would overwrite: Git refuses and nothing moves.
        git(p, &["checkout", "-q", "main"]).unwrap();
        write("mine.txt", "main edit\n");
        git(p, &["commit", "-qam", "main edits mine"]).unwrap();
        let main = head(p);
        git(p, &["checkout", "-q", "topic"]).unwrap();
        write("mine.txt", "local edit\n");
        for request in [
            Request::Merge {
                target: main.clone(),
            },
            Request::CherryPick {
                target: main.clone(),
            },
        ] {
            let name = format!("{request:?}");
            assert!(act(p, request).is_err(), "{name}");
            assert_eq!(text("mine.txt"), "local edit\n", "{name}");
            assert_eq!(head(p), topic, "{name}");
            assert!(snapshot(p).unwrap().operation.is_none(), "{name}");
        }
        // A rebase puts the change back on top of the new history; if it does not fit, Git keeps it in
        // the stash. It is never lost.
        let result = act(p, Request::Rebase { target: main });
        let stashed = !snapshot(p).unwrap().stashes.is_empty();
        assert!(
            text("mine.txt").contains("local edit") || stashed,
            "{result:?}"
        );
    }

    /// Stash apply and pop with a local change in another file work as in Git; one that would
    /// overwrite a changed file is refused, the change and the stash stay.
    #[test]
    fn stash_apply_and_pop_run_with_unrelated_local_changes() {
        let d = repository();
        let p = d.path();
        let write = |file: &str, text: &str| std::fs::write(p.join(file), text).unwrap();
        let text = |file: &str| std::fs::read_to_string(p.join(file)).unwrap();
        write("a.txt", "a\n");
        write("b.txt", "b\n");
        commit(p, "one", "1\n");
        write("a.txt", "a stashed\n");
        git(p, &["stash", "push", "-q"]).unwrap();
        write("b.txt", "b local\n");
        let id = snapshot(p).unwrap().stashes[0].id.clone();
        act(p, Request::StashApply { id: id.clone() }).unwrap_or_else(|e| panic!("apply: {e}"));
        assert_eq!(
            (text("a.txt"), text("b.txt")),
            ("a stashed\n".into(), "b local\n".into())
        );
        git(p, &["checkout", "-q", "--", "a.txt"]).unwrap();
        act(p, Request::StashPop { id }).unwrap_or_else(|e| panic!("pop: {e}"));
        assert_eq!(
            (text("a.txt"), text("b.txt")),
            ("a stashed\n".into(), "b local\n".into())
        );
        assert!(snapshot(p).unwrap().stashes.is_empty());
        // A stash that would overwrite a local edit: refused, nothing lost.
        git(p, &["stash", "push", "-q", "--", "a.txt"]).unwrap();
        write("a.txt", "a local\n");
        let id = snapshot(p).unwrap().stashes[0].id.clone();
        assert!(act(p, Request::StashPop { id }).is_err());
        assert_eq!(text("a.txt"), "a local\n");
        assert_eq!(snapshot(p).unwrap().stashes.len(), 1);
    }

    #[test]
    fn stash_save_apply_and_pop_run_in_a_repository_with_submodules() {
        let d = repository();
        let p = d.path();
        let _module = add_submodule(p);
        commit(p, "base", "base\n");
        git(p, &["config", "submodule.recurse", "true"]).unwrap();
        let module_head = head(&p.join("sub"));
        std::fs::write(p.join("file.txt"), "stashed\n").unwrap();
        std::fs::write(p.join("new.txt"), "untracked\n").unwrap();
        std::fs::write(p.join("sub/untracked.txt"), "keep\n").unwrap();
        act(
            p,
            Request::StashSave {
                message: "with a module".into(),
                keep_index: false,
                tracked_only: false,
                files: vec![],
            },
        )
        .unwrap_or_else(|e| panic!("save: {e}"));
        assert_ne!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "stashed\n"
        );
        let id = snapshot(p).unwrap().stashes[0].id.clone();
        act(p, Request::StashApply { id: id.clone() }).unwrap_or_else(|e| panic!("apply: {e}"));
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "stashed\n"
        );
        git(p, &["checkout", "-q", "--", "file.txt"]).unwrap();
        std::fs::remove_file(p.join("new.txt")).unwrap();
        act(p, Request::StashPop { id }).unwrap_or_else(|e| panic!("pop: {e}"));
        assert!(snapshot(p).unwrap().stashes.is_empty());
        assert_eq!(head(&p.join("sub")), module_head);
        assert_eq!(
            std::fs::read_to_string(p.join("sub/untracked.txt")).unwrap(),
            "keep\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("new.txt")).unwrap(),
            "untracked\n"
        );
    }

    fn add_submodule(p: &Path) -> tempfile::TempDir {
        let module = repository();
        commit(module.path(), "module one", "module one\n");
        git(
            p,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &module.path().to_string_lossy().replace('\\', "/"),
                "sub",
            ],
        )
        .unwrap();
        for (key, value) in [
            ("core.autocrlf", "false"),
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
        ] {
            git(&p.join("sub"), &["config", key, value]).unwrap();
        }
        git(p, &["commit", "-qm", "add submodule"]).unwrap();
        module
    }

    #[test]
    fn rebase_with_submodules_preserves_their_checkout_and_local_files() {
        let d = repository();
        let p = d.path();
        let _module = add_submodule(p);
        let sub = p.join("sub");
        let module_head = head(&sub);
        git(p, &["switch", "-qc", "topic"]).unwrap();
        let topic = commit(p, "topic change", "topic\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        // The target changes the recorded gitlink. Rebase must replay the parent history,
        // but leave the module's checkout at its old commit even with recursion enabled.
        commit(&sub, "module two", "module two\n");
        let target_module = head(&sub);
        git(p, &["add", "sub"]).unwrap();
        git(p, &["commit", "-qm", "update module"]).unwrap();
        let target = head(p);
        git(&sub, &["checkout", "-q", &module_head]).unwrap();
        git(p, &["switch", "-q", "topic"]).unwrap();
        git(p, &["config", "submodule.recurse", "true"]).unwrap();
        std::fs::write(sub.join("file.txt"), "local module edit\n").unwrap();
        std::fs::write(sub.join("untracked.txt"), "keep\n").unwrap();

        act(
            p,
            Request::Rebase {
                target: target.clone(),
            },
        )
        .unwrap();
        assert_ne!(head(p), topic);
        assert_eq!(git(p, &["rev-parse", "HEAD^"]).unwrap().trim(), target);
        assert_eq!(
            git(p, &["branch", "--show-current"]).unwrap().trim(),
            "topic"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "topic\n"
        );
        assert_eq!(
            git(p, &["rev-parse", "HEAD:sub"]).unwrap().trim(),
            target_module
        );
        assert_eq!(head(&sub), module_head);
        assert_eq!(
            std::fs::read_to_string(sub.join("file.txt")).unwrap(),
            "local module edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(sub.join("untracked.txt")).unwrap(),
            "keep\n"
        );
        assert!(snapshot(p).unwrap().operation.is_none());
    }

    #[test]
    fn rebase_with_submodules_preserves_an_unstaged_module_checkout() {
        let d = repository();
        let p = d.path();
        let _module = add_submodule(p);
        git(p, &["switch", "-qc", "topic"]).unwrap();
        commit(p, "topic change", "topic\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        std::fs::write(p.join("target.txt"), "target\n").unwrap();
        git(p, &["add", "target.txt"]).unwrap();
        git(p, &["commit", "-qm", "target change"]).unwrap();
        let target = head(p);
        git(p, &["switch", "-q", "topic"]).unwrap();
        let moved = commit(&p.join("sub"), "local module commit", "local module\n");
        git(p, &["config", "submodule.recurse", "true"]).unwrap();
        act(p, Request::Rebase { target }).unwrap();
        assert_eq!(head(&p.join("sub")), moved);
        assert_eq!(
            std::fs::read_to_string(p.join("sub/file.txt")).unwrap(),
            "local module\n"
        );
        assert!(p.join("target.txt").exists());
    }

    #[test]
    fn rebase_with_submodules_rejects_staged_gitlinks_and_keeps_parent_edits() {
        let d = repository();
        let p = d.path();
        let _module = add_submodule(p);
        std::fs::write(p.join("notes.txt"), "notes\n").unwrap();
        git(p, &["add", "--", "notes.txt"]).unwrap();
        git(p, &["commit", "-qm", "notes"]).unwrap();
        let original = head(p);
        let target = commit(p, "target change", "target\n");
        git(p, &["reset", "-q", "--hard", &original]).unwrap();
        let moved = commit(&p.join("sub"), "local module commit", "local module\n");
        git(p, &["add", "sub"]).unwrap();
        assert!(act(
            p,
            Request::Rebase {
                target: target.clone()
            }
        )
        .is_err());
        assert_eq!(head(p), original);
        assert_eq!(head(&p.join("sub")), moved);
        assert!(snapshot(p).unwrap().operation.is_none());
        // A staged removal is still a gitlink change even with no index entry.
        git(p, &["update-index", "--force-remove", "sub"]).unwrap();
        assert!(act(
            p,
            Request::Rebase {
                target: target.clone()
            }
        )
        .is_err());
        assert_eq!(head(p), original);
        assert_eq!(head(&p.join("sub")), moved);
        assert!(snapshot(p).unwrap().operation.is_none());
        git(p, &["reset", "-q", "HEAD", "--", "sub"]).unwrap();
        std::fs::write(p.join("notes.txt"), "local parent edit\n").unwrap();
        act(
            p,
            Request::Rebase {
                target: target.clone(),
            },
        )
        .unwrap();
        assert_eq!(head(p), target);
        assert_eq!(head(&p.join("sub")), moved);
        assert_eq!(
            std::fs::read_to_string(p.join("notes.txt")).unwrap(),
            "local parent edit\n"
        );
    }

    #[test]
    fn embedded_worktree_folder_is_visible_and_can_be_unstaged_and_excluded_before_rebase() {
        let d = repository();
        let p = d.path();
        commit(p, "base", "base\n");
        git(p, &["switch", "-qc", "topic"]).unwrap();
        let original = commit(p, "topic", "topic\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        std::fs::write(p.join("upstream.txt"), "upstream\n").unwrap();
        git(p, &["add", "upstream.txt"]).unwrap();
        git(p, &["commit", "-qm", "upstream"]).unwrap();
        let target = head(p);
        git(p, &["switch", "-q", "topic"]).unwrap();
        let nested = p.join(".worktrees");
        std::fs::create_dir(&nested).unwrap();
        git(&nested, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [
            ("core.autocrlf", "false"),
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
        ] {
            git(&nested, &["config", key, value]).unwrap();
        }
        let module_head = commit(&nested, "nested", "keep nested content\n");
        std::fs::write(nested.join("untracked.txt"), "keep untracked\n").unwrap();
        git(p, &["add", "--", ".worktrees"]).unwrap();
        let before = graph_view(p, false, false, 200, None).unwrap();
        assert!(before.graph.changes.contains("A  .worktrees"));
        let error = act(
            p,
            Request::Rebase {
                target: target.clone(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(".worktrees"), "{error}");
        assert!(error.contains("Working Changes"), "{error}");
        assert_eq!(head(p), original);
        assert!(snapshot(p).unwrap().operation.is_none());
        let workspace = super::super::gitworktree::workspace(p).unwrap();
        assert!(workspace
            .changes
            .iter()
            .any(|entry| entry.path == ".worktrees" && entry.index == 'A'));
        super::super::gitworktree::index_action(
            p,
            super::super::gitworktree::IndexAction::Unstage,
            Some(".worktrees"),
            "",
            &workspace.token,
        )
        .unwrap();
        let workspace = super::super::gitworktree::workspace(p).unwrap();
        let untracked = workspace
            .changes
            .iter()
            .find(|entry| entry.path.trim_end_matches('/') == ".worktrees" && entry.index == '?')
            .unwrap();
        super::super::gitworktree::index_action(
            p,
            super::super::gitworktree::IndexAction::Exclude,
            Some(&untracked.path),
            "",
            &workspace.token,
        )
        .unwrap();
        act(
            p,
            Request::Rebase {
                target: target.clone(),
            },
        )
        .unwrap();
        assert_eq!(git(p, &["rev-parse", "HEAD^"]).unwrap().trim(), target);
        assert_eq!(
            git(p, &["branch", "--show-current"]).unwrap().trim(),
            "topic"
        );
        assert_eq!(head(&nested), module_head);
        assert_eq!(
            std::fs::read_to_string(nested.join("file.txt")).unwrap(),
            "keep nested content\n"
        );
        assert_eq!(
            std::fs::read_to_string(nested.join("untracked.txt")).unwrap(),
            "keep untracked\n"
        );
        assert!(snapshot(p).unwrap().operation.is_none());
    }

    #[test]
    fn rebase_with_submodules_can_be_resolved_skipped_and_aborted() {
        let d = repository();
        let p = d.path();
        let _module = add_submodule(p);
        let sub = p.join("sub");
        let module_head = head(&sub);
        let (main, other) = conflict(p);
        git(p, &["config", "submodule.recurse", "true"]).unwrap();
        std::fs::write(sub.join("file.txt"), "module edit\n").unwrap();
        std::fs::write(sub.join("untracked.txt"), "keep\n").unwrap();
        for command in ["continue", "skip", "abort"] {
            git(
                p,
                &[
                    "-c",
                    "submodule.recurse=false",
                    "reset",
                    "-q",
                    "--hard",
                    &main,
                ],
            )
            .unwrap();
            assert!(act(
                p,
                Request::Rebase {
                    target: other.clone()
                }
            )
            .is_err());
            assert_eq!(snapshot(p).unwrap().operation.as_deref(), Some("rebase"));
            match command {
                "continue" => {
                    act(
                        p,
                        Request::ResolveContent {
                            file: "file.txt".into(),
                            content: "resolved\n".into(),
                        },
                    )
                    .unwrap();
                    act(p, Request::Continue).unwrap();
                    assert_eq!(
                        std::fs::read_to_string(p.join("file.txt")).unwrap(),
                        "resolved\n"
                    );
                }
                "skip" => {
                    act(p, Request::Skip).unwrap();
                    assert_eq!(head(p), other);
                }
                _ => {
                    act(p, Request::Abort).unwrap();
                    assert_eq!(head(p), main);
                }
            }
            assert!(snapshot(p).unwrap().operation.is_none());
            assert_eq!(head(&sub), module_head);
            assert_eq!(
                std::fs::read_to_string(sub.join("file.txt")).unwrap(),
                "module edit\n"
            );
            assert_eq!(
                std::fs::read_to_string(sub.join("untracked.txt")).unwrap(),
                "keep\n"
            );
        }
    }

    /// Checks that the module's checkout and local files were left alone.
    fn assert_module_kept(sub: &Path, module_head: &str) {
        assert_eq!(head(sub), module_head);
        assert_eq!(
            std::fs::read_to_string(sub.join("file.txt")).unwrap(),
            "local module edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(sub.join("untracked.txt")).unwrap(),
            "keep\n"
        );
    }

    #[test]
    fn hard_reset_checks_out_the_recorded_submodule_commits_and_keeps_their_edits_for_recovery() {
        let d = repository();
        let p = d.path();
        let _module = add_submodule(p);
        let sub = p.join("sub");
        let module_head = head(&sub);
        // Another branch records a newer module commit and a parent change.
        git(p, &["switch", "-qc", "other"]).unwrap();
        commit(&sub, "module two", "module two\n");
        let target_module = head(&sub);
        git(p, &["add", "sub"]).unwrap();
        git(p, &["commit", "-qm", "update module"]).unwrap();
        let target = commit(p, "other change", "other\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        git(&sub, &["checkout", "-q", &module_head]).unwrap();
        let original = head(p);
        git(p, &["config", "submodule.recurse", "true"]).unwrap();
        std::fs::write(sub.join("file.txt"), "local module edit\n").unwrap();
        std::fs::write(sub.join("untracked.txt"), "keep\n").unwrap();

        for mode in [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard] {
            git(
                p,
                &[
                    "-c",
                    "submodule.recurse=false",
                    "reset",
                    "-q",
                    "--hard",
                    &original,
                ],
            )
            .unwrap();
            let hard = matches!(mode, ResetMode::Hard);
            act(
                p,
                Request::Reset {
                    target: target.clone(),
                    mode,
                },
            )
            .unwrap();
            assert_eq!(head(p), target);
            assert_eq!(
                git(p, &["branch", "--show-current"]).unwrap().trim(),
                "main"
            );
            assert_eq!(
                git(p, &["rev-parse", "HEAD:sub"]).unwrap().trim(),
                target_module
            );
            if !hard {
                // Soft and mixed resets leave every working folder alone.
                assert_module_kept(&sub, &module_head);
                continue;
            }
            // A hard reset makes everything match the target, the submodule included.
            assert_eq!(
                std::fs::read_to_string(p.join("file.txt")).unwrap(),
                "other\n"
            );
            assert_eq!(head(&sub), target_module);
            assert_eq!(
                std::fs::read_to_string(sub.join("file.txt")).unwrap(),
                "module two\n"
            );
            assert!(
                git(&sub, &["status", "--porcelain", "--untracked-files=no"])
                    .unwrap()
                    .is_empty()
            );
            assert!(status(p).unwrap().is_empty());
            // Untracked files are kept; the old checkout and the discarded edit are
            // recovery references of the submodule.
            assert_eq!(
                std::fs::read_to_string(sub.join("untracked.txt")).unwrap(),
                "keep\n"
            );
            let recovery = git(
                &sub,
                &[
                    "for-each-ref",
                    "--format=%(objectname)",
                    "refs/gall/recovery/",
                ],
            )
            .unwrap();
            assert!(recovery.contains(&module_head), "{recovery}");
            let edit = recovery
                .lines()
                .find(|id| *id != module_head && *id != target_module)
                .expect("the discarded edit is kept");
            assert_eq!(
                git(&sub, &["show", &format!("{edit}:file.txt")]).unwrap(),
                "local module edit\n"
            );
        }
    }

    #[test]
    fn hard_reset_checks_out_nested_submodules_too() {
        let configure = |repo: &Path| {
            for (key, value) in [
                ("core.autocrlf", "false"),
                ("user.name", "Test"),
                ("user.email", "test@example.com"),
            ] {
                git(repo, &["config", key, value]).unwrap();
            }
        };
        let add = |repo: &Path, source: &Path, name: &str| {
            git(
                repo,
                &[
                    "-c",
                    "protocol.file.allow=always",
                    "submodule",
                    "add",
                    "-q",
                    &source.to_string_lossy().replace('\\', "/"),
                    name,
                ],
            )
            .unwrap();
            configure(&repo.join(name));
            git(repo, &["commit", "-qm", "add submodule"]).unwrap();
        };
        let inner = repository();
        commit(inner.path(), "inner one", "inner one\n");
        let middle = repository();
        commit(middle.path(), "middle one", "middle one\n");
        add(middle.path(), inner.path(), "nested");
        let d = repository();
        let p = d.path();
        add(p, middle.path(), "sub");
        let sub = p.join("sub");
        git(
            &sub,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "-q",
                "--init",
            ],
        )
        .unwrap();
        let nested = sub.join("nested");
        configure(&nested);
        let original = head(p);
        let sub_old = head(&sub);
        let nested_old = head(&nested);
        // The target records a newer nested commit through a newer middle commit.
        let nested_new = commit(&nested, "inner two", "inner two\n");
        // The nested checkout is detached; `submodule update` fetches a commit no reference
        // reaches, as one that only came from its remote would be.
        git(&nested, &["branch", "inner-two"]).unwrap();
        git(&sub, &["add", "nested"]).unwrap();
        git(&sub, &["commit", "-qm", "update nested"]).unwrap();
        git(p, &["add", "sub"]).unwrap();
        git(p, &["commit", "-qm", "update sub"]).unwrap();
        let target = head(p);
        git(p, &["reset", "-q", "--hard", &original]).unwrap();
        git(&sub, &["checkout", "-q", &sub_old]).unwrap();
        git(&nested, &["checkout", "-q", &nested_old]).unwrap();
        std::fs::write(nested.join("file.txt"), "nested edit\n").unwrap();

        act(
            p,
            Request::Reset {
                target,
                mode: ResetMode::Hard,
            },
        )
        .unwrap();
        assert_eq!(head(&nested), nested_new);
        assert_eq!(
            std::fs::read_to_string(nested.join("file.txt")).unwrap(),
            "inner two\n"
        );
        let recovery = git(
            &nested,
            &[
                "for-each-ref",
                "--format=%(objectname)",
                "refs/gall/recovery/",
            ],
        )
        .unwrap();
        assert!(recovery.contains(&nested_old), "{recovery}");
        assert!(recovery
            .lines()
            .any(|id| git(&nested, &["show", &format!("{id}:file.txt")])
                .is_ok_and(|text| text == "nested edit\n")));
    }

    #[test]
    fn hard_reset_refuses_to_overwrite_an_untracked_submodule_file() {
        let d = repository();
        let p = d.path();
        let _module = add_submodule(p);
        let sub = p.join("sub");
        let module_head = head(&sub);
        let original = head(p);
        // The target records a module commit that adds `new.txt`.
        std::fs::write(sub.join("new.txt"), "tracked\n").unwrap();
        commit(&sub, "module two", "module two\n");
        git(p, &["add", "sub"]).unwrap();
        git(p, &["commit", "-qm", "update module"]).unwrap();
        let target = head(p);
        git(p, &["reset", "-q", "--hard", &original]).unwrap();
        git(&sub, &["checkout", "-q", &module_head]).unwrap();
        std::fs::write(sub.join("new.txt"), "my untracked file\n").unwrap();

        let error = act(
            p,
            Request::Reset {
                target: target.clone(),
                mode: ResetMode::Hard,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Tools → Submodules"), "{error}");
        assert_eq!(head(p), target);
        assert_eq!(head(&sub), module_head);
        assert_eq!(
            std::fs::read_to_string(sub.join("new.txt")).unwrap(),
            "my untracked file\n"
        );
    }

    #[test]
    fn merge_cherry_pick_revert_and_abort_with_submodules_keep_the_module_checkout() {
        let d = repository();
        let p = d.path();
        let _module = add_submodule(p);
        let sub = p.join("sub");
        let module_head = head(&sub);
        git(p, &["switch", "-qc", "topic"]).unwrap();
        std::fs::write(p.join("topic.txt"), "topic\n").unwrap();
        git(p, &["add", "topic.txt"]).unwrap();
        git(p, &["commit", "-qm", "topic"]).unwrap();
        let topic = head(p);
        git(p, &["switch", "-q", "main"]).unwrap();
        commit(&sub, "module two", "module two\n");
        let main_module = head(&sub);
        git(p, &["add", "sub"]).unwrap();
        git(p, &["commit", "-qm", "update module"]).unwrap();
        let updated = head(p);
        git(&sub, &["checkout", "-q", &module_head]).unwrap();
        git(p, &["config", "submodule.recurse", "true"]).unwrap();
        std::fs::write(sub.join("file.txt"), "local module edit\n").unwrap();
        std::fs::write(sub.join("untracked.txt"), "keep\n").unwrap();

        act(
            p,
            Request::Merge {
                target: topic.clone(),
            },
        )
        .unwrap();
        assert!(p.join("topic.txt").exists());
        assert_eq!(
            git(p, &["rev-parse", "HEAD:sub"]).unwrap().trim(),
            main_module
        );
        assert_module_kept(&sub, &module_head);

        git(
            p,
            &[
                "-c",
                "submodule.recurse=false",
                "reset",
                "-q",
                "--hard",
                &updated,
            ],
        )
        .unwrap();
        act(
            p,
            Request::CherryPick {
                target: topic.clone(),
            },
        )
        .unwrap();
        assert_eq!(git(p, &["rev-parse", "HEAD^"]).unwrap().trim(), updated);
        assert!(p.join("topic.txt").exists());
        assert_module_kept(&sub, &module_head);

        act(p, Request::Revert { target: updated }).unwrap();
        assert_eq!(
            git(p, &["rev-parse", "HEAD:sub"]).unwrap().trim(),
            module_head
        );
        assert!(p.join("topic.txt").exists());
        assert_module_kept(&sub, &module_head);

        // A conflicting merge can be aborted without touching the module either.
        let before = commit(p, "main side", "main side\n");
        let switch = |args: &[&str]| {
            let mut all = vec!["-c", "submodule.recurse=false", "switch", "-q"];
            all.extend_from_slice(args);
            git(p, &all).unwrap();
        };
        switch(&["-c", "side", "HEAD^"]);
        let side = commit(p, "side", "side\n");
        switch(&["main"]);
        assert!(act(p, Request::Merge { target: side }).is_err());
        assert_eq!(snapshot(p).unwrap().operation.as_deref(), Some("merge"));
        act(p, Request::Abort).unwrap();
        assert_eq!(head(p), before);
        assert!(snapshot(p).unwrap().operation.is_none());
        assert_module_kept(&sub, &module_head);
    }

    #[test]
    fn merge_applies_smudge_and_refuses_overlapping_filtered_edits() {
        let d = repository();
        let p = d.path();
        git(
            p,
            &[
                "config",
                "filter.roundtrip.clean",
                "sed 's/working/repository/g'",
            ],
        )
        .unwrap();
        git(
            p,
            &[
                "config",
                "filter.roundtrip.smudge",
                "sed 's/repository/working/g'",
            ],
        )
        .unwrap();
        std::fs::write(p.join(".gitattributes"), "file.txt filter=roundtrip\n").unwrap();
        let base = commit(p, "base", "working base\n");
        git(p, &["switch", "-qc", "topic"]).unwrap();
        let topic = commit(p, "topic", "working topic\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        std::fs::write(p.join("file.txt"), "working local\n").unwrap();
        assert!(act(
            p,
            Request::Merge {
                target: topic.clone()
            }
        )
        .is_err());
        assert_eq!(head(p), base);
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "working local\n"
        );
        assert!(snapshot(p).unwrap().operation.is_none());

        git(p, &["restore", "--", "file.txt"]).unwrap();
        std::fs::write(p.join("unrelated"), "keep\n").unwrap();
        act(
            p,
            Request::Merge {
                target: topic.clone(),
            },
        )
        .unwrap();
        assert_eq!(head(p), topic);
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "working topic\n"
        );
        assert_eq!(
            git(p, &["show", "HEAD:file.txt"]).unwrap(),
            "repository topic\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("unrelated")).unwrap(),
            "keep\n"
        );
    }

    #[test]
    fn filtered_soft_and_mixed_resets_keep_working_bytes_and_are_undoable() {
        for mode in [ResetMode::Soft, ResetMode::Mixed] {
            let d = repository();
            let p = d.path();
            git(
                p,
                &[
                    "config",
                    "filter.roundtrip.clean",
                    "sed 's/working/repository/g'",
                ],
            )
            .unwrap();
            git(
                p,
                &[
                    "config",
                    "filter.roundtrip.smudge",
                    "sed 's/repository/working/g'",
                ],
            )
            .unwrap();
            std::fs::write(p.join(".gitattributes"), "file.txt filter=roundtrip\n").unwrap();
            let first = commit(p, "first", "working first\n");
            let second = commit(p, "second", "working second\n");
            std::fs::write(p.join("file.txt"), "working staged\n").unwrap();
            git(p, &["add", "--", "file.txt"]).unwrap();
            std::fs::write(p.join("file.txt"), "working unstaged\n").unwrap();
            let expected_index = if matches!(mode, ResetMode::Soft) {
                "repository staged\n"
            } else {
                "repository first\n"
            };
            act(
                p,
                Request::Reset {
                    target: first.clone(),
                    mode,
                },
            )
            .unwrap();
            assert_eq!(head(p), first);
            assert_eq!(git(p, &["show", ":file.txt"]).unwrap(), expected_index);
            assert_eq!(
                std::fs::read_to_string(p.join("file.txt")).unwrap(),
                "working unstaged\n"
            );

            let journal = super::super::gitjournal::view(p).unwrap();
            super::super::gitjournal::move_to(p, journal.applied - 1, journal.applied).unwrap();
            assert_eq!(head(p), second);
            assert_eq!(
                git(p, &["show", ":file.txt"]).unwrap(),
                "repository staged\n"
            );
            assert_eq!(
                std::fs::read_to_string(p.join("file.txt")).unwrap(),
                "working unstaged\n"
            );
        }
    }

    #[test]
    fn filtered_hard_reset_checks_that_recovery_can_reproduce_raw_bytes() {
        let d = repository();
        let p = d.path();
        git(
            p,
            &[
                "config",
                "filter.roundtrip.clean",
                "sed 's/working/repository/g; s/ local-only$//'",
            ],
        )
        .unwrap();
        git(
            p,
            &[
                "config",
                "filter.roundtrip.smudge",
                "sed 's/repository/working/g'",
            ],
        )
        .unwrap();
        std::fs::write(p.join(".gitattributes"), "file.txt filter=roundtrip\n").unwrap();
        let first = commit(p, "first", "working first\n");
        let second = commit(p, "second", "working second\n");
        let reset = || {
            act(
                p,
                Request::Reset {
                    target: first.clone(),
                    mode: ResetMode::Hard,
                },
            )
        };

        std::fs::write(p.join("file.txt"), "working second local-only\n").unwrap();
        assert!(reset().unwrap_err().to_string().contains("file.txt"));
        assert_eq!(head(p), second);
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "working second local-only\n"
        );

        std::fs::write(p.join("file.txt"), "working recoverable\n").unwrap();
        reset().unwrap();
        assert_eq!(head(p), first);
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "working first\n"
        );
        let journal = super::super::gitjournal::view(p).unwrap();
        super::super::gitjournal::move_to(p, journal.applied - 1, journal.applied).unwrap();
        assert_eq!(head(p), second);
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "working recoverable\n"
        );
    }

    #[test]
    fn stash_filter_preflight_skips_an_unrelated_required_filter() {
        let d = repository();
        let p = d.path();
        std::fs::write(p.join("other.txt"), "other\n").unwrap();
        commit(p, "base", "base\n");
        git(p, &["config", "filter.failing.clean", "false"]).unwrap();
        git(p, &["config", "filter.failing.required", "true"]).unwrap();
        std::fs::write(p.join(".git/info/attributes"), "other.txt filter=failing\n").unwrap();
        std::fs::write(p.join("other.txt"), "unrelated local edit\n").unwrap();
        std::fs::write(p.join("file.txt"), "selected local edit\n").unwrap();

        super::super::gitadvanced::verify_filtered_stash_paths(p, &[":(literal)file.txt"], false)
            .unwrap();
        assert!(super::super::gitadvanced::verify_filtered_stash_paths(p, &[], false).is_err());
        assert_eq!(
            std::fs::read_to_string(p.join("other.txt")).unwrap(),
            "unrelated local edit\n"
        );
    }

    #[test]
    fn filtered_stash_save_checks_only_selected_tracked_and_untracked_paths() {
        let d = repository();
        let p = d.path();
        git(
            p,
            &[
                "config",
                "filter.roundtrip.clean",
                "sed 's/working/repository/g; s/ local-only$//'",
            ],
        )
        .unwrap();
        git(
            p,
            &[
                "config",
                "filter.roundtrip.smudge",
                "sed 's/repository/working/g'",
            ],
        )
        .unwrap();
        std::fs::write(p.join(".gitattributes"), "*.txt filter=roundtrip\n").unwrap();
        std::fs::write(p.join("other.txt"), "working other\n").unwrap();
        commit(p, "base", "working base\n");
        std::fs::write(p.join("file.txt"), "working edited\n").unwrap();
        std::fs::write(p.join("other.txt"), "working other local-only\n").unwrap();
        std::fs::write(p.join("new.txt"), "working new local-only\n").unwrap();
        let save = |files: &[&str], tracked_only| {
            act(
                p,
                Request::StashSave {
                    message: "filter test".into(),
                    keep_index: false,
                    tracked_only,
                    files: files.iter().map(|file| (*file).to_owned()).collect(),
                },
            )
        };

        assert!(save(&[], true)
            .unwrap_err()
            .to_string()
            .contains("other.txt"));
        assert!(save(&["new.txt"], false)
            .unwrap_err()
            .to_string()
            .contains("new.txt"));
        assert!(snapshot(p).unwrap().stashes.is_empty());
        assert_eq!(
            std::fs::read_to_string(p.join("new.txt")).unwrap(),
            "working new local-only\n"
        );
        save(&["file.txt"], true).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "working base\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("other.txt")).unwrap(),
            "working other local-only\n"
        );
        let first = snapshot(p).unwrap().stashes[0].id.clone();
        act(p, Request::StashPop { id: first }).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "working edited\n"
        );

        std::fs::write(p.join("new.txt"), "working new\n").unwrap();
        save(&["new.txt"], false).unwrap();
        assert!(!p.join("new.txt").exists());
        let second = snapshot(p).unwrap().stashes[0].id.clone();
        act(p, Request::StashPop { id: second }).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("new.txt")).unwrap(),
            "working new\n"
        );
    }

    #[test]
    fn filtered_stash_apply_pop_and_branch_keep_worktree_representation() {
        for action in ["apply", "pop", "branch"] {
            let d = repository();
            let p = d.path();
            git(
                p,
                &[
                    "config",
                    "filter.roundtrip.clean",
                    "sed 's/working/repository/g'",
                ],
            )
            .unwrap();
            git(
                p,
                &[
                    "config",
                    "filter.roundtrip.smudge",
                    "sed 's/repository/working/g'",
                ],
            )
            .unwrap();
            std::fs::write(p.join(".gitattributes"), "file.txt filter=roundtrip\n").unwrap();
            let base = commit(p, "base", "working base\n");
            std::fs::write(p.join("file.txt"), "working saved\n").unwrap();
            git(p, &["add", "--", "file.txt"]).unwrap();
            git(p, &["stash", "push", "-m", "filtered stash"]).unwrap();
            let id = snapshot(p).unwrap().stashes[0].id.clone();
            let request = || match action {
                "apply" => Request::StashApply { id: id.clone() },
                "pop" => Request::StashPop { id: id.clone() },
                _ => Request::StashBranch {
                    id: id.clone(),
                    name: "restored".into(),
                },
            };
            if action != "branch" {
                std::fs::write(p.join("file.txt"), "working local\n").unwrap();
                assert!(act(p, request()).is_err());
                assert_eq!(
                    std::fs::read_to_string(p.join("file.txt")).unwrap(),
                    "working local\n"
                );
                assert_eq!(snapshot(p).unwrap().stashes[0].id, id);
                git(p, &["restore", "--", "file.txt"]).unwrap();
            }

            std::fs::write(p.join("unrelated"), "keep\n").unwrap();
            act(p, request()).unwrap();
            assert_eq!(head(p), base);
            assert_eq!(
                git(p, &["show", ":file.txt"]).unwrap(),
                "repository saved\n"
            );
            assert_eq!(
                std::fs::read_to_string(p.join("file.txt")).unwrap(),
                "working saved\n"
            );
            assert_eq!(
                std::fs::read_to_string(p.join("unrelated")).unwrap(),
                "keep\n"
            );
            assert_eq!(
                snapshot(p).unwrap().stashes.len(),
                usize::from(action == "apply")
            );
        }
    }

    #[test]
    fn stash_works_with_gitmodules() {
        let d = repository();
        let p = d.path();
        commit(p, "first", "first\n");
        std::fs::write(p.join(".gitmodules"), "[submodule \"future\"]\n").unwrap();
        assert!(snapshot(p).is_ok());
        // A `.gitmodules` file does not stop a stash any more (Git and lazygit run it).
        std::fs::write(p.join("file.txt"), "edited\n").unwrap();
        act(
            p,
            Request::StashSave {
                message: "".into(),
                keep_index: false,
                tracked_only: true,
                files: Vec::new(),
            },
        )
        .unwrap_or_else(|e| panic!("stash with .gitmodules: {e}"));
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "first\n"
        );
    }

    #[test]
    fn conflict_preview_reports_three_stages_and_rejects_stale_resolution() {
        let d = repository();
        let p = d.path();
        let (_, other) = conflict(p);
        assert!(act(p, Request::Merge { target: other }).is_err());
        let view = super::conflict(p, "file.txt").unwrap();
        assert_eq!(view.file, "file.txt");
        assert_eq!(view.base, "first\n");
        assert_eq!(view.ours, "main\n");
        assert_eq!(view.theirs, "other\n");
        assert!(!view.binary);
        assert!(view.result.contains("<<<<<<<"));
        assert!(super::conflict(p, "../file.txt").is_err());
        std::fs::write(p.join("file.txt"), "manual resolution\n").unwrap();
        assert!(execute(
            p,
            Request::Resolve {
                file: "file.txt".into(),
                side: ResolveSide::Theirs
            },
            &view.token
        )
        .is_err());
        act(
            p,
            Request::Resolve {
                file: "file.txt".into(),
                side: ResolveSide::Mark,
            },
        )
        .unwrap();
        act(p, Request::Continue).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "manual resolution\n"
        );
    }

    #[test]
    fn mixed_reset_retains_the_index_version_without_changing_working_files() {
        let d = repository();
        let p = d.path();
        let first = commit(p, "first", "first\n");
        commit(p, "second", "second\n");
        std::fs::write(p.join("file.txt"), "staged version\n").unwrap();
        git(p, &["add", "file.txt"]).unwrap();
        std::fs::write(p.join("file.txt"), "working version\n").unwrap();
        act(
            p,
            Request::Reset {
                target: first,
                mode: ResetMode::Mixed,
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "working version\n"
        );
        let recovery = git(
            p,
            &[
                "log",
                "--all",
                "--format=%H",
                "--grep=Gall recovery before mixed reset",
            ],
        )
        .unwrap();
        let id = recovery.lines().next().unwrap();
        assert_eq!(
            git(p, &["show", &format!("{id}^2:file.txt")]).unwrap(),
            "staged version\n"
        );
        assert_eq!(
            git(p, &["show", &format!("{id}:file.txt")]).unwrap(),
            "working version\n"
        );
        assert!(snapshot(p).unwrap().stashes.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn choosing_either_side_resolves_a_modify_delete_conflict() {
        for (delete_on_current, choose_deletion) in [(false, false), (false, true), (true, true)] {
            let d = repository();
            let p = d.path();
            commit(p, "base", "base\n");
            git(p, &["switch", "-qc", "topic"]).unwrap();
            if delete_on_current {
                commit(p, "modified", "modified\n");
            } else {
                git(p, &["rm", "--", "file.txt"]).unwrap();
                git(p, &["commit", "-qm", "deleted"]).unwrap();
            }
            git(p, &["switch", "-q", "main"]).unwrap();
            if delete_on_current {
                git(p, &["rm", "--", "file.txt"]).unwrap();
                git(p, &["commit", "-qm", "deleted"]).unwrap();
            } else {
                commit(p, "modified", "modified\n");
            }
            assert!(git(p, &["merge", "topic"]).is_err());
            let side = if delete_on_current == choose_deletion {
                ResolveSide::Ours
            } else {
                ResolveSide::Theirs
            };
            std::fs::write(p.join("unrelated"), "keep\n").unwrap();
            act(
                p,
                Request::Resolve {
                    file: "file.txt".into(),
                    side,
                },
            )
            .unwrap();
            assert!(conflicts(p).unwrap().is_empty());
            assert_eq!(p.join("file.txt").exists(), !choose_deletion);
            if !choose_deletion {
                assert_eq!(
                    std::fs::read_to_string(p.join("file.txt")).unwrap(),
                    "modified\n"
                );
            }
            assert_eq!(
                std::fs::read_to_string(p.join("unrelated")).unwrap(),
                "keep\n"
            );
        }
    }

    #[test]
    fn choosing_a_gitlink_stages_that_commit_without_changing_the_checkout() {
        let d = repository();
        let p = d.path();
        let _module = add_submodule(p);
        let sub = p.join("sub");
        let module_base = head(&sub);
        git(p, &["switch", "-qc", "topic"]).unwrap();
        let theirs = commit(&sub, "topic module", "theirs\n");
        git(p, &["add", "--", "sub"]).unwrap();
        git(p, &["commit", "-qm", "topic pointer"]).unwrap();
        git(p, &["switch", "-q", "main"]).unwrap();
        git(&sub, &["switch", "--detach", &module_base]).unwrap();
        let ours = commit(&sub, "main module", "ours\n");
        git(p, &["add", "--", "sub"]).unwrap();
        git(p, &["commit", "-qm", "main pointer"]).unwrap();

        assert!(git(p, &["merge", "topic"]).is_err());
        std::fs::write(sub.join("file.txt"), "local module edit\n").unwrap();
        act(
            p,
            Request::Resolve {
                file: "sub".into(),
                side: ResolveSide::Theirs,
            },
        )
        .unwrap();

        assert!(conflicts(p).unwrap().is_empty());
        assert_eq!(git(p, &["rev-parse", ":sub"]).unwrap().trim(), theirs);
        assert_eq!(head(&sub), ours);
        assert_eq!(
            std::fs::read_to_string(sub.join("file.txt")).unwrap(),
            "local module edit\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn conflict_preview_refuses_symlink_working_file() {
        let d = repository();
        let p = d.path();
        let (_, other) = conflict(p);
        assert!(act(p, Request::Merge { target: other }).is_err());
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), "private outside content").unwrap();
        std::fs::remove_file(p.join("file.txt")).unwrap();
        std::os::unix::fs::symlink(outside.path(), p.join("file.txt")).unwrap();
        assert!(super::conflict(p, "file.txt").is_err());
        act(
            p,
            Request::Resolve {
                file: "file.txt".into(),
                side: ResolveSide::Theirs,
            },
        )
        .unwrap();
        assert!(conflicts(p).unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "other\n"
        );
        assert_eq!(
            std::fs::read_to_string(outside.path()).unwrap(),
            "private outside content"
        );
    }

    #[test]
    fn cherry_pick_keeps_empty_and_already_applied_commits() {
        let d = repository();
        let p = d.path();
        let base = commit(p, "base", "base\n");
        git(p, &["switch", "-qc", "topic"]).unwrap();
        git(p, &["commit", "--allow-empty", "-qm", "marker"]).unwrap();
        let marker = head(p);
        let edit = commit(p, "edit", "edited\n");
        git(p, &["switch", "-q", "main"]).unwrap();

        act(
            p,
            Request::CherryPick {
                target: marker.clone(),
            },
        )
        .unwrap();
        assert_eq!(git(p, &["rev-parse", "HEAD^"]).unwrap().trim(), base);
        act(
            p,
            Request::CherryPickMany {
                targets: vec![marker, edit.clone()],
            },
        )
        .unwrap();
        let edited = head(p);
        act(p, Request::CherryPick { target: edit }).unwrap();
        assert_eq!(git(p, &["rev-parse", "HEAD^"]).unwrap().trim(), edited);
        assert_eq!(
            git(p, &["rev-parse", "HEAD^{tree}"]).unwrap(),
            git(p, &["rev-parse", "HEAD^^{tree}"]).unwrap()
        );
        assert!(snapshot(p).unwrap().operation.is_none());
    }

    #[test]
    fn cherry_pick_and_revert_ignore_an_unrelated_path_in_the_source_tree() {
        let d = repository();
        let p = d.path();
        std::fs::write(p.join("untouched.tmp"), "tracked in historical trees\n").unwrap();
        commit(p, "base", "base\n");
        git(p, &["switch", "-qc", "topic"]).unwrap();
        let topic = commit(p, "topic", "changed\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        git(p, &["rm", "--", "untouched.tmp"]).unwrap();
        git(p, &["commit", "-qm", "remove unrelated file"]).unwrap();
        std::fs::write(p.join(".git/info/exclude"), "*.tmp\n").unwrap();
        std::fs::write(p.join("untouched.tmp"), "ignored locally\n").unwrap();

        act(
            p,
            Request::CherryPick {
                target: topic.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "changed\n"
        );
        act(p, Request::Revert { target: topic }).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("untouched.tmp")).unwrap(),
            "ignored locally\n"
        );
    }

    #[test]
    fn history_operations_preserve_ignored_files_that_git_would_overwrite() {
        for action in ["cherryPick", "rebase", "revert"] {
            let d = repository();
            let p = d.path();
            commit(p, "first", "first\n");
            git(p, &["switch", "-qc", "other"]).unwrap();
            std::fs::write(p.join("collision.tmp"), "tracked elsewhere\n").unwrap();
            git(p, &["add", "--all"]).unwrap();
            git(p, &["commit", "-qm", "add ignored collision"]).unwrap();
            let mut target = head(p);
            if action == "revert" {
                git(p, &["rm", "--", "collision.tmp"]).unwrap();
                git(p, &["commit", "-qm", "remove collision"]).unwrap();
                target = head(p);
            } else {
                git(p, &["switch", "-q", "main"]).unwrap();
                commit(p, "main changes", "main\n");
            }
            std::fs::write(p.join(".git/info/exclude"), "*.tmp\n").unwrap();
            std::fs::write(p.join("collision.tmp"), "private ignored contents\n").unwrap();
            let old = head(p);
            let request = match action {
                "cherryPick" => Request::CherryPick { target },
                "rebase" => Request::Rebase { target },
                _ => Request::Revert { target },
            };
            assert!(
                act(p, request).is_err(),
                "{action} overwrote an ignored file"
            );
            assert_eq!(head(p), old);
            assert_eq!(
                std::fs::read_to_string(p.join("collision.tmp")).unwrap(),
                "private ignored contents\n"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn plain_rebase_restores_recoverable_filtered_edits() {
        let d = repository();
        let p = d.path();
        git(
            p,
            &[
                "config",
                "filter.roundtrip.clean",
                "sed 's/working/repository/g; s/ local-only$//'",
            ],
        )
        .unwrap();
        git(
            p,
            &[
                "config",
                "filter.roundtrip.smudge",
                "sed 's/repository/working/g'",
            ],
        )
        .unwrap();
        std::fs::write(p.join(".gitattributes"), "file.txt filter=roundtrip\n").unwrap();
        commit(p, "base", "working base\n");
        git(p, &["switch", "-qc", "topic"]).unwrap();
        std::fs::write(p.join("topic.txt"), "topic\n").unwrap();
        let before = commit(p, "topic", "working base\n");
        git(p, &["switch", "-q", "main"]).unwrap();
        std::fs::write(p.join("main.txt"), "main\n").unwrap();
        let target = commit(p, "main", "working base\n");
        git(p, &["switch", "-q", "topic"]).unwrap();
        let rebase = || {
            act(
                p,
                Request::Rebase {
                    target: target.clone(),
                },
            )
        };

        std::fs::write(p.join("file.txt"), "working base local-only\n").unwrap();
        assert!(rebase().unwrap_err().to_string().contains("file.txt"));
        assert_eq!(head(p), before);
        assert!(snapshot(p).unwrap().operation.is_none());

        std::fs::write(p.join("file.txt"), "working recoverable\n").unwrap();
        rebase().unwrap();
        assert_eq!(git(p, &["rev-parse", "HEAD^"]).unwrap().trim(), target);
        assert_eq!(
            std::fs::read_to_string(p.join("file.txt")).unwrap(),
            "working recoverable\n"
        );
        assert!(p.join("main.txt").is_file() && p.join("topic.txt").is_file());
    }

    #[test]
    fn rebase_protects_ignored_paths_present_only_in_intermediate_commits() {
        let d = repository();
        let p = d.path();
        let first = commit(p, "first", "first\n");
        // Windows file names cannot contain a newline.
        let name = if cfg!(windows) {
            "[cached] version.tmp"
        } else {
            "[cached]\nversion.tmp"
        };
        std::fs::write(p.join(name), "intermediate\n").unwrap();
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "temporary file"]).unwrap();
        git(p, &["rm", "--", name]).unwrap();
        git(p, &["commit", "-qm", "remove temporary file"]).unwrap();
        std::fs::write(p.join(".git/info/exclude"), "*.tmp\n").unwrap();
        std::fs::write(p.join(name), "ignored local copy\n").unwrap();
        assert!(act(p, Request::Rebase { target: first }).is_err());
        assert_eq!(
            std::fs::read_to_string(p.join(name)).unwrap(),
            "ignored local copy\n"
        );
    }
}
