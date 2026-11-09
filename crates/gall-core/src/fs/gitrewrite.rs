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
    Path::new(value.trim())
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
    let snapshot = super::gitrepository::snapshot(&root)?;
    if snapshot.head.is_empty() {
        return Err(fail("Create a commit before rewriting history"));
    }
    let base = commit_id(&root, base)?;
    let ids = git(
        &root,
        &[
            "rev-list",
            "--reverse",
            "--topo-order",
            "--no-merges",
            "--max-count=1001",
            &format!("{base}..{}", snapshot.head),
        ],
    )?;
    let skipped_merges = git(
        &root,
        &[
            "rev-list",
            "--count",
            "--merges",
            &format!("{base}..{}", snapshot.head),
        ],
    )?
    .trim()
    .parse()
    .unwrap_or(0);
    if ids.lines().count() > 1000 {
        return Err(fail("Select a range of at most 1000 commits"));
    }
    let mut commits = Vec::new();
    for id in ids.lines() {
        let raw = git(&root, &["show", "-s", "--format=%s%x00%B", id])?;
        let (subject, message) = raw
            .split_once('\0')
            .ok_or_else(|| fail("Invalid commit metadata"))?;
        commits.push(RewriteCommit {
            id: id.into(),
            subject: subject.into(),
            message: message.trim_end_matches('\n').into(),
        });
    }
    Ok(RewritePlan {
        head: snapshot.head.clone(),
        base: base.clone(),
        token: plan_token(&snapshot, &base),
        skipped_merges,
        commits,
    })
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
    if path.join(".gitmodules").exists()
        || git(path, &["ls-files", "--stage"])?
            .lines()
            .any(|line| line.starts_with("160000 "))
    {
        return Err(fail(
            "Interactive rebase with submodules requires an external Git client",
        ));
    }
    if !git(path, &["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
        return Err(fail(
            "Commit or stash local changes before rewriting history",
        ));
    }
    Ok(())
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
            .join(git(path, &["rev-parse", "--git-path", "rebase-merge"])?.trim())
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
    let mut cmd = Command::new("git");
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
    output
}
/// Reword a single commit directly. The current HEAD can be reworded without consuming the index.
pub fn reword(
    path: &Path,
    target: &str,
    message: &str,
    expected: &str,
    editor_executable: &Path,
) -> Result<String> {
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
    let Ok(directory) = git_dir(path) else {
        return false;
    };
    let state = directory.join("rebase-merge");
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
        assert_eq!(
            git(
                path,
                &[
                    "rev-parse",
                    &format!("refs/gall/recovery/{}", original.trim())
                ]
            )
            .unwrap(),
            original
        );
        let default_history = super::super::gitgraph::history_graph(path, false, false)
            .unwrap()
            .history;
        assert!(!default_history.contains(original.trim()));
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
        assert_eq!(
            git(path, &["rev-parse", &format!("refs/gall/recovery/{third}")])
                .unwrap()
                .trim(),
            third
        );
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
            assert_eq!(
                git(path, &["rev-parse", &format!("refs/gall/recovery/{third}")])
                    .unwrap()
                    .trim(),
                third
            );
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
