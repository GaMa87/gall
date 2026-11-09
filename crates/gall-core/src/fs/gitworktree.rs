//! Commit preparation with stale-state checks and recoverable file discard.
use super::gitgraph::{git, has_content_filters, root};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub path: String,
    pub original: Option<String>,
    pub index: char,
    pub worktree: char,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub root: String,
    pub edit_stop: bool,
    pub changes: Vec<Change>,
    pub token: String,
    pub head: String,
    pub subject: String,
    pub message: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexAction {
    Stage,
    Unstage,
    Commit,
    StageAll,
    UnstageAll,
    Amend,
    Discard,
    Ignore,
    Exclude,
}

fn checked_root(path: &Path) -> Result<PathBuf> {
    let p = root(path)?;
    if has_content_filters(&p) {
        return Err(CoreError::Other(
            "Index operations with content filters are not supported yet".into(),
        ));
    }
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "BISECT_START",
    ] {
        let value = git(&p, &["rev-parse", "--git-path", marker])?;
        if p.join(value.trim()).exists() {
            if marker == "rebase-merge" && super::gitrewrite::is_edit_stop(&p) {
                continue;
            }
            return Err(CoreError::Other(
                "Finish or abort the current Git operation first".into(),
            ));
        }
    }
    Ok(p)
}
fn head(path: &Path) -> String {
    git(path, &["rev-parse", "--verify", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_owned()
}
fn token(path: &Path) -> Result<String> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    head(path).hash(&mut hash);
    git(path, &["ls-files", "--stage", "-z"])?.hash(&mut hash);
    for change in changes(path)? {
        change.path.hash(&mut hash);
        change.index.hash(&mut hash);
        change.worktree.hash(&mut hash);
        let file = path.join(&change.path);
        match file.symlink_metadata() {
            Ok(metadata) if metadata.is_symlink() => {
                std::fs::read_link(&file)
                    .map_err(|e| CoreError::Other(e.to_string()))?
                    .hash(&mut hash);
            }
            Ok(metadata) if metadata.is_file() => {
                let mut input =
                    super::open_regular(&file).map_err(|e| CoreError::Other(e.to_string()))?;
                let mut buffer = [0u8; 65536];
                loop {
                    let n = input
                        .read(&mut buffer)
                        .map_err(|e| CoreError::Other(e.to_string()))?;
                    if n == 0 {
                        break;
                    }
                    hash.write(&buffer[..n]);
                }
                metadata.len().hash(&mut hash);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode().hash(&mut hash);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing".hash(&mut hash),
            Ok(_) => {
                return Err(CoreError::Other(
                    "A changed path is not a regular file or symlink".into(),
                ))
            }
            Err(e) => return Err(CoreError::Other(e.to_string())),
        }
    }
    Ok(format!("{:016x}", hash.finish()))
}
fn verify_token(path: &Path, expected: &str) -> Result<()> {
    if token(path)? != expected {
        return Err(CoreError::Other(
            "The index, HEAD or working files changed. Refresh and review the changes again".into(),
        ));
    }
    Ok(())
}
fn changes(path: &Path) -> Result<Vec<Change>> {
    let raw = git(
        path,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
    )?;
    if raw.contains('\u{fffd}') {
        return Err(CoreError::Other(
            "Git paths with invalid UTF-8 are not supported".into(),
        ));
    }
    let mut fields = raw.split('\0').filter(|v| !v.is_empty());
    let mut out = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            return Err(CoreError::Other("Invalid Git status record".into()));
        }
        let index = field.as_bytes()[0] as char;
        let worktree = field.as_bytes()[1] as char;
        let original = if index == 'R' || index == 'C' || worktree == 'R' || worktree == 'C' {
            fields.next().map(str::to_owned)
        } else {
            None
        };
        out.push(Change {
            path: field[3..].into(),
            original,
            index,
            worktree,
        });
    }
    Ok(out)
}
pub fn workspace(path: &Path) -> Result<Workspace> {
    let root = checked_root(path)?;
    Ok(Workspace {
        root: root.to_string_lossy().into_owned(),
        edit_stop: super::gitrewrite::is_edit_stop(&root),
        changes: changes(&root)?,
        token: token(&root)?,
        head: head(&root),
        subject: git(&root, &["log", "-1", "--format=%s"])
            .unwrap_or_default()
            .trim_end()
            .to_owned(),
        message: git(&root, &["log", "-1", "--format=%B"])
            .unwrap_or_default()
            .trim_end()
            .to_owned(),
    })
}
pub fn index_action(
    path: &Path,
    action: IndexAction,
    file: Option<&str>,
    message: &str,
    expected: &str,
) -> Result<()> {
    let root = checked_root(path)?;
    verify_token(&root, expected)?;
    if matches!(action, IndexAction::Commit) && super::gitrewrite::is_edit_stop(&root) {
        return Err(CoreError::Other(
            "Use Amend at this interactive edit stop, then Continue the rebase".into(),
        ));
    }
    match action {
        IndexAction::Commit | IndexAction::Amend => {
            if message.trim().is_empty() {
                return Err(CoreError::Other("A commit message is required".into()));
            }
            if matches!(action, IndexAction::Commit)
                && git(&root, &["diff", "--cached", "--name-only", "--no-ext-diff"])?.is_empty()
            {
                return Err(CoreError::Other("Stage changes before committing".into()));
            }
            if matches!(action, IndexAction::Amend) {
                let previous = head(&root);
                if previous.is_empty() {
                    return Err(CoreError::Other("There is no commit to amend".into()));
                }
                git(
                    &root,
                    &[
                        "update-ref",
                        "--no-deref",
                        &format!("refs/gall/recovery/{previous}"),
                        &previous,
                    ],
                )?;
                git(&root, &["commit", "--amend", "-m", message])?;
            } else {
                git(&root, &["commit", "-m", message])?;
            }
            super::gitgraph::retain_detached(&root).map_err(|e| {
                CoreError::Other(format!(
                    "Commit created, but its recovery reference could not be saved: {e}"
                ))
            })?;
        }
        IndexAction::StageAll => {
            // Submodule pointers are not listed in the changes, so they are not staged blindly.
            let mut args = vec!["add".to_owned(), "--all".into(), "--".into(), ".".into()];
            for line in git(&root, &["ls-files", "--stage", "-z"])?.split('\0') {
                if let Some((_, name)) = line
                    .strip_prefix("160000 ")
                    .and_then(|rest| rest.split_once('\t'))
                {
                    args.push(format!(":(exclude,literal){name}"));
                }
            }
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            git(&root, &args)?;
        }
        IndexAction::UnstageAll => {
            if head(&root).is_empty() {
                git(&root, &["rm", "--cached", "-r", "--", "."])?;
            } else {
                git(&root, &["restore", "--staged", "--", "."])?;
            }
        }
        IndexAction::Discard | IndexAction::Ignore | IndexAction::Exclude => {
            let list = changes(&root)?;
            let entry = list
                .iter()
                .find(|entry| Some(entry.path.as_str()) == file)
                .ok_or_else(|| {
                    CoreError::Other("This file is no longer in the change list".into())
                })?;
            if matches!(action, IndexAction::Discard) {
                discard(&root, entry, expected)?;
            } else {
                if entry.index != '?' {
                    return Err(CoreError::Other(
                        "Only untracked files can be ignored without changing tracked history"
                            .into(),
                    ));
                }
                ignore(&root, &entry.path, matches!(action, IndexAction::Exclude))?;
            }
        }
        IndexAction::Stage | IndexAction::Unstage => {
            let list = changes(&root)?;
            let entry = list
                .iter()
                .find(|e| Some(e.path.as_str()) == file)
                .ok_or_else(|| {
                    CoreError::Other("This file is no longer in the change list".into())
                })?;
            let paths: Vec<String> = std::iter::once(entry.path.as_str())
                .chain(if matches!(action, IndexAction::Unstage) {
                    entry.original.as_deref()
                } else {
                    None
                })
                .map(|p| format!(":(literal){p}"))
                .collect();
            let unborn = head(&root).is_empty();
            let mut args = match action {
                IndexAction::Stage => vec!["add", "--all", "--"],
                _ if unborn => vec!["rm", "--cached", "--"],
                _ => vec!["restore", "--staged", "--"],
            };
            args.extend(paths.iter().map(String::as_str));
            git(&root, &args)?;
        }
    }
    Ok(())
}
fn discard(path: &Path, entry: &Change, expected: &str) -> Result<()> {
    let spec = format!(":(literal){}", entry.path);
    if entry.index == '?' {
        git(
            path,
            &[
                "stash",
                "push",
                "--include-untracked",
                "-m",
                "Gall recovery before file discard",
                "--",
                &spec,
            ],
        )?;
        return Ok(());
    }
    if entry.worktree == ' ' {
        return Err(CoreError::Other(
            "Unstage the file before discarding staged changes".into(),
        ));
    }
    let saved = git(
        path,
        &["stash", "create", "Gall recovery before file discard"],
    )?;
    if saved.trim().is_empty() {
        return Err(CoreError::Other(
            "Could not preserve the file before discarding".into(),
        ));
    }
    git(
        path,
        &[
            "stash",
            "store",
            "-m",
            "Gall recovery before file discard",
            saved.trim(),
        ],
    )?;
    verify_token(path, expected)?;
    git(path, &["restore", "--worktree", "--", &spec])?;
    Ok(())
}
fn ignore(path: &Path, file: &str, local: bool) -> Result<()> {
    if file.contains(['\n', '\r']) {
        return Err(CoreError::Other(
            "Filenames with line breaks cannot be written as ignore rules".into(),
        ));
    }
    let mut rule = String::from("/");
    for c in file.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']' | '!' | '#' | ' ') {
            rule.push('\\');
        }
        rule.push(c);
    }
    let target = if local {
        let common = git(path, &["rev-parse", "--git-common-dir"])?;
        let common = path
            .join(common.trim())
            .canonicalize()
            .map_err(|e| CoreError::Other(e.to_string()))?;
        let info = common.join("info");
        if info
            .symlink_metadata()
            .is_ok_and(|m| !m.is_dir() || m.is_symlink())
        {
            return Err(CoreError::Other(
                "Refusing to append local excludes through a symlinked metadata directory".into(),
            ));
        }
        info.join("exclude")
    } else {
        path.join(".gitignore")
    };
    if target
        .symlink_metadata()
        .is_ok_and(|m| m.is_symlink() || !m.is_file())
    {
        return Err(CoreError::Other(
            "Refusing to append ignore rules through a symlink".into(),
        ));
    }
    let content = match std::fs::read_to_string(&target) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(CoreError::Other(e.to_string())),
    };
    if content.lines().any(|line| line == rule) {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::Other(e.to_string()))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut output = options
        .open(&target)
        .map_err(|e| CoreError::Other(e.to_string()))?;
    // Always delimit an appended rule, including if another writer just appended a partial line.
    let suffix = format!("\n{rule}\n");
    output
        .write_all(suffix.as_bytes())
        .map_err(|e| CoreError::Other(e.to_string()))
}
pub fn patch(path: &Path, file: &str, staged: bool) -> Result<String> {
    let root = checked_root(path)?;
    let list = changes(&root)?;
    let entry = list
        .iter()
        .find(|e| e.path == file)
        .ok_or_else(|| CoreError::Other("This file is no longer in the change list".into()))?;
    if entry.index == '?' {
        use std::io::Read;
        let p = root.join(file);
        if p.symlink_metadata()
            .map_err(|e| CoreError::Other(e.to_string()))?
            .is_symlink()
        {
            return Ok(std::fs::read_link(p)
                .map_err(|e| CoreError::Other(e.to_string()))?
                .to_string_lossy()
                .into_owned());
        }
        let mut bytes = Vec::new();
        super::open_regular(&p)
            .map_err(|e| CoreError::Other(e.to_string()))?
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|e| CoreError::Other(e.to_string()))?;
        if bytes.contains(&0) {
            return Ok("Binary file (preview unavailable)".into());
        }
        let truncated = bytes.len() > 65536;
        bytes.truncate(65536);
        return Ok(format!(
            "{}{}",
            String::from_utf8_lossy(&bytes),
            if truncated {
                "\n[Preview truncated]"
            } else {
                ""
            }
        ));
    }
    let spec = format!(":(literal){file}");
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", &spec]);
    let original = entry.original.as_ref().map(|p| format!(":(literal){p}"));
    if staged {
        if let Some(original) = &original {
            args.push(original);
        }
    }
    git(&root, &args)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchLine {
    pub index: usize,
    pub text: String,
    pub kind: String,
    pub hunk: Option<usize>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionPatch {
    pub text: String,
    pub token: String,
    pub selectable: bool,
    pub reason: Option<String>,
    pub lines: Vec<PatchLine>,
}
fn patch_token(path: &Path, file: &str, staged: bool, text: &str) -> Result<String> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    token(path)?.hash(&mut hash);
    file.hash(&mut hash);
    staged.hash(&mut hash);
    text.hash(&mut hash);
    Ok(format!("{:016x}", hash.finish()))
}
pub fn selection_patch(path: &Path, file: &str, staged: bool) -> Result<SelectionPatch> {
    let root = checked_root(path)?;
    let before = token(&root)?;
    let text = patch(&root, file, staged)?;
    let entry = changes(&root)?
        .into_iter()
        .find(|e| e.path == file)
        .ok_or_else(|| CoreError::Other("The selected file changed. Refresh the changes".into()))?;
    verify_token(&root, &before)?;
    let unsupported = entry.index == '?'
        || entry.original.is_some()
        || text.len() > 2_000_000
        || text.lines().any(|l| {
            l.starts_with("new file mode ")
                || l.starts_with("deleted file mode ")
                || l.starts_with("old mode ")
                || l.starts_with("new mode ")
                || l.starts_with("Binary files ")
                || l.starts_with("GIT binary patch")
                || l.starts_with("\\ No newline at end of file")
        })
        || !text.lines().any(|l| l.starts_with("@@ "));
    let mut hunk = None;
    let lines = text
        .split_terminator('\n')
        .enumerate()
        .map(|(index, line)| {
            if line.starts_with("@@ ") {
                hunk = Some(hunk.map_or(0, |n| n + 1));
            }
            let kind = if hunk.is_some() && line.starts_with('+') {
                "added"
            } else if hunk.is_some() && line.starts_with('-') {
                "deleted"
            } else if hunk.is_some() && line.starts_with(' ') {
                "context"
            } else {
                "header"
            };
            PatchLine {
                index,
                text: line.into(),
                kind: kind.into(),
                hunk,
            }
        })
        .collect();
    Ok(SelectionPatch { token: patch_token(&root, file, staged, &text)?, text, selectable: !unsupported,
        reason: unsupported.then(|| "Select the whole file for additions, deletions, renames, binary/mode changes, very large files or files without a final newline.".into()), lines })
}
fn hunk_start(header: &str, index: usize) -> Result<i64> {
    header
        .split_whitespace()
        .nth(index)
        .and_then(|field| field.get(1..))
        .and_then(|field| field.split(',').next())
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| CoreError::Other("Invalid Git patch hunk header".into()))
}
fn selected_patch(patch: &SelectionPatch, selected: &[usize], reverse: bool) -> Result<String> {
    if !patch.selectable {
        return Err(CoreError::Other(patch.reason.clone().unwrap_or_default()));
    }
    if selected.is_empty() {
        return Err(CoreError::Other(
            "Select at least one changed line or hunk".into(),
        ));
    }
    let chosen: std::collections::HashSet<usize> = selected.iter().copied().collect();
    if chosen.iter().any(|index| {
        !patch
            .lines
            .iter()
            .any(|line| line.index == *index && matches!(line.kind.as_str(), "added" | "deleted"))
    }) {
        return Err(CoreError::Other(
            "The selection includes a line that cannot be staged".into(),
        ));
    }
    let mut result = String::new();
    for line in patch.lines.iter().take_while(|line| line.hunk.is_none()) {
        result.push_str(&line.text);
        result.push('\n');
    }
    let count = patch
        .lines
        .iter()
        .filter_map(|line| line.hunk)
        .max()
        .unwrap_or(0)
        + 1;
    let mut delta = 0i64;
    for hunk in 0..count {
        let lines: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| line.hunk == Some(hunk))
            .collect();
        if !lines.iter().any(|line| chosen.contains(&line.index)) {
            continue;
        }
        let header = &lines[0].text;
        let mut old_count = 0i64;
        let mut new_count = 0i64;
        let mut body = String::new();
        for line in lines.iter().skip(1) {
            let chosen = chosen.contains(&line.index);
            let sign = match line.kind.as_str() {
                "added" if chosen => Some('+'),
                "deleted" if chosen => Some('-'),
                "added" if reverse => Some(' '),
                "deleted" if !reverse => Some(' '),
                "context" => Some(' '),
                _ => None,
            };
            if let Some(sign) = sign {
                if sign != '+' {
                    old_count += 1;
                }
                if sign != '-' {
                    new_count += 1;
                }
                body.push(sign);
                body.push_str(&line.text[1..]);
                body.push('\n');
            }
        }
        let (mut old_start, mut new_start) = if reverse {
            let start = hunk_start(header, 2)?;
            (start - delta, start)
        } else {
            let start = hunk_start(header, 1)?;
            (start, start + delta)
        };
        if old_count == 0 {
            old_start -= 1;
        }
        if new_count == 0 {
            new_start -= 1;
        }
        result.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n{body}"
        ));
        delta += new_count - old_count;
    }
    Ok(result)
}
fn apply_cached(path: &Path, text: &str, reverse: bool) -> Result<()> {
    let mut command = Command::new("git");
    command.arg("-C").arg(path).args([
        "-c",
        "core.fsmonitor=false",
        "-c",
        if cfg!(windows) {
            "core.hooksPath=NUL"
        } else {
            "core.hooksPath=/dev/null"
        },
        "apply",
        "--cached",
        "--unidiff-zero",
        "--whitespace=nowarn",
        "--recount",
    ]);
    if reverse {
        command.arg("--reverse");
    }
    command
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| CoreError::Other(e.to_string()))?;
    let write = child.stdin.take().unwrap().write_all(text.as_bytes());
    let output = child
        .wait_with_output()
        .map_err(|e| CoreError::Other(e.to_string()))?;
    if !output.status.success() {
        return Err(CoreError::Other(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    write.map_err(|e| CoreError::Other(e.to_string()))
}
pub fn apply_selection(
    path: &Path,
    file: &str,
    staged: bool,
    selected: &[usize],
    expected: &str,
) -> Result<()> {
    let root = checked_root(path)?;
    let patch = selection_patch(&root, file, staged)?;
    if patch.token != expected {
        return Err(CoreError::Other(
            "The selected diff changed. Refresh and select the changes again".into(),
        ));
    }
    let text = selected_patch(&patch, selected, staged)?;
    if patch_token(&root, file, staged, &patch.text)? != expected {
        return Err(CoreError::Other(
            "The index or working file changed while applying the selection".into(),
        ));
    }
    apply_cached(&root, &text, staged)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commits_in_a_repository_with_a_submodule_without_staging_its_pointer() {
        let temp = fixture();
        let root = temp.path();
        let sub = tempfile::tempdir().unwrap();
        let s = sub.path();
        git(s, &["init", "-q"]).unwrap();
        git(s, &["config", "user.email", "t@t"]).unwrap();
        git(s, &["config", "user.name", "t"]).unwrap();
        std::fs::write(s.join("f"), "1").unwrap();
        git(s, &["add", "f"]).unwrap();
        git(s, &["commit", "-q", "-m", "init"]).unwrap();
        git(
            root,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &s.to_string_lossy(),
                "vendor",
            ],
        )
        .unwrap();
        git(root, &["commit", "-q", "-m", "add submodule"]).unwrap();
        std::fs::write(s.join("f"), "2").unwrap();
        git(s, &["commit", "-q", "-am", "next"]).unwrap();
        git(&root.join("vendor"), &["pull", "-q"]).unwrap();
        std::fs::write(root.join("new.txt"), "x").unwrap();
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::StageAll, None, "", &state.token).unwrap();
        let staged = git(root, &["diff", "--cached", "--name-only"]).unwrap();
        assert_eq!(staged.trim(), "new.txt");
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::Commit, None, "with sub", &state.token).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn local_exclude_refuses_a_symlinked_info_directory() {
        use std::os::unix::fs::symlink;
        let temp = fixture();
        let root = temp.path();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("exclude"), "valuable existing rules\n").unwrap();
        std::fs::rename(root.join(".git/info"), root.join(".git/original-info")).unwrap();
        symlink(external.path(), root.join(".git/info")).unwrap();
        std::fs::write(root.join("untracked.txt"), "keep").unwrap();
        let state = workspace(root).unwrap();
        assert!(index_action(
            root,
            IndexAction::Exclude,
            Some("untracked.txt"),
            "",
            &state.token
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(external.path().join("exclude")).unwrap(),
            "valuable existing rules\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("untracked.txt")).unwrap(),
            "keep"
        );
    }
    #[cfg(unix)]
    #[test]
    fn selected_staging_never_runs_repository_index_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let temp = fixture();
        let root = temp.path();
        let hook = root.join(".git/hooks/post-index-change");
        std::fs::write(
            &hook,
            "#!/bin/sh\ngit update-ref refs/heads/hook-ran HEAD\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("a.txt"), "new staged content\n").unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        let selected: Vec<_> = patch
            .lines
            .iter()
            .filter(|l| matches!(l.kind.as_str(), "added" | "deleted"))
            .map(|l| l.index)
            .collect();
        apply_selection(root, "a.txt", false, &selected, &patch.token).unwrap();
        let patch = selection_patch(root, "a.txt", true).unwrap();
        let selected: Vec<_> = patch
            .lines
            .iter()
            .filter(|l| matches!(l.kind.as_str(), "added" | "deleted"))
            .map(|l| l.index)
            .collect();
        apply_selection(root, "a.txt", true, &selected, &patch.token).unwrap();
        assert!(git(root, &["show-ref", "--verify", "refs/heads/hook-ran"]).is_err());
    }
    #[test]
    fn commit_only_staged_file_and_reject_stale_index() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.com"]).unwrap();
        std::fs::write(p.join("a.txt"), "old").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "initial"]).unwrap();
        std::fs::write(p.join("a.txt"), "new").unwrap();
        std::fs::write(p.join("[b].txt"), "untracked").unwrap();
        let initial = workspace(p).unwrap();
        assert_eq!(initial.changes.len(), 2);
        index_action(p, IndexAction::Stage, Some("[b].txt"), "", &initial.token).unwrap();
        assert!(index_action(p, IndexAction::Commit, None, "stale", &initial.token).is_err());
        let staged = workspace(p).unwrap();
        index_action(p, IndexAction::Unstage, Some("[b].txt"), "", &staged.token).unwrap();
        assert!(p.join("[b].txt").exists());
        let state = workspace(p).unwrap();
        index_action(p, IndexAction::Stage, Some("a.txt"), "", &state.token).unwrap();
        assert!(patch(p, "a.txt", true).unwrap().contains("+new"));
        let staged = workspace(p).unwrap();
        index_action(p, IndexAction::Commit, None, "selected only", &staged.token).unwrap();
        assert_eq!(git(p, &["show", "HEAD:a.txt"]).unwrap(), "new");
        assert!(git(p, &["cat-file", "-e", "HEAD:[b].txt"]).is_err());
        assert_eq!(
            std::fs::read_to_string(p.join("[b].txt")).unwrap(),
            "untracked"
        );
        git(p, &["mv", "a.txt", "renamed.txt"]).unwrap();
        assert!(patch(p, "renamed.txt", true)
            .unwrap()
            .contains("rename from a.txt"));
        std::fs::write(p.join("renamed.txt"), "renamed and edited").unwrap();
        let state = workspace(p).unwrap();
        index_action(p, IndexAction::Stage, Some("renamed.txt"), "", &state.token).unwrap();
        let state = workspace(p).unwrap();
        index_action(
            p,
            IndexAction::Unstage,
            Some("renamed.txt"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(p.join("renamed.txt").exists());
    }

    fn fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-q"]).unwrap();
        git(root, &["config", "user.name", "Test"]).unwrap();
        git(root, &["config", "user.email", "test@example.com"]).unwrap();
        git(root, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(
            root.join("a.txt"),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\ntwelve\n",
        )
        .unwrap();
        std::fs::write(root.join("other.txt"), "other\n").unwrap();
        git(root, &["add", "."]).unwrap();
        git(root, &["commit", "-qm", "initial"]).unwrap();
        temp
    }
    #[test]
    fn selected_lines_stage_and_unstage_without_worktree_writes() {
        let temp = fixture();
        let root = temp.path();
        let working = "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\nTWELVE\n";
        std::fs::write(root.join("a.txt"), working).unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        assert!(patch.selectable);
        let first: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| {
                line.hunk == Some(0) && matches!(line.kind.as_str(), "added" | "deleted")
            })
            .map(|line| line.index)
            .collect();
        apply_selection(root, "a.txt", false, &first, &patch.token).unwrap();
        assert!(git(root, &["show", ":a.txt"]).unwrap().starts_with("ONE\n"));
        assert!(git(root, &["show", ":a.txt"])
            .unwrap()
            .ends_with("twelve\n"));
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            working
        );
        let patch = selection_patch(root, "a.txt", true).unwrap();
        let plus = patch
            .lines
            .iter()
            .find(|line| line.text == "+ONE")
            .unwrap()
            .index;
        apply_selection(root, "a.txt", true, &[plus], &patch.token).unwrap();
        assert!(!git(root, &["show", ":a.txt"]).unwrap().contains("ONE"));
        assert!(!git(root, &["show", ":a.txt"]).unwrap().starts_with("one\n"));
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            working
        );
        assert!(apply_selection(root, "a.txt", true, &[plus], &patch.token).is_err());
    }
    #[test]
    fn selected_hunks_preserve_offsets_across_additions_and_deletions() {
        let temp = fixture();
        let root = temp.path();
        let base = std::fs::read_to_string(root.join("a.txt")).unwrap();
        let work = base
            .replace("one\n", "one\nextra\n")
            .replace("twelve\n", "TWELVE\n");
        std::fs::write(root.join("a.txt"), &work).unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        let last: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| {
                line.hunk == Some(1) && matches!(line.kind.as_str(), "added" | "deleted")
            })
            .map(|line| line.index)
            .collect();
        assert!(!last.is_empty());
        apply_selection(root, "a.txt", false, &last, &patch.token).unwrap();
        let staged = git(root, &["show", ":a.txt"]).unwrap();
        assert!(!staged.contains("extra"));
        assert!(staged.ends_with("TWELVE\n"));
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::StageAll, None, "", &state.token).unwrap();
        let patch = selection_patch(root, "a.txt", true).unwrap();
        let selected: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| {
                line.hunk == Some(1) && matches!(line.kind.as_str(), "added" | "deleted")
            })
            .map(|line| line.index)
            .collect();
        apply_selection(root, "a.txt", true, &selected, &patch.token).unwrap();
        let staged = git(root, &["show", ":a.txt"]).unwrap();
        assert!(staged.contains("extra"));
        assert!(staged.ends_with("twelve\n"));
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), work);
    }
    #[test]
    fn stale_working_content_and_unsupported_partial_changes_are_rejected() {
        let temp = fixture();
        let root = temp.path();
        std::fs::write(root.join("a.txt"), "new\n").unwrap();
        let state = workspace(root).unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        let line = patch
            .lines
            .iter()
            .find(|l| l.kind == "added")
            .unwrap()
            .index;
        std::fs::write(root.join("a.txt"), "newer\n").unwrap();
        assert!(index_action(root, IndexAction::Stage, Some("a.txt"), "", &state.token).is_err());
        assert!(apply_selection(root, "a.txt", false, &[line], &patch.token).is_err());
        std::fs::write(root.join("a.txt"), b"\0binary").unwrap();
        assert!(!selection_patch(root, "a.txt", false).unwrap().selectable);
        std::fs::write(root.join("a.txt"), "no newline").unwrap();
        assert!(!selection_patch(root, "a.txt", false).unwrap().selectable);
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        assert!(!selection_patch(root, "new.txt", false).unwrap().selectable);
    }
    #[test]
    fn all_index_operations_and_amend_retain_working_files_and_old_commit() {
        let temp = fixture();
        let root = temp.path();
        let original = head(root);
        std::fs::write(root.join("a.txt"), "changed\n").unwrap();
        std::fs::write(root.join("[new].txt"), "untracked\n").unwrap();
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::StageAll, None, "", &state.token).unwrap();
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::UnstageAll, None, "", &state.token).unwrap();
        assert!(git(root, &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "changed\n"
        );
        assert!(root.join("[new].txt").exists());
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::Stage, Some("a.txt"), "", &state.token).unwrap();
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::Amend, None, "amended", &state.token).unwrap();
        assert_eq!(git(root, &["show", "HEAD:a.txt"]).unwrap(), "changed\n");
        assert_eq!(
            git(
                root,
                &["rev-parse", &format!("refs/gall/recovery/{original}")]
            )
            .unwrap()
            .trim(),
            original
        );
        assert!(root.join("[new].txt").exists());
    }
    #[test]
    fn targeted_discard_keeps_staged_content_and_unrelated_changes_recoverable() {
        let temp = fixture();
        let root = temp.path();
        std::fs::write(root.join("a.txt"), "staged\n").unwrap();
        git(root, &["add", "a.txt"]).unwrap();
        std::fs::write(root.join("a.txt"), "unstaged\n").unwrap();
        std::fs::write(root.join("other.txt"), "unrelated\n").unwrap();
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::Discard, Some("a.txt"), "", &state.token).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "staged\n"
        );
        assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), "staged\n");
        assert_eq!(
            std::fs::read_to_string(root.join("other.txt")).unwrap(),
            "unrelated\n"
        );
        assert_eq!(
            git(root, &["show", "stash@{0}:a.txt"]).unwrap(),
            "unstaged\n"
        );
        std::fs::write(root.join("[new].txt"), "recover me\n").unwrap();
        let state = workspace(root).unwrap();
        index_action(
            root,
            IndexAction::Discard,
            Some("[new].txt"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(!root.join("[new].txt").exists());
        assert_eq!(
            git(root, &["show", "stash@{0}^3:[new].txt"]).unwrap(),
            "recover me\n"
        );
        assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), "staged\n");
        assert_eq!(
            std::fs::read_to_string(root.join("other.txt")).unwrap(),
            "unrelated\n"
        );
    }
    #[test]
    fn ignore_rules_are_literal_and_append_without_replacing_existing_rules() {
        let temp = fixture();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "existing").unwrap();
        std::fs::write(root.join("[file] #!.txt"), "ignored\n").unwrap();
        std::fs::write(root.join("f #!.txt"), "visible\n").unwrap();
        let state = workspace(root).unwrap();
        index_action(
            root,
            IndexAction::Ignore,
            Some("[file] #!.txt"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(std::fs::read_to_string(root.join(".gitignore"))
            .unwrap()
            .starts_with("existing\n"));
        assert!(git(root, &["check-ignore", "--", "[file] #!.txt"]).is_ok());
        assert!(git(root, &["check-ignore", "--", "f #!.txt"]).is_err());
        let state = workspace(root).unwrap();
        index_action(
            root,
            IndexAction::Exclude,
            Some("f #!.txt"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(git(root, &["check-ignore", "--", "f #!.txt"]).is_ok());
        assert!(root.join("f #!.txt").exists());
    }
    #[test]
    fn unborn_repository_can_stage_unstage_and_create_first_commit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-q"]).unwrap();
        git(root, &["config", "user.name", "Test"]).unwrap();
        git(root, &["config", "user.email", "test@example.com"]).unwrap();
        std::fs::write(root.join("first.txt"), "first\n").unwrap();
        let state = workspace(root).unwrap();
        assert!(state.head.is_empty());
        index_action(root, IndexAction::StageAll, None, "", &state.token).unwrap();
        let state = workspace(root).unwrap();
        index_action(
            root,
            IndexAction::Unstage,
            Some("first.txt"),
            "",
            &state.token,
        )
        .unwrap();
        assert!(root.join("first.txt").exists());
        let state = workspace(root).unwrap();
        index_action(root, IndexAction::StageAll, None, "", &state.token).unwrap();
        let state = workspace(root).unwrap();
        index_action(
            root,
            IndexAction::Commit,
            None,
            "first commit",
            &state.token,
        )
        .unwrap();
        assert!(!head(root).is_empty());
    }
    #[test]
    fn individual_replacement_lines_can_be_staged_and_unstaged_without_context() {
        for staged in [false, true] {
            for choose_addition in [false, true] {
                let temp = fixture();
                let root = temp.path();
                std::fs::write(root.join("a.txt"), "old\n").unwrap();
                git(root, &["add", "a.txt"]).unwrap();
                git(root, &["commit", "-qm", "single line"]).unwrap();
                std::fs::write(root.join("a.txt"), "new\n").unwrap();
                if staged {
                    git(root, &["add", "a.txt"]).unwrap();
                }
                let patch = selection_patch(root, "a.txt", staged).unwrap();
                let selected = patch
                    .lines
                    .iter()
                    .find(|line| line.kind == if choose_addition { "added" } else { "deleted" })
                    .unwrap()
                    .index;
                apply_selection(root, "a.txt", staged, &[selected], &patch.token).unwrap();
                let expected = if staged == choose_addition {
                    ""
                } else {
                    "old\nnew\n"
                };
                assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), expected);
                assert_eq!(
                    std::fs::read_to_string(root.join("a.txt")).unwrap(),
                    "new\n"
                );
            }
        }
    }
    #[test]
    fn partial_staging_preserves_crlf_content_bytes() {
        let temp = fixture();
        let root = temp.path();
        git(root, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(root.join("a.txt"), "old\r\nkeep\r\n").unwrap();
        git(root, &["add", "a.txt"]).unwrap();
        git(root, &["commit", "-qm", "crlf"]).unwrap();
        std::fs::write(root.join("a.txt"), "new\r\nkeep\r\n").unwrap();
        let patch = selection_patch(root, "a.txt", false).unwrap();
        let selected: Vec<_> = patch
            .lines
            .iter()
            .filter(|line| matches!(line.kind.as_str(), "added" | "deleted"))
            .map(|line| line.index)
            .collect();
        apply_selection(root, "a.txt", false, &selected, &patch.token).unwrap();
        assert_eq!(git(root, &["show", ":a.txt"]).unwrap(), "new\r\nkeep\r\n");
    }
}
