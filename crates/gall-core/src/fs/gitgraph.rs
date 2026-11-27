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
pub(super) fn checkout_workers(root: &Path) -> Option<String> {
    (cfg!(windows) && super::gitexec::place(root).is_none()).then(|| {
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
        format!("checkout.workers={workers}")
    })
}

pub(super) fn git(root: &Path, args: &[&str]) -> Result<String> {
    git_with_index(root, args, None)
}

pub(super) fn git_with_index(root: &Path, args: &[&str], index: Option<&Path>) -> Result<String> {
    git_with_options(root, args, index, None)
}

/// Use a pinned tree for attribute lookup without changing the index or working tree.
pub(super) fn git_with_attr_source(root: &Path, args: &[&str], source: &str) -> Result<String> {
    git_with_options(root, args, None, Some(source))
}

fn git_with_options(
    root: &Path,
    args: &[&str],
    index: Option<&Path>,
    source: Option<&str>,
) -> Result<String> {
    let mut cmd =
        super::gitexec::command_with_options(root, super::gitstatus::program(), index, source);
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
        .env("GIT_TERMINAL_PROMPT", "0")
        // Non-interactive Git calls must never launch or fail on an inherited GIT_EDITOR
        // (GIT_EDITOR takes precedence over `-c core.editor=true`).
        .env("GIT_EDITOR", "true");
    if args.starts_with(&["stash", "push"]) {
        cmd.env("LC_ALL", "C");
    }
    super::gitexec::forward_env(&mut cmd);
    let out = if !reads_only(args) {
        if let Some(tracker) = super::gitprogress::active() {
            super::gitprogress::run_streaming_output(&mut cmd, &tracker)
        } else {
            match super::gitexec::run(&cmd, std::time::Duration::from_secs(120)) {
                Some(out) => out,
                None => cmd.output(),
            }
        }
    } else {
        match super::gitexec::run(&cmd, std::time::Duration::from_secs(120)) {
            Some(out) => out,
            None => cmd.output(),
        }
    }
    .map_err(|e| CoreError::Other(e.to_string()))?;
    if !out.status.success() {
        // Git sometimes reports failures on stdout (notably `rebase --continue`), and
        // either stream may contain information the other lacks.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let message = [stdout.trim(), stderr.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
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
    // The live stash stack, from Git's files (no process; where they do not say, the rows are read here once). A
    // scoped history keeps its own references.
    let (stack, read_rows) = if reference.is_none() {
        live_stashes(root)?
    } else {
        (Vec::new(), None)
    };
    // In Reflog mode the stash commits and their helper commits are walked too and removed afterwards, so
    // the budget is raised by what is removed (at most three commits per entry).
    let walked = if reflog {
        limit + 3 * stack.len()
    } else {
        limit
    };
    let count = format!("--max-count={}", walked.clamp(1, 20_000));
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
        args.extend([
            "--exclude=refs/gall/recovery/*",
            "--exclude=refs/gall/journal/*",
        ]);
        // A stash is shown as one row of its own (see `merge_stash_rows`), not as the commit with its helper
        // commits (index, untracked files) that `refs/stash` leads to.
        if !stack.is_empty() {
            args.push("--exclude=refs/stash");
        }
        args.push("--all");
        args.push("--decorate-refs-exclude=refs/gall/recovery/*");
        args.push("--decorate-refs-exclude=refs/gall/journal/*");
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
        // The stash rows (one process for all entries) are read while the log runs, unless already read.
        let rows =
            (read_rows.is_none() && !stack.is_empty()).then(|| scope.spawn(|| stash_rows(root)));
        let log = git(root, &args);
        let head = match read {
            Some(read) => joined(read),
            None => Ok(head.unwrap_or_default().to_owned()),
        };
        (
            log.and_then(|log| match (read_rows, rows) {
                (Some(rows), _) => Ok(merge_stash_rows(&log, &rows)),
                (None, Some(rows)) => Ok(merge_stash_rows(&log, &joined(rows)??)),
                (None, None) => Ok(log),
            }),
            head,
        )
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
/// A live stash entry as a row of the history: the stash commit, its base (the first parent) and what the row shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StashRow {
    pub id: String,
    /// The commit the stash was made on: the only parent the row has.
    pub base: String,
    /// The index and untracked-files commits behind the stash; they are not history.
    pub helpers: Vec<String>,
    pub selector: String,
    pub subject: String,
    pub author: String,
    pub date: String,
}

/// The ids and selectors (`stash@{n}`) of the live stack, and its rows when they were read to learn it.
type LiveStashes = (Vec<(String, String)>, Option<Vec<StashRow>>);

/// The ids and selectors (`stash@{n}`) of the live stack, newest first, and the rows when they had to be read to
/// learn it. Read from Git's files (no process, no rows); a repository whose files do not say (WSL, reftable) is
/// asked once, for the rows.
pub(super) fn live_stashes(root: &Path) -> Result<LiveStashes> {
    if let Some(list) = super::gitfacts::stashes(root) {
        return Ok((
            list.into_iter()
                .map(|(id, selector, _)| (id, selector))
                .collect(),
            None,
        ));
    }
    let rows = stash_rows(root)?;
    let stack = rows
        .iter()
        .map(|row| (row.id.clone(), row.selector.clone()))
        .collect();
    Ok((stack, Some(rows)))
}

/// Every entry of the stack with what its row needs, in one process.
pub(super) fn stash_rows(root: &Path) -> Result<Vec<StashRow>> {
    let output = git(
        root,
        &[
            "stash",
            "list",
            "--no-show-signature",
            "--format=%H%x00%P%x00%gd%x00%gs%x00%an%x00%aI",
        ],
    )?;
    Ok(parse_stash_rows(&output))
}

/// The live stack with what its rows need, or nothing when there is no stash (then no process runs where Git's
/// files say so).
pub(super) fn stash_entries(root: &Path) -> Result<Vec<StashRow>> {
    match live_stashes(root)? {
        (_, Some(rows)) => Ok(rows),
        (stack, None) if stack.is_empty() => Ok(Vec::new()),
        (_, None) => stash_rows(root),
    }
}

/// The history line of a stash row: the format of a commit, its only parent the base, no decoration, and the selector
/// as a seventh field.
pub(super) fn stash_line(row: &StashRow) -> String {
    format!(
        "{}\0{}\0\0{}\0{}\0{}\0{}",
        row.id, row.base, row.subject, row.author, row.date, row.selector
    )
}

pub(super) fn parse_stash_rows(output: &str) -> Vec<StashRow> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let id = fields.next()?.to_owned();
            let mut parents = fields.next()?.split(' ').filter(|p| !p.is_empty());
            let base = parents.next()?.to_owned();
            let helpers = parents.map(str::to_owned).collect();
            Some(StashRow {
                id,
                base,
                helpers,
                selector: fields.next()?.to_owned(),
                subject: fields.next()?.to_owned(),
                author: fields.next()?.to_owned(),
                date: fields.next()?.to_owned(),
            })
        })
        .collect()
}

/// Makes each live stash one row of the history. A row has the line format of a commit plus a seventh field, the
/// selector (`stash@{n}`), which also tells the row from a commit of the same id; its only parent is the base. The
/// stash commits that a walk of `refs/stash` or of the reflogs brought along, and their helper commits (index,
/// untracked files), are removed, unless the object also belongs to ordinary history: another reference points at it,
/// or a commit that is shown has it as a parent. A row is placed just before its base, so a parent still follows its
/// child; when the base is not in the list (paged out) the row comes last and its edge leaves the page like any other.
pub(super) fn merge_stash_rows(history: &str, rows: &[StashRow]) -> String {
    use std::collections::HashSet;
    if rows.is_empty() {
        return history.to_owned();
    }
    struct Line<'a> {
        text: &'a str,
        id: &'a str,
        parents: Vec<&'a str>,
        /// A reference other than `refs/stash` names it.
        named: bool,
    }
    let lines: Vec<Line> = history
        .lines()
        .filter(|text| !text.is_empty())
        .map(|text| {
            let mut fields = text.split('\0');
            let id = fields.next().unwrap_or("");
            let parents = fields
                .next()
                .unwrap_or("")
                .split(' ')
                .filter(|p| !p.is_empty())
                .collect();
            let named = fields
                .next()
                .unwrap_or("")
                .split(", ")
                .any(|name| !name.is_empty() && name != "refs/stash");
            Line {
                text,
                id,
                parents,
                named,
            }
        })
        .collect();
    let stash_ids: HashSet<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    let helpers: HashSet<&str> = rows
        .iter()
        .flat_map(|row| row.helpers.iter().map(String::as_str))
        .collect();
    let behind = |line: &Line| stash_ids.contains(line.id) || helpers.contains(line.id);
    // What commits of ordinary history point to.
    let mut referenced: HashSet<&str> = lines
        .iter()
        .filter(|line| !behind(line))
        .flat_map(|line| line.parents.iter().copied())
        .collect();
    // A stash commit that is ordinary history keeps its own parents in the list.
    let independent: Vec<&Line> = lines
        .iter()
        .filter(|line| stash_ids.contains(line.id) && (line.named || referenced.contains(line.id)))
        .collect();
    for line in independent {
        referenced.extend(line.parents.iter().copied());
    }
    let keep = |line: &Line| !behind(line) || line.named || referenced.contains(line.id);
    let line_of = stash_line;
    let mut out = String::with_capacity(history.len() + rows.len() * 160);
    let mut placed: HashSet<usize> = HashSet::new();
    for line in lines.iter().filter(|line| keep(line)) {
        for (index, row) in rows.iter().enumerate() {
            if row.base == line.id && placed.insert(index) {
                out.push_str(&line_of(row));
                out.push('\n');
            }
        }
        out.push_str(line.text);
        out.push('\n');
    }
    for (index, row) in rows.iter().enumerate() {
        if !placed.contains(&index) {
            out.push_str(&line_of(row));
            out.push('\n');
        }
    }
    out
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

/// The recovery reference of a history rewrite that finished successfully becomes one of the
/// operation journal, which every Gall graph leaves out. Best effort: a leftover reference is
/// harmless.
pub(super) fn drop_recovery(root: &Path, id: &str) {
    super::gitjournal::keep_for_undo(root, id);
}

/// Keep a working-tree stash snapshot reachable and associate it with the current operation.
pub(super) fn retain_recovery(root: &Path, id: &str, message: &str) -> Result<()> {
    retain_commit(root, id, message)?;
    super::gitjournal::saved_snapshot(id);
    Ok(())
}

/// Keep a commit reachable without treating it as a snapshot of working changes.
pub(super) fn retain_commit(root: &Path, id: &str, message: &str) -> Result<()> {
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
/// Returns the stash commit; None when there was nothing to remove.
pub(super) fn push_recovery(root: &Path, extra: &[&str], message: &str) -> Result<Option<String>> {
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
        return Ok(None);
    }
    retain_recovery(root, &created, message)?;
    git(root, &["stash", "drop", "-q", "stash@{0}"])?;
    Ok(Some(created))
}

pub fn switch(path: &Path, target: &str, branch: bool, action: &str, message: &str) -> Result<()> {
    let label = if branch {
        target
    } else {
        target.get(..8).unwrap_or(target)
    };
    let operation = Some((super::gitjournal::Kind::Checkout, label.to_owned()));
    super::gitjournal::around(path, operation, || {
        switch_now(path, target, branch, action, message)
    })
}

fn switch_now(path: &Path, target: &str, branch: bool, action: &str, message: &str) -> Result<()> {
    let _busy = super::gitfacts::mutating();
    let root = root(path)?;
    let changes = if action == "clean" {
        // Git checks collisions itself, including staged and untracked paths.
        String::new()
    } else {
        status_z(&root)?
    };
    let known = if branch { branches(&root)? } else { Vec::new() };
    if !changes.is_empty() && matches!(action, "discard" | "wip") && has_content_filters(&root) {
        return Err(CoreError::Other(
            "The recovery snapshot cannot guarantee original filtered bytes. Keep the changes, stash them, or commit them before switching".into(),
        ));
    }
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
                super::gitprogress::report_phase("Stashing changes", None);
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
            "discard" => {
                super::gitprogress::report_phase("Saving recovery snapshot", None);
                push_recovery(&root, &[], "Gall recovery before discard")?;
            }
            // Opt-in: the changes stay with the branch they were made on and return with it (`gitwip`).
            "wip" => {
                super::gitprogress::report_phase("Saving branch changes", None);
                match super::gitwip::current_branch(&root) {
                    Some(branch) => super::gitwip::save(&root, &branch)?,
                    None => {
                        return Err(CoreError::Other(
                            "A detached HEAD has no branch to remember the changes for. Stash or commit them".into(),
                        ))
                    }
                }
            }
            "commit" => {
                if message.trim().is_empty() {
                    return Err(CoreError::Other("A commit message is required".into()));
                }
                super::gitprogress::report_phase("Committing changes", None);
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
    let with_progress = super::gitprogress::is_active();
    // Never force checkout: Git protects ignored/untracked collisions and concurrent edits.
    if branch {
        super::gitprogress::report_phase("Switching branch", Some(target.to_owned()));
        let mut args = vec!["-c", "submodule.recurse=false", "switch"];
        if with_progress {
            args.push("--progress");
        }
        args.extend(["--no-overwrite-ignore", "--", target]);
        git(&root, &args)?;
    } else {
        super::gitprogress::report_phase(
            "Checking out commit",
            Some(target.get(..8).unwrap_or(target).to_owned()),
        );
        let mut args = vec!["-c", "submodule.recurse=false", "switch"];
        if with_progress {
            args.push("--progress");
        }
        args.extend(["--detach", "--no-overwrite-ignore", target]);
        git(&root, &args)?;
    }
    // With `wip`, the work this branch was left with comes back; one that does not apply cleanly stays (see `gitwip`).
    if action == "wip" && branch {
        super::gitwip::restore(&root, target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_rebase_continue_reports_stdout_and_preserves_the_index_and_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-q"]).unwrap();
        let excludes = root.join(".git/excludes-empty");
        std::fs::write(&excludes, "").unwrap();
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.invalid"),
            ("core.autocrlf", "false"),
            ("core.excludesFile", excludes.to_str().unwrap()),
        ] {
            git(root, &["config", key, value]).unwrap();
        }
        std::fs::write(root.join("conflict.txt"), "base\n").unwrap();
        std::fs::write(root.join("other.txt"), "original\n").unwrap();
        git(root, &["add", "--all"]).unwrap();
        git(root, &["commit", "-qm", "base"]).unwrap();
        let base = git(root, &["branch", "--show-current"]).unwrap();
        git(root, &["switch", "-qc", "topic"]).unwrap();
        std::fs::write(root.join("conflict.txt"), "topic\n").unwrap();
        git(root, &["commit", "-qam", "topic"]).unwrap();
        git(root, &["switch", "-q", base.trim()]).unwrap();
        std::fs::write(root.join("conflict.txt"), "main\n").unwrap();
        git(root, &["commit", "-qam", "main"]).unwrap();
        git(root, &["switch", "-q", "topic"]).unwrap();
        assert!(git(root, &["rebase", base.trim()]).is_err());
        std::fs::write(root.join("conflict.txt"), "resolved\n").unwrap();
        git(root, &["add", "--", "conflict.txt"]).unwrap();
        std::fs::write(root.join("other.txt"), "unstaged\n").unwrap();
        let before = git(root, &["diff", "--cached", "--binary"]).unwrap();
        let error = git(root, &["-c", "core.editor=true", "rebase", "--continue"])
            .unwrap_err()
            .to_string();
        assert!(!error.contains("Git command failed ("), "{error}");
        assert_eq!(
            git(root, &["diff", "--cached", "--binary"]).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read_to_string(root.join("other.txt")).unwrap(),
            "unstaged\n"
        );
        assert!(root.join(".git/rebase-merge").exists());
    }

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
    fn filtered_switch_keeps_unrelated_local_bytes_and_refuses_an_overwrite() {
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
        std::fs::write(p.join(".gitattributes"), "filtered filter=roundtrip\n").unwrap();
        std::fs::write(p.join("filtered"), "repo main\n").unwrap();
        std::fs::write(p.join("unrelated"), "original\n").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "initial"]).unwrap();
        git(p, &["switch", "-q", "-c", "other"]).unwrap();
        std::fs::write(p.join("filtered"), "repo other\n").unwrap();
        git(p, &["commit", "-qam", "other content"]).unwrap();
        git(p, &["switch", "-q", "main"]).unwrap();
        for (key, value) in [
            ("filter.roundtrip.clean", "sed 's/working/repo/g'"),
            ("filter.roundtrip.smudge", "sed 's/repo/working/g'"),
            ("filter.roundtrip.required", "true"),
        ] {
            git(p, &["config", key, value]).unwrap();
        }
        std::fs::write(p.join("unrelated"), "local change\n").unwrap();
        std::fs::write(p.join("extra"), "untracked\n").unwrap();
        switch(p, "other", true, "clean", "").unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("filtered")).unwrap(),
            "working other\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("unrelated")).unwrap(),
            "local change\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("extra")).unwrap(),
            "untracked\n"
        );
        std::fs::write(p.join("filtered"), "working local\n").unwrap();
        assert!(switch(p, "main", true, "clean", "")
            .unwrap_err()
            .to_string()
            .contains("filtered"));
        assert_eq!(
            git(p, &["symbolic-ref", "--short", "HEAD"]).unwrap().trim(),
            "other"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("filtered")).unwrap(),
            "working local\n"
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
        switch(p, &first, false, "clean", "").unwrap();
        assert_eq!(std::fs::read_to_string(p.join("f")).unwrap(), "dirty");
        assert_eq!(std::fs::read_to_string(p.join("new")).unwrap(), "untracked");
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
        switch(p, &first, false, "clean", "").unwrap();
        assert!(!comparison_graph(p).unwrap().history.is_empty());
    }

    /// A repository with two commits and two stashes of different shapes: tracked changes only, and with a new file.
    fn two_stashes() -> (tempfile::TempDir, String, Vec<String>) {
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
        let write = |name: &str, text: &str| std::fs::write(p.join(name), text).unwrap();
        write("a.txt", "one\n");
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "one"]).unwrap();
        write("a.txt", "two\n");
        git(p, &["commit", "-qam", "two"]).unwrap();
        let base = git(p, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        write("a.txt", "tracked change\n");
        git(p, &["stash", "push", "-q", "-m", "tracked only"]).unwrap();
        write("a.txt", "second change\n");
        write("new.txt", "untracked\n");
        git(
            p,
            &[
                "stash",
                "push",
                "-q",
                "--include-untracked",
                "-m",
                "with untracked",
            ],
        )
        .unwrap();
        let ids = git(p, &["stash", "list", "--format=%H"])
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        (d, base, ids)
    }

    fn rows(history: &str) -> Vec<Vec<&str>> {
        history
            .lines()
            .map(|line| line.split('\0').collect())
            .collect()
    }

    #[test]
    fn each_live_stash_is_one_row_with_only_its_base_as_parent_and_no_helper_rows() {
        let (d, base, ids) = two_stashes();
        let p = d.path();
        let history = history_part(p, false, 200, None, None).unwrap().history;
        let all = rows(&history);
        // Two commits and two stash rows: no index or untracked-files commits.
        assert_eq!(all.len(), 4, "{history}");
        let stash: Vec<_> = all.iter().filter(|row| row.len() == 7).collect();
        assert_eq!(stash.len(), 2);
        // Newest first, selector in the seventh field, the base as the only parent, no decoration.
        assert_eq!((stash[0][0], stash[0][6]), (ids[0].as_str(), "stash@{0}"));
        assert_eq!((stash[1][0], stash[1][6]), (ids[1].as_str(), "stash@{1}"));
        assert!(stash.iter().all(|row| row[1] == base && row[2].is_empty()));
        // The helper commits are not rows.
        for id in &ids {
            for parent in ["^2", "^3"] {
                if let Ok(helper) = git(
                    p,
                    &["rev-parse", "--verify", "-q", &format!("{id}{parent}")],
                ) {
                    assert!(!history.contains(helper.trim()), "helper {helper} is a row");
                }
            }
        }
        // Both rows sit above their base, which is the latest commit.
        let at = |id: &str| all.iter().position(|row| row[0] == id).unwrap();
        assert!(at(&ids[0]) < at(&base) && at(&ids[1]) < at(&base));
        assert_eq!(all[at(&base)].len(), 6);
    }

    #[test]
    fn a_stash_whose_object_is_also_a_branch_keeps_both_rows_and_its_history() {
        let (d, _base, ids) = two_stashes();
        let p = d.path();
        git(p, &["branch", "same", &ids[0]]).unwrap();
        let history = history_part(p, false, 200, None, None).unwrap().history;
        let all = rows(&history);
        let same: Vec<_> = all.iter().filter(|row| row[0] == ids[0]).collect();
        // The ordinary commit (named by the branch, with its own parents) and the stash row of the same object.
        assert_eq!(same.len(), 2, "{history}");
        assert!(same
            .iter()
            .any(|row| row.len() == 6 && row[2].contains("same") && row[1].split(' ').count() > 1));
        assert!(same
            .iter()
            .any(|row| row.len() == 7 && row[6] == "stash@{0}" && !row[1].contains(' ')));
    }

    #[test]
    fn dropping_and_popping_stashes_changes_the_rows_at_the_next_read() {
        let (d, _base, ids) = two_stashes();
        let p = d.path();
        git(p, &["stash", "drop", "-q", "stash@{0}"]).unwrap();
        let history = history_part(p, false, 200, None, None).unwrap().history;
        let stash: Vec<_> = rows(&history)
            .into_iter()
            .filter(|row| row.len() == 7)
            .collect();
        // The one that is left is stash@{0} now, and it is the other object.
        assert_eq!(stash.len(), 1);
        assert_eq!((stash[0][0], stash[0][6]), (ids[1].as_str(), "stash@{0}"));
        git(p, &["stash", "drop", "-q"]).unwrap();
        let history = history_part(p, false, 200, None, None).unwrap().history;
        assert!(rows(&history).iter().all(|row| row.len() == 6));
        assert_eq!(rows(&history).len(), 2);
    }

    #[test]
    fn reflog_mode_collapses_the_stashes_too_and_scoped_history_is_left_alone() {
        let (d, base, ids) = two_stashes();
        let p = d.path();
        let history = history_part(p, true, 200, None, None).unwrap().history;
        let all = rows(&history);
        let stash: Vec<_> = all.iter().filter(|row| row.len() == 7).collect();
        assert_eq!(stash.len(), 2, "{history}");
        assert!(stash.iter().all(|row| row[1] == base));
        // Each stash id appears once, and no commit that only the stash led to is left.
        for id in &ids {
            assert_eq!(all.iter().filter(|row| row[0] == id.as_str()).count(), 1);
            for parent in ["^2", "^3"] {
                if let Ok(helper) = git(
                    p,
                    &["rev-parse", "--verify", "-q", &format!("{id}{parent}")],
                ) {
                    assert!(!history.contains(helper.trim()));
                }
            }
        }
        // A history scoped to a reference has no stash rows.
        let scoped = history_part(p, false, 200, Some("refs/heads/main"), None)
            .unwrap()
            .history;
        assert!(rows(&scoped).iter().all(|row| row.len() == 6));
    }

    #[test]
    fn a_stash_whose_base_is_paged_out_comes_last_and_names_its_base() {
        let (d, base, ids) = two_stashes();
        let p = d.path();
        // Commit on top of the base and stash later: the base is the second commit of the page.
        std::fs::write(p.join("b.txt"), "x\n").unwrap();
        git(p, &["add", "b.txt"]).unwrap();
        git(p, &["commit", "-qm", "three"]).unwrap();
        let history = history_part(p, false, 1, None, None).unwrap().history;
        let all = rows(&history);
        // One ordinary row (the budget), then the two stash rows whose base is not in the list.
        assert_eq!(all.len(), 3, "{history}");
        assert_eq!(all[0].len(), 6);
        assert_eq!((all[1][0], all[2][0]), (ids[0].as_str(), ids[1].as_str()));
        assert!(all[1][1] == base && all[2][1] == base);
        assert!(!all.iter().any(|row| row[0] == base));
    }

    #[test]
    fn stash_rows_keep_the_order_of_the_stack_and_sit_before_their_base() {
        let row = |id: &str, base: &str, n: usize| StashRow {
            id: id.into(),
            base: base.into(),
            helpers: vec![format!("{id}i")],
            selector: format!("stash@{{{n}}}"),
            subject: format!("On main: {id}"),
            author: "A".into(),
            date: "2026-01-01".into(),
        };
        let line =
            |id: &str, parents: &str, refs: &str| format!("{id}\0{parents}\0{refs}\0s\0A\0d\n");
        let history = [
            line("tip", "b1", "HEAD -> main"),
            line("b1", "b0", ""),
            line("b0", "", ""),
        ]
        .concat();
        let merged = merge_stash_rows(
            &history,
            &[row("s0", "b1", 0), row("s1", "b1", 1), row("s2", "b0", 2)],
        );
        let ids: Vec<&str> = merged
            .lines()
            .map(|l| l.split('\0').next().unwrap())
            .collect();
        assert_eq!(ids, ["tip", "s0", "s1", "b1", "s2", "b0"]);
        // No stash: the history comes back as it was.
        assert_eq!(merge_stash_rows(&history, &[]), history);
    }

    #[test]
    fn non_interactive_git_overrides_inherited_git_editor() {
        let d = tempfile::tempdir().unwrap();
        git(d.path(), &["init", "-q"]).unwrap();
        let editor = git(d.path(), &["var", "GIT_EDITOR"]).unwrap();
        assert_eq!(editor.trim(), "true");
    }
}
