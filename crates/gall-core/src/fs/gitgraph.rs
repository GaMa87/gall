//! Repository history and explicit revision switching.
use crate::{CoreError, Result};
use serde::Serialize;
use std::path::Path;
use std::process::{Command, Stdio};

/// Whether `config_lines` (`git config --get-regexp` output, `key value` per
/// line) define a content filter that would run a program on checkout or
/// staging. The standard Git LFS filter is trusted and does not count.
///
/// The LFS exception is a prefix check (`filter.lfs.*` whose value starts with
/// `git-lfs `), which is what `git lfs install` writes. It is not a security
/// boundary: anyone who can write the user's Git configuration can already run
/// programs, so this only avoids refusing ordinary LFS repositories.
fn filters_block(config_lines: &str) -> bool {
    config_lines.lines().any(|line| {
        let (key, value) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let standard_lfs = key.starts_with("filter.lfs.") && value.trim().starts_with("git-lfs ");
        !standard_lfs
    })
}

/// True when the repository's effective configuration (including global and
/// system files) has a content filter other than standard Git LFS.
pub(super) fn has_content_filters(path: &Path) -> bool {
    git(
        path,
        &[
            "config",
            "--get-regexp",
            "^filter\\..*\\.(clean|smudge|process)$",
        ],
    )
    .is_ok_and(|lines| filters_block(&lines))
}

/// Queries that only read the repository. They are the ones a view repeats many times, so
/// they start the real Git program directly (see `gitstatus::program`); everything that
/// changes the repository keeps going through `git` on `PATH`, exactly as in a terminal.
fn reads_only(args: &[&str]) -> bool {
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

pub(super) fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = if reads_only(args) {
        Command::new(super::gitstatus::program())
    } else {
        Command::new("git")
    };
    cmd.args([
        "-c",
        "core.fsmonitor=false",
        "-c",
        if cfg!(windows) {
            "core.hooksPath=NUL"
        } else {
            "core.hooksPath=/dev/null"
        },
        "-c",
        "commit.gpgSign=false",
        "-c",
        "log.showSignature=false",
        "-C",
    ])
    .arg(root)
    .args(args)
    .stdin(Stdio::null())
    .env("GIT_OPTIONAL_LOCKS", "0")
    .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let out = cmd.output().map_err(|e| CoreError::Other(e.to_string()))?;
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

/// Work tree status with untracked files, in the machine-readable `-z` form.
pub(super) fn status_z(root: &Path) -> Result<String> {
    git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
    )
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
pub(super) fn retain_detached(root: &Path) -> Result<()> {
    if git(root, &["symbolic-ref", "-q", "HEAD"]).is_err() {
        let head = git(root, &["rev-parse", "--verify", "HEAD"])?;
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

pub fn switch(path: &Path, target: &str, branch: bool, action: &str, message: &str) -> Result<()> {
    let root = root(path)?;
    if has_content_filters(&root) {
        return Err(CoreError::Other(
            "Switching repositories with content filters is not supported yet".into(),
        ));
    }
    let state = graph(&root)?;
    if branch {
        if !state.branches.iter().any(|b| b == target) {
            return Err(CoreError::Other("branch no longer exists".into()));
        }
    } else if !(target.len() == 40 || target.len() == 64)
        || !target.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(CoreError::Other("invalid commit ID".into()));
    }
    // Reject unfinished operations and submodules rather than risk partial data loss.
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "BISECT_START",
    ] {
        let p = git(&root, &["rev-parse", "--git-path", marker])?;
        let p = std::path::PathBuf::from(p.trim());
        if (if p.is_absolute() { p } else { root.join(p) }).exists() {
            return Err(CoreError::Other(
                "Finish or abort the current Git operation first".into(),
            ));
        }
    }
    if root.join(".gitmodules").exists()
        || git(&root, &["ls-files", "--stage"])?
            .lines()
            .any(|line| line.starts_with("160000 "))
    {
        return Err(CoreError::Other(
            "Switching repositories with submodules is not supported yet".into(),
        ));
    }
    if !state.changes.is_empty() {
        match action {
            "stash" | "discard" => {
                git(
                    &root,
                    &[
                        "stash",
                        "push",
                        "--include-untracked",
                        "-m",
                        if action == "discard" {
                            "Gall recovery before discard"
                        } else {
                            "Gall before revision switch"
                        },
                    ],
                )?;
            }
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
    if !git(
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
        git(&root, &["switch", "--no-overwrite-ignore", "--", target])?;
    } else {
        git(
            &root,
            &["switch", "--detach", "--no-overwrite-ignore", target],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(git(p, &["stash", "list"])
            .unwrap()
            .contains("Gall recovery before discard"));
        git(p, &["config", "filter.example.clean", "false"]).unwrap();
        assert!(graph(p).unwrap().read_only_reason.is_some());
        assert!(switch(p, &first, false, "clean", "").is_err());
        assert!(!comparison_graph(p).unwrap().history.is_empty());
    }
}
