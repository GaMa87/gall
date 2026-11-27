//! Read-only inspection of commits, changed files and line provenance.
use super::gitgraph::root;
use crate::{CoreError, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;

const METADATA_LIMIT: usize = 8 * 1024 * 1024;
pub(super) const PATCH_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub original: Option<String>,
    pub status: String,
    pub added: Option<u64>,
    pub deleted: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Details {
    pub root: String,
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub date: String,
    pub committer: String,
    pub commit_date: String,
    pub message: String,
    pub base: Option<String>,
    pub files: Vec<ChangedFile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    /// The name of the file in that commit.
    pub file: String,
    /// The parents of the commit, so a merge and its parent choice are known without its details.
    pub parents: Vec<String>,
}

/// A commit that changed the chosen lines, with the part of its diff that did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineHistoryEntry {
    pub id: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub patch: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineHistory {
    pub entries: Vec<LineHistoryEntry>,
    /// There may be more than these.
    pub truncated: bool,
}

/// The code at the start of the error of a file that is not in the chosen comparison: callers (the window) recognise
/// it by this code, never by the words after it, which may change or be translated.
pub const NOT_IN_COMPARISON: &str = "not-in-comparison";

fn error(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// The bytes of one stage of an unresolved file in the index: 1 the common ancestor, 2 ours, 3 theirs. `None` when
/// the file has no such stage (a file added on both sides has no ancestor, a deleted one lacks its side). More than
/// `limit` bytes come back as `limit + 1` bytes, so a caller can tell a stage that is too large.
pub fn conflict_stage(path: &Path, file: &str, stage: u8, limit: usize) -> Result<Option<Vec<u8>>> {
    if !(1..=3).contains(&stage) {
        return Err(error("A stage is 1 (base), 2 (ours) or 3 (theirs)"));
    }
    relative_file(file)?;
    let root = root(path)?;
    let spec = format!(":(literal){file}");
    let (listing, _) = read_git(
        &root,
        &["ls-files", "--unmerged", "-z", "--", &spec],
        METADATA_LIMIT,
    )?;
    let listing = String::from_utf8_lossy(&listing);
    // `<mode> <object> <stage>\t<path>` for each stage, separated by NUL.
    let object = listing.split('\0').find_map(|entry| {
        let (info, _) = entry.split_once('\t')?;
        let mut parts = info.split(' ');
        let (_, object, found) = (parts.next()?, parts.next()?, parts.next()?);
        (found == stage.to_string() && object.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| object.to_owned())
    });
    let Some(object) = object else {
        return Ok(None);
    };
    let (bytes, truncated) = read_git(&root, &["cat-file", "blob", &object], limit)?;
    let mut bytes = bytes;
    if truncated && bytes.len() <= limit {
        bytes.resize(limit + 1, b' ');
    }
    Ok(Some(bytes))
}

/// Limit output before allocating a large preview. No subprocess runs an external
/// diff, text converter, pager or editor, and no optional index refresh is needed.
pub(super) fn read_git(root: &Path, args: &[&str], limit: usize) -> Result<(Vec<u8>, bool)> {
    read_git_until(root, args, limit, None)
}

/// [`read_git`] that stops the process when `cancelled` says so, also while it prints nothing.
fn read_git_until(
    root: &Path,
    args: &[&str],
    limit: usize,
    cancelled: Option<&(dyn Fn() -> bool + Sync)>,
) -> Result<(Vec<u8>, bool)> {
    let mut command = super::gitexec::command(root, super::gitstatus::program());
    command
        .args([
            "--no-pager",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "log.showSignature=false",
            "-c",
            "color.ui=false",
            "-C",
        ])
        .arg(super::gitexec::dir_arg(root))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1");
    super::gitexec::forward_env(&mut command);
    let mut child = command.spawn().map_err(|e| error(e.to_string()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| error("Missing Git stderr"))?;
    let error_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.by_ref().take(16384).read_to_end(&mut bytes);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        bytes
    });
    let mut bytes = Vec::new();
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| error("Missing Git stdout"))?
        .take(limit as u64 + 1);
    let child = std::sync::Mutex::new(child);
    let kill = || {
        if let Ok(mut child) = child.lock() {
            let _ = child.kill();
        }
    };
    let done = std::sync::atomic::AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let read_all = |stdout: &mut std::io::Take<std::process::ChildStdout>,
                        bytes: &mut Vec<u8>| {
            if !super::gitprogress::is_active() {
                return stdout.read_to_end(bytes);
            }
            let mut buf = [0u8; 8192];
            let mut records = 0u64;
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => return Ok(bytes.len()),
                    Ok(n) => {
                        bytes.extend_from_slice(&buf[..n]);
                        let added = buf[..n].iter().filter(|&&b| b == 0x1e).count() as u64;
                        if added > 0 {
                            records += added;
                            super::gitprogress::report_counts(
                                "Loading line history",
                                records,
                                None,
                                "commits",
                                None,
                            );
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        let Some(cancelled) = cancelled else {
            return read_all(&mut stdout, &mut bytes);
        };
        let watch = scope.spawn(|| {
            while !done.load(std::sync::atomic::Ordering::Acquire) {
                if cancelled() {
                    kill();
                    return;
                }
                std::thread::park_timeout(std::time::Duration::from_millis(30));
            }
        });
        let result = read_all(&mut stdout, &mut bytes);
        done.store(true, std::sync::atomic::Ordering::Release);
        watch.thread().unpark();
        result
    });
    let truncated = bytes.len() > limit;
    if truncated || result.is_err() {
        kill();
    }
    let mut child = child
        .into_inner()
        .map_err(|_| error("A Git worker thread stopped unexpectedly"))?;
    let status = child.wait().map_err(|e| error(e.to_string()))?;
    let stderr = error_reader.join().unwrap_or_default();
    if cancelled.is_some_and(|cancelled| cancelled()) {
        return Err(error("Cancelled"));
    }
    result.map_err(|e| error(e.to_string()))?;
    if !truncated && !status.success() {
        return Err(error(String::from_utf8_lossy(&stderr).trim().to_owned()));
    }
    bytes.truncate(limit);
    Ok((bytes, truncated))
}

fn text(root: &Path, args: &[&str]) -> Result<String> {
    let (bytes, truncated) = read_git(root, args, METADATA_LIMIT)?;
    if truncated {
        return Err(error("Git metadata exceeds the 8 MiB inspection limit"));
    }
    String::from_utf8(bytes).map_err(|_| error("Git metadata contains invalid UTF-8"))
}

pub(super) fn commit_id(root: &Path, value: &str) -> Result<String> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error("A full Git commit ID is required"));
    }
    let value = value.to_ascii_lowercase();
    let actual = text(
        root,
        &["rev-parse", "--verify", &format!("{value}^{{commit}}")],
    )?;
    if actual.trim() != value {
        return Err(error("The selected object is not a commit"));
    }
    Ok(value)
}

pub(super) fn relative_file(file: &str) -> Result<()> {
    if file.is_empty()
        || file.contains('\0')
        || file.starts_with('/')
        || file
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || (cfg!(windows) && (file.contains('\\') || file.contains(':')))
    {
        return Err(error("A repository-relative file path is required"));
    }
    Ok(())
}

fn changed_files(root: &Path, base: &str, commit: &str) -> Result<Vec<ChangedFile>> {
    let list = |kind: &str| {
        text(
            root,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                kind,
                "-z",
                "-M",
                base,
                commit,
                "--",
            ],
        )
    };
    // Names and line counts are two queries: asked at the same time.
    let (names, stats) = std::thread::scope(|scope| {
        let stats = scope.spawn(|| list("--numstat"));
        (list("--name-status"), stats.join())
    });
    let names = names?;
    let stats = stats.map_err(|_| error("A Git worker thread stopped unexpectedly"))??;
    let mut counts = HashMap::new();
    let mut records = stats.split('\0').filter(|s| !s.is_empty());
    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, '\t');
        let added = fields.next().and_then(|v| v.parse::<u64>().ok());
        let deleted = fields.next().and_then(|v| v.parse::<u64>().ok());
        let file = fields
            .next()
            .ok_or_else(|| error("Invalid Git numstat record"))?;
        let file = if file.is_empty() {
            records
                .next()
                .ok_or_else(|| error("Missing rename source"))?;
            records
                .next()
                .ok_or_else(|| error("Missing rename target"))?
        } else {
            file
        };
        counts.insert(file.to_owned(), (added, deleted));
    }
    let mut records = names.split('\0').filter(|s| !s.is_empty());
    let mut files = Vec::new();
    while let Some(status) = records.next() {
        let first = records
            .next()
            .ok_or_else(|| error("Missing changed file path"))?;
        let renamed = status.starts_with('R') || status.starts_with('C');
        let (file, original) = if renamed {
            (
                records
                    .next()
                    .ok_or_else(|| error("Missing changed rename target"))?,
                Some(first.to_owned()),
            )
        } else {
            (first, None)
        };
        let (added, deleted) = counts.remove(file).unwrap_or((None, None));
        files.push(ChangedFile {
            path: file.into(),
            original,
            status: status.into(),
            added,
            deleted,
        });
    }
    Ok(files)
}

/// Read metadata and compare a commit to its first parent or an explicit base.
/// Root commits are compared to the empty tree without storing an object.
pub fn details(path: &Path, commit: &str, base: Option<&str>) -> Result<Details> {
    let root = root(path)?;
    // A commit and its comparison with another commit never change, and the patch of
    // every file clicked in the list starts from these details: they are kept.
    let key = (
        root.clone(),
        commit.to_ascii_lowercase(),
        base.map(str::to_ascii_lowercase),
    );
    if let Ok(known) = KNOWN_DETAILS.lock() {
        if let Some((_, details)) = known.iter().find(|(k, _)| *k == key) {
            return Ok(details.clone());
        }
    }
    let details = read_details(&root, commit, base)?;
    if let Ok(mut known) = KNOWN_DETAILS.lock() {
        if known.len() >= 16 {
            known.remove(0);
        }
        known.push((key, details.clone()));
    }
    Ok(details)
}

type DetailsKey = (std::path::PathBuf, String, Option<String>);
static KNOWN_DETAILS: std::sync::Mutex<Vec<(DetailsKey, Details)>> =
    std::sync::Mutex::new(Vec::new());

/// The tree without entries, which a root commit is compared to. Its ID is a constant of
/// the object format (SHA-1 or SHA-256, told apart by the length of `like`).
fn empty_tree(like: &str) -> &'static str {
    if like.len() == 64 {
        "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321"
    } else {
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
    }
}

fn read_details(root: &Path, commit: &str, base: Option<&str>) -> Result<Details> {
    let root = root.to_path_buf();
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error("A full Git commit ID is required"));
    }
    let id = commit.to_ascii_lowercase();
    // One query both checks that the ID names a commit (not a tag or a tree, which
    // `^{commit}` would peel or refuse) and reads it.
    let metadata = text(
        &root,
        &[
            "show",
            "--no-show-signature",
            "-s",
            "--format=%H%x00%P%x00%an%x00%ae%x00%aI%x00%cn%x00%cI%x00%B",
            &format!("{id}^{{commit}}"),
            "--",
        ],
    )?;
    let mut fields = metadata.splitn(8, '\0');
    if fields.next() != Some(id.as_str()) {
        return Err(error("The selected object is not a commit"));
    }
    let parents: Vec<String> = fields
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let author = fields.next().unwrap_or_default().to_owned();
    let email = fields.next().unwrap_or_default().to_owned();
    let date = fields.next().unwrap_or_default().to_owned();
    let committer = fields.next().unwrap_or_default().to_owned();
    let commit_date = fields.next().unwrap_or_default().to_owned();
    let message = fields.next().unwrap_or_default().trim_end().to_owned();
    let base = match base {
        Some(value) => Some(commit_id(&root, value)?),
        None => parents.first().cloned(),
    };
    let files = changed_files(&root, base.as_deref().unwrap_or(empty_tree(&id)), &id)?;
    Ok(Details {
        root: root.to_string_lossy().into_owned(),
        id,
        parents,
        author,
        email,
        date,
        committer,
        commit_date,
        message,
        base,
        files,
    })
}

/// The whole diff of a commit against its first parent (or the empty tree for a root commit), for
/// copying to the clipboard. Read-only and capped like the per-file preview.
pub fn commit_diff(path: &Path, commit: &str) -> Result<String> {
    let details = details(path, commit, None)?;
    let root = Path::new(&details.root);
    let empty = empty_tree(&details.id);
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "-M",
        details.base.as_deref().unwrap_or(empty),
        &details.id,
    ];
    let (bytes, truncated) = read_git(root, &args, PATCH_LIMIT)?;
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        output.push_str("\n[Diff truncated at 1 MiB]\n");
    }
    Ok(output)
}

/// The commit as an email-style patch (`git format-patch`), for copying to the clipboard so it can be
/// pasted into "Apply patch from clipboard" or `git am`. Unlike `commit_diff` it is never truncated:
/// a cut-off patch would not apply, so a commit above 16 MiB (or with non-UTF-8 content) is refused.
pub fn commit_patch_text(path: &Path, commit: &str) -> Result<String> {
    const LIMIT: usize = 16 * 1024 * 1024;
    let details = details(path, commit, None)?;
    if details.parents.len() > 1 {
        return Err(error(
            "A merge commit has no single-parent patch; select a non-merge commit",
        ));
    }
    let root = Path::new(&details.root);
    let mut args = vec!["format-patch", "-1", "--stdout", "--no-signature"];
    if details.parents.is_empty() {
        args.push("--root");
    }
    args.extend([
        "--no-stat",
        "--binary",
        "--full-index",
        "--no-ext-diff",
        "--no-textconv",
        details.id.as_str(),
    ]);
    let (bytes, truncated) = read_git(root, &args, LIMIT)?;
    if truncated {
        return Err(error(
            "This commit's patch is larger than 16 MiB; export it to a file instead",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| error("This commit's patch is not valid UTF-8; export it to a file instead"))
}

/// Preview only a path known to be in this comparison, preserving rename context.
pub fn patch(path: &Path, commit: &str, base: Option<&str>, file: &str) -> Result<String> {
    relative_file(file)?;
    let details = details(path, commit, base)?;
    let entry = details
        .files
        .iter()
        .find(|entry| entry.path == file)
        .ok_or_else(|| {
            error(format!(
                "{NOT_IN_COMPARISON}: File is not in the selected comparison"
            ))
        })?;
    let root = Path::new(&details.root);
    let empty = empty_tree(&details.id);
    let spec = format!(":(literal){file}");
    let original = entry
        .original
        .as_ref()
        .map(|name| format!(":(literal){name}"));
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "-M",
        details.base.as_deref().unwrap_or(empty),
        &details.id,
        "--",
        &spec,
    ];
    if let Some(original) = &original {
        args.push(original);
    }
    let (bytes, truncated) = read_git(root, &args, PATCH_LIMIT)?;
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        output.push_str("\n[Diff preview truncated at 1 MiB]\n");
    }
    Ok(output)
}

/// The file that `commit` renamed or copied to `file`, if it did (as `git log --follow`
/// finds it). A merge is compared with no parent, so the history ends there.
fn came_from(root: &Path, commit: &str, file: &str) -> Result<Option<String>> {
    let found = text(
        root,
        &[
            "diff-tree",
            "--no-commit-id",
            "-r",
            "-M",
            "-C",
            "--diff-filter=RC",
            "--name-status",
            "-z",
            commit,
        ],
    )?;
    // `R<score>` or `C<score>`, the old name, the new name.
    let mut fields = found.split('\0');
    while let (Some(_), Some(old), Some(new)) = (fields.next(), fields.next(), fields.next()) {
        if new == file {
            return Ok(Some(old.to_owned()));
        }
    }
    Ok(None)
}

/// Resolve the worktree and HEAD sides without treating the index as either side.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeComparison {
    pub left: Option<super::diff::DiffSource>,
    pub right: Option<super::diff::DiffSource>,
    pub last_touch: Option<HistoryEntry>,
}

fn worktree_path(path: &Path) -> Result<(std::path::PathBuf, std::path::PathBuf, String)> {
    use crate::error::IoContext;

    let parent = path
        .parent()
        .ok_or_else(|| error("A file path is required"))?;
    let name = path
        .file_name()
        .ok_or_else(|| error("A file path is required"))?;
    let mut existing = parent;
    let resolved_parent = loop {
        match crate::nav::path::canonicalize(existing) {
            Ok(resolved) => break resolved,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                existing = existing
                    .parent()
                    .ok_or_else(|| error("No existing folder for this file"))?;
            }
            Err(e) => return Err(CoreError::io("resolve folder", existing, e)),
        }
    };
    let root =
        crate::nav::path::canonicalize(&root(&resolved_parent)?).ctx("resolve repository", path)?;
    let tail = parent
        .strip_prefix(existing)
        .map_err(|_| error("Could not resolve file path"))?;
    let absolute = resolved_parent.join(tail).join(name);
    let relative = absolute
        .strip_prefix(&root)
        .map_err(|_| error("File is outside the repository"))?;
    let file = relative
        .to_str()
        .ok_or_else(|| error("File path contains invalid UTF-8"))?;
    let file = file.replace(std::path::MAIN_SEPARATOR, "/");
    relative_file(&file)?;
    Ok((root, absolute, file))
}

pub fn worktree_comparison(path: &Path) -> Result<WorktreeComparison> {
    use super::diff::DiffSource;
    let (root, absolute, file) = worktree_path(path)?;
    let right = match std::fs::symlink_metadata(&absolute) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(&absolute)
                .map_err(|e| CoreError::io("read link", &absolute, e))?;
            let text = target
                .into_os_string()
                .into_string()
                .map_err(|_| error("Symbolic link target contains invalid UTF-8"))?;
            Some(DiffSource::Text {
                label: file.clone(),
                text,
            })
        }
        Ok(meta) if meta.is_file() => Some(DiffSource::File {
            path: absolute.clone(),
        }),
        Ok(_) => return Err(error("A regular worktree file is required")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(crate::CoreError::io("read", &absolute, e)),
    };

    let head = text(&root, &["rev-parse", "--verify", "HEAD^{commit}"]);
    let head = match head {
        Ok(head) => head.trim().to_owned(),
        Err(e) => {
            let branch = text(&root, &["symbolic-ref", "--quiet", "HEAD"]).map_err(|_| e)?;
            if text(&root, &["show-ref", "--verify", branch.trim()]).is_ok() {
                return Err(error("Could not resolve the HEAD commit"));
            }
            return Ok(WorktreeComparison {
                left: None,
                right,
                last_touch: None,
            });
        }
    };
    let Some(original) = head_blob_path(&root, &file, &head)? else {
        return Ok(WorktreeComparison {
            left: None,
            right,
            last_touch: None,
        });
    };
    let last_touch = file_history(&root, &original, 1, Some(&head))?
        .into_iter()
        .next();
    let left = Some(DiffSource::GitBlob {
        root,
        file: original,
        commit: head,
    });
    Ok(WorktreeComparison {
        left,
        right,
        last_touch,
    })
}

fn head_blob_path(root: &Path, file: &str, head: &str) -> Result<Option<String>> {
    let has_blob = |name: &str| -> Result<bool> {
        let record = text(
            root,
            &[
                "ls-tree",
                "-r",
                "-z",
                head,
                "--",
                &format!(":(literal){name}"),
            ],
        )?;
        Ok(record.split_terminator('\0').any(|record| {
            record.split_once('\t').is_some_and(|(fields, path)| {
                path == name && fields.split_whitespace().nth(1) == Some("blob")
            })
        }))
    };
    if has_blob(file)? {
        return Ok(Some(file.to_owned()));
    }
    let renamed = text(
        root,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "-M",
            "--diff-filter=R",
            "--name-status",
            "-z",
            head,
            "--",
        ],
    )?;
    let mut fields = renamed.split_terminator('\0');
    while let (Some(status), Some(from), Some(to)) = (fields.next(), fields.next(), fields.next()) {
        if status.starts_with('R') && to == file && has_blob(from)? {
            return Ok(Some(from.to_owned()));
        }
    }
    Ok(None)
}

pub fn file_history(
    path: &Path,
    file: &str,
    limit: usize,
    commit: Option<&str>,
) -> Result<Vec<HistoryEntry>> {
    Ok(file_history_page(path, file, limit, commit)?.entries)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryPage {
    pub entries: Vec<HistoryEntry>,
    pub truncated: bool,
    pub incomplete: bool,
    pub read_only_reason: Option<String>,
}

pub fn scoped_history(
    repository: &Path,
    path: &Path,
    limit: usize,
    commit: Option<&str>,
) -> Result<FileHistoryPage> {
    use crate::error::IoContext;
    let (root, _, file) = worktree_path(path)?;
    if root != crate::nav::path::canonicalize(repository).ctx("resolve repository", repository)? {
        return Err(error("File is outside the selected repository"));
    }
    let read_only_reason =
        super::gitgraph::read_only_reason(super::gitgraph::has_content_filters(&root));
    let Some(head) = commit else {
        return Ok(FileHistoryPage {
            entries: Vec::new(),
            truncated: false,
            incomplete: false,
            read_only_reason,
        });
    };
    let head = commit_id(&root, head)?;
    let historical_path = head_blob_path(&root, &file, &head)?.unwrap_or(file);
    let mut page = file_history_page(&root, &historical_path, limit, Some(&head))?;
    page.read_only_reason = read_only_reason;
    Ok(page)
}

pub fn file_history_page(
    path: &Path,
    file: &str,
    limit: usize,
    commit: Option<&str>,
) -> Result<FileHistoryPage> {
    relative_file(file)?;
    let root = root(path)?;
    super::gitfacts::keep_commit_graph(&root);
    let page_size = limit.clamp(1, 1000);
    let limit = page_size + 1;
    let start = commit.map(|value| commit_id(&root, value)).transpose()?;
    // The history is followed across renames here instead of with `git log --follow`,
    // which compares the trees of every commit of the repository (0.6 s for 20 000
    // commits). A plain `log -- path` uses the changed-path filters of the commit-graph
    // and stops at the commit that brought the path in; if that commit renamed another
    // file to it, the history goes on from its parent under the old name.
    let mut output = String::new();
    // The name of the file for each commit in `output`.
    let mut names: Vec<String> = Vec::new();
    let mut from = start.unwrap_or_else(|| "HEAD".to_owned());
    let mut name = file.to_owned();
    let mut found = 0;
    let mut incomplete = false;
    for hop in 0..32 {
        let spec = format!(":(literal){name}");
        let count = format!("-{}", limit - found);
        // Next to the list: the newest commit that brought this name in. A name can have
        // been used by another file before; the history of this one starts there.
        let (part, added) = std::thread::scope(|scope| {
            let added = scope.spawn(|| {
                text(
                    &root,
                    &[
                        "log",
                        "--no-show-signature",
                        "-1",
                        "--diff-filter=A",
                        "--format=%H",
                        &from,
                        "--",
                        &spec,
                    ],
                )
            });
            let part = text(
                &root,
                &[
                    "log",
                    "--no-show-signature",
                    &count,
                    "--format=%H%x00%P%x00%s%x00%an%x00%aI%x00",
                    &from,
                    "--",
                    &spec,
                ],
            );
            (part, added.join())
        });
        let part = part?;
        incomplete |= !matches!(&added, Ok(Ok(_)));
        let added = added
            .ok()
            .and_then(|added| added.ok())
            .unwrap_or_default()
            .trim()
            .to_owned();
        let ids: Vec<&str> = part
            .split('\0')
            .step_by(5)
            .map(|id| id.trim_start_matches('\n'))
            .filter(|id| !id.is_empty())
            .collect();
        let start = ids.iter().position(|id| *id == added);
        let kept = start.map_or(ids.len(), |at| at + 1);
        output.extend(part.split_inclusive('\0').take(kept * 5));
        names.extend(std::iter::repeat_n(name.clone(), kept));
        found += kept;
        super::gitprogress::report_counts(
            "Loading file history",
            found.min(page_size) as u64,
            None,
            "commits",
            Some(file.to_owned()),
        );
        // The start is not among the commits listed (the limit came first): done.
        if start.is_none() || found >= limit {
            break;
        }
        match came_from(&root, &added, &name) {
            Ok(Some(old)) => {
                from = format!("{added}^");
                name = old;
                incomplete |= hop == 31;
            }
            Ok(None) => break,
            Err(_) => {
                incomplete = true;
                break;
            }
        }
    }
    let mut fields = output.split('\0');
    let mut entries = Vec::new();
    while let Some(id) = fields.next() {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        let parents = fields
            .next()
            .ok_or_else(|| error("Invalid file history record"))?;
        let subject = fields
            .next()
            .ok_or_else(|| error("Invalid file history record"))?;
        let author = fields
            .next()
            .ok_or_else(|| error("Invalid file history record"))?;
        let date = fields
            .next()
            .ok_or_else(|| error("Invalid file history record"))?;
        entries.push(HistoryEntry {
            parents: parents.split_whitespace().map(str::to_owned).collect(),
            id: id.into(),
            subject: subject.into(),
            author: author.into(),
            date: date.into(),
            file: names
                .get(entries.len())
                .cloned()
                .unwrap_or_else(|| file.to_owned()),
        });
    }
    let truncated = entries.len() > page_size;
    entries.truncate(page_size);
    Ok(FileHistoryPage {
        entries,
        truncated,
        incomplete,
        read_only_reason: None,
    })
}

/// The commits that changed the lines `start` to `end` (1-based) of a file as it is in `commit`,
/// newest first (`git log -L`), across renames.
pub fn line_history(
    path: &Path,
    commit: &str,
    file: &str,
    (start, end): (u32, u32),
    limit: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<LineHistory> {
    relative_file(file)?;
    if start == 0 || end < start {
        return Err(error("Choose lines of the file"));
    }
    let root = root(path)?;
    let commit = commit_id(&root, commit)?;
    let limit = limit.clamp(1, 500);
    // One more than asked for tells whether there are more.
    let count = format!("-{}", limit + 1);
    // Everything after the first `:` is the name, whatever it contains.
    let lines = format!("-L{start},{end}:{file}");
    let (bytes, cut) = read_git_until(
        &root,
        &[
            "log",
            "--no-show-signature",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-prefix",
            &count,
            "--format=%x1e%H%x00%s%x00%an%x00%aI%x00",
            &lines,
            &commit,
            "--",
        ],
        METADATA_LIMIT,
        Some(cancelled),
    )?;
    let output = String::from_utf8_lossy(&bytes);
    let mut entries = parse_line_history(&output);
    // The record the limit cut off may lack the end of its diff.
    if cut {
        entries.pop();
    }
    let truncated = cut || entries.len() > limit;
    entries.truncate(limit);
    Ok(LineHistory { entries, truncated })
}

fn parse_line_history(output: &str) -> Vec<LineHistoryEntry> {
    let mut entries: Vec<LineHistoryEntry> = Vec::new();
    for part in output.split('\x1e').skip(1) {
        let mut fields = part.splitn(5, '\0');
        let record = (|| {
            let id = fields.next()?;
            let whole = matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit());
            whole.then_some(())?;
            Some(LineHistoryEntry {
                id: id.into(),
                subject: fields.next()?.into(),
                author: fields.next()?.into(),
                date: fields.next()?.into(),
                patch: fields.next()?.trim_start_matches('\n').into(),
            })
        })();
        match (record, entries.last_mut()) {
            (Some(entry), _) => entries.push(entry),
            // The separator was a character of the diff.
            (None, Some(last)) => {
                last.patch.push('\x1e');
                last.patch.push_str(part);
            }
            (None, None) => {}
        }
    }
    for entry in &mut entries {
        let end = entry.patch.trim_end_matches('\n').len();
        entry.patch.truncate(end);
    }
    entries
}

pub fn content(path: &Path, commit: &str, file: &str) -> Result<String> {
    relative_file(file)?;
    let root = root(path)?;
    let commit = commit_id(&root, commit)?;
    let spec = format!("{commit}:{file}");
    let (bytes, truncated) = read_git(&root, &["cat-file", "blob", &spec], PATCH_LIMIT)?;
    if bytes.contains(&0) {
        return Err(error("Binary file (text preview unavailable)"));
    }
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        output.push_str("\n[File preview truncated at 1 MiB]\n");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::super::gitgraph::git;
    use super::*;
    #[test]
    fn working_comparison_resolves_the_head_blob_and_ignores_staged_contents() {
        use super::super::diff::{compare_paged, DiffOptions, DiffSource};
        use std::sync::atomic::AtomicBool;

        let directory = repository();
        let root = directory.path();
        fs::create_dir(root.join("nested")).unwrap();
        let file = root.join("nested/- č spaced.txt");
        fs::write(&file, "head\n").unwrap();
        let head = commit(root, "initial blob");
        let compare = || worktree_comparison(&file).unwrap();
        assert_eq!(compare().last_touch.unwrap().id, head);
        fs::write(&file, "staged\n").unwrap();
        git(root, &["add", "--", "nested/- č spaced.txt"]).unwrap();
        fs::write(&file, "working\n").unwrap();
        let resolved = compare();
        let left = resolved.left.unwrap();
        let right = resolved.right.unwrap();
        let patch = super::super::diff::unified_patch(&left, &right).unwrap();
        assert!(patch
            .lines()
            .any(|line| line == "--- \"a/nested/- č spaced.txt\""));
        assert!(patch
            .lines()
            .any(|line| line == "+++ \"b/nested/- č spaced.txt\""));
        let result = compare_paged(
            &left,
            &right,
            DiffOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let text = result.text.unwrap();
        assert_eq!(text.diff().left_lines, ["head"]);
        assert_eq!(text.diff().right_lines, ["working"]);

        fs::remove_file(&file).unwrap();
        fs::remove_dir(root.join("nested")).unwrap();
        let deleted = compare();
        assert!(deleted.right.is_none());
        assert!(matches!(deleted.left, Some(DiffSource::GitBlob { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn working_comparison_uses_a_symlink_payload_without_following_the_target() {
        use super::super::diff::{compare_paged, DiffOptions, DiffSource};
        use std::sync::atomic::AtomicBool;

        let directory = repository();
        let root = directory.path();
        let link = root.join("link");
        std::os::unix::fs::symlink("missing-target", &link).unwrap();
        commit(root, "tracked symlink");
        let comparison = worktree_comparison(&link).unwrap();
        assert!(
            matches!(&comparison.right, Some(DiffSource::Text { text, .. }) if text == "missing-target")
        );
        let result = compare_paged(
            &comparison.left.unwrap(),
            &comparison.right.unwrap(),
            DiffOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.text.unwrap().diff().ratio, 1.0);
    }

    #[test]
    fn working_comparison_labels_verified_rename_and_handles_missing_head_blob() {
        use super::super::diff::DiffSource;

        let directory = repository();
        let root = directory.path();
        let old = root.join("before.txt");
        let new = root.join("renamed č.txt");
        fs::write(&old, "contents\n").unwrap();
        assert!(worktree_comparison(&old).unwrap().left.is_none());
        let head = commit(root, "original name");
        git(root, &["mv", "--", "before.txt", "renamed č.txt"]).unwrap();
        let renamed = worktree_comparison(&new).unwrap();
        assert!(
            matches!(renamed.left, Some(DiffSource::GitBlob { file, commit, .. })
            if file == "before.txt" && commit == head)
        );
        let history = scoped_history(root, &new, 100, Some(&head)).unwrap();
        assert_eq!(history.entries[0].file, "before.txt");
        assert_eq!(renamed.last_touch.unwrap().file, "before.txt");
        let extra = root.join("untracked.txt");
        fs::write(&extra, "extra\n").unwrap();
        let untracked = worktree_comparison(&extra).unwrap();
        assert!(untracked.left.is_none());
        assert!(untracked.right.is_some());
    }

    /// The history of a file starts where its name was last brought in: a rename or a
    /// copy is followed, and an earlier file that had the same name is not part of it.
    #[test]
    fn file_history_follows_renames_and_copies_but_not_an_earlier_file_of_that_name() {
        let directory = repository();
        let root = directory.path();
        let subjects = |file: &str| -> Vec<String> {
            file_history(root, file, 100, None)
                .unwrap()
                .into_iter()
                .map(|entry| entry.subject)
                .collect()
        };
        let body = "one\ntwo\nthree\nfour\nfive\nsix\n";
        fs::write(root.join("b.txt"), "an unrelated earlier file\n").unwrap();
        commit(root, "add old b");
        fs::remove_file(root.join("b.txt")).unwrap();
        commit(root, "delete old b");
        fs::write(root.join("a.txt"), body).unwrap();
        commit(root, "add a");
        fs::write(root.join("a.txt"), format!("{body}seven\n")).unwrap();
        commit(root, "edit a");
        fs::rename(root.join("a.txt"), root.join("b.txt")).unwrap();
        commit(root, "rename a to b");
        fs::write(root.join("b.txt"), format!("{body}seven\neight\n")).unwrap();
        commit(root, "edit b");
        assert_eq!(
            subjects("b.txt"),
            ["edit b", "rename a to b", "edit a", "add a"]
        );
        // A copy made in a commit that also changes the original.
        fs::copy(root.join("b.txt"), root.join("c.txt")).unwrap();
        fs::write(root.join("b.txt"), format!("{body}seven\neight\nnine\n")).unwrap();
        commit(root, "copy b to c");
        assert_eq!(
            subjects("c.txt"),
            ["copy b to c", "edit b", "rename a to b", "edit a", "add a"]
        );
        // The limit still counts across names.
        let git_follow = git(root, &["log", "--follow", "--format=%H", "--", "b.txt"]).unwrap();
        let gall_ids: Vec<_> = file_history(root, "b.txt", 100, None)
            .unwrap()
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(gall_ids, git_follow.lines().collect::<Vec<_>>());
        // Each entry carries its parents (as Git prints them): one for these commits, none for the first one.
        let with_parents = file_history(root, "b.txt", 100, None).unwrap();
        for entry in &with_parents {
            let printed = git(root, &["log", "-1", "--format=%P", &entry.id]).unwrap();
            assert_eq!(
                entry.parents,
                printed.split_whitespace().collect::<Vec<_>>(),
                "{}",
                entry.id
            );
        }
        assert!(with_parents.last().unwrap().parents.is_empty() || with_parents.len() > 1);
        let page = file_history_page(root, "c.txt", 3, None).unwrap();
        assert_eq!(page.entries.len(), 3);
        assert!(page.truncated);
        assert!(!page.incomplete);
        assert_eq!(file_history(root, "c.txt", 3, None).unwrap().len(), 3);
    }
    #[test]
    fn line_history_agrees_with_git() {
        let directory = repository();
        let root = directory.path();
        let write = |name: &str, lines: &[&str]| {
            fs::write(root.join(name), lines.join("\n") + "\n").unwrap();
        };
        write("old name.txt", &["a", "b", "c", "d", "e", "f", "g", "h"]);
        commit(root, "add");
        write("old name.txt", &["a", "b", "C", "d", "e", "f", "g", "h"]);
        commit(root, "change c");
        write("old name.txt", &["a", "b", "C", "d", "e", "f", "G", "h"]);
        commit(root, "change g");
        fs::rename(root.join("old name.txt"), root.join("new: ž.txt")).unwrap_or_else(|_| {
            // Windows has no `:` in names.
            fs::rename(root.join("old name.txt"), root.join("new ž.txt")).unwrap()
        });
        let name = ["new: ž.txt", "new ž.txt"]
            .into_iter()
            .find(|name| root.join(name).exists())
            .unwrap();
        commit(root, "rename");
        // The separator of the records as text of the file.
        write(name, &["a", "b", "C \x1e", "d", "e", "f", "G", "h"]);
        commit(root, "separator");
        let head = git(root, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        let read = |range, limit| line_history(root, &head, name, range, limit, &|| false);
        for range in [(2, 4), (7, 7), (1, 8)] {
            let expected: Vec<String> = git(
                root,
                &[
                    "log",
                    "--format=%s",
                    "--no-patch",
                    &format!("-L{},{}:{name}", range.0, range.1),
                ],
            )
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
            let found = read(range, 100).unwrap();
            let subjects: Vec<_> = found.entries.iter().map(|e| e.subject.clone()).collect();
            assert_eq!(subjects, expected, "{range:?}");
            assert!(!found.truncated);
        }
        let found = read((3, 3), 100).unwrap();
        assert!(found.entries[0].patch.contains("+C \x1e"));
        assert!(found.entries[0].patch.contains("-C\n"));
        // The limit says that there is more.
        let first = read((1, 8), 1).unwrap();
        assert!(first.truncated && first.entries.len() == 1);
        // Refused and cancelled.
        for range in [(0, 1), (3, 2), (900, 901)] {
            assert!(read(range, 10).is_err(), "{range:?}");
        }
        assert!(line_history(root, &head, "../x", (1, 1), 10, &|| false).is_err());
        assert!(line_history(root, &head, name, (1, 8), 10, &|| true).is_err());
        // The scrubber asks for each revision under the name it had.
        let names: Vec<_> = file_history(root, name, 100, None)
            .unwrap()
            .into_iter()
            .map(|entry| content(root, &entry.id, &entry.file).is_ok())
            .collect();
        assert_eq!(names, [true; 5]);
    }
    use std::fs;

    #[derive(Debug)]
    struct BlameLine {
        commit: String,
        line: u64,
        original_line: u64,
        author: String,
        date: String,
        text: String,
    }

    /// The streamed blame as lines with their text.
    fn blame(path: &Path, commit: &str, file: &str) -> Result<Vec<BlameLine>> {
        let mut commits = HashMap::new();
        let mut ranges = Vec::new();
        super::super::gitblame::blame(path, Some(commit), file, &|| false, |part| {
            commits.extend(part.commits.into_iter().map(|c| (c.id.clone(), c)));
            ranges.extend(part.ranges);
        })?;
        let text = content(path, commit, file)?;
        let mut lines: Vec<BlameLine> = ranges
            .into_iter()
            .flat_map(|range| {
                (0..range.count)
                    .map(move |i| (range.commit.clone(), range.line + i, range.original + i))
            })
            .map(|(commit, line, original)| BlameLine {
                line: line.into(),
                original_line: original.into(),
                author: commits[&commit].author.clone(),
                date: chrono::DateTime::from_timestamp(commits[&commit].time, 0)
                    .unwrap()
                    .to_rfc3339(),
                text: text
                    .lines()
                    .nth(line as usize - 1)
                    .unwrap_or_default()
                    .into(),
                commit,
            })
            .collect();
        lines.sort_by_key(|line| line.line);
        Ok(lines)
    }

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-q"]).unwrap();
        git(root, &["config", "user.name", "Test User"]).unwrap();
        git(root, &["config", "user.email", "test@example.com"]).unwrap();
        git(root, &["config", "core.autocrlf", "false"]).unwrap();
        // An empty file rather than the null device: Git on Windows rejects `NUL` here.
        let excludes = root.join(".git").join("empty-excludes");
        fs::write(&excludes, "").unwrap();
        git(
            root,
            &[
                "config",
                "core.excludesFile",
                &excludes.to_string_lossy().replace('\\', "/"),
            ],
        )
        .unwrap();
        directory
    }

    fn commit(root: &Path, message: &str) -> String {
        git(root, &["add", "--all"]).unwrap();
        git(root, &["commit", "-qm", message]).unwrap();
        git(root, &["rev-parse", "HEAD"]).unwrap().trim().into()
    }

    #[test]
    fn whole_commit_diff_covers_every_file_and_root_commits() {
        let directory = repository();
        let root = directory.path();
        fs::write(root.join("a.txt"), "one\n").unwrap();
        let first = commit(root, "First");
        let root_diff = commit_diff(root, &first).unwrap();
        assert!(root_diff.contains("diff --git a/a.txt b/a.txt") && root_diff.contains("+one"));
        fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        fs::write(root.join("b.txt"), "bee\n").unwrap();
        let second = commit(root, "Second");
        let diff = commit_diff(root, &second).unwrap();
        assert!(diff.contains("diff --git a/a.txt b/a.txt") && diff.contains("+two"));
        assert!(diff.contains("diff --git a/b.txt b/b.txt") && diff.contains("+bee"));
        assert!(
            !diff.contains("+one\n+two"),
            "only the change against the parent"
        );
        assert!(commit_diff(root, "--output=x").is_err());
    }

    #[test]
    fn root_rename_delete_binary_and_literal_paths_are_read_only() {
        let directory = repository();
        let root = directory.path();
        fs::write(root.join("before.txt"), "first\nsecond\n").unwrap();
        fs::write(root.join("[literal].txt"), "literal\n").unwrap();
        fs::write(root.join("remove.txt"), "remove\n").unwrap();
        let first = commit(root, "First commit\n\nA complete message");
        let initial = details(root, &first, None).unwrap();
        assert!(initial.parents.is_empty());
        assert_eq!(initial.base, None);
        assert_eq!(initial.message, "First commit\n\nA complete message");
        assert_eq!(initial.author, "Test User");
        assert_eq!(initial.email, "test@example.com");
        assert_eq!(initial.files.len(), 3);
        assert!(initial.files.iter().all(|entry| entry.status == "A"));
        assert!(patch(root, &first, None, "[literal].txt")
            .unwrap()
            .contains("+literal"));
        git(root, &["mv", "before.txt", "after.txt"]).unwrap();
        fs::write(root.join("[literal].txt"), "changed literal\n").unwrap();
        fs::remove_file(root.join("remove.txt")).unwrap();
        fs::write(root.join("binary.bin"), [1, 0, 2]).unwrap();
        let second = commit(root, "Rename and changes");
        fs::write(root.join("[literal].txt"), "Uncommitted working content\n").unwrap();
        let before_status = git(root, &["status", "--porcelain=v1", "-z"]).unwrap();
        let before_time = fs::metadata(root.join("[literal].txt"))
            .unwrap()
            .modified()
            .unwrap();
        let result = details(root, &second, None).unwrap();
        assert_eq!(result.parents.as_slice(), std::slice::from_ref(&first));
        assert_eq!(result.base, Some(first));
        let rename = result
            .files
            .iter()
            .find(|entry| entry.path == "after.txt")
            .unwrap();
        assert_eq!(rename.original.as_deref(), Some("before.txt"));
        assert_eq!(rename.status, "R100");
        assert_eq!(rename.added, Some(0));
        assert_eq!(rename.deleted, Some(0));
        let binary = result
            .files
            .iter()
            .find(|entry| entry.path == "binary.bin")
            .unwrap();
        assert_eq!((binary.added, binary.deleted), (None, None));
        assert!(patch(root, &second, None, "after.txt")
            .unwrap()
            .contains("rename from before.txt"));
        assert!(patch(root, &second, None, "remove.txt")
            .unwrap()
            .contains("-remove"));
        assert!(patch(root, &second, None, "[literal].txt")
            .unwrap()
            .contains("+changed literal"));
        assert!(patch(root, &second, None, "binary.bin")
            .unwrap()
            .contains("Binary files"));
        assert!(patch(root, &second, None, "before.txt").is_err());
        assert_eq!(
            git(root, &["status", "--porcelain=v1", "-z"]).unwrap(),
            before_status
        );
        assert_eq!(
            fs::metadata(root.join("[literal].txt"))
                .unwrap()
                .modified()
                .unwrap(),
            before_time
        );
        assert_eq!(
            fs::read_to_string(root.join("[literal].txt")).unwrap(),
            "Uncommitted working content\n"
        );
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), second);
    }

    #[test]
    fn merge_parents_and_arbitrary_commit_comparison() {
        let directory = repository();
        let root = directory.path();
        fs::write(root.join("shared.txt"), "base\n").unwrap();
        let first = commit(root, "Base");
        let main = git(root, &["symbolic-ref", "--short", "HEAD"])
            .unwrap()
            .trim()
            .to_owned();
        git(root, &["checkout", "-qb", "topic"]).unwrap();
        fs::write(root.join("topic.txt"), "topic\n").unwrap();
        let topic = commit(root, "Topic");
        git(root, &["checkout", "-q", &main]).unwrap();
        fs::write(root.join("main.txt"), "main\n").unwrap();
        let main_commit = commit(root, "Main");
        git(root, &["merge", "--no-ff", "topic", "-m", "Merge topic"]).unwrap();
        let merged = git(root, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        let result = details(root, &merged, None).unwrap();
        assert_eq!(result.parents, [main_commit.clone(), topic.clone()]);
        assert_eq!(
            result
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["topic.txt"]
        );
        let second_parent = details(root, &merged, Some(&topic)).unwrap();
        assert_eq!(
            second_parent
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["main.txt"]
        );
        let range = details(root, &merged, Some(&first)).unwrap();
        assert_eq!(range.files.len(), 2);
        assert!(patch(root, &merged, Some(&first), "main.txt")
            .unwrap()
            .contains("+main"));
        let reverse = details(root, &first, Some(&merged)).unwrap();
        assert!(reverse.files.iter().all(|file| file.status == "D"));
    }

    #[test]
    fn history_follows_renames_and_blame_tracks_original_lines() {
        let directory = repository();
        let root = directory.path();
        fs::write(root.join("before.txt"), "first\nsecond\n").unwrap();
        let first = commit(root, "Original");
        assert_eq!(
            content(root, &first, "before.txt").unwrap(),
            "first\nsecond\n"
        );
        git(root, &["mv", "before.txt", "after.txt"]).unwrap();
        let renamed = commit(root, "Renamed");
        fs::write(root.join("after.txt"), "first\nchanged\n").unwrap();
        let last = commit(root, "Edited");
        let history = file_history(root, "after.txt", 100, Some(&last)).unwrap();
        assert_eq!(
            history
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            [&last, &renamed, &first]
        );
        assert_eq!(file_history(root, "after.txt", 1, None).unwrap().len(), 1);
        let lines = blame(root, &last, "after.txt").unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].commit, first);
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[0].original_line, 1);
        assert_eq!(lines[1].commit, last);
        assert_eq!(lines[1].line, 2);
        assert_eq!(lines[1].text, "changed");
        assert_eq!(lines[0].author, "Test User");
        assert!(lines[0].date.contains('T'));
        fs::write(root.join("binary.bin"), [0, 1, 2]).unwrap();
        let binary = commit(root, "Binary");
        assert!(content(root, &binary, "binary.bin").is_err());
        assert!(blame(root, &binary, "binary.bin")
            .unwrap_err()
            .to_string()
            .contains("binary"));
    }

    #[test]
    fn rejects_revision_options_tree_objects_and_outside_paths() {
        let directory = repository();
        let root = directory.path();
        fs::write(root.join("file.txt"), "content\n").unwrap();
        let first = commit(root, "Initial");
        assert!(details(root, "--all", None).is_err());
        assert!(details(root, "HEAD", None).is_err());
        assert!(details(root, &first, Some("--output=unwanted")).is_err());
        let tree = git(root, &["rev-parse", "HEAD^{tree}"]).unwrap();
        assert!(details(root, tree.trim(), None).is_err());
        for file in [
            "../file.txt",
            "/file.txt",
            "a/../../file.txt",
            "file.txt\0other",
            "./file.txt",
        ] {
            assert!(patch(root, &first, None, file).is_err());
            assert!(blame(root, &first, file).is_err());
            assert!(file_history(root, file, 100, None).is_err());
        }
        assert!(patch(root, &first, None, "not-changed.txt").is_err());
    }

    #[test]
    fn history_and_content_include_a_detached_commit_without_touching_head() {
        let directory = repository();
        let root = directory.path();
        let file = "[file] č.txt";
        fs::write(root.join(file), "initial\n").unwrap();
        let first = commit(root, "Initial");
        git(root, &["checkout", "--detach", "-q"]).unwrap();
        fs::write(root.join(file), "detached\n").unwrap();
        let detached = commit(root, "Detached only");
        git(root, &["checkout", "--detach", "-q", &first]).unwrap();
        let history = file_history(root, file, 100, Some(&detached)).unwrap();
        assert_eq!(history[0].id, detached);
        assert_eq!(content(root, &detached, file).unwrap(), "detached\n");
        assert_eq!(blame(root, &detached, file).unwrap()[0].commit, detached);
        assert!(patch(root, &detached, None, file)
            .unwrap()
            .contains("+detached"));
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), first);
        assert_eq!(fs::read_to_string(root.join(file)).unwrap(), "initial\n");
    }

    #[test]
    #[cfg(not(windows))]
    fn nul_delimited_paths_preserve_tabs_newlines_and_colons() {
        let directory = repository();
        let root = directory.path();
        let old = "colon:tab\told\nč.txt";
        let new = "colon:tab\tnew\nč.txt";
        fs::write(root.join(old), "first\nsecond\n").unwrap();
        commit(root, "Original strange path");
        git(root, &["mv", "--", old, new]).unwrap();
        let last = commit(root, "Rename strange path");
        let metadata = details(root, &last, None).unwrap();
        assert_eq!(metadata.files[0].path, new);
        assert_eq!(metadata.files[0].original.as_deref(), Some(old));
        assert_eq!(metadata.files[0].added, Some(0));
        assert!(patch(root, &last, None, new).unwrap().contains("rename"));
        assert_eq!(content(root, &last, new).unwrap(), "first\nsecond\n");
        assert_eq!(file_history(root, new, 100, Some(&last)).unwrap().len(), 2);
        assert_eq!(blame(root, &last, new).unwrap().len(), 2);
    }

    #[test]
    fn previews_large_diffs_without_unbounded_output() {
        let directory = repository();
        let root = directory.path();
        fs::write(root.join("large.txt"), "first\n").unwrap();
        commit(root, "Initial");
        fs::write(
            root.join("large.txt"),
            "a long changed line\n".repeat(90000),
        )
        .unwrap();
        let last = commit(root, "Large");
        let preview = patch(root, &last, None, "large.txt").unwrap();
        assert!(preview.len() < PATCH_LIMIT + 100);
        assert!(preview.contains("[Diff preview truncated at 1 MiB]"));
        assert!(blame(root, &last, "large.txt")
            .unwrap_err()
            .to_string()
            .contains("1 MiB"));
    }

    #[test]
    fn the_stages_of_an_unresolved_file_are_read_as_bytes_and_a_missing_stage_is_none() {
        use super::super::gitgraph::git;
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
        let write = |name: &str, bytes: &[u8]| std::fs::write(p.join(name), bytes).unwrap();
        write("f.txt", b"base\n");
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "base"]).unwrap();
        git(p, &["switch", "-qc", "side"]).unwrap();
        write("f.txt", b"theirs\r\n");
        write("both.txt", b"side add\n");
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "side"]).unwrap();
        git(p, &["switch", "-q", "main"]).unwrap();
        write("f.txt", b"ours\n");
        write("both.txt", b"main add\n");
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "main"]).unwrap();
        assert!(git(p, &["merge", "side"]).is_err());
        let stage = |file: &str, n: u8| conflict_stage(p, file, n, 1024).unwrap();
        // Modified on both sides: all three stages, with the bytes as they are (CRLF kept).
        assert_eq!(stage("f.txt", 1).as_deref(), Some(b"base\n".as_slice()));
        assert_eq!(stage("f.txt", 2).as_deref(), Some(b"ours\n".as_slice()));
        assert_eq!(stage("f.txt", 3).as_deref(), Some(b"theirs\r\n".as_slice()));
        // Added on both sides: no common ancestor.
        assert_eq!(stage("both.txt", 1), None);
        assert_eq!(
            stage("both.txt", 2).as_deref(),
            Some(b"main add\n".as_slice())
        );
        // A file that is not in conflict has no stages; bad arguments are refused.
        assert_eq!(stage("nothing.txt", 2), None);
        assert!(conflict_stage(p, "f.txt", 4, 1024).is_err());
        assert!(conflict_stage(p, "../f.txt", 2, 1024).is_err());
        // A stage larger than the limit comes back one byte over it.
        let big = conflict_stage(p, "f.txt", 3, 4).unwrap().unwrap();
        assert_eq!(big.len(), 5);
    }

    #[test]
    fn a_merge_shows_its_change_against_the_chosen_parent_and_says_when_there_is_none() {
        // What the revision scrubber asks for: the patch of a file at a merge against each parent in turn.
        let directory = repository();
        let root = directory.path();
        fs::write(root.join("main.txt"), "m1\n").unwrap();
        fs::write(root.join("side.txt"), "s1\n").unwrap();
        let base = commit(root, "Base");
        git(root, &["checkout", "-q", "-b", "topic"]).unwrap();
        fs::write(root.join("side.txt"), "s1\ns2 from topic\n").unwrap();
        let topic = commit(root, "Topic changes side");
        git(root, &["checkout", "-q", "-"]).unwrap();
        fs::write(root.join("main.txt"), "m1\nm2 on main\n").unwrap();
        let main = commit(root, "Main changes main");
        git(root, &["merge", "--no-ff", "topic", "-m", "Merge topic"]).unwrap();
        let merged = git(root, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        let parents = details(root, &merged, None).unwrap().parents;
        assert_eq!(parents, [main.clone(), topic.clone()]);
        // Against the first parent the merge brought in the topic's change to side.txt …
        let against_first = patch(root, &merged, Some(&main), "side.txt").unwrap();
        assert!(against_first.contains("+s2 from topic"), "{against_first}");
        // … against the second parent side.txt is the same file, so it is not in that comparison: an answer, not a failure.
        let err = patch(root, &merged, Some(&topic), "side.txt")
            .unwrap_err()
            .to_string();
        assert!(err.starts_with(&format!("{NOT_IN_COMPARISON}:")), "{err}");
        // main.txt is the other way round.
        assert!(patch(root, &merged, Some(&topic), "main.txt")
            .unwrap()
            .contains("+m2 on main"));
        assert!(patch(root, &merged, Some(&main), "main.txt").is_err());
        // A first commit has no parent: it is shown against the empty tree.
        assert!(details(root, &base, None).unwrap().parents.is_empty());
        assert!(patch(root, &base, None, "main.txt")
            .unwrap()
            .contains("+m1"));
    }
}
