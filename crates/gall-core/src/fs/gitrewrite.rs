//! A validated interactive-rebase plan. Git performs replay and retains resumable conflicts.
use super::gitgraph::{git, has_content_filters, root};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_PLAN: u64 = 8 * 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewritePlan {
    pub head: String,
    pub base: String,
    pub token: String,
    pub skipped_merges: usize,
    pub commits: Vec<RewriteCommit>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewriteCommit {
    pub id: String,
    pub subject: String,
    pub message: String,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RewriteAction {
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
    Edit,
}
impl RewriteAction {
    fn name(self) -> &'static str {
        match self {
            Self::Pick => "pick",
            Self::Reword => "reword",
            Self::Squash => "squash",
            Self::Fixup => "fixup",
            Self::Drop => "drop",
            Self::Edit => "edit",
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RewriteEntry {
    pub id: String,
    pub action: RewriteAction,
    #[serde(default)]
    pub message: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HelperPlan {
    schema: u8,
    root: String,
    git_dir: String,
    base: String,
    head: String,
    entries: Vec<RewriteEntry>,
}
fn commit_id(path: &Path, value: &str) -> Result<String> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(fail("Select a complete commit ID for the rebase base"));
    }
    let id = git(
        path,
        &["rev-parse", "--verify", &format!("{value}^{{commit}}")],
    )?;
    if id.trim() != value.to_ascii_lowercase() {
        return Err(fail("The selected object is not a commit"));
    }
    Ok(id.trim().into())
}
fn git_dir(path: &Path) -> Result<PathBuf> {
    let value = git(path, &["rev-parse", "--absolute-git-dir"])?;
    super::gitexec::host_path(path, value.trim())
        .canonicalize()
        .map_err(|e| fail(e.to_string()))
}
fn plan_token(snapshot: &super::gitrepository::Repository, base: &str) -> String {
    format!("{}:{base}", snapshot.token)
}
/// The base stays unchanged; every non-merge commit reachable from HEAD but not from the base appears
/// exactly once, oldest first. The base may be any commit, so a branch can be rebased onto another one.
/// Merge commits are flattened exactly as `git rebase -i` does without `--rebase-merges`.
pub fn plan(path: &Path, base: &str) -> Result<RewritePlan> {
    let root = root(path)?;
    // The commits of the plan only depend on HEAD, which Git's files name without a
    // process: they are read while the snapshot loads, and used if its HEAD is the same.
    let guess = super::gitfacts::head_id(&root);
    let (snapshot, early) = std::thread::scope(|scope| {
        let early = guess
            .as_deref()
            .map(|head| scope.spawn(|| plan_commits(&root, base, head)));
        (
            super::gitrepository::snapshot(&root),
            early.map(super::gitgraph::joined),
        )
    });
    let snapshot = snapshot?;
    if snapshot.head.is_empty() {
        return Err(fail("Create a commit before rewriting history"));
    }
    let (base, listed, skipped_merges) = match early {
        Some(Ok(Ok(found))) if guess.as_deref() == Some(snapshot.head.as_str()) => found,
        _ => plan_commits(&root, base, &snapshot.head)?,
    };
    let mut commits = Vec::new();
    let mut fields = listed.split('\0');
    while let Some(id) = fields.next() {
        // Git ends every commit of the list with a line break.
        let id = id.trim_start_matches('\n');
        if id.is_empty() {
            break;
        }
        let (Some(subject), Some(message)) = (fields.next(), fields.next()) else {
            return Err(fail("Invalid commit metadata"));
        };
        commits.push(RewriteCommit {
            id: id.into(),
            subject: subject.into(),
            message: message.trim_end_matches('\n').into(),
        });
    }
    if commits.len() > 1000 {
        return Err(fail("Select a range of at most 1000 commits"));
    }
    Ok(RewritePlan {
        head: snapshot.head.clone(),
        base: base.clone(),
        token: plan_token(&snapshot, &base),
        skipped_merges,
        commits,
    })
}
/// The checked base, the commits from it to `head` with their messages (`log` output) and the
/// number of merges among them, for [`plan`].
fn plan_commits(root: &Path, base: &str, head: &str) -> Result<(String, String, usize)> {
    let base = commit_id(root, base)?;
    let range = format!("{base}..{head}");
    // The commits with their messages in one query (it was one `show` per commit, a
    // thousand processes for the longest plan), next to the count of merges.
    let (listed, skipped_merges) = std::thread::scope(|scope| {
        let merges = scope.spawn(|| git(root, &["rev-list", "--count", "--merges", &range]));
        let listed = git(
            root,
            &[
                "log",
                "--no-show-signature",
                "--reverse",
                "--topo-order",
                "--no-merges",
                "--max-count=1001",
                "--format=%H%x00%s%x00%B%x00",
                &range,
            ],
        );
        (listed, super::gitgraph::joined(merges))
    });
    let skipped_merges = skipped_merges??.trim().parse().unwrap_or(0);
    Ok((base, listed?, skipped_merges))
}

/// A commit that would stop the rebase with a conflict, and the paths Git reports for it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictPrediction {
    pub id: String,
    pub files: Vec<String>,
}
/// Merges `theirs` into `ours` (commits or trees; `base` is any revision) the way a cherry-pick does. Returns the resulting tree (conflict
/// markers included when it conflicts) and the conflicted paths, or None when it merged cleanly.
fn merge_trees(
    root: &Path,
    base: &str,
    ours: &str,
    theirs: &str,
) -> Result<(String, Option<Vec<String>>)> {
    let mut cmd = super::gitexec::command(root, super::gitstatus::program());
    cmd.args(["-c", "core.fsmonitor=false", "-C"])
        .arg(super::gitexec::dir_arg(root))
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "-z",
        ])
        .arg(format!("--merge-base={base}"))
        .args([ours, theirs])
        .stdin(Stdio::null())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    super::gitexec::forward_env(&mut cmd);
    let out = match super::gitexec::run(&cmd, std::time::Duration::from_secs(120)) {
        Some(out) => out,
        None => cmd.output(),
    }
    .map_err(|e| fail(e.to_string()))?;
    let conflicted = match out.status.code() {
        Some(0) => false,
        Some(1) => true,
        _ => {
            let message = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            return Err(fail(if message.is_empty() {
                "Git could not check the commits for conflicts".to_owned()
            } else {
                message
            }));
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.split('\0');
    let tree = parts.next().unwrap_or_default().trim().to_owned();
    if tree.is_empty() {
        return Err(fail("Git returned no tree for the conflict check"));
    }
    let files = parts
        .take_while(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    Ok((tree, conflicted.then_some(files)))
}
/// [`merge_trees`] for every commit of `ids` in turn, starting from `start`, in one
/// `merge-tree --stdin` process: Git answers each input line (`<base> -- <ours> <theirs>`)
/// before it reads the next, so each merge can start from the tree of the one before. A
/// process per commit cost 30–40 ms each on Windows. None when this cannot be used (a
/// repository in WSL, whose Git runs through a shell without input, or a Git that refuses
/// the input); the caller then merges one commit per process, which also reports the error.
fn replay_streamed(root: &Path, start: &str, ids: &[String]) -> Option<Vec<ConflictPrediction>> {
    use std::io::BufRead;
    if ids.is_empty() {
        return Some(Vec::new());
    }
    if super::gitexec::place(root).is_some() {
        return None;
    }
    let mut cmd = super::gitexec::command(root, super::gitstatus::program());
    cmd.args(["-c", "core.fsmonitor=false", "-C"])
        .arg(super::gitexec::dir_arg(root))
        .args(["merge-tree", "--stdin", "--name-only", "--no-messages"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    let mut child = cmd.spawn().ok()?;
    let found = (|| {
        let mut input = child.stdin.take()?;
        let mut output = std::io::BufReader::new(child.stdout.take()?);
        let mut field = || -> Option<String> {
            let mut bytes = Vec::new();
            output.read_until(0, &mut bytes).ok()?;
            // A field always ends with NUL; anything else is the end of the output.
            (bytes.pop() == Some(0)).then_some(())?;
            String::from_utf8(bytes).ok()
        };
        let mut current = start.to_owned();
        let mut found = Vec::new();
        for id in ids {
            writeln!(input, "{id}^1 -- {current} {id}").ok()?;
            input.flush().ok()?;
            // `<status>NUL<tree>NUL` then the conflicted paths, each with NUL, and a NUL.
            let clean = match field()?.as_str() {
                "1" => true,
                "0" => false,
                _ => return None,
            };
            let tree = field()?;
            if tree.is_empty() {
                return None;
            }
            let mut files = Vec::new();
            loop {
                let name = field()?;
                if name.is_empty() {
                    break;
                }
                files.push(name);
            }
            if !clean {
                found.push(ConflictPrediction {
                    id: id.clone(),
                    files,
                });
            }
            current = tree;
        }
        drop(input);
        Some(found)
    })();
    // Without input Git ends; on a failure it may still wait for a line.
    if found.is_none() {
        let _ = child.kill();
    }
    let finished = child.wait().ok()?;
    found.filter(|_| finished.success())
}

/// Replays the entries in memory, in order, and reports every commit that would stop the rebase
/// with a conflict. Nothing in the working tree, index or any reference changes; Git only writes
/// unreferenced tree objects. Pick, reword, edit, squash and fixup all apply their changes to the
/// result so far, so they are checked alike; a dropped commit is skipped. After a conflict the
/// replay continues from the conflicted tree, so later reports assume the conflict gets resolved
/// without changing the commit's own side. Needs Git 2.40 or newer (`merge-tree --write-tree --merge-base`).
pub fn predict_conflicts(
    path: &Path,
    base: &str,
    entries: &[RewriteEntry],
) -> Result<Vec<ConflictPrediction>> {
    let root = root(path)?;
    if entries.len() > 1000 {
        return Err(fail("Select a range of at most 1000 commits"));
    }
    let mut ids = Vec::new();
    for entry in entries.iter().filter(|e| e.action != RewriteAction::Drop) {
        if !matches!(entry.id.len(), 40 | 64) || !entry.id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(fail("Invalid commit ID in the rebase plan"));
        }
        ids.push(entry.id.to_ascii_lowercase());
    }
    // The base is checked while the replay runs: it only writes unreferenced trees, and its
    // result is used only when the base is a commit.
    let (base, streamed) = std::thread::scope(|scope| {
        let checked = scope.spawn(|| commit_id(&root, base));
        let streamed = (base.len() == 40 || base.len() == 64)
            .then(|| replay_streamed(&root, &base.to_ascii_lowercase(), &ids))
            .flatten();
        (super::gitgraph::joined(checked), streamed)
    });
    let base = base??;
    if let Some(found) = streamed {
        return Ok(found);
    }
    let mut current = base;
    let mut found = Vec::new();
    for id in &ids {
        let (tree, conflict) = merge_trees(&root, &format!("{id}^1"), &current, id)?;
        if let Some(files) = conflict {
            found.push(ConflictPrediction {
                id: id.clone(),
                files,
            });
        }
        current = tree;
    }
    Ok(found)
}
fn validate_entries(plan: &RewritePlan, entries: &[RewriteEntry]) -> Result<()> {
    let expected: BTreeSet<_> = plan.commits.iter().map(|c| c.id.as_str()).collect();
    let actual: BTreeSet<_> = entries.iter().map(|c| c.id.as_str()).collect();
    if entries.is_empty() || entries.len() != expected.len() || actual != expected {
        return Err(fail(
            "The plan must contain every original commit exactly once; use Drop to remove a commit",
        ));
    }
    let mut prior = false;
    let mut message_size = 0usize;
    for entry in entries {
        if matches!(entry.action, RewriteAction::Squash | RewriteAction::Fixup) && !prior {
            return Err(fail("Squash and fixup need a preceding retained commit"));
        }
        if entry.action != RewriteAction::Drop {
            prior = true;
        }
        if entry.action == RewriteAction::Reword && entry.message.trim().is_empty() {
            return Err(fail(
                "A replacement message is required for every Reword entry",
            ));
        }
        if entry.message.contains('\0') {
            return Err(fail("Commit messages cannot contain NUL characters"));
        }
        message_size = message_size.saturating_add(entry.message.len());
    }
    if message_size > MAX_PLAN as usize / 2 {
        return Err(fail("The rebase messages are too large"));
    }
    Ok(())
}
fn clean_guard(path: &Path) -> Result<()> {
    let state = super::gitrepository::snapshot(path)?;
    if state.bisect_active {
        return Err(fail("Finish bisect before rewriting history"));
    }
    if state.operation.is_some() || !state.conflicts.is_empty() {
        return Err(fail(
            "Finish or abort the current Git operation before rewriting history",
        ));
    }
    if has_content_filters(path) {
        return Err(fail(
            "Interactive rebase with content filters requires an external Git client",
        ));
    }
    // Submodules are supported: a rebase rewrites the superproject's commits (a submodule is just a
    // recorded commit ID there) and never touches the submodule's own working folder. Edits and
    // untracked files inside a submodule are therefore safe and ignored here, but a changed
    // submodule commit (staged or not) is a local change of this repository like any other.
    if !git(
        path,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignore-submodules=dirty",
        ],
    )?
    .is_empty()
    {
        return Err(fail(
            "Commit or stash local changes before rewriting history (a submodule checked out at a different commit than this repository records counts as one)",
        ));
    }
    Ok(())
}
/// After a finished rebase: submodules whose checked-out commit is not the one the rewritten history
/// records. Gall never changes a submodule's folder, so the user decides when to update it.
fn submodule_note(path: &Path) -> String {
    let Ok(status) = git(path, &["submodule", "status"]) else {
        return String::new();
    };
    let moved: Vec<&str> = status
        .lines()
        .filter(|line| line.starts_with('+'))
        .filter_map(|line| line[1..].split_whitespace().nth(1))
        .collect();
    if moved.is_empty() {
        return String::new();
    }
    format!(
        "\nSubmodule folders were left as they were and no longer match the rewritten history: {}. Update them in Tools → Submodules when needed.",
        moved.join(", ")
    )
}
fn protect_ignored(path: &Path, plan: &RewritePlan) -> Result<()> {
    let other = git(path, &["ls-files", "--others", "-z"])?;
    if other.is_empty() {
        return Ok(());
    }
    if other.contains('\u{fffd}') {
        return Err(fail(
            "Untracked paths with invalid UTF-8 cannot be protected during rebase",
        ));
    }
    let mut touched = BTreeSet::new();
    for id in std::iter::once(plan.base.as_str()).chain(plan.commits.iter().map(|c| c.id.as_str()))
    {
        for file in git(path, &["ls-tree", "-r", "--name-only", "-z", id])?
            .split('\0')
            .filter(|s| !s.is_empty())
        {
            if file.contains('\u{fffd}') {
                return Err(fail(
                    "Historical paths with invalid UTF-8 cannot be protected during rebase",
                ));
            }
            touched.insert(file.to_owned());
        }
    }
    for file in other.split('\0').filter(|s| !s.is_empty()) {
        let prefix = format!("{file}/");
        let ancestor_collision = file
            .match_indices('/')
            .any(|(index, _)| touched.contains(&file[..index]));
        if touched.contains(file)
            || ancestor_collision
            || touched
                .range(prefix.clone()..)
                .next()
                .is_some_and(|s| s.starts_with(&prefix))
        {
            return Err(fail(format!("An ignored or untracked path may be overwritten during rebase: {file}. Move it aside before continuing")));
        }
    }
    Ok(())
}
fn io_context(what: &str) -> impl Fn(std::io::Error) -> CoreError + '_ {
    move |e| {
        let reason = if e.kind() == std::io::ErrorKind::NotFound {
            "the file or folder does not exist".to_owned()
        } else {
            e.to_string()
        };
        fail(format!("{what}: {reason}"))
    }
}
fn plain_file(path: &Path) -> Result<File> {
    let what = format!("Gall's saved rebase plan {} cannot be read", path.display());
    if !path
        .symlink_metadata()
        .map_err(io_context(&what))?
        .is_file()
    {
        return Err(fail("The rebase plan must be a regular file"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path).map_err(io_context(&what))
}
fn read_pointer(path: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    plain_file(path)?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| fail(e.to_string()))?;
    if bytes.len() > 65536 {
        return Err(fail("The rebase plan pointer is too large"));
    }
    String::from_utf8(bytes).map_err(|_| fail("The rebase plan pointer is not valid UTF-8"))
}
fn read_plan(path: &Path) -> Result<HelperPlan> {
    let file = plain_file(path)?;
    let mut data = Vec::new();
    file.take(MAX_PLAN + 1)
        .read_to_end(&mut data)
        .map_err(|e| fail(e.to_string()))?;
    if data.len() as u64 > MAX_PLAN {
        return Err(fail("The rebase plan is too large"));
    }
    let plan: HelperPlan = serde_json::from_slice(&data)
        .map_err(|e| fail(format!("Invalid saved rebase plan: {e}")))?;
    if plan.schema != 1 {
        return Err(fail("Unsupported saved rebase plan version"));
    }
    let git_dir = Path::new(&plan.git_dir)
        .canonicalize()
        .map_err(io_context("The saved Git directory no longer exists"))?;
    if git_dir != Path::new(&plan.git_dir) {
        return Err(fail("The saved Git directory changed"));
    }
    let canonical = path
        .canonicalize()
        .map_err(io_context("Gall's saved rebase plan no longer exists"))?;
    let parent = canonical
        .parent()
        .ok_or_else(|| fail("Invalid rebase plan path"))?;
    if canonical.file_name().is_none_or(|s| s != "plan.json")
        || parent.parent() != Some(git_dir.join("gall-rebase-plans").as_path())
        || path
            .symlink_metadata()
            .map_err(|e| fail(e.to_string()))?
            .is_symlink()
    {
        return Err(fail(
            "The saved rebase plan is outside Gall's plan directory",
        ));
    }
    for entry in &plan.entries {
        if !matches!(entry.id.len(), 40 | 64) || !entry.id.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(fail("Invalid saved rebase commit ID"));
        }
    }
    Ok(plan)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| fail(e.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| fail(e.to_string()))
}
fn save_plan(path: &Path, plan: &HelperPlan) -> Result<PathBuf> {
    let git_dir = Path::new(&plan.git_dir);
    let directory = git_dir.join("gall-rebase-plans");
    match std::fs::create_dir(&directory) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if !directory
                .symlink_metadata()
                .map_err(|e| fail(e.to_string()))?
                .is_dir()
            {
                return Err(fail(
                    "Gall's rebase plan directory is not a regular directory",
                ));
            }
        }
        Err(e) => return Err(fail(e.to_string())),
    }
    let child = directory.join(format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| fail(e.to_string()))?
            .as_nanos(),
        SERIAL.fetch_add(1, Ordering::SeqCst)
    ));
    let active = git_dir.join("gall-rebase-active");
    // Validate the existing pointer before creating anything, so a refusal
    // cannot leave an orphan plan directory behind.
    let replace_pointer = active.exists();
    if replace_pointer {
        // Only replace a pointer that resolves to one of our validated old plans,
        // or whose plan was already removed (a stale pointer is safe to drop).
        let old = read_pointer(&active)?;
        let old = Path::new(old.trim());
        match old.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => {
                read_plan(old)?;
            }
        }
        if path
            .join(super::gitexec::host_path(
                path,
                git(path, &["rev-parse", "--git-path", "rebase-merge"])?.trim(),
            ))
            .exists()
        {
            return Err(fail("A rebase is already in progress"));
        }
    }
    std::fs::create_dir(&child).map_err(|e| fail(e.to_string()))?;
    let file = child.join("plan.json");
    let written = (|| {
        let bytes = serde_json::to_vec(plan).map_err(|e| fail(e.to_string()))?;
        write_new(&file, &bytes)?;
        if replace_pointer {
            std::fs::remove_file(&active).map_err(|e| fail(e.to_string()))?;
        }
        write_new(&active, file.to_string_lossy().as_bytes())
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_dir_all(&child);
        return Err(error);
    }
    Ok(file)
}
fn shell_quote(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| fail("The Git helper path must be valid Unicode"))?;
    let value = if cfg!(windows) {
        value.replace('\\', "/")
    } else {
        value.into()
    };
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}
fn editor_command(executable: &Path, mode: &str, plan: &Path) -> Result<String> {
    Ok(format!(
        "{} {mode} {}",
        shell_quote(executable)?,
        shell_quote(plan)?
    ))
}
fn run_rebase(path: &Path, args: &[&str], plan: &Path, executable: &Path) -> Result<String> {
    let sequence = editor_command(executable, "--git-rebase-editor", plan)?;
    let message = editor_command(executable, "--git-rebase-message", plan)?;
    let mut cmd = Command::new(super::gitstatus::program());
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
        "rebase.updateRefs=false",
        "-c",
        "rebase.autoStash=false",
        // Never let a user setting make Git recurse into submodule folders during the rebase.
        "-c",
        "submodule.recurse=false",
        "-C",
    ])
    .arg(path)
    .args(args)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .env("GIT_SEQUENCE_EDITOR", sequence)
    .env("GIT_EDITOR", message)
    .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let out = cmd.output().map_err(io_context(
        "Could not start git. Make sure Git is installed and available",
    ))?;
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        return Err(fail(format!(
            "{message}\nThe rebase remains available to resolve, continue or abort."
        )));
    }
    Ok(message.trim().into())
}
fn cleanup(_path: &Path, plan_file: &Path) {
    let Ok(saved) = read_plan(plan_file) else {
        return;
    };
    let git_dir = Path::new(&saved.git_dir);
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        return;
    }
    let active = git_dir.join("gall-rebase-active");
    if read_pointer(&active).is_ok_and(|s| Path::new(s.trim()) == plan_file) {
        let _ = std::fs::remove_file(active);
    }
    let _ = std::fs::remove_file(plan_file);
    if let Some(parent) = plan_file.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}
pub fn execute(
    path: &Path,
    entries: Vec<RewriteEntry>,
    base: &str,
    expected: &str,
    editor_executable: &Path,
) -> Result<String> {
    let _busy = super::gitfacts::mutating();
    let root = root(path)?;
    let current = plan(&root, base)?;
    if current.token != expected {
        return Err(fail(
            "The repository changed. Reload and review the rebase plan again",
        ));
    }
    validate_entries(&current, &entries)?;
    clean_guard(&root)?;
    protect_ignored(&root, &current)?;
    let executable = editor_executable.canonicalize().map_err(io_context(
        "Gall cannot find its own program file for the Git editor helper (it may have been updated or moved while running). Restart Gall and try again",
    ))?;
    let directory = git_dir(&root)?;
    let helper = HelperPlan {
        schema: 1,
        root: root
            .canonicalize()
            .map_err(io_context("The repository folder no longer exists"))?
            .to_string_lossy()
            .into_owned(),
        git_dir: directory.to_string_lossy().into_owned(),
        base: current.base.clone(),
        head: current.head.clone(),
        entries,
    };
    let saved = save_plan(&root, &helper)?;
    if let Err(error) = git(
        &root,
        &[
            "update-ref",
            "--no-deref",
            &format!("refs/gall/recovery/{}", current.head),
            &current.head,
        ],
    ) {
        cleanup(&root, &saved);
        return Err(error);
    }
    let output = run_rebase(
        &root,
        &[
            "rebase",
            "--interactive",
            "--merge",
            "--no-autostash",
            "--no-update-refs",
            "--no-fork-point",
            "--no-rebase-merges",
            "--keep-empty",
            "--empty=keep",
            "--reapply-cherry-picks",
            &current.base,
        ],
        &saved,
        &executable,
    );
    cleanup(&root, &saved);
    if output.is_ok() && !rebase_in_progress(&directory) {
        super::gitgraph::drop_recovery(&root, &current.head);
        return output.map(|text| format!("{text}{}", submodule_note(&root)));
    }
    output
}
fn rebase_in_progress(git_dir: &Path) -> bool {
    git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists()
}
/// Reword a single commit directly. The current HEAD can be reworded without consuming the index.
pub fn reword(
    path: &Path,
    target: &str,
    message: &str,
    expected: &str,
    editor_executable: &Path,
) -> Result<String> {
    let _busy = super::gitfacts::mutating();
    if message.trim().is_empty() || message.contains('\0') {
        return Err(fail(
            "A nonempty commit message without NUL characters is required",
        ));
    }
    let root = root(path)?;
    let snapshot = super::gitrepository::snapshot(&root)?;
    if snapshot.token != expected {
        return Err(fail(
            "The repository changed. Refresh before rewording this commit",
        ));
    }
    if snapshot.bisect_active {
        return Err(fail("Finish bisect before rewording history"));
    }
    let target = commit_id(&root, target)?;
    if target == snapshot.head {
        if snapshot.operation.is_some() && !is_edit_stop(&root) {
            return Err(fail(
                "Finish or abort the current Git operation before rewording",
            ));
        }
        git(
            &root,
            &[
                "update-ref",
                "--no-deref",
                &format!("refs/gall/recovery/{target}"),
                &target,
            ],
        )?;
        let output = git(&root, &["commit", "--amend", "--only", "-m", message])?;
        super::gitgraph::retain_detached(&root)?;
        super::gitgraph::drop_recovery(&root, &target);
        return Ok(output);
    }
    let parents = git(&root, &["show", "-s", "--format=%P", &target])?;
    let parents: Vec<_> = parents.split_whitespace().collect();
    if parents.len() != 1 {
        return Err(fail(
            "Rewording a root or merge commit requires an external Git client",
        ));
    }
    let plan = plan(&root, parents[0])?;
    if !plan.commits.iter().any(|entry| entry.id == target) {
        return Err(fail("The selected commit is not on the current branch"));
    }
    let entries = plan
        .commits
        .iter()
        .map(|entry| RewriteEntry {
            id: entry.id.clone(),
            action: if entry.id == target {
                RewriteAction::Reword
            } else {
                RewriteAction::Pick
            },
            message: if entry.id == target {
                message.into()
            } else {
                String::new()
            },
        })
        .collect();
    execute(&root, entries, &plan.base, &plan.token, editor_executable)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum QuickAction {
    Fixup,
    Squash,
    Drop,
    MoveEarlier,
    MoveLater,
}

/// Apply one direct history action through the same validated and recoverable rebase engine.
pub fn quick_edit(
    path: &Path,
    target: &str,
    action: QuickAction,
    expected: &str,
    editor_executable: &Path,
) -> Result<String> {
    let _busy = super::gitfacts::mutating();
    let root = root(path)?;
    let snapshot = super::gitrepository::snapshot(&root)?;
    if snapshot.token != expected {
        return Err(fail(
            "The repository changed. Refresh before editing this commit",
        ));
    }
    if snapshot.bisect_active {
        return Err(fail("Finish bisect before editing history"));
    }
    let target = commit_id(&root, target)?;
    let parents = git(&root, &["show", "-s", "--format=%P", &target])?;
    let parents: Vec<_> = parents.split_whitespace().collect();
    if parents.len() != 1 {
        return Err(fail(
            "Direct history actions require a non-root commit with one parent",
        ));
    }
    let parent = parents[0];
    let base = if matches!(
        action,
        QuickAction::Fixup | QuickAction::Squash | QuickAction::MoveEarlier
    ) {
        let grandparents = git(&root, &["show", "-s", "--format=%P", parent])?;
        let grandparents: Vec<_> = grandparents.split_whitespace().collect();
        if grandparents.len() != 1 {
            return Err(fail("The previous commit is a root or merge commit and cannot be rewritten by this action"));
        }
        grandparents[0].to_owned()
    } else {
        parent.to_owned()
    };
    let plan = plan(&root, &base)?;
    let selected = plan
        .commits
        .iter()
        .position(|entry| entry.id == target)
        .ok_or_else(|| fail("The selected commit is not in the current HEAD history"))?;
    let mut entries: Vec<_> = plan
        .commits
        .iter()
        .map(|entry| RewriteEntry {
            id: entry.id.clone(),
            action: RewriteAction::Pick,
            message: String::new(),
        })
        .collect();
    match action {
        QuickAction::Fixup | QuickAction::Squash => {
            if selected == 0 || entries[selected - 1].id != parent {
                return Err(fail(
                    "The selected commit has no preceding linear commit to combine with",
                ));
            }
            entries[selected].action = if action == QuickAction::Fixup {
                RewriteAction::Fixup
            } else {
                RewriteAction::Squash
            };
        }
        QuickAction::Drop => entries[selected].action = RewriteAction::Drop,
        QuickAction::MoveEarlier => {
            if selected == 0 {
                return Err(fail("This commit cannot be moved earlier"));
            }
            entries.swap(selected - 1, selected);
        }
        QuickAction::MoveLater => {
            if selected + 1 >= entries.len() {
                return Err(fail(
                    "The latest commit has no later commit to exchange positions with",
                ));
            }
            entries.swap(selected, selected + 1);
        }
    }
    execute(&root, entries, &base, &plan.token, editor_executable)
}

/// Resume a Gall rebase with the saved message editor after a restart or an edit stop.
/// `None` leaves ordinary externally started operations to the regular Git controls.
/// Only typed commit actions may be resumed from the graphical workspace.
/// In particular, a foreign or modified todo must never introduce shell commands.
pub(super) fn pending_commits(path: &Path) -> Result<Vec<String>> {
    let directory = git_dir(path)?;
    let mut ids = Vec::new();
    for state in ["rebase-merge/git-rebase-todo", "sequencer/todo"] {
        let file = directory.join(state);
        if !file.exists() {
            continue;
        }
        let mut text = String::new();
        plain_file(&file)?
            .take(MAX_PLAN + 1)
            .read_to_string(&mut text)
            .map_err(|e| fail(e.to_string()))?;
        if text.len() > MAX_PLAN as usize {
            return Err(fail("The pending Git sequence is too large"));
        }
        for line in text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let mut words = line.split_whitespace();
            let action = words.next().unwrap_or_default();
            if !matches!(
                action,
                "pick"
                    | "p"
                    | "reword"
                    | "r"
                    | "edit"
                    | "e"
                    | "squash"
                    | "s"
                    | "fixup"
                    | "f"
                    | "drop"
                    | "d"
                    | "revert"
            ) {
                return Err(fail(format!("The pending Git sequence contains unsupported action '{action}'. Continue in a terminal or abort it.")));
            }
            let mut id = words.next().unwrap_or_default();
            if matches!(action, "fixup" | "f") && matches!(id, "-C" | "-c") {
                id = words.next().unwrap_or_default();
            }
            if id.len() < 4 || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(fail(
                    "The pending Git sequence contains an invalid commit ID",
                ));
            }
            let full = git(
                path,
                &["rev-parse", "--verify", &format!("{id}^{{commit}}")],
            )?;
            ids.push(full.trim().to_owned());
        }
    }
    Ok(ids)
}

pub fn resume(path: &Path, command: &str, editor_executable: &Path) -> Option<Result<String>> {
    let directory = match git_dir(path) {
        Ok(value) => value,
        Err(_) => return None,
    };
    let active = directory.join("gall-rebase-active");
    if !active.exists() || !directory.join("rebase-merge").exists() {
        return None;
    }
    Some((|| {
        if command != "--abort" {
            pending_commits(path)?;
        }
        if !matches!(command, "--continue" | "--skip" | "--abort") {
            return Err(fail("Unsupported rebase control"));
        }
        if active
            .symlink_metadata()
            .map_err(|e| fail(e.to_string()))?
            .is_symlink()
        {
            return Err(fail("Invalid Gall rebase pointer"));
        }
        let pointer = read_pointer(&active)?;
        let file = Path::new(pointer.trim());
        let saved = read_plan(file)?;
        if Path::new(&saved.git_dir) != directory {
            return Err(fail("The active rebase belongs to another repository"));
        }
        let original = std::fs::read_to_string(directory.join("rebase-merge/orig-head")).map_err(
            io_context("The rebase state is incomplete (orig-head is missing). Reload the Git workspace; the rebase may have been finished or aborted outside Gall"),
        )?;
        if original.trim() != saved.head {
            return Err(fail("The active rebase does not match Gall's saved plan"));
        }
        let result = run_rebase(path, &["rebase", command], file, editor_executable);
        cleanup(path, file);
        if result.is_ok() && !rebase_in_progress(&directory) {
            super::gitgraph::drop_recovery(path, &saved.head);
            if command != "--abort" {
                return result.map(|text| format!("{text}{}", submodule_note(path)));
            }
        }
        result
    })())
}
fn replace_helper_file(target: &Path, content: &str) -> Result<()> {
    if !target
        .symlink_metadata()
        .map_err(|e| fail(e.to_string()))?
        .is_file()
    {
        return Err(fail("The editor target is not a regular file"));
    }
    let mut options = OpenOptions::new();
    options.write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(target)
        .map_err(io_context("The Git editor file cannot be opened"))?;
    file.write_all(content.as_bytes())
        .map_err(|e| fail(e.to_string()))
}
fn helper(mode: &str, plan_file: &Path, target: &Path) -> Result<()> {
    let plan = read_plan(plan_file)?;
    let directory = Path::new(&plan.git_dir);
    let original = std::fs::read_to_string(directory.join("rebase-merge/orig-head")).map_err(
        io_context("The rebase state is incomplete (orig-head is missing). Reload the Git workspace; the rebase may have been finished or aborted outside Gall"),
    )?;
    if original.trim() != plan.head {
        return Err(fail(
            "The Git editor invocation does not match the saved rebase",
        ));
    }
    let canonical = target
        .canonicalize()
        .map_err(io_context("The file Git asked Gall to edit does not exist"))?;
    if target
        .symlink_metadata()
        .map_err(|e| fail(e.to_string()))?
        .is_symlink()
    {
        return Err(fail("The Git editor target cannot be a symlink"));
    }
    if mode == "--git-rebase-editor" {
        if canonical != directory.join("rebase-merge/git-rebase-todo") {
            return Err(fail("Unexpected Git sequence editor target"));
        }
        let todo: String = plan
            .entries
            .iter()
            .map(|entry| format!("{} {}\n", entry.action.name(), entry.id))
            .collect();
        return replace_helper_file(target, &todo);
    }
    if ![
        directory.join("COMMIT_EDITMSG"),
        directory.join("rebase-merge/message"),
        directory.join("rebase-merge/message-squash"),
    ]
    .contains(&canonical)
    {
        return Err(fail("Unexpected Git message editor target"));
    }
    let done = std::fs::read_to_string(directory.join("rebase-merge/done")).map_err(io_context(
        "The rebase state is incomplete (the list of completed steps is missing)",
    ))?;
    let operations: Vec<_> = done
        .lines()
        .rev()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let action = parts.next()?;
            let id = parts.next()?;
            (!action.starts_with('#')).then_some((action, id))
        })
        .collect();
    let last = operations
        .first()
        .ok_or_else(|| fail("The rebase has no completed plan entry"))?;
    let group = matches!(last.0, "squash" | "fixup");
    for (index, (action, id)) in operations.iter().enumerate() {
        if index > 0 && (!group || !matches!(*action, "squash" | "fixup" | "drop")) {
            break;
        }
        if let Some(entry) = plan.entries.iter().find(|entry| entry.id.starts_with(id)) {
            if ((*action == "reword" && index == 0) || *action == "squash")
                && !entry.message.trim().is_empty()
            {
                return replace_helper_file(
                    target,
                    &format!("{}\n", entry.message.trim_end_matches('\n')),
                );
            }
        }
        if !group {
            break;
        }
    }
    // Git's generated squash or conflict-resolution message is retained when no replacement was requested.
    Ok(())
}
/// Called before Tauri setup. Arguments may include or omit the executable name.
pub fn handle_editor_args(args: &[OsString]) -> Option<i32> {
    let args = if args
        .first()
        .is_some_and(|value| value.to_string_lossy().starts_with("--git-rebase-"))
    {
        args
    } else {
        args.get(1..).unwrap_or_default()
    };
    let mode = args.first()?.to_str()?;
    if !matches!(mode, "--git-rebase-editor" | "--git-rebase-message") {
        return None;
    }
    let result = if args.len() == 3 {
        helper(mode, Path::new(&args[1]), Path::new(&args[2]))
    } else {
        Err(fail("Invalid Git editor helper arguments"))
    };
    if let Err(error) = &result {
        eprintln!("Gall Git editor: {error}");
    }
    Some(if result.is_ok() { 0 } else { 1 })
}

/// An interactive edit stop has already replayed its commit and explicitly permits amend.
pub fn is_edit_stop(path: &Path) -> bool {
    // Usually there is no rebase: seen in the Git directory without asking Git for it, and
    // from one listing of it (a missing file is slow to look up in WSL).
    if super::gitfacts::markers(path).is_some_and(|listed| !listed.exists("rebase-merge")) {
        return false;
    }
    let state = match super::gitfacts::marker(path, "rebase-merge") {
        Some(state) => state,
        None => match git_dir(path) {
            Ok(directory) => directory.join("rebase-merge"),
            Err(_) => return false,
        },
    };
    if !state.join("amend").is_file() || !state.join("interactive").is_file() {
        return false;
    }
    let Ok(done) = std::fs::read_to_string(state.join("done")) else {
        return false;
    };
    if !done
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .is_some_and(|line| line.starts_with("edit "))
    {
        return false;
    }
    git(path, &["ls-files", "--unmerged", "-z"]).is_ok_and(|raw| raw.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repository() -> tempfile::TempDir {
        let temp = tempfile::Builder::new()
            .prefix("gall-rewrite ' $ test-")
            .tempdir()
            .unwrap();
        let path = temp.path();
        git(path, &["init", "-q"]).unwrap();
        git(path, &["config", "user.name", "Test"]).unwrap();
        git(path, &["config", "user.email", "test@example.com"]).unwrap();
        commit(path, "base", "base.txt");
        temp
    }
    fn commit(path: &Path, message: &str, file: &str) -> String {
        std::fs::write(path.join(file), format!("{message}\n")).unwrap();
        git(path, &["add", "--all"]).unwrap();
        git(path, &["commit", "-qm", message]).unwrap();
        git(path, &["rev-parse", "HEAD"]).unwrap().trim().into()
    }
    #[test]
    fn stale_pointer_is_replaced_and_refusals_leave_no_orphans() {
        let temp = repository();
        let path = temp.path();
        let head = git(path, &["rev-parse", "HEAD"]).unwrap();
        let directory = git_dir(path).unwrap();
        let plan = HelperPlan {
            schema: 1,
            root: path.to_string_lossy().into_owned(),
            git_dir: directory.to_string_lossy().into_owned(),
            head: head.trim().into(),
            base: head.trim().into(),
            entries: vec![],
        };
        let plans = directory.join("gall-rebase-plans");
        let first = save_plan(path, &plan).unwrap();
        // Simulate manual cleanup that removed the plan but not the pointer.
        std::fs::remove_dir_all(first.parent().unwrap()).unwrap();
        let second = save_plan(path, &plan).unwrap();
        assert!(read_plan(&second).is_ok());
        let pointer = std::fs::read_to_string(directory.join("gall-rebase-active")).unwrap();
        assert_eq!(Path::new(pointer.trim()), second);
        // A refusal (rebase in progress) must not create another plan directory.
        let count = |p: &Path| std::fs::read_dir(p).unwrap().count();
        let before = count(&plans);
        std::fs::create_dir(directory.join("rebase-merge")).unwrap();
        assert!(save_plan(path, &plan).is_err());
        assert_eq!(count(&plans), before);
    }
    fn entry(id: &str, action: RewriteAction, message: &str) -> RewriteEntry {
        RewriteEntry {
            id: id.into(),
            action,
            message: message.into(),
        }
    }
    #[test]
    fn reordering_dependent_commits_is_predicted_as_a_conflict_without_touching_the_repository() {
        let temp = repository();
        let path = temp.path();
        git(path, &["config", "core.autocrlf", "false"]).unwrap();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        let a = commit(path, "a", "shared.txt");
        let b = commit(path, "b", "shared.txt");
        let c = commit(path, "c", "other.txt");
        let head = git(path, &["rev-parse", "HEAD"]).unwrap();
        let status = git(path, &["status", "--porcelain"]).unwrap();
        let pick = |id: &str| entry(id, RewriteAction::Pick, "");
        let ids =
            |list: Vec<ConflictPrediction>| list.into_iter().map(|c| c.id).collect::<Vec<_>>();
        let predict = |entries: &[RewriteEntry]| {
            ids(super::predict_conflicts(path, base.trim(), entries).unwrap())
        };
        assert!(predict(&[pick(&a), pick(&b), pick(&c)]).is_empty());
        // `a` creates the file that `b` already changed, so it conflicts on top of `b` too.
        assert_eq!(
            predict(&[pick(&b), pick(&a), pick(&c)]),
            [b.clone(), a.clone()]
        );
        assert!(predict(&[pick(&a), pick(&c), pick(&b)]).is_empty());
        assert!(predict(&[pick(&c), pick(&a), pick(&b)]).is_empty());
        // Dropping the commit that the next one builds on makes the next one conflict.
        assert_eq!(
            predict(&[entry(&a, RewriteAction::Drop, ""), pick(&b), pick(&c)]),
            std::slice::from_ref(&b)
        );
        // Squash and fixup apply their changes like a pick.
        assert_eq!(
            predict(&[pick(&c), entry(&b, RewriteAction::Fixup, ""), pick(&a)]),
            [b.clone(), a.clone()]
        );
        let found = super::predict_conflicts(path, base.trim(), &[pick(&b), pick(&a)]).unwrap();
        assert_eq!(found[0].files, ["shared.txt"]);
        assert!(super::predict_conflicts(path, base.trim(), &[pick("zz")]).is_err());
        assert_eq!(git(path, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(git(path, &["status", "--porcelain"]).unwrap(), status);
        assert!(!path.join(".git/rebase-merge").exists());
    }
    #[test]
    fn the_replay_in_one_process_matches_one_merge_per_commit() {
        let temp = repository();
        let path = temp.path();
        git(path, &["config", "core.autocrlf", "false"]).unwrap();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        let a = commit(path, "a", "shared.txt");
        let b = commit(path, "b", "shared.txt");
        let c = commit(path, "c", "other file.txt");
        let d = commit(path, "d", "shared.txt");
        // The replay as `predict_conflicts` did it before: one `merge-tree` per commit.
        let serial = |ids: &[&String]| {
            let mut current = base.clone();
            let mut found = Vec::new();
            for id in ids {
                let (tree, conflict) = merge_trees(path, &format!("{id}^1"), &current, id).unwrap();
                if let Some(files) = conflict {
                    found.push((id.to_string(), files));
                }
                current = tree;
            }
            found
        };
        for order in [
            vec![&a, &b, &c, &d],
            vec![&d, &c, &b, &a],
            vec![&b, &a, &d],
            vec![&c],
        ] {
            let ids: Vec<String> = order.iter().map(|id| id.to_string()).collect();
            let streamed = replay_streamed(path, &base, &ids)
                .expect("the replay runs in one process")
                .into_iter()
                .map(|found| (found.id, found.files))
                .collect::<Vec<_>>();
            assert_eq!(streamed, serial(&order), "{order:?}");
        }
        assert!(
            !serial(&[&b, &a]).is_empty(),
            "the orders include conflicts"
        );
        // A commit Git does not know: no answer here, so the caller merges one by one and
        // reports Git's error.
        let unknown = "0".repeat(base.len());
        assert!(replay_streamed(path, &base, &[a.clone(), unknown.clone()]).is_none());
        assert!(
            super::predict_conflicts(path, &base, &[entry(&unknown, RewriteAction::Pick, "")])
                .is_err()
        );
    }
    #[test]
    fn missing_helper_executable_is_explained() {
        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        let a = commit(path, "first", "a.txt");
        let plan = super::plan(path, base.trim()).unwrap();
        let error = execute(
            path,
            vec![entry(&a, RewriteAction::Pick, "")],
            base.trim(),
            &plan.token,
            &path.join("missing-gall"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("own program file"), "{error}");
        assert!(!error.contains("os error"), "{error}");
    }
    #[test]
    fn linear_plan_is_oldest_first_and_validation_preserves_repository() {
        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        let a = commit(path, "first", "a.txt");
        let b = commit(path, "second", "b.txt");
        let plan = super::plan(path, base.trim()).unwrap();
        assert_eq!(
            plan.commits
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            [a.as_str(), b.as_str()]
        );
        assert_eq!(plan.commits[0].message, "first");
        assert!(validate_entries(
            &plan,
            &[
                entry(&a, RewriteAction::Squash, ""),
                entry(&b, RewriteAction::Pick, "")
            ]
        )
        .is_err());
        assert!(validate_entries(
            &plan,
            &[
                entry(&a, RewriteAction::Pick, ""),
                entry(&a, RewriteAction::Pick, "")
            ]
        )
        .is_err());
        assert!(validate_entries(
            &plan,
            &[
                entry(&a, RewriteAction::Reword, " "),
                entry(&b, RewriteAction::Pick, "")
            ]
        )
        .is_err());
        assert!(validate_entries(
            &plan,
            &[
                entry(&a, RewriteAction::Drop, ""),
                entry(&b, RewriteAction::Drop, "")
            ]
        )
        .is_ok());
        assert!(super::plan(path, "--exec=malicious").is_err());
        let before = git(path, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(path.join("a.txt"), "dirty\n").unwrap();
        assert!(execute(
            path,
            vec![
                entry(&a, RewriteAction::Pick, ""),
                entry(&b, RewriteAction::Pick, "")
            ],
            base.trim(),
            &plan.token,
            &std::env::current_exe().unwrap()
        )
        .is_err());
        assert_eq!(git(path, &["rev-parse", "HEAD"]).unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "dirty\n"
        );
    }
    #[test]
    fn plans_can_rebase_onto_a_diverged_branch_and_flatten_merges() {
        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        git(path, &["switch", "-qc", "side"]).unwrap();
        let side = commit(path, "side", "side.txt");
        git(path, &["switch", "-q", "-"]).unwrap();
        let main = commit(path, "main", "main.txt");
        git(path, &["switch", "-qc", "feature", base.trim()]).unwrap();
        let first = commit(path, "first", "first.txt");
        git(path, &["merge", "--no-edit", "-q", &side]).unwrap();
        let second = commit(path, "second", "second.txt");
        let onto = plan(path, &main).unwrap();
        let ids: BTreeSet<_> = onto.commits.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            BTreeSet::from([first.as_str(), side.as_str(), second.as_str()])
        );
        assert_eq!(onto.skipped_merges, 1);
    }
    #[test]
    fn reword_head_preserves_staged_and_unstaged_contents_and_retains_original() {
        let temp = repository();
        let path = temp.path();
        let original = git(path, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(path.join("base.txt"), "staged\n").unwrap();
        git(path, &["add", "base.txt"]).unwrap();
        std::fs::write(path.join("base.txt"), "working\n").unwrap();
        let state = super::super::gitrepository::snapshot(path).unwrap();
        reword(
            path,
            original.trim(),
            "renamed root",
            &state.token,
            &std::env::current_exe().unwrap(),
        )
        .unwrap();
        assert_eq!(
            git(path, &["log", "-1", "--format=%B"]).unwrap().trim(),
            "renamed root"
        );
        assert_eq!(git(path, &["show", "HEAD:base.txt"]).unwrap(), "base\n");
        assert_eq!(git(path, &["show", ":base.txt"]).unwrap(), "staged\n");
        assert_eq!(
            std::fs::read_to_string(path.join("base.txt")).unwrap(),
            "working\n"
        );
        // The recovery reference is removed once the reword succeeded, so no graph shows it.
        assert!(git(
            path,
            &[
                "show-ref",
                "--verify",
                &format!("refs/gall/recovery/{}", original.trim())
            ]
        )
        .is_err());
        assert!(!super::super::gitgraph::history_graph(path, false, false)
            .unwrap()
            .history
            .contains(original.trim()));
        // The original stays reachable through the reflog view.
        assert!(super::super::gitgraph::history_graph(path, false, true)
            .unwrap()
            .history
            .contains(original.trim()));
        assert!(reword(
            path,
            original.trim(),
            "stale",
            &state.token,
            &std::env::current_exe().unwrap()
        )
        .is_err());
    }
    #[test]
    fn ignored_collision_guard_includes_intermediate_commit_trees() {
        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        commit(path, "temporary", "cache.txt");
        std::fs::remove_file(path.join("cache.txt")).unwrap();
        git(path, &["add", "--all"]).unwrap();
        git(path, &["commit", "-qm", "removed"]).unwrap();
        let directory = git_dir(path).unwrap();
        std::fs::write(directory.join("info/exclude"), "cache.txt\n").unwrap();
        std::fs::write(path.join("cache.txt"), "valuable ignored cache\n").unwrap();
        let plan = super::plan(path, base.trim()).unwrap();
        assert!(protect_ignored(path, &plan).is_err());
        assert_eq!(
            std::fs::read_to_string(path.join("cache.txt")).unwrap(),
            "valuable ignored cache\n"
        );
    }
    #[test]
    fn helper_limits_writes_to_git_todo_and_message_files() {
        let temp = repository();
        let path = temp.path();
        let head = git(path, &["rev-parse", "HEAD"]).unwrap();
        let head = head.trim();
        let directory = git_dir(path).unwrap();
        let saved = save_plan(
            path,
            &HelperPlan {
                schema: 1,
                root: path.to_string_lossy().into_owned(),
                git_dir: directory.to_string_lossy().into_owned(),
                head: head.into(),
                base: head.into(),
                entries: vec![entry(
                    head,
                    RewriteAction::Reword,
                    "literal $(not executed) `value`\nsecond line",
                )],
            },
        )
        .unwrap();
        std::fs::create_dir(directory.join("rebase-merge")).unwrap();
        std::fs::write(directory.join("rebase-merge/orig-head"), head).unwrap();
        let todo = directory.join("rebase-merge/git-rebase-todo");
        std::fs::write(&todo, "old").unwrap();
        helper("--git-rebase-editor", &saved, &todo).unwrap();
        assert_eq!(
            std::fs::read_to_string(todo).unwrap(),
            format!("reword {head}\n")
        );
        std::fs::write(
            directory.join("rebase-merge/done"),
            format!("reword {head} # original\n"),
        )
        .unwrap();
        let message = directory.join("COMMIT_EDITMSG");
        std::fs::write(&message, "old message").unwrap();
        helper("--git-rebase-message", &saved, &message).unwrap();
        assert_eq!(
            std::fs::read_to_string(message).unwrap(),
            "literal $(not executed) `value`\nsecond line\n"
        );
        let outside = path.join("outside.txt");
        std::fs::write(&outside, "keep").unwrap();
        assert!(helper("--git-rebase-message", &saved, &outside).is_err());
        assert!(helper("--git-rebase-editor", &saved, &outside).is_err());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "keep");
        assert_eq!(
            handle_editor_args(&["gall".into(), "--git-rebase-editor".into()]),
            Some(1)
        );
    }
    #[cfg(unix)]
    #[test]
    fn editor_helpers_reject_symlink_plan_and_target_files() {
        use std::os::unix::fs::symlink;
        let temp = repository();
        let path = temp.path();
        let head = git(path, &["rev-parse", "HEAD"]).unwrap();
        let directory = git_dir(path).unwrap();
        let saved = save_plan(
            path,
            &HelperPlan {
                schema: 1,
                root: path.to_string_lossy().into_owned(),
                git_dir: directory.to_string_lossy().into_owned(),
                head: head.trim().into(),
                base: head.trim().into(),
                entries: vec![],
            },
        )
        .unwrap();
        let alias = path.join("alias.json");
        symlink(&saved, &alias).unwrap();
        assert!(read_plan(&alias).is_err());
        std::fs::create_dir(directory.join("rebase-merge")).unwrap();
        let outside = path.join("valuable");
        std::fs::write(&outside, "keep").unwrap();
        let todo = directory.join("rebase-merge/git-rebase-todo");
        symlink(&outside, &todo).unwrap();
        assert!(helper("--git-rebase-editor", &saved, &todo).is_err());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "keep");
    }
    #[test]
    fn submodules_do_not_block_rewriting_but_a_moved_submodule_commit_does() {
        let temp = repository();
        let path = temp.path();
        let module = tempfile::tempdir().unwrap();
        let module_path = module.path();
        git(module_path, &["init", "-q"]).unwrap();
        git(module_path, &["config", "user.name", "Test"]).unwrap();
        git(module_path, &["config", "user.email", "test@example.com"]).unwrap();
        git(module_path, &["config", "core.autocrlf", "false"]).unwrap();
        commit(module_path, "module one", "m.txt");
        git(path, &["config", "core.autocrlf", "false"]).unwrap();
        // A local path is refused as a submodule source unless the file protocol is allowed.
        git(
            path,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &module_path.to_string_lossy().replace('\\', "/"),
                "sub",
            ],
        )
        .unwrap();
        git(path, &["commit", "-qm", "add submodule"]).unwrap();

        // Used to be refused outright.
        clean_guard(path).unwrap();
        assert_eq!(submodule_note(path), "");
        // Untracked and edited files inside the submodule are not at risk: a rebase never touches them.
        std::fs::write(path.join("sub").join("scratch.txt"), "untracked\n").unwrap();
        std::fs::write(path.join("sub").join("m.txt"), "edited\n").unwrap();
        clean_guard(path).unwrap();

        // A submodule checked out at another commit than the repository records is a local change.
        let sub = path.join("sub");
        git(&sub, &["config", "user.name", "Test"]).unwrap();
        git(&sub, &["config", "user.email", "test@example.com"]).unwrap();
        git(&sub, &["checkout", "-q", "--", "m.txt"]).unwrap();
        std::fs::remove_file(sub.join("scratch.txt")).unwrap();
        commit(&sub, "module two", "m.txt");
        let error = clean_guard(path).unwrap_err().to_string();
        assert!(error.contains("local changes"), "{error}");
        assert!(submodule_note(path).contains("sub"));
    }
    #[test]
    #[ignore = "Requires GALL_REBASE_HELPER pointing to a bundled Gall application"]
    fn rebase_in_a_repository_with_a_submodule_keeps_the_pointer_and_the_submodule_folder() {
        let executable =
            PathBuf::from(std::env::var_os("GALL_REBASE_HELPER").expect("Set GALL_REBASE_HELPER"));
        let temp = repository();
        let path = temp.path();
        let module = tempfile::tempdir().unwrap();
        let module_path = module.path();
        git(module_path, &["init", "-q"]).unwrap();
        for dir in [module_path, path] {
            git(dir, &["config", "core.autocrlf", "false"]).unwrap();
        }
        git(module_path, &["config", "user.name", "Test"]).unwrap();
        git(module_path, &["config", "user.email", "test@example.com"]).unwrap();
        commit(module_path, "module one", "m.txt");
        git(
            path,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &module_path.to_string_lossy().replace('\\', "/"),
                "sub",
            ],
        )
        .unwrap();
        git(path, &["commit", "-qm", "add submodule"]).unwrap();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        let first = commit(path, "first", "a.txt");
        // A commit that moves the submodule pointer, in the middle of the range.
        let sub = path.join("sub");
        git(&sub, &["config", "user.name", "Test"]).unwrap();
        git(&sub, &["config", "user.email", "test@example.com"]).unwrap();
        let moved_to = commit(&sub, "module two", "m.txt");
        git(path, &["add", "sub"]).unwrap();
        git(path, &["commit", "-qm", "bump submodule"]).unwrap();
        let bump = git(path, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        let third = commit(path, "third", "c.txt");
        let plan = super::plan(path, base.trim()).unwrap();
        let output = execute(
            path,
            vec![
                entry(&third, RewriteAction::Pick, ""),
                entry(&first, RewriteAction::Pick, ""),
                entry(&bump, RewriteAction::Pick, ""),
            ],
            base.trim(),
            &plan.token,
            &executable,
        )
        .unwrap();
        assert_eq!(
            git(
                path,
                &["log", "--format=%s", &format!("{}..HEAD", base.trim())]
            )
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
            ["bump submodule", "first", "third"]
        );
        // The rewritten history still records the moved submodule commit, and the submodule's own
        // folder was neither touched nor reset: it is still at that commit.
        assert!(git(path, &["ls-tree", "HEAD", "sub"])
            .unwrap()
            .contains(&moved_to));
        assert_eq!(git(&sub, &["rev-parse", "HEAD"]).unwrap().trim(), moved_to);
        assert!(!is_edit_stop(path) && !rebase_in_progress(&git_dir(path).unwrap()));
        assert!(!output.contains("no longer match"), "{output}");
        clean_guard(path).unwrap();
    }
    #[test]
    #[ignore = "Requires GALL_REBASE_HELPER pointing to a bundled Gall application"]
    fn interactive_rebase_with_bundled_helper() {
        let executable =
            PathBuf::from(std::env::var_os("GALL_REBASE_HELPER").expect("Set GALL_REBASE_HELPER"));
        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        let first = commit(path, "first", "a.txt");
        let second = commit(path, "second", "b.txt");
        let third = commit(path, "third", "c.txt");
        let plan = super::plan(path, base.trim()).unwrap();
        execute(
            path,
            vec![
                entry(&second, RewriteAction::Reword, "renamed second"),
                entry(&first, RewriteAction::Squash, "combined message"),
                entry(&third, RewriteAction::Fixup, ""),
            ],
            base.trim(),
            &plan.token,
            &executable,
        )
        .unwrap();
        assert_eq!(
            git(path, &["log", "-1", "--format=%B"]).unwrap().trim(),
            "combined message"
        );
        assert_eq!(
            git(
                path,
                &["rev-list", "--count", &format!("{}..HEAD", base.trim())]
            )
            .unwrap()
            .trim(),
            "1"
        );
        assert!(
            path.join("a.txt").exists()
                && path.join("b.txt").exists()
                && path.join("c.txt").exists()
        );
        assert!(!git_dir(path).unwrap().join("gall-rebase-active").exists());
        assert!(git(
            path,
            &[
                "show-ref",
                "--verify",
                &format!("refs/gall/recovery/{third}")
            ]
        )
        .is_err());
        let snapshot = super::super::gitrepository::snapshot(path).unwrap();
        let current = snapshot.head;
        commit(path, "fourth", "four.txt");
        let snapshot = super::super::gitrepository::snapshot(path).unwrap();
        reword(
            path,
            &current,
            "background reword",
            &snapshot.token,
            &executable,
        )
        .unwrap();
        assert_eq!(
            git(path, &["log", "-2", "--format=%s"]).unwrap().trim(),
            "fourth\nbackground reword"
        );

        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        let first = commit(path, "first", "a.txt");
        let second = commit(path, "second", "b.txt");
        let third = commit(path, "third", "c.txt");
        let plan = super::plan(path, base.trim()).unwrap();
        execute(
            path,
            vec![
                entry(&first, RewriteAction::Drop, ""),
                entry(&second, RewriteAction::Pick, ""),
                entry(&third, RewriteAction::Edit, ""),
            ],
            base.trim(),
            &plan.token,
            &executable,
        )
        .unwrap();
        assert!(is_edit_stop(path));
        assert!(!path.join("a.txt").exists());
        std::fs::write(path.join("c.txt"), "edited third\n").unwrap();
        let state = super::super::gitworktree::workspace(path).unwrap();
        assert!(state.edit_stop);
        super::super::gitworktree::index_action(
            path,
            super::super::gitworktree::IndexAction::Stage,
            Some("c.txt"),
            "",
            &state.token,
        )
        .unwrap();
        let state = super::super::gitworktree::workspace(path).unwrap();
        assert!(super::super::gitworktree::index_action(
            path,
            super::super::gitworktree::IndexAction::Commit,
            None,
            "invalid normal commit",
            &state.token
        )
        .is_err());
        super::super::gitworktree::index_action(
            path,
            super::super::gitworktree::IndexAction::Amend,
            None,
            "edited third",
            &state.token,
        )
        .unwrap();
        resume(path, "--continue", &executable).unwrap().unwrap();
        assert!(!is_edit_stop(path));
        assert!(super::super::gitrepository::snapshot(path)
            .unwrap()
            .operation
            .is_none());
        assert_eq!(
            git(path, &["show", "HEAD:c.txt"]).unwrap(),
            "edited third\n"
        );
    }
    #[test]
    fn quick_history_actions_reject_stale_and_root_context_without_writes() {
        let temp = repository();
        let path = temp.path();
        let original = git(path, &["rev-parse", "HEAD"]).unwrap();
        let initial = super::super::gitrepository::snapshot(path).unwrap();
        let binary = std::env::current_exe().unwrap();
        assert!(quick_edit(
            path,
            original.trim(),
            QuickAction::Drop,
            &initial.token,
            &binary
        )
        .is_err());
        let next = commit(path, "next", "next.txt");
        assert!(quick_edit(path, &next, QuickAction::Drop, &initial.token, &binary).is_err());
        let state = super::super::gitrepository::snapshot(path).unwrap();
        assert!(quick_edit(path, &next, QuickAction::MoveLater, &state.token, &binary).is_err());
        assert!(quick_edit(path, &next, QuickAction::Fixup, &state.token, &binary).is_err());
        assert_eq!(git(path, &["rev-parse", "HEAD"]).unwrap().trim(), next);
        assert!(path.join("next.txt").exists());
    }
    #[test]
    #[ignore = "Requires GALL_REBASE_HELPER pointing to a bundled Gall application"]
    fn quick_history_actions_with_bundled_helper() {
        let binary =
            PathBuf::from(std::env::var_os("GALL_REBASE_HELPER").expect("Set GALL_REBASE_HELPER"));
        for action in [
            QuickAction::Fixup,
            QuickAction::Squash,
            QuickAction::Drop,
            QuickAction::MoveEarlier,
            QuickAction::MoveLater,
        ] {
            let temp = repository();
            let path = temp.path();
            let base = git(path, &["rev-parse", "HEAD"]).unwrap();
            let first = commit(path, "first", "a.txt");
            let second = commit(path, "second", "b.txt");
            let third = commit(path, "third", "c.txt");
            let target = if action == QuickAction::MoveLater {
                &first
            } else {
                &second
            };
            let state = super::super::gitrepository::snapshot(path).unwrap();
            quick_edit(path, target, action, &state.token, &binary).unwrap();
            let subjects = git(
                path,
                &[
                    "log",
                    "--reverse",
                    "--format=%s",
                    &format!("{}..HEAD", base.trim()),
                ],
            )
            .unwrap();
            match action {
                QuickAction::Fixup => {
                    assert_eq!(subjects.trim(), "first\nthird");
                    assert!(path.join("b.txt").exists());
                }
                QuickAction::Squash => {
                    assert_eq!(subjects.trim(), "first\nthird");
                    let messages = git(
                        path,
                        &["log", "--format=%B", &format!("{}..HEAD", base.trim())],
                    )
                    .unwrap();
                    assert!(messages.contains("second"));
                    assert!(path.join("b.txt").exists());
                }
                QuickAction::Drop => {
                    assert_eq!(subjects.trim(), "first\nthird");
                    assert!(!path.join("b.txt").exists());
                }
                QuickAction::MoveEarlier | QuickAction::MoveLater => {
                    assert_eq!(subjects.trim(), "second\nfirst\nthird")
                }
            }
            assert!(path.join("a.txt").exists() && path.join("c.txt").exists());
            assert!(git(
                path,
                &[
                    "show-ref",
                    "--verify",
                    &format!("refs/gall/recovery/{third}")
                ]
            )
            .is_err());
            assert!(super::super::gitrepository::snapshot(path)
                .unwrap()
                .operation
                .is_none());
        }
    }
    #[test]
    fn pending_sequences_reject_scripts_and_include_fixup_message_commits() {
        let temp = repository();
        let path = temp.path();
        let id = commit(path, "first", "a.txt");
        let state = git_dir(path).unwrap().join("rebase-merge");
        std::fs::create_dir(&state).unwrap();
        let todo = state.join("git-rebase-todo");
        std::fs::write(&todo, format!("fixup -C {id} first\n")).unwrap();
        assert_eq!(pending_commits(path).unwrap(), vec![id.clone()]);
        for action in [
            "exec git update-ref refs/heads/exec-ran HEAD",
            "x git status",
            "label new-base",
            "reset HEAD",
            "update-ref refs/heads/main",
        ] {
            std::fs::write(&todo, format!("pick {id} first\n{action}\n")).unwrap();
            assert!(pending_commits(path)
                .unwrap_err()
                .to_string()
                .contains("unsupported action"));
        }
        assert!(git(path, &["show-ref", "--verify", "refs/heads/exec-ran"]).is_err());
    }
    #[test]
    #[ignore = "Requires GALL_REBASE_HELPER pointing to a bundled Gall application"]
    fn resuming_never_executes_modified_or_foreign_todo_scripts() {
        let executable =
            PathBuf::from(std::env::var_os("GALL_REBASE_HELPER").expect("Set GALL_REBASE_HELPER"));
        for foreign in [false, true] {
            let temp = repository();
            let path = temp.path();
            let base = git(path, &["rev-parse", "HEAD"]).unwrap();
            let first = commit(path, "first", "a.txt");
            let second = commit(path, "second", "b.txt");
            let plan = super::plan(path, base.trim()).unwrap();
            execute(
                path,
                vec![
                    entry(&first, RewriteAction::Edit, ""),
                    entry(&second, RewriteAction::Pick, ""),
                ],
                &plan.base,
                &plan.token,
                &executable,
            )
            .unwrap();
            let directory = git_dir(path).unwrap();
            let todo = directory.join("rebase-merge/git-rebase-todo");
            let original = std::fs::read_to_string(&todo).unwrap();
            std::fs::write(
                &todo,
                format!("{original}\nexec git update-ref refs/heads/exec-ran HEAD\n"),
            )
            .unwrap();
            if foreign {
                std::fs::remove_file(directory.join("gall-rebase-active")).unwrap();
            }
            let state = super::super::gitrepository::snapshot(path).unwrap();
            let error = super::super::gitrepository::execute(
                path,
                super::super::gitrepository::Request::Continue,
                &state.token,
            )
            .unwrap_err();
            assert!(error.to_string().contains("unsupported action"));
            if !foreign {
                assert!(resume(path, "--continue", &executable)
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("unsupported action"));
            }
            assert!(git(path, &["show-ref", "--verify", "refs/heads/exec-ran"]).is_err());
            git(path, &["rebase", "--abort"]).unwrap();
        }
    }
}
