//! Read-only inspection of commits, changed files and line provenance.
use super::gitgraph::root;
use crate::{CoreError, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

const METADATA_LIMIT: usize = 8 * 1024 * 1024;
const PATCH_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub original: Option<String>,
    pub status: String,
    pub added: Option<u64>,
    pub deleted: Option<u64>,
}

#[derive(Debug, Serialize)]
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameLine {
    pub commit: String,
    pub line: u64,
    pub original_line: u64,
    pub author: String,
    pub date: String,
    pub text: String,
}

fn error(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// Limit output before allocating a large preview. No subprocess runs an external
/// diff, text converter, pager or editor, and no optional index refresh is needed.
fn read_git(root: &Path, args: &[&str], limit: usize) -> Result<(Vec<u8>, bool)> {
    let mut command = Command::new(super::gitstatus::program());
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
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
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
    let result = child
        .stdout
        .take()
        .ok_or_else(|| error("Missing Git stdout"))?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes);
    let truncated = bytes.len() > limit;
    if truncated || result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| error(e.to_string()))?;
    let stderr = error_reader.join().unwrap_or_default();
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

fn commit_id(root: &Path, value: &str) -> Result<String> {
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

fn relative_file(file: &str) -> Result<()> {
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
    let names = text(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-status",
            "-z",
            "-M",
            base,
            commit,
            "--",
        ],
    )?;
    let stats = text(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--numstat",
            "-z",
            "-M",
            base,
            commit,
            "--",
        ],
    )?;
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
    let id = commit_id(&root, commit)?;
    let metadata = text(
        &root,
        &[
            "show",
            "--no-show-signature",
            "-s",
            "--format=%H%x00%P%x00%an%x00%ae%x00%aI%x00%cn%x00%cI%x00%B",
            &id,
            "--",
        ],
    )?;
    let mut fields = metadata.splitn(8, '\0');
    fields.next();
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
    let empty = if base.is_none() {
        text(&root, &["hash-object", "-t", "tree", "--stdin"])?
            .trim()
            .to_owned()
    } else {
        String::new()
    };
    let files = changed_files(&root, base.as_deref().unwrap_or(&empty), &id)?;
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
    let empty = if details.base.is_none() {
        text(root, &["hash-object", "-t", "tree", "--stdin"])?
            .trim()
            .to_owned()
    } else {
        String::new()
    };
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "-M",
        details.base.as_deref().unwrap_or(&empty),
        &details.id,
    ];
    let (bytes, truncated) = read_git(root, &args, PATCH_LIMIT)?;
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        output.push_str("\n[Diff truncated at 1 MiB]\n");
    }
    Ok(output)
}

/// Preview only a path known to be in this comparison, preserving rename context.
pub fn patch(path: &Path, commit: &str, base: Option<&str>, file: &str) -> Result<String> {
    relative_file(file)?;
    let details = details(path, commit, base)?;
    let entry = details
        .files
        .iter()
        .find(|entry| entry.path == file)
        .ok_or_else(|| error("File is not in the selected comparison"))?;
    let root = Path::new(&details.root);
    let empty = if details.base.is_none() {
        text(root, &["hash-object", "-t", "tree", "--stdin"])?
            .trim()
            .to_owned()
    } else {
        String::new()
    };
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
        details.base.as_deref().unwrap_or(&empty),
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

pub fn file_history(
    path: &Path,
    file: &str,
    limit: usize,
    commit: Option<&str>,
) -> Result<Vec<HistoryEntry>> {
    relative_file(file)?;
    let root = root(path)?;
    let limit = format!("-{}", limit.clamp(1, 1000));
    let spec = format!(":(literal){file}");
    let start = commit.map(|value| commit_id(&root, value)).transpose()?;
    let output = text(
        &root,
        &[
            "log",
            "--no-show-signature",
            "--follow",
            &limit,
            "--format=%H%x00%s%x00%an%x00%aI%x00",
            start.as_deref().unwrap_or("HEAD"),
            "--",
            &spec,
        ],
    )?;
    let mut fields = output.split('\0');
    let mut entries = Vec::new();
    while let Some(id) = fields.next() {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
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
            id: id.into(),
            subject: subject.into(),
            author: author.into(),
            date: date.into(),
        });
    }
    Ok(entries)
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

/// Line porcelain repeats provenance per source line and handles tabs in content.
pub fn blame(path: &Path, commit: &str, file: &str) -> Result<Vec<BlameLine>> {
    relative_file(file)?;
    let root = root(path)?;
    let commit = commit_id(&root, commit)?;
    // Reject binary content and large blobs before asking Git to attribute lines.
    let spec = format!("{commit}:{file}");
    let (content, truncated) = read_git(&root, &["cat-file", "blob", &spec], PATCH_LIMIT)?;
    if truncated {
        return Err(error("Blame preview is limited to files up to 1 MiB"));
    }
    if content.contains(&0) {
        return Err(error("Blame is not available for binary files"));
    }
    let output = text(
        &root,
        &[
            "blame",
            "--line-porcelain",
            "--no-textconv",
            &commit,
            "--",
            file,
        ],
    )?;
    let mut entries = Vec::new();
    let mut lines = output.lines();
    while let Some(header) = lines.next() {
        let mut fields = header.split_whitespace();
        let commit = fields
            .next()
            .ok_or_else(|| error("Invalid blame record"))?
            .to_owned();
        let original_line = fields
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| error("Invalid blame line"))?;
        let line = fields
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| error("Invalid blame line"))?;
        let mut author = String::new();
        let mut date = String::new();
        for metadata in lines.by_ref() {
            if let Some(value) = metadata.strip_prefix("author ") {
                author = value.into();
            } else if let Some(value) = metadata.strip_prefix("author-time ") {
                date = value
                    .parse::<i64>()
                    .ok()
                    .and_then(|n| chrono::DateTime::from_timestamp(n, 0))
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default();
            } else if let Some(value) = metadata.strip_prefix('\t') {
                entries.push(BlameLine {
                    commit: commit.clone(),
                    line,
                    original_line,
                    author,
                    date,
                    text: value.into(),
                });
                break;
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::super::gitgraph::git;
    use super::*;
    use std::fs;

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
}
