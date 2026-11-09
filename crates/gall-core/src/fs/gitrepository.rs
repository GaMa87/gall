//! Typed local repository operations used by the Git workspace.
//!
//! The caller serializes mutations and supplies a freshly reviewed snapshot token.
//! Git still checks its own locks; a token cannot lock out another Git application.
use super::gitgraph::{git, has_content_filters, retain_detached, root};
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
    let value = git(path, &["rev-parse", "--git-path", name])?;
    Ok(path.join(value.trim()))
}

fn operation(path: &Path) -> Result<Option<String>> {
    for (marker, name) in [
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
    ] {
        if git_path(path, marker)?.exists() {
            return Ok(Some(name.into()));
        }
    }
    let todo = git_path(path, "sequencer/todo")?;
    if let Ok(value) = std::fs::read_to_string(todo) {
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
    git(path, &["rev-parse", "--verify", "HEAD"])
        .unwrap_or_default()
        .trim()
        .into()
}

fn status(path: &Path) -> Result<String> {
    checked_text(git(
        path,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
    )?)
}

fn conflicts(path: &Path) -> Result<Vec<String>> {
    let raw = checked_text(git(path, &["ls-files", "--unmerged", "-z"])?)?;
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
    checked_text(git(path, &["remote"])?)?
        .lines()
        .map(|name| {
            Ok(Remote {
                name: name.into(),
                fetch_url: git(path, &["remote", "get-url", "--", name])?.trim().into(),
                push_url: git(path, &["remote", "get-url", "--push", "--", name])?
                    .trim()
                    .into(),
            })
        })
        .collect()
}

/// Token covers references, index, tracked changes and untracked file metadata.
/// External Git clients must still be treated as concurrent writers after validation.
fn token(path: &Path) -> Result<String> {
    let mut h = DefaultHasher::new();
    head(path).hash(&mut h);
    git(path, &["symbolic-ref", "-q", "HEAD"])
        .unwrap_or_default()
        .hash(&mut h);
    git(path, &["for-each-ref", "--format=%(refname) %(objectname)"])?.hash(&mut h);
    git(path, &["ls-files", "--stage", "-z"])?.hash(&mut h);
    // Even status/diff may execute a configured clean filter. Filter repositories
    // are browsable, but working-tree mutations are refused by worktree_guard.
    if !has_filters(path) {
        status(path)?.hash(&mut h);
        git(
            path,
            &[
                "diff",
                "--binary",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--ignore-submodules=all",
            ],
        )?
        .hash(&mut h);
    }
    git(path, &["config", "--local", "--list", "-z"])?.hash(&mut h);
    git(path, &["stash", "list", "--format=%H%x00%gd"])?.hash(&mut h);
    operation(path)?.hash(&mut h);
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
        std::fs::read(git_path(path, marker)?).ok().hash(&mut h);
    }
    for name in checked_text(git(
        path,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)?
    .split('\0')
    .filter(|s| !s.is_empty())
    {
        name.hash(&mut h);
        if let Ok(meta) = path.join(name).symlink_metadata() {
            meta.len().hash(&mut h);
            meta.modified().ok().hash(&mut h);
        }
    }
    Ok(format!("{:016x}", h.finish()))
}

pub fn snapshot(path: &Path) -> Result<Repository> {
    let p = root(path)?;
    let before = token(&p)?;
    let state = Repository {
        root: p.to_string_lossy().into_owned(),
        head: head(&p),
        branch: git(&p, &["symbolic-ref", "--short", "-q", "HEAD"])
            .ok()
            .map(|s| s.trim().into()),
        refs: refs(&p)?,
        stashes: stashes(&p)?,
        remotes: remotes(&p)?,
        operation: operation(&p)?,
        operation_commit: operation_commit(&p),
        edit_stop: super::gitrewrite::is_edit_stop(&p),
        bisect_active: git_path(&p, "BISECT_START")?.exists(),
        conflicts: conflicts(&p)?,
        token: token(&p)?,
    };
    if before != state.token {
        return Err(fail(
            "The repository changed while loading. Refresh to review its current state",
        ));
    }
    Ok(state)
}

fn commit_id(path: &Path, value: &str) -> Result<()> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(fail("Select a concrete commit before this operation"));
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
    let checked = match kind {
        // Validate the literal ref, rather than allowing --branch to expand @{-1}.
        "branch" => git(path, &["check-ref-format", &format!("refs/heads/{name}")]),
        "tag" => git(path, &["check-ref-format", &format!("refs/tags/{name}")]),
        "remote" => git(
            path,
            &["check-ref-format", &format!("refs/remotes/{name}/check")],
        ),
        _ => return Err(fail("Invalid reference kind")),
    };
    checked
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

fn reference(path: &Path, kind: &str, name: &str) -> Result<Reference> {
    validate_name(path, name, kind)?;
    refs(path)?
        .into_iter()
        .find(|r| r.kind == kind && r.name == name)
        .ok_or_else(|| fail(format!("The selected {kind} no longer exists")))
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

fn worktree_guard(path: &Path) -> Result<()> {
    if has_filters(path) {
        return Err(fail(
            "Worktree operations with content filters require the terminal",
        ));
    }
    if path.join(".gitmodules").exists()
        || git(path, &["ls-files", "--stage"])?
            .lines()
            .any(|line| line.starts_with("160000 "))
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

fn protect_untracked(path: &Path, targets: &[&str], replay: Option<&str>) -> Result<()> {
    let other = checked_text(git(path, &["ls-files", "--others", "-z"])?)?;
    if other.is_empty() {
        return Ok(());
    }
    let mut touched = BTreeSet::new();
    for target in targets {
        for name in checked_text(git(path, &["ls-tree", "-r", "--name-only", "-z", target])?)?
            .split('\0')
            .filter(|name| !name.is_empty())
        {
            touched.insert(name.to_owned());
        }
    }
    if let Some(target) = replay {
        // Rebase can temporarily restore a path deleted again by a later commit.
        // Include every replayed path, not only the final HEAD tree.
        let raw = checked_text(git(
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
        )?)?;
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

fn save_index_and_worktree(path: &Path) -> Result<()> {
    // `stash create` stores index and tracked working content without cleaning
    // the working tree or changing the user's stash stack.
    let id = git(
        path,
        &["stash", "create", "Gall recovery before mixed reset"],
    )?;
    let id = id.trim();
    if !id.is_empty() {
        git(
            path,
            &[
                "update-ref",
                "--no-deref",
                &format!("refs/gall/recovery/{id}"),
                id,
            ],
        )?;
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
    git(path, &["-c", "core.editor=true", &current, command])
}

/// Execute one explicitly selected local action. Commits are unsigned, with hooks disabled.
pub fn execute(path: &Path, request: Request, expected: &str) -> Result<String> {
    let p = root(path)?;
    if git_path(&p, "BISECT_START")?.exists() {
        return Err(fail(
            "Finish bisect in Tools before other repository operations.",
        ));
    }
    if token(&p)? != expected {
        return Err(fail(
            "The repository changed. Refresh and review the selected action again",
        ));
    }
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
        Request::DeleteBranch { name } => {
            reference(&p, "branch", &name)?;
            git(&p, &["branch", "--delete", "--", &name])
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
            worktree_guard(&p)?;
            control(&p, "--continue")
        }
        Request::Skip => {
            worktree_guard(&p)?;
            control(&p, "--skip")
        }
        Request::Abort => {
            worktree_guard(&p)?;
            control(&p, "--abort")
        }
        Request::Resolve { file, side } => {
            worktree_guard(&p)?;
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
            worktree_guard(&p)?;
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
            worktree_guard(&p)?;
            no_operation(&p)?;
            clean(&p)?;
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
                    save_index_and_worktree(&p)?;
                    git(&p, &["reset", "--mixed", &target])
                }
                ResetMode::Hard => {
                    // A hard reset can overwrite ignored/untracked collisions. Reject
                    // those repositories instead of relying on Git's destructive default.
                    if !git(&p, &["ls-files", "--others", "-z"])?.is_empty() {
                        return Err(fail(
                            "Move or stash untracked and ignored files before a hard reset",
                        ));
                    }
                    if !status(&p)?.is_empty() {
                        git(
                            &p,
                            &[
                                "stash",
                                "push",
                                "--include-untracked",
                                "-m",
                                "Gall recovery before hard reset",
                            ],
                        )?;
                    }
                    clean(&p)?;
                    git(&p, &["reset", "--hard", &target])
                }
            }
        }
    }?;
    if !head(&p).is_empty() {
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
            },
        )
        .unwrap();
        act(p, Request::DeleteTag { name: "v1".into() }).unwrap();
        assert!(act(
            p,
            Request::DeleteBranch {
                name: "main".into()
            }
        )
        .is_err());
        assert!(!snapshot(p).unwrap().refs.iter().any(|r| r.name == "v1"));
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
        let state = snapshot(p).unwrap();
        assert_eq!(state.stashes.len(), 1);
        assert_eq!(
            git(p, &["show", &format!("{}:file.txt", state.stashes[0].id)]).unwrap(),
            "uncommitted\n"
        );
        std::fs::write(p.join("untracked.txt"), "preserve me\n").unwrap();
        assert!(act(
            p,
            Request::Reset {
                target: second,
                mode: ResetMode::Hard
            }
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(p.join("untracked.txt")).unwrap(),
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
