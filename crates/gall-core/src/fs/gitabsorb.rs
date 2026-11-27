//! Absorb: puts staged hunks into the commits they belong to.
//!
//! A hunk belongs to a commit when all lines it replaces come from it; anything else is left
//! staged. The commits are rewritten as objects without touching the working files or the real
//! index. HEAD moves only when the result has exactly the old HEAD with the chosen hunks.
use super::gitgraph::{git, git_with_index, root};
use super::gitinspect::{read_git, relative_file};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

/// Commits looked at, and the most that are rewritten.
const MAX_STACK: usize = 100;
const MAX_DIFF: usize = 4 * 1024 * 1024;
const MAX_COMMIT: usize = 1024 * 1024;

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// A commit that may receive hunks: on the linear history of HEAD and not pushed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackCommit {
    pub id: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub index: usize,
    pub file: String,
    /// Lines before and after the change, 1-based; `count` 0 is an insertion or a deletion.
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// The changed lines, with `-` and `+`.
    pub lines: Vec<String>,
    /// The commit it belongs to.
    pub target: Option<String>,
    /// Certain: it replaces lines of that commit. Else it is an insertion between two of them.
    pub certain: bool,
    /// Why it has no target.
    pub reason: Option<String>,
    /// It can be put into a commit that the user chooses.
    pub movable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub token: String,
    pub hunks: Vec<Hunk>,
    /// Newest first.
    pub stack: Vec<StackCommit>,
    /// Why there is no commit to put anything into.
    pub reason: Option<String>,
}

/// A hunk and the commit it goes into.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    pub hunk: usize,
    pub target: String,
}

/// A changed file of the staged diff.
struct File {
    name: String,
    header: String,
    movable: bool,
    hunks: Vec<(Range, String)>,
}

#[derive(Clone, Copy)]
struct Range {
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
}

fn parse_range(header: &str) -> Option<Range> {
    let mut parts = header.strip_prefix("@@ -")?.split(' ');
    let pair = |text: &str| -> Option<(u32, u32)> {
        match text.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((text.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = pair(parts.next()?)?;
    let (new_start, new_count) = pair(parts.next()?.strip_prefix('+')?)?;
    Some(Range {
        old_start,
        old_count,
        new_start,
        new_count,
    })
}

fn parse_diff(text: &str) -> Vec<File> {
    let mut files: Vec<File> = Vec::new();
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            files.push(File {
                name: String::new(),
                header: line.to_owned(),
                movable: true,
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if let Some(range) = parse_range(line) {
            file.hunks.push((range, line.to_owned()));
        } else if let Some((_, hunk)) = file.hunks.last_mut() {
            hunk.push_str(line);
        } else {
            let name = line
                .strip_prefix("+++ b/")
                .or_else(|| line.strip_prefix("--- a/"));
            if let Some(name) = name {
                file.name = name.trim_end_matches(['\n', '\r', '\t']).to_owned();
            }
            if [
                "new file mode",
                "deleted file mode",
                "old mode",
                "new mode",
                "Binary files",
                "GIT binary patch",
            ]
            .iter()
            .any(|start| line.starts_with(start))
            {
                file.movable = false;
            }
            file.header.push_str(line);
        }
    }
    for file in &mut files {
        if file.hunks.is_empty() {
            file.movable = false;
        }
        if file.name.is_empty() {
            // Only a mode or a binary file: named in the first line.
            file.name = file
                .header
                .lines()
                .next()
                .and_then(|line| line.rsplit_once(" b/"))
                .map(|(_, name)| name.to_owned())
                .unwrap_or_default();
        }
    }
    files
}

/// The staged changes without context, so that every hunk is one change.
fn staged_diff(root: &Path) -> Result<String> {
    let (bytes, cut) = read_git(
        root,
        &[
            "-c",
            "core.quotePath=false",
            "-c",
            "diff.noprefix=false",
            "-c",
            "diff.mnemonicPrefix=false",
            "diff",
            "--cached",
            "-U0",
            "--inter-hunk-context=0",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--ignore-submodules=all",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "HEAD",
            "--",
        ],
        MAX_DIFF,
    )?;
    if cut {
        return Err(fail("The staged changes are too large to absorb"));
    }
    String::from_utf8(bytes).map_err(|_| fail("The staged changes contain invalid UTF-8"))
}

fn token(root: &Path, diff: &str) -> Result<String> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    super::gitrepository::snapshot(root)?.token.hash(&mut hash);
    diff.hash(&mut hash);
    Ok(format!("{:016x}", hash.finish()))
}

struct Stack {
    commits: Vec<StackCommit>,
    /// Parent of each commit.
    parents: HashMap<String, String>,
    reason: Option<String>,
}

/// The commits of HEAD up to the first merge, the first commit or what is pushed, newest first.
fn stack(root: &Path, head: &str) -> Result<Stack> {
    let upstream = git(
        root,
        &["rev-parse", "-q", "--verify", "@{upstream}^{commit}"],
    )
    .ok();
    let count = format!("--max-count={MAX_STACK}");
    let mut args = vec![
        "log",
        "--no-show-signature",
        "--topo-order",
        &count,
        "--format=%H%x00%P%x00%s",
        head,
    ];
    let exclude = upstream.as_deref().map(|id| format!("^{}", id.trim()));
    if let Some(exclude) = &exclude {
        args.push(exclude);
    }
    args.extend(["--not", "--remotes", "--"]);
    let listed = git(root, &args)?;
    let mut found = Stack {
        commits: Vec::new(),
        parents: HashMap::new(),
        reason: None,
    };
    let mut wanted = head.to_owned();
    for line in listed.lines() {
        let mut fields = line.splitn(3, '\0');
        let (Some(id), Some(parents), Some(subject)) =
            (fields.next(), fields.next(), fields.next())
        else {
            break;
        };
        // Not linear any more, or the first commit, which has nothing to be rewritten on.
        if id != wanted || parents.is_empty() || parents.contains(' ') {
            break;
        }
        found.parents.insert(id.to_owned(), parents.to_owned());
        found.commits.push(StackCommit {
            id: id.to_owned(),
            subject: subject.to_owned(),
        });
        wanted = parents.to_owned();
    }
    if found.commits.is_empty() {
        found.reason = Some(if listed.trim().is_empty() {
            "Every commit is pushed already".into()
        } else {
            "HEAD is a merge or the first commit".into()
        });
    }
    Ok(found)
}

/// The commit of every line of a file in HEAD.
fn line_commits(
    root: &Path,
    head: &str,
    file: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<String>> {
    let args: Vec<String> = [
        "-c",
        // A commit that is skipped here would get lines that are not its own.
        "blame.ignoreRevsFile=",
        "blame",
        "--incremental",
        "--no-textconv",
        head,
        "--",
        file,
    ]
    .map(String::from)
    .to_vec();
    let mut parser = super::gitblame::Parser::default();
    let mut lines: Vec<String> = Vec::new();
    super::gitsearch::stream(root, &args, cancelled, |bytes| {
        for range in parser.feed(&String::from_utf8_lossy(&bytes)).ranges {
            let last = (range.line - 1 + range.count) as usize;
            if lines.len() < last {
                lines.resize(last, String::new());
            }
            lines[(range.line - 1) as usize..last].fill(range.commit);
        }
        true
    })?;
    Ok(lines)
}

/// The commit that a change belongs to, and whether that is certain.
fn owner(range: Range, lines: &[String]) -> std::result::Result<(String, bool), &'static str> {
    let same = |ids: &[&String]| ids.windows(2).all(|pair| pair[0] == pair[1]);
    if range.old_count > 0 {
        let from = range.old_start.saturating_sub(1) as usize;
        let own: Vec<&String> = lines
            .iter()
            .skip(from)
            .take(range.old_count as usize)
            .collect();
        if own.len() != range.old_count as usize || own.iter().any(|id| id.is_empty()) {
            return Err("It is not known where its lines come from");
        }
        if !same(&own) {
            return Err("Its lines come from several commits");
        }
        return Ok((own[0].clone(), true));
    }
    // An insertion: between lines of one commit. `old_start` is the line before it.
    let at = range.old_start as usize;
    let around: Vec<&String> = [at.checked_sub(1), Some(at)]
        .into_iter()
        .flatten()
        .filter_map(|i| lines.get(i))
        .collect();
    match around.as_slice() {
        [] => Err("It is not known where its lines come from"),
        around if !same(around) => Err("It is inserted between lines of two commits"),
        [only, ..] => Ok(((*only).clone(), false)),
    }
}

/// What absorbing the staged changes would do. Changes nothing.
pub fn propose(path: &Path) -> Result<Proposal> {
    propose_inner(path, &|| false, false)
}

static CACHE: Mutex<Option<(PathBuf, Proposal)>> = Mutex::new(None);

pub fn propose_cancellable(path: &Path, cancelled: &dyn Fn() -> bool) -> Result<Proposal> {
    propose_inner(path, cancelled, true)
}

fn propose_inner(path: &Path, cancelled: &dyn Fn() -> bool, cached: bool) -> Result<Proposal> {
    let check = || {
        if cancelled() {
            Err(fail("Absorb proposal cancelled"))
        } else {
            Ok(())
        }
    };
    check()?;
    let root = root(path)?;
    let head = head(&root)?;
    let diff = staged_diff(&root)?;
    let stack = stack(&root, &head)?;
    let token = token(&root, &diff)?;
    check()?;
    if cached {
        if let Some(hit) = CACHE.lock().ok().and_then(|cache| {
            cache
                .as_ref()
                .filter(|(path, proposal)| {
                    path == &root
                        && proposal.token == token
                        && proposal.stack == stack.commits
                        && proposal.reason == stack.reason
                })
                .map(|(_, proposal)| proposal.clone())
        }) {
            return Ok(hit);
        }
    }
    let known: BTreeSet<&str> = stack.commits.iter().map(|c| c.id.as_str()).collect();
    let parsed_files = parse_diff(&diff);
    let total_files = parsed_files.len() as u64;
    if total_files > 0 {
        super::gitprogress::report_counts(
            "Analyzing staged changes",
            0,
            Some(total_files),
            "files",
            None,
        );
    }
    let mut hunks = Vec::new();
    for (file_idx, file) in parsed_files.into_iter().enumerate() {
        let movable = file.movable && relative_file(&file.name).is_ok();
        super::gitprogress::report_counts(
            "Analyzing staged changes",
            file_idx as u64,
            Some(total_files),
            "files",
            Some(file.name.clone()),
        );
        let lines = if movable && stack.reason.is_none() {
            line_commits(&root, &head, &file.name, cancelled).unwrap_or_default()
        } else {
            Vec::new()
        };
        check()?;
        if file.hunks.is_empty() {
            hunks.push(Hunk {
                index: hunks.len(),
                file: file.name.clone(),
                old_start: 0,
                old_count: 0,
                new_start: 0,
                new_count: 0,
                lines: Vec::new(),
                target: None,
                certain: false,
                reason: Some("The file is binary or only its mode changed".into()),
                movable: false,
            });
            continue;
        }
        for (range, text) in &file.hunks {
            let (target, certain, reason) = if !movable {
                (
                    None,
                    false,
                    Some("The file is new, deleted, binary or its mode changed".to_owned()),
                )
            } else if let Some(reason) = &stack.reason {
                (None, false, Some(reason.clone()))
            } else {
                match owner(*range, &lines) {
                    Ok((id, certain)) if known.contains(id.as_str()) => (Some(id), certain, None),
                    Ok(_) => (
                        None,
                        false,
                        Some("Its commit is pushed, behind a merge or too far back".to_owned()),
                    ),
                    Err(reason) => (None, false, Some(reason.to_owned())),
                }
            };
            hunks.push(Hunk {
                index: hunks.len(),
                file: file.name.clone(),
                old_start: range.old_start,
                old_count: range.old_count,
                new_start: range.new_start,
                new_count: range.new_count,
                lines: text.lines().skip(1).map(str::to_owned).collect(),
                target,
                certain,
                reason,
                movable,
            });
        }
    }
    let proposal = Proposal {
        token,
        hunks,
        stack: stack.commits,
        reason: stack.reason,
    };
    if cached {
        if let Ok(mut cache) = CACHE.lock() {
            *cache = Some((root, proposal.clone()));
        }
    }
    Ok(proposal)
}

fn head(root: &Path) -> Result<String> {
    git(root, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])
        .map(|id| id.trim().to_owned())
        .map_err(|_| fail("Create a commit before absorbing changes"))
}

/// A patch against HEAD of the hunks in `chosen` (numbered over all files).
fn patch_of(files: &[File], chosen: &BTreeSet<usize>) -> String {
    let mut text = String::new();
    let mut index = 0usize;
    for file in files {
        let first = index;
        index += file.hunks.len().max(1);
        let mut delta = 0i64;
        let mut open = false;
        for (at, (range, hunk)) in file.hunks.iter().enumerate() {
            if !chosen.contains(&(first + at)) {
                continue;
            }
            if !open {
                text.push_str(&file.header);
                open = true;
            }
            // Where the change begins: lines before it, then the same after the hunks above it.
            let before = i64::from(range.old_start) - i64::from(range.old_count > 0) + delta;
            let start = before + i64::from(range.new_count > 0);
            text.push_str(&format!(
                "@@ -{},{} +{start},{} @@\n",
                range.old_start, range.old_count, range.new_count
            ));
            text.push_str(hunk.split_once('\n').map_or("", |(_, body)| body));
            delta += i64::from(range.new_count) - i64::from(range.old_count);
        }
    }
    text
}

/// Writes a commit object.
fn write_commit(root: &Path, bytes: &[u8]) -> Result<String> {
    let mut command = super::gitexec::command(root, super::gitstatus::program());
    command
        .arg("-C")
        .arg(super::gitexec::dir_arg(root))
        .args(["hash-object", "-t", "commit", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0");
    super::gitexec::forward_env(&mut command);
    let mut child = command.spawn().map_err(|e| fail(e.to_string()))?;
    let mut input = child.stdin.take().ok_or_else(|| fail("No Git input"))?;
    let written = input.write_all(bytes);
    drop(input);
    let output = child.wait_with_output().map_err(|e| fail(e.to_string()))?;
    written.map_err(|e| fail(e.to_string()))?;
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || !matches!(id.len(), 40 | 64) {
        return Err(fail(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(id)
}

/// The commit `raw` with another tree and parent. Its author, message and encoding stay byte for
/// byte; a signature would not match any more and is left out.
fn with_tree(raw: &[u8], tree: &str, parent: &str, committer: &str) -> Result<Vec<u8>> {
    let end = raw
        .windows(2)
        .position(|pair| pair == b"\n\n")
        .ok_or_else(|| fail("A commit has no message"))?;
    let mut result = Vec::with_capacity(raw.len());
    let mut skipping = false;
    for line in raw[..end].split(|b| *b == b'\n') {
        if line.first() == Some(&b' ') {
            if !skipping {
                result.extend_from_slice(line);
                result.push(b'\n');
            }
            continue;
        }
        skipping = line.starts_with(b"gpgsig") || line.starts_with(b"mergetag");
        if skipping {
            continue;
        }
        if line.starts_with(b"tree ") {
            result.extend_from_slice(format!("tree {tree}").as_bytes());
        } else if line.starts_with(b"parent ") {
            result.extend_from_slice(format!("parent {parent}").as_bytes());
        } else if line.starts_with(b"committer ") {
            result.extend_from_slice(format!("committer {committer}").as_bytes());
        } else {
            result.extend_from_slice(line);
        }
        result.push(b'\n');
    }
    result.extend_from_slice(&raw[end + 1..]);
    Ok(result)
}

/// Puts staged hunks into commits. Returns how many commits were rewritten.
pub fn apply(path: &Path, assignments: &[Assignment], expected: &str) -> Result<usize> {
    let targets: BTreeSet<&str> = assignments.iter().map(|a| a.target.as_str()).collect();
    let label = format!("{} commits", targets.len());
    let operation = Some((super::gitjournal::Kind::Absorb, label));
    super::gitjournal::around(path, operation, || apply_now(path, assignments, expected))
}

fn apply_now(path: &Path, assignments: &[Assignment], expected: &str) -> Result<usize> {
    let _busy = super::gitfacts::mutating();
    let root = root(path)?;
    let root = root.as_path();
    let state = super::gitrepository::snapshot(root)?;
    if state.operation.is_some() || !state.conflicts.is_empty() {
        return Err(fail("Finish or abort the current Git operation first"));
    }
    let old_head = head(root)?;
    let diff = staged_diff(root)?;
    if token(root, &diff)? != expected {
        return Err(fail(
            "The staged changes or the repository changed. Review them again",
        ));
    }
    let files = parse_diff(&diff);
    let stack = stack(root, &old_head)?;
    let mut groups: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    let total: usize = files.iter().map(|file| file.hunks.len().max(1)).sum();
    let mut movable = Vec::with_capacity(total);
    for file in &files {
        let able = file.movable && relative_file(&file.name).is_ok();
        movable.extend(std::iter::repeat_n(able, file.hunks.len().max(1)));
    }
    let mut seen = BTreeSet::new();
    for assignment in assignments {
        if movable.get(assignment.hunk) != Some(&true) || !seen.insert(assignment.hunk) {
            return Err(fail("A chosen change cannot be put into a commit"));
        }
        let at = stack
            .commits
            .iter()
            .position(|commit| commit.id == assignment.target)
            .ok_or_else(|| fail("The chosen commit cannot be rewritten"))?;
        groups.entry(at).or_default().insert(assignment.hunk);
    }
    if groups.is_empty() {
        return Err(fail("Choose changes to absorb"));
    }
    // Build the cumulative fixups in a private index on the same filesystem as Git.
    let git_dir = super::gitadvanced::worktree_git_dir(root)
        .ok_or_else(|| fail("The Git directory is not readable"))?;
    let temporary = tempfile::Builder::new()
        .prefix("gall-absorb-")
        .tempdir_in(git_dir)
        .map_err(|e| fail(format!("Cannot create a temporary index: {e}")))?;
    let index = temporary.path().join("index");
    let mut made: Vec<(usize, String, String)> = Vec::new();
    let mut chosen = BTreeSet::new();
    let mut fixup_parent = old_head.clone();
    let oldest = *groups.keys().next_back().unwrap_or(&0);
    let total_steps = (groups.len() + oldest + 1) as u64;
    let mut done_steps = 0u64;
    super::gitprogress::report_counts("Absorbing changes", 0, Some(total_steps), "steps", None);
    for (at, hunks) in &groups {
        chosen.extend(hunks.iter().copied());
        git_with_index(root, &["read-tree", &old_head], Some(&index))?;
        super::gitworktree::apply_cached_to(root, &patch_of(&files, &chosen), &index)?;
        let tree = git_with_index(root, &["write-tree"], Some(&index))?
            .trim()
            .to_owned();
        let commit = git(
            root,
            &["commit-tree", &tree, "-p", &fixup_parent, "-m", "absorb"],
        )?
        .trim()
        .to_owned();
        made.push((*at, commit.clone(), tree));
        fixup_parent = commit;
        done_steps += 1;
        super::gitprogress::report_counts(
            "Absorbing changes",
            done_steps,
            Some(total_steps),
            "steps",
            None,
        );
    }
    let wanted = made
        .last()
        .map(|(_, _, tree)| tree.clone())
        .unwrap_or_default();
    let committer = git(root, &["var", "GIT_COMMITTER_IDENT"])?;
    let committer = committer.trim();
    let mut parent = stack
        .parents
        .get(&stack.commits[oldest].id)
        .cloned()
        .ok_or_else(|| fail("The chosen commit cannot be rewritten"))?;
    let mut tree = String::new();
    for at in (0..=oldest).rev() {
        let commit = &stack.commits[at];
        done_steps += 1;
        super::gitprogress::report_counts(
            "Rewriting commits",
            done_steps,
            Some(total_steps),
            "steps",
            Some(commit.id[..8.min(commit.id.len())].to_owned()),
        );
        let conflict = |with: &StackCommit| {
            fail(format!(
                "The changes do not fit: {} ({}) would conflict. Nothing was changed",
                &with.id[..8],
                with.subject
            ))
        };
        let (raw, cut) = read_git(root, &["cat-file", "commit", &commit.id], MAX_COMMIT)?;
        if cut {
            return Err(fail("A commit is too large to rewrite"));
        }
        tree = if at == oldest {
            git(root, &["rev-parse", &format!("{}^{{tree}}", commit.id)])?
                .trim()
                .to_owned()
        } else {
            let base = &stack.parents[&commit.id];
            match super::gitrewrite::merge_trees(root, base, &parent, &commit.id)? {
                (tree, None) => tree,
                (_, Some(_)) => return Err(conflict(commit)),
            }
        };
        let mut new = write_commit(root, &with_tree(&raw, &tree, &parent, committer)?)?;
        if let Some(k) = made.iter().position(|(target, _, _)| *target == at) {
            let base = if k == 0 { &old_head } else { &made[k - 1].1 };
            tree = match super::gitrewrite::merge_trees(root, base, &new, &made[k].1)? {
                (tree, None) => tree,
                (_, Some(_)) => return Err(conflict(commit)),
            };
            new = write_commit(root, &with_tree(&raw, &tree, &parent, committer)?)?;
        }
        parent = new;
    }
    // Exactly the old files with the chosen changes, or nothing happens.
    if tree != wanted {
        return Err(fail(
            "The rewritten commits would not give the same files. Nothing was changed",
        ));
    }
    if head(root)? != old_head
        || token(root, &staged_diff(root)?)? != expected
        || self::stack(root, &old_head)?.commits != stack.commits
    {
        return Err(fail(
            "The staged changes or the repository changed. Review them again",
        ));
    }
    git(
        root,
        &[
            "update-ref",
            "--no-deref",
            &format!("refs/gall/recovery/{old_head}"),
            &old_head,
        ],
    )?;
    // The branch moves only if it still is where it was.
    git(
        root,
        &[
            "update-ref",
            "-m",
            "gall: absorb",
            "HEAD",
            &parent,
            &old_head,
        ],
    )?;
    super::gitgraph::drop_recovery(root, &old_head);
    Ok(oldest + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "Test"],
            &["config", "user.email", "test@example.com"],
            &["config", "core.autocrlf", "false"],
        ] {
            git(d.path(), args).unwrap();
        }
        d
    }

    fn write(p: &Path, file: &str, lines: &[&str]) {
        std::fs::write(p.join(file), lines.join("\n") + "\n").unwrap();
    }

    fn commit(p: &Path, message: &str) -> String {
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", message]).unwrap();
        head(p).unwrap()
    }

    fn show(p: &Path, spec: &str) -> String {
        git(p, &["show", spec]).unwrap()
    }

    /// Three commits, each with lines of its own in one file.
    fn stacked() -> (tempfile::TempDir, [String; 3]) {
        let d = repository();
        let p = d.path();
        write(p, "base.txt", &["base"]);
        commit(p, "base");
        write(p, "f.txt", &["a1", "a2", "a3"]);
        let a = commit(p, "A: subject\n\nbody of A");
        write(p, "f.txt", &["a1", "a2", "a3", "b1", "b2", "b3"]);
        let b = commit(p, "B");
        write(
            p,
            "f.txt",
            &["a1", "a2", "a3", "b1", "b2", "b3", "c1", "c2"],
        );
        let c = commit(p, "C");
        (d, [a, b, c])
    }

    fn all(proposal: &Proposal) -> Vec<Assignment> {
        proposal
            .hunks
            .iter()
            .filter_map(|hunk| {
                Some(Assignment {
                    hunk: hunk.index,
                    target: hunk.target.clone()?,
                })
            })
            .collect()
    }

    #[test]
    fn hunks_go_into_their_commits_and_ambiguous_ones_stay_staged() {
        let (d, [a, b, c]) = stacked();
        let p = d.path();
        // A mark of the index that reading a tree into it would lose.
        git(p, &["update-index", "--skip-worktree", "base.txt"]).unwrap();
        // One change in A, one in B, one across B and C, an insertion inside A and a new file.
        write(
            p,
            "f.txt",
            &["A1", "a2", "in", "a3", "b1", "B2", "X", "Y", "c2"],
        );
        write(p, "new.txt", &["new"]);
        git(p, &["add", "--all"]).unwrap();
        // Unstaged work and an untracked file are never looked at.
        write(p, "base.txt", &["mine"]);
        write(p, "untracked.txt", &["mine"]);
        let before = git(p, &["status", "--porcelain"]).unwrap();
        let proposal = propose(p).unwrap();
        let targets: Vec<_> = proposal
            .hunks
            .iter()
            .filter(|hunk| hunk.file == "f.txt")
            .map(|hunk| (hunk.target.clone(), hunk.certain))
            .collect();
        assert_eq!(
            targets,
            [
                (Some(a.clone()), true),
                (Some(a.clone()), false),
                // B2 and the lines after it are one change, of two commits.
                (None, false),
            ]
        );
        assert!(proposal
            .hunks
            .iter()
            .any(|hunk| hunk.file == "new.txt" && !hunk.movable));
        assert_eq!(
            proposal
                .stack
                .iter()
                .map(|commit| commit.id.clone())
                .collect::<Vec<_>>(),
            [c.clone(), b.clone(), a.clone()]
        );
        // A stale token changes nothing.
        assert!(apply(p, &all(&proposal), "0").is_err());
        assert_eq!(head(p).unwrap(), c);
        assert_eq!(apply(p, &all(&proposal), &proposal.token).unwrap(), 3);
        assert_eq!(show(p, "HEAD~2:f.txt"), "A1\na2\nin\na3\n");
        assert_eq!(show(p, "HEAD~1:f.txt"), "A1\na2\nin\na3\nb1\nb2\nb3\n");
        let log = git(p, &["log", "--format=%s|%b|%an", "-4"]).unwrap();
        assert!(log.contains("A: subject|body of A") && log.contains("C|"));
        // What was not absorbed is still staged, and everything else is as it was.
        assert_eq!(show(p, ":f.txt"), "A1\na2\nin\na3\nb1\nB2\nX\nY\nc2\n");
        assert_eq!(git(p, &["status", "--porcelain"]).unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(p.join("base.txt")).unwrap(),
            "mine\n"
        );
        assert!(git(p, &["ls-files", "-t", "base.txt"])
            .unwrap()
            .starts_with("S "));
        // Undo gives the hunks back as staged.
        let applied = super::super::gitjournal::view(p).unwrap().applied;
        super::super::gitjournal::move_to(p, applied - 1, applied).unwrap();
        assert_eq!(head(p).unwrap(), c);
        assert_eq!(git(p, &["status", "--porcelain"]).unwrap(), before);
        assert_eq!(propose(p).unwrap().hunks.len(), proposal.hunks.len());
    }

    #[test]
    fn absorb_uses_staged_filtered_bytes_and_does_not_rewrite_local_files() {
        let d = repository();
        let p = d.path();
        write(p, ".gitattributes", &["f.txt filter=roundtrip"]);
        write(p, "base.txt", &["base"]);
        commit(p, "Base");
        write(p, "f.txt", &["repository old", "keep"]);
        let target = commit(p, "Add filtered text");
        for (key, value) in [
            ("filter.roundtrip.clean", "sed 's/working/repository/g'"),
            ("filter.roundtrip.smudge", "sed 's/repository/working/g'"),
            ("filter.roundtrip.required", "true"),
        ] {
            git(p, &["config", key, value]).unwrap();
        }
        write(p, "f.txt", &["working staged", "keep"]);
        git(p, &["add", "--", "f.txt"]).unwrap();
        write(p, "f.txt", &["working unstaged", "keep"]);
        write(p, "untracked.txt", &["preserve"]);
        let proposal = propose(p).unwrap();
        let assignments = [Assignment {
            hunk: proposal.hunks[0].index,
            target: target.clone(),
        }];
        let before = show(p, ":f.txt");
        assert!(apply(p, &assignments, "stale").is_err());
        assert_eq!(head(p).unwrap(), target);
        assert_eq!(show(p, ":f.txt"), before);
        assert_eq!(apply(p, &assignments, &proposal.token).unwrap(), 1);
        assert_eq!(show(p, "HEAD:f.txt"), "repository staged\nkeep\n");
        assert_eq!(show(p, ":f.txt"), before);
        assert_eq!(
            std::fs::read_to_string(p.join("f.txt")).unwrap(),
            "working unstaged\nkeep\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("untracked.txt")).unwrap(),
            "preserve\n"
        );
    }

    #[test]
    fn a_chosen_commit_takes_hunks_only_when_they_fit() {
        let (d, [a, _, c]) = stacked();
        let p = d.path();
        write(
            p,
            "f.txt",
            &["a1", "a2", "a3", "b1", "b2", "b3", "c1", "C2"],
        );
        git(p, &["add", "--all"]).unwrap();
        let proposal = propose(p).unwrap();
        assert_eq!(proposal.hunks[0].target.as_ref(), Some(&c));
        let into = |target: &str| {
            apply(
                p,
                &[Assignment {
                    hunk: 0,
                    target: target.to_owned(),
                }],
                &proposal.token,
            )
        };
        let staged = show(p, ":f.txt");
        // A has no such line: refused without replacing Git's index or someone else's lock.
        let index = p.join(".git").join("index");
        let before_index = std::fs::read(&index).unwrap();
        let lock = index.with_file_name("index.lock");
        std::fs::write(&lock, b"another Git command").unwrap();
        assert!(into(&a).unwrap_err().to_string().contains("Nothing was"));
        assert_eq!(std::fs::read(&index).unwrap(), before_index);
        assert_eq!(std::fs::read(&lock).unwrap(), b"another Git command");
        std::fs::remove_file(lock).unwrap();
        assert!(into(&"1".repeat(40)).is_err());
        assert_eq!(head(p).unwrap(), c);
        assert_eq!(show(p, ":f.txt"), staged);
        assert_eq!(into(&c).unwrap(), 1);
        assert_eq!(show(p, "HEAD:f.txt"), staged);
        assert_eq!(git(p, &["status", "--porcelain"]).unwrap(), "");
    }

    #[test]
    fn pushed_commits_merges_and_operations_are_left_alone() {
        let (d, [a, b, c]) = stacked();
        let p = d.path();
        let bare = tempfile::tempdir().unwrap();
        git(bare.path(), &["init", "-q", "--bare"]).unwrap();
        git(
            p,
            &["remote", "add", "origin", &bare.path().to_string_lossy()],
        )
        .unwrap();
        git(
            p,
            &["push", "-q", "origin", &format!("{b}:refs/heads/main")],
        )
        .unwrap();
        git(p, &["fetch", "-q", "origin"]).unwrap();
        git(p, &["branch", "-q", "--set-upstream-to=origin/main"]).unwrap();
        write(
            p,
            "f.txt",
            &["A1", "a2", "a3", "b1", "b2", "b3", "c1", "C2"],
        );
        git(p, &["add", "--all"]).unwrap();
        let proposal = propose(p).unwrap();
        assert_eq!(proposal.stack.len(), 1);
        assert_eq!(proposal.hunks[0].target, None);
        assert_eq!(proposal.hunks[1].target, Some(c.clone()));
        assert!(apply(p, &[Assignment { hunk: 0, target: a }], &proposal.token).is_err());
        git(p, &["push", "-q", "origin", "HEAD:refs/heads/main"]).unwrap();
        let proposal = propose(p).unwrap();
        assert!(proposal.reason.is_some() && all(&proposal).is_empty());
        std::fs::write(p.join(".git/MERGE_HEAD"), format!("{c}\n")).unwrap();
        assert!(apply(p, &[], "x").is_err());
    }

    #[test]
    fn patches_of_some_hunks_have_the_right_places() {
        let text = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1 +1,2 @@\n-a\n+A\n+AA\n@@ -3,0 +5 @@ x\n+new\n@@ -6,2 +8,0 @@\n-y\n-z\n";
        let files = parse_diff(text);
        assert_eq!(files[0].name, "f");
        let patch = |chosen: &[usize]| patch_of(&files, &chosen.iter().copied().collect());
        assert!(patch(&[0, 1, 2]).contains("@@ -3,0 +5,1 @@"));
        assert!(patch(&[1]).contains("@@ -3,0 +4,1 @@"));
        assert!(patch(&[1, 2]).contains("@@ -6,2 +6,0 @@"));
        assert!(patch(&[2]).contains("@@ -6,2 +5,0 @@"));
        assert_eq!(patch(&[]), "");
    }

    #[test]
    fn a_rewritten_commit_keeps_its_bytes_but_not_its_signature() {
        let raw = b"tree 1\nparent 2\nauthor A <a> 1 +0000\ncommitter B <b> 2 +0000\ngpgsig -----BEGIN\n more\n -----END\nencoding latin1\n\nsubject \xe9\n\nbody\n";
        let new = with_tree(raw, "T", "P", "C <c> 3 +0000").unwrap();
        assert_eq!(
            new,
            b"tree T\nparent P\nauthor A <a> 1 +0000\ncommitter C <c> 3 +0000\nencoding latin1\n\nsubject \xe9\n\nbody\n"
        );
    }
}
