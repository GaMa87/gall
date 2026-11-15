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
                return Err(fail("Symlink conflicts require an external editor"))
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
                return Err(fail("Symlink conflicts require an external editor"))
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
                "Conflict files larger than 1 MiB require an external editor",
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
            "Conflict files larger than 1 MiB require an external editor",
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
    Merge {
        target: String,
    },
    Rebase {
        target: String,
    },
    CherryPick {
        target: String,
    },
    Revert {
        target: String,
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
        Request::Merge { target } => (target, false, false, false),
        Request::Rebase { target } => (target, true, true, true),
        Request::CherryPick { target } => (target, true, true, false),
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
fn marker_exists(
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
fn marker_content(
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

/// The index as `ls-files --stage -z` lists it: `<mode> <object> <stage>\t<name>` records.
fn index_listing(path: &Path) -> Result<String> {
    if let Some(index) = seen(path, |seen| Some(seen.index.clone())) {
        return Ok(index);
    }
    git(path, &["ls-files", "--stage", "-z"])
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

/// For rebase and steps of an operation that is already running (continue, skip, abort,
/// resolving a conflict). These support submodules; rebase commands explicitly disable
/// recursion so that the submodule working folders stay untouched.
fn control_guard(path: &Path) -> Result<()> {
    worktree_guard_with(path, true)
}

fn worktree_guard(path: &Path) -> Result<()> {
    worktree_guard_with(path, false)
}

fn worktree_guard_with(path: &Path, submodules_ok: bool) -> Result<()> {
    if has_filters(path) {
        return Err(fail(
            "Worktree operations with content filters require the terminal",
        ));
    }
    if !submodules_ok
        && (path.join(".gitmodules").exists()
            || index_listing(path)?
                .split('\0')
                .any(|record| record.starts_with("160000 ")))
    {
        return Err(fail(
            "Worktree operations with submodules require the terminal",
        ));
    }
    if head(path).is_empty() {
        return Err(fail("Create the first commit before this operation"));
    }
    Ok(())
}

fn clean(path: &Path) -> Result<()> {
    if !status(path)?.is_empty() {
        return Err(fail("Commit or stash local changes before this operation"));
    }
    Ok(())
}

/// Give the user the paths that block rebase; the graph and Working Changes read
/// the same status, including staged gitlinks for embedded repositories.
fn rebase_clean(path: &Path) -> Result<()> {
    let changes = status(path)?;
    if changes.is_empty() {
        return Ok(());
    }
    let names: Vec<_> = super::gitgraph::status_entries(&changes)
        .into_iter()
        .take(5)
        .map(|entry| format!("{:?}", entry.name))
        .collect();
    Err(fail(format!(
        "Local changes block rebase: {}. Open Working Changes to review, commit, or unstage them. Unwanted untracked folders can be excluded locally without deleting them.",
        names.join(", ")
    )))
}

fn protect_untracked(path: &Path, targets: &[&str], replay: Option<&str>) -> Result<()> {
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

fn save_head(path: &Path) -> Result<()> {
    let id = head(path);
    if id.is_empty() {
        return Ok(());
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
    Ok(())
}

fn save_index_and_worktree(path: &Path, message: &str) -> Result<()> {
    // `stash create` stores index and tracked working content without cleaning
    // the working tree or changing the user's stash stack.
    let id = git(path, &["stash", "create", message])?;
    let id = id.trim();
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

fn control(path: &Path, command: &str) -> Result<String> {
    // Continue, skip and abort change the repository through other modules, which the
    // local `git` does not see: from here on every question is asked again.
    SEEN.with(|slot| *slot.borrow_mut() = None);
    let current = operation(path)?.ok_or_else(|| fail("There is no interrupted Git operation"))?;
    if command == "--continue" && !conflicts(path)?.is_empty() {
        return Err(fail("Resolve and stage all conflicts before continuing"));
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
        if current == "merge" {
            let id = git(path, &["rev-parse", "--verify", "ORIG_HEAD"])?;
            commit_id(path, id.trim())?;
            targets.push(id.trim().into());
        }
    } else if current != "merge" {
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
    if current == "merge" {
        return match command {
            "--continue" => git(path, &["-c", "core.editor=true", "commit", "--no-edit"]),
            "--abort" => git(path, &["merge", "--abort"]),
            _ => Err(fail("A merge cannot be skipped; continue or abort it")),
        };
    }
    git(
        path,
        &[
            "-c",
            "core.editor=true",
            "-c",
            "submodule.recurse=false",
            &current,
            command,
        ],
    )
}

/// Execute one explicitly selected local action. Commits are unsigned, with hooks disabled.
pub fn execute(path: &Path, request: Request, expected: &str) -> Result<String> {
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
        Request::StashSave { message } => {
            worktree_guard(&p)?;
            no_operation(&p)?;
            if status(&p)?.is_empty() {
                return Err(fail("There are no local changes to stash"));
            }
            git(
                &p,
                &[
                    "stash",
                    "push",
                    "--include-untracked",
                    "-m",
                    if message.trim().is_empty() {
                        "Gall workspace stash"
                    } else {
                        &message
                    },
                ],
            )
        }
        Request::StashApply { id } => {
            worktree_guard(&p)?;
            no_operation(&p)?;
            clean(&p)?;
            stash(&p, &id)?;
            git(&p, &["stash", "apply", "--index", &id])
        }
        Request::StashPop { id } => {
            worktree_guard(&p)?;
            no_operation(&p)?;
            clean(&p)?;
            stash(&p, &id)?;
            let output = git(&p, &["stash", "apply", "--index", &id])?;
            drop_stash(&p, &id).map_err(|e| {
                fail(format!(
                    "Stash applied, but its stack entry could not be removed: {e}"
                ))
            })?;
            Ok(output)
        }
        Request::Continue => {
            control_guard(&p)?;
            control(&p, "--continue")
        }
        Request::Skip => {
            control_guard(&p)?;
            control(&p, "--skip")
        }
        Request::Abort => {
            control_guard(&p)?;
            control(&p, "--abort")
        }
        Request::Resolve { file, side } => {
            control_guard(&p)?;
            if !conflicts(&p)?.contains(&file) {
                return Err(fail("This file is no longer an unresolved conflict"));
            }
            let literal = format!(":(literal){file}");
            match side {
                ResolveSide::Ours | ResolveSide::Theirs => {
                    let stage = if matches!(side, ResolveSide::Ours) {
                        "--ours"
                    } else {
                        "--theirs"
                    };
                    git(&p, &["restore", "--worktree", stage, "--", &literal])?;
                    git(&p, &["add", "--", &literal])
                }
                ResolveSide::Mark => git(&p, &["add", "--all", "--", &literal]),
            }
        }
        Request::ResolveContent { file, content } => {
            control_guard(&p)?;
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
        Request::Merge { target } => {
            worktree_guard(&p)?;
            no_operation(&p)?;
            clean(&p)?;
            commit_id(&p, &target)?;
            save_head(&p)?;
            git(
                &p,
                &[
                    "-c",
                    "merge.autoStash=false",
                    "merge",
                    "--no-edit",
                    "--no-overwrite-ignore",
                    &target,
                ],
            )
        }
        Request::Rebase { target } => {
            // Rebase rewrites the superproject's gitlinks without checking out submodules.
            // Reuse the token's status rather than recursively scanning their folders.
            control_guard(&p)?;
            no_operation(&p)?;
            rebase_clean(&p)?;
            commit_id(&p, &target)?;
            protect_untracked(&p, &[&target], Some(&target))?;
            save_head(&p)?;
            git(
                &p,
                &[
                    "-c",
                    "core.editor=true",
                    "-c",
                    "rebase.autoStash=false",
                    "-c",
                    "submodule.recurse=false",
                    "rebase",
                    &target,
                ],
            )
        }
        Request::CherryPick { target } => {
            worktree_guard(&p)?;
            no_operation(&p)?;
            clean(&p)?;
            commit_id(&p, &target)?;
            protect_untracked(&p, &[&target], None)?;
            retain_detached(&p)?;
            git(&p, &["cherry-pick", &target])
        }
        Request::Revert { target } => {
            worktree_guard(&p)?;
            no_operation(&p)?;
            clean(&p)?;
            commit_id(&p, &target)?;
            let parents = git(&p, &["rev-list", "--parents", "-n", "1", &target])?;
            let trees: Vec<_> = parents.split_whitespace().collect();
            protect_untracked(&p, &trees, None)?;
            retain_detached(&p)?;
            git(&p, &["revert", "--no-edit", &target])
        }
        Request::Reset { target, mode } => {
            worktree_guard(&p)?;
            no_operation(&p)?;
            commit_id(&p, &target)?;
            save_head(&p)?;
            match mode {
                ResetMode::Soft => git(&p, &["reset", "--soft", &target]),
                ResetMode::Mixed => {
                    save_index_and_worktree(&p, "Gall recovery before mixed reset")?;
                    git(&p, &["reset", "--mixed", &target])
                }
                ResetMode::Hard => {
                    // A hard reset overwrites untracked or ignored files that the target
                    // commit tracks. Reject only those collisions: unrelated untracked and
                    // ignored files (build output, dependencies) are left alone by Git.
                    protect_untracked(&p, &[&target], None)?;
                    save_index_and_worktree(&p, "Gall recovery before hard reset")?;
                    git(&p, &["reset", "--hard", &target])
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
    fn conflicting_stash_pop_keeps_stack_entry() {
        let d = repository();
        let p = d.path();
        commit(p, "first", "first\n");
        std::fs::write(p.join("file.txt"), "stash contents\n").unwrap();
        act(
            p,
            Request::StashSave {
                message: "conflicting".into(),
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
    fn rebase_with_submodules_rejects_staged_gitlinks_and_parent_edits() {
        let d = repository();
        let p = d.path();
        let _module = add_submodule(p);
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
        // Removing the last gitlink leaves no submodule entry in the index. The
        // remaining .gitmodules must still make us check before moving HEAD.
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
        std::fs::write(p.join(".gitmodules"), "local parent edit\n").unwrap();
        assert!(act(
            p,
            Request::Rebase {
                target: target.clone()
            }
        )
        .is_err());
        assert_eq!(head(p), original);
        assert_eq!(
            std::fs::read_to_string(p.join(".gitmodules")).unwrap(),
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

    #[test]
    fn literal_conflict_paths_and_refusal_of_unsupported_worktrees() {
        let d = repository();
        let p = d.path();
        let first = commit(p, "first", "first\n");
        std::fs::write(p.join(".gitmodules"), "[submodule \"future\"]\n").unwrap();
        assert!(snapshot(p).is_ok());
        assert!(act(p, Request::StashSave { message: "".into() }).is_err());
        std::fs::remove_file(p.join(".gitmodules")).unwrap();
        git(p, &["config", "filter.test.clean", "cat"]).unwrap();
        assert!(snapshot(p).is_ok());
        assert!(act(
            p,
            Request::Reset {
                target: first,
                mode: ResetMode::Soft
            }
        )
        .is_err());
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
        assert_eq!(
            std::fs::read_to_string(outside.path()).unwrap(),
            "private outside content"
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
    fn rebase_protects_ignored_paths_present_only_in_intermediate_commits() {
        let d = repository();
        let p = d.path();
        let first = commit(p, "first", "first\n");
        let name = "[cached]\nversion.tmp";
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
