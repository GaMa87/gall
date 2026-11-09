//! Explicit advanced Git workflows. Reads never change HEAD; writes are reviewed,
//! stale-checked and serialized by the application adapter.
use super::gitgraph::{git, has_content_filters, root};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub head: String,
    pub branch: Option<String>,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Submodule {
    pub name: String,
    pub path: String,
    pub url: String,
    pub id: String,
    pub state: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bisect {
    pub current: String,
    pub candidate: String,
    pub checkout_pending: bool,
    pub log: String,
    pub remaining: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedSnapshot {
    pub repository: Option<String>,
    pub token: String,
    pub worktrees: Vec<Worktree>,
    pub submodules: Vec<Submodule>,
    pub bisect: Option<Bisect>,
    pub notes: String,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Request {
    CreateWorktree {
        destination: String,
        target: String,
        branch: Option<String>,
    },
    RemoveWorktree {
        destination: String,
    },
    LockWorktree {
        destination: String,
    },
    UnlockWorktree {
        destination: String,
    },
    InitSubmodule {
        file: String,
    },
    UpdateSubmodule {
        file: String,
    },
    BisectStart {
        good: String,
        bad: String,
    },
    BisectGood,
    BisectBad,
    BisectSkip,
    BisectReset,
    BisectCheckout,
    WriteNote {
        commit: String,
        message: String,
    },
    ExportPatch {
        commit: String,
        destination: String,
    },
    ApplyPatch {
        file: String,
    },
    Init {
        branch: String,
    },
    Clone {
        url: String,
        destination: String,
    },
}

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

fn concrete_commit(path: &Path, value: &str) -> Result<String> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(fail("Select a full commit ID before this operation"));
    }
    let value = value.to_ascii_lowercase();
    let resolved = git(
        path,
        &["rev-parse", "--verify", &format!("{value}^{{commit}}")],
    )?;
    if resolved.trim() != value {
        return Err(fail("The selected object is not a commit"));
    }
    Ok(value)
}

fn branch_name(path: &Path, name: &str) -> Result<()> {
    if name.starts_with('-') || name.chars().any(char::is_control) {
        return Err(fail("Enter a valid branch name"));
    }
    if git(path, &["check-ref-format", "--branch", name])?.trim() != name {
        return Err(fail(
            "Enter a literal branch name instead of a previous-checkout expression",
        ));
    }
    Ok(())
}

fn git_path(path: &Path, name: &str) -> Result<PathBuf> {
    let value = git(path, &["rev-parse", "--git-path", name])?;
    Ok(path.join(value.trim()))
}

fn read_marker(path: &Path, name: &str) -> Result<Option<String>> {
    let location = git_path(path, name)?;
    match location.symlink_metadata() {
        Ok(meta) => {
            if !meta.is_file() || meta.is_symlink() || meta.len() > 128 * 1024 {
                return Err(fail("Unsupported Git operation metadata"));
            }
            std::fs::read_to_string(location)
                .map(Some)
                .map_err(|e| fail(e.to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(fail(e.to_string())),
    }
}

fn worktrees(path: &Path) -> Result<Vec<Worktree>> {
    let output = git(path, &["worktree", "list", "--porcelain", "-z"])?;
    if output.contains('\u{fffd}') {
        return Err(fail("Worktree paths must contain valid UTF-8"));
    }
    let mut result: Vec<Worktree> = Vec::new();
    for field in output.split('\0') {
        if let Some(value) = field.strip_prefix("worktree ") {
            result.push(Worktree {
                // Git prints `C:/dir` on Windows; the rest of the app uses `C:\dir`.
                path: if cfg!(windows) {
                    value.replace('/', "\\")
                } else {
                    value.into()
                },
                head: String::new(),
                branch: None,
                bare: false,
                locked: false,
                prunable: false,
            });
        } else if let Some(current) = result.last_mut() {
            if let Some(value) = field.strip_prefix("HEAD ") {
                current.head = value.into();
            } else if let Some(value) = field.strip_prefix("branch ") {
                current.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).into());
            } else if field == "bare" {
                current.bare = true;
            } else if field == "locked" || field.starts_with("locked ") {
                current.locked = true;
            } else if field == "prunable" || field.starts_with("prunable ") {
                current.prunable = true;
            }
        }
    }
    Ok(result)
}

fn relative_path(value: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains('\0')
        || value.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.eq_ignore_ascii_case(".git")
        })
        || (cfg!(windows) && (value.contains('\\') || value.contains(':')))
    {
        return Err(fail("A repository-relative submodule path is required"));
    }
    Ok(())
}

fn no_symlink_parents(path: &Path, relative: &str) -> Result<()> {
    relative_path(relative)?;
    let mut current = path.to_owned();
    for part in relative.split('/') {
        current.push(part);
        match current.symlink_metadata() {
            Ok(meta) if meta.is_symlink() => {
                return Err(fail("Submodule paths containing symlinks are unsupported"))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(fail(e.to_string())),
        }
    }
    Ok(())
}

fn initialized_submodule(path: &Path, file: &str) -> Result<()> {
    let module = path.join(file);
    let marker = module
        .join(".git")
        .symlink_metadata()
        .map_err(|e| fail(e.to_string()))?;
    if marker.is_symlink() {
        return Err(fail("Symlink submodule Git directories are unsupported"));
    }
    let actual = root(&module)?;
    if actual.canonicalize().map_err(|e| fail(e.to_string()))?
        != module.canonicalize().map_err(|e| fail(e.to_string()))?
    {
        return Err(fail(
            "The submodule Git directory points outside its working folder",
        ));
    }
    Ok(())
}

fn submodules(path: &Path) -> Result<Vec<Submodule>> {
    match path.join(".gitmodules").symlink_metadata() {
        Ok(meta) if meta.is_file() && !meta.is_symlink() && meta.len() <= 1024 * 1024 => {}
        Ok(_) => {
            return Err(fail(
                "The .gitmodules file must be a regular file up to 1 MiB",
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(fail(e.to_string())),
    }
    git(
        path,
        &[
            "config",
            "--no-includes",
            "--null",
            "--file",
            ".gitmodules",
            "--list",
        ],
    )?;
    let output = git(
        path,
        &[
            "config",
            "--no-includes",
            "--null",
            "--file",
            ".gitmodules",
            "--get-regexp",
            "^submodule\\..*\\.path$",
        ],
    )
    .unwrap_or_default();
    if output.contains('\u{fffd}') {
        return Err(fail("Submodule paths must contain valid UTF-8"));
    }
    let mut result = Vec::new();
    for field in output.split('\0').filter(|v| !v.is_empty()) {
        let (key, file) = field
            .split_once('\n')
            .ok_or_else(|| fail("Invalid .gitmodules path record"))?;
        let name = key
            .strip_prefix("submodule.")
            .and_then(|v| v.strip_suffix(".path"))
            .ok_or_else(|| fail("Invalid submodule name"))?;
        if name.chars().any(char::is_control) {
            return Err(fail("Unsupported submodule name"));
        }
        let url = git(
            path,
            &[
                "config",
                "--no-includes",
                "--file",
                ".gitmodules",
                "--get",
                &format!("submodule.{name}.url"),
            ],
        )
        .unwrap_or_default()
        .trim_end()
        .to_owned();
        let literal = format!(":(literal){file}");
        let index = git(path, &["ls-files", "--stage", "-z", "--", &literal])?;
        let id = index
            .split('\t')
            .next()
            .and_then(|v| v.strip_prefix("160000 "))
            .and_then(|v| v.split_whitespace().next())
            .unwrap_or_default()
            .to_owned();
        let state = if no_symlink_parents(path, file).is_err() {
            "unsupportedPath".to_owned()
        } else if !path.join(file).join(".git").exists() {
            "notInitialized".into()
        } else if initialized_submodule(path, file).is_err() {
            "unsupportedPath".into()
        } else {
            let actual =
                git(&path.join(file), &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
            if actual.trim() == id {
                "current".into()
            } else {
                "differentCommit".into()
            }
        };
        result.push(Submodule {
            name: name.into(),
            path: file.into(),
            url,
            id,
            state,
        });
    }
    Ok(result)
}

fn bisect(path: &Path) -> Result<Option<Bisect>> {
    if read_marker(path, "BISECT_START")?.is_none() {
        return Ok(None);
    }
    let log = read_marker(path, "BISECT_LOG")?.unwrap_or_default();
    let remaining = read_marker(path, "BISECT_EXPECTED_REV")?.unwrap_or_default();
    let current = git(path, &["rev-parse", "HEAD"])?.trim().to_owned();
    let candidate = git(path, &["rev-parse", "--verify", "BISECT_HEAD"])
        .unwrap_or_else(|_| current.clone())
        .trim()
        .to_owned();
    let checkout_pending = current != candidate;
    Ok(Some(Bisect {
        current,
        candidate,
        checkout_pending,
        log,
        remaining: remaining.trim().into(),
    }))
}

fn nonrepo_token(path: &Path) -> Result<String> {
    let canonical = path.canonicalize().map_err(|e| fail(e.to_string()))?;
    if !canonical.is_dir() {
        return Err(fail("Select an existing folder"));
    }
    let mut entries = std::fs::read_dir(&canonical)
        .map_err(|e| fail(e.to_string()))?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|e| fail(e.to_string()))?;
    entries.sort();
    let mut hasher = DefaultHasher::new();
    canonical.hash(&mut hasher);
    entries.hash(&mut hasher);
    Ok(format!("folder:{:016x}", hasher.finish()))
}

pub fn snapshot(path: &Path, commit: Option<&str>) -> Result<AdvancedSnapshot> {
    let repository = match root(path) {
        Ok(repository) => repository,
        Err(_) => {
            return Ok(AdvancedSnapshot {
                repository: None,
                token: nonrepo_token(path)?,
                worktrees: Vec::new(),
                submodules: Vec::new(),
                bisect: None,
                notes: String::new(),
            })
        }
    };
    let state = super::gitrepository::snapshot(&repository)?;
    let worktrees = worktrees(&repository)?;
    let submodules = submodules(&repository)?;
    let bisect = bisect(&repository)?;
    let notes = match commit {
        Some(commit) => {
            let commit = concrete_commit(&repository, commit)?;
            git(
                &repository,
                &["notes", "--ref=refs/notes/commits", "show", &commit],
            )
            .unwrap_or_default()
        }
        None => String::new(),
    };
    let after = super::gitrepository::snapshot(&repository)?;
    if state.token != after.token {
        return Err(fail(
            "Repository changed while loading advanced Git details. Refresh to review again",
        ));
    }
    let mut hasher = DefaultHasher::new();
    state.token.hash(&mut hasher);
    serde_json::to_string(&worktrees)
        .map_err(|e| fail(e.to_string()))?
        .hash(&mut hasher);
    serde_json::to_string(&submodules)
        .map_err(|e| fail(e.to_string()))?
        .hash(&mut hasher);
    serde_json::to_string(&bisect)
        .map_err(|e| fail(e.to_string()))?
        .hash(&mut hasher);
    Ok(AdvancedSnapshot {
        repository: Some(repository.to_string_lossy().into_owned()),
        token: format!("advanced:{:016x}", hasher.finish()),
        worktrees,
        submodules,
        bisect,
        notes,
    })
}

fn stop(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x08000000)
            .output();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run(
    path: &Path,
    args: &[&str],
    cancel: &AtomicBool,
    progress: &impl Fn(String),
    max_output: usize,
) -> Result<Vec<u8>> {
    if cancel.load(Ordering::Relaxed) {
        return Err(fail("Git operation cancelled before starting"));
    }
    let mut command = Command::new("git");
    command
        .args([
            "--no-pager",
            "-c",
            "core.fsmonitor=false",
            "-c",
            if cfg!(windows) {
                "core.hooksPath=NUL"
            } else {
                "core.hooksPath=/dev/null"
            },
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "log.showSignature=false",
            "-c",
            "submodule.recurse=false",
            "-C",
        ])
        .arg(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", if cfg!(windows) { "" } else { "/bin/false" })
        .env(
            "GIT_ALLOW_PROTOCOL",
            if args.contains(&"clone")
                || git(path, &["config", "--get", "protocol.file.allow"])
                    .is_ok_and(|v| v.trim() == "always")
            {
                "https:ssh:git:file"
            } else {
                "https:ssh:git"
            },
        );
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=15");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|e| fail(e.to_string()))?;
    let (tx, rx) = mpsc::sync_channel(32);
    for (is_error, mut pipe) in [
        (
            false,
            Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
        ),
        (
            true,
            Box::new(child.stderr.take().unwrap()) as Box<dyn Read + Send>,
        ),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(count) = pipe.read(&mut bytes) {
                if count == 0 || tx.send((is_error, bytes[..count].to_vec())).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);
    // Inactivity deadline: any output (including `--progress` lines) proves the
    // operation is still advancing, so long transfers are not cut off.
    let mut last_activity = Instant::now();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    loop {
        for _ in 0..128 {
            let Ok((is_error, chunk)) = rx.try_recv() else {
                break;
            };
            last_activity = Instant::now();
            if is_error {
                progress(String::from_utf8_lossy(&chunk).into_owned());
                stderr.extend(chunk);
                if stderr.len() > 128 * 1024 {
                    stderr.drain(..stderr.len() - 64 * 1024);
                }
            } else {
                stdout.extend(chunk);
                if stdout.len() > max_output {
                    stop(&mut child);
                    return Err(fail("Git output exceeds this operation's size limit"));
                }
            }
        }
        if cancel.load(Ordering::Relaxed) || last_activity.elapsed() > Duration::from_secs(180) {
            stop(&mut child);
            return Err(fail("Git operation stopped (cancelled or no output for 3 minutes). A partially created worktree remains available for inspection; refresh before continuing"));
        }
        if let Some(status) = child.try_wait().map_err(|e| fail(e.to_string()))? {
            let drain_start = Instant::now();
            loop {
                if drain_start.elapsed() > Duration::from_secs(2) {
                    stop(&mut child);
                    return Err(fail("Git output did not finish after the process exited; refresh before retrying"));
                }
                match rx.recv_timeout(Duration::from_millis(10)) {
                    Ok((is_error, chunk)) if is_error => {
                        progress(String::from_utf8_lossy(&chunk).into_owned());
                        stderr.extend(chunk);
                        if stderr.len() > 128 * 1024 {
                            stderr.drain(..stderr.len() - 64 * 1024);
                        }
                    }
                    Ok((_, chunk)) => {
                        stdout.extend(chunk);
                        if stdout.len() > max_output {
                            return Err(fail("Git output exceeds this operation's size limit"));
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(_) => {}
                }
            }
            return if status.success() {
                Ok(stdout)
            } else {
                let output = format!(
                    "{}{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                );
                Err(fail(if output.trim().is_empty() {
                    "Git operation failed"
                } else {
                    output.trim()
                }))
            };
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn new_destination(value: &str) -> Result<PathBuf> {
    let destination = PathBuf::from(value);
    if !destination.is_absolute() {
        return Err(fail("Choose an absolute destination path"));
    }
    let name = destination
        .file_name()
        .ok_or_else(|| fail("Choose a new destination folder or file name"))?;
    let parent = destination
        .parent()
        .ok_or_else(|| fail("Destination parent is missing"))?;
    // Not `\\?\C:\…` on Windows: Git mixes in `/` and cannot open such a path.
    let parent = crate::nav::path::canonicalize(parent)
        .map_err(|e| fail(format!("Destination parent: {e}")))?;
    if !parent.is_dir() {
        return Err(fail("Destination parent must be a folder"));
    }
    let destination = parent.join(name);
    if destination.symlink_metadata().is_ok() {
        return Err(fail("Destination already exists; choose a new path"));
    }
    Ok(destination)
}

fn clean_worktree(path: &Path, allow_submodules: bool) -> Result<()> {
    let state = super::gitrepository::snapshot(path)?;
    if state
        .operation
        .as_deref()
        .is_some_and(|value| value != "bisect")
        || !state.conflicts.is_empty()
    {
        return Err(fail("Finish or abort the current Git operation first"));
    }
    if has_content_filters(path) {
        return Err(fail(
            "This worktree operation with content filters requires the terminal",
        ));
    }
    if !allow_submodules
        && (path.join(".gitmodules").exists()
            || git(path, &["ls-files", "--stage"])?
                .lines()
                .any(|line| line.starts_with("160000 ")))
    {
        return Err(fail("This operation with submodules requires the terminal"));
    }
    if !git(
        path,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
    )?
    .is_empty()
    {
        return Err(fail("Commit or stash local changes before this operation"));
    }
    Ok(())
}

/// Native worktree removal and submodule checkout can discard ignored content
/// even when ordinary status is clean. Refuse all untracked content, regardless
/// of ignore rules, before delegating either destructive filesystem operation.
fn no_untracked_content(path: &Path, operation: &str) -> Result<()> {
    if !git(path, &["ls-files", "--others", "--directory", "-z"])?.is_empty() {
        return Err(fail(format!(
            "Untracked or ignored files are present. Move them outside this worktree before {operation}"
        )));
    }
    Ok(())
}

fn empty_submodule_destination(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(meta) if !meta.is_dir() || meta.is_symlink() => Err(fail(
            "The uninitialized submodule destination must be an empty folder",
        )),
        Ok(_) => {
            if std::fs::read_dir(path)
                .map_err(|e| fail(e.to_string()))?
                .next()
                .transpose()
                .map_err(|e| fail(e.to_string()))?
                .is_some()
            {
                return Err(fail("The uninitialized submodule destination contains files. Move them outside the folder before initializing it"));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(fail(e.to_string())),
    }
}

fn recover_head(path: &Path) -> Result<()> {
    let head = git(path, &["rev-parse", "--verify", "HEAD"])?;
    let head = head.trim();
    git(
        path,
        &[
            "update-ref",
            "--no-deref",
            &format!("refs/gall/recovery/{head}"),
            head,
        ],
    )?;
    Ok(())
}

fn checkout_bisect(path: &Path, cancel: &AtomicBool, progress: &impl Fn(String)) -> Result<String> {
    let value = git(path, &["rev-parse", "--verify", "BISECT_HEAD"])?;
    let target = concrete_commit(path, value.trim())?;
    let output = run(
        path,
        &["switch", "--detach", "--no-overwrite-ignore", &target],
        cancel,
        progress,
        1024 * 1024,
    )?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn checked_url(url: &str) -> Result<()> {
    if url.is_empty()
        || url.starts_with('-')
        || url.chars().any(char::is_control)
        || (url.contains("::") && !url.contains("://"))
    {
        return Err(fail(
            "Enter a repository URL or an absolute local repository path",
        ));
    }
    if url.contains("://")
        && !["https://", "ssh://", "git://", "file://"]
            .iter()
            .any(|prefix| url.starts_with(prefix))
    {
        return Err(fail(
            "Only HTTPS, SSH, Git and local repository URLs are supported",
        ));
    }
    Ok(())
}

/// All callers must serialize writes and refresh the snapshot after success or failure.
pub fn execute(
    path: &Path,
    request: Request,
    expected: &str,
    cancel: &AtomicBool,
    progress: impl Fn(String),
) -> Result<String> {
    if cancel.load(Ordering::Relaxed) {
        return Err(fail("Git operation cancelled before starting"));
    }
    let state = snapshot(path, None)?;
    if state.token != expected {
        return Err(fail(
            "Repository changed. Refresh and review this advanced operation again",
        ));
    }
    let current = state.repository.as_deref().map(Path::new);
    let p = current.unwrap_or(path);
    let worktree_action = match &request {
        Request::RemoveWorktree { .. } => "remove",
        Request::LockWorktree { .. } => "lock",
        _ => "unlock",
    };
    let invoke = |args: &[&str]| -> Result<String> {
        let bytes = run(p, args, cancel, &progress, 1024 * 1024)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    match request {
        Request::Init { branch } => {
            if current.is_some() {
                return Err(fail("This folder already belongs to a Git repository"));
            }
            branch_name(p, &branch)?;
            invoke(&["init", "--initial-branch", &branch])
        }
        Request::Clone { url, destination } => {
            checked_url(&url)?;
            let destination = new_destination(&destination)?;
            std::fs::create_dir(&destination)
                .map_err(|e| fail(format!("Clone destination: {e}")))?;
            // Cloning without checkout avoids running content filters before their
            // configuration can be reviewed. Plain repositories are checked out next.
            let Some(destination_text) = destination.to_str() else {
                let _ = std::fs::remove_dir(&destination);
                return Err(fail("Destination must be UTF-8"));
            };
            let output = match invoke(&[
                "clone",
                "--no-checkout",
                "--progress",
                "--",
                &url,
                destination_text,
            ]) {
                Ok(output) => output,
                Err(error) => {
                    // The directory was created above and was empty before the clone,
                    // so a failed or cancelled clone must not block retrying the path.
                    let _ = std::fs::remove_dir_all(&destination);
                    return Err(error);
                }
            };
            if has_content_filters(&destination) {
                return Ok(format!("{output}\nRepository cloned without checking out files because content filters are configured. Review it in the terminal: {}", destination.display()));
            }
            if git(&destination, &["rev-parse", "--verify", "HEAD"]).is_ok() {
                run(
                    &destination,
                    &["read-tree", "HEAD"],
                    cancel,
                    &progress,
                    1024 * 1024,
                )?;
                // Without --force, checkout-index refuses any file another program
                // created while the clone ran and preserves safe partial progress.
                run(
                    &destination,
                    &["checkout-index", "--all"],
                    cancel,
                    &progress,
                    1024 * 1024,
                )?;
            }
            Ok(format!("{output}\nCloned to {}", destination.display()))
        }
        _ if current.is_none() => Err(fail("Initialize or clone a Git repository first")),
        Request::CreateWorktree {
            destination,
            target,
            branch,
        } => {
            if state.bisect.is_some() {
                return Err(fail("Reset the active bisect before creating a worktree"));
            }
            if has_content_filters(p) {
                return Err(fail(
                    "Creating worktrees with content filters requires the terminal",
                ));
            }
            let target = concrete_commit(p, &target)?;
            let destination = new_destination(&destination)?;
            let destination = destination
                .to_str()
                .ok_or_else(|| fail("Destination must be UTF-8"))?;
            let mut args = vec!["worktree", "add"];
            if let Some(name) = &branch {
                branch_name(p, name)?;
                args.extend(["-b", name]);
            } else {
                args.push("--detach");
            }
            args.extend(["--", destination, &target]);
            invoke(&args)
        }
        Request::RemoveWorktree { destination }
        | Request::LockWorktree { destination }
        | Request::UnlockWorktree { destination } => {
            let selected = state
                .worktrees
                .iter()
                .find(|entry| entry.path == destination)
                .ok_or_else(|| fail("Select an existing worktree"))?;
            let selected_path = Path::new(&selected.path);
            if selected.bare || selected_path.canonicalize().ok() == p.canonicalize().ok() {
                return Err(fail(
                    "The current or main worktree cannot be removed or locked here",
                ));
            }
            if worktree_action == "remove" {
                no_untracked_content(selected_path, "removing it")?;
                clean_worktree(selected_path, true)?;
            }
            invoke(&["worktree", worktree_action, "--", &selected.path])
        }
        Request::InitSubmodule { file } | Request::UpdateSubmodule { file } => {
            let module = state
                .submodules
                .iter()
                .find(|entry| entry.path == file)
                .ok_or_else(|| fail("Select an existing submodule"))?;
            no_symlink_parents(p, &module.path)?;
            if state.bisect.is_some() {
                return Err(fail("Reset the active bisect before updating submodules"));
            }
            clean_worktree(p, true)?;
            if p.join(&module.path).join(".git").exists() {
                initialized_submodule(p, &module.path)?;
                no_untracked_content(&p.join(&module.path), "updating this submodule")?;
                clean_worktree(&p.join(&module.path), false)?;
            } else {
                empty_submodule_destination(&p.join(&module.path))?;
            }
            checked_url(&module.url)?;
            // An explicit checkout strategy cannot execute a configured custom
            // submodule update command. Never force or recurse into nested modules.
            invoke(&[
                "submodule",
                "update",
                "--init",
                "--checkout",
                "--",
                &module.path,
            ])
        }
        Request::BisectStart { good, bad } => {
            if state.bisect.is_some() {
                return Err(fail("A bisect is already active"));
            }
            clean_worktree(p, false)?;
            let good = concrete_commit(p, &good)?;
            let bad = concrete_commit(p, &bad)?;
            if good == bad {
                return Err(fail("Good and bad commits must differ"));
            }
            git(p, &["merge-base", "--is-ancestor", &good, &bad])
                .map_err(|_| fail("The good commit must be an ancestor of the bad commit"))?;
            recover_head(p)?;
            // Separate candidate selection from checkout so ignored collisions
            // abort the move instead of being overwritten by Git's bisect helper.
            let output = invoke(&["bisect", "start", "--no-checkout", &bad, &good])?;
            checkout_bisect(p, cancel, &progress)?;
            Ok(output)
        }
        Request::BisectGood
        | Request::BisectBad
        | Request::BisectSkip
        | Request::BisectReset
        | Request::BisectCheckout => {
            if state.bisect.is_none() {
                return Err(fail("There is no active bisect"));
            }
            clean_worktree(p, false)?;
            if matches!(request, Request::BisectReset) {
                let start = read_marker(p, "BISECT_START")?
                    .ok_or_else(|| fail("Bisect starting revision is missing"))?;
                let start = start.trim();
                if git(p, &["show-ref", "--verify", &format!("refs/heads/{start}")]).is_ok() {
                    branch_name(p, start)?;
                    invoke(&["switch", "--no-overwrite-ignore", "--", start])?;
                } else {
                    let target = concrete_commit(p, start)?;
                    invoke(&["switch", "--detach", "--no-overwrite-ignore", &target])?;
                }
                return invoke(&["bisect", "reset", "HEAD"]);
            }
            if matches!(request, Request::BisectCheckout) {
                return checkout_bisect(p, cancel, &progress);
            }
            if git(p, &["rev-parse", "--verify", "BISECT_HEAD"]).is_err() {
                return Err(fail("This bisect was started in another client. Continue it there or use Finish / reset"));
            }
            if state
                .bisect
                .as_ref()
                .is_some_and(|value| value.checkout_pending)
            {
                return Err(fail(
                    "Check out the pending test revision before marking its result",
                ));
            }
            let action = match request {
                Request::BisectGood => "good",
                Request::BisectBad => "bad",
                Request::BisectSkip => "skip",
                _ => "reset",
            };
            let output = invoke(&["bisect", action])?;
            checkout_bisect(p, cancel, &progress)?;
            Ok(output)
        }
        Request::WriteNote { commit, message } => {
            let commit = concrete_commit(p, &commit)?;
            if git(p, &["symbolic-ref", "-q", "refs/notes/commits"]).is_ok() {
                return Err(fail("The notes reference is symbolic. Restore a direct refs/notes/commits reference in the terminal before editing notes"));
            }
            if message.len() > 128 * 1024 {
                return Err(fail("A commit note is limited to 128 KiB"));
            }
            if let Ok(old) = git(p, &["rev-parse", "--verify", "refs/notes/commits"]) {
                let old = old.trim();
                git(
                    p,
                    &[
                        "update-ref",
                        "--no-deref",
                        &format!("refs/gall/recovery/notes-{old}"),
                        old,
                    ],
                )?;
            }
            if message.trim().is_empty() {
                invoke(&[
                    "notes",
                    "--ref=refs/notes/commits",
                    "remove",
                    "--ignore-missing",
                    &commit,
                ])
            } else {
                invoke(&[
                    "notes",
                    "--ref=refs/notes/commits",
                    "add",
                    "--force",
                    "-m",
                    &message,
                    &commit,
                ])
            }
        }
        Request::ExportPatch {
            commit,
            destination,
        } => {
            let commit = concrete_commit(p, &commit)?;
            let destination = new_destination(&destination)?;
            let bytes = run(
                p,
                &[
                    "format-patch",
                    "-1",
                    "--stdout",
                    "--no-signature",
                    "--no-stat",
                    "--binary",
                    "--full-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    &commit,
                ],
                cancel,
                &progress,
                16 * 1024 * 1024,
            )?;
            if bytes.is_empty() {
                return Err(fail(
                    "This commit has no single-parent patch to export; select a non-merge commit",
                ));
            }
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .map_err(|e| fail(format!("Patch destination: {e}")))?;
            output
                .write_all(&bytes)
                .and_then(|_| output.sync_all())
                .map_err(|e| {
                    fail(format!(
                        "Patch export failed; a partial file remains at {}: {e}",
                        destination.display()
                    ))
                })?;
            Ok(format!("Exported patch to {}", destination.display()))
        }
        Request::ApplyPatch { file } => {
            if state.bisect.is_some() {
                return Err(fail("Reset the active bisect before applying a patch"));
            }
            clean_worktree(p, false)?;
            let file = PathBuf::from(file);
            let metadata = file.symlink_metadata().map_err(|e| fail(e.to_string()))?;
            if !file.is_absolute()
                || metadata.is_symlink()
                || !metadata.is_file()
                || metadata.len() > 16 * 1024 * 1024
            {
                return Err(fail(
                    "Choose a regular patch file up to 16 MiB using an absolute path",
                ));
            }
            let mut bytes = Vec::new();
            super::open_regular(&file)
                .map_err(|e| fail(e.to_string()))?
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| fail(e.to_string()))?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(fail("Patch file exceeds the 16 MiB limit"));
            }
            // Keep the reviewed bytes immutable between --check and --index. A file
            // supplied by another application may be rewritten while Git runs.
            let scratch = git_path(
                p,
                &format!(
                    "gall-patch-{}-{}.patch",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| fail(e.to_string()))?
                        .as_nanos()
                ),
            )?;
            let mut temporary = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&scratch)
                .map_err(|e| fail(e.to_string()))?;
            temporary
                .write_all(&bytes)
                .map_err(|e| fail(e.to_string()))?;
            drop(temporary);
            let name = scratch
                .to_str()
                .ok_or_else(|| fail("Git directory must be UTF-8"))?;
            let result = (|| {
                invoke(&["apply", "--check", "--index", "--", name])?;
                if snapshot(p, None)?.token != expected {
                    return Err(fail(
                        "Repository changed while checking the patch. Review again",
                    ));
                }
                recover_head(p)?;
                invoke(&["apply", "--index", "--", name])
            })();
            let _ = std::fs::remove_file(scratch);
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn init(path: &Path) {
        git(path, &["init", "-q", "-b", "main"]).unwrap();
        git(path, &["config", "user.name", "Test User"]).unwrap();
        git(path, &["config", "user.email", "test@example.com"]).unwrap();
        git(path, &["config", "core.autocrlf", "false"]).unwrap();
        // An empty file rather than the null device: Git on Windows rejects `NUL` here.
        let excludes = path.join(".git").join("empty-excludes");
        fs::write(&excludes, "").unwrap();
        git(
            path,
            &[
                "config",
                "core.excludesFile",
                &excludes.to_string_lossy().replace('\\', "/"),
            ],
        )
        .unwrap();
    }

    /// A temporary directory path as Git reports it: macOS temp folders sit behind the
    /// `/var` → `/private/var` link and Windows ones may use 8.3 short names.
    fn reported(dir: &Path) -> PathBuf {
        crate::nav::path::canonicalize(dir).unwrap()
    }

    fn commit(path: &Path, message: &str) -> String {
        git(path, &["add", "--all"]).unwrap();
        git(path, &["commit", "-qm", message]).unwrap();
        git(path, &["rev-parse", "HEAD"]).unwrap().trim().into()
    }

    fn act(path: &Path, request: Request) -> Result<String> {
        execute(
            path,
            request,
            &snapshot(path, None)?.token,
            &AtomicBool::new(false),
            |_| {},
        )
    }

    #[test]
    fn worktree_create_lock_dirty_removal_and_normal_removal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        fs::write(root.join("file.txt"), "first\n").unwrap();
        let first = commit(root, "First");
        let destinations = tempfile::tempdir().unwrap();
        let destination = reported(destinations.path()).join("worktree with spaces");
        let destination = destination.to_str().unwrap().to_owned();
        act(
            root,
            Request::CreateWorktree {
                destination: destination.clone(),
                target: first.clone(),
                branch: Some("topic".into()),
            },
        )
        .unwrap();
        let trees = snapshot(root, None).unwrap().worktrees;
        assert_eq!(trees.len(), 2);
        assert!(trees
            .iter()
            .any(|tree| tree.path == destination && tree.branch.as_deref() == Some("topic")));
        assert_eq!(
            fs::read_to_string(Path::new(&destination).join("file.txt")).unwrap(),
            "first\n"
        );
        act(
            root,
            Request::LockWorktree {
                destination: destination.clone(),
            },
        )
        .unwrap();
        assert!(act(
            root,
            Request::RemoveWorktree {
                destination: destination.clone()
            }
        )
        .is_err());
        act(
            root,
            Request::UnlockWorktree {
                destination: destination.clone(),
            },
        )
        .unwrap();
        fs::write(Path::new(&destination).join("file.txt"), "local\n").unwrap();
        assert!(act(
            root,
            Request::RemoveWorktree {
                destination: destination.clone()
            }
        )
        .is_err());
        assert_eq!(
            fs::read_to_string(Path::new(&destination).join("file.txt")).unwrap(),
            "local\n"
        );
        fs::write(Path::new(&destination).join("file.txt"), "first\n").unwrap();
        act(
            root,
            Request::RemoveWorktree {
                destination: destination.clone(),
            },
        )
        .unwrap();
        assert!(!Path::new(&destination).exists());
        assert_eq!(
            git(root, &["rev-parse", "refs/heads/topic"])
                .unwrap()
                .trim(),
            first
        );
        assert_eq!(snapshot(root, None).unwrap().worktrees.len(), 1);
        assert!(act(
            root,
            Request::CreateWorktree {
                destination: root.to_str().unwrap().into(),
                target: first,
                branch: Some("--bad".into())
            }
        )
        .is_err());
    }

    #[test]
    fn bisect_finds_bad_commit_and_returns_to_start() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        let mut commits = Vec::new();
        for index in 0..6 {
            fs::write(root.join("file.txt"), format!("version {index}\n")).unwrap();
            commits.push(commit(root, &format!("Version {index}")));
        }
        act(
            root,
            Request::BisectStart {
                good: commits[0].clone(),
                bad: commits[5].clone(),
            },
        )
        .unwrap();
        for _ in 0..8 {
            let state = snapshot(root, None).unwrap().bisect.unwrap();
            if state.log.contains("first 'bad' commit") {
                break;
            }
            let position = commits.iter().position(|id| *id == state.current).unwrap();
            act(
                root,
                if position >= 3 {
                    Request::BisectBad
                } else {
                    Request::BisectGood
                },
            )
            .unwrap();
        }
        let state = snapshot(root, None).unwrap().bisect.unwrap();
        assert!(
            state.log.contains("first 'bad' commit"),
            "{}\nCurrent: {}\nCandidate: {}",
            state.log,
            state.current,
            state.candidate
        );
        assert_eq!(state.current, commits[3]);
        act(root, Request::BisectReset).unwrap();
        assert!(snapshot(root, None).unwrap().bisect.is_none());
        assert_eq!(
            git(root, &["symbolic-ref", "--short", "HEAD"])
                .unwrap()
                .trim(),
            "main"
        );
        assert_eq!(
            git(root, &["rev-parse", "HEAD"]).unwrap().trim(),
            commits[5]
        );
        assert_eq!(
            git(
                root,
                &["rev-parse", &format!("refs/gall/recovery/{}", commits[5])]
            )
            .unwrap()
            .trim(),
            commits[5]
        );
    }

    #[test]
    fn notes_export_patch_and_apply_preserve_reviewed_data() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        fs::write(root.join("file.txt"), "first\n").unwrap();
        let first = commit(root, "First");
        fs::write(root.join("file.txt"), "second\n").unwrap();
        let second = commit(root, "Second");
        act(
            root,
            Request::WriteNote {
                commit: second.clone(),
                message: "A useful note".into(),
            },
        )
        .unwrap();
        assert_eq!(
            snapshot(root, Some(&second)).unwrap().notes.trim(),
            "A useful note"
        );
        let old_note = git(root, &["rev-parse", "refs/notes/commits"]).unwrap();
        let old_note = old_note.trim();
        act(
            root,
            Request::WriteNote {
                commit: second.clone(),
                message: "Replacement".into(),
            },
        )
        .unwrap();
        assert_eq!(
            git(
                root,
                &["rev-parse", &format!("refs/gall/recovery/notes-{old_note}")]
            )
            .unwrap()
            .trim(),
            old_note
        );
        let outputs = tempfile::tempdir().unwrap();
        let patch = outputs.path().join("second.patch");
        let patch = patch.to_str().unwrap().to_owned();
        act(
            root,
            Request::ExportPatch {
                commit: second.clone(),
                destination: patch.clone(),
            },
        )
        .unwrap();
        assert!(fs::read_to_string(&patch).unwrap().contains("+second"));
        let previous = fs::read(&patch).unwrap();
        assert!(act(
            root,
            Request::ExportPatch {
                commit: second.clone(),
                destination: patch.clone()
            }
        )
        .is_err());
        assert_eq!(fs::read(&patch).unwrap(), previous);
        git(root, &["checkout", "--detach", "-q", &first]).unwrap();
        act(
            root,
            Request::ApplyPatch {
                file: patch.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "second\n"
        );
        assert!(git(root, &["diff", "--cached", "--no-ext-diff"])
            .unwrap()
            .contains("+second"));
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), first);
        assert!(act(root, Request::ApplyPatch { file: patch }).is_err());
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "second\n"
        );
    }

    #[test]
    fn bisect_preserves_ignored_collisions_and_can_retry_checkout() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        fs::write(root.join(".gitignore"), "collision.txt\n").unwrap();
        fs::write(root.join("file.txt"), "first\n").unwrap();
        let first = commit(root, "First");
        fs::write(root.join("collision.txt"), "tracked middle\n").unwrap();
        git(root, &["add", "--force", "collision.txt"]).unwrap();
        let middle = commit(root, "Middle");
        fs::remove_file(root.join("collision.txt")).unwrap();
        let last = commit(root, "Last");
        fs::write(root.join("collision.txt"), "protected ignored data\n").unwrap();
        assert!(act(
            root,
            Request::BisectStart {
                good: first,
                bad: last.clone()
            }
        )
        .is_err());
        assert_eq!(
            fs::read_to_string(root.join("collision.txt")).unwrap(),
            "protected ignored data\n"
        );
        let state = snapshot(root, None).unwrap().bisect.unwrap();
        assert_eq!(state.current, last);
        assert_eq!(state.candidate, middle);
        assert!(state.checkout_pending);
        assert!(act(root, Request::BisectGood).is_err());
        let preserved = tempfile::tempdir().unwrap();
        fs::rename(
            root.join("collision.txt"),
            preserved.path().join("protected.txt"),
        )
        .unwrap();
        act(root, Request::BisectCheckout).unwrap();
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), middle);
        act(root, Request::BisectBad).unwrap();
        act(root, Request::BisectReset).unwrap();
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), last);
        assert_eq!(
            fs::read_to_string(preserved.path().join("protected.txt")).unwrap(),
            "protected ignored data\n"
        );
    }

    #[test]
    fn export_drains_the_complete_large_patch() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        fs::write(root.join("file.txt"), "first\n").unwrap();
        commit(root, "First");
        fs::write(
            root.join("file.txt"),
            format!("{}tail-marker\n", "a long changed line\n".repeat(20000)),
        )
        .unwrap();
        let last = commit(root, "Large patch");
        let outputs = tempfile::tempdir().unwrap();
        let destination = outputs.path().join("large.patch");
        act(
            root,
            Request::ExportPatch {
                commit: last,
                destination: destination.to_str().unwrap().into(),
            },
        )
        .unwrap();
        let text = fs::read_to_string(destination).unwrap();
        assert!(text.len() > 400000);
        assert!(text.contains("+tail-marker\n"));
    }

    #[test]
    fn initialize_clone_cancel_and_existing_destination_are_safe() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(snapshot(root, None).unwrap().repository.is_none());
        act(
            root,
            Request::Init {
                branch: "main".into(),
            },
        )
        .unwrap();
        git(root, &["config", "user.name", "Test"]).unwrap();
        git(root, &["config", "user.email", "test@example.com"]).unwrap();
        fs::write(root.join("file.txt"), "content\n").unwrap();
        commit(root, "First");
        let outputs = tempfile::tempdir().unwrap();
        let cloned = outputs.path().join("clone");
        let cloned = cloned.to_str().unwrap().to_owned();
        act(
            root,
            Request::Clone {
                url: root.to_str().unwrap().into(),
                destination: cloned.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(Path::new(&cloned).join("file.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "content\n"
        );
        assert!(act(
            root,
            Request::Clone {
                url: root.to_str().unwrap().into(),
                destination: cloned.clone()
            }
        )
        .is_err());
        let token = snapshot(root, None).unwrap().token;
        let cancelled = outputs.path().join("cancelled");
        assert!(execute(
            root,
            Request::Clone {
                url: root.to_str().unwrap().into(),
                destination: cancelled.to_str().unwrap().into()
            },
            &token,
            &AtomicBool::new(true),
            |_| {}
        )
        .is_err());
        assert!(!cancelled.exists());
        let invalid = outputs.path().join("failed");
        assert!(act(
            root,
            Request::Clone {
                url: outputs
                    .path()
                    .join("missing-repository")
                    .to_str()
                    .unwrap()
                    .into(),
                destination: invalid.to_str().unwrap().into()
            }
        )
        .is_err());
        // A failed clone must not leave a directory that blocks retrying the path.
        assert!(!invalid.exists());
        act(
            root,
            Request::Clone {
                url: root.to_str().unwrap().into(),
                destination: invalid.to_str().unwrap().into(),
            },
        )
        .unwrap();
        assert!(invalid.join("file.txt").exists());
    }

    #[test]
    fn submodule_update_preserves_local_edits() {
        let source = tempfile::tempdir().unwrap();
        init(source.path());
        fs::write(source.path().join("file.txt"), "first\n").unwrap();
        let first = commit(source.path(), "First");
        fs::write(source.path().join("file.txt"), "second\n").unwrap();
        let second = commit(source.path(), "Second");
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path();
        init(root);
        git(
            root,
            &[
                "-c",
                "protocol.file.allow=always",
                // Must apply while the module is cloned and checked out: a later config
                // change leaves CRLF files that Git then reports as modified.
                "-c",
                "core.autocrlf=false",
                "submodule",
                "add",
                "--",
                source.path().to_str().unwrap(),
                "module with spaces",
            ],
        )
        .unwrap();
        commit(root, "Submodule");
        git(root, &["config", "protocol.file.allow", "always"]).unwrap();
        let module = root.join("module with spaces");
        // The module is a clone: it does not inherit the repository-local setting.
        git(&module, &["config", "core.autocrlf", "false"]).unwrap();
        git(&module, &["checkout", "--detach", "-q", &first]).unwrap();
        assert_eq!(
            snapshot(root, None).unwrap().submodules[0].state,
            "differentCommit"
        );
        fs::write(module.join("file.txt"), "local edits\n").unwrap();
        assert!(act(
            root,
            Request::UpdateSubmodule {
                file: "module with spaces".into()
            }
        )
        .is_err());
        assert_eq!(
            fs::read_to_string(module.join("file.txt")).unwrap(),
            "local edits\n"
        );
        fs::write(module.join("file.txt"), "first\n").unwrap();
        act(
            root,
            Request::UpdateSubmodule {
                file: "module with spaces".into(),
            },
        )
        .unwrap();
        assert_eq!(git(&module, &["rev-parse", "HEAD"]).unwrap().trim(), second);
        assert_eq!(
            fs::read_to_string(module.join("file.txt")).unwrap(),
            "second\n"
        );
        git(root, &["submodule", "deinit", "--", "module with spaces"]).unwrap();
        assert_eq!(
            snapshot(root, None).unwrap().submodules[0].state,
            "notInitialized"
        );
        act(
            root,
            Request::InitSubmodule {
                file: "module with spaces".into(),
            },
        )
        .unwrap();
        assert_eq!(git(&module, &["rev-parse", "HEAD"]).unwrap().trim(), second);
    }

    #[test]
    fn submodule_update_preserves_ignored_file_and_directory_collisions() {
        let source = tempfile::tempdir().unwrap();
        init(source.path());
        fs::write(source.path().join("file.txt"), "first\n").unwrap();
        let first = commit(source.path(), "First");
        fs::write(source.path().join("cache"), "tracked cache\n").unwrap();
        fs::create_dir(source.path().join("tree")).unwrap();
        fs::write(source.path().join("tree/leaf.txt"), "tracked leaf\n").unwrap();
        fs::write(source.path().join("flat"), "tracked flat\n").unwrap();
        let target = commit(source.path(), "Tracked targets");
        let parent = tempfile::tempdir().unwrap();
        let p = parent.path();
        init(p);
        git(
            p,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--",
                source.path().to_str().unwrap(),
                "module",
            ],
        )
        .unwrap();
        commit(p, "Submodule");
        git(p, &["config", "protocol.file.allow", "always"]).unwrap();
        let module = p.join("module");
        git(&module, &["checkout", "--detach", "-q", &first]).unwrap();
        let exclude = git_path(&module, "info/exclude").unwrap();
        fs::write(exclude, "cache\ntree\nflat/\n").unwrap();
        for (file, value) in [
            ("cache", "valuable ignored file"),
            ("tree", "valuable ignored ancestor"),
            ("flat/kept.txt", "valuable ignored descendant"),
        ] {
            let location = module.join(file);
            fs::create_dir_all(location.parent().unwrap()).unwrap();
            fs::write(&location, value).unwrap();
            assert!(git(
                &module,
                &["status", "--porcelain=v1", "--untracked-files=all"]
            )
            .unwrap()
            .is_empty());
            let error = act(
                p,
                Request::UpdateSubmodule {
                    file: "module".into(),
                },
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("Untracked or ignored files"), "{error}");
            assert_eq!(fs::read_to_string(&location).unwrap(), value);
            assert_eq!(git(&module, &["rev-parse", "HEAD"]).unwrap().trim(), first);
            fs::remove_file(location).unwrap();
            if file.contains('/') {
                fs::remove_dir(module.join("flat")).unwrap();
            }
        }
        act(
            p,
            Request::UpdateSubmodule {
                file: "module".into(),
            },
        )
        .unwrap();
        assert_eq!(git(&module, &["rev-parse", "HEAD"]).unwrap().trim(), target);
    }

    #[test]
    fn submodule_initialization_preserves_a_nonempty_destination() {
        let source = tempfile::tempdir().unwrap();
        init(source.path());
        fs::write(source.path().join("cache"), "tracked cache\n").unwrap();
        commit(source.path(), "First");
        let parent = tempfile::tempdir().unwrap();
        let p = parent.path();
        init(p);
        git(
            p,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--",
                source.path().to_str().unwrap(),
                "module",
            ],
        )
        .unwrap();
        commit(p, "Submodule");
        git(p, &["config", "protocol.file.allow", "always"]).unwrap();
        git(p, &["submodule", "deinit", "--", "module"]).unwrap();
        let module = p.join("module");
        fs::write(module.join("cache"), "valuable existing file\n").unwrap();
        fs::create_dir(module.join("nested")).unwrap();
        fs::write(module.join("nested/local.txt"), "valuable nested file\n").unwrap();
        let error = act(
            p,
            Request::InitSubmodule {
                file: "module".into(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("destination contains files"), "{error}");
        assert_eq!(
            fs::read_to_string(module.join("cache")).unwrap(),
            "valuable existing file\n"
        );
        assert_eq!(
            fs::read_to_string(module.join("nested/local.txt")).unwrap(),
            "valuable nested file\n"
        );
        assert!(!module.join(".git").exists());
        fs::remove_file(module.join("cache")).unwrap();
        fs::remove_dir_all(module.join("nested")).unwrap();
        act(
            p,
            Request::InitSubmodule {
                file: "module".into(),
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(module.join("cache"))
                .unwrap()
                .replace("\r\n", "\n"),
            "tracked cache\n"
        );
    }

    #[test]
    fn worktree_removal_preserves_ignored_files_and_empty_directories() {
        let directory = tempfile::tempdir().unwrap();
        let p = directory.path();
        init(p);
        fs::write(p.join(".gitignore"), "cache\nempty-cache/\n").unwrap();
        let head = commit(p, "Ignore local caches");
        let destinations = tempfile::tempdir().unwrap();
        let tree = reported(destinations.path()).join("worktree");
        let destination = tree.to_str().unwrap().to_owned();
        act(
            p,
            Request::CreateWorktree {
                destination: destination.clone(),
                target: head,
                branch: Some("topic".into()),
            },
        )
        .unwrap();
        fs::write(tree.join("cache"), "valuable ignored cache\n").unwrap();
        fs::create_dir(tree.join("empty-cache")).unwrap();
        assert!(git(
            &tree,
            &["status", "--porcelain=v1", "--untracked-files=all"]
        )
        .unwrap()
        .is_empty());
        let error = act(
            p,
            Request::RemoveWorktree {
                destination: destination.clone(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Untracked or ignored files"), "{error}");
        assert_eq!(
            fs::read_to_string(tree.join("cache")).unwrap(),
            "valuable ignored cache\n"
        );
        assert_eq!(snapshot(p, None).unwrap().worktrees.len(), 2);
        fs::remove_file(tree.join("cache")).unwrap();
        assert!(act(
            p,
            Request::RemoveWorktree {
                destination: destination.clone()
            }
        )
        .is_err());
        assert!(tree.join("empty-cache").is_dir());
        fs::remove_dir(tree.join("empty-cache")).unwrap();
        act(p, Request::RemoveWorktree { destination }).unwrap();
        assert!(!tree.exists());
    }

    #[test]
    fn notes_reject_symbolic_reference_without_moving_its_branch_target() {
        let directory = tempfile::tempdir().unwrap();
        let p = directory.path();
        init(p);
        fs::write(p.join("file.txt"), "first\n").unwrap();
        let head = commit(p, "First");
        git(
            p,
            &["symbolic-ref", "refs/notes/commits", "refs/heads/main"],
        )
        .unwrap();
        for message in ["A note that must not change main", ""] {
            let error = act(
                p,
                Request::WriteNote {
                    commit: head.clone(),
                    message: message.into(),
                },
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("notes reference is symbolic"), "{error}");
            assert_eq!(
                git(p, &["rev-parse", "refs/heads/main"]).unwrap().trim(),
                head
            );
            assert_eq!(
                git(p, &["symbolic-ref", "refs/notes/commits"])
                    .unwrap()
                    .trim(),
                "refs/heads/main"
            );
            assert_eq!(git(p, &["rev-parse", "HEAD"]).unwrap().trim(), head);
        }
        assert!(git(p, &["status", "--porcelain=v1"]).unwrap().is_empty());
    }

    #[test]
    fn stale_and_hostile_inputs_cannot_mutate_or_escape() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        fs::write(root.join("file.txt"), "first\n").unwrap();
        let first = commit(root, "First");
        let stale = snapshot(root, None).unwrap().token;
        fs::write(root.join("file.txt"), "local edits\n").unwrap();
        assert!(execute(
            root,
            Request::WriteNote {
                commit: first.clone(),
                message: "stale".into()
            },
            &stale,
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err());
        assert!(act(
            root,
            Request::WriteNote {
                commit: "--all".into(),
                message: "invalid".into()
            }
        )
        .is_err());
        assert!(act(
            root,
            Request::UpdateSubmodule {
                file: "../../outside".into()
            }
        )
        .is_err());
        assert!(act(
            root,
            Request::Clone {
                url: "ext::sh -c echo forbidden".into(),
                destination: root.join("forbidden").to_str().unwrap().into()
            }
        )
        .is_err());
        assert!(act(
            root,
            Request::Clone {
                url: "--upload-pack=forbidden".into(),
                destination: root.join("forbidden").to_str().unwrap().into()
            }
        )
        .is_err());
        assert!(!root.join("forbidden").exists());
        let outputs = tempfile::tempdir().unwrap();
        let hostile = outputs.path().join("hostile.patch");
        fs::write(&hostile, "diff --git a/../outside.txt b/../outside.txt\nnew file mode 100644\n--- /dev/null\n+++ b/../outside.txt\n@@ -0,0 +1 @@\n+escape\n").unwrap();
        fs::write(root.join("file.txt"), "first\n").unwrap();
        assert!(act(
            root,
            Request::ApplyPatch {
                file: hostile.to_str().unwrap().into()
            }
        )
        .is_err());
        assert!(!root.parent().unwrap().join("outside.txt").exists());
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), first);
    }

    #[test]
    #[cfg(unix)]
    fn symlink_export_and_patch_inputs_are_rejected() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        fs::write(root.join("file.txt"), "first\n").unwrap();
        let first = commit(root, "First");
        let outputs = tempfile::tempdir().unwrap();
        let protected = outputs.path().join("protected");
        fs::write(&protected, "protected").unwrap();
        let link = outputs.path().join("link.patch");
        symlink(&protected, &link).unwrap();
        assert!(act(
            root,
            Request::ExportPatch {
                commit: first,
                destination: link.to_str().unwrap().into()
            }
        )
        .is_err());
        assert!(act(
            root,
            Request::ApplyPatch {
                file: link.to_str().unwrap().into()
            }
        )
        .is_err());
        assert_eq!(fs::read_to_string(protected).unwrap(), "protected");
    }
}
