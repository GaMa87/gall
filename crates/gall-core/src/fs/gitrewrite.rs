//! A validated interactive-rebase plan. Git performs replay and retains resumable conflicts.
use super::gitgraph::{git, root};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

const MAX_PLAN: u64 = 8 * 1024 * 1024;
/// A plan of more commits would not fit into [`MAX_PLAN`].
const MAX_COMMITS: usize = 50_000;
/// Without `merge-tree --stdin` every commit of a forecast is a process of its own.
const MAX_SERIAL: usize = 1_000;
/// The base of a plan that rewrites from the root commit (git rebase -i --root). It is no commit ID.
pub const ROOT_BASE: &str = "root";
/// The longest xec command of a plan.
const MAX_EXEC: usize = 4096;
static SERIAL: AtomicU64 = AtomicU64::new(0);
static STREAMED_REPLAY_SUPPORTED: OnceLock<bool> = OnceLock::new();

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
}
/// The whole message of a commit, asked for when its editor opens ([`messages`]).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitMessage {
    pub id: String,
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
    /// A shell command run after the commits before it; the command is the entry's message and the ID is empty.
    Exec,
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
            Self::Exec => "exec",
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RewriteEntry {
    #[serde(default)]
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
        let Some(subject) = fields.next() else {
            return Err(fail("Invalid commit metadata"));
        };
        commits.push(RewriteCommit {
            id: id.into(),
            subject: subject.into(),
        });
    }
    if commits.len() > MAX_COMMITS {
        return Err(fail(format!(
            "Select a range of at most {MAX_COMMITS} commits"
        )));
    }
    Ok(RewritePlan {
        head: snapshot.head.clone(),
        base: base.clone(),
        token: plan_token(&snapshot, &base),
        skipped_merges,
        commits,
    })
}
/// The messages of commits, with one process.
pub fn messages(path: &Path, ids: &[String]) -> Result<Vec<CommitMessage>> {
    const MAX_IDS: usize = 500;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    if ids.len() > MAX_IDS {
        return Err(fail(format!("Ask for at most {MAX_IDS} messages at once")));
    }
    if ids
        .iter()
        .any(|id| !matches!(id.len(), 40 | 64) || !id.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(fail("Invalid commit ID in the rebase plan"));
    }
    let root = root(path)?;
    let mut args = vec![
        "log",
        "--no-show-signature",
        "--no-walk=unsorted",
        "--format=%H%x00%B%x00",
    ];
    args.extend(ids.iter().map(String::as_str));
    args.push("--");
    let listed = git(&root, &args)?;
    let mut fields = listed.split('\0');
    let mut found = Vec::new();
    while let (Some(id), Some(message)) = (fields.next(), fields.next()) {
        found.push(CommitMessage {
            id: id.trim_start_matches('\n').into(),
            message: message.trim_end_matches('\n').into(),
        });
    }
    Ok(found)
}
/// The checked base, the commits from it to `head` with their subjects (`log` output) and the
/// number of merges among them, for [`plan`].
fn plan_commits(root: &Path, base: &str, head: &str) -> Result<(String, String, usize)> {
    let base = if base == ROOT_BASE {
        ROOT_BASE.to_owned()
    } else {
        commit_id(root, base)?
    };
    let range = if base == ROOT_BASE {
        head.to_owned()
    } else {
        format!("{base}..{head}")
    };
    // The commits in one query, next to the count of merges. Messages are asked for when an editor
    // needs one: for a long plan they are most of the output.
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
                &format!("--max-count={}", MAX_COMMITS + 1),
                "--format=%H%x00%s%x00",
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
pub(super) fn merge_trees(
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
    if tree.len() != theirs.len() || !tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let message = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(fail(if message.is_empty() {
            "Git returned an invalid tree for the conflict check".to_owned()
        } else {
            message
        }));
    }
    let files = parts
        .take_while(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    Ok((tree, conflicted.then_some(files)))
}
fn git_supports_streamed_replay(version: &str) -> bool {
    let Some(version) = version.trim().strip_prefix("git version ") else {
        return false;
    };
    let Some(version) = version.split_whitespace().next() else {
        return false;
    };
    let mut parts = version.split('.');
    let mut number = || parts.next().and_then(|part| part.parse::<u32>().ok());
    let (Some(major), Some(minor), Some(_patch)) = (number(), number(), number()) else {
        return false;
    };
    // Git 2.49 both accepts tree inputs and flushes each --stdin reply. Older versions
    // can wait for EOF while Gall waits for the reply: fall back before opening pipes.
    major > 2 || (major == 2 && minor >= 49)
}

/// [`merge_trees`] for every commit of `ids` in turn, starting from `start`, in one
/// `merge-tree --stdin` process: Git answers each input line (`<base> -- <ours> <theirs>`)
/// before it reads the next, so each merge can start from the tree of the one before. A
/// process per commit cost 30–40 ms each on Windows. None when this cannot be used (a
/// repository in WSL, whose Git runs through a shell without input, or a Git that refuses
/// the input); the caller then merges one commit per process, which also reports the error.
fn replay_streamed(
    root: &Path,
    start: &str,
    ids: &[String],
    parents: &Parents,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<ConflictPrediction>> {
    use std::io::BufRead;
    if ids.is_empty() {
        return Some(Vec::new());
    }
    if super::gitexec::place(root).is_some() {
        return None;
    }
    if !*STREAMED_REPLAY_SUPPORTED.get_or_init(|| {
        super::gitexec::command(root, super::gitstatus::program())
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .is_some_and(|version| git_supports_streamed_replay(&version))
    }) {
        return None;
    }
    let mut cmd = super::gitexec::command(root, super::gitstatus::program());
    cmd.args(["-c", "core.fsmonitor=false", "-C"])
        .arg(super::gitexec::dir_arg(root))
        .args(["merge-tree", "--stdin", "--name-only", "--no-messages"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_FLUSH", "1")
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
        let total = ids.len() as u64;
        for (idx, id) in ids.iter().enumerate() {
            if cancelled() {
                return None;
            }
            writeln!(input, "{} -- {current} {id}", parents.of(id)).ok()?;
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
            super::gitprogress::report_counts(
                "Checking rebase conflicts",
                (idx + 1) as u64,
                Some(total),
                "commits",
                Some(id[..8.min(id.len())].to_owned()),
            );
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

fn preview_object_directory(root: &Path) -> Result<PathBuf> {
    // Git resolves linked worktrees, submodules, object-directory overrides and WSL.
    // Ask once per fallback replay, never once per intermediate commit.
    let path = git(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "objects",
        ],
    )?;
    let path = path.strip_suffix('\n').unwrap_or(&path);
    let directory = super::gitexec::host_path(root, path);
    if !directory.is_absolute() {
        return Err(fail("Git returned a relative object directory"));
    }
    Ok(directory)
}

fn existing_preview_commit(target: &Path, object: &[u8]) -> Result<bool> {
    let metadata = match target.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(fail(format!("Cannot read a Git preview object: {error}"))),
    };
    let mut existing = Vec::new();
    if !metadata.is_file()
        || File::open(target)
            .and_then(|file| {
                flate2::read::ZlibDecoder::new(file)
                    .take(object.len() as u64 + 1)
                    .read_to_end(&mut existing)
            })
            .is_err()
        || existing != object
    {
        return Err(fail(
            "An existing Git preview object has different contents",
        ));
    }
    Ok(true)
}

/// Older Git accepts commits rather than raw trees as the sides of merge-tree. Wrap
/// the intermediate tree in an unreferenced commit; no ref, index or working file changes.
/// Writing its canonical loose object avoids a commit-tree process for every replay step.
fn preview_commit(directory: &Path, tree: &str, hash_len: usize) -> Result<String> {
    use sha1::Digest;
    if !matches!(hash_len, 40 | 64)
        || tree.len() != hash_len
        || !tree.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(fail("Git returned an invalid tree for the conflict check"));
    }
    let payload = format!(
        "tree {}\nauthor Gall <preview@gall.invalid> 0 +0000\ncommitter Gall <preview@gall.invalid> 0 +0000\n\nConflict preview\n",
        tree.to_ascii_lowercase()
    );
    let mut object = format!("commit {}\0", payload.len()).into_bytes();
    object.extend_from_slice(payload.as_bytes());
    let id = if hash_len == 40 {
        sha1::Sha1::digest(&object)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    } else {
        sha2::Sha256::digest(&object)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let fanout = directory.join(&id[..2]);
    let target = fanout.join(&id[2..]);
    if existing_preview_commit(&target, &object)? {
        return Ok(id);
    }
    #[cfg(unix)]
    let object_permissions = {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mode = directory
            .metadata()
            .map_err(io_context("Cannot read Git object permissions"))?
            .permissions()
            .mode();
        // Shared Git object stores use setgid directories. Inherit that mode even when
        // the process umask is more restrictive, but never relax an existing directory.
        match std::fs::DirBuilder::new()
            .mode(mode & 0o2777)
            .create(&fanout)
        {
            Ok(()) => {
                std::fs::set_permissions(&fanout, std::fs::Permissions::from_mode(mode & 0o2777))
                    .map_err(io_context("Cannot set Git object directory permissions"))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(fail(format!("Cannot create Git preview objects: {error}"))),
        }
        // A user who can traverse the object store must be able to read its objects.
        // Loose objects remain read-only; Windows keeps the inherited directory ACL.
        std::fs::Permissions::from_mode((mode & 0o111) << 2)
    };
    #[cfg(not(unix))]
    std::fs::create_dir_all(&fanout).map_err(io_context("Cannot create Git preview objects"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(&fanout)
        .map_err(io_context("Cannot create a Git preview object"))?;
    {
        let mut compressed = flate2::write::ZlibEncoder::new(
            temporary.as_file_mut(),
            flate2::Compression::default(),
        );
        compressed
            .write_all(&object)
            .map_err(io_context("Cannot write a Git preview object"))?;
        compressed
            .finish()
            .map_err(io_context("Cannot finish a Git preview object"))?;
    }
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(object_permissions)
        .map_err(io_context("Cannot set Git preview object permissions"))?;
    // No user reference reaches these disposable objects. Atomic publication is enough;
    // forcing every intermediate tree to disk would add round trips over a WSL share.
    match temporary.persist_noclobber(&target) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another preview may have written the same immutable object. Reuse only an
            // identical regular object; never replace corrupt or conflicting data.
            if !existing_preview_commit(&target, &object)? {
                return Err(fail("The Git preview object disappeared while being saved"));
            }
        }
        Err(error) => {
            return Err(fail(format!(
                "Cannot save a Git preview object: {}",
                error.error
            )))
        }
    }
    Ok(id)
}

/// What a commit is replayed against: its parent, or the empty tree for a root commit.
#[derive(Default)]
struct Parents {
    roots: BTreeSet<String>,
    empty: String,
}
impl Parents {
    fn of(&self, id: &str) -> String {
        if self.roots.contains(id) {
            self.empty.clone()
        } else {
            format!("{id}^1")
        }
    }
}
/// The ID of the empty tree in the hash algorithm of the repository (Git knows it without storing it).
fn empty_tree(hash_len: usize) -> String {
    use sha1::Digest;
    let object = b"tree 0\0";
    let bytes: Vec<u8> = if hash_len == 40 {
        sha1::Sha1::digest(object).to_vec()
    } else {
        sha2::Sha256::digest(object).to_vec()
    };
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
/// `start_tree`: `base` is a tree (the empty one, for a plan from the root), which older Git does not accept as a side of the merge.
fn replay_serial(
    root: &Path,
    base: &str,
    ids: &[String],
    parents: &Parents,
    start_tree: bool,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<ConflictPrediction>> {
    if cancelled() {
        return Err(fail("Cancelled"));
    }
    if ids.len() > MAX_SERIAL {
        return Err(fail(format!(
            "Checking more than {MAX_SERIAL} commits for conflicts needs a newer Git"
        )));
    }
    let mut current = base.to_owned();
    let mut found = Vec::new();
    let mut objects = None;
    if start_tree {
        let directory = preview_object_directory(root)?;
        current = preview_commit(&directory, base, base.len())?;
        objects = Some(directory);
    }
    let total = ids.len() as u64;
    for (index, id) in ids.iter().enumerate() {
        if cancelled() {
            return Err(fail("Cancelled"));
        }
        let (tree, conflict) = merge_trees(root, &parents.of(id), &current, id)?;
        if let Some(files) = conflict {
            found.push(ConflictPrediction {
                id: id.clone(),
                files,
            });
        }
        super::gitprogress::report_counts(
            "Checking rebase conflicts",
            (index + 1) as u64,
            Some(total),
            "commits",
            Some(id[..8.min(id.len())].to_owned()),
        );
        if index + 1 < ids.len() {
            if objects.is_none() {
                objects = Some(preview_object_directory(root)?);
            }
            if let Some(directory) = &objects {
                current = preview_commit(directory, &tree, base.len())?;
            }
        }
    }
    Ok(found)
}

/// Replays the entries in memory, in order, and reports every commit that would stop the rebase
/// with a conflict. Nothing in the working tree, index or any reference changes; only
/// unreferenced tree and preview commit objects are written. Pick, reword, edit, squash and fixup all apply their changes to the
/// result so far, so they are checked alike; a dropped commit is skipped. After a conflict the
/// replay continues from the conflicted tree, so later reports assume the conflict gets resolved
/// without changing the commit's own side. Needs Git 2.40 or newer (`merge-tree --write-tree --merge-base`).
pub fn predict_conflicts(
    path: &Path,
    base: &str,
    entries: &[RewriteEntry],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<ConflictPrediction>> {
    let root = root(path)?;
    if entries.len() > MAX_COMMITS * 2 {
        return Err(fail(format!(
            "Select a range of at most {MAX_COMMITS} commits"
        )));
    }
    let mut ids = Vec::new();
    for entry in entries
        .iter()
        .filter(|e| !matches!(e.action, RewriteAction::Drop | RewriteAction::Exec))
    {
        if !matches!(entry.id.len(), 40 | 64) || !entry.id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(fail("Invalid commit ID in the rebase plan"));
        }
        ids.push(entry.id.to_ascii_lowercase());
    }
    if base == ROOT_BASE {
        let Some(first) = ids.first() else {
            return Ok(Vec::new());
        };
        let parents = Parents {
            roots: git(&root, &["rev-list", "--max-parents=0", "HEAD"])?
                .split_whitespace()
                .map(str::to_ascii_lowercase)
                .collect(),
            empty: empty_tree(first.len()),
        };
        if let Some(found) = replay_streamed(&root, &parents.empty, &ids, &parents, cancelled) {
            return Ok(found);
        }
        return replay_serial(&root, &parents.empty, &ids, &parents, true, cancelled);
    }
    let parents = Parents::default();
    // The base is checked while the replay runs: it only writes unreferenced trees, and its
    // result is used only when the base is a commit.
    let (base, streamed) = std::thread::scope(|scope| {
        let checked = scope.spawn(|| commit_id(&root, base));
        let streamed = (base.len() == 40 || base.len() == 64)
            .then(|| replay_streamed(&root, &base.to_ascii_lowercase(), &ids, &parents, cancelled))
            .flatten();
        (super::gitgraph::joined(checked), streamed)
    });
    let base = base??;
    if let Some(found) = streamed {
        return Ok(found);
    }
    replay_serial(&root, &base, &ids, &parents, false, cancelled)
}
/// The command of an `exec` line: one line, since a line break would add lines to the plan Git runs.
fn exec_command(command: &str) -> Result<()> {
    if command.trim().is_empty() {
        return Err(fail("An exec line needs a command"));
    }
    if command.contains(['\n', '\r', '\0']) {
        return Err(fail("An exec command must be a single line"));
    }
    if command.len() > MAX_EXEC {
        return Err(fail("An exec command is too long"));
    }
    Ok(())
}
fn validate_entries(plan: &RewritePlan, entries: &[RewriteEntry]) -> Result<()> {
    let expected: BTreeSet<_> = plan.commits.iter().map(|c| c.id.as_str()).collect();
    let commits: Vec<_> = entries
        .iter()
        .filter(|c| c.action != RewriteAction::Exec)
        .collect();
    let actual: BTreeSet<_> = commits.iter().map(|c| c.id.as_str()).collect();
    if commits.is_empty() || commits.len() != expected.len() || actual != expected {
        return Err(fail(
            "The plan must contain every original commit exactly once; use Drop to remove a commit",
        ));
    }
    let mut prior = false;
    let mut after_exec = false;
    let mut message_size = 0usize;
    for entry in entries {
        if entry.action == RewriteAction::Exec {
            exec_command(&entry.message)?;
            after_exec = true;
            message_size = message_size.saturating_add(entry.message.len());
            continue;
        }
        if matches!(entry.action, RewriteAction::Squash | RewriteAction::Fixup) {
            if !prior {
                return Err(fail("Squash and fixup need a preceding retained commit"));
            }
            if after_exec {
                return Err(fail(
                    "Move the exec line after the commits that are squashed or fixed up",
                ));
            }
        }
        if entry.action != RewriteAction::Drop {
            prior = true;
            after_exec = false;
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
#[cfg(test)]
fn clean_guard(path: &Path) -> Result<()> {
    guard(path, false).map(|_| ())
}
/// Check whether rebase can run and whether to ask Git to autostash tracked edits.
fn guard(path: &Path, allow_tracked: bool) -> Result<bool> {
    let state = super::gitrepository::snapshot(path)?;
    if state.bisect_active {
        return Err(fail("Finish bisect before rewriting history"));
    }
    if state.operation.is_some() || !state.conflicts.is_empty() {
        return Err(fail(
            "Finish or abort the current Git operation before rewriting history",
        ));
    }
    // Submodules are supported: a rebase rewrites the superproject's commits (a submodule is just a
    // recorded commit ID there) and never touches the submodule's own working folder. Edits and
    // untracked files inside a submodule are therefore safe and ignored here, but a changed
    // submodule commit (staged or not) is a local change of this repository like any other.
    let changes = git(
        path,
        &[
            "status",
            "--porcelain",
            "-z",
            // Untracked files are no local change for a rebase; `protect_ignored` refuses the
            // ones it could overwrite.
            "--untracked-files=no",
            "--ignore-submodules=dirty",
        ],
    )?;
    if !changes.is_empty() {
        if allow_tracked {
            super::gitrepository::protect_staged_gitlinks(path)?;
            super::gitrepository::verify_filtered_autostash(path)?;
            return Ok(true);
        }
        return Err(fail(
            "Commit or stash local changes before rewriting history (a submodule checked out at a different commit than this repository records counts as one)",
        ));
    }
    super::gitrepository::verify_filtered_autostash(path)?;
    Ok(false)
}
/// After a finished rebase: submodules whose checked-out commit is not the one the rewritten history
/// records. Gall never changes a submodule's folder, so the user decides when to update it.
pub(super) fn submodule_note(path: &Path) -> String {
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
    let base = (plan.base != ROOT_BASE).then_some(plan.base.as_str());
    // Every file the rebase can write is in the base, in the result, or changed by a commit of the
    // plan. One query for the changes: a tree per commit was a process per commit.
    let range = match base {
        Some(base) => format!("{base}..{}", plan.head),
        None => plan.head.clone(),
    };
    let changed = [
        "log",
        "--no-show-signature",
        "--no-renames",
        "--no-merges",
        "--format=",
        "--name-only",
        "-z",
        &range,
        "--",
    ];
    let mut lists = vec![git(path, &changed)?];
    for id in base.into_iter().chain([plan.head.as_str()]) {
        lists.push(git(path, &["ls-tree", "-r", "--name-only", "-z", id])?);
    }
    for list in &lists {
        for file in list
            .split('\0')
            .map(|name| name.trim_start_matches('\n'))
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
        if entry.action == RewriteAction::Exec {
            exec_command(&entry.message)?;
            continue;
        }
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
        // The helper would refuse to read it in the middle of the rebase.
        if bytes.len() as u64 > MAX_PLAN {
            return Err(fail("The rebase plan is too large"));
        }
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
    let editor = |mode| {
        if super::gitexec::place(path).is_none() {
            return editor_command(executable, mode, plan);
        }
        let exe = shell_quote(&crate::nav::path::strip_verbatim(executable.to_path_buf()))?;
        let plan = shell_quote(&crate::nav::path::strip_verbatim(plan.to_path_buf()))?;
        // Git appends its editor target as an argument. Translate it without evaluating it.
        Ok(format!(
            "gall_editor() {{ exe=$(wslpath -au -- {exe}) && target=$(wslpath -aw -- \"$1\") && exec \"$exe\" {mode} {plan} \"$target\"; }}; gall_editor"
        ))
    };
    let mut cmd = super::gitexec::command(path, super::gitstatus::program());
    cmd.args(["-c", "core.fsmonitor=false", "-c"])
        .arg(format!(
            "core.hooksPath={}",
            super::gitexec::null_device(path)
        ))
        .args([
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
        .arg(super::gitexec::dir_arg(path))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_SEQUENCE_EDITOR", editor("--git-rebase-editor")?)
        .env("GIT_EDITOR", editor("--git-rebase-message")?)
        .env("GIT_TERMINAL_PROMPT", "0");
    super::gitexec::forward_env(&mut cmd);
    let out = if let Some(tracker) = super::gitprogress::active() {
        super::gitprogress::run_streaming_output(&mut cmd, &tracker)
    } else {
        cmd.output()
    }
    .map_err(io_context(
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
    let label = format!("{} commits", entries.len());
    journaled(path, label, || {
        execute_now(path, entries, base, expected, editor_executable)
    })
}

/// Runs a rewrite as one entry of the operation journal.
fn journaled(path: &Path, label: String, run: impl FnOnce() -> Result<String>) -> Result<String> {
    let operation = Some((super::gitjournal::Kind::Rewrite, label));
    super::gitjournal::around(path, operation, run)
}

fn execute_now(
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
    let autostash = guard(&root, true)?;
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
            if autostash {
                "--autostash"
            } else {
                "--no-autostash"
            },
            "--no-update-refs",
            "--no-fork-point",
            "--no-rebase-merges",
            "--keep-empty",
            "--empty=keep",
            "--reapply-cherry-picks",
            // The root plan replays every commit of the history.
            if current.base == ROOT_BASE {
                "--root"
            } else {
                current.base.as_str()
            },
        ],
        &saved,
        &executable,
    );
    cleanup(&root, &saved);
    if output.is_ok() && !rebase_in_progress(&directory) {
        super::gitgraph::retain_detached(&root).map_err(|error| {
            fail(format!("The rewrite completed, but its detached commit could not be saved for recovery: {error}"))
        })?;
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
    _editor_executable: &Path,
) -> Result<String> {
    let label = message.trim().lines().next().unwrap_or_default();
    let label = label.chars().take(100).collect();
    journaled(path, label, || reword_now(path, target, message, expected))
}

/// Recreate a commit object with remapped parents without checking out its tree.
pub(super) fn rewrite_commit_object(
    root: &Path,
    id: &str,
    rewritten: &std::collections::HashMap<String, String>,
    message: Option<&str>,
    tree: Option<&str>,
) -> Result<String> {
    let (object, truncated) =
        super::gitinspect::read_git(root, &["cat-file", "commit", id], MAX_PLAN as usize)?;
    if truncated {
        return Err(fail("A commit exceeds the history rewrite size limit"));
    }
    let Some(separator) = object.windows(2).position(|pair| pair == b"\n\n") else {
        return Err(fail("Git returned a commit without a message separator"));
    };
    let mut updated = Vec::with_capacity(object.len());
    let mut omit = false;
    for line in object[..separator].split(|byte| *byte == b'\n') {
        if !line.starts_with(b" ") {
            // Signatures and signed merge tags describe the original commit or parents.
            omit = line.starts_with(b"gpgsig ")
                || line.starts_with(b"gpgsig-sha256 ")
                || line.starts_with(b"mergetag ");
        }
        if omit {
            continue;
        }
        let parent = line
            .strip_prefix(b"parent ")
            .and_then(|id| std::str::from_utf8(id).ok());
        if let Some(tree) = tree.filter(|_| line.starts_with(b"tree ")) {
            updated.extend_from_slice(b"tree ");
            updated.extend_from_slice(tree.as_bytes());
        } else if let Some(mapped) = parent.and_then(|id| rewritten.get(id)) {
            updated.extend_from_slice(b"parent ");
            updated.extend_from_slice(mapped.as_bytes());
        } else {
            updated.extend_from_slice(line);
        }
        updated.push(b'\n');
    }
    updated.push(b'\n');
    if let Some(message) = message {
        updated.extend_from_slice(message.trim_end_matches('\n').as_bytes());
        updated.push(b'\n');
    } else {
        updated.extend_from_slice(&object[separator + 2..]);
    }
    let mut command = super::gitexec::command(root, super::gitstatus::program());
    command
        .arg("-C")
        .arg(super::gitexec::dir_arg(root))
        .args(["hash-object", "-w", "-t", "commit", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    super::gitexec::forward_env(&mut command);
    let mut child = command.spawn().map_err(|e| fail(e.to_string()))?;
    let written = child
        .stdin
        .take()
        .ok_or_else(|| fail("Git did not open commit input"))?
        .write_all(&updated);
    let output = child.wait_with_output().map_err(|e| fail(e.to_string()))?;
    if !output.status.success() {
        return Err(fail(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    written.map_err(|e| fail(e.to_string()))?;
    let new_id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if new_id.len() != id.len() || !new_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(fail("Git returned an invalid rewritten commit ID"));
    }
    Ok(new_id)
}

fn descendants(root: &Path, head: &str, target: &str) -> Result<Vec<String>> {
    git(root, &["merge-base", "--is-ancestor", target, head])
        .map_err(|_| fail("The selected commit is not on the current branch"))?;
    let range = format!("{target}..{head}");
    let count = format!("--max-count={MAX_COMMITS}");
    let (output, truncated) = super::gitinspect::read_git(
        root,
        &[
            "rev-list",
            "--ancestry-path",
            "--topo-order",
            "--reverse",
            &count,
            &range,
        ],
        MAX_PLAN as usize,
    )?;
    let descendants = std::str::from_utf8(&output).map_err(|_| fail("Invalid commit list"))?;
    if truncated || descendants.lines().count() >= MAX_COMMITS {
        return Err(fail("Too many descendant commits to rewrite at once"));
    }

    Ok(descendants.lines().map(str::to_owned).collect())
}

fn reword_ancestor(
    root: &Path,
    head: &str,
    target: &str,
    message: &str,
    expected: &str,
) -> Result<String> {
    let descendants = descendants(root, head, target)?;
    let mut rewritten = std::collections::HashMap::new();
    for id in std::iter::once(target).chain(descendants.iter().map(String::as_str)) {
        let new_id = rewrite_commit_object(
            root,
            id,
            &rewritten,
            (id == target).then_some(message),
            None,
        )?;
        rewritten.insert(id.to_owned(), new_id);
    }
    let new_head = rewritten
        .get(head)
        .ok_or_else(|| fail("Could not rewrite the current revision"))?;
    let state = super::gitrepository::snapshot(root)?;
    if state.token != expected {
        return Err(fail(
            "The repository changed. Refresh before rewording this commit",
        ));
    }
    super::gitgraph::retain_commit(root, head, "Gall recovery before rewording history")?;
    if state.branch.is_none() {
        super::gitgraph::retain_commit(root, new_head, "Gall reworded detached commit")?;
    }
    git(
        root,
        &[
            "update-ref",
            "-m",
            "Gall reword commit",
            "HEAD",
            new_head,
            head,
        ],
    )?;
    Ok(format!("Reworded {target}"))
}

/// Fold the staged delta into the selected commit while keeping the working tree and index.
pub fn amend_staged(path: &Path, target: &str, expected: &str) -> Result<String> {
    let label = target.get(..8).unwrap_or(target).to_owned();
    super::gitjournal::around(path, Some((super::gitjournal::Kind::Amend, label)), || {
        let _busy = super::gitfacts::mutating();
        let root = root(path)?;
        let state = super::gitrepository::snapshot(&root)?;
        let stale = || fail("The repository changed. Review the staged changes and try again.");
        if state.token != expected {
            return Err(stale());
        }
        if state.bisect_active || state.operation.is_some() || !state.conflicts.is_empty() {
            return Err(fail(
                "Finish or abort the current Git operation before amending a commit.",
            ));
        }
        let head = &state.head;
        let target = commit_id(&root, target)?;
        let descendants = descendants(&root, head, &target)?;
        let write_tree = || git(&root, &["write-tree"]).map(|tree| tree.trim().to_owned());
        let staged = write_tree()?;
        if git(&root, &["rev-parse", "HEAD^{tree}"])?.trim() == staged {
            return Err(fail(
                "Stage the changes to include in Working Changes first.",
            ));
        }
        let mut rewritten = std::collections::HashMap::new();
        let staged_commit = if target == *head {
            None
        } else {
            Some(rewrite_commit_object(
                &root,
                head,
                &rewritten,
                None,
                Some(&staged),
            )?)
        };

        for id in std::iter::once(&target).chain(&descendants) {
            let tree = if id == head {
                staged.clone()
            } else {
                let source = staged_commit
                    .as_deref()
                    .ok_or_else(|| fail("The staged commit is missing"))?;
                let (tree, conflicts) = merge_trees(&root, head, id, source)?;
                if let Some(files) = conflicts {
                    return Err(fail(format!(
                        "The staged changes conflict with {} at {}. Review the staged paths in Working Changes, or use an interactive rebase edit stop to resolve conflicts.",
                        &id[..12],
                        files.join(", ")
                    )));
                }
                tree
            };
            let new_id = rewrite_commit_object(&root, id, &rewritten, None, Some(&tree))?;
            rewritten.insert(id.clone(), new_id);
        }

        let new_head = rewritten
            .get(head)
            .ok_or_else(|| fail("Could not rewrite the current revision"))?;
        if super::gitrepository::snapshot(&root)?.token != expected || write_tree()? != staged {
            return Err(stale());
        }
        super::gitgraph::retain_commit(
            &root,
            head,
            "Gall recovery before amending staged changes",
        )?;
        if state.branch.is_none() {
            super::gitgraph::retain_commit(&root, new_head, "Gall amended detached commit")?;
        }
        git(
            &root,
            &[
                "update-ref",
                "-m",
                "Gall amend staged changes",
                "HEAD",
                new_head,
                head,
            ],
        )?;
        Ok(format!("Amended {target}"))
    })
}

fn reword_now(path: &Path, target: &str, message: &str, expected: &str) -> Result<String> {
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
    if snapshot.operation.is_some() || !snapshot.conflicts.is_empty() {
        return Err(fail(
            "Finish or abort the current Git operation before rewording",
        ));
    }
    reword_ancestor(&root, &snapshot.head, &target, message, expected)
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
    let label = target.get(..8).unwrap_or(target).to_owned();
    journaled(path, label, || {
        quick_edit_now(path, target, action, expected, editor_executable)
    })
}

/// Remove a merge and the commits introduced by it, keeping later merge structure.
fn drop_merge(root: &Path, target: &str, first_parent: &str, expected: &str) -> Result<String> {
    let _busy = super::gitfacts::mutating();
    let state = super::gitrepository::snapshot(root)?;
    if state.token != expected {
        return Err(fail(
            "The repository changed. Refresh before editing this commit",
        ));
    }
    replay_first_parent_tail(root, &state, target, first_parent)
}

fn replay_first_parent_tail(
    root: &Path,
    state: &super::gitrepository::Repository,
    target: &str,
    onto: &str,
) -> Result<String> {
    let line = git(root, &["rev-list", "--first-parent", &state.head])?;
    if !line.lines().any(|id| id == target) {
        return Err(fail(
            "Select a commit on the current branch's first-parent history.",
        ));
    }
    let autostash = guard(root, true)?;
    super::gitrepository::protect_untracked(root, &[onto], Some(target))?;
    super::gitgraph::retain_commit(root, &state.head, "Gall recovery before rewriting a merge")?;
    let output = git(
        root,
        &[
            "-c",
            "core.editor=true",
            "-c",
            "submodule.recurse=false",
            "rebase",
            "--merge",
            "--rebase-merges",
            "--no-update-refs",
            "--no-fork-point",
            "--no-autosquash",
            "--reapply-cherry-picks",
            "--empty=keep",
            if autostash {
                "--autostash"
            } else {
                "--no-autostash"
            },
            "--onto",
            onto,
            target,
        ],
    )?;
    if state.branch.is_none() {
        super::gitgraph::retain_detached(root).map_err(|error| {
            fail(format!("The rewrite completed, but its detached commit could not be saved for recovery: {error}"))
        })?;
    }
    Ok(output)
}

fn combine_with_merge(
    root: &Path,
    state: &super::gitrepository::Repository,
    target: &str,
    parent: &str,
    action: QuickAction,
) -> Result<String> {
    let _busy = super::gitfacts::mutating();
    let tree = git(root, &["rev-parse", &format!("{target}^{{tree}}")])?;
    let message = if action == QuickAction::Squash {
        let parent = git(root, &["show", "-s", "--format=%B", parent])?;
        let child = git(root, &["show", "-s", "--format=%B", target])?;
        Some(format!(
            "{}\n\n{}",
            parent.trim_end_matches('\n'),
            child.trim_end_matches('\n')
        ))
    } else {
        None
    };
    let combined = rewrite_commit_object(
        root,
        parent,
        &std::collections::HashMap::new(),
        message.as_deref(),
        Some(tree.trim()),
    )?;
    if super::gitrepository::snapshot(root)?.token != state.token {
        return Err(fail(
            "The repository changed. Refresh before editing this commit",
        ));
    }
    if state.head != target {
        return replay_first_parent_tail(root, state, target, &combined);
    }
    if state.operation.is_some() || !state.conflicts.is_empty() {
        return Err(fail(
            "Finish or abort the current Git operation before rewriting history",
        ));
    }
    super::gitgraph::retain_commit(root, target, "Gall recovery before rewriting a merge")?;
    if state.branch.is_none() {
        super::gitgraph::retain_commit(root, &combined, "Gall combined detached merge")?;
    }
    git(
        root,
        &[
            "update-ref",
            "-m",
            "Gall combine into merge",
            "HEAD",
            &combined,
            target,
        ],
    )
}

fn quick_edit_now(
    path: &Path,
    target: &str,
    action: QuickAction,
    expected: &str,
    editor_executable: &Path,
) -> Result<String> {
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
    let (parent, base) = match parents.as_slice() {
        [] if matches!(action, QuickAction::Drop | QuickAction::MoveLater) => {
            (None, ROOT_BASE.to_owned())
        }
        [] => return Err(fail("The first commit has no earlier commit")),
        [parent] => {
            let base = if matches!(
                action,
                QuickAction::Fixup | QuickAction::Squash | QuickAction::MoveEarlier
            ) {
                let grandparents = git(&root, &["show", "-s", "--format=%P", parent])?;
                let grandparents: Vec<_> = grandparents.split_whitespace().collect();
                match grandparents.as_slice() {
                    [] => ROOT_BASE.to_owned(),
                    [base] => (*base).to_owned(),
                    _ if matches!(action, QuickAction::Fixup | QuickAction::Squash) => {
                        return combine_with_merge(&root, &snapshot, &target, parent, action);
                    }
                    _ => return Err(fail("The previous commit is a merge. Use Reword to change its message while preserving its parents")),
                }
            } else {
                (*parent).to_owned()
            };
            (Some(*parent), base)
        }
        [first_parent, ..] if action == QuickAction::Drop => {
            return drop_merge(&root, &target, first_parent, expected);
        }
        _ => return Err(fail("Direct history actions cannot replay a merge commit. Use Reword to change its message or Revert to reverse its changes.")),
    };
    let plan = plan(&root, &base)?;
    if plan.skipped_merges != 0 {
        return Err(fail("This action would replay one or more merges as linear history. Open the rebase plan to review the merges first."));
    }
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
            if selected == 0 || Some(entries[selected - 1].id.as_str()) != parent {
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
        QuickAction::Drop => {
            if base == ROOT_BASE && entries.len() == 1 {
                return Err(fail("Dropping the only commit would leave no commit for this branch. Amend it to change its contents."));
            }
            entries[selected].action = RewriteAction::Drop;
        }
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
    execute_now(&root, entries, &base, &plan.token, editor_executable)
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
            ids.push(format!("{id}^{{commit}}"));
        }
    }
    // A long sequence is resolved in groups, not with a process for each commit.
    let mut full = Vec::with_capacity(ids.len());
    for group in ids.chunks(200) {
        let mut args = vec!["rev-parse"];
        args.extend(group.iter().map(String::as_str));
        let found = git(path, &args)?;
        let found: Vec<&str> = found.lines().map(str::trim).collect();
        if found.len() != group.len() {
            return Err(fail(
                "The pending Git sequence contains an invalid commit ID",
            ));
        }
        full.extend(found.into_iter().map(str::to_owned));
    }
    Ok(full)
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
            .map(|entry| {
                if entry.action == RewriteAction::Exec {
                    format!("exec {}\n", entry.message)
                } else {
                    format!("{} {}\n", entry.action.name(), entry.id)
                }
            })
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
    #[test]
    fn streamed_replay_requires_a_version_with_tree_inputs_and_incremental_replies() {
        for version in [
            "git version 2.49.0",
            "git version 2.49.1.windows.1\n",
            "git version 2.56.1",
            "git version 2.50.0 (Apple Git-200)",
            "git version 3.0.0",
        ] {
            assert!(git_supports_streamed_replay(version), "{version}");
        }
        for version in [
            "git version 2.40.0",
            "git version 2.43.0",
            "git version 2.48.9",
            "git version 2.49",
            "git version 2.49.bad",
            "git version 2.49.0-rc0",
            "git version unknown",
            "2.56.1",
            "",
        ] {
            assert!(!git_supports_streamed_replay(version), "{version}");
        }
    }

    fn repository() -> tempfile::TempDir {
        let temp = tempfile::Builder::new()
            .prefix("gall-rewrite ' $ test-")
            .tempdir()
            .unwrap();
        let path = temp.path();
        git(path, &["init", "-q"]).unwrap();
        git(path, &["config", "user.name", "Test"]).unwrap();
        git(path, &["config", "user.email", "test@example.com"]).unwrap();
        git(path, &["config", "core.autocrlf", "false"]).unwrap();
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
            ids(super::predict_conflicts(path, base.trim(), entries, &|| false).unwrap())
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
        let found =
            super::predict_conflicts(path, base.trim(), &[pick(&b), pick(&a)], &|| false).unwrap();
        assert_eq!(found[0].files, ["shared.txt"]);
        assert!(super::predict_conflicts(path, base.trim(), &[pick("zz")], &|| false).is_err());
        assert_eq!(git(path, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(git(path, &["status", "--porcelain"]).unwrap(), status);
        assert!(!path.join(".git/rebase-merge").exists());
    }
    #[test]
    #[cfg(unix)]
    fn preview_objects_preserve_shared_store_access_and_existing_directory_permissions() {
        use std::os::unix::fs::PermissionsExt;
        for (mode, readable) in [(0o700, 0o400), (0o2770, 0o440), (0o2775, 0o444)] {
            let temp = tempfile::tempdir().unwrap();
            let objects = temp.path().join("objects");
            std::fs::create_dir(&objects).unwrap();
            std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(mode)).unwrap();
            let tree = "1".repeat(40);
            let id = preview_commit(&objects, &tree, 40).unwrap();
            let fanout = objects.join(&id[..2]);
            let object = fanout.join(&id[2..]);
            assert_eq!(
                fanout.metadata().unwrap().permissions().mode() & 0o2777,
                mode
            );
            assert_eq!(
                object.metadata().unwrap().permissions().mode() & 0o777,
                readable
            );
            // Existing fanout restrictions belong to the repository: writing another
            // preview must not relax them, even in a more permissive shared store.
            std::fs::remove_file(&object).unwrap();
            std::fs::set_permissions(&fanout, std::fs::Permissions::from_mode(0o2700)).unwrap();
            assert_eq!(preview_commit(&objects, &tree, 40).unwrap(), id);
            assert_eq!(
                fanout.metadata().unwrap().permissions().mode() & 0o2777,
                0o2700
            );
            assert_eq!(
                objects.metadata().unwrap().permissions().mode() & 0o2777,
                mode
            );
        }
    }

    #[test]
    fn preview_commits_are_valid_sha1_and_sha256_objects_and_never_overwrite_existing_data() {
        for format in ["sha1", "sha256"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path();
            if let Err(error) = git(path, &["init", "-q", &format!("--object-format={format}")]) {
                if format == "sha256"
                    && (error.to_string().contains("unknown option")
                        || error.to_string().contains("not supported"))
                {
                    eprintln!("Git does not support SHA-256 repositories: skipped");
                    continue;
                }
                panic!("Cannot initialize {format} repository: {error}");
            }
            git(path, &["config", "user.name", "Test"]).unwrap();
            git(path, &["config", "user.email", "test@example.com"]).unwrap();
            git(path, &["config", "core.autocrlf", "false"]).unwrap();
            let head = commit(path, "base", "base.txt");
            let tree = git(path, &["rev-parse", "HEAD^{tree}"])
                .unwrap()
                .trim()
                .to_owned();
            let directory = preview_object_directory(path).unwrap();
            let wrapped = preview_commit(&directory, &tree, head.len()).unwrap();
            assert_eq!(
                git(path, &["cat-file", "-t", &wrapped]).unwrap().trim(),
                "commit"
            );
            assert_eq!(
                git(path, &["rev-parse", &format!("{wrapped}^{{tree}}")])
                    .unwrap()
                    .trim(),
                tree
            );
            // Exercise successive compatibility merges with both object formats, not
            // just acceptance of the standalone wrapper by cat-file.
            let a = commit(path, "a", "shared.txt");
            let b = commit(path, "b", "shared.txt");
            let refs = git(path, &["show-ref"]).unwrap();
            let index_path = git_dir(path).unwrap().join("index");
            let index = std::fs::read(&index_path).unwrap();
            assert!(replay_serial(
                path,
                &head,
                &[a.clone(), b.clone()],
                &Parents::default(),
                false,
                &|| false
            )
            .unwrap()
            .is_empty());
            let conflicts = replay_serial(
                path,
                &head,
                &[b.clone(), a.clone()],
                &Parents::default(),
                false,
                &|| false,
            )
            .unwrap();
            assert_eq!(
                conflicts.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
                [&b, &a]
            );
            assert_eq!(git(path, &["show-ref"]).unwrap(), refs);
            assert_eq!(std::fs::read(index_path).unwrap(), index);
            assert_eq!(std::fs::read(path.join("shared.txt")).unwrap(), b"b\n");
            let object = directory.join(&wrapped[..2]).join(&wrapped[2..]);
            let original = std::fs::read(&object).unwrap();
            let written = object.metadata().unwrap().modified().unwrap();
            assert_eq!(
                preview_commit(&directory, &tree, head.len()).unwrap(),
                wrapped
            );
            assert_eq!(std::fs::read(&object).unwrap(), original);
            assert_eq!(object.metadata().unwrap().modified().unwrap(), written);
            // A hash-shaped pathname alone is not proof that an existing object is valid.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            std::fs::write(&object, b"preserve conflicting data").unwrap();
            assert!(preview_commit(&directory, &tree, head.len()).is_err());
            assert_eq!(
                std::fs::read(&object).unwrap(),
                b"preserve conflicting data"
            );
            assert!(preview_commit(&directory, "not a tree", head.len()).is_err());
            assert!(preview_commit(&directory, &tree, 12).is_err());
        }
    }

    #[test]
    fn compatible_replay_preserves_refs_index_and_working_files_for_clean_and_conflicting_plans() {
        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        let a = commit(path, "a", "shared.txt");
        let b = commit(path, "b", "shared.txt");
        let c = commit(path, "c", "independent.txt");
        std::fs::write(path.join("shared.txt"), "unsaved working content\n").unwrap();
        std::fs::write(path.join("untracked.txt"), "untracked content\n").unwrap();
        let refs = git(path, &["show-ref"]).unwrap();
        let head = git(path, &["rev-parse", "HEAD"]).unwrap();
        let index_path = git_dir(path).unwrap().join("index");
        let index = std::fs::read(&index_path).unwrap();
        let status = git(path, &["status", "--porcelain"]).unwrap();
        assert!(replay_serial(
            path,
            &base,
            &[a.clone(), b.clone(), c.clone()],
            &Parents::default(),
            false,
            &|| false
        )
        .unwrap()
        .is_empty());
        let conflicts = replay_serial(
            path,
            &base,
            &[b.clone(), a.clone(), c.clone()],
            &Parents::default(),
            false,
            &|| false,
        )
        .unwrap();
        assert_eq!(
            conflicts.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
            [&b, &a]
        );
        assert_eq!(conflicts[0].files, ["shared.txt"]);
        assert!(
            replay_serial(path, &base, &[c, a, b], &Parents::default(), false, &|| {
                false
            })
            .unwrap()
            .is_empty()
        );
        assert_eq!(git(path, &["show-ref"]).unwrap(), refs);
        assert_eq!(git(path, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(git(path, &["status", "--porcelain"]).unwrap(), status);
        assert_eq!(std::fs::read(index_path).unwrap(), index);
        assert_eq!(
            std::fs::read(path.join("shared.txt")).unwrap(),
            b"unsaved working content\n"
        );
        assert_eq!(
            std::fs::read(path.join("untracked.txt")).unwrap(),
            b"untracked content\n"
        );
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
        // The compatibility path uses one merge-tree per commit and unreferenced commit
        // wrappers, so it also works with Git versions that reject raw tree inputs.
        let serial = |ids: &[&String]| {
            let ids = ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
            replay_serial(path, &base, &ids, &Parents::default(), false, &|| false)
                .unwrap()
                .into_iter()
                .map(|entry| (entry.id, entry.files))
                .collect::<Vec<_>>()
        };
        for order in [
            vec![&a, &b, &c, &d],
            vec![&d, &c, &b, &a],
            vec![&b, &a, &d],
            vec![&c],
        ] {
            let ids: Vec<String> = order.iter().map(|id| id.to_string()).collect();
            let compatible = serial(&order);
            if let Some(streamed) =
                replay_streamed(path, &base, &ids, &Parents::default(), &|| false)
            {
                let streamed = streamed
                    .into_iter()
                    .map(|entry| (entry.id, entry.files))
                    .collect::<Vec<_>>();
                assert_eq!(streamed, compatible, "{order:?}");
            } else {
                let entries = ids
                    .iter()
                    .map(|id| entry(id, RewriteAction::Pick, ""))
                    .collect::<Vec<_>>();
                let predicted = predict_conflicts(path, &base, &entries, &|| false)
                    .unwrap()
                    .into_iter()
                    .map(|entry| (entry.id, entry.files))
                    .collect::<Vec<_>>();
                assert_eq!(predicted, compatible, "{order:?}");
            }
        }
        assert!(
            !serial(&[&b, &a]).is_empty(),
            "the orders include conflicts"
        );
        // A commit Git does not know: no answer here, so the caller merges one by one and
        // reports Git's error.
        let unknown = "0".repeat(base.len());
        assert!(replay_streamed(
            path,
            &base,
            &[a.clone(), unknown.clone()],
            &Parents::default(),
            &|| false
        )
        .is_none());
        assert!(super::predict_conflicts(
            path,
            &base,
            &[entry(&unknown, RewriteAction::Pick, "")],
            &|| false
        )
        .is_err());
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
        assert_eq!(plan.commits[0].subject, "first");
        git(
            path,
            &["commit", "-q", "--amend", "-m", "second\n\nbody\nmore\n"],
        )
        .unwrap();
        let amended = git(path, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        let asked = messages(path, &[amended.clone(), a.clone()]).unwrap();
        let found: Vec<_> = asked
            .iter()
            .map(|m| (m.id.as_str(), m.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (amended.as_str(), "second\n\nbody\nmore"),
                (a.as_str(), "first")
            ]
        );
        assert!(messages(path, &["HEAD".into()]).is_err());
        assert!(messages(path, &[]).unwrap().is_empty());
        git(path, &["reset", "-q", "--hard", &b]).unwrap();
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
    fn untracked_files_do_not_block_a_rewrite_unless_it_would_overwrite_them() {
        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        commit(path, "second", "second.txt");
        // Not in .gitignore and not touched by the plan: allowed, like in Git and lazygit.
        std::fs::write(path.join("scratch.log"), "local\n").unwrap();
        std::fs::create_dir(path.join("build")).unwrap();
        std::fs::write(path.join("build").join("out.bin"), "local\n").unwrap();
        clean_guard(path).unwrap();
        let plan = super::plan(path, base.trim()).unwrap();
        protect_ignored(path, &plan).unwrap();
        // A tracked change still blocks, and so does an untracked file a commit of the plan writes.
        std::fs::write(path.join("base.txt"), "edited\n").unwrap();
        assert!(clean_guard(path).is_err());
        git(path, &["checkout", "-q", "--", "base.txt"]).unwrap();
        git(path, &["rm", "-q", "--cached", "second.txt"]).unwrap();
        git(path, &["commit", "-qm", "untrack second"]).unwrap();
        let plan = super::plan(path, base.trim()).unwrap();
        assert!(protect_ignored(path, &plan).is_err());
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
    fn the_first_commit_can_be_reworded_in_the_app() {
        let temp = repository();
        let path = temp.path();
        let root_id = git(path, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        commit(path, "second", "b.txt");
        commit(path, "third", "c.txt");
        // Even with a configured filter, rewording never needs to rewrite this tree.
        git(path, &["config", "filter.unused.clean", "false"]).unwrap();
        let tree = git(path, &["rev-parse", "HEAD^{tree}"]).unwrap();
        std::fs::write(path.join("base.txt"), "local edit\n").unwrap();
        let token = super::super::gitrepository::snapshot(path).unwrap().token;
        reword(
            path,
            &root_id,
            "new first message",
            &token,
            Path::new("unused"),
        )
        .unwrap();
        let subjects = git(path, &["log", "--reverse", "--format=%s"]).unwrap();
        assert_eq!(
            subjects.lines().collect::<Vec<_>>(),
            ["new first message", "second", "third"]
        );
        assert_eq!(
            std::fs::read_to_string(path.join("base.txt")).unwrap(),
            "local edit\n"
        );
        assert!(git(path, &["cat-file", "-e", &format!("{root_id}^{{commit}}")]).is_ok());
        assert_eq!(git(path, &["rev-parse", "HEAD^{tree}"]).unwrap(), tree);
        assert!(!rebase_in_progress(&git_dir(path).unwrap()));
    }

    #[test]
    fn rewording_an_older_merge_preserves_its_parents_tree_and_local_changes() {
        let temp = repository();
        let path = temp.path();
        git(path, &["checkout", "-q", "-b", "side"]).unwrap();
        commit(path, "side", "side.txt");
        git(path, &["checkout", "-q", "-"]).unwrap();
        git(
            path,
            &["merge", "--no-ff", "-q", "-m", "merge side", "side"],
        )
        .unwrap();
        let merge = git(path, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        commit(path, "after merge", "d.txt");
        let old_head = git(path, &["rev-parse", "HEAD"]).unwrap();
        let old_merge_parents = git(path, &["show", "-s", "--format=%P", &merge]).unwrap();
        let old_tree = git(path, &["rev-parse", "HEAD^{tree}"]).unwrap();
        std::fs::write(path.join("base.txt"), "staged local\n").unwrap();
        git(path, &["add", "base.txt"]).unwrap();
        std::fs::write(path.join("base.txt"), "worktree local\n").unwrap();
        let token = super::super::gitrepository::snapshot(path).unwrap().token;
        let unused_helper = Path::new("unused");
        assert!(reword(path, &merge, "old request", "stale-token", unused_helper).is_err());
        assert_eq!(git(path, &["rev-parse", "HEAD"]).unwrap(), old_head);
        reword(
            path,
            &merge,
            "New merge subject\n\nNew body",
            &token,
            unused_helper,
        )
        .unwrap();
        assert_eq!(
            git(path, &["show", "-s", "--format=%P", "HEAD^"]).unwrap(),
            old_merge_parents
        );
        assert_eq!(
            git(path, &["show", "-s", "--format=%B", "HEAD^"])
                .unwrap()
                .trim(),
            "New merge subject\n\nNew body"
        );
        assert_eq!(git(path, &["rev-parse", "HEAD^{tree}"]).unwrap(), old_tree);
        assert_eq!(git(path, &["show", ":base.txt"]).unwrap(), "staged local\n");
        assert_eq!(
            std::fs::read_to_string(path.join("base.txt")).unwrap(),
            "worktree local\n"
        );
        assert!(!rebase_in_progress(&git_dir(path).unwrap()));
        assert!(git(
            path,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/gall/recovery/{}", old_head.trim())
            ]
        )
        .is_ok());
    }
    #[test]
    fn filtered_rebase_accepts_recoverable_edits_and_names_lossy_paths() {
        let temp = repository();
        let path = temp.path();
        git(
            path,
            &["config", "filter.preview.clean", "sed 's/ local-only$//'"],
        )
        .unwrap();
        git(path, &["config", "filter.preview.smudge", "cat"]).unwrap();
        std::fs::write(
            path.join(".git/info/attributes"),
            "base.txt filter=preview\n",
        )
        .unwrap();
        assert!(!guard(path, true).unwrap());
        std::fs::write(path.join("base.txt"), "recoverable edit\n").unwrap();
        assert!(guard(path, true).unwrap());
        std::fs::write(path.join("base.txt"), "base local-only\n").unwrap();
        let error = guard(path, true).unwrap_err().to_string();
        assert!(error.contains("base.txt"), "{error}");
        assert_eq!(
            std::fs::read_to_string(path.join("base.txt")).unwrap(),
            "base local-only\n"
        );
    }

    #[test]
    #[ignore = "Requires GALL_REBASE_HELPER pointing to a bundled Gall application"]
    fn interactive_rebase_applies_configured_checkout_filters() {
        let executable =
            PathBuf::from(std::env::var_os("GALL_REBASE_HELPER").expect("Set GALL_REBASE_HELPER"));
        let temp = repository();
        let path = temp.path();
        std::fs::write(
            path.join(".gitattributes"),
            "filtered.txt filter=roundtrip\n",
        )
        .unwrap();
        git(path, &["add", "--", ".gitattributes"]).unwrap();
        git(path, &["commit", "-qm", "attributes"]).unwrap();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        let first = commit(path, "first", "filtered.txt");
        let second = commit(path, "second", "other.txt");
        for (key, value) in [
            ("filter.roundtrip.clean", "sed 's/working/first/g'"),
            ("filter.roundtrip.smudge", "sed 's/first/working/g'"),
            ("filter.roundtrip.required", "true"),
        ] {
            git(path, &["config", key, value]).unwrap();
        }
        std::fs::write(path.join("filtered.txt"), "working\n").unwrap();
        let plan = plan(path, base.trim()).unwrap();
        let pick = |id: String| RewriteEntry {
            id,
            action: RewriteAction::Pick,
            message: String::new(),
        };
        execute(
            path,
            vec![pick(second), pick(first)],
            &plan.base,
            &plan.token,
            &executable,
        )
        .unwrap();
        assert_eq!(
            git(path, &["show", "HEAD:filtered.txt"]).unwrap(),
            "first\n"
        );
        assert_eq!(
            std::fs::read_to_string(path.join("filtered.txt")).unwrap(),
            "working\n"
        );
        assert!(!rebase_in_progress(&git_dir(path).unwrap()));
    }

    #[test]
    #[ignore = "Requires GALL_REBASE_HELPER pointing to a bundled Gall application"]
    fn an_interactive_rebase_keeps_a_tracked_local_change_aside_and_puts_it_back() {
        let executable =
            PathBuf::from(std::env::var_os("GALL_REBASE_HELPER").expect("Set GALL_REBASE_HELPER"));
        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        commit(path, "first", "a.txt");
        commit(path, "second", "b.txt");
        // A tracked change that no commit of the plan touches, staged in the second run.
        for staged in [false, true] {
            std::fs::write(path.join("base.txt"), "local edit\n").unwrap();
            if staged {
                git(path, &["add", "base.txt"]).unwrap();
            }
            // The rebase rewrites the commits, so their IDs are read again each time.
            let ids = git(
                path,
                &["rev-list", "--reverse", &format!("{}..HEAD", base.trim())],
            )
            .unwrap();
            let ids: Vec<&str> = ids.lines().collect();
            let (one, two) = (ids[1], ids[0]);
            let plan = super::plan(path, base.trim()).unwrap();
            execute(
                path,
                vec![
                    entry(one, RewriteAction::Pick, ""),
                    entry(two, RewriteAction::Pick, ""),
                ],
                base.trim(),
                &plan.token,
                &executable,
            )
            .unwrap_or_else(|e| panic!("staged {staged}: {e}"));
            assert_eq!(
                std::fs::read_to_string(path.join("base.txt")).unwrap(),
                "local edit\n"
            );
            assert!(git(path, &["stash", "list"]).unwrap().is_empty());
            assert!(!is_edit_stop(path) && !rebase_in_progress(&git_dir(path).unwrap()));
            git(path, &["reset", "-q", "--hard"]).unwrap();
        }
    }
    fn exec(command: &str) -> RewriteEntry {
        entry("", RewriteAction::Exec, command)
    }
    #[test]
    fn exec_lines_are_single_lines_after_a_commit_and_not_part_of_the_commit_set() {
        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap();
        let first = commit(path, "first", "a.txt");
        let second = commit(path, "second", "b.txt");
        let plan = super::plan(path, base.trim()).unwrap();
        let ok = |entries: Vec<RewriteEntry>| validate_entries(&plan, &entries);
        assert!(ok(vec![
            entry(&first, RewriteAction::Pick, ""),
            exec("cargo test"),
            entry(&second, RewriteAction::Pick, ""),
            exec("cargo clippy"),
        ])
        .is_ok());
        // A line break would add plan lines of the caller's choosing, so none is accepted.
        for bad in ["a\nb", "a\rb", "a\0b", "", "   "] {
            assert!(ok(vec![
                entry(&first, RewriteAction::Pick, ""),
                entry(&second, RewriteAction::Pick, ""),
                exec(bad)
            ])
            .is_err());
        }
        // A plan of exec lines only has no commits, and a missing commit is still refused.
        assert!(ok(vec![exec("true")]).is_err());
        assert!(ok(vec![entry(&first, RewriteAction::Pick, ""), exec("true")]).is_err());
        // The combined message of a squash is read from consecutive plan lines: an exec between them breaks the group.
        assert!(ok(vec![
            entry(&first, RewriteAction::Pick, ""),
            exec("true"),
            entry(&second, RewriteAction::Squash, ""),
        ])
        .is_err());
        assert!(ok(vec![
            entry(&first, RewriteAction::Pick, ""),
            entry(&second, RewriteAction::Squash, ""),
            exec("true"),
        ])
        .is_ok());
    }
    #[test]
    fn a_plan_from_the_root_lists_every_commit_and_predicts_with_the_empty_tree() {
        let temp = repository();
        let path = temp.path();
        let first = commit(path, "first", "a.txt");
        let second = commit(path, "second", "b.txt");
        let plan = super::plan(path, ROOT_BASE).unwrap();
        assert_eq!(plan.base, ROOT_BASE);
        let roots = git(path, &["rev-list", "--max-parents=0", "HEAD"]).unwrap();
        assert_eq!(
            plan.commits.first().map(|c| c.id.as_str()),
            Some(roots.trim())
        );
        assert!(
            plan.commits.iter().any(|c| c.id == first)
                && plan.commits.iter().any(|c| c.id == second)
        );
        let picks: Vec<_> = plan
            .commits
            .iter()
            .map(|c| entry(&c.id, RewriteAction::Pick, ""))
            .collect();
        validate_entries(&plan, &picks).unwrap();
        assert!(predict_conflicts(path, ROOT_BASE, &picks, &|| false)
            .unwrap()
            .is_empty());
        // Applying the later commits without their parents' files replays them against the empty tree and a conflict is not invented.
        let reversed: Vec<_> = picks.iter().rev().cloned().collect();
        assert!(predict_conflicts(path, ROOT_BASE, &reversed, &|| false).is_ok());
        assert_eq!(empty_tree(40), "4b825dc642cb6eb9a060e54bf8d69288fbee4904");
        assert_eq!(
            empty_tree(64),
            "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321"
        );
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
    fn amend_staged_updates_root_or_merge_with_no_checkout() {
        for amend_root in [true, false] {
            let temp = repository();
            let path = temp.path();
            let first = git(path, &["rev-parse", "HEAD"]).unwrap();
            let main = git(path, &["symbolic-ref", "--short", "HEAD"]).unwrap();
            git(path, &["switch", "-qc", "side"]).unwrap();
            commit(path, "side", "side.txt");
            git(path, &["switch", "-q", main.trim()]).unwrap();
            commit(path, "main", "main.txt");
            git(path, &["merge", "--no-ff", "-qm", "merge side", "side"]).unwrap();
            let merge = git(path, &["rev-parse", "HEAD"]).unwrap();
            let old_head = commit(path, "later", "later.txt");
            std::fs::write(path.join("amend.txt"), "staged\n").unwrap();
            git(path, &["add", "--", "amend.txt"]).unwrap();
            std::fs::write(path.join("amend.txt"), "unstaged\n").unwrap();
            let target = if amend_root {
                first.trim()
            } else {
                merge.trim()
            };
            let token = super::super::gitrepository::snapshot(path).unwrap().token;
            amend_staged(path, target, &token).unwrap();

            let amended = if amend_root { "HEAD~3" } else { "HEAD^" };
            assert_eq!(
                git(path, &["show", &format!("{amended}:amend.txt")]).unwrap(),
                "staged\n"
            );
            assert_eq!(git(path, &["show", "HEAD:amend.txt"]).unwrap(), "staged\n");
            assert_eq!(
                git(path, &["show", "-s", "--format=%P", "HEAD^"])
                    .unwrap()
                    .split_whitespace()
                    .count(),
                2
            );
            assert!(git(path, &["diff", "--cached", "--name-only"])
                .unwrap()
                .is_empty());
            assert_eq!(
                std::fs::read_to_string(path.join("amend.txt")).unwrap(),
                "unstaged\n"
            );
            let journal = super::super::gitjournal::view(path).unwrap();
            assert!(journal.entries.last().unwrap().snapshot.is_none());
            let undo =
                super::super::gitjournal::move_to(path, journal.applied - 1, journal.applied)
                    .unwrap();
            assert!(!undo.contains("could not be applied"));
            assert_eq!(git(path, &["rev-parse", "HEAD"]).unwrap().trim(), old_head);
            assert_eq!(git(path, &["show", ":amend.txt"]).unwrap(), "staged\n");
        }
    }

    #[test]
    fn conflicting_older_amend_keeps_index_and_working_file() {
        let temp = repository();
        let path = temp.path();
        let first = git(path, &["rev-parse", "HEAD"]).unwrap();
        let head = commit(path, "later", "base.txt");
        std::fs::write(path.join("base.txt"), "staged\n").unwrap();
        git(path, &["add", "--", "base.txt"]).unwrap();
        std::fs::write(path.join("base.txt"), "unstaged\n").unwrap();
        let token = super::super::gitrepository::snapshot(path).unwrap().token;
        let error = amend_staged(path, first.trim(), &token).unwrap_err();
        assert!(error.to_string().contains("base.txt"));
        assert_eq!(git(path, &["rev-parse", "HEAD"]).unwrap().trim(), head);
        assert_eq!(git(path, &["show", ":base.txt"]).unwrap(), "staged\n");
        assert_eq!(
            std::fs::read_to_string(path.join("base.txt")).unwrap(),
            "unstaged\n"
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
        assert_eq!(git(path, &["rev-parse", "HEAD"]).unwrap().trim(), next);
        assert!(path.join("next.txt").exists());
    }
    #[test]
    fn quick_fixup_and_squash_keep_the_merge_parents() {
        for (action, later) in [(QuickAction::Fixup, false), (QuickAction::Squash, true)] {
            let d = repository();
            let p = d.path();
            let main = git(p, &["symbolic-ref", "--short", "HEAD"]).unwrap();
            git(p, &["switch", "-qc", "topic"]).unwrap();
            commit(p, "topic", "topic.txt");
            git(p, &["switch", "-q", main.trim()]).unwrap();
            git(p, &["merge", "--no-ff", "-qm", "merge topic", "topic"]).unwrap();
            let parents = git(p, &["show", "-s", "--format=%P", "HEAD"]).unwrap();
            let child = commit(p, "follow-up", "follow-up.txt");
            let tree = git(p, &["rev-parse", "HEAD^{tree}"]).unwrap();
            if later {
                commit(p, "later", "later.txt");
            }
            let combined = if later { "HEAD^" } else { "HEAD" };
            let token = super::super::gitrepository::snapshot(p).unwrap().token;
            quick_edit(p, &child, action, &token, &std::env::current_exe().unwrap()).unwrap();
            assert_eq!(
                git(p, &["show", "-s", "--format=%P", combined]).unwrap(),
                parents
            );
            assert_eq!(
                git(p, &["rev-parse", &format!("{combined}^{{tree}}")]).unwrap(),
                tree
            );
            let message = git(p, &["show", "-s", "--format=%B", combined]).unwrap();
            assert_eq!(message.contains("follow-up"), action == QuickAction::Squash);
        }
    }

    #[test]
    fn quick_drop_merge_removes_its_side_and_preserves_a_later_merge() {
        let d = repository();
        let p = d.path();
        let main = git(p, &["symbolic-ref", "--short", "HEAD"]).unwrap();
        git(p, &["switch", "-qc", "drop-side"]).unwrap();
        let imported = commit(p, "imported", "imported.txt");
        git(p, &["switch", "-q", main.trim()]).unwrap();
        commit(p, "main", "main.txt");
        git(
            p,
            &["merge", "--no-ff", "-qm", "drop this merge", "drop-side"],
        )
        .unwrap();
        let dropped = git(p, &["rev-parse", "HEAD"]).unwrap();
        git(p, &["switch", "-qc", "keep-side"]).unwrap();
        commit(p, "keep side", "keep-side.txt");
        git(p, &["switch", "-q", main.trim()]).unwrap();
        commit(p, "keep main", "keep-main.txt");
        git(
            p,
            &["merge", "--no-ff", "-qm", "keep this merge", "keep-side"],
        )
        .unwrap();
        let original = git(p, &["rev-parse", "HEAD"]).unwrap();
        let token = super::super::gitrepository::snapshot(p).unwrap().token;

        quick_edit(
            p,
            dropped.trim(),
            QuickAction::Drop,
            &token,
            &std::env::current_exe().unwrap(),
        )
        .unwrap();
        assert!(!p.join("imported.txt").exists());
        assert!(p.join("keep-main.txt").exists() && p.join("keep-side.txt").exists());
        assert_eq!(
            git(p, &["rev-list", "--count", "--merges", "HEAD"])
                .unwrap()
                .trim(),
            "1"
        );
        assert!(git(p, &["merge-base", "--is-ancestor", &imported, "HEAD"]).is_err());
        assert_eq!(
            git(p, &["rev-parse", "refs/heads/drop-side"])
                .unwrap()
                .trim(),
            imported
        );
        assert_eq!(
            git(
                p,
                &[
                    "rev-parse",
                    &format!("refs/gall/recovery/{}", original.trim())
                ]
            )
            .unwrap(),
            original
        );
    }

    #[test]
    fn quick_drop_keeps_the_rewritten_detached_tip_reachable() {
        let d = repository();
        let p = d.path();
        let main = git(p, &["symbolic-ref", "--short", "HEAD"]).unwrap();
        git(p, &["switch", "-qc", "topic"]).unwrap();
        commit(p, "topic", "topic.txt");
        git(p, &["switch", "-q", main.trim()]).unwrap();
        git(p, &["merge", "--no-ff", "-qm", "drop this merge", "topic"]).unwrap();
        let merge = git(p, &["rev-parse", "HEAD"]).unwrap();
        git(p, &["switch", "-q", "--detach"]).unwrap();
        commit(p, "later", "later.txt");
        let token = super::super::gitrepository::snapshot(p).unwrap().token;

        quick_edit(
            p,
            merge.trim(),
            QuickAction::Drop,
            &token,
            &std::env::current_exe().unwrap(),
        )
        .unwrap();
        let rewritten = git(p, &["rev-parse", "HEAD"]).unwrap();
        assert!(git(p, &["symbolic-ref", "-q", "HEAD"]).is_err());
        assert_ne!(rewritten, merge);
        git(p, &["switch", "-q", main.trim()]).unwrap();
        let recovery = format!("refs/gall/recovery/{}", rewritten.trim());
        assert_eq!(
            git(p, &["rev-parse", "--verify", &recovery]).unwrap(),
            rewritten
        );
    }

    #[test]
    fn quick_drop_does_not_silently_flatten_a_merge() {
        let temp = repository();
        let path = temp.path();
        let root = git(path, &["rev-parse", "HEAD"]).unwrap();
        let main = git(path, &["symbolic-ref", "--short", "HEAD"]).unwrap();
        git(path, &["switch", "-qc", "topic"]).unwrap();
        commit(path, "topic", "topic.txt");
        git(path, &["switch", "-q", main.trim()]).unwrap();
        git(path, &["merge", "--no-ff", "-qm", "merge topic", "topic"]).unwrap();
        let merged = git(path, &["rev-parse", "HEAD"]).unwrap();
        let token = super::super::gitrepository::snapshot(path).unwrap().token;
        let error = quick_edit(
            path,
            root.trim(),
            QuickAction::Drop,
            &token,
            &std::env::current_exe().unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("merges"));
        assert_eq!(git(path, &["rev-parse", "HEAD"]).unwrap(), merged);
        assert!(path.join("base.txt").exists() && path.join("topic.txt").exists());
        assert!(!rebase_in_progress(&git_dir(path).unwrap()));
    }

    #[test]
    #[ignore = "Requires GALL_REBASE_HELPER pointing to a bundled Gall application"]
    fn quick_drop_and_move_later_can_replay_from_the_root() {
        let binary =
            PathBuf::from(std::env::var_os("GALL_REBASE_HELPER").expect("Set GALL_REBASE_HELPER"));
        for (action, subjects, keeps_base) in [
            (QuickAction::Drop, "second", false),
            (QuickAction::MoveLater, "second\nbase", true),
        ] {
            let temp = repository();
            let path = temp.path();
            let root = git(path, &["rev-parse", "HEAD"]).unwrap();
            commit(path, "second", "second.txt");
            let token = super::super::gitrepository::snapshot(path).unwrap().token;
            quick_edit(path, root.trim(), action, &token, &binary).unwrap();
            assert_eq!(
                git(path, &["log", "--reverse", "--format=%s"])
                    .unwrap()
                    .trim(),
                subjects
            );
            assert_eq!(path.join("base.txt").exists(), keeps_base);
            assert!(path.join("second.txt").exists());
            assert!(!rebase_in_progress(&git_dir(path).unwrap()));
        }
    }

    #[test]
    #[ignore = "Requires GALL_REBASE_HELPER pointing to a bundled Gall application"]
    fn quick_fixup_and_squash_can_combine_with_the_root_commit() {
        let binary =
            PathBuf::from(std::env::var_os("GALL_REBASE_HELPER").expect("Set GALL_REBASE_HELPER"));
        for action in [QuickAction::Fixup, QuickAction::Squash] {
            let temp = repository();
            let path = temp.path();
            let original = git(path, &["show", "-s", "--format=%s", "HEAD"]).unwrap();
            let next = commit(path, "second commit", "second.txt");
            let tree = git(path, &["rev-parse", "HEAD^{tree}"]).unwrap();
            let token = super::super::gitrepository::snapshot(path).unwrap().token;
            quick_edit(path, &next, action, &token, &binary).unwrap();
            assert_eq!(
                git(path, &["rev-list", "--count", "HEAD"]).unwrap().trim(),
                "1"
            );
            assert_eq!(git(path, &["rev-parse", "HEAD^{tree}"]).unwrap(), tree);
            let message = git(path, &["show", "-s", "--format=%B", "HEAD"]).unwrap();
            assert!(message.starts_with(original.trim_end()));
            assert_eq!(
                message.contains("second commit"),
                action == QuickAction::Squash
            );
            assert!(!rebase_in_progress(&git_dir(path).unwrap()));
        }
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
    fn a_plan_of_thousands_of_commits_opens_and_is_checked_for_conflicts() {
        let temp = repository();
        let path = temp.path();
        let base = git(path, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        // Made by fast-import: a process for each commit would take minutes.
        let mut stream = String::new();
        for i in 0..1_500 {
            let from = if i == 0 {
                format!("from {base}\n")
            } else {
                String::new()
            };
            stream.push_str(&format!(
                "commit refs/heads/long\ncommitter T <t@example.com> {} +0000\ndata 4\nc{i:03}\n{from}M 644 inline f{}.txt\ndata 5\n{i:04}\n\n",
                1_700_000_000 + i,
                i % 7
            ).replace("data 4\nc", if i < 1000 { "data 4\nc" } else { "data 5\nc" }));
        }
        let mut child = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["fast-import", "--quiet"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stream.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
        git(path, &["checkout", "-q", "long"]).unwrap();
        let plan = super::plan(path, &base).unwrap();
        assert_eq!(plan.commits.len(), 1_500);
        let entries: Vec<_> = plan
            .commits
            .iter()
            .map(|c| entry(&c.id, RewriteAction::Pick, ""))
            .collect();
        validate_entries(&plan, &entries).unwrap();
        // Clean where Git can do it in one process; otherwise refused instead of taking minutes.
        match predict_conflicts(path, &base, &entries, &|| false) {
            Ok(found) => assert!(found.is_empty()),
            Err(error) => assert!(error.to_string().contains("newer Git")),
        }
        assert!(predict_conflicts(path, &base, &entries, &|| true).is_err());
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
