//! Commit preparation with stale-state checks and recoverable file discard.
use super::gitexec::{last_changed, remember_changed};
use super::gitgraph::{git, has_content_filters, root};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub path: String,
    pub original: Option<String>,
    pub index: char,
    pub worktree: char,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub root: String,
    pub edit_stop: bool,
    pub changes: Vec<Change>,
    pub token: String,
    pub head: String,
    pub subject: String,
    pub message: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexAction {
    Stage,
    Unstage,
    Commit,
    StageAll,
    UnstageAll,
    Amend,
    Discard,
    Ignore,
    Exclude,
}

fn protect_filtered_path(root: &Path, file: &str) -> Result<()> {
    if !has_content_filters(root) {
        return Ok(());
    }
    let record = git(root, &["check-attr", "-z", "filter", "--", file])?;
    match record.split_terminator('\0').nth(2) {
        Some("unspecified" | "unset") => Ok(()),
        Some(_) => Err(CoreError::Other(format!(
            "This operation cannot guarantee the original filtered bytes at {file}. Use index-only staging or keep a copy of the working file"
        ))),
        None => Err(CoreError::Other(format!("Could not inspect content filters for {file}"))),
    }
}

fn native_index_root(path: &Path) -> Result<PathBuf> {
    let p = root(path)?;
    let listed = super::gitfacts::markers(&p);
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "BISECT_START",
    ] {
        // One listing of the Git directory answers for all markers (see `gitfacts::Markers`).
        let present = match &listed {
            Some(listed) => listed.exists(marker),
            None => {
                let value = git(&p, &["rev-parse", "--git-path", marker])?;
                p.join(super::gitexec::host_path(&p, value.trim())).exists()
            }
        };
        if present {
            if marker == "rebase-merge" && super::gitrewrite::is_edit_stop(&p) {
                continue;
            }
            return Err(CoreError::Other(
                "Finish or abort the current Git operation first".into(),
            ));
        }
    }
    Ok(p)
}
fn head(path: &Path) -> String {
    if let Some(id) = super::gitfacts::head_id(path) {
        return id;
    }
    git(path, &["rev-parse", "--verify", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_owned()
}
/// HEAD, the index and the list of changes, read at the same time: three Git processes
/// side by side instead of one after another.
struct Seen {
    head: String,
    index: String,
    changes: Vec<Change>,
    /// Size and modification time of the index file before it was read.
    index_file: Stamp,
    /// Content digests of the paths changed at the last look, read while the changes were
    /// listed; `None` for a path that could not be read.
    early: Option<(Vec<String>, Vec<Option<u64>>)>,
    /// In WSL: the `stat` fingerprint of the paths changed at the last look, read alike.
    early_stat: Option<(Vec<String>, Vec<u8>)>,
}
type Stamp = Option<(u64, std::time::SystemTime)>;
fn stamp(file: &Path) -> Stamp {
    let meta = file.symlink_metadata().ok()?;
    Some((meta.len(), meta.modified().ok()?))
}
fn index_stamp(path: &Path) -> Stamp {
    stamp(&super::gitfacts::git_dir(path)?.join("index"))
}
fn see(path: &Path) -> Result<Seen> {
    let index_file = index_stamp(path);
    // The files changed at the last look are probably the ones changed now: their content
    // is read while Git lists the changes, instead of after it (see `token_of`).
    // In WSL their sizes and times, from one `stat` in the distribution.
    let hint = last_changed(path);
    let wsl = super::gitexec::place(path).is_some();
    std::thread::scope(|scope| {
        let head = scope.spawn(|| head(path));
        let index = scope.spawn(|| git(path, &["ls-files", "--stage", "-z"]));
        let early_stat = hint.clone().filter(|_| wsl).map(|names| {
            scope.spawn(move || super::gitexec::fingerprint(path, &names).map(|stat| (names, stat)))
        });
        let early = hint.filter(|_| !wsl).map(|names| {
            scope.spawn(move || {
                let digests = super::gitrepository::parallel_map(&names, |name| {
                    content_digest(&path.join(name)).ok()
                });
                (names, digests)
            })
        });
        let changes = changes(path);
        Ok(Seen {
            head: super::gitgraph::joined(head)?,
            index: super::gitgraph::joined(index)??,
            changes: changes?,
            index_file,
            early: early.and_then(|early| early.join().ok()),
            early_stat: early_stat.and_then(|early| early.join().ok().flatten()),
        })
    })
}

fn token(path: &Path) -> Result<String> {
    token_of(path, &see(path)?)
}
/// The state `seen` and the content of every changed file as one value: equal tokens mean
/// that nothing a commit, stage or discard depends on changed in between.
fn token_of(path: &Path, seen: &Seen) -> Result<String> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    seen.head.hash(&mut hash);
    seen.index.hash(&mut hash);
    let list = &seen.changes;
    for change in list {
        change.path.hash(&mut hash);
        change.index.hash(&mut hash);
        change.worktree.hash(&mut hash);
    }
    // In WSL one `stat` in the distribution covers every path. Reading each one over 9P
    // is slow, and symlinks there are reparse points that Windows cannot classify.
    // The one read while Git listed the changes counts when the list is the same.
    let names: Vec<String> = list.iter().map(|c| c.path.clone()).collect();
    let early_stat = seen
        .early_stat
        .as_ref()
        .filter(|(early, _)| *early == names)
        .map(|(_, stat)| stat.clone());
    if let Some(stat) = early_stat.or_else(|| super::gitexec::fingerprint(path, &names)) {
        stat.hash(&mut hash);
        remember_changed(path, names);
        return Ok(format!("{:016x}", hash.finish()));
    }
    // Thousands of changed files are read on several threads; the digests go in in order.
    // Read while Git listed the changes when the list is the one of the last look: the
    // content is as current as the list. A path that could not be read then is read again,
    // so its error is the one reported.
    let early = seen
        .early
        .as_ref()
        .filter(|(early, _)| *early == names)
        .map(|(_, digests)| digests);
    let digests = match early {
        Some(early) => early
            .iter()
            .zip(list)
            .map(|(digest, change)| match digest {
                Some(digest) => Ok(*digest),
                None => content_digest(&path.join(&change.path)),
            })
            .collect(),
        None => super::gitrepository::parallel_map(list, |change| {
            content_digest(&path.join(&change.path))
        }),
    };
    remember_changed(path, names);
    for digest in digests {
        digest?.hash(&mut hash);
    }
    Ok(format!("{:016x}", hash.finish()))
}
/// The content of one changed path for [`token_of`].
fn content_digest(file: &Path) -> Result<u64> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    match file.symlink_metadata() {
        Ok(metadata) if metadata.is_symlink() => {
            std::fs::read_link(file)
                .map_err(|e| CoreError::Other(e.to_string()))?
                .hash(&mut hash);
        }
        Ok(metadata) if metadata.is_file() => {
            let mut input =
                super::open_regular(file).map_err(|e| CoreError::Other(e.to_string()))?;
            let mut buffer = [0u8; 65536];
            loop {
                let n = input
                    .read(&mut buffer)
                    .map_err(|e| CoreError::Other(e.to_string()))?;
                if n == 0 {
                    break;
                }
                hash.write(&buffer[..n]);
            }
            metadata.len().hash(&mut hash);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode().hash(&mut hash);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing".hash(&mut hash),
        // An untracked nested repository is listed as a folder (`dir/`).
        Ok(metadata) if metadata.is_dir() => "dir".hash(&mut hash),
        Ok(_) => {
            return Err(CoreError::Other(
                "A changed path is not a regular file or symlink".into(),
            ))
        }
        Err(e) => return Err(CoreError::Other(e.to_string())),
    }
    Ok(hash.finish())
}
fn verify_token(path: &Path, expected: &str) -> Result<()> {
    verified(path, expected).map(|_| ())
}
/// Checks the token and returns the state it was computed from, so the action that
/// follows does not ask Git for HEAD and the changes a second time.
fn verified(path: &Path, expected: &str) -> Result<Seen> {
    let seen = see(path)?;
    if token_of(path, &seen)? != expected {
        return Err(CoreError::Other(
            "The index, HEAD or working files changed. Refresh and review the changes again".into(),
        ));
    }
    Ok(seen)
}
fn changes(path: &Path) -> Result<Vec<Change>> {
    let raw = super::gitgraph::status_z(path)?;
    if raw.contains('\u{fffd}') {
        return Err(CoreError::Other(
            "Git paths with invalid UTF-8 are not supported".into(),
        ));
    }
    let mut fields = raw.split('\0').filter(|v| !v.is_empty());
    let mut out = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            return Err(CoreError::Other("Invalid Git status record".into()));
        }
        let index = field.as_bytes()[0] as char;
        let worktree = field.as_bytes()[1] as char;
        let original = if index == 'R' || index == 'C' || worktree == 'R' || worktree == 'C' {
            fields.next().map(str::to_owned)
        } else {
            None
        };
        out.push(Change {
            path: field[3..].into(),
            original,
            index,
            worktree,
        });
    }
    Ok(out)
}
pub fn workspace(path: &Path) -> Result<Workspace> {
    let root = native_index_root(path)?;
    super::gitfacts::keep_commit_graph(&root);
    std::thread::scope(|scope| {
        // Subject and full message in one query, next to the state.
        let last = scope.spawn(|| git(&root, &["log", "-1", "--format=%s%x00%B"]));
        let seen = see(&root);
        let last = super::gitgraph::joined(last)?.unwrap_or_default();
        let seen = seen?;
        let (subject, message) = last.split_once('\0').unwrap_or((&last, ""));
        Ok(Workspace {
            root: root.to_string_lossy().into_owned(),
            edit_stop: super::gitrewrite::is_edit_stop(&root),
            token: token_of(&root, &seen)?,
            head: seen.head,
            changes: seen.changes,
            subject: subject.trim_end().to_owned(),
            message: message.trim_end().to_owned(),
        })
    })
}
pub fn index_action(
    path: &Path,
    action: IndexAction,
    file: Option<&str>,
    message: &str,
    expected: &str,
) -> Result<()> {
    use super::gitjournal::Kind;
    let label = || {
        let subject = message.trim().lines().next().unwrap_or_default();
        subject.chars().take(100).collect::<String>()
    };
    let operation = match action {
        IndexAction::Commit => Some((Kind::Commit, label())),
        IndexAction::Amend => Some((Kind::Amend, label())),
        _ => None,
    };
    super::gitjournal::around(path, operation, || {
        index_action_now(path, action, file, message, expected)
    })
}

fn index_action_now(
    path: &Path,
    action: IndexAction,
    file: Option<&str>,
    message: &str,
    expected: &str,
) -> Result<()> {
    let _busy = super::gitfacts::mutating();
    let root = native_index_root(path)?;
    if let (IndexAction::Discard, Some(file)) = (&action, file) {
        protect_filtered_path(&root, file)?;
    }
    let seen = verified(&root, expected)?;
    if matches!(action, IndexAction::Commit) && super::gitrewrite::is_edit_stop(&root) {
        return Err(CoreError::Other(
            "Use Amend at this interactive edit stop, then Continue the rebase".into(),
        ));
    }
    match action {
        IndexAction::Commit | IndexAction::Amend => {
            super::gitprogress::report_phase(
                if matches!(action, IndexAction::Amend) {
                    "Amending commit"
                } else {
                    "Committing changes"
                },
                None,
            );
            if message.trim().is_empty() {
                return Err(CoreError::Other("A commit message is required".into()));
            }
            // Staged: the index column of the status says so (what
            // `diff --cached --name-only` lists).
            if matches!(action, IndexAction::Commit)
                && !seen
                    .changes
                    .iter()
                    .any(|change| !matches!(change.index, ' ' | '?' | '!'))
            {
                return Err(CoreError::Other("Stage changes before committing".into()));
            }
            if matches!(action, IndexAction::Amend) {
                let previous = head(&root);
                if previous.is_empty() {
                    return Err(CoreError::Other("There is no commit to amend".into()));
                }
                git(
                    &root,
                    &[
                        "update-ref",
                        "--no-deref",
                        &format!("refs/gall/recovery/{previous}"),
                        &previous,
                    ],
                )?;
                git(&root, &["commit", "--amend", "-m", message])?;
                super::gitgraph::drop_recovery(&root, &previous);
            } else {
                git(&root, &["commit", "-m", message])?;
            }
            super::gitgraph::retain_detached(&root).map_err(|e| {
                CoreError::Other(format!(
                    "Commit created, but its recovery reference could not be saved: {e}"
                ))
            })?;
        }
        IndexAction::StageAll => {
            super::gitprogress::report_phase("Staging changes", None);
            // Submodule pointers are not listed in the changes, so they are not staged blindly.
            let mut args = vec!["add".to_owned(), "--all".into(), "--".into(), ".".into()];
            for line in git(&root, &["ls-files", "--stage", "-z"])?.split('\0') {
                if let Some((_, name)) = line
                    .strip_prefix("160000 ")
                    .and_then(|rest| rest.split_once('\t'))
                {
                    args.push(format!(":(exclude,literal){name}"));
                }
            }
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            git(&root, &args)?;
        }
        IndexAction::UnstageAll => {
            super::gitprogress::report_phase("Unstaging changes", None);
            if head(&root).is_empty() {
                git(&root, &["rm", "--cached", "-r", "--", "."])?;
            } else {
                git(&root, &["restore", "--staged", "--", "."])?;
            }
        }
        IndexAction::Discard | IndexAction::Ignore | IndexAction::Exclude => {
            let list = changes(&root)?;
            let entry = list
                .iter()
                .find(|entry| Some(entry.path.as_str()) == file)
                .ok_or_else(|| {
                    CoreError::Other("This file is no longer in the change list".into())
                })?;
            if matches!(action, IndexAction::Discard) {
                discard(&root, entry, expected)?;
            } else {
                if entry.index != '?' {
                    return Err(CoreError::Other(
                        "Only untracked files can be ignored without changing tracked history"
                            .into(),
                    ));
                }
                ignore(&root, &entry.path, matches!(action, IndexAction::Exclude))?;
            }
        }
        IndexAction::Stage | IndexAction::Unstage => {
            let entry = seen
                .changes
                .iter()
                .find(|e| Some(e.path.as_str()) == file)
                .ok_or_else(|| {
                    CoreError::Other("This file is no longer in the change list".into())
                })?;
            let paths: Vec<String> = std::iter::once(entry.path.as_str())
                .chain(if matches!(action, IndexAction::Unstage) {
                    entry.original.as_deref()
                } else {
                    None
                })
                .map(|p| format!(":(literal){p}"))
                .collect();
            let unborn = seen.head.is_empty();
            let mut args = match action {
                IndexAction::Stage => vec!["add", "--all", "--"],
                _ if unborn => vec!["rm", "--cached", "--"],
                _ => vec!["restore", "--staged", "--"],
            };
            args.extend(paths.iter().map(String::as_str));
            git(&root, &args)?;
        }
    }
    Ok(())
}
fn discard(path: &Path, entry: &Change, expected: &str) -> Result<()> {
    let spec = format!(":(literal){}", entry.path);
    if entry.index == '?' {
        super::gitgraph::push_recovery(path, &["--", &spec], "Gall recovery before file discard")?;
        return Ok(());
    }
    if entry.worktree == ' ' {
        return Err(CoreError::Other(
            "Unstage the file before discarding staged changes".into(),
        ));
    }
    let saved = git(
        path,
        &["stash", "create", "Gall recovery before file discard"],
    )?;
    if saved.trim().is_empty() {
        return Err(CoreError::Other(
            "Could not preserve the file before discarding".into(),
        ));
    }
    super::gitgraph::retain_recovery(path, saved.trim(), "Gall recovery before file discard")?;
    verify_token(path, expected)?;
    git(path, &["restore", "--worktree", "--", &spec])?;
    Ok(())
}
fn ignore(path: &Path, file: &str, local: bool) -> Result<()> {
    if file.contains(['\n', '\r']) {
        return Err(CoreError::Other(
            "Filenames with line breaks cannot be written as ignore rules".into(),
        ));
    }
    let rule = format!("/{}", escape_rule(file));
    if local {
        let common = git(path, &["rev-parse", "--git-common-dir"])?;
        let common = path
            .join(super::gitexec::host_path(path, common.trim()))
            .canonicalize()
            .map_err(|e| CoreError::Other(e.to_string()))?;
        exclude_locally(&common, &rule)
    } else {
        append_rule(&path.join(".gitignore"), &rule, &[])
    }
}
/// `name` with the characters escaped that have a meaning in ignore rules.
pub(super) fn escape_rule(name: &str) -> String {
    let mut rule = String::new();
    for c in name.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']' | '!' | '#' | ' ') {
            rule.push('\\');
        }
        rule.push(c);
    }
    rule
}
/// Adds `rule` to `info/exclude` of the common Git directory, which no commit contains.
pub(super) fn exclude_locally(common: &Path, rule: &str) -> Result<()> {
    let info = common.join("info");
    if info
        .symlink_metadata()
        .is_ok_and(|m| !m.is_dir() || m.is_symlink())
    {
        return Err(CoreError::Other(
            "Refusing to append local excludes through a symlinked metadata directory".into(),
        ));
    }
    // A folder rule may already be there in another spelling that means the same.
    let mut same = Vec::new();
    if let Some(folder) = rule
        .strip_prefix('/')
        .and_then(|rule| rule.strip_suffix('/'))
    {
        same.push(format!("/{folder}"));
        if !folder.contains('/') {
            same.extend([folder.to_string(), format!("{folder}/")]);
        }
    }
    append_rule(&info.join("exclude"), rule, &same)
}
/// Appends `rule` to an ignore file unless it, or one of `same`, is already there.
fn append_rule(target: &Path, rule: &str, same: &[String]) -> Result<()> {
    if target
        .symlink_metadata()
        .is_ok_and(|m| m.is_symlink() || !m.is_file())
    {
        return Err(CoreError::Other(
            "Refusing to append ignore rules through a symlink".into(),
        ));
    }
    let content = match std::fs::read_to_string(target) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(CoreError::Other(e.to_string())),
    };
    if content
        .lines()
        .any(|line| line == rule || same.iter().any(|other| line == other))
    {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::Other(e.to_string()))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut output = options
        .open(target)
        .map_err(|e| CoreError::Other(e.to_string()))?;
    // Always delimit an appended rule, including if another writer just appended a partial line.
    let suffix = format!("\n{rule}\n");
    output
        .write_all(suffix.as_bytes())
        .map_err(|e| CoreError::Other(e.to_string()))
}
pub fn patch(path: &Path, file: &str, staged: bool) -> Result<String> {
    let root = native_index_root(path)?;
    patch_of(&root, &changes(&root)?, file, staged)
}
/// The patch of `file`, one entry of the change `list` of the work tree `root`.
fn patch_of(root: &Path, list: &[Change], file: &str, staged: bool) -> Result<String> {
    let entry = list
        .iter()
        .find(|e| e.path == file)
        .ok_or_else(|| CoreError::Other("This file is no longer in the change list".into()))?;
    if entry.index == '?' {
        use std::io::Read;
        let p = root.join(file);
        if p.symlink_metadata()
            .map_err(|e| CoreError::Other(e.to_string()))?
            .is_symlink()
        {
            return Ok(std::fs::read_link(p)
                .map_err(|e| CoreError::Other(e.to_string()))?
                .to_string_lossy()
                .into_owned());
        }
        let mut bytes = Vec::new();
        super::open_regular(&p)
            .map_err(|e| CoreError::Other(e.to_string()))?
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|e| CoreError::Other(e.to_string()))?;
        if bytes.contains(&0) {
            return Ok("Binary file (preview unavailable)".into());
        }
        let truncated = bytes.len() > 65536;
        bytes.truncate(65536);
        return Ok(format!(
            "{}{}",
            String::from_utf8_lossy(&bytes),
            if truncated {
                "\n[Preview truncated]"
            } else {
                ""
            }
        ));
    }
    let spec = format!(":(literal){file}");
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", &spec]);
    let original = entry.original.as_ref().map(|p| format!(":(literal){p}"));
    if staged {
        if let Some(original) = &original {
            args.push(original);
        }
    }
    git(root, &args)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchLine {
    pub index: usize,
    pub text: String,
    pub kind: String,
    pub hunk: Option<usize>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionPatch {
    pub text: String,
    pub token: String,
    pub selectable: bool,
    pub reason: Option<String>,
    pub lines: Vec<PatchLine>,
}
fn patch_token(path: &Path, file: &str, staged: bool, text: &str) -> Result<String> {
    Ok(patch_token_of(&token(path)?, file, staged, text))
}
fn patch_token_of(token: &str, file: &str, staged: bool, text: &str) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hash);
    file.hash(&mut hash);
    staged.hash(&mut hash);
    text.hash(&mut hash);
    format!("{:016x}", hash.finish())
}
pub fn selection_patch(path: &Path, file: &str, staged: bool) -> Result<SelectionPatch> {
    // The index may be rewritten once in the background while this reads (see
    // `gitfacts::refresh_index`); that is no change of content, so one more try.
    match selection_patch_once(path, file, staged) {
        Err(CoreError::Other(message)) if message.starts_with("The index, HEAD or working") => {
            selection_patch_once(path, file, staged)
        }
        result => result,
    }
}
fn selection_patch_once(path: &Path, file: &str, staged: bool) -> Result<SelectionPatch> {
    let root = native_index_root(path)?;
    // The patch must describe one state: the state is read, then the patch, then the
    // state again, and the two have to agree. Each state is read once (it used to be read
    // three times, and the list of changes five times).
    let seen = see(&root)?;
    let before = token_of(&root, &seen)?;
    let on_disk = stamp(&root.join(file));
    let text = patch_of(&root, &seen.changes, file, staged)?;
    // What this patch is made of must not have moved while it was read: HEAD, the index
    // and the file itself. Anything else that changed meanwhile makes the token stale,
    // and applying a selection checks the whole state again before it touches the index.
    // (A full second look cost another status of the whole work tree per file clicked.)
    if head(&root) != seen.head
        || seen.index_file.is_none()
        || index_stamp(&root) != seen.index_file
        || stamp(&root.join(file)) != on_disk
    {
        return Err(CoreError::Other(
            "The index, HEAD or working files changed. Refresh and review the changes again".into(),
        ));
    }
    let entry = seen
        .changes
        .into_iter()
        .find(|e| e.path == file)
        .ok_or_else(|| CoreError::Other("The selected file changed. Refresh the changes".into()))?;
    let unsupported = entry.index == '?'
        || entry.original.is_some()
        || text.len() > 2_000_000
        || text.lines().any(|l| {
            l.starts_with("new file mode ")
                || l.starts_with("deleted file mode ")
                || l.starts_with("old mode ")
                || l.starts_with("new mode ")
                || l.starts_with("Binary files ")
                || l.starts_with("GIT binary patch")
                || l.starts_with("\\ No newline at end of file")
        })
        || !text.lines().any(|l| l.starts_with("@@ "));
    let mut hunk = None;
    let lines = text
        .split_terminator('\n')
        .enumerate()
        .map(|(index, line)| {
            if line.starts_with("@@ ") {
                hunk = Some(hunk.map_or(0, |n| n + 1));
            }
            let kind = if hunk.is_some() && line.starts_with('+') {
                "added"
            } else if hunk.is_some() && line.starts_with('-') {
                "deleted"
            } else if hunk.is_some() && line.starts_with(' ') {
                "context"
            } else {
                "header"
            };
            PatchLine {
                index,
                text: line.into(),
                kind: kind.into(),
                hunk,
            }
        })
        .collect();
    Ok(SelectionPatch { token: patch_token_of(&before, file, staged, &text), text, selectable: !unsupported,
        reason: unsupported.then(|| "Select the whole file for additions, deletions, renames, binary/mode changes, very large files or files without a final newline.".into()), lines })
}
/// The line a hunk side starts at, as the position of its first line. A side without
/// lines (`-0,0` of an empty file) names the line before it, so it is moved forward.
fn hunk_start(header: &str, index: usize) -> Result<i64> {
    let invalid = || CoreError::Other("Invalid Git patch hunk header".into());
    let field = header
        .split_whitespace()
        .nth(index)
        .and_then(|field| field.get(1..))
        .ok_or_else(invalid)?;
    let mut parts = field.split(',');
    let start: i64 = parts
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(invalid)?;
    let count: i64 = match parts.next() {
        Some(n) => n.parse().map_err(|_| invalid())?,
        None => 1,
    };
    Ok(if count == 0 { start + 1 } else { start })
}
fn selected_patch(patch: &SelectionPatch, selected: &[usize], reverse: bool) -> Result<String> {
    if !patch.selectable {
        return Err(CoreError::Other(patch.reason.clone().unwrap_or_default()));
    }
    if selected.is_empty() {
        return Err(CoreError::Other(
            "Select at least one changed line or hunk".into(),
        ));
    }
    let chosen: std::collections::HashSet<usize> = selected.iter().copied().collect();
    if chosen.iter().any(|index| {
        !patch
            .lines
            .iter()
            .any(|line| line.index == *index && matches!(line.kind.as_str(), "added" | "deleted"))
    }) {
        return Err(CoreError::Other(
            "The selection includes a line that cannot be staged".into(),
        ));
    }
    let mut result = String::new();
    for line in patch.lines.iter().take_while(|line| line.hunk.is_none()) {
        result.push_str(&line.text);
        result.push('\n');
    }
    let count = patch
        .lines
        .iter()
        .filter_map(|line| line.hunk)
        .max()
        .unwrap_or(0)
        + 1;
    let mut delta = 0i64;
    for hunk in 0..count {
        let lines: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| line.hunk == Some(hunk))
            .collect();
        if !lines.iter().any(|line| chosen.contains(&line.index)) {
            continue;
        }
        let header = &lines[0].text;
        let mut old_count = 0i64;
        let mut new_count = 0i64;
        let mut body = String::new();
        for line in lines.iter().skip(1) {
            let chosen = chosen.contains(&line.index);
            let sign = match line.kind.as_str() {
                "added" if chosen => Some('+'),
                "deleted" if chosen => Some('-'),
                "added" if reverse => Some(' '),
                "deleted" if !reverse => Some(' '),
                "context" => Some(' '),
                _ => None,
            };
            if let Some(sign) = sign {
                if sign != '+' {
                    old_count += 1;
                }
                if sign != '-' {
                    new_count += 1;
                }
                body.push(sign);
                body.push_str(&line.text[1..]);
                body.push('\n');
            }
        }
        let (mut old_start, mut new_start) = if reverse {
            let start = hunk_start(header, 2)?;
            (start - delta, start)
        } else {
            let start = hunk_start(header, 1)?;
            (start, start + delta)
        };
        if old_count == 0 {
            old_start -= 1;
        }
        if new_count == 0 {
            new_start -= 1;
        }
        result.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n{body}"
        ));
        delta += new_count - old_count;
    }
    Ok(result)
}
fn apply_cached(path: &Path, text: &str, reverse: bool) -> Result<()> {
    apply_patch(path, text, reverse, true)
}

pub(super) fn apply_cached_to(path: &Path, text: &str, index: &Path) -> Result<()> {
    apply_text_with_index(path, text, false, true, false, true, Some(index))
}
/// `git apply` of a selection patch: to the index (`cached`) or, reversed, to the working file.
fn apply_patch(path: &Path, text: &str, reverse: bool, cached: bool) -> Result<()> {
    apply_text(path, text, reverse, cached, false, true)
}
/// `git apply`: `check` only tests, `zero` accepts patches without context (the staging patches), which must not be used
/// for a patch that is applied to a file that may differ from the one it was made from.
fn apply_text(
    path: &Path,
    text: &str,
    reverse: bool,
    cached: bool,
    check: bool,
    zero: bool,
) -> Result<()> {
    apply_text_with_index(path, text, reverse, cached, check, zero, None)
}
fn apply_text_with_index(
    path: &Path,
    text: &str,
    reverse: bool,
    cached: bool,
    check: bool,
    zero: bool,
    index: Option<&Path>,
) -> Result<()> {
    let mut command = super::gitexec::command_with_index(path, super::gitstatus::program(), index);
    command
        .arg("-C")
        .arg(super::gitexec::dir_arg(path))
        .args(["-c", "core.fsmonitor=false", "-c"])
        .arg(format!(
            "core.hooksPath={}",
            super::gitexec::null_device(path)
        ))
        .arg("apply");
    if cached {
        command.arg("--cached");
    }
    if zero {
        command.arg("--unidiff-zero");
    }
    command.args(["--whitespace=nowarn", "--recount"]);
    if check {
        command.arg("--check");
    }
    if reverse {
        command.arg("--reverse");
    }
    command
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    super::gitexec::forward_env(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| CoreError::Other(e.to_string()))?;
    let write = child.stdin.take().unwrap().write_all(text.as_bytes());
    let output = child
        .wait_with_output()
        .map_err(|e| CoreError::Other(e.to_string()))?;
    if !output.status.success() {
        return Err(CoreError::Other(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    write.map_err(|e| CoreError::Other(e.to_string()))
}
pub fn apply_selection(
    path: &Path,
    file: &str,
    staged: bool,
    selected: &[usize],
    expected: &str,
) -> Result<()> {
    let _busy = super::gitfacts::mutating();
    let root = native_index_root(path)?;
    let patch = selection_patch(&root, file, staged)?;
    if patch.token != expected {
        return Err(CoreError::Other(
            "The selected diff changed. Refresh and select the changes again".into(),
        ));
    }
    let text = selected_patch(&patch, selected, staged)?;
    if patch_token(&root, file, staged, &patch.text)? != expected {
        return Err(CoreError::Other(
            "The index or working file changed while applying the selection".into(),
        ));
    }
    apply_cached(&root, &text, staged)
}
/// Discards the selected changed lines of an unstaged file from the working file. The whole state is saved first
/// as a recovery reference (like a file discard), and the same token checks as staging apply, so nothing is
/// thrown away from a diff that is no longer what the user saw.
pub fn discard_selection(
    path: &Path,
    file: &str,
    selected: &[usize],
    expected: &str,
) -> Result<()> {
    let _busy = super::gitfacts::mutating();
    let root = native_index_root(path)?;
    protect_filtered_path(&root, file)?;
    let patch = selection_patch(&root, file, false)?;
    if patch.token != expected {
        return Err(CoreError::Other(
            "The selected diff changed. Refresh and select the changes again".into(),
        ));
    }
    // The working file is the new side of this diff, so the "unstage" form of the patch (new side kept in
    // step) is the one that reverses cleanly onto it.
    let text = selected_patch(&patch, selected, true)?;
    let saved = git(
        &root,
        &["stash", "create", "Gall recovery before line discard"],
    )?;
    if saved.trim().is_empty() {
        return Err(CoreError::Other(
            "Could not preserve the file before discarding lines".into(),
        ));
    }
    super::gitgraph::retain_recovery(&root, saved.trim(), "Gall recovery before line discard")?;
    if patch_token(&root, file, false, &patch.text)? != expected {
        return Err(CoreError::Other(
            "The index or working file changed while discarding the selection".into(),
        ));
    }
    apply_patch(&root, &text, true, false)
}

/// Stashes the selected changed lines of an unstaged file and removes them from the working file. The whole state
/// is saved first as a recovery reference. Nothing may be staged: the stash is made of HEAD and these lines alone.
pub fn stash_selection(
    path: &Path,
    file: &str,
    selected: &[usize],
    message: &str,
    expected: &str,
) -> Result<()> {
    let _busy = super::gitfacts::mutating();
    let root = native_index_root(path)?;
    protect_filtered_path(&root, file)?;
    let patch = selection_patch(&root, file, false)?;
    if patch.token != expected {
        return Err(CoreError::Other(
            "The selected diff changed. Refresh and select the changes again".into(),
        ));
    }
    if !git(&root, &["diff", "--cached", "--name-only", "-z"])?.is_empty() {
        return Err(CoreError::Other(
            "Unstage or commit the staged changes before stashing lines".into(),
        ));
    }
    let forward = selected_patch(&patch, selected, false)?;
    let backward = selected_patch(&patch, selected, true)?;
    let saved = git(
        &root,
        &["stash", "create", "Gall recovery before line stash"],
    )?;
    if saved.trim().is_empty() {
        return Err(CoreError::Other(
            "Could not preserve the file before stashing lines".into(),
        ));
    }
    super::gitgraph::retain_recovery(&root, saved.trim(), "Gall recovery before line stash")?;
    if patch_token(&root, file, false, &patch.text)? != expected {
        return Err(CoreError::Other(
            "The index or working file changed while stashing the selection".into(),
        ));
    }
    // The lines are put into the index for a moment to get the tree with them; nothing else is staged.
    apply_cached(&root, &forward, false)?;
    let tree = git(&root, &["write-tree"]);
    let restored = apply_cached(&root, &forward, true)
        .or_else(|_| git(&root, &["reset", "-q", "--mixed", "HEAD"]).map(|_| ()));
    let tree = tree?;
    restored?;
    let head = git(&root, &["rev-parse", "--verify", "HEAD"])?;
    let head = head.trim();
    let branch = git(&root, &["symbolic-ref", "-q", "--short", "HEAD"])
        .map(|name| name.trim().to_owned())
        .unwrap_or_else(|_| "(no branch)".into());
    let message = match message.trim() {
        "" => "Gall line stash",
        message => message,
    };
    let title = format!("On {branch}: {message}");
    let index = git(
        &root,
        &[
            "commit-tree",
            &format!("{head}^{{tree}}"),
            "-p",
            head,
            "-m",
            &format!("index on {branch}"),
        ],
    )?;
    let stash = git(
        &root,
        &[
            "commit-tree",
            tree.trim(),
            "-p",
            head,
            "-p",
            index.trim(),
            "-m",
            &title,
        ],
    )?;
    let stash = stash.trim();
    git(&root, &["stash", "store", "-m", &title, stash])?;
    // The working file is the new side of the diff, see `discard_selection`.
    if let Err(error) = apply_patch(&root, &backward, true, false) {
        // The lines are still in the file, so the entry would be a second copy of them.
        let top = git(&root, &["rev-parse", "-q", "--verify", "refs/stash"]).unwrap_or_default();
        if top.trim() == stash {
            let _ = git(&root, &["stash", "drop", "-q", "stash@{0}"]);
        }
        return Err(error);
    }
    Ok(())
}

/// What dragging a changed file or hunk onto another worktree would do, and the token of the state it was made from.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub token: String,
    /// The file is copied (a new file, which the target does not have) instead of a patch applied.
    pub copy: bool,
    /// Changed lines in the patch (added plus deleted); 0 for a copy.
    pub lines: usize,
    /// The target has local changes; they are saved as a recovery reference before anything is applied.
    pub target_changed: bool,
}
fn same_repository(a: &Path, b: &Path) -> Result<bool> {
    let common = |path: &Path| -> Result<PathBuf> {
        let value = git(path, &["rev-parse", "--git-common-dir"])?;
        path.join(super::gitexec::host_path(path, value.trim()))
            .canonicalize()
            .map_err(|e| CoreError::Other(e.to_string()))
    };
    Ok(common(a)? == common(b)?)
}
/// The patch of the chosen lines (`selected`, or all changed lines of the file) for a forward `git apply` in another worktree.
fn transfer_patch(patch: &SelectionPatch, selected: Option<&[usize]>) -> Result<(String, usize)> {
    let all: Vec<usize> = patch
        .lines
        .iter()
        .filter(|line| matches!(line.kind.as_str(), "added" | "deleted"))
        .map(|line| line.index)
        .collect();
    let chosen = selected.map_or(all, |list| list.to_vec());
    let count = chosen.len();
    // The patch keeps its context lines: Git then only applies it where the file there looks the same.
    Ok((selected_patch(patch, &chosen, false)?, count))
}
/// Brings the changed file `file` of the worktree `path`, or only the changed lines `selected` of it, to the worktree
/// `target` of the same repository (the working files only; nothing is staged there). `apply: false` only checks and
/// returns the token the real call must name. Refused when Git cannot apply the patch to the file as it is there, or for
/// a change that is no plain text patch (deleted, renamed, binary or mode changes); a new file is copied when the target
/// has nothing at that path. The local changes of the target are saved as a recovery reference first.
pub fn transfer(
    path: &Path,
    target: &Path,
    file: &str,
    staged: bool,
    selected: Option<&[usize]>,
    expected: Option<&str>,
    apply: bool,
) -> Result<Transfer> {
    let _busy = apply.then(super::gitfacts::mutating);
    let root = native_index_root(path)?;
    let destination = native_index_root(target)?;
    if root == destination {
        return Err(CoreError::Other("Choose another worktree".into()));
    }
    if !same_repository(&root, &destination)? {
        return Err(CoreError::Other(
            "The two folders are not worktrees of the same repository".into(),
        ));
    }
    if file.is_empty()
        || Path::new(file).is_absolute()
        || Path::new(file)
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(CoreError::Other("Invalid file path".into()));
    }
    protect_filtered_path(&root, file)?;
    protect_filtered_path(&destination, file)?;
    let patch = selection_patch(&root, file, staged)?;
    if expected.is_some_and(|token| token != patch.token) {
        return Err(CoreError::Other(
            "The change was edited meanwhile. Refresh and drag it again".into(),
        ));
    }
    if apply && expected.is_none() {
        return Err(CoreError::Other(
            "Check the transfer before applying it".into(),
        ));
    }
    let target_changed = !git(
        &destination,
        &["status", "--porcelain", "--ignore-submodules=dirty"],
    )?
    .is_empty();
    if !patch.selectable {
        // A new file: copied when the other worktree has nothing there.
        let source = root.join(file);
        let target_file = destination.join(file);
        let new_file = !patch.text.lines().any(|l| l.starts_with("@@ "))
            || patch.text.lines().any(|l| l.starts_with("new file mode "));
        let plain = source
            .symlink_metadata()
            .is_ok_and(|m| m.is_file() && m.len() <= 50 * 1024 * 1024);
        let binary = patch.text.lines().any(|l| {
            l.starts_with("Binary files ")
                || l.starts_with("GIT binary patch")
                || l.starts_with("deleted file mode ")
                || l.starts_with("rename ")
        });
        if !(new_file && plain && !binary && selected.is_none()) {
            return Err(CoreError::Other(patch.reason.clone().unwrap_or_else(
                || "This change cannot be moved by dragging; copy the file by hand".into(),
            )));
        }
        if target_file.symlink_metadata().is_ok() {
            return Err(CoreError::Other(format!(
                "{file} already exists in the other worktree; nothing was overwritten"
            )));
        }
        if apply {
            if let Some(parent) = target_file.parent() {
                std::fs::create_dir_all(parent).map_err(|e| CoreError::Other(e.to_string()))?;
            }
            std::fs::copy(&source, &target_file).map_err(|e| CoreError::Other(e.to_string()))?;
        }
        return Ok(Transfer {
            token: patch.token,
            copy: true,
            lines: 0,
            target_changed,
        });
    }
    let (text, lines) = transfer_patch(&patch, selected)?;
    apply_text(&destination, &text, false, false, true, false).map_err(|e| {
        CoreError::Other(format!(
            "The change does not fit the file in the other worktree: {e}"
        ))
    })?;
    if apply {
        if target_changed {
            let saved = git(
                &destination,
                &[
                    "stash",
                    "create",
                    "Gall recovery before a transferred change",
                ],
            )?;
            if !saved.trim().is_empty() {
                super::gitgraph::retain_recovery(
                    &destination,
                    saved.trim(),
                    "Gall recovery before a transferred change",
                )?;
            }
        }
        apply_text(&destination, &text, false, false, false, false)?;
    }
    Ok(Transfer {
        token: patch.token,
        copy: false,
        lines,
        target_changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_index_actions_stage_filtered_bytes_without_changing_the_worktree() {
        let directory = tempfile::tempdir().unwrap();
        let p = directory.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(p, &["config", key, value]).unwrap();
        }
        std::fs::write(p.join(".gitattributes"), "filtered.txt filter=roundtrip\n").unwrap();
        std::fs::write(p.join("filtered.txt"), "repository old\n").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "Base"]).unwrap();
        for (key, value) in [
            ("filter.roundtrip.clean", "sed 's/working/repository/g'"),
            ("filter.roundtrip.smudge", "sed 's/repository/working/g'"),
            ("filter.roundtrip.required", "true"),
        ] {
            git(p, &["config", key, value]).unwrap();
        }
        let action = |action, message: &str| {
            let state = workspace(p).unwrap();
            index_action(p, action, Some("filtered.txt"), message, &state.token)
        };
        let working = "working change\n";
        std::fs::write(p.join("filtered.txt"), working).unwrap();
        action(IndexAction::Stage, "").unwrap();
        assert_eq!(
            git(p, &["show", ":filtered.txt"]).unwrap(),
            "repository change\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("filtered.txt")).unwrap(),
            working
        );
        action(IndexAction::Unstage, "").unwrap();
        assert_eq!(
            git(p, &["show", ":filtered.txt"]).unwrap(),
            "repository old\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("filtered.txt")).unwrap(),
            working
        );
        assert!(action(IndexAction::Discard, "")
            .unwrap_err()
            .to_string()
            .contains("filtered.txt"));
        assert_eq!(
            std::fs::read_to_string(p.join("filtered.txt")).unwrap(),
            working
        );
        std::fs::write(p.join("unrelated.txt"), "untracked local bytes\n").unwrap();
        let unrelated = workspace(p).unwrap();
        index_action(
            p,
            IndexAction::Discard,
            Some("unrelated.txt"),
            "",
            &unrelated.token,
        )
        .unwrap();
        assert!(!p.join("unrelated.txt").exists());
        assert_eq!(
            std::fs::read_to_string(p.join("filtered.txt")).unwrap(),
            working
        );
        action(IndexAction::StageAll, "").unwrap();
        action(IndexAction::Commit, "Filtered commit").unwrap();
        assert_eq!(
            git(p, &["show", "HEAD:filtered.txt"]).unwrap(),
            "repository change\n"
        );
    }

    #[test]
    fn changes_load_when_an_untracked_nested_repository_is_listed() {
        let temp = fixture();
        let root = temp.path();
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        git(&nested, &["init", "-q"]).unwrap();
        std::fs::write(nested.join("a.txt"), "x").unwrap();
        std::fs::write(root.join("new.txt"), "y").unwrap();
        let state = workspace(root).unwrap();
        assert!(state.changes.iter().any(|c| c.path == "nested/"));
        assert_eq!(workspace(root).unwrap().token, state.token);
    }
    #[test]
    fn staged_nested_repository_is_visible_and_can_be_unstaged_without_touching_it() {
        let temp = fixture();
        let root = temp.path();
        let nested = root.join(".worktrees");
        std::fs::create_dir(&nested).unwrap();
        git(&nested, &["init", "-q"]).unwrap();
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(&nested, &["config", key, value]).unwrap();
        }
        std::fs::write(nested.join("keep.txt"), "original\n").unwrap();
        git(&nested, &["add", "keep.txt"]).unwrap();
        git(&nested, &["commit", "-qm", "nested first"]).unwrap();
        let first = head(&nested);
        git(root, &["add", ".worktrees"]).unwrap();
        // The previous ignore-all status incorrectly called this real staged change clean.
        assert!(git(
            root,
            &["status", "--porcelain=v1", "--ignore-submodules=all"]
        )
        .unwrap()
        .is_empty());
        let state = workspace(root).unwrap();
        assert_eq!(state.changes.len(), 1);
        assert_eq!(state.changes[0].path, ".worktrees");
        assert_eq!(state.changes[0].index, 'A');
        assert_eq!(state.changes[0].worktree, ' ');
        index_action(
            root,
            IndexAction::Unstage,
            Some(".worktrees"),
            "",
            &state.token,
        )
        .unwrap();
        assert_eq!(head(&nested), first);
        assert_eq!(
            std::fs::read_to_string(nested.join("keep.txt")).unwrap(),
            "original\n"
        );
        assert_eq!(workspace(root).unwrap().changes[0].path, ".worktrees/");

        git(root, &["add", ".worktrees"]).unwrap();
        git(root, &["commit", "-qm", "record nested repository"]).unwrap();
        std::fs::write(nested.join("keep.txt"), "next\n").unwrap();
        git(&nested, &["commit", "-qam", "nested second"]).unwrap();
        let second = head(&nested);
        std::fs::write(nested.join("keep.txt"), "local edit\n").unwrap();
        std::fs::write(nested.join("untracked.txt"), "untracked\n").unwrap();
        // A different module checkout and local files stay outside the parent's changes.
        assert!(workspace(root).unwrap().changes.is_empty());
        git(root, &["add", ".worktrees"]).unwrap();
        let state = workspace(root).unwrap();
        assert_eq!(state.changes.len(), 1);
        assert_eq!(state.changes[0].index, 'M');
        assert_eq!(state.changes[0].worktree, ' ');
        // Last-gitlink deletion must remain visible even without .gitmodules.
        git(root, &["update-index", "--force-remove", ".worktrees"]).unwrap();
        let state = workspace(root).unwrap();
        assert_eq!(state.changes.len(), 2);
        assert!(state
            .changes
            .iter()
            .any(|change| change.path == ".worktrees" && change.index == 'D'));
        assert!(state
            .changes
            .iter()
            .any(|change| change.path == ".worktrees/" && change.index == '?'));
        index_action(
            root,
            IndexAction::Unstage,
            Some(".worktrees"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(workspace(root).unwrap().changes.is_empty());
        assert_eq!(head(&nested), second);
        assert_eq!(
            std::fs::read_to_string(nested.join("keep.txt")).unwrap(),
            "local edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(nested.join("untracked.txt")).unwrap(),
            "untracked\n"
        );
    }

    #[test]
    fn commits_in_a_repository_with_a_submodule_without_staging_its_pointer() {
        let temp = fixture();
        let root = temp.path();
        let sub = tempfile::tempdir().unwrap();
        let s = sub.path();
        git(s, &["init", "-q"]).unwrap();
        git(s, &["config", "user.email", "t@t"]).unwrap();
        git(s, &["config", "user.name", "t"]).unwrap();
        git(s, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(s.join("f"), "1").unwrap();
        git(s, &["add", "f"]).unwrap();
        git(s, &["commit", "-q", "-m", "init"]).unwrap();
        git(
            root,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &s.to_string_lossy(),
                "vendor",
            ],
        )
        .unwrap();
        git(&root.join("vendor"), &["config", "core.autocrlf", "false"]).unwrap();
        git(root, &["commit", "-q", "-m", "add submodule"]).unwrap();
        std::fs::write(s.join("f"), "2").unwrap();
        git(s, &["commit", "-q", "-am", "next"]).unwrap();
        git(&root.join("vendor"), &["pull", "-q"]).unwrap();
        std::fs::write(root.join("new.txt"), "x").unwrap();
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::StageAll, None, "", &state.token).unwrap();
        let staged = git(root, &["diff", "--cached", "--name-only"]).unwrap();
        assert_eq!(staged.trim(), "new.txt");
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::Commit, None, "with sub", &state.token).unwrap();
        // A staged submodule pointer is visible and can be committed on its own.
        git(root, &["add", "vendor"]).unwrap();
        let state = workspace(root).unwrap();
        assert_eq!(state.changes.len(), 1);
        assert_eq!(state.changes[0].path, "vendor");
        assert_eq!(state.changes[0].index, 'M');
        assert_eq!(state.changes[0].worktree, ' ');
        index_action(root, IndexAction::Commit, None, "bump sub", &state.token).unwrap();
        let last = git(root, &["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(last.trim(), "bump sub");
    }
    #[cfg(unix)]
    #[test]
    fn local_exclude_refuses_a_symlinked_info_directory() {
        use std::os::unix::fs::symlink;
        let temp = fixture();
        let root = temp.path();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("exclude"), "valuable existing rules\n").unwrap();
        std::fs::rename(root.join(".git/info"), root.join(".git/original-info")).unwrap();
        symlink(external.path(), root.join(".git/info")).unwrap();
        std::fs::write(root.join("untracked.txt"), "keep").unwrap();
        let state = workspace(root).unwrap();
        assert!(index_action(
            root,
            IndexAction::Exclude,
            Some("untracked.txt"),
            "",
            &state.token
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(external.path().join("exclude")).unwrap(),
            "valuable existing rules\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("untracked.txt")).unwrap(),
            "keep"
        );
    }
    #[cfg(unix)]
    #[test]
    fn selected_staging_never_runs_repository_index_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let temp = fixture();
        let root = temp.path();
        let hook = root.join(".git/hooks/post-index-change");
        std::fs::write(
            &hook,
            "#!/bin/sh\ngit update-ref refs/heads/hook-ran HEAD\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("a.txt"), "new staged content\n").unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        let selected: Vec<_> = patch
            .lines
            .iter()
            .filter(|l| matches!(l.kind.as_str(), "added" | "deleted"))
            .map(|l| l.index)
            .collect();
        apply_selection(root, "a.txt", false, &selected, &patch.token).unwrap();
        let patch = selection_patch(root, "a.txt", true).unwrap();
        let selected: Vec<_> = patch
            .lines
            .iter()
            .filter(|l| matches!(l.kind.as_str(), "added" | "deleted"))
            .map(|l| l.index)
            .collect();
        apply_selection(root, "a.txt", true, &selected, &patch.token).unwrap();
        assert!(git(root, &["show-ref", "--verify", "refs/heads/hook-ran"]).is_err());
    }
    #[test]
    fn commit_only_staged_file_and_reject_stale_index() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.com"]).unwrap();
        std::fs::write(p.join("a.txt"), "old").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "initial"]).unwrap();
        std::fs::write(p.join("a.txt"), "new").unwrap();
        std::fs::write(p.join("[b].txt"), "untracked").unwrap();
        let initial = workspace(p).unwrap();
        assert_eq!(initial.changes.len(), 2);
        index_action(p, IndexAction::Stage, Some("[b].txt"), "", &initial.token).unwrap();
        assert!(index_action(p, IndexAction::Commit, None, "stale", &initial.token).is_err());
        let staged = workspace(p).unwrap();
        index_action(p, IndexAction::Unstage, Some("[b].txt"), "", &staged.token).unwrap();
        assert!(p.join("[b].txt").exists());
        let state = workspace(p).unwrap();
        index_action(p, IndexAction::Stage, Some("a.txt"), "", &state.token).unwrap();
        assert!(patch(p, "a.txt", true).unwrap().contains("+new"));
        let staged = workspace(p).unwrap();
        index_action(p, IndexAction::Commit, None, "selected only", &staged.token).unwrap();
        assert_eq!(git(p, &["show", "HEAD:a.txt"]).unwrap(), "new");
        assert!(git(p, &["cat-file", "-e", "HEAD:[b].txt"]).is_err());
        assert_eq!(
            std::fs::read_to_string(p.join("[b].txt")).unwrap(),
            "untracked"
        );
        git(p, &["mv", "a.txt", "renamed.txt"]).unwrap();
        assert!(patch(p, "renamed.txt", true)
            .unwrap()
            .contains("rename from a.txt"));
        std::fs::write(p.join("renamed.txt"), "renamed and edited").unwrap();
        let state = workspace(p).unwrap();
        index_action(p, IndexAction::Stage, Some("renamed.txt"), "", &state.token).unwrap();
        let state = workspace(p).unwrap();
        index_action(
            p,
            IndexAction::Unstage,
            Some("renamed.txt"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(p.join("renamed.txt").exists());
    }

    fn fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-q"]).unwrap();
        git(root, &["config", "user.name", "Test"]).unwrap();
        git(root, &["config", "user.email", "test@example.com"]).unwrap();
        git(root, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(
            root.join("a.txt"),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\ntwelve\n",
        )
        .unwrap();
        std::fs::write(root.join("other.txt"), "other\n").unwrap();
        git(root, &["add", "."]).unwrap();
        git(root, &["commit", "-qm", "initial"]).unwrap();
        temp
    }
    /// A linked worktree of the fixture, next to it; the folder is removed with the temp folder.
    fn linked(temp: &tempfile::TempDir) -> PathBuf {
        let other = temp.path().with_file_name(format!(
            "{}-linked",
            temp.path().file_name().unwrap().to_string_lossy()
        ));
        git(
            temp.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                other.to_str().unwrap(),
            ],
        )
        .unwrap();
        other
    }
    #[test]
    fn a_hunk_or_a_file_moves_to_another_worktree_and_what_does_not_fit_is_refused() {
        let temp = fixture();
        let root = temp.path();
        let other = linked(&temp);
        std::fs::write(
            root.join("a.txt"),
            "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\nTWELVE\n",
        )
        .unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        let first: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| matches!(line.text.as_str(), "-one" | "+ONE"))
            .map(|line| line.index)
            .collect();
        // The check changes nothing, and the real call needs its token.
        let found = transfer(root, &other, "a.txt", false, Some(&first), None, false).unwrap();
        assert!(!found.copy && found.lines == 2 && !found.target_changed);
        assert_eq!(
            std::fs::read_to_string(other.join("a.txt"))
                .unwrap()
                .lines()
                .next(),
            Some("one")
        );
        assert!(transfer(root, &other, "a.txt", false, Some(&first), None, true).is_err());
        transfer(
            root,
            &other,
            "a.txt",
            false,
            Some(&first),
            Some(&found.token),
            true,
        )
        .unwrap();
        let there = std::fs::read_to_string(other.join("a.txt")).unwrap();
        assert!(
            there.starts_with("ONE\ntwo\n") && there.ends_with("twelve\n"),
            "{there}"
        );
        // Nothing is staged in the target, and the source is untouched.
        assert!(git(&other, &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
        assert!(std::fs::read_to_string(root.join("a.txt"))
            .unwrap()
            .ends_with("TWELVE\n"));
        // The same lines again do not fit the file there any more: refused, nothing changes.
        let again = transfer(root, &other, "a.txt", false, Some(&first), None, false);
        assert!(again.is_err(), "{again:?}");
        // The other changed line fits, and the local change of the target is saved first.
        let last: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| matches!(line.text.as_str(), "-twelve" | "+TWELVE"))
            .map(|line| line.index)
            .collect();
        let found = transfer(root, &other, "a.txt", false, Some(&last), None, false).unwrap();
        assert!(found.target_changed);
        transfer(
            root,
            &other,
            "a.txt",
            false,
            Some(&last),
            Some(&found.token),
            true,
        )
        .unwrap();
        assert!(std::fs::read_to_string(other.join("a.txt"))
            .unwrap()
            .ends_with("TWELVE\n"));
        assert!(git(&other, &["for-each-ref", "refs/gall/recovery"])
            .unwrap()
            .contains("recovery"));
        // A stale token (the source changed after the check) is refused.
        std::fs::write(root.join("other.txt"), "one\n").unwrap();
        let found = transfer(root, &other, "other.txt", false, None, None, false).unwrap();
        std::fs::write(root.join("other.txt"), "two\n").unwrap();
        assert!(transfer(
            root,
            &other,
            "other.txt",
            false,
            None,
            Some(&found.token),
            true
        )
        .is_err());
        // The same folder, and a folder of another repository, are no targets.
        assert!(transfer(root, root, "other.txt", false, None, None, false).is_err());
        let stranger = fixture();
        assert!(transfer(root, stranger.path(), "other.txt", false, None, None, false).is_err());
        // A path that leaves the worktree is refused.
        assert!(transfer(root, &other, "../escape.txt", false, None, None, false).is_err());
        let _ = std::fs::remove_dir_all(&other);
    }
    #[test]
    fn a_new_file_is_copied_only_where_nothing_is_at_its_path() {
        let temp = fixture();
        let root = temp.path();
        let other = linked(&temp);
        std::fs::write(root.join("fresh.txt"), "new\n").unwrap();
        let found = transfer(root, &other, "fresh.txt", false, None, None, false).unwrap();
        assert!(found.copy);
        assert!(!other.join("fresh.txt").exists());
        transfer(
            root,
            &other,
            "fresh.txt",
            false,
            None,
            Some(&found.token),
            true,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(other.join("fresh.txt")).unwrap(),
            "new\n"
        );
        // It now exists there: a second copy is refused and the file there is kept.
        std::fs::write(other.join("fresh.txt"), "mine\n").unwrap();
        assert!(transfer(root, &other, "fresh.txt", false, None, None, false).is_err());
        assert_eq!(
            std::fs::read_to_string(other.join("fresh.txt")).unwrap(),
            "mine\n"
        );
        let _ = std::fs::remove_dir_all(&other);
    }
    #[test]
    fn discarding_selected_lines_keeps_the_others_and_saves_the_file() {
        let temp = fixture();
        let root = temp.path();
        let working = "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\nTWELVE\n";
        std::fs::write(root.join("a.txt"), working).unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        let first: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| {
                line.hunk == Some(0) && matches!(line.kind.as_str(), "added" | "deleted")
            })
            .map(|line| line.index)
            .collect();
        discard_selection(root, "a.txt", &first, &patch.token).unwrap();
        // The first hunk is back to the committed text, the last one is still changed, the index is untouched.
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\nTWELVE\n"
        );
        assert_eq!(
            git(root, &["show", ":a.txt"]).unwrap().lines().next(),
            Some("one")
        );
        assert_eq!(
            git(
                root,
                &["show", &format!("{}:a.txt", only(&recovery_ids(root)))]
            )
            .unwrap(),
            working
        );
        // The same diff cannot be discarded twice.
        assert!(discard_selection(root, "a.txt", &first, &patch.token).is_err());
    }
    #[test]
    fn selected_lines_stage_and_unstage_without_worktree_writes() {
        let temp = fixture();
        let root = temp.path();
        let working = "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\nTWELVE\n";
        std::fs::write(root.join("a.txt"), working).unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        assert!(patch.selectable);
        let first: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| {
                line.hunk == Some(0) && matches!(line.kind.as_str(), "added" | "deleted")
            })
            .map(|line| line.index)
            .collect();
        apply_selection(root, "a.txt", false, &first, &patch.token).unwrap();
        assert!(git(root, &["show", ":a.txt"]).unwrap().starts_with("ONE\n"));
        assert!(git(root, &["show", ":a.txt"])
            .unwrap()
            .ends_with("twelve\n"));
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            working
        );
        let patch = selection_patch(root, "a.txt", true).unwrap();
        let plus = patch
            .lines
            .iter()
            .find(|line| line.text == "+ONE")
            .unwrap()
            .index;
        apply_selection(root, "a.txt", true, &[plus], &patch.token).unwrap();
        assert!(!git(root, &["show", ":a.txt"]).unwrap().contains("ONE"));
        assert!(!git(root, &["show", ":a.txt"]).unwrap().starts_with("one\n"));
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            working
        );
        assert!(apply_selection(root, "a.txt", true, &[plus], &patch.token).is_err());
    }
    #[test]
    fn selected_hunks_preserve_offsets_across_additions_and_deletions() {
        let temp = fixture();
        let root = temp.path();
        let base = std::fs::read_to_string(root.join("a.txt")).unwrap();
        let work = base
            .replace("one\n", "one\nextra\n")
            .replace("twelve\n", "TWELVE\n");
        std::fs::write(root.join("a.txt"), &work).unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        let last: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| {
                line.hunk == Some(1) && matches!(line.kind.as_str(), "added" | "deleted")
            })
            .map(|line| line.index)
            .collect();
        assert!(!last.is_empty());
        apply_selection(root, "a.txt", false, &last, &patch.token).unwrap();
        let staged = git(root, &["show", ":a.txt"]).unwrap();
        assert!(!staged.contains("extra"));
        assert!(staged.ends_with("TWELVE\n"));
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::StageAll, None, "", &state.token).unwrap();
        let patch = selection_patch(root, "a.txt", true).unwrap();
        let selected: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| {
                line.hunk == Some(1) && matches!(line.kind.as_str(), "added" | "deleted")
            })
            .map(|line| line.index)
            .collect();
        apply_selection(root, "a.txt", true, &selected, &patch.token).unwrap();
        let staged = git(root, &["show", ":a.txt"]).unwrap();
        assert!(staged.contains("extra"));
        assert!(staged.ends_with("twelve\n"));
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), work);
    }
    #[test]
    fn stale_working_content_and_unsupported_partial_changes_are_rejected() {
        let temp = fixture();
        let root = temp.path();
        std::fs::write(root.join("a.txt"), "new\n").unwrap();
        let state = workspace(root).unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        let line = patch
            .lines
            .iter()
            .find(|l| l.kind == "added")
            .unwrap()
            .index;
        std::fs::write(root.join("a.txt"), "newer\n").unwrap();
        assert!(index_action(root, IndexAction::Stage, Some("a.txt"), "", &state.token).is_err());
        assert!(apply_selection(root, "a.txt", false, &[line], &patch.token).is_err());
        std::fs::write(root.join("a.txt"), b"\0binary").unwrap();
        assert!(!selection_patch(root, "a.txt", false).unwrap().selectable);
        std::fs::write(root.join("a.txt"), "no newline").unwrap();
        assert!(!selection_patch(root, "a.txt", false).unwrap().selectable);
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        assert!(!selection_patch(root, "new.txt", false).unwrap().selectable);
    }
    #[test]
    fn all_index_operations_and_amend_retain_working_files_and_old_commit() {
        let temp = fixture();
        let root = temp.path();
        let original = head(root);
        std::fs::write(root.join("a.txt"), "changed\n").unwrap();
        std::fs::write(root.join("[new].txt"), "untracked\n").unwrap();
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::StageAll, None, "", &state.token).unwrap();
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::UnstageAll, None, "", &state.token).unwrap();
        assert!(git(root, &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "changed\n"
        );
        assert!(root.join("[new].txt").exists());
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::Stage, Some("a.txt"), "", &state.token).unwrap();
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::Amend, None, "amended", &state.token).unwrap();
        assert_eq!(git(root, &["show", "HEAD:a.txt"]).unwrap(), "changed\n");
        // A finished amend leaves no recovery reference behind (it would clutter other graphs).
        assert!(git(
            root,
            &[
                "show-ref",
                "--verify",
                &format!("refs/gall/recovery/{original}")
            ]
        )
        .is_err());
        assert!(root.join("[new].txt").exists());
    }
    fn recovery_ids(root: &Path) -> std::collections::BTreeSet<String> {
        git(
            root,
            &[
                "for-each-ref",
                "--format=%(objectname)",
                "refs/gall/recovery",
            ],
        )
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
    }
    fn only(ids: &std::collections::BTreeSet<String>) -> String {
        assert_eq!(ids.len(), 1, "{ids:?}");
        ids.iter().next().unwrap().clone()
    }
    #[test]
    fn targeted_discard_keeps_staged_content_and_unrelated_changes_recoverable() {
        let temp = fixture();
        let root = temp.path();
        std::fs::write(root.join("a.txt"), "staged\n").unwrap();
        git(root, &["add", "a.txt"]).unwrap();
        std::fs::write(root.join("a.txt"), "unstaged\n").unwrap();
        std::fs::write(root.join("other.txt"), "unrelated\n").unwrap();
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::Discard, Some("a.txt"), "", &state.token).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "staged\n"
        );
        assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), "staged\n");
        assert_eq!(
            std::fs::read_to_string(root.join("other.txt")).unwrap(),
            "unrelated\n"
        );
        assert_eq!(
            git(
                root,
                &["show", &format!("{}:a.txt", only(&recovery_ids(root)))]
            )
            .unwrap(),
            "unstaged\n"
        );
        let first_ids = recovery_ids(root);
        assert!(git(root, &["stash", "list"]).unwrap().is_empty());
        std::fs::write(root.join("[new].txt"), "recover me\n").unwrap();
        let state = workspace(root).unwrap();
        index_action(
            root,
            IndexAction::Discard,
            Some("[new].txt"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(!root.join("[new].txt").exists());
        assert_eq!(
            git(
                root,
                &[
                    "show",
                    &format!(
                        "{}^3:[new].txt",
                        only(&recovery_ids(root).difference(&first_ids).cloned().collect())
                    )
                ]
            )
            .unwrap(),
            "recover me\n"
        );
        assert!(git(root, &["stash", "list"]).unwrap().is_empty());
        assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), "staged\n");
        assert_eq!(
            std::fs::read_to_string(root.join("other.txt")).unwrap(),
            "unrelated\n"
        );
    }
    #[test]
    fn ignore_rules_are_literal_and_append_without_replacing_existing_rules() {
        let temp = fixture();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "existing").unwrap();
        std::fs::write(root.join("[file] #!.txt"), "ignored\n").unwrap();
        std::fs::write(root.join("f #!.txt"), "visible\n").unwrap();
        let state = workspace(root).unwrap();
        index_action(
            root,
            IndexAction::Ignore,
            Some("[file] #!.txt"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(std::fs::read_to_string(root.join(".gitignore"))
            .unwrap()
            .starts_with("existing\n"));
        assert!(git(root, &["check-ignore", "--", "[file] #!.txt"]).is_ok());
        assert!(git(root, &["check-ignore", "--", "f #!.txt"]).is_err());
        let state = workspace(root).unwrap();
        index_action(
            root,
            IndexAction::Exclude,
            Some("f #!.txt"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(git(root, &["check-ignore", "--", "f #!.txt"]).is_ok());
        assert!(root.join("f #!.txt").exists());
    }
    #[test]
    fn unborn_repository_can_stage_unstage_and_create_first_commit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-q"]).unwrap();
        git(root, &["config", "user.name", "Test"]).unwrap();
        git(root, &["config", "user.email", "test@example.com"]).unwrap();
        std::fs::write(root.join("first.txt"), "first\n").unwrap();
        let state = workspace(root).unwrap();
        assert!(state.head.is_empty());
        index_action(root, IndexAction::StageAll, None, "", &state.token).unwrap();
        let state = workspace(root).unwrap();
        index_action(
            root,
            IndexAction::Unstage,
            Some("first.txt"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(root.join("first.txt").exists());
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::StageAll, None, "", &state.token).unwrap();
        let state = workspace(root).unwrap();
        index_action(
            root,
            IndexAction::Commit,
            None,
            "first commit",
            &state.token,
        )
        .unwrap();
        assert!(!head(root).is_empty());
    }
    #[test]
    fn individual_replacement_lines_can_be_staged_and_unstaged_without_context() {
        for staged in [false, true] {
            for choose_addition in [false, true] {
                let temp = fixture();
                let root = temp.path();
                std::fs::write(root.join("a.txt"), "old\n").unwrap();
                git(root, &["add", "a.txt"]).unwrap();
                git(root, &["commit", "-qm", "single line"]).unwrap();
                std::fs::write(root.join("a.txt"), "new\n").unwrap();
                if staged {
                    git(root, &["add", "a.txt"]).unwrap();
                }
                let patch = selection_patch(root, "a.txt", staged).unwrap();
                let selected = patch
                    .lines
                    .iter()
                    .find(|line| line.kind == if choose_addition { "added" } else { "deleted" })
                    .unwrap()
                    .index;
                apply_selection(root, "a.txt", staged, &[selected], &patch.token).unwrap();
                let expected = if staged == choose_addition {
                    ""
                } else {
                    "old\nnew\n"
                };
                assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), expected);
                assert_eq!(
                    std::fs::read_to_string(root.join("a.txt")).unwrap(),
                    "new\n"
                );
            }
        }
    }
    #[test]
    fn partial_staging_preserves_crlf_content_bytes() {
        let temp = fixture();
        let root = temp.path();
        git(root, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(root.join("a.txt"), "old\r\nkeep\r\n").unwrap();
        git(root, &["add", "a.txt"]).unwrap();
        git(root, &["commit", "-qm", "crlf"]).unwrap();
        std::fs::write(root.join("a.txt"), "new\r\nkeep\r\n").unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        let selected: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| matches!(line.kind.as_str(), "added" | "deleted"))
            .map(|line| line.index)
            .collect();
        apply_selection(root, "a.txt", false, &selected, &patch.token).unwrap();
        assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), "new\r\nkeep\r\n");
    }
    #[test]
    fn partial_index_actions_use_canonical_filtered_lines_and_reject_a_stale_selection() {
        let temp = fixture();
        let root = temp.path();
        std::fs::write(root.join(".gitattributes"), "a.txt filter=roundtrip\n").unwrap();
        std::fs::write(root.join("a.txt"), "repository old\nkeep\n").unwrap();
        git(root, &["add", "--", ".gitattributes", "a.txt"]).unwrap();
        git(root, &["commit", "-qm", "Filtered base"]).unwrap();
        for (key, value) in [
            ("filter.roundtrip.clean", "sed 's/working/repository/g'"),
            ("filter.roundtrip.smudge", "sed 's/repository/working/g'"),
            ("filter.roundtrip.required", "true"),
        ] {
            git(root, &["config", key, value]).unwrap();
        }
        let working = "working new\nkeep\n";
        std::fs::write(root.join("a.txt"), working).unwrap();
        let select = |patch: &SelectionPatch| {
            patch
                .lines
                .iter()
                .filter(|line| matches!(line.kind.as_str(), "added" | "deleted"))
                .map(|line| line.index)
                .collect::<Vec<_>>()
        };
        let pending = selection_patch(root, "a.txt", false).unwrap();
        assert!(pending.text.contains("+repository new"));
        apply_selection(root, "a.txt", false, &select(&pending), &pending.token).unwrap();
        assert_eq!(
            git(root, &["show", ":a.txt"]).unwrap(),
            "repository new\nkeep\n"
        );
        let staged = selection_patch(root, "a.txt", true).unwrap();
        apply_selection(root, "a.txt", true, &select(&staged), &staged.token).unwrap();
        assert_eq!(
            git(root, &["show", ":a.txt"]).unwrap(),
            "repository old\nkeep\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            working
        );
        let pending = selection_patch(root, "a.txt", false).unwrap();
        let newer = "working newer edit\nkeep\n";
        std::fs::write(root.join("a.txt"), newer).unwrap();
        assert!(apply_selection(root, "a.txt", false, &select(&pending), &pending.token).is_err());
        assert_eq!(
            git(root, &["show", ":a.txt"]).unwrap(),
            "repository old\nkeep\n"
        );
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), newer);
    }

    #[test]
    fn selected_lines_stage_into_and_unstage_out_of_an_empty_file() {
        let temp = fixture();
        let root = temp.path();
        std::fs::write(root.join("a.txt"), "").unwrap();
        git(root, &["add", "a.txt"]).unwrap();
        git(root, &["commit", "-qm", "empty"]).unwrap();
        // An empty tracked file gains lines: `@@ -0,0 +1,2 @@`.
        std::fs::write(root.join("a.txt"), "first\nsecond\n").unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        assert!(patch.selectable);
        let second = patch
            .lines
            .iter()
            .find(|line| line.text == "+second")
            .unwrap()
            .index;
        apply_selection(root, "a.txt", false, &[second], &patch.token).unwrap();
        assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), "second\n");
        // Unstaging it again leaves the index empty: the new side has no lines.
        let patch = selection_patch(root, "a.txt", true).unwrap();
        let added = patch
            .lines
            .iter()
            .find(|line| line.kind == "added")
            .unwrap()
            .index;
        apply_selection(root, "a.txt", true, &[added], &patch.token).unwrap();
        assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), "");

        // A staged change that empties a file: `@@ -1,2 +0,0 @@`, one line unstaged.
        std::fs::write(root.join("a.txt"), "first\nsecond\n").unwrap();
        git(root, &["add", "a.txt"]).unwrap();
        git(root, &["commit", "-qm", "two lines"]).unwrap();
        std::fs::write(root.join("a.txt"), "").unwrap();
        git(root, &["add", "a.txt"]).unwrap();
        let patch = selection_patch(root, "a.txt", true).unwrap();
        assert!(patch.selectable);
        let first = patch
            .lines
            .iter()
            .find(|line| line.text == "-first")
            .unwrap()
            .index;
        apply_selection(root, "a.txt", true, &[first], &patch.token).unwrap();
        assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), "first\n");
    }
}
