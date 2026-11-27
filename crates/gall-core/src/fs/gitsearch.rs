//! Search through the whole history: one cancellable `git log` whose matches are passed on as
//! they are found. Read-only.
use super::gitgraph::root;
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

/// The most matches one search returns; the list says that there are more.
pub const MAX_HITS: usize = 500;

/// What to look for. Filled fields must all match; texts are literal and ignore case, except
/// `content`, which Git compares exactly.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommitQuery {
    /// Part of the author's name or address (`--author`).
    pub author: String,
    /// Part of the message (`--grep`).
    pub message: String,
    /// Part of the path of a changed file or of a folder above it.
    pub path: String,
    /// Text whose number of occurrences a commit changed (`-S`).
    pub content: String,
    /// A full or abbreviated commit ID, looked up besides the other fields.
    pub hash: String,
}

/// Commits in the format of the history view, one per line.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchEnd {
    pub count: usize,
    pub truncated: bool,
}

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

fn checked(text: &str) -> Result<&str> {
    let text = text.trim();
    if text.len() > 1000 || text.chars().any(|c| c == '\0' || c == '\n' || c == '\r') {
        return Err(fail("The search text is too long or has line breaks"));
    }
    Ok(text)
}

/// Pathspecs for "the path contains this": files, and everything in folders, that do.
fn path_specs(text: &str) -> [String; 2] {
    let mut literal = String::new();
    for c in text.replace('\\', "/").trim_matches('/').chars() {
        if matches!(c, '*' | '?' | '[' | ']') {
            literal.push('\\');
        }
        literal.push(c);
    }
    [
        format!(":(top,icase,glob)**/*{literal}*"),
        format!(":(top,icase,glob)**/*{literal}*/**"),
    ]
}

/// The start of every `git log` of a search: the format, the matching rules and the filters of `query` (no walk and no
/// path yet).
fn base_args(query: &CommitQuery) -> Result<Vec<String>> {
    let mut args: Vec<String> = [
        "log",
        "--decorate-refs=*",
        "--no-color",
        "--no-show-signature",
        "--format=%H%x00%P%x00%D%x00%s%x00%an%x00%aI",
        "--regexp-ignore-case",
        "--fixed-strings",
        "--all-match",
    ]
    .map(String::from)
    .to_vec();
    args.push(format!("--max-count={}", MAX_HITS + 1));
    for (option, text) in [("--author=", &query.author), ("--grep=", &query.message)] {
        let text = checked(text)?;
        if !text.is_empty() {
            args.push(format!("{option}{text}"));
        }
    }
    let content = checked(&query.content)?;
    if !content.is_empty() {
        args.push(format!("-S{content}"));
    }
    Ok(args)
}

/// The end of every `git log` of a search: the path filter, after `--`.
fn path_args(query: &CommitQuery) -> Result<Vec<String>> {
    let path = checked(&query.path)?;
    let mut args = vec!["--".to_owned()];
    if !path.is_empty() {
        args.extend(path_specs(path));
    }
    Ok(args)
}

/// The arguments of `git log` for `query`, without the hash. `stash` keeps the walk of the stash stack out of the
/// all-reference search: its entries are searched on their own (see `stash_args`), once each.
fn log_args(
    query: &CommitQuery,
    reflog: bool,
    reference: Option<&str>,
    stash: bool,
) -> Result<Vec<String>> {
    let mut args = base_args(query)?;
    match reference.filter(|name| !name.is_empty()) {
        // A full name only, so that a search cannot pass an option or a range.
        Some(name) if name.starts_with("refs/") && !name.contains("..") => args.push(name.into()),
        Some(_) => return Err(fail("The selected reference no longer exists")),
        None if reflog => args.extend(["--all".into(), "--reflog".into()]),
        None => {
            args.extend([
                "--exclude=refs/gall/recovery/*".into(),
                "--exclude=refs/gall/journal/*".into(),
            ]);
            if stash {
                args.push("--exclude=refs/stash".into());
            }
            args.extend([
                "--all".into(),
                "--decorate-refs-exclude=refs/gall/recovery/*".into(),
                "--decorate-refs-exclude=refs/gall/journal/*".into(),
            ]);
        }
    }
    args.extend(path_args(query)?);
    Ok(args)
}

/// `git log` for the stash commits themselves (no walk behind them) with the filters of `query`: which entries of the
/// stack match. A stash commit differs from its base by the stashed changes, so a path or a pickaxe filter applies.
fn stash_args(query: &CommitQuery, ids: impl Iterator<Item = String>) -> Result<Vec<String>> {
    let mut args = base_args(query)?;
    args.push("--no-walk=unsorted".into());
    args.extend(ids);
    args.extend(path_args(query)?);
    Ok(args)
}

/// Runs Git and passes complete lines on. Ends early when `cancelled` says so or `hit` returns
/// false (enough matches).
pub(super) fn stream(
    root: &Path,
    args: &[String],
    cancelled: &dyn Fn() -> bool,
    mut hit: impl FnMut(Vec<u8>) -> bool,
) -> Result<bool> {
    let mut command = super::gitexec::command(root, super::gitstatus::program());
    command
        .args(["--no-pager", "-c", "core.fsmonitor=false", "-C"])
        .arg(super::gitexec::dir_arg(root))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1");
    super::gitexec::forward_env(&mut command);
    let mut child = command.spawn().map_err(|e| fail(e.to_string()))?;
    let (send, receive) = mpsc::sync_channel::<Vec<u8>>(16);
    let mut stdout = child.stdout.take().ok_or_else(|| fail("No Git output"))?;
    std::thread::spawn(move || {
        let mut bytes = [0; 16 * 1024];
        while let Ok(count) = stdout.read(&mut bytes) {
            if count == 0 || send.send(bytes[..count].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut stderr = child.stderr.take().ok_or_else(|| fail("No Git output"))?;
    let errors = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.by_ref().take(16 * 1024).read_to_end(&mut text);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        text
    });
    let mut rest: Vec<u8> = Vec::new();
    let mut stopped = false;
    loop {
        if cancelled() {
            stopped = true;
            break;
        }
        match receive.recv_timeout(Duration::from_millis(20)) {
            Ok(chunk) => {
                rest.extend(chunk);
                let Some(end) = rest.iter().rposition(|b| *b == b'\n') else {
                    continue;
                };
                let tail = rest.split_off(end + 1);
                if !hit(std::mem::replace(&mut rest, tail)) {
                    stopped = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if stopped {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(false);
    }
    let status = child.wait().map_err(|e| fail(e.to_string()))?;
    if !status.success() {
        let text = errors.join().unwrap_or_default();
        return Err(fail(String::from_utf8_lossy(&text).trim().to_owned()));
    }
    if !rest.is_empty() {
        hit(rest);
    }
    Ok(true)
}

/// Searches the history of `path`; `found` gets the matches in history order, as lines in the
/// format of the history view. An empty query finds nothing. Stops silently when cancelled.
pub fn search(
    path: &Path,
    query: &CommitQuery,
    reflog: bool,
    reference: Option<&str>,
    cancelled: &dyn Fn() -> bool,
    mut found: impl FnMut(String),
) -> Result<SearchEnd> {
    let root = root(path)?;
    let mut count = 0;
    let mut truncated = false;
    let mut by_hash = String::new();
    let hash = checked(&query.hash)?.to_ascii_lowercase();
    if (4..=64).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        // Git decides what an abbreviation means; an unknown or ambiguous one finds nothing.
        let args = [
            "log",
            "--no-walk",
            "--max-count=1",
            "--decorate-refs=*",
            "--decorate-refs-exclude=refs/gall/recovery/*",
            "--decorate-refs-exclude=refs/gall/journal/*",
            "--no-color",
            "--no-show-signature",
            "--format=%H%x00%P%x00%D%x00%s%x00%an%x00%aI",
            &format!("{hash}^{{commit}}"),
            "--",
        ]
        .map(String::from);
        let mut line = Vec::new();
        let done = stream(&root, &args, cancelled, |bytes| {
            line.extend(bytes);
            true
        });
        if matches!(done, Ok(true)) && !line.is_empty() {
            let line = String::from_utf8_lossy(&line).into_owned();
            by_hash = line.split('\0').next().unwrap_or_default().to_owned();
            count += 1;
            found(line);
        }
    }
    let texts = [&query.author, &query.message, &query.path, &query.content];
    if texts.iter().all(|text| text.trim().is_empty()) || cancelled() {
        return Ok(SearchEnd { count, truncated });
    }
    // The live stash stack (none for a search scoped to a reference): its entries are found once each, as rows of
    // their own, and neither the stash commits nor their helper commits (index, untracked files) are matches of the walk.
    let stack = if reference.is_some_and(|name| !name.is_empty()) {
        Vec::new()
    } else {
        super::gitgraph::stash_entries(&root)?
    };
    let hidden: std::collections::HashSet<String> = stack
        .iter()
        .flat_map(|row| std::iter::once(row.id.clone()).chain(row.helpers.iter().cloned()))
        .collect();
    let args = log_args(query, reflog, reference, !stack.is_empty())?;
    stream(&root, &args, cancelled, |bytes| {
        let mut lines = String::new();
        for line in String::from_utf8_lossy(&bytes).lines() {
            // Found by its hash already.
            if line.is_empty() || (!by_hash.is_empty() && line.starts_with(&by_hash)) {
                continue;
            }
            // Reflog mode walks the stash commits too; they come back below as rows.
            if reflog && hidden.contains(line.split('\0').next().unwrap_or("")) {
                continue;
            }
            if count >= MAX_HITS {
                truncated = true;
                break;
            }
            count += 1;
            lines.push_str(line);
            lines.push('\n');
        }
        if !lines.is_empty() {
            super::gitprogress::report_counts(
                "Searching history",
                count as u64,
                None,
                "matches",
                None,
            );
            found(lines);
        }
        !truncated
    })?;
    if !stack.is_empty() && !truncated && !cancelled() {
        // The entries that match, with the same filters: only the stash commits, no walk behind them.
        let mut matching = String::new();
        let stash_args = stash_args(query, stack.iter().map(|row| row.id.clone()))?;
        stream(&root, &stash_args, cancelled, |bytes| {
            matching.push_str(&String::from_utf8_lossy(&bytes));
            true
        })?;
        let mut lines = String::new();
        for line in matching.lines() {
            let id = line.split('\0').next().unwrap_or("");
            let Some(row) = stack.iter().find(|row| row.id == id) else {
                continue;
            };
            if count >= MAX_HITS {
                truncated = true;
                break;
            }
            count += 1;
            lines.push_str(&super::gitgraph::stash_line(row));
            lines.push('\n');
        }
        if !lines.is_empty() {
            super::gitprogress::report_counts(
                "Searching history",
                count as u64,
                None,
                "matches",
                None,
            );
            found(lines);
        }
    }
    Ok(SearchEnd { count, truncated })
}

#[cfg(test)]
mod tests {
    use super::super::gitgraph::git;
    use super::*;
    use std::cell::Cell;

    fn repository() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [
            ("user.name", "Test User"),
            ("user.email", "test@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(p, &["config", key, value]).unwrap();
        }
        let commit = |file: &str, text: &str, message: &str, author: &str| {
            let file = p.join(file);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
            git(p, &["add", "--all"]).unwrap();
            git(
                p,
                &["commit", "-qm", message, &format!("--author={author}")],
            )
            .unwrap();
        };
        commit(
            "src/login.rs",
            "fn a() {}\n",
            "Add login",
            "Ana Novak <ana@x.si>",
        );
        commit(
            "src/login.rs",
            "fn a() { Needle }\n",
            "Fix [bug] 100%",
            "Bor Kos <bor@x.si>",
        );
        commit(
            "docs/Guide.md",
            "text\n",
            "Write a guide",
            "Ana Novak <ana@x.si>",
        );
        git(p, &["switch", "-qc", "side"]).unwrap();
        commit(
            "src/a[1].txt",
            "x\n",
            "Side work\n\nbody mentions login",
            "Bor Kos <bor@x.si>",
        );
        git(p, &["switch", "-q", "main"]).unwrap();
        d
    }

    fn ids(path: &Path, query: CommitQuery) -> Vec<String> {
        let mut all = String::new();
        let end = search(path, &query, false, None, &|| false, |lines| {
            all.push_str(&lines)
        })
        .unwrap();
        let ids: Vec<String> = all
            .lines()
            .map(|line| line.split('\0').next().unwrap().to_owned())
            .collect();
        assert_eq!(ids.len(), end.count);
        ids
    }

    fn log(path: &Path, args: &[&str]) -> Vec<String> {
        let mut all = vec!["log", "--format=%H", "--all"];
        all.extend(args);
        git(path, &all)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn finds_what_git_log_finds() {
        let d = repository();
        let p = d.path();
        let query = |author: &str, message: &str, path: &str, content: &str| CommitQuery {
            author: author.into(),
            message: message.into(),
            path: path.into(),
            content: content.into(),
            hash: String::new(),
        };
        for (found, expected) in [
            (query("ana", "", "", ""), log(p, &["-i", "--author=ana"])),
            (
                query("bor@x", "", "", ""),
                log(p, &["-i", "--author=bor@x"]),
            ),
            (query("", "login", "", ""), log(p, &["-i", "--grep=login"])),
            // Literal text, not a pattern.
            (
                query("", "[bug] 100%", "", ""),
                log(p, &["-F", "--grep=[bug] 100%"]),
            ),
            (query("", "", "", "Needle"), log(p, &["-SNeedle"])),
            (
                query("bor", "login", "", ""),
                log(p, &["-i", "--author=bor", "--grep=login"]),
            ),
            (
                query("", "", "login.rs", ""),
                log(p, &["--", "src/login.rs"]),
            ),
            (
                query("", "", "SRC/Log", ""),
                log(p, &["--", "src"])[1..].to_vec(),
            ),
            (query("", "", "docs", ""), log(p, &["--", "docs"])),
            (
                query("", "", "a[1]", ""),
                log(p, &["--", ":(literal)src/a[1].txt"]),
            ),
            (
                query("ana", "", "src", ""),
                log(p, &["-i", "--author=ana", "--", "src"]),
            ),
        ] {
            assert!(!expected.is_empty());
            assert_eq!(ids(p, found.clone()), expected, "{found:?}");
        }
        assert!(ids(p, query("nobody", "", "", "")).is_empty());
        assert!(ids(p, CommitQuery::default()).is_empty());
    }

    #[test]
    fn a_hash_comes_first_and_only_once() {
        let d = repository();
        let p = d.path();
        let head = git(p, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        let by = |hash: &str, message: &str| {
            ids(
                p,
                CommitQuery {
                    hash: hash.into(),
                    message: message.into(),
                    ..Default::default()
                },
            )
        };
        assert_eq!(by(&head, ""), vec![head.clone()]);
        assert_eq!(by(&head[..7].to_uppercase(), ""), vec![head.clone()]);
        assert_eq!(by(&head[..10], "guide"), vec![head.clone()]);
        for nothing in ["0000000", "zzzz", "abc", "--all", "HEAD"] {
            assert!(by(nothing, "").is_empty(), "{nothing}");
        }
        // A blob is no commit.
        let blob = git(p, &["rev-parse", "HEAD:src/login.rs"]).unwrap();
        assert!(by(blob.trim(), "").is_empty());
    }

    #[test]
    fn stays_within_the_chosen_reference_and_rejects_anything_else() {
        let d = repository();
        let p = d.path();
        let run = |reference: &str, query: &CommitQuery| {
            let mut count = 0;
            search(p, query, false, Some(reference), &|| false, |lines| {
                count += lines.lines().count()
            })
            .map(|_| count)
        };
        let bor = CommitQuery {
            author: "bor".into(),
            ..Default::default()
        };
        assert_eq!(run("refs/heads/main", &bor).unwrap(), 1);
        assert_eq!(run("refs/heads/side", &bor).unwrap(), 2);
        for bad in ["--output=x", "main", "refs/heads/a..b", "refs/heads/gone"] {
            assert!(run(bad, &bor).is_err(), "{bad}");
        }
        let broken = CommitQuery {
            message: "a\nb".into(),
            ..Default::default()
        };
        assert!(run("refs/heads/main", &broken).is_err());
        // An option cannot be smuggled in through a text.
        let option = CommitQuery {
            message: "--output=owned".into(),
            path: "--output=owned".into(),
            ..Default::default()
        };
        assert_eq!(run("refs/heads/main", &option).unwrap(), 0);
        assert!(!p.join("owned").exists());
    }

    #[test]
    fn stops_when_cancelled_and_at_the_limit() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [("user.name", "T"), ("user.email", "t@example.com")] {
            git(p, &["config", key, value]).unwrap();
        }
        git(p, &["commit", "-q", "--allow-empty", "-m", "base"]).unwrap();
        // Reachable commits in one go, without a process for each.
        let tree = git(p, &["rev-parse", "HEAD^{tree}"]).unwrap();
        let mut stream = String::new();
        for i in 0..MAX_HITS + 50 {
            stream.push_str(&format!(
                "commit refs/heads/many\ncommitter T <t@example.com> {} +0000\ndata 4\nsame\n\n",
                1_700_000_000 + i
            ));
        }
        let file = p.join("stream");
        std::fs::write(&file, stream).unwrap();
        let status = std::process::Command::new(super::super::gitstatus::program())
            .arg("-C")
            .arg(p)
            .args(["fast-import", "--quiet"])
            .stdin(std::fs::File::open(&file).unwrap())
            .status()
            .unwrap();
        assert!(status.success() && !tree.is_empty());
        let query = CommitQuery {
            message: "same".into(),
            ..Default::default()
        };
        let lines = Cell::new(0);
        let add = |text: String| lines.set(lines.get() + text.lines().count());
        let end = search(p, &query, false, None, &|| false, add).unwrap();
        assert!(end.truncated && end.count == MAX_HITS && lines.get() == MAX_HITS);
        lines.set(0);
        let end = search(p, &query, false, None, &|| true, add).unwrap();
        assert!(end.count == 0 && lines.get() == 0);
    }

    /// Two commits and two stashes: one of tracked changes ("needle one"), one with a new file ("second").
    fn stashed() -> (tempfile::TempDir, Vec<String>, Vec<String>) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(p, &["init", "-q", "-b", "main"]).ok();
            git(p, &["config", key, value]).unwrap();
        }
        let write = |name: &str, text: &str| std::fs::write(p.join(name), text).unwrap();
        write("a.txt", "one\n");
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "one"]).unwrap();
        write("a.txt", "two\n");
        git(p, &["commit", "-qam", "two"]).unwrap();
        write("a.txt", "tracked change\n");
        git(p, &["stash", "push", "-q", "-m", "needle one"]).unwrap();
        write("a.txt", "second change\n");
        write("new.txt", "untracked\n");
        git(
            p,
            &["stash", "push", "-q", "--include-untracked", "-m", "second"],
        )
        .unwrap();
        let ids: Vec<String> = git(p, &["stash", "list", "--format=%H"])
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        let mut helpers = Vec::new();
        for id in &ids {
            for parent in ["^2", "^3"] {
                if let Ok(h) = git(
                    p,
                    &["rev-parse", "--verify", "-q", &format!("{id}{parent}")],
                ) {
                    helpers.push(h.trim().to_owned());
                }
            }
        }
        (d, ids, helpers)
    }

    fn hits(
        path: &Path,
        query: CommitQuery,
        reflog: bool,
        reference: Option<&str>,
    ) -> Vec<Vec<String>> {
        let mut all = String::new();
        let end = search(path, &query, reflog, reference, &|| false, |lines| {
            all.push_str(&lines)
        })
        .unwrap();
        let rows: Vec<Vec<String>> = all
            .lines()
            .map(|line| line.split('\0').map(str::to_owned).collect())
            .collect();
        assert_eq!(rows.len(), end.count);
        rows
    }

    fn query_message(text: &str) -> CommitQuery {
        CommitQuery {
            message: text.into(),
            ..CommitQuery::default()
        }
    }

    #[test]
    fn a_search_finds_each_live_stash_once_as_a_stash_row_and_never_its_helper_commits() {
        let (d, ids, helpers) = stashed();
        let p = d.path();
        // Only the first stash has the word; it is found once, in the format of a stash row.
        let rows = hits(p, query_message("needle"), false, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0][0].as_str(), rows[0].len(), rows[0][6].as_str()),
            (ids[1].as_str(), 7, "stash@{1}")
        );
        // "on main" is in both stash messages and in the helper commits ("index on main", "untracked files on main").
        let rows = hits(p, query_message("on main"), false, None);
        let found: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
        for id in &ids {
            assert_eq!(found.iter().filter(|f| *f == id).count(), 1, "{found:?}");
        }
        assert!(
            helpers.iter().all(|h| !found.contains(&h.as_str())),
            "a helper commit matched: {found:?}"
        );
        assert!(rows
            .iter()
            .filter(|r| ids.contains(&r[0]))
            .all(|r| r.len() == 7 && !r[1].contains(' ')));
        // A path filter applies to the stashed changes.
        let rows = hits(
            p,
            CommitQuery {
                path: "a.txt".into(),
                ..CommitQuery::default()
            },
            false,
            None,
        );
        let found: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
        assert!(
            ids.iter()
                .all(|id| found.iter().filter(|f| *f == id).count() == 1),
            "{found:?}"
        );
    }

    #[test]
    fn reflog_mode_and_a_scoped_search_treat_stashes_as_above() {
        let (d, ids, helpers) = stashed();
        let p = d.path();
        let rows = hits(p, query_message("on main"), true, None);
        let found: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
        for id in &ids {
            assert_eq!(found.iter().filter(|f| *f == id).count(), 1, "{found:?}");
        }
        assert!(
            helpers.iter().all(|h| !found.contains(&h.as_str())),
            "{found:?}"
        );
        // A search scoped to a reference does not know the stash stack.
        let scoped = hits(p, query_message("on main"), false, Some("refs/heads/main"));
        assert!(scoped.iter().all(|r| !ids.contains(&r[0]) && r.len() == 6));
    }

    #[test]
    fn without_stashes_nothing_changes() {
        let d = repository();
        let p = d.path();
        let rows = hits(p, query_message("login"), false, None);
        assert!(!rows.is_empty() && rows.iter().all(|r| r.len() == 6));
    }
}
