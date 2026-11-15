//! Repository history and explicit revision switching.
use crate::{CoreError, Result};
use serde::Serialize;
use std::path::Path;
use std::process::Stdio;

/// Whether `config_lines` (`git config --get-regexp` output, `key value` per
/// line) define a content filter that would run a program on checkout or
/// staging. The standard Git LFS filter is trusted and does not count.
///
/// The LFS exception is a prefix check (`filter.lfs.*` whose value starts with
/// `git-lfs `), which is what `git lfs install` writes. It is not a security
/// boundary: anyone who can write the user's Git configuration can already run
/// programs, so this only avoids refusing ordinary LFS repositories.
#[cfg(test)]
fn filters_block(config_lines: &str) -> bool {
    config_lines.lines().any(|line| {
        let (key, value) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        filter_blocks(key, value)
    })
}

/// One filter setting: everything but the standard Git LFS filter blocks.
fn filter_blocks(key: &str, value: &str) -> bool {
    !(key.starts_with("filter.lfs.") && value.trim().starts_with("git-lfs "))
}

/// True when the repository's effective configuration (including global and
/// system files) has a content filter other than standard Git LFS.
pub(super) fn has_content_filters(path: &Path) -> bool {
    super::gitfacts::filters(path)
        .iter()
        .any(|(key, value)| filter_blocks(key, value))
}

/// Queries that only read the repository; any other command may change it.
pub(super) fn reads_only(args: &[&str]) -> bool {
    match args {
        ["stash", "list", ..] | ["remote", "-v"] => true,
        ["config", rest @ ..] => rest
            .iter()
            .any(|a| matches!(*a, "--get" | "--get-regexp" | "--list")),
        // With a second name `symbolic-ref` writes the reference.
        ["symbolic-ref", rest @ ..] => rest.iter().filter(|a| !a.starts_with('-')).count() == 1,
        [command, ..] => matches!(
            *command,
            "rev-parse"
                | "for-each-ref"
                | "log"
                | "rev-list"
                | "status"
                | "ls-files"
                | "ls-tree"
                | "cat-file"
        ),
        [] => false,
    }
}

/// Parallel checkout for Git for Windows on a Windows folder: writing the files is most of
/// a branch switch there, and Git writes them one after the other by default (a switch that
/// changes 800 files of a 50 000-file work tree took 400 ms instead of 310 ms). Git uses
/// the workers only for 100 files or more (`checkout.thresholdForParallelism`), and the
/// files written are the same; commands that write no files ignore the setting. Not for
/// other systems, where it was not measured.
fn checkout_workers(root: &Path) -> Option<String> {
    (cfg!(windows) && super::gitexec::place(root).is_none()).then(|| {
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
        format!("checkout.workers={workers}")
    })
}

pub(super) fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = super::gitexec::command(root, super::gitstatus::program());
    cmd.args(["-c", "core.fsmonitor=false", "-c"])
        .arg(format!(
            "core.hooksPath={}",
            super::gitexec::null_device(root)
        ))
        .args([
            "-c",
            "commit.gpgSign=false",
            "-c",
            "log.showSignature=false",
        ]);
    if let Some(workers) = checkout_workers(root) {
        cmd.arg("-c").arg(workers);
    }
    cmd.arg("-C")
        .arg(super::gitexec::dir_arg(root))
        .args(args)
        .stdin(Stdio::null())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    super::gitexec::forward_env(&mut cmd);
    let out = match super::gitexec::run(&cmd, std::time::Duration::from_secs(120)) {
        Some(out) => out,
        None => cmd.output(),
    }
    .map_err(|e| CoreError::Other(e.to_string()))?;
    if !out.status.success() {
        let message = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(CoreError::Other(if message.is_empty() {
            format!(
                "Git command failed ({}): git {}",
                out.status,
                args.join(" ")
            )
        } else {
            message
        }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
/// Waits for a scoped worker thread. A panic in the worker becomes an error.
pub(super) fn joined<T>(handle: std::thread::ScopedJoinHandle<'_, T>) -> Result<T> {
    handle
        .join()
        .map_err(|_| CoreError::Other("A Git worker thread stopped unexpectedly".into()))
}

/// Work tree status in porcelain-v1 `-z` form. Staged submodule pointers are shown,
/// while submodule checkout drift, edits and untracked files are ignored.
///
/// Git does the two halves of a status one after the other: it compares every tracked
/// file with the index, then walks the work tree for files it does not know. In a large
/// repository each half takes about as long as the other, so here they are two processes
/// side by side (`status` without untracked files and `ls-files --others`), joined in
/// Git's order: tracked changes first, then the untracked files.
///
/// Not with Git for Windows on a Windows folder: there `status` reads folders through its
/// file system cache (`core.fscache`) and `ls-files` does not, so one `status` that walks
/// the work tree as well takes about a third of the time `ls-files --others` alone does.
pub(super) fn status_z(root: &Path) -> Result<String> {
    if cfg!(windows) && super::gitexec::place(root).is_none() {
        return status_v1(&git(
            root,
            &[
                "status",
                "--porcelain=v2",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=dirty",
            ],
        )?);
    }
    std::thread::scope(|scope| {
        let untracked =
            scope.spawn(|| git(root, &["ls-files", "--others", "--exclude-standard", "-z"]));
        let tracked = git(
            root,
            &[
                "status",
                "--porcelain=v2",
                "-z",
                "--untracked-files=no",
                "--ignore-submodules=dirty",
            ],
        );
        let mut status = status_v1(&tracked?)?;
        for name in joined(untracked)??.split_terminator('\0') {
            status.push_str("?? ");
            status.push_str(name);
            status.push('\0');
        }
        Ok(status)
    })
}

/// Porcelain v2 includes each path's modes, so a worktree-only gitlink change can
/// be ignored without another Git query or a scan of the module's files. Unlike
/// `--ignore-submodules=all`, this keeps staged additions, updates and removals.
fn status_v1(raw: &str) -> Result<String> {
    let invalid = || CoreError::Other("Invalid Git status record".into());
    let mut out = String::with_capacity(raw.len());
    let mut records = raw.split_terminator('\0');
    while let Some(record) = records.next() {
        let (code, name, original, gitlink) = match record.as_bytes().first() {
            // Optional headers (for example status.showStash) do not describe paths.
            // Ignore unknown headers too, as porcelain v2 permits future extensions.
            Some(b'#') => continue,
            Some(b'1' | b'2') => {
                let renamed = record.starts_with('2');
                let fields: Vec<_> = record.splitn(if renamed { 10 } else { 9 }, ' ').collect();
                if fields.len() != if renamed { 10 } else { 9 } {
                    return Err(invalid());
                }
                let original = if renamed {
                    Some(records.next().ok_or_else(invalid)?)
                } else {
                    None
                };
                // After a staged type change from a module to a regular file, retain
                // that file's worktree edits. A removed module has a zero index mode.
                let gitlink =
                    fields[4] == "160000" || (fields[3] == "160000" && fields[4] == "000000");
                (fields[1], fields[fields.len() - 1], original, gitlink)
            }
            Some(b'u') => {
                let fields: Vec<_> = record.splitn(11, ' ').collect();
                if fields.len() != 11 {
                    return Err(invalid());
                }
                (fields[1], fields[10], None, false)
            }
            Some(b'?' | b'!') if record.as_bytes().get(1) == Some(&b' ') && record.len() > 2 => {
                out.push(record.as_bytes()[0] as char);
                out.push(record.as_bytes()[0] as char);
                out.push(' ');
                out.push_str(&record[2..]);
                out.push('\0');
                continue;
            }
            _ => return Err(invalid()),
        };
        let code = code.as_bytes();
        if code.len() != 2 || !code.is_ascii() || name.is_empty() {
            return Err(invalid());
        }
        if gitlink && code[0] == b'.' {
            continue;
        }
        out.push(if code[0] == b'.' {
            ' '
        } else {
            code[0] as char
        });
        out.push(if gitlink || code[1] == b'.' {
            ' '
        } else {
            code[1] as char
        });
        out.push(' ');
        out.push_str(name);
        out.push('\0');
        if let Some(original) = original {
            out.push_str(original);
            out.push('\0');
        }
    }
    Ok(out)
}

/// One record of `status --porcelain=v1 -z`.
pub(super) struct StatusEntry<'a> {
    pub code: &'a str,
    pub name: &'a str,
    /// The original path of a rename or copy.
    pub original: Option<&'a str>,
}

pub(super) fn status_entries(z: &str) -> Vec<StatusEntry<'_>> {
    let mut entries = Vec::new();
    let mut records = z.split('\0');
    while let Some(record) = records.next() {
        let Some((code, name)) = record.split_at_checked(3) else {
            continue;
        };
        let renamed = matches!(code.as_bytes()[..2], [b'R' | b'C', _] | [_, b'R' | b'C']);
        let original = if renamed { records.next() } else { None };
        entries.push(StatusEntry {
            code,
            name,
            original,
        });
    }
    entries
}

/// Whether Git escapes non-ASCII bytes in printed paths (`core.quotePath`, on by default).
pub(super) fn quote_non_ascii(root: &Path) -> bool {
    git(root, &["config", "--type=bool", "--get", "core.quotePath"])
        .map_or(true, |v| v.trim() != "false")
}

/// A path as `git status --porcelain` prints it: wrapped in double quotes, with C escapes and
/// octal bytes, when it has a space, a control character, a quote, a backslash or non-ASCII
/// bytes (Git's default `core.quotePath`); otherwise as is.
fn quote_path(name: &str, escape_non_ascii: bool) -> String {
    let plain = name.bytes().all(|b| {
        b > b' ' && b != 0x7f && b != b'"' && b != b'\\' && (b < 0x80 || !escape_non_ascii)
    });
    if plain {
        return name.to_owned();
    }
    let mut out = String::from("\"");
    for b in name.bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            0x0b => out.push_str("\\v"),
            0x0c => out.push_str("\\f"),
            b'\r' => out.push_str("\\r"),
            b' '..=b'~' => out.push(char::from(b)),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push('"');
    out
}

/// The classic `status --porcelain` text (`XY path`, `XY old -> new`) built from a `-z` status,
/// one line per entry, with names quoted the way Git quotes them.
pub(super) fn changes_text(z: &str, escape_non_ascii: bool) -> String {
    let mut text = String::new();
    for entry in status_entries(z) {
        text.push_str(entry.code);
        if let Some(original) = entry.original {
            text.push_str(&quote_path(original, escape_non_ascii));
            text.push_str(" -> ");
        }
        text.push_str(&quote_path(entry.name, escape_non_ascii));
        text.push('\n');
    }
    text
}

pub(super) fn root(path: &Path) -> Result<std::path::PathBuf> {
    super::diff::git_root(path)
        .ok_or_else(|| CoreError::Other("not inside a Git repository".into()))
}
#[derive(Serialize)]
pub struct Graph {
    pub history: String,
    pub branches: Vec<String>,
    pub head: String,
    pub changes: String,
    #[serde(rename = "readOnlyReason")]
    pub read_only_reason: Option<String>,
}
pub fn graph(path: &Path) -> Result<Graph> {
    graph_impl(path, false, false)
}

pub fn comparison_graph(path: &Path) -> Result<Graph> {
    graph_impl(path, true, false)
}

pub fn history_graph(path: &Path, comparison: bool, reflog: bool) -> Result<Graph> {
    graph_impl(path, comparison, reflog)
}

fn graph_impl(path: &Path, comparison: bool, reflog: bool) -> Result<Graph> {
    query_history(path, comparison, reflog, 200, None)
}

/// Read history without changing HEAD, the index or working files.
pub fn query_history(
    path: &Path,
    comparison: bool,
    reflog: bool,
    limit: usize,
    reference: Option<&str>,
) -> Result<Graph> {
    let root = &root(path)?;
    std::thread::scope(|scope| {
        // These do not depend on the history query: run them while it works. Only the status
        // has to wait for the filter check, because it must not run when there are filters.
        let branches = scope.spawn(|| branches(root));
        let side = scope.spawn(|| -> Result<(bool, String)> {
            let filters = has_content_filters(root);
            let changes = if comparison || filters {
                String::new()
            } else {
                changes_text(&status_z(root)?, quote_non_ascii(root))
            };
            Ok((filters, changes))
        });
        let mut graph = history_part(root, reflog, limit, reference, None)?;
        graph.branches = joined(branches)??;
        let (filters, changes) = joined(side)??;
        graph.read_only_reason = read_only_reason(filters);
        graph.changes = changes;
        Ok(graph)
    })
}

/// Why working-tree operations are refused in a repository with content filters.
pub(super) fn read_only_reason(filters: bool) -> Option<String> {
    filters.then(|| "History is read-only: content filters require the terminal for working-tree operations.".into())
}

/// Names of the local branches, as Git shortens them.
pub(super) fn branches(root: &Path) -> Result<Vec<String>> {
    if let Some(names) = super::gitfacts::branches(root) {
        return Ok(names);
    }
    let names = git(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    )?;
    Ok(names.lines().map(str::to_owned).collect())
}

/// The history itself (`log`, HEAD): everything except the branch list, the changes and the
/// read-only reason. `head` is the HEAD commit when the caller has just read it (empty when
/// there is none); otherwise it is read here.
pub(super) fn history_part(
    root: &Path,
    reflog: bool,
    limit: usize,
    reference: Option<&str>,
    head: Option<&str>,
) -> Result<Graph> {
    super::gitfacts::keep_commit_graph(root);
    let reference = reference.filter(|s| !s.is_empty());
    if let Some(reference) = reference {
        let known = git(root, &["for-each-ref", "--format=%(refname)"])?;
        if !known.lines().any(|s| s == reference) {
            return Err(CoreError::Other(
                "The selected reference no longer exists".into(),
            ));
        }
    }
    let count = format!("--max-count={}", limit.clamp(1, 20_000));
    let mut args = vec![
        "log",
        "--decorate-refs=*",
        "--topo-order",
        "--no-color",
        "--no-show-signature",
        &count,
        "--format=%H%x00%P%x00%D%x00%s%x00%an%x00%aI",
    ];
    if let Some(reference) = reference {
        args.push(reference);
    } else if reflog {
        args.extend(["--all", "--reflog"]);
    } else {
        // Saved pre-rewrite commits stay hidden unless Reflog is enabled.
        args.extend(["--exclude=refs/gall/recovery/*", "--all"]);
        args.push("--decorate-refs-exclude=refs/gall/recovery/*");
    }
    let (log, head) = std::thread::scope(|scope| {
        // HEAD is only needed once the log is there: read it while the log runs.
        let read = head.is_none().then(|| {
            scope.spawn(|| {
                git(root, &["rev-parse", "--verify", "HEAD"])
                    .map(|s| s.trim().to_owned())
                    .unwrap_or_default()
            })
        });
        let log = git(root, &args);
        let head = match read {
            Some(read) => joined(read),
            None => Ok(head.unwrap_or_default().to_owned()),
        };
        (log, head)
    });
    let head = head?;
    // A repository without commits and references has no history; there `log` may fail.
    let mut history =
        if head.is_empty() && git(root, &["for-each-ref", "--format=%(refname)"])?.is_empty() {
            String::new()
        } else {
            log?
        };
    if !head.is_empty()
        && reference.is_none()
        && !history
            .lines()
            .any(|line| line.split('\0').next() == Some(head.as_str()))
    {
        history.push_str(&git(
            root,
            &[
                "log",
                "--no-walk",
                "--decorate-refs=*",
                "--no-show-signature",
                "--format=%H%x00%P%x00%D%x00%s%x00%an%x00%aI",
                "HEAD",
            ],
        )?);
    }
    Ok(Graph {
        read_only_reason: None,
        history,
        branches: Vec::new(),
        head,
        changes: String::new(),
    })
}
/// Retain a detached commit even after its HEAD reflog expires.
/// HEAD's file says whether it is detached; Git is asked where the file cannot answer.
pub(super) fn retain_detached(root: &Path) -> Result<()> {
    let detached = match super::gitfacts::detached_head(root) {
        Some(state) => state,
        None if git(root, &["symbolic-ref", "-q", "HEAD"]).is_ok() => None,
        None => Some(git(root, &["rev-parse", "--verify", "HEAD"])?),
    };
    if let Some(head) = detached {
        let head = head.trim();
        git(
            root,
            &[
                "update-ref",
                "--no-deref",
                &format!("refs/gall/recovery/{head}"),
                head,
            ],
        )?;
    }
    Ok(())
}

/// Remove the recovery reference of a history rewrite that finished successfully, so it no
/// longer clutters other tools' graphs. Best effort: a leftover reference is harmless.
pub(super) fn drop_recovery(root: &Path, id: &str) {
    let _ = git(
        root,
        &["update-ref", "-d", &format!("refs/gall/recovery/{id}")],
    );
}

/// Keep a commit (such as one made by `git stash create`) reachable under
/// `refs/gall/recovery/<id>` with a reflog entry that names the reason. Unlike the stash
/// list, these references do not show up in other Git tools' stash or branch views.
pub(super) fn retain_recovery(root: &Path, id: &str, message: &str) -> Result<()> {
    git(
        root,
        &[
            "update-ref",
            "--no-deref",
            "--create-reflog",
            "-m",
            message,
            &format!("refs/gall/recovery/{id}"),
            id,
        ],
    )?;
    Ok(())
}

/// Remove local changes with `git stash push` options but keep them only as a recovery
/// reference, not as a visible stash entry. `extra` is appended (for example a pathspec).
pub(super) fn push_recovery(root: &Path, extra: &[&str], message: &str) -> Result<()> {
    let top = |root: &Path| {
        git(root, &["rev-parse", "-q", "--verify", "refs/stash"])
            .map(|id| id.trim().to_owned())
            .unwrap_or_default()
    };
    let before = top(root);
    let mut args = vec!["stash", "push", "--include-untracked", "-m", message];
    args.extend_from_slice(extra);
    git(root, &args)?;
    let created = top(root);
    if created.is_empty() || created == before {
        return Ok(());
    }
    retain_recovery(root, &created, message)?;
    git(root, &["stash", "drop", "-q", "stash@{0}"])?;
    Ok(())
}

pub fn switch(path: &Path, target: &str, branch: bool, action: &str, message: &str) -> Result<()> {
    let _busy = super::gitfacts::mutating();
    let root = root(path)?;
    if has_content_filters(&root) {
        return Err(CoreError::Other(
            "Switching repositories with content filters is not supported yet".into(),
        ));
    }
    // Only the branch names and whether there are changes matter here; this used to load
    // the whole history graph for them.
    let (known, changes) = std::thread::scope(|scope| {
        let known = scope.spawn(|| branches(&root));
        (joined(known), status_z(&root))
    });
    let (known, changes) = (known??, changes?);
    if branch {
        if !known.iter().any(|b| b == target) {
            return Err(CoreError::Other("branch no longer exists".into()));
        }
    } else if !(target.len() == 40 || target.len() == 64)
        || !target.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(CoreError::Other("invalid commit ID".into()));
    }
    // Reject unfinished operations rather than risk partial data loss. Submodules are supported: the
    // switch never recurses into their folders (they stay as they are, like in lazygit or GitLens).
    let listed = super::gitfacts::markers(&root);
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
                let p = git(&root, &["rev-parse", "--git-path", marker])?;
                root.join(super::gitexec::host_path(&root, p.trim()))
                    .exists()
            }
        };
        if present {
            return Err(CoreError::Other(
                "Finish or abort the current Git operation first".into(),
            ));
        }
    }
    if !changes.is_empty() {
        match action {
            "stash" => {
                git(
                    &root,
                    &[
                        "stash",
                        "push",
                        "--include-untracked",
                        "-m",
                        "Gall before revision switch",
                    ],
                )?;
            }
            "discard" => push_recovery(&root, &[], "Gall recovery before discard")?,
            "commit" => {
                if message.trim().is_empty() {
                    return Err(CoreError::Other("A commit message is required".into()));
                }
                git(&root, &["add", "--all"])?;
                git(&root, &["commit", "-m", message])?;
            }
            _ => {
                return Err(CoreError::Other(
                    "Local changes require stash, commit or discard".into(),
                ))
            }
        }
    }
    // After a stash, commit or discard the work tree has to be clean now. Without one it
    // was read as clean a moment ago, and Git itself refuses to overwrite a change.
    if !changes.is_empty()
        && !git(
            &root,
            &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ],
        )?
        .is_empty()
    {
        return Err(CoreError::Other(
            "Files changed during preparation; switching cancelled".into(),
        ));
    }
    retain_detached(&root)?;
    // Never force checkout: Git protects ignored/untracked collisions and concurrent edits.
    if branch {
        git(
            &root,
            &[
                "-c",
                "submodule.recurse=false",
                "switch",
                "--no-overwrite-ignore",
                "--",
                target,
            ],
        )?;
    } else {
        git(
            &root,
            &[
                "-c",
                "submodule.recurse=false",
                "switch",
                "--detach",
                "--no-overwrite-ignore",
                target,
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn porcelain_v2_keeps_staged_gitlinks_and_literal_paths() {
        let oid = "0".repeat(40);
        let raw = format!(
            "1 A. S... 000000 160000 160000 {oid} {oid} .worktrees\0\
             1 .M SC.. 160000 160000 160000 {oid} {oid} drift only\0\
             1 MM SC.. 160000 160000 160000 {oid} {oid} staged module\0\
             1 D. S... 160000 000000 000000 {oid} {oid} removed module\0\
             1 TM N... 160000 100644 100644 {oid} {oid} regular now\0\
             2 R. N... 100644 100644 100644 {oid} {oid} R100 new é\nname\0old name\0\
             u UU N... 100644 100644 100644 100644 {oid} {oid} {oid} conflicted file\0\
             ? untracked\nfile\0"
        );
        assert_eq!(
            status_v1(&raw).unwrap(),
            "A  .worktrees\0M  staged module\0D  removed module\0TM regular now\0\
             R  new é\nname\0old name\0UU conflicted file\0?? untracked\nfile\0"
        );
    }

    #[test]
    fn porcelain_v2_ignores_optional_headers() {
        let raw = "# stash 1\0# future.extension optional metadata\0? new file\0";
        assert_eq!(status_v1(raw).unwrap(), "?? new file\0");
        assert_eq!(status_v1("# stash 1\0").unwrap(), "");
    }

    #[test]
    fn malformed_porcelain_v2_is_rejected_instead_of_hiding_changes() {
        for raw in [
            "1 M. truncated\0",
            "2 R. N... 100644 100644 100644 a b R100 new\0",
            "u UU truncated\0",
            "unexpected\0",
        ] {
            assert!(status_v1(raw).is_err(), "{raw:?}");
        }
    }

    /// The status put together from two processes is byte for byte what one
    /// `git status` prints, for every kind of entry.
    #[test]
    fn the_joined_status_equals_gits_own() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        if git(r, &["init", "-q"]).is_err() {
            eprintln!("git not installed: skipped");
            return;
        }
        for (key, value) in [
            ("user.email", "t@example.com"),
            ("user.name", "t"),
            ("core.autocrlf", "false"),
        ] {
            git(r, &["config", key, value]).unwrap();
        }
        let write = |name: &str, text: &str| {
            let file = r.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        };
        write(".gitignore", "build/\n*.o\n");
        for name in [
            "a.txt",
            "dir/b.txt",
            "dir/c.txt",
            "gone.txt",
            "old name.txt",
        ] {
            write(name, &format!("{name}\n").repeat(30));
        }
        git(r, &["add", "."]).unwrap();
        git(r, &["commit", "-qm", "init"]).unwrap();
        write("a.txt", "changed\n");
        write("dir/b.txt", "staged\n");
        git(r, &["add", "dir/b.txt"]).unwrap();
        std::fs::remove_file(r.join("gone.txt")).unwrap();
        git(r, &["mv", "old name.txt", "new name.txt"]).unwrap();
        write("added.txt", "new and staged\n");
        git(r, &["add", "added.txt"]).unwrap();
        // Untracked: a file, files in folders, odd names, an ignored file and folder, and
        // a nested repository (listed as a folder).
        write("zz untracked.txt", "u");
        write("dir/new/deep/u.txt", "u");
        write("-dash.txt", "u");
        write("build/out.bin", "ignored");
        write("dir/x.o", "ignored");
        write("nested/inner.txt", "u");
        git(&r.join("nested"), &["init", "-q"]).unwrap();
        let whole = git(
            r,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ],
        )
        .unwrap();
        assert!(whole.contains("?? nested/\0") && whole.contains("R  new name.txt\0"));
        assert_eq!(status_z(r).unwrap(), whole);
    }
    #[test]
    fn standard_lfs_filters_do_not_block_but_other_filters_do() {
        let lfs = "filter.lfs.clean git-lfs clean -- %f\n\
                   filter.lfs.smudge git-lfs smudge -- %f\n\
                   filter.lfs.process git-lfs filter-process\n";
        assert!(!filters_block(lfs));
        assert!(!filters_block(""));
        assert!(filters_block(&format!(
            "{lfs}filter.custom.clean sed s/a/b/\n"
        )));
        assert!(filters_block("filter.lfs.smudge /tmp/evil %f\n"));
        assert!(filters_block("filter.lfs.clean\n"));
    }
    #[test]
    fn repository_with_a_custom_filter_is_detected_and_lfs_is_not() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        git(p, &["config", "filter.lfs.clean", "git-lfs clean -- %f"]).unwrap();
        git(p, &["config", "filter.lfs.smudge", "git-lfs smudge -- %f"]).unwrap();
        assert!(!has_content_filters(p));
        git(p, &["config", "filter.other.smudge", "cat"]).unwrap();
        assert!(has_content_filters(p));
    }
    #[test]
    fn history_filters_and_paging_do_not_change_head_or_working_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.com"]).unwrap();
        assert!(graph(p).unwrap().head.is_empty());
        for i in 0..3 {
            std::fs::write(p.join("file"), i.to_string()).unwrap();
            git(p, &["add", "file"]).unwrap();
            git(p, &["commit", "-qm", &format!("commit {i}")]).unwrap();
            if i == 0 {
                git(p, &["branch", "older"]).unwrap();
            }
        }
        let head = graph(p).unwrap().head;
        let modified = std::fs::metadata(p.join("file"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            query_history(p, false, false, 1, None)
                .unwrap()
                .history
                .lines()
                .count(),
            1
        );
        assert_eq!(
            query_history(p, false, false, 3, None)
                .unwrap()
                .history
                .lines()
                .count(),
            3
        );
        let older = query_history(p, false, false, 200, Some("refs/heads/older")).unwrap();
        assert_eq!(older.history.lines().count(), 1);
        assert!(older.history.contains("commit 0"));
        assert!(!older.history.contains(&head));
        assert!(query_history(p, false, false, 200, Some("--all")).is_err());
        assert_eq!(graph(p).unwrap().head, head);
        assert_eq!(
            std::fs::metadata(p.join("file"))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
        assert_eq!(std::fs::read_to_string(p.join("file")).unwrap(), "2");
    }
    #[test]
    fn history_keeps_head_outside_the_display_limit() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.com"]).unwrap();
        std::fs::write(p.join("f"), "first").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "first"]).unwrap();
        let head = git(p, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        let tree = git(p, &["rev-parse", "HEAD^{tree}"])
            .unwrap()
            .trim()
            .to_owned();
        let mut tip = head.clone();
        for _ in 0..205 {
            tip = git(p, &["commit-tree", &tree, "-p", &tip, "-m", "newer"])
                .unwrap()
                .trim()
                .to_owned();
        }
        git(p, &["update-ref", "refs/heads/newer", &tip]).unwrap();
        git(p, &["checkout", "--detach", &head]).unwrap();
        let graph = graph(p).unwrap();
        assert_eq!(graph.history.lines().count(), 201);
        assert!(graph
            .history
            .lines()
            .any(|line| line.split('\0').next() == Some(head.as_str())));
    }

    #[test]
    fn detached_commit_is_visible_after_leaving_head() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.com"]).unwrap();
        std::fs::write(p.join("f"), "base").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "base"]).unwrap();
        let branch = git(p, &["symbolic-ref", "HEAD"]).unwrap().trim().to_owned();
        let base = graph(p).unwrap().head;
        git(p, &["checkout", "--detach", &base]).unwrap();
        std::fs::write(p.join("f"), "detached").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "detached saved work"]).unwrap();
        let saved = graph(p).unwrap().head;
        // Existing commits must remain discoverable via reflog even without a recovery ref.
        git(p, &["checkout", "--detach", &base]).unwrap();
        assert!(!history_graph(p, false, false)
            .unwrap()
            .history
            .contains(&saved));
        assert!(history_graph(p, false, true)
            .unwrap()
            .history
            .contains(&saved));
        git(p, &["checkout", "--detach", &saved]).unwrap();
        git(
            p,
            &[
                "symbolic-ref",
                &format!("refs/gall/recovery/{saved}"),
                &branch,
            ],
        )
        .unwrap();
        switch(p, &base, false, "clean", "").unwrap();
        assert_eq!(git(p, &["rev-parse", &branch]).unwrap().trim(), base);
        assert!(!graph(p).unwrap().history.contains(&saved));
        assert!(history_graph(p, false, true)
            .unwrap()
            .history
            .contains(&format!("refs/gall/recovery/{saved}")));
        assert_eq!(
            git(p, &["rev-parse", &format!("refs/gall/recovery/{saved}")])
                .unwrap()
                .trim(),
            saved
        );
    }

    #[test]
    fn switch_works_with_submodules_and_leaves_their_folders_alone() {
        let d = tempfile::tempdir().unwrap();
        let module = tempfile::tempdir().unwrap();
        let (p, m) = (d.path(), module.path());
        for dir in [p, m] {
            git(dir, &["init", "-q", "-b", "main"]).unwrap();
            git(dir, &["config", "user.name", "Test"]).unwrap();
            git(dir, &["config", "user.email", "test@example.com"]).unwrap();
            git(dir, &["config", "core.autocrlf", "false"]).unwrap();
        }
        std::fs::write(m.join("m.txt"), "one").unwrap();
        git(m, &["add", "."]).unwrap();
        git(m, &["commit", "-qm", "module one"]).unwrap();
        git(
            p,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &m.to_string_lossy().replace('\\', "/"),
                "sub",
            ],
        )
        .unwrap();
        git(p, &["commit", "-qm", "add submodule"]).unwrap();
        git(p, &["branch", "other"]).unwrap();
        std::fs::write(p.join("f"), "main only").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "on main"]).unwrap();
        // Untracked and edited files inside the submodule must not block or be touched.
        std::fs::write(p.join("sub").join("untracked"), "keep").unwrap();
        std::fs::write(p.join("sub").join("m.txt"), "edited").unwrap();
        switch(p, "other", true, "clean", "").unwrap();
        assert!(!p.join("f").exists());
        assert_eq!(
            std::fs::read_to_string(p.join("sub").join("untracked")).unwrap(),
            "keep"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("sub").join("m.txt")).unwrap(),
            "edited"
        );
    }

    #[test]
    fn switch_preserves_dirty_data_in_stash() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.com"]).unwrap();
        std::fs::write(p.join("f"), "first").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "first"]).unwrap();
        let first = graph(p).unwrap().head;
        git(p, &["branch", "example"]).unwrap();
        std::fs::write(p.join("f"), "dirty").unwrap();
        std::fs::write(p.join("new"), "untracked").unwrap();
        assert!(switch(p, &first, false, "clean", "").is_err());
        switch(p, &first, false, "stash", "").unwrap();
        assert_eq!(std::fs::read_to_string(p.join("f")).unwrap(), "first");
        assert!(!p.join("new").exists());
        switch(p, "example", true, "clean", "").unwrap();
        git(p, &["stash", "pop"]).unwrap();
        assert_eq!(std::fs::read_to_string(p.join("f")).unwrap(), "dirty");
        assert_eq!(std::fs::read_to_string(p.join("new")).unwrap(), "untracked");
        switch(p, &first, false, "commit", "saved").unwrap();
        assert!(git(p, &["log", "example", "-1", "--format=%s"])
            .unwrap()
            .contains("saved"));
        std::fs::write(p.join("f"), "discarded").unwrap();
        switch(p, "example", true, "discard", "").unwrap();
        assert!(git(p, &["stash", "list"]).unwrap().is_empty());
        let reflog = git(p, &["reflog", "show", "--all", "--format=%gs"]).unwrap();
        assert!(reflog.contains("Gall recovery before discard"));
        git(p, &["config", "filter.example.clean", "false"]).unwrap();
        assert!(graph(p).unwrap().read_only_reason.is_some());
        assert!(switch(p, &first, false, "clean", "").is_err());
        assert!(!comparison_graph(p).unwrap().history.is_empty());
    }
}
