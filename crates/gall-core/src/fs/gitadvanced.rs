//! Explicit advanced Git workflows. Reads never change HEAD; writes are reviewed,
//! stale-checked and serialized by the application adapter.
use super::gitgraph::{git, has_content_filters, joined, root};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// Creates a worktree at the location a template names, see [`quick_worktree`].
    QuickWorktree(QuickWorktree),
    /// Takes back a [`Request::QuickWorktree`] of this session while nothing changed in it.
    UndoWorktree {
        destination: String,
    },
    RemoveWorktree {
        destination: String,
    },
    /// Like `RemoveWorktree`, but also deletes ignored files (caches, build output). Tracked
    /// changes and untracked files that are not ignored still block removal.
    RemoveWorktreeWithIgnored {
        destination: String,
    },
    /// Like `RemoveWorktree` for one with local changes: they are first kept under
    /// `refs/gall/recovery`. `ignored` also deletes ignored files.
    RemoveWorktreeWithSnapshot {
        destination: String,
        ignored: bool,
    },
    MoveWorktree {
        destination: String,
        to: String,
    },
    /// Writes the links between a worktree that was moved by hand and its repository again.
    RepairWorktree {
        destination: String,
    },
    /// Forgets the worktrees whose folder is gone.
    PruneWorktrees,
    LockWorktree {
        destination: String,
    },
    UnlockWorktree {
        destination: String,
    },
    DeinitSubmodule {
        file: String,
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
    /// Several commits as one mailbox of patches, in the given order.
    ExportPatches {
        commits: Vec<String>,
        destination: String,
    },
    ApplyPatch {
        file: String,
    },
    /// The same as `ApplyPatch`, for patch text pasted from the clipboard.
    ApplyPatchText {
        text: String,
    },
    Init {
        branch: String,
    },
    Clone {
        url: String,
        destination: String,
    },
}

/// What a new worktree checks out. Every `id` is the commit the user saw; a reference that
/// moved since is refused instead of checked out somewhere else.
#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WorktreeSource {
    /// An existing local branch.
    Branch { name: String, id: String },
    /// A new local branch `name` that tracks the remote branch `full_name`.
    Remote {
        full_name: String,
        name: String,
        id: String,
    },
    /// A new local branch at a commit.
    NewBranch { name: String, id: String },
    /// A detached HEAD at a commit; `label` (a tag, for example) names the folder.
    Commit { id: String, label: Option<String> },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickWorktree {
    pub source: WorktreeSource,
    /// Empty: [`super::gitlocation::DEFAULT_TEMPLATE`].
    pub template: String,
    /// Main worktree folder → its own template, which wins over `template`.
    #[serde(default)]
    pub templates: std::collections::BTreeMap<String, String>,
    /// Exactly this folder (typed in the form) instead of a template.
    #[serde(default)]
    pub destination: Option<String>,
    /// Main worktree folder → the command (program and arguments) to run in a new worktree of it after it was created
    /// and its files copied.
    #[serde(default)]
    pub commands: std::collections::BTreeMap<String, Vec<String>>,
    /// Main worktree folder → link the folders of `.worktreeinclude` instead of copying them.
    #[serde(default)]
    pub links: std::collections::BTreeMap<String, bool>,
}

/// The template for the repository whose main worktree is `main`.
fn template_for<'a>(
    main: &Path,
    template: &'a str,
    templates: &'a std::collections::BTreeMap<String, String>,
) -> &'a str {
    templates
        .iter()
        .find(|(folder, own)| Path::new(folder) == main && !own.trim().is_empty())
        .map_or(template, |(_, own)| own)
}

/// The answer to [`Request::QuickWorktree`], as JSON.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickResult {
    pub path: String,
    /// The branch already has a worktree, at `path`; nothing was created.
    pub existing: bool,
    /// Ignored files copied by `.worktreeinclude`.
    pub copied: usize,
    /// Ignored folders linked by `.worktreeinclude`.
    pub linked: usize,
    /// What did not work although the worktree exists.
    pub warnings: Vec<String>,
    /// The output of the command that ran after the creation, for the log; empty when none ran.
    #[serde(default)]
    pub log: String,
}

/// Where a worktree would go, for the live preview of a template.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeLocation {
    /// The folder of the main worktree, which names the repository in the settings.
    pub main: String,
    /// None with an `error` when the template cannot be used here.
    pub path: Option<String>,
    pub error: Option<String>,
    /// Inside the main worktree: its folder is added to the local excludes.
    pub inside: bool,
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
    // As in `gitrepository::validate_name`: Git accepts what its documented rules accept.
    // `HEAD` is a valid reference name but refused as a branch, so Git decides there.
    if name != "HEAD" && super::gitfacts::plain_ref_name(&format!("refs/heads/{name}")) {
        return Ok(());
    }
    if git(path, &["check-ref-format", "--branch", name])?.trim() != name {
        return Err(fail(
            "Enter a literal branch name instead of a previous-checkout expression",
        ));
    }
    Ok(())
}

fn git_path(path: &Path, name: &str) -> Result<PathBuf> {
    if let Some(found) = super::gitfacts::marker(path, name) {
        return Ok(found);
    }
    let value = git(path, &["rev-parse", "--git-path", name])?;
    Ok(path.join(super::gitexec::host_path(path, value.trim())))
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

/// A local core.worktree value, only when this file can be read unambiguously without
/// following includes. Git's generated submodule configuration uses this format.
fn configured_worktree(text: &str) -> Option<String> {
    let mut core = false;
    let mut result = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            let end = line.find(']')?;
            let section = line[1..end].trim();
            if section.to_ascii_lowercase().starts_with("include") {
                return None;
            }
            core = section.eq_ignore_ascii_case("core");
        } else if core {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if !key.trim().eq_ignore_ascii_case("worktree") {
                continue;
            }
            let mut value_chars = value.trim_start().chars();
            let (mut parsed, mut quoted, mut kept) = (String::new(), false, 0);
            while let Some(ch) = value_chars.next() {
                match ch {
                    '"' => quoted = !quoted,
                    '#' | ';' if !quoted => break,
                    '\\' => {
                        parsed.push(match value_chars.next()? {
                            'n' => '\n',
                            't' => '\t',
                            'b' => '\u{8}',
                            '\\' => '\\',
                            '"' => '"',
                            _ => return None,
                        });
                        kept = parsed.len();
                    }
                    _ => {
                        parsed.push(ch);
                        if quoted || !ch.is_whitespace() {
                            kept = parsed.len();
                        }
                    }
                }
            }
            if quoted {
                return None;
            }
            parsed.truncate(kept);
            result = Some(parsed);
        }
    }
    result.filter(|value| !value.is_empty())
}

pub(super) fn worktree_metadata_path(root: &Path, base: &Path, value: &str) -> PathBuf {
    if super::gitexec::place(root).is_some() && value.starts_with('/') {
        super::gitexec::host_path(root, value)
    } else {
        base.join(value)
    }
}

pub(super) fn worktree_git_dir(root: &Path) -> Option<PathBuf> {
    if super::gitexec::place(root).is_none() {
        return super::gitfacts::git_dir(root);
    }
    // An absolute gitdir inside WSL is a Linux path, not an absolute Windows path.
    let dot = root.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(dot).ok()?;
    let value = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    Some(worktree_metadata_path(root, root, value))
}

/// Git lists the common Git directory as the main worktree of a submodule or a
/// separate Git directory. Recover the actual folder, including from a linked worktree,
/// without another Git process. Never replace a bare entry or an ambiguous config value.
fn main_worktree_path(root: &Path, listed: &Path) -> Option<PathBuf> {
    // Ordinary main and linked worktrees need no metadata/configuration reads. In WSL
    // each unnecessary read or canonicalization through the host share costs a round trip.
    // A `.git` folder in `listed` does not make it a checkout when `root` has a `.git` file: it
    // may be a separate Git directory named `.git` that Git lists by its parent.
    if root == listed || (listed.join(".git").is_dir() && root.join(".git").is_dir()) {
        return None;
    }
    let canonical = crate::nav::path::canonicalize;
    let git_dir = worktree_git_dir(root)?;
    let common = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(value) => worktree_metadata_path(root, &git_dir, value.trim()),
        Err(_) => git_dir.clone(),
    };
    let common = canonical(&common).ok()?;
    // Git prints a common directory named `.git` as its parent, whether or not a checkout is there.
    let listed = canonical(listed).ok()?;
    let named_dot_git = common.file_name().is_some_and(|name| name == ".git")
        && common.parent() == Some(listed.as_path());
    if listed != common && !named_dot_git {
        return None;
    }
    if canonical(&git_dir).ok()? == common {
        return canonical(root).ok();
    }
    let config = std::fs::read_to_string(common.join("config")).ok()?;
    let value = configured_worktree(&config)?;
    let candidate = canonical(&worktree_metadata_path(root, &common, &value)).ok()?;
    let candidate_git_dir = worktree_git_dir(&candidate)?;
    (canonical(&candidate_git_dir).ok()? == common).then_some(candidate)
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
                // Git prints `C:/dir` on Windows; the rest of the app uses `C:\dir`. A
                // repository in WSL prints Linux paths, which become `\\wsl.localhost\…`.
                path: if super::gitexec::place(path).is_some() {
                    super::gitexec::host_path(path, value)
                        .to_string_lossy()
                        .into_owned()
                } else if cfg!(windows) {
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
    if let Some(first) = result.first_mut().filter(|entry| !entry.bare) {
        if let Some(root) = main_worktree_path(path, Path::new(&first.path)) {
            first.path = root.to_string_lossy().into_owned();
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

fn submodule_marker(path: &Path, file: &str) -> Result<PathBuf> {
    let module = path.join(file);
    let marker = module
        .join(".git")
        .symlink_metadata()
        .map_err(|e| fail(e.to_string()))?;
    if marker.is_symlink() {
        return Err(fail("Symlink submodule Git directories are unsupported"));
    }
    Ok(module)
}

fn same_folder(module: &Path, actual: &Path) -> Result<()> {
    if actual.canonicalize().map_err(|e| fail(e.to_string()))?
        != module.canonicalize().map_err(|e| fail(e.to_string()))?
    {
        return Err(fail(
            "The submodule Git directory points outside its working folder",
        ));
    }
    Ok(())
}

fn initialized_submodule(path: &Path, file: &str) -> Result<()> {
    let module = submodule_marker(path, file)?;
    let actual = root(&module)?;
    same_folder(&module, &actual)
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
    // Every Git process costs a round trip (about 70 ms inside WSL), so the questions that do
    // not depend on each other run side by side, and those about all submodules are asked
    // once: the validity check, the paths and the URLs together, then the recorded commits.
    let combined = git(
        path,
        &[
            "config",
            "--no-includes",
            "--null",
            "--file",
            ".gitmodules",
            "--get-regexp",
            "^submodule\\..*\\.(path|url)$",
        ],
    );
    let combined = match combined {
        Ok(text) => text,
        Err(_) => {
            // No match also fails: only a file Git cannot read at all is an error.
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
            String::new()
        }
    };
    let mut output = String::new();
    let mut urls: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for field in combined.split('\0').filter(|f| !f.is_empty()) {
        let Some((key, value)) = field.split_once('\n') else {
            continue;
        };
        if key.ends_with(".path") {
            output.push_str(field);
            output.push('\0');
        } else if let Some(name) = key
            .strip_prefix("submodule.")
            .and_then(|k| k.strip_suffix(".url"))
        {
            urls.insert(name.to_owned(), value.trim_end().to_owned());
        }
    }
    if output.contains('\u{fffd}') {
        return Err(fail("Submodule paths must contain valid UTF-8"));
    }
    let literals: Vec<String> = output
        .split('\0')
        .filter_map(|f| f.split_once('\n'))
        .map(|(_, file)| format!(":(literal){file}"))
        .collect();
    let mut records: Vec<(&str, &str)> = Vec::new();
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
        records.push((name, file));
    }
    // The recorded commits of all submodules in one call.
    let read_ids = || -> Result<std::collections::HashMap<String, String>> {
        let mut ids = std::collections::HashMap::new();
        if literals.is_empty() {
            return Ok(ids);
        }
        let mut args = vec!["ls-files", "--stage", "-z", "--"];
        args.extend(literals.iter().map(String::as_str));
        for record in git(path, &args)?.split('\0') {
            if let Some((meta, file)) = record.split_once('\t') {
                if let Some(id) = meta
                    .strip_prefix("160000 ")
                    .and_then(|v| v.split_whitespace().next())
                {
                    ids.entry(file.to_owned()).or_insert_with(|| id.to_owned());
                }
            }
        }
        Ok(ids)
    };
    // What can be said about a submodule without knowing its recorded commit: a fixed
    // state, or the HEAD it is at.
    let head_of = |file: &str| -> std::result::Result<String, &'static str> {
        if no_symlink_parents(path, file).is_err() {
            return Err("unsupportedPath");
        }
        let module = path.join(file);
        if !module.join(".git").exists() {
            return Err("notInitialized");
        }
        // One Git process answers both questions (its work tree and its HEAD); when it
        // fails (no commit yet, a foreign Git directory) the questions are asked apart.
        let quick = submodule_marker(path, file).and_then(|module| {
            let both = git(&module, &["rev-parse", "--show-toplevel", "HEAD"])?;
            let mut lines = both.lines();
            let top = lines.next().ok_or_else(|| fail("No work tree"))?;
            same_folder(&module, &super::gitexec::host_path(&module, top))?;
            Ok(lines.next().unwrap_or_default().to_owned())
        });
        match quick {
            Ok(head) => Ok(head),
            Err(_) => {
                let (initialized, head) = std::thread::scope(|scope| {
                    let head = scope.spawn(|| {
                        git(&module, &["rev-parse", "--verify", "HEAD"]).unwrap_or_default()
                    });
                    (
                        initialized_submodule(path, file),
                        head.join().unwrap_or_default(),
                    )
                });
                initialized.map(|_| head).map_err(|_| "unsupportedPath")
            }
        }
    };
    // The commits and all the submodules are read at once.
    let (ids, heads) = std::thread::scope(|scope| {
        let ids = scope.spawn(read_ids);
        let heads: Vec<_> = records
            .iter()
            .map(|(_, file)| scope.spawn(|| head_of(file)))
            .collect();
        (
            ids.join()
                .unwrap_or_else(|_| Err(fail("Submodule listing failed"))),
            heads
                .into_iter()
                .map(|h| h.join().unwrap_or(Err("unsupportedPath")))
                .collect::<Vec<_>>(),
        )
    });
    let ids = ids?;
    Ok(records
        .into_iter()
        .zip(heads)
        .map(|((name, file), head)| {
            let id = ids.get(file).cloned().unwrap_or_default();
            let state = match head {
                Err(fixed) => fixed.to_owned(),
                Ok(head) if head.trim() == id => "current".to_owned(),
                Ok(_) => "differentCommit".to_owned(),
            };
            Submodule {
                name: name.into(),
                path: file.into(),
                url: urls.get(name).cloned().unwrap_or_default(),
                id,
                state,
            }
        })
        .collect())
}

/// One submodule at any depth below the opened repository.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmoduleNode {
    /// Path from the opened repository, for example `libs/a/inner`.
    pub label: String,
    /// Path relative to the submodule's parent repository.
    pub name: String,
    /// Folder of the submodule (native path of the parent joined with the relative path).
    pub path: String,
    pub state: String,
    pub depth: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmoduleTree {
    pub repository: Option<String>,
    pub entries: Vec<SubmoduleNode>,
}

const MAX_SUBMODULE_DEPTH: usize = 4;

fn collect_submodules(
    repository: &Path,
    prefix: &str,
    depth: usize,
    entries: &mut Vec<SubmoduleNode>,
) -> Result<()> {
    let modules = submodules(repository)?;
    // The nested lists of the submodules are read side by side, then put in order.
    let nested: Vec<Vec<SubmoduleNode>> = std::thread::scope(|scope| {
        let handles: Vec<_> = modules
            .iter()
            .map(|module| {
                let usable = matches!(module.state.as_str(), "current" | "differentCommit")
                    && depth + 1 < MAX_SUBMODULE_DEPTH;
                scope.spawn(move || {
                    let mut nested = Vec::new();
                    // A broken nested module must not hide its siblings.
                    if usable
                        && collect_submodules(
                            &repository.join(&module.path),
                            &format!("{prefix}{}/", module.path),
                            depth + 1,
                            &mut nested,
                        )
                        .is_err()
                    {
                        nested.clear();
                    }
                    nested
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    for (module, nested) in modules.into_iter().zip(nested) {
        entries.push(SubmoduleNode {
            label: format!("{prefix}{}", module.path),
            name: module.path.clone(),
            path: repository.join(&module.path).to_string_lossy().into_owned(),
            state: module.state,
            depth,
        });
        entries.extend(nested);
    }
    Ok(())
}

/// Lists submodules recursively (depth first) without loading the full advanced snapshot,
/// which is far slower for every level.
pub fn submodule_tree(path: &Path) -> Result<SubmoduleTree> {
    let Ok(repository) = root(path) else {
        return Ok(SubmoduleTree {
            repository: None,
            entries: Vec::new(),
        });
    };
    let mut entries = Vec::new();
    collect_submodules(&repository, "", 0, &mut entries)?;
    Ok(SubmoduleTree {
        repository: Some(repository.to_string_lossy().into_owned()),
        entries,
    })
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
    // Another Git client, or Gall's own background index refresh, may rewrite the index
    // or a reference once while this reads: one more try before reporting it.
    match snapshot_once(path, commit) {
        Err(CoreError::Other(message)) if message.starts_with("Repository changed while") => {
            snapshot_once(path, commit)
        }
        result => result,
    }
}

fn snapshot_once(path: &Path, commit: Option<&str>) -> Result<AdvancedSnapshot> {
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
    // Everything is read side by side, between two light tokens (references, HEAD,
    // configuration, stashes, operation markers and the index file) that have to agree.
    // This used to be two full repository snapshots, one before and one after, with the
    // details read one by one in between: twice the work tree status and some thirty
    // processes in a row. A work tree change during the load still changes the token of
    // the repository snapshot, so an action that starts from this snapshot is refused.
    let repo = &repository;
    let before = super::gitrepository::light_token(repo)?;
    let (state, worktrees, submodules, bisect, notes) = std::thread::scope(|scope| {
        let worktrees = scope.spawn(|| worktrees(repo));
        let submodules = scope.spawn(|| submodules(repo));
        let bisect = scope.spawn(|| bisect(repo));
        let notes = scope.spawn(|| -> Result<String> {
            let Some(commit) = commit else {
                return Ok(String::new());
            };
            let commit = concrete_commit(repo, commit)?;
            Ok(git(
                repo,
                &["notes", "--ref=refs/notes/commits", "show", &commit],
            )
            .unwrap_or_default())
        });
        let state = super::gitrepository::snapshot(repo);
        (
            state,
            joined(worktrees),
            joined(submodules),
            joined(bisect),
            joined(notes),
        )
    });
    let state = state?;
    let (worktrees, submodules, bisect, notes) = (worktrees??, submodules??, bisect??, notes??);
    if before != super::gitrepository::light_token(repo)? {
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
    let mut command = super::gitexec::command(path, super::gitstatus::program());
    command
        .args(["--no-pager", "-c", "core.fsmonitor=false", "-c"])
        .arg(format!(
            "core.hooksPath={}",
            super::gitexec::null_device(path)
        ))
        .args([
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "log.showSignature=false",
            "-c",
            "submodule.recurse=false",
        ]);
    // A new worktree writes every file.
    if let Some(workers) = super::gitgraph::checkout_workers(path) {
        command.arg("-c").arg(workers);
    }
    command
        .arg("-C")
        .arg(super::gitexec::dir_arg(path))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env(
            "GIT_ASKPASS",
            if cfg!(windows) && super::gitexec::place(path).is_none() {
                ""
            } else {
                "/bin/false"
            },
        )
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
    super::gitexec::forward_env(&mut command);
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
                let text = String::from_utf8_lossy(&chunk).into_owned();
                super::gitprogress::feed_stderr(&text);
                progress(text);
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
                        let text = String::from_utf8_lossy(&chunk).into_owned();
                        super::gitprogress::feed_stderr(&text);
                        progress(text);
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

fn first_checkout_change(path: &Path, submodule_status: &str) -> Result<Option<String>> {
    let status = git(
        path,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            submodule_status,
        ],
    )?;
    Ok(status
        .split_terminator('\0')
        .next()
        .and_then(|record| record.get(3..))
        .map(str::to_owned))
}

fn require_clean_checkout(path: &Path, submodule_status: &str) -> Result<()> {
    if let Some(name) = first_checkout_change(path, submodule_status)? {
        return Err(fail(format!(
            "Commit or stash local changes at {name} before this operation"
        )));
    }
    Ok(())
}

fn filtered_index_files<'a>(path: &Path, index: &'a str) -> Result<Vec<(&'a str, &'a str)>> {
    if index.contains('\u{fffd}') {
        return Err(fail(
            "Cannot verify filtered bytes for paths with invalid UTF-8",
        ));
    }
    let mut files = Vec::new();
    for record in index.split_terminator('\0') {
        let Some((metadata, name)) = record.split_once('\t') else {
            return Err(fail("Git returned an invalid index record"));
        };
        let mut fields = metadata.split_whitespace();
        if !matches!(fields.next(), Some("100644" | "100755")) {
            continue;
        }
        let id = fields
            .next()
            .ok_or_else(|| fail("Git returned an index record without a blob"))?;
        files.push((name, id));
    }
    paths_with_filters(path, &files)
}

fn paths_with_filters<'a>(
    path: &Path,
    files: &[(&'a str, &'a str)],
) -> Result<Vec<(&'a str, &'a str)>> {
    let mut filtered = Vec::new();
    let mut pending = files;
    while !pending.is_empty() {
        // Bound command-line size for Windows and WSL as well as Unix.
        let mut end = 1;
        let mut bytes = pending[0].0.len() + 1;
        while end < pending.len() && bytes + pending[end].0.len() < 12 * 1024 {
            bytes += pending[end].0.len() + 1;
            end += 1;
        }
        let (batch, rest) = pending.split_at(end);
        pending = rest;
        let mut args = vec!["check-attr", "-z", "filter", "--"];
        args.extend(batch.iter().map(|(name, _)| *name));
        let output = git(path, &args)?;
        let mut attributes = output.split_terminator('\0');
        for &(name, id) in batch {
            match (attributes.next(), attributes.next(), attributes.next()) {
                (Some(file), Some("filter"), Some(value)) if file == name => {
                    if !matches!(value, "unspecified" | "unset") {
                        filtered.push((name, id));
                    }
                }
                _ => {
                    return Err(fail(format!(
                        "Could not inspect content filters for {name}"
                    )))
                }
            }
        }
        if attributes.next().is_some() {
            return Err(fail("Git returned extra filter attributes"));
        }
    }
    Ok(filtered)
}

/// Git status may hide bytes dropped by a clean filter. Check the raw
/// checkout against Git's smudged representation before deleting it.
fn verify_filtered_checkout_bytes(path: &Path, cancel: &AtomicBool) -> Result<()> {
    if !has_content_filters(path) {
        return Ok(());
    }
    let check_cancel = || {
        if cancel.load(Ordering::Relaxed) {
            Err(fail("Cancelled before removing the worktree"))
        } else {
            Ok(())
        }
    };
    check_cancel()?;
    let index = git(path, &["ls-files", "--stage", "-z"])?;
    for (name, id) in filtered_index_files(path, &index)? {
        check_cancel()?;
        verify_filtered_file(path, name, id, cancel, || {
            fail(format!(
                "Could not verify original filtered bytes at {name}. Move the file aside before removing the worktree"
            ))
        })?;
    }
    Ok(())
}

pub(super) fn verify_selected_filtered_checkout_bytes(
    path: &Path,
    tagged_index: &str,
    file: &str,
) -> Result<()> {
    if !has_content_filters(path) {
        return Ok(());
    }
    let index: String = tagged_index
        .split_inclusive('\0')
        .filter_map(|entry| entry.get(2..))
        .collect();
    for (name, id) in filtered_index_files(path, &index)? {
        verify_filtered_file(path, name, id, &AtomicBool::new(false), || {
            fail(format!(
                "Could not verify original filtered bytes at {file}. Move it aside in the file manager before checkout."
            ))
        })?;
    }
    Ok(())
}

/// Check that the saved tree can reproduce the current raw bytes before a history operation.
pub(super) fn verify_filtered_recovery(path: &Path, tree: &str, operation: &str) -> Result<()> {
    if !has_content_filters(path) {
        return Ok(());
    }
    let listing = git(
        path,
        &[
            "ls-tree",
            "-r",
            "-z",
            "--format=%(objectmode) %(objectname)%x09%(path)",
            tree,
        ],
    )?;
    let cancel = AtomicBool::new(false);
    let verify = |file: &str, id: &str| {
        verify_filtered_file(path, file, id, &cancel, || {
            fail(format!(
                "{operation} cannot recover the original filtered bytes at {file}. Copy the file outside the worktree before continuing."
            ))
        })
    };
    let validate_path = |file| {
        no_symlink_parents(path, file).map_err(|error| {
            fail(format!(
                "Could not verify filtered bytes at {file}: {error}"
            ))
        })
    };
    for (file, id) in filtered_index_files(path, &listing)? {
        validate_path(file)?;
        match path.join(file).symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(fail(format!("Could not inspect {file}: {error}"))),
            Ok(_) => {}
        }
        verify(file, id)?;
    }
    Ok(())
}

/// Inspect the working bytes Git would stash for precisely the requested paths.
pub(super) fn verify_filtered_stash_paths(
    path: &Path,
    specs: &[&str],
    include_untracked: bool,
) -> Result<()> {
    let mut args = vec!["ls-files", "--cached", "--deduplicate", "-z"];
    if include_untracked {
        args.extend(["--others", "--exclude-standard"]);
    }
    args.push("--");
    args.extend_from_slice(specs);
    let listing = git(path, &args)?;
    if listing.contains('\u{fffd}') {
        return Err(fail(
            "Cannot verify filtered bytes for paths with invalid UTF-8",
        ));
    }
    let files: Vec<_> = listing
        .split_terminator('\0')
        .filter(|file| !file.ends_with('/'))
        .map(|file| (file, ""))
        .collect();
    let cancel = AtomicBool::new(false);
    for (file, _) in paths_with_filters(path, &files)? {
        match path.join(file).symlink_metadata() {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(fail(format!("Could not inspect {file}: {error}"))),
        }
        no_symlink_parents(path, file)?;
        let id = git(
            path,
            &["hash-object", "-w", &format!("--path={file}"), "--", file],
        )?;
        verify_filtered_file(path, file, id.trim(), &cancel, || {
            fail(format!(
                "Stash cannot recover the original filtered bytes at {file}. Copy the file outside the worktree before continuing."
            ))
        })?;
    }
    Ok(())
}

fn verify_filtered_file(
    path: &Path,
    name: &str,
    id: &str,
    cancel: &AtomicBool,
    changed: impl Fn() -> CoreError,
) -> Result<()> {
    use std::io::{BufRead, BufReader};
    let file = path.join(name);
    if !file.symlink_metadata().is_ok_and(|meta| meta.is_file()) {
        return Err(changed());
    }
    let file = super::open_regular(&file).map_err(|_| changed())?;
    let mut command = super::gitexec::command(path, super::gitstatus::program());
    command
        .arg("-C")
        .arg(super::gitexec::dir_arg(path))
        .args(["cat-file", "--filters", &format!("--path={name}"), id])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
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
    super::gitexec::forward_env(&mut command);
    let mut child = command.spawn().map_err(|_| changed())?;
    let Some(stdout) = child.stdout.take() else {
        stop(&mut child);
        return Err(changed());
    };
    let (sender, receiver) = mpsc::sync_channel(1);
    let transferred = AtomicU64::new(0);
    std::thread::scope(|scope| {
        let transferred = &transferred;
        scope.spawn(move || {
            let compare = || -> std::io::Result<bool> {
                let mut actual = BufReader::new(file);
                let mut expected = BufReader::new(stdout);
                loop {
                    let a = actual.fill_buf()?;
                    let b = expected.fill_buf()?;
                    if a.is_empty() || b.is_empty() {
                        return Ok(a.is_empty() && b.is_empty());
                    }
                    let count = a.len().min(b.len());
                    if a[..count] != b[..count] {
                        return Ok(false);
                    }
                    actual.consume(count);
                    expected.consume(count);
                    transferred.fetch_add(count as u64, Ordering::Relaxed);
                }
            };
            let _ = sender.send(compare());
        });
        let mut matched = false;
        let mut seen = 0;
        let mut last_activity = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) || last_activity.elapsed() > Duration::from_secs(180)
            {
                stop(&mut child);
                return Err(changed());
            }
            if matched {
                match child.try_wait() {
                    Ok(Some(status)) if status.success() => return Ok(()),
                    Ok(Some(_)) => return Err(changed()),
                    Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                    Err(_) => {
                        stop(&mut child);
                        return Err(changed());
                    }
                }
            } else {
                match receiver.recv_timeout(Duration::from_millis(25)) {
                    Ok(Ok(true)) => matched = true,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    _ => {
                        stop(&mut child);
                        return Err(changed());
                    }
                }
            }
            let current = transferred.load(Ordering::Relaxed);
            if current != seen {
                seen = current;
                last_activity = Instant::now();
            }
        }
    })
}

fn require_idle_checkout(path: &Path) -> Result<()> {
    let state = super::gitrepository::snapshot(path)?;
    if state.operation.is_some() || !state.conflicts.is_empty() {
        return Err(fail("Finish or abort the current Git operation first"));
    }
    Ok(())
}

/// Check operation state before a native checkout or indexed patch.
fn ready_worktree(path: &Path) -> Result<()> {
    let state = super::gitrepository::snapshot(path)?;
    if state
        .operation
        .as_deref()
        .is_some_and(|value| value != "bisect")
        || !state.conflicts.is_empty()
    {
        return Err(fail("Finish or abort the current Git operation first"));
    }
    Ok(())
}

/// Fetch the recorded gitlink without running the submodule's implicit checkout.
fn fetch_submodule_commit(
    path: &Path,
    id: &str,
    cancel: &AtomicBool,
    progress: &impl Fn(String),
) -> Result<String> {
    if !matches!(id.len(), 40 | 64) || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(fail(
            "The submodule has no valid commit in the parent index",
        ));
    }
    let remote = git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .and_then(|branch| {
            git(
                path,
                &[
                    "config",
                    "--get",
                    &format!("branch.{}.remote", branch.trim()),
                ],
            )
            .ok()
        })
        .map(|remote| remote.trim().to_owned())
        .filter(|remote| !remote.is_empty())
        .unwrap_or_else(|| "origin".into());
    let url = git(path, &["remote", "get-url", "--", &remote])?;
    checked_url(url.trim())?;
    let mut fetch_args = vec!["fetch", "--no-recurse-submodules"];
    if super::gitprogress::is_active() {
        fetch_args.push("--progress");
    }
    fetch_args.extend(["--", &remote]);
    run(path, &fetch_args, cancel, progress, 1024 * 1024)?;
    if let Ok(target) = concrete_commit(path, id) {
        return Ok(target);
    }
    let mut fetch_id_args = vec!["fetch", "--no-recurse-submodules"];
    if super::gitprogress::is_active() {
        fetch_id_args.push("--progress");
    }
    fetch_id_args.extend(["--", &remote, id]);
    run(path, &fetch_id_args, cancel, progress, 1024 * 1024)?;
    concrete_commit(path, id)
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
    super::gitprogress::report_phase(
        "Checking out test revision",
        Some(target[..8.min(target.len())].to_owned()),
    );
    let mut args = vec!["switch", "--detach"];
    if super::gitprogress::is_active() {
        args.push("--progress");
    }
    args.extend(["--no-overwrite-ignore", &target]);
    let output = run(path, &args, cancel, progress, 1024 * 1024)?;
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

/// The common Git directory of the work tree `root` (linked worktrees share it).
pub(super) fn common_dir(root: &Path) -> Result<PathBuf> {
    let git_dir =
        worktree_git_dir(root).ok_or_else(|| fail("The Git directory is not readable"))?;
    let common = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(value) => worktree_metadata_path(root, &git_dir, value.trim()),
        Err(_) => git_dir,
    };
    crate::nav::path::canonicalize(&common).map_err(|e| fail(e.to_string()))
}

/// The folder of the main worktree and the common Git directory, read from Git's files.
pub(super) fn main_and_common(root: &Path) -> Result<(PathBuf, PathBuf)> {
    let common = common_dir(root)?;
    if root.join(".git").is_dir() {
        return Ok((root.to_path_buf(), common));
    }
    let parent = || {
        common
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| fail("The main worktree could not be found"))
    };
    // A submodule or a separate Git directory names its folder; a bare repository has none, so
    // its worktrees are placed beside it.
    let configured = std::fs::read_to_string(common.join("config"))
        .ok()
        .as_deref()
        .and_then(configured_worktree)
        .and_then(|value| {
            crate::nav::path::canonicalize(&worktree_metadata_path(root, &common, &value)).ok()
        });
    if let Some(main) = configured {
        return Ok((main, common));
    }
    // The checkout whose own Git directory is the common one is the main worktree, also when that
    // directory is a separate one that happens to be named `.git` (it then lies outside the
    // checkout, and its parent is not the checkout). From a linked worktree of such a repository
    // nothing in the files names the main checkout, and Git itself guesses the parent too.
    let own = worktree_git_dir(root)
        .and_then(|dir| crate::nav::path::canonicalize(&dir).ok())
        .is_some_and(|dir| dir == common);
    if own {
        return Ok((root.to_path_buf(), common));
    }
    Ok((parent()?, common))
}

/// Where a worktree for `branch` would go. Reads files only, so it may run on every keystroke.
/// Outside a repository `path` itself stands for the main worktree.
pub fn worktree_location(
    path: &Path,
    template: &str,
    templates: &std::collections::BTreeMap<String, String>,
    branch: &str,
) -> Result<WorktreeLocation> {
    let (main, common) = match root(path).ok() {
        Some(repository) => {
            let (main, common) = main_and_common(&repository)?;
            (main, Some(common))
        }
        None => (path.to_path_buf(), None),
    };
    let template = template_for(&main, template, templates);
    let place = super::gitlocation::expand(template, &main, branch).and_then(|place| {
        if let Some(common) = &common {
            super::gitlocation::check(&main, common, &place)?;
        }
        super::gitlocation::free(&place, |_| false)
    });
    let (path, error) = match place {
        Ok(place) => (Some(place), None),
        Err(error) => (None, Some(error.to_string())),
    };
    Ok(WorktreeLocation {
        inside: path.as_ref().is_some_and(|place| place.starts_with(&main)),
        path: path.map(|place| place.to_string_lossy().into_owned()),
        error,
        main: main.to_string_lossy().into_owned(),
    })
}

/// Creates the folders above `path`; returns the new ones, outermost first.
fn create_parents(path: &Path) -> Result<Vec<PathBuf>> {
    let mut missing: Vec<PathBuf> = path
        .ancestors()
        .skip(1)
        .take_while(|p| p.symlink_metadata().is_err())
        .map(Path::to_path_buf)
        .collect();
    missing.reverse();
    let mut made = Vec::new();
    for folder in missing {
        if let Err(error) = std::fs::create_dir(&folder) {
            remove_empty(&made);
            return Err(fail(format!("{}: {error}", folder.display())));
        }
        made.push(folder);
    }
    Ok(made)
}

/// Removes the folders of [`create_parents`] that are empty; others stay.
fn remove_empty(made: &[PathBuf]) {
    for folder in made.iter().rev() {
        if std::fs::remove_dir(folder).is_err() {
            break;
        }
    }
}

/// Adds a worktree inside the main one to the local excludes, so Working Changes, badges,
/// searches and the index do not take its files for untracked content.
fn hide_nested(main: &Path, common: &Path, place: &Path, known: &[PathBuf]) -> Result<()> {
    let only_worktrees = place.parent().is_some_and(|parent| {
        std::fs::read_dir(parent).is_ok_and(|entries| {
            entries
                .flatten()
                .all(|entry| entry.path() == place || known.contains(&entry.path()))
        })
    });
    match super::gitlocation::exclude_rule(main, place, only_worktrees) {
        Some(rule) => super::gitworktree::exclude_locally(common, &rule),
        None => Ok(()),
    }
}

/// What has to be unchanged for an undo of a created worktree.
struct CreatedWorktree {
    head: String,
    /// The branch that was created with it.
    branch: Option<String>,
    /// Included files, with the size and time needed to check an undo.
    copied: Vec<(PathBuf, u64, Option<std::time::SystemTime>)>,
    links: Vec<(PathBuf, PathBuf)>,
    /// Fallback-copied folders, with a content fingerprint for safe undo.
    copied_folders: Vec<(PathBuf, u64)>,
    made: Vec<PathBuf>,
}

static CREATED: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, CreatedWorktree>>> =
    std::sync::Mutex::new(None);

fn created<T>(f: impl FnOnce(&mut std::collections::HashMap<PathBuf, CreatedWorktree>) -> T) -> T {
    let mut created = CREATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(created.get_or_insert_default())
}

/// Index refreshes of new worktrees that still run, by folder.
static REFRESHING: std::sync::Mutex<
    Option<std::collections::HashMap<PathBuf, std::thread::JoinHandle<()>>>,
> = std::sync::Mutex::new(None);

/// Settles the index of a new worktree in the background. Git for Windows compares file times in
/// whole seconds, so every file written in the second the checkout wrote the index (28 % of 25 000
/// files in a measured run) is "racily clean": the first `status` reads and hashes it, which took
/// 130 to 260 s on a machine with a virus scanner (a refresh of the settled index takes 0.5 s).
/// One `status` rewrites the index so that nothing is racy; it runs here, off the path of the
/// creation, instead of in the first view or the undo that asks for it. It is `status` and not
/// `update-index --refresh` because that holds `index.lock` for the whole run and would make other
/// Git commands in the new worktree fail meanwhile; `status` takes the lock only to write. The
/// result does not matter: a failure leaves the first status as slow as it was.
///
/// The cost is a delay per file (23 to 33 ms for a freshly written file, read for the first time),
/// not a throughput limit: reading the 7,823 racy files of a 25,001-file worktree with 16 threads
/// took 19 s and the `status` after that 0.4 s, against 130 to 260 s for the `status` alone. So the
/// racy files are read by several threads first ([`warm_racy_files`]), then `status` finds them cheap.
fn settle_index_in_background(place: &Path) {
    let folder = place.to_path_buf();
    let work = folder.clone();
    let handle = std::thread::spawn(move || {
        // Only on a Windows folder: elsewhere there is no delay per new file to wait out, and the
        // files of a repository in WSL are read through the `\\wsl.localhost` share, which made
        // this 311 s instead of 20 s for the creation row in a measured run.
        if cfg!(windows) && super::gitexec::place(&work).is_none() {
            warm_racy_files(&work);
        }
        let _ = git(
            &work,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=no",
                "--ignore-submodules=all",
            ],
        );
    });
    let mut all = REFRESHING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    all.get_or_insert_default().insert(folder, handle);
}

/// Reads, with several threads, the tracked files of `place` that were written in the second the
/// index was written or later: the ones Git compares by content on its next `status`. Nothing is
/// changed and every failure is ignored; a file that cannot be read is Git's to report later.
pub fn warm_racy_files(place: &Path) {
    let Some(git_dir) = worktree_git_dir(place) else {
        return;
    };
    let seconds = |time: std::time::SystemTime| {
        time.duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    };
    let Ok(index) = std::fs::metadata(git_dir.join("index")).and_then(|m| m.modified()) else {
        return;
    };
    let index = seconds(index);
    let Ok(listed) = git(place, &["ls-files", "-z"]) else {
        return;
    };
    let racy: Vec<PathBuf> = listed
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(|name| place.join(name))
        .filter(|file| {
            std::fs::symlink_metadata(file).is_ok_and(|meta| {
                meta.is_file() && meta.modified().is_ok_and(|t| seconds(t) >= index)
            })
        })
        .collect();
    if racy.is_empty() {
        return;
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(2, 16)
        .min(racy.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let at = next.fetch_add(1, Ordering::Relaxed);
                let Some(file) = racy.get(at) else {
                    break;
                };
                if let Ok(mut open) = std::fs::File::open(file) {
                    let _ = std::io::copy(&mut open, &mut std::io::sink());
                }
            });
        }
    });
}

/// Waits for the background refresh of a new worktree, if one runs. An undo calls it so that its
/// own `status` does not hash the same files at the same time; the speed benchmark calls it so the
/// undo row measures the undo.
pub fn settle_index(place: &Path) {
    let handle = REFRESHING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .and_then(|all| all.remove(place));
    if let Some(handle) = handle {
        let _ = handle.join();
    }
}

/// Creates a worktree for `request.source` without a form: the folder comes from the template,
/// a nested one is added to the local excludes and the files that `.worktreeinclude` names are
/// copied into it; ignored folders can instead be linked. A branch that already has a worktree
/// answers with that one.
fn quick_worktree(
    path: &Path,
    request: &QuickWorktree,
    cancel: &AtomicBool,
    progress: &impl Fn(String),
) -> Result<QuickResult> {
    let repository = root(path)?;
    let p = repository.as_path();
    let (id, reference, new_branch, label) = match &request.source {
        WorktreeSource::Branch { name, id } => {
            (id, Some(format!("refs/heads/{name}")), None, name.clone())
        }
        WorktreeSource::Remote {
            full_name,
            name,
            id,
        } => {
            if !full_name.starts_with("refs/remotes/") {
                return Err(fail("Select a remote branch"));
            }
            (id, Some(full_name.clone()), Some(name), name.clone())
        }
        WorktreeSource::NewBranch { name, id } => (id, None, Some(name), name.clone()),
        WorktreeSource::Commit { id, label } => {
            let label = label
                .clone()
                .unwrap_or_else(|| id.chars().take(8).collect());
            (id, None, None, label)
        }
    };
    let (main, common) = main_and_common(p)?;
    let wanted = match &request.destination {
        Some(destination) => {
            let destination = PathBuf::from(destination);
            if !destination.is_absolute() {
                return Err(fail("Choose an absolute destination path"));
            }
            if destination.symlink_metadata().is_ok() {
                return Err(fail("Destination already exists; choose a new path"));
            }
            destination
        }
        None => {
            let template = template_for(&main, &request.template, &request.templates);
            super::gitlocation::expand(template, &main, &label)?
        }
    };
    super::gitlocation::check(&main, &common, &wanted)?;
    if let Some(name) = new_branch {
        branch_name(p, name)?;
    }
    let (listed, commit) = std::thread::scope(|scope| {
        let listed = scope.spawn(|| worktrees(p));
        let commit = match &reference {
            None => concrete_commit(p, id),
            Some(reference) => git(
                p,
                &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
            )
            .map(|value| value.trim().to_owned()),
        };
        (joined(listed), commit)
    });
    let (listed, commit) = (listed??, commit?);
    if !commit.eq_ignore_ascii_case(id) {
        return Err(fail(
            "This reference moved. Refresh and choose the commit again",
        ));
    }
    if let WorktreeSource::Branch { name, .. } = &request.source {
        if let Some(entry) = listed
            .iter()
            .find(|entry| entry.branch.as_deref() == Some(name))
        {
            return Ok(QuickResult {
                path: entry.path.clone(),
                existing: true,
                copied: 0,
                linked: 0,
                warnings: Vec::new(),
                log: String::new(),
            });
        }
    }
    let known: Vec<PathBuf> = listed
        .iter()
        .map(|entry| PathBuf::from(&entry.path))
        .collect();
    let place = super::gitlocation::free(&wanted, |candidate| {
        known.iter().any(|path| path == candidate)
    })?;
    let made = create_parents(&place)?;
    let add = || -> Result<PathBuf> {
        let place = new_destination(&place.to_string_lossy())?;
        let text = place
            .to_str()
            .ok_or_else(|| fail("Destination must be UTF-8"))?;
        let destination = super::gitexec::path_arg(p, Path::new(text))
            .to_string_lossy()
            .into_owned();
        let mut args = vec!["worktree", "add"];
        match &request.source {
            WorktreeSource::Branch { name, .. } => args.extend(["--", &destination, name]),
            WorktreeSource::Remote {
                full_name, name, ..
            } => args.extend(["--track", "-b", name, "--", &destination, full_name]),
            WorktreeSource::NewBranch { name, .. } => {
                args.extend(["--no-track", "-b", name, "--", &destination, &commit])
            }
            WorktreeSource::Commit { .. } => args.extend(["--detach", "--", &destination, &commit]),
        }
        run(p, &args, cancel, progress, 1024 * 1024)?;
        Ok(place)
    };
    let place = add().inspect_err(|_| remove_empty(&made))?;
    settle_index_in_background(&place);
    let mut warnings = Vec::new();
    if let Err(error) = hide_nested(&main, &common, &place, &known) {
        warnings.push(error.to_string());
    }
    let link = request
        .links
        .iter()
        .any(|(folder, on)| *on && Path::new(folder) == main);
    let included = copy_included(&main, &place, link, &mut warnings, cancel, progress);
    let copied = included.copied;
    let linked = included.linked;
    let after = request
        .commands
        .iter()
        .find(|(folder, _)| Path::new(folder) == main)
        .map_or(&[][..], |(_, command)| command.as_slice());
    let log = after_created(after, &main, &place, &label, &mut warnings, cancel);
    created(|all| {
        all.insert(
            place.clone(),
            CreatedWorktree {
                head: commit,
                branch: new_branch.cloned(),
                copied: included.items,
                links: included.links,
                copied_folders: included.folders,
                made,
            },
        )
    });
    Ok(QuickResult {
        path: place.to_string_lossy().into_owned(),
        existing: false,
        copied,
        linked,
        warnings,
        log,
    })
}

/// Runs the command of the repository in the new worktree, with `{path}`, `{branch}` and `{repo}` filled in. A failure is
/// a warning (the worktree exists), never a reason to undo it; the output is returned for the log.
fn after_created(
    command: &[String],
    main: &Path,
    place: &Path,
    label: &str,
    warnings: &mut Vec<String>,
    cancel: &AtomicBool,
) -> String {
    if command.is_empty() {
        return String::new();
    }
    let variables = std::collections::BTreeMap::from([
        ("path".to_owned(), place.to_string_lossy().into_owned()),
        ("branch".to_owned(), label.to_owned()),
        ("repo".to_owned(), main.to_string_lossy().into_owned()),
    ]);
    let argv = match super::gitcustom::expand(command, &variables) {
        Ok(argv) => argv,
        Err(error) => {
            warnings.push(format!(
                "The command after creating the worktree was not run: {error}"
            ));
            return String::new();
        }
    };
    match super::gitcustom::run_in(place, &argv, super::gitcustom::TIMEOUT, &|| {
        cancel.load(std::sync::atomic::Ordering::Relaxed)
    }) {
        Ok(outcome) => {
            if outcome.timed_out {
                warnings.push(
                    "The command after creating the worktree took too long and was stopped".into(),
                );
            } else if outcome.cancelled {
                warnings.push("The command after creating the worktree was cancelled".into());
            } else if outcome.code != Some(0) {
                warnings.push(format!(
                    "The command after creating the worktree failed (exit code {})",
                    outcome
                        .code
                        .map_or("unknown".into(), |code| code.to_string())
                ));
            }
            outcome.output
        }
        Err(error) => {
            warnings.push(format!(
                "The command after creating the worktree was not run: {error}"
            ));
            String::new()
        }
    }
}

const INCLUDE_FILE: &str = ".worktreeinclude";
const MAX_INCLUDED: usize = 200_000;

/// Whether `path` is itself a link: a symbolic link, or on Windows a junction, which is a reparse point that the standard
/// library does not call a symbolic link.
fn is_link(path: &Path) -> bool {
    let Ok(meta) = path.symlink_metadata() else {
        return false;
    };
    let symlink = meta.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        symlink || meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    symlink
}

/// Deletes the link itself and never what it points at: `remove_dir` on a directory link or junction, `remove_file` on a
/// symbolic link of a file. (`remove_dir_all` must never be used here: on some systems it follows the link.)
fn remove_link(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    if let Some(link) = super::gitexec::place(path) {
        return wsl_link_command(&link.distro, &["unlink", "--", &link.linux]).map(drop);
    }
    std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path))
}

#[cfg(windows)]
fn wsl_link_command(distro: &str, args: &[&str]) -> std::io::Result<String> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new(super::wsl::wsl_exe())
        .args(["-d", distro, "--exec"])
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(0x0800_0000)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    String::from_utf8(output.stdout).map_err(std::io::Error::other)
}

fn read_included_link(path: &Path) -> std::io::Result<PathBuf> {
    #[cfg(windows)]
    if let Some(link) = super::gitexec::place(path) {
        let target = wsl_link_command(&link.distro, &["readlink", "-fz", "--", &link.linux])?;
        let target = target.strip_suffix('\0').ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Incomplete WSL link target",
            )
        })?;
        return Ok(super::gitexec::host_path(path, target));
    }
    std::fs::read_link(path)
}

/// Makes `link` a link to the folder `target`: a symbolic link, or on Windows a junction (it needs no privilege, unlike a
/// symbolic link). A path with a character that `cmd` would treat as syntax inside quotes is refused, so the caller copies.
fn make_link(target: &Path, link: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(|e| fail(e.to_string()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Some(at) = super::gitexec::place(link) {
            let to = super::gitexec::place(target)
                .filter(|to| to.distro.eq_ignore_ascii_case(&at.distro))
                .ok_or_else(|| fail("A WSL link must stay inside its distribution"))?;
            return wsl_link_command(&at.distro, &["ln", "-sT", "--", &to.linux, &at.linux])
                .map(drop)
                .map_err(|e| fail(e.to_string()));
        }
        let (to, at) = (
            crate::nav::path::strip_verbatim(target.to_path_buf()),
            crate::nav::path::strip_verbatim(link.to_path_buf()),
        );
        if [&to, &at].iter().any(|path| {
            path.to_string_lossy().contains([
                '%', '"', '&', '|', '<', '>', '^', '(', ')', '!', '\r', '\n', '\0',
            ])
        }) {
            return Err(fail(
                "the path has a character that cannot be used for a junction",
            ));
        }
        let out = std::process::Command::new("cmd.exe")
            .args(["/D", "/V:OFF", "/C", "mklink", "/J"])
            .raw_arg(format!("\"{}\" \"{}\"", at.display(), to.display()))
            .stdin(Stdio::null())
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| fail(e.to_string()))?;
        if out.status.success() && is_link(link) {
            Ok(())
        } else {
            Err(fail(
                String::from_utf8_lossy(&out.stdout).trim().to_owned()
                    + &String::from_utf8_lossy(&out.stderr),
            ))
        }
    }
}

/// Before a worktree is removed, delete its ignored links into the main worktree so Git cannot remove their targets.
/// (`git worktree remove` once left the worktree folder behind with a junction inside.) Only the link is deleted.
fn included_links(main: &Path, place: &Path, listed: &str) -> Result<Vec<PathBuf>> {
    let main = crate::nav::path::canonicalize(main).map_err(|e| fail(e.to_string()))?;
    let mut links = std::collections::BTreeSet::new();
    for entry in listed.split('\0').filter(|entry| !entry.is_empty()) {
        let path: PathBuf = place
            .join(entry.trim_end_matches('/'))
            .components()
            .collect();
        // Git for Windows may report a directory junction's children instead of the junction.
        for path in path
            .ancestors()
            .take_while(|path| *path != place)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if !is_link(path) {
                continue;
            }
            let target =
                read_included_link(path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
            let target = crate::nav::path::canonicalize(
                &path
                    .parent()
                    .unwrap_or(place)
                    .join(crate::nav::path::strip_verbatim(target)),
            )
            .map_err(|e| fail(format!("{}: {e}", path.display())))?;
            if target.starts_with(&main) {
                links.insert(path.to_path_buf());
            }
            break;
        }
    }
    Ok(links.into_iter().collect())
}

fn ignored_entries(place: &Path) -> Result<String> {
    git(
        place,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
    )
}

/// Keep junction targets outside native removal and put the links back if removal fails.
fn remove_without_included_links<T>(
    main: &Path,
    place: &Path,
    remove: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let links = included_links(main, place, &ignored_entries(place)?)?;
    let mut removed = Vec::new();
    let result = (|| {
        for path in &links {
            let target =
                read_included_link(path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
            remove_link(path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
            removed.push((path, target));
        }
        remove()
    })();
    let Err(error) = result else {
        return result;
    };
    if !place.is_dir() {
        return Err(error);
    }
    let mut failures = Vec::new();
    for (path, target) in removed {
        if let Err(restore) = make_link(&target, path) {
            failures.push(format!("{}: {restore}", path.display()));
        }
    }
    if failures.is_empty() {
        Err(error)
    } else {
        Err(fail(format!(
            "{error}\nCould not restore worktree links:\n{}",
            failures.join("\n")
        )))
    }
}

/// Copies ignored files named in `.worktreeinclude` from the main worktree. Tracked and merely
/// untracked files are left out, and so is everything the new worktree would not ignore. Nothing
/// existing is replaced.
///
/// With `link`, a whole ignored folder that the file names is not copied but linked to the folder of the main worktree (a
/// symbolic link, on Windows a junction): no copy of a large `node_modules` or build folder, and the changes of one
/// worktree are those of the other. The folders are found with `--directory`, so a huge folder is one entry, not a
/// listing of every file in it. A folder that cannot be linked is copied.
#[derive(Default)]
struct IncludedFiles {
    items: Vec<(PathBuf, u64, Option<std::time::SystemTime>)>,
    links: Vec<(PathBuf, PathBuf)>,
    folders: Vec<(PathBuf, u64)>,
    copied: usize,
    linked: usize,
}

fn copied_folder_fingerprint(root: &Path) -> Result<u64> {
    fn add(path: &Path, hasher: &mut DefaultHasher) -> Result<()> {
        let metadata = path.symlink_metadata().map_err(|e| fail(e.to_string()))?;
        if is_link(path) {
            1u8.hash(hasher);
            std::fs::read_link(path)
                .map_err(|e| fail(e.to_string()))?
                .hash(hasher);
        } else if metadata.is_dir() {
            2u8.hash(hasher);
            let mut entries = std::fs::read_dir(path)
                .map_err(|e| fail(e.to_string()))?
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|e| fail(e.to_string()))?;
            entries.sort_by_key(|entry| entry.file_name());
            entries.len().hash(hasher);
            for entry in entries {
                entry.file_name().hash(hasher);
                add(&entry.path(), hasher)?;
            }
        } else if metadata.is_file() {
            3u8.hash(hasher);
            metadata.len().hash(hasher);
            let mut file = std::fs::File::open(path).map_err(|e| fail(e.to_string()))?;
            let mut buffer = [0; 64 * 1024];
            loop {
                let count = file.read(&mut buffer).map_err(|e| fail(e.to_string()))?;
                if count == 0 {
                    break;
                }
                hasher.write(&buffer[..count]);
            }
        } else {
            4u8.hash(hasher);
            metadata.len().hash(hasher);
        }
        Ok(())
    }

    let mut hasher = DefaultHasher::new();
    add(root, &mut hasher)?;
    Ok(hasher.finish())
}

fn record_link(place: &Path, name: &str, included: &mut IncludedFiles, warnings: &mut Vec<String>) {
    let link = place.join(name);
    match read_included_link(&link) {
        Ok(target) => {
            included.links.push((link, target));
            included.linked += 1;
        }
        Err(error) => warnings.push(format!("{name}: could not record link for undo: {error}")),
    }
}

fn copy_included(
    main: &Path,
    place: &Path,
    link: bool,
    warnings: &mut Vec<String>,
    cancel: &AtomicBool,
    progress: &impl Fn(String),
) -> IncludedFiles {
    let mut included = IncludedFiles::default();
    if !main
        .join(INCLUDE_FILE)
        .symlink_metadata()
        .is_ok_and(|meta| meta.is_file() && meta.len() <= 64 * 1024)
    {
        return included;
    }
    // A name that is not UTF-8 is left out; a folder (with `--directory`) or a nested repository ends in a slash.
    let listed = |dir: &Path, rest: &[&str]| -> Result<Vec<String>> {
        let mut args = vec!["ls-files", "--others", "-z"];
        args.extend(rest);
        Ok(git(dir, &args)?
            .split('\0')
            .filter(|name| !name.is_empty() && !name.contains('\u{fffd}'))
            .map(str::to_owned)
            .collect())
    };
    let untracked = |dir: &Path, rest: &[&str]| -> Result<Vec<String>> {
        Ok(listed(dir, rest)?
            .into_iter()
            .filter(|name| !name.ends_with('/'))
            .collect())
    };
    let from = format!("--exclude-from={INCLUDE_FILE}");
    let folders_too: &[&str] = if link { &["--directory"] } else { &[] };
    let (named, visible) = std::thread::scope(|scope| {
        let visible = scope.spawn(|| {
            let mut rest = vec!["--exclude-standard"];
            rest.extend_from_slice(folders_too);
            listed(main, &rest)
        });
        let mut rest = vec!["--ignored", from.as_str()];
        rest.extend_from_slice(folders_too);
        (listed(main, &rest), joined(visible))
    });
    let (named, visible) = match (named, visible.and_then(|visible| visible)) {
        (Ok(named), Ok(visible)) => (named, visible),
        (Err(error), _) | (_, Err(error)) => {
            warnings.push(format!("{INCLUDE_FILE}: {error}"));
            return included;
        }
    };
    let visible: std::collections::HashSet<String> = visible.into_iter().collect();
    // Without `link` a folder is never listed (its files are), and a nested repository is left out in both modes.
    let (folders, named): (Vec<String>, Vec<String>) =
        named.into_iter().partition(|name| name.ends_with('/'));
    if link {
        let folders: Vec<String> = folders
            .into_iter()
            .filter(|name| {
                !visible.contains(name)
                    && !main.join(name.trim_end_matches('/')).join(".git").exists()
            })
            .collect();
        let folders = link_folders(main, place, &folders, warnings, cancel);
        included.copied += folders.copied;
        included.linked += folders.linked;
        included.items.extend(folders.items);
        included.links.extend(folders.links);
        included.folders.extend(folders.folders);
    }
    if named.len() > MAX_INCLUDED {
        warnings.push(format!(
            "{INCLUDE_FILE} names more than {MAX_INCLUDED} files; none was copied"
        ));
        return included;
    }
    let files: Vec<String> = named
        .into_iter()
        .filter(|name| !visible.contains(name))
        .collect();
    let total_files = files.len() as u64;
    if total_files > 0 {
        super::gitprogress::report_counts(
            "Copying .worktreeinclude",
            0,
            Some(total_files),
            "files",
            None,
        );
    }
    let mut names = Vec::new();
    for (idx, name) in files.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            warnings.push(format!("Copying the files of {INCLUDE_FILE} was cancelled"));
            break;
        }
        let target = place.join(&name);
        let copy = || -> Result<()> {
            // The checked out commit may hold a link where the main worktree has a folder.
            no_symlink_parents(place, &name)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| fail(e.to_string()))?;
            }
            super::ops::copy_item(&main.join(&name), &target, cancel)
        };
        // A file the commit brings is not replaced.
        if target.symlink_metadata().is_ok() {
            super::gitprogress::report_counts(
                "Copying .worktreeinclude",
                (idx + 1) as u64,
                Some(total_files),
                "files",
                Some(name),
            );
            continue;
        }
        let detail_name = name.clone();
        match copy() {
            Ok(()) => names.push(name),
            Err(error) => warnings.push(format!("{name}: {error}")),
        }
        super::gitprogress::report_counts(
            "Copying .worktreeinclude",
            (idx + 1) as u64,
            Some(total_files),
            "files",
            Some(detail_name),
        );
        if names.len() % 500 == 499 {
            progress(format!("{INCLUDE_FILE}: {}\n", names.len()));
        }
    }
    if names.is_empty() {
        return included;
    }
    // What this commit does not ignore would show up as a change of the new worktree.
    match untracked(place, &["--exclude-standard"]) {
        Ok(shown) => {
            let shown: std::collections::HashSet<String> = shown.into_iter().collect();
            names.retain(|name| {
                let keep = !shown.contains(name);
                if !keep {
                    let _ = std::fs::remove_file(place.join(name));
                    warnings.push(format!("{name}: not ignored at this commit, not copied"));
                }
                keep
            });
        }
        Err(error) => warnings.push(format!("{INCLUDE_FILE}: {error}")),
    }
    for name in names {
        let file = place.join(name);
        if let Ok(meta) = file.symlink_metadata() {
            included
                .items
                .push((file, meta.len(), meta.modified().ok()));
            included.copied += 1;
        }
    }
    included
}

/// Links the ignored folders `folders` (with a trailing slash, as `ls-files --directory` lists them) of the main worktree
/// into the new one. A folder the commit already has is left alone; one that cannot be linked is copied, with a warning.
/// A link that the new worktree would show as untracked (a symbolic link is no folder to a rule like `node_modules/`) goes
/// into that worktree's local excludes, and one that still shows is removed again. Returns the links with the size and time
/// they have now, which an undo compares with. Folders copied after a failed link are included too.
fn link_folders(
    main: &Path,
    place: &Path,
    folders: &[String],
    warnings: &mut Vec<String>,
    cancel: &AtomicBool,
) -> IncludedFiles {
    let mut made: Vec<String> = Vec::new();
    let mut included = IncludedFiles::default();
    let total_folders = folders.len() as u64;
    if total_folders > 0 {
        super::gitprogress::report_counts(
            "Linking .worktreeinclude",
            0,
            Some(total_folders),
            "folders",
            None,
        );
    }
    for (idx, folder) in folders.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            warnings.push(format!(
                "Linking the folders of {INCLUDE_FILE} was cancelled"
            ));
            break;
        }
        let name = folder.trim_end_matches('/');
        super::gitprogress::report_counts(
            "Linking .worktreeinclude",
            (idx + 1) as u64,
            Some(total_folders),
            "folders",
            Some(name.to_owned()),
        );
        let (source, target) = (main.join(name), place.join(name));
        // A folder the commit brings is not replaced.
        if target.symlink_metadata().is_ok() {
            continue;
        }
        let prepared = no_symlink_parents(place, name).and_then(|()| match target.parent() {
            Some(parent) => std::fs::create_dir_all(parent).map_err(|e| fail(e.to_string())),
            None => Ok(()),
        });
        if let Err(error) = prepared {
            warnings.push(format!("{name}: {error}"));
            continue;
        }
        match make_link(&source, &target) {
            Ok(()) => made.push(name.to_owned()),
            Err(reason) => {
                let fallback = if is_link(&target) {
                    remove_link(&target).map_err(|error| fail(error.to_string()))
                } else {
                    match target.symlink_metadata() {
                        Ok(_) => Err(fail("destination appeared while creating the link")),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(fail(error.to_string())),
                    }
                }
                .and_then(|()| super::ops::copy_item(&source, &target, cancel));
                match fallback {
                    Ok(()) => {
                        warnings.push(format!(
                            "{name}: could not be linked ({reason}); copied instead"
                        ));
                        included.copied += 1;
                        match copied_folder_fingerprint(&target) {
                            Ok(fingerprint) => {
                                included.folders.push((target, fingerprint));
                            }
                            Err(error) => warnings.push(format!(
                                "{name}: copied folder could not be verified for undo: {error}"
                            )),
                        }
                    }
                    Err(error) => warnings.push(format!("{name}: {error}")),
                }
            }
        }
    }
    if made.is_empty() {
        return included;
    }
    {
        let shown = |place: &Path| -> Result<std::collections::HashSet<String>> {
            Ok(git(
                place,
                &[
                    "ls-files",
                    "--others",
                    "--exclude-standard",
                    "--directory",
                    "-z",
                ],
            )?
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.trim_end_matches('/').to_owned())
            .collect())
        };
        let visible = match shown(place) {
            Ok(visible) => visible,
            Err(error) => {
                warnings.push(format!("Could not check linked folders: {error}"));
                for name in &made {
                    if let Err(error) = remove_link(&place.join(name)) {
                        warnings.push(format!("{name}: could not remove link: {error}"));
                        record_link(place, name, &mut included, warnings);
                    }
                }
                return included;
            }
        };
        for name in visible.iter().filter(|entry| made.contains(entry)) {
            let rule = format!("/{}", super::gitworktree::escape_rule(name));
            // Git reads info/exclude from the common directory, not from worktrees/<name>.
            let excluded = common_dir(place)
                .and_then(|common| super::gitworktree::exclude_locally(&common, &rule));
            if let Err(error) = excluded {
                warnings.push(format!("{name}: {error}"));
            }
        }
        // What is still shown would be a change of the new worktree: take the link back.
        let still = match shown(place) {
            Ok(still) => still,
            Err(error) => {
                warnings.push(format!("Could not verify linked folders: {error}"));
                for name in &made {
                    if let Err(error) = remove_link(&place.join(name)) {
                        warnings.push(format!("{name}: could not remove link: {error}"));
                        record_link(place, name, &mut included, warnings);
                    }
                }
                return included;
            }
        };
        for name in made {
            if still.contains(&name) {
                match remove_link(&place.join(&name)) {
                    Ok(()) => warnings.push(format!("{name}: not ignored here, not linked")),
                    Err(error) => {
                        warnings.push(format!(
                            "{name}: not ignored here and could not remove link: {error}"
                        ));
                        record_link(place, &name, &mut included, warnings);
                    }
                }
            } else {
                record_link(place, &name, &mut included, warnings);
            }
        }
    }
    included
}

/// Removes a worktree that [`quick_worktree`] created in this session, and the branch that came
/// with it, unless anything changed in it since: the commit, a file, or a file it did not copy.
fn undo_worktree(
    path: &Path,
    destination: &str,
    cancel: &AtomicBool,
    progress: &impl Fn(String),
) -> Result<String> {
    let p = root(path)?;
    let place = PathBuf::from(destination);
    let changed = || {
        fail("This worktree changed since it was created. Remove it in the Worktrees tab instead")
    };
    let check = |entry: &CreatedWorktree| -> Result<()> {
        let listed = worktrees(&p)?;
        let found = listed
            .iter()
            .find(|tree| Path::new(&tree.path) == place)
            .ok_or_else(changed)?;
        if found.locked
            || found.head != entry.head
            || entry
                .branch
                .as_ref()
                .is_some_and(|name| found.branch.as_ref() != Some(name))
        {
            return Err(changed());
        }
        let git_dir = worktree_git_dir(&place).ok_or_else(changed)?;
        let operations = [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_START",
            "rebase-merge",
            "rebase-apply",
            "sequencer",
        ];
        if operations.iter().any(|name| git_dir.join(name).exists()) {
            return Err(fail("Finish or abort the current Git operation first"));
        }
        // One status for everything: tracked changes, new files and each ignored file.
        let status = git(
            &place,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignored=traditional",
                "--ignore-submodules=none",
            ],
        )?;
        let copies: std::collections::HashMap<&Path, _> = entry
            .copied
            .iter()
            .map(|(file, size, time)| (file.as_path(), (*size, *time)))
            .collect();
        let folders: std::collections::HashMap<&Path, _> = entry
            .copied_folders
            .iter()
            .map(|(folder, fingerprint)| (folder.as_path(), *fingerprint))
            .collect();
        let changed_path = |name: &Path| {
            fail(format!(
                "Undo would remove local changes at {}. Review the worktree in the Worktrees tab",
                name.display()
            ))
        };
        for (folder, fingerprint) in &folders {
            if !copied_folder_fingerprint(folder).is_ok_and(|actual| actual == *fingerprint) {
                return Err(changed_path(folder));
            }
        }
        for record in status
            .split_terminator('\0')
            .filter(|record| !record.is_empty())
        {
            if let Some(name) = record.strip_prefix("!! ") {
                // Normalize Git's "/" separators before comparing with Windows paths.
                let file: PathBuf = place.join(name).components().collect();
                let unchanged_copy = copies.get(file.as_path()).is_some_and(|(size, time)| {
                    file.symlink_metadata()
                        .is_ok_and(|meta| *size == meta.len() && *time == meta.modified().ok())
                });
                let unchanged_link = entry.links.iter().any(|(link, target)| {
                    file.starts_with(link)
                        && is_link(link)
                        && read_included_link(link).is_ok_and(|actual| actual == *target)
                });
                if unchanged_copy
                    || folders.keys().any(|folder| file.starts_with(folder))
                    || unchanged_link
                {
                    continue;
                }
            }
            let name = record.get(3..).ok_or_else(changed)?;
            return Err(changed_path(Path::new(name)));
        }
        verify_filtered_checkout_bytes(&place, cancel)
    };
    settle_index(&place);
    let entry = created(|all| all.remove(&place)).ok_or_else(|| {
        fail("This worktree can no longer be undone. Remove it in the Worktrees tab instead")
    })?;
    if let Err(error) = check(&entry) {
        // Cleaned up later, it can still be undone.
        created(|all| all.insert(place.clone(), entry));
        return Err(error);
    }
    let argument = super::gitexec::path_arg(&p, &place)
        .to_string_lossy()
        .into_owned();
    let remove = || {
        run(
            &p,
            &["worktree", "remove", "--", &argument],
            cancel,
            progress,
            1024 * 1024,
        )
        .map(|_| String::new())
    };
    let removed = with_dormant_module_metadata(&place, WorktreeRelocation::Remove, || {
        if entry.links.is_empty() {
            remove()
        } else {
            main_and_common(&p)
                .and_then(|(main, _)| remove_without_included_links(&main, &place, remove))
        }
    });
    let output = match removed {
        Ok(output) => output,
        Err(error) => {
            created(|all| all.insert(place.clone(), entry));
            return Err(error);
        }
    };
    remove_empty(&entry.made);
    // The worktree had this branch checked out at `head` a moment ago, so the branch still
    // points where it was created.
    if let Some(name) = &entry.branch {
        git(&p, &["branch", "-D", "--", name])?;
    }
    Ok(output)
}

/// The folder of a listed worktree that may be removed or moved from here.
fn removable(state: &AdvancedSnapshot, current: &Path, destination: &str) -> Result<PathBuf> {
    let selected = state
        .worktrees
        .iter()
        .find(|entry| entry.path == destination)
        .ok_or_else(|| fail("Select an existing worktree"))?;
    let place = PathBuf::from(&selected.path);
    let main = state
        .worktrees
        .first()
        .is_some_and(|first| first.path == selected.path);
    if selected.bare || main || place.canonicalize().ok() == current.canonicalize().ok() {
        return Err(fail(
            "The current or main worktree cannot be removed or moved here",
        ));
    }
    if selected.locked {
        return Err(fail("This worktree is locked. Unlock it first"));
    }
    Ok(place)
}

fn require_deinitialized_submodules(place: &Path) -> Result<()> {
    let index = git(place, &["ls-files", "--stage", "-z"])?;
    for record in index.split_terminator('\0') {
        let Some((metadata, name)) = record.split_once('\t') else {
            continue;
        };
        if !metadata.starts_with("160000 ") {
            continue;
        }
        match place.join(name).join(".git").symlink_metadata() {
            Ok(_) => return Err(fail(format!(
                "The initialized submodule at {name} has its own checkout. Move it out of the worktree or deinitialize it before moving or removing the worktree"
            ))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(fail(format!("Could not inspect submodule {name}: {error}"))),
        }
    }
    Ok(())
}

enum WorktreeRelocation {
    Move,
    Remove,
}

/// Git refuses moves and ordinary removals while the linked Git directory has module metadata.
fn with_dormant_module_metadata(
    place: &Path,
    operation: WorktreeRelocation,
    run: impl FnOnce() -> Result<String>,
) -> Result<String> {
    let directory = worktree_git_dir(place)
        .ok_or_else(|| fail("The worktree Git directory is not readable"))?;
    let modules = directory.join("modules");
    match modules.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return run(),
        Err(error) => return Err(fail(format!("{}: {error}", modules.display()))),
        Ok(_) => {}
    }

    require_deinitialized_submodules(place)?;
    let recovery = tempfile::Builder::new()
        .prefix("gall-worktree-modules-")
        .tempdir_in(common_dir(place)?)
        .map_err(|error| {
            fail(format!(
                "Could not create submodule recovery storage: {error}"
            ))
        })?;
    let destination = recovery.path().join("modules");
    std::fs::rename(&modules, &destination)
        .map_err(|error| fail(format!("Could not preserve submodule metadata: {error}")))?;
    // From here on the directory owns user data, so it must survive an interruption.
    let recovery = recovery.keep();
    let retained = || format!("Submodule Git data is kept in {}", destination.display());
    let result = run();
    if (matches!(operation, WorktreeRelocation::Move) || result.is_err())
        && directory.is_dir()
        && std::fs::rename(&destination, &modules).is_ok()
    {
        let _ = std::fs::remove_dir(recovery);
        return result;
    }
    match result {
        Ok(output) => Ok(format!("{output}\n{}", retained())),
        Err(error) => Err(fail(format!("{error}\n{}", retained()))),
    }
}

/// Removes a worktree with local changes after keeping them. It never forces: once the changes
/// are stored the worktree is clean, and Git itself refuses when anything is left in it.
fn remove_with_snapshot(
    p: &Path,
    place: &Path,
    ignored: bool,
    cancel: &AtomicBool,
    invoke: impl Fn(&[&str]) -> Result<String>,
) -> Result<String> {
    require_idle_checkout(place)?;
    let filtered = has_content_filters(place);
    if filtered {
        if let Some(name) = first_checkout_change(place, "--ignore-submodules=none")? {
            return Err(fail(format!(
                "The recovery snapshot cannot guarantee the original filtered bytes at {name}. Commit the change or move the file outside the worktree before removing it"
            )));
        }
    }
    verify_filtered_checkout_bytes(place, cancel)?;
    // The snapshot holds neither of these, so they are checked before anything changes.
    if !git(
        place,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
    )?
    .is_empty()
        && !ignored
    {
        return Err(fail(
            "Untracked or ignored files are present. Move them outside this worktree before removing it",
        ));
    }
    require_deinitialized_submodules(place)?;
    super::gitgraph::retain_detached(place)?;
    let kept = if filtered {
        None
    } else {
        super::gitgraph::push_recovery(place, &[], "Gall recovery before removing a worktree")?
    };
    let removed = require_clean_checkout(place, "--ignore-submodules=none").and_then(|()| {
        let argument = super::gitexec::path_arg(p, place)
            .to_string_lossy()
            .into_owned();
        let remove = || invoke(&["worktree", "remove", "--", &argument]);
        with_dormant_module_metadata(place, WorktreeRelocation::Remove, || {
            if ignored {
                remove_without_included_links(&main_and_common(p)?.0, place, remove)
            } else {
                remove()
            }
        })
    });
    match (removed, kept) {
        (Ok(output), Some(id)) => Ok(format!(
            "{output}\nLocal changes are kept in refs/gall/recovery/{id}"
        )),
        (Ok(output), None) => Ok(output),
        (Err(error), None) => Err(error),
        (Err(error), Some(id)) => {
            // Still there: its changes go back where they were.
            if place.is_dir() && git(place, &["stash", "apply", "--index", "-q", &id]).is_ok() {
                return Err(error);
            }
            Err(fail(format!(
                "{error}\nThe local changes of the worktree are kept in refs/gall/recovery/{id}"
            )))
        }
    }
}

/// All callers must serialize writes and refresh the snapshot after success or failure.
pub fn execute(
    path: &Path,
    request: Request,
    expected: &str,
    cancel: &AtomicBool,
    progress: impl Fn(String),
) -> Result<String> {
    let _busy = super::gitfacts::mutating();
    if cancel.load(Ordering::Relaxed) {
        return Err(fail("Git operation cancelled before starting"));
    }
    // These two check exactly what they depend on (the commit of the source, the state of the
    // created worktree) instead of a token of the whole repository, which costs a full status.
    let request = match request {
        Request::QuickWorktree(quick) => {
            return serde_json::to_string(&quick_worktree(path, &quick, cancel, &progress)?)
                .map_err(|e| fail(e.to_string()))
        }
        Request::UndoWorktree { destination } => {
            return undo_worktree(path, &destination, cancel, &progress)
        }
        other => other,
    };
    let state = snapshot(path, None)?;
    if state.token != expected {
        return Err(fail(
            "Repository changed. Refresh and review this advanced operation again",
        ));
    }
    let current = state.repository.as_deref().map(Path::new);
    let p = current.unwrap_or(path);
    let worktree_action = match &request {
        Request::RemoveWorktree { .. } | Request::RemoveWorktreeWithIgnored { .. } => "remove",
        Request::LockWorktree { .. } => "lock",
        _ => "unlock",
    };
    let ignored_ok = matches!(&request, Request::RemoveWorktreeWithIgnored { .. });
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
            // Git inside WSL needs the destination as a Linux path.
            let destination_arg = super::gitexec::path_arg(p, Path::new(destination_text))
                .to_string_lossy()
                .into_owned();
            let source_arg = if Path::new(&url).is_absolute() {
                super::gitexec::path_arg(p, Path::new(&url))
                    .to_string_lossy()
                    .into_owned()
            } else {
                url
            };
            let output = match invoke(&[
                "clone",
                "--no-checkout",
                "--progress",
                "--",
                &source_arg,
                &destination_arg,
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
                super::gitprogress::report_phase("Checking out files", None);
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
        Request::QuickWorktree(_) | Request::UndoWorktree { .. } => {
            unreachable!("handled before the snapshot")
        }
        _ if current.is_none() => Err(fail("Initialize or clone a Git repository first")),
        Request::CreateWorktree {
            destination,
            target,
            branch,
        } => {
            let target = concrete_commit(p, &target)?;
            let (main, common) = main_and_common(p)?;
            let place = PathBuf::from(&destination);
            if !place.is_absolute() {
                return Err(fail("Choose an absolute destination path"));
            }
            super::gitlocation::check(&main, &common, &place)?;
            let made = create_parents(&place)?;
            let destination = new_destination(&destination).inspect_err(|_| remove_empty(&made))?;
            let Some(destination) = destination.to_str() else {
                remove_empty(&made);
                return Err(fail("Destination must be UTF-8"));
            };
            let destination = super::gitexec::path_arg(p, Path::new(destination))
                .to_string_lossy()
                .into_owned();
            let destination = destination.as_str();
            let mut args = vec!["worktree", "add"];
            if let Some(name) = &branch {
                if let Err(error) = branch_name(p, name) {
                    remove_empty(&made);
                    return Err(error);
                }
                args.extend(["-b", name]);
            } else {
                args.push("--detach");
            }
            args.extend(["--", destination, &target]);
            let output = invoke(&args).inspect_err(|_| remove_empty(&made))?;
            let known: Vec<PathBuf> = state
                .worktrees
                .iter()
                .map(|entry| PathBuf::from(&entry.path))
                .collect();
            match hide_nested(&main, &common, &place, &known) {
                Ok(()) => Ok(output),
                Err(error) => Ok(format!("{output}\n{error}")),
            }
        }
        Request::RemoveWorktreeWithSnapshot {
            destination,
            ignored,
        } => {
            let place = removable(&state, p, &destination)?;
            remove_with_snapshot(p, &place, ignored, cancel, |args| invoke(args))
        }
        Request::MoveWorktree { destination, to } => {
            let place = removable(&state, p, &destination)?;
            // Git moves the folder of a worktree with local changes too; only a running
            // operation and submodules are in its way.
            super::gitrepository::snapshot(&place).and_then(|state| {
                if state.operation.is_some() || !state.conflicts.is_empty() {
                    return Err(fail("Finish or abort the current Git operation first"));
                }
                Ok(())
            })?;
            let target = PathBuf::from(&to);
            if !target.is_absolute() {
                return Err(fail("Choose an absolute destination path"));
            }
            let (main, common) = main_and_common(p)?;
            super::gitlocation::check(&main, &common, &target)?;
            let made = create_parents(&target)?;
            let target = new_destination(&to).inspect_err(|_| remove_empty(&made))?;
            let argument = |path: &Path| {
                super::gitexec::path_arg(p, path)
                    .to_string_lossy()
                    .into_owned()
            };
            let (from, into) = (argument(&place), argument(&target));
            let output = with_dormant_module_metadata(&place, WorktreeRelocation::Move, || {
                invoke(&["worktree", "move", "--", &from, &into])
            })
            .inspect_err(|_| remove_empty(&made))?;
            let known: Vec<PathBuf> = state
                .worktrees
                .iter()
                .map(|entry| PathBuf::from(&entry.path))
                .collect();
            match hide_nested(&main, &common, &target, &known) {
                Ok(()) => Ok(output),
                Err(error) => Ok(format!("{output}\n{error}")),
            }
        }
        Request::RepairWorktree { destination } => {
            let place = PathBuf::from(&destination);
            // Only a linked worktree: `.git` is a file there.
            let git_dir = (place.is_absolute() && place.join(".git").is_file())
                .then(|| worktree_git_dir(&place))
                .flatten()
                .ok_or_else(|| fail("Choose the folder of a worktree"))?;
            // It names its entry in this repository, or nothing that exists (the repository was
            // moved). The worktree of another repository is never rewritten.
            let own = common_dir(p)?.join("worktrees");
            let named = git_dir
                .parent()
                .and_then(|parent| crate::nav::path::canonicalize(parent).ok());
            if git_dir.exists() && named.as_deref() != Some(own.as_path()) {
                return Err(fail("This worktree belongs to another repository"));
            }
            let argument = super::gitexec::path_arg(p, &place)
                .to_string_lossy()
                .into_owned();
            invoke(&["worktree", "repair", "--", &argument])
        }
        Request::PruneWorktrees => invoke(&["worktree", "prune", "--verbose"]),
        Request::RemoveWorktree { destination }
        | Request::RemoveWorktreeWithIgnored { destination }
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
                let main = main_and_common(p)?.0;
                if !ignored_ok {
                    if !git(
                        selected_path,
                        &["ls-files", "--others", "--exclude-standard", "-z"],
                    )?
                    .is_empty()
                    {
                        return Err(fail("Untracked files are present. Commit or move them outside this worktree before removing it"));
                    }
                    let listed = ignored_entries(selected_path)?;
                    let links = included_links(&main, selected_path, &listed)?;
                    if listed.split_terminator('\0').any(|name| {
                        let file: PathBuf = selected_path
                            .join(name.trim_end_matches('/'))
                            .components()
                            .collect();
                        !links.iter().any(|link| file.starts_with(link))
                    }) {
                        return Err(fail("Untracked or ignored files are present. Move them outside this worktree before removing it"));
                    }
                }
                require_idle_checkout(selected_path)?;
                require_clean_checkout(selected_path, "--ignore-submodules=none")?;
                verify_filtered_checkout_bytes(selected_path, cancel)?;
                let selected_arg = super::gitexec::path_arg(p, selected_path)
                    .to_string_lossy()
                    .into_owned();
                return with_dormant_module_metadata(
                    selected_path,
                    WorktreeRelocation::Remove,
                    || {
                        remove_without_included_links(&main, selected_path, || {
                            invoke(&["worktree", "remove", "--", &selected_arg])
                        })
                    },
                );
            }
            let selected_arg = super::gitexec::path_arg(p, selected_path)
                .to_string_lossy()
                .into_owned();
            invoke(&["worktree", worktree_action, "--", &selected_arg])
        }
        Request::DeinitSubmodule { file } => {
            let module = state
                .submodules
                .iter()
                .find(|entry| entry.path == file)
                .ok_or_else(|| fail("Select an existing submodule"))?;
            no_symlink_parents(p, &module.path)?;
            initialized_submodule(p, &module.path)?;
            let checkout = p.join(&module.path);
            require_idle_checkout(&checkout)?;
            require_clean_checkout(&checkout, "--ignore-submodules=none")?;
            require_deinitialized_submodules(&checkout)?;
            if let Some(name) = ignored_entries(&checkout)?.split_terminator('\0').next() {
                return Err(fail(format!(
                    "Deinitializing {} would remove the ignored path {name}. Move it outside the checkout before continuing.",
                    module.path
                )));
            }
            verify_filtered_checkout_bytes(&checkout, cancel)?;
            invoke(&[
                "--literal-pathspecs",
                "submodule",
                "deinit",
                "--",
                &module.path,
            ])
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
            // The checkout writes inside this module only. Keep unrelated parent changes in place.
            require_idle_checkout(p)?;
            let checkout = p.join(&module.path);
            if checkout.join(".git").exists() {
                initialized_submodule(p, &module.path)?;
                require_idle_checkout(&checkout)?;
                let target = match concrete_commit(&checkout, &module.id) {
                    Ok(target) => target,
                    Err(_) => fetch_submodule_commit(&checkout, &module.id, cancel, &progress)?,
                };
                if git(&checkout, &["rev-parse", "--verify", "HEAD"])?.trim() == target {
                    return Ok(String::new());
                }
                // The implicit submodule checkout can overwrite ignored files.
                super::gitprogress::report_phase("Updating submodule", Some(module.path.clone()));
                let mut switch_args = vec!["switch", "--detach"];
                if super::gitprogress::is_active() {
                    switch_args.push("--progress");
                }
                switch_args.extend(["--no-overwrite-ignore", &target]);
                let output = run(&checkout, &switch_args, cancel, &progress, 1024 * 1024)?;
                return Ok(String::from_utf8_lossy(&output).into_owned());
            } else {
                empty_submodule_destination(&checkout)?;
            }
            checked_url(&module.url)?;
            // An explicit checkout strategy cannot execute a configured custom
            // submodule update command. Never force or recurse into nested modules.
            super::gitprogress::report_phase("Updating submodule", Some(module.path.clone()));
            let mut sub_args = vec!["submodule", "update", "--init", "--checkout"];
            if super::gitprogress::is_active() {
                sub_args.push("--progress");
            }
            sub_args.extend(["--", &module.path]);
            invoke(&sub_args)
        }
        Request::BisectStart { good, bad } => {
            if state.bisect.is_some() {
                return Err(fail("A bisect is already active"));
            }
            // Build output and other untracked files, and changes the checkout does not touch,
            // are no reason to stop (they are what a bisect run leaves behind); `switch` refuses,
            // without changing anything, what it would overwrite.
            ready_worktree(p)?;
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
            ready_worktree(p)?;
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
        Request::ExportPatch { .. } | Request::ExportPatches { .. } => {
            let (commits, destination) = match request {
                Request::ExportPatch {
                    commit,
                    destination,
                } => (vec![commit], destination),
                Request::ExportPatches {
                    commits,
                    destination,
                } => (commits, destination),
                _ => unreachable!("only the two exports come here"),
            };
            if commits.is_empty() || commits.len() > 200 {
                return Err(fail("Select between 1 and 200 commits to export"));
            }
            let destination = new_destination(&destination)?;
            // Every patch is read before the file is created, so a refused commit leaves none.
            let mut bytes = Vec::new();
            let total_commits = commits.len() as u64;
            for (idx, commit) in commits.iter().enumerate() {
                let commit = concrete_commit(p, commit)?;
                super::gitprogress::report_counts(
                    "Exporting patches",
                    (idx + 1) as u64,
                    Some(total_commits),
                    "commits",
                    Some(commit[..8.min(commit.len())].to_owned()),
                );
                let patch = run(
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
                if patch.is_empty() {
                    return Err(fail(
                        "This commit has no single-parent patch to export; select a non-merge commit",
                    ));
                }
                bytes.extend(patch);
                if bytes.len() > 64 * 1024 * 1024 {
                    return Err(fail("The patches are larger than 64 MiB together"));
                }
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
            // `apply --check --index` below refuses, as one all-or-nothing step, a patch that
            // touches a changed file or writes a file that exists.
            ready_worktree(p)?;
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
            apply_reviewed_patch(p, &bytes, expected, &invoke)
        }
        Request::ApplyPatchText { text } => {
            if state.bisect.is_some() {
                return Err(fail("Reset the active bisect before applying a patch"));
            }
            ready_worktree(p)?;
            let bytes = clipboard_patch_bytes(&text)?;
            apply_reviewed_patch(p, &bytes, expected, &invoke)
        }
    }
}

/// Patch text from the clipboard as the bytes to apply. Some clipboards turn every line break into
/// CRLF; a patch whose own header line ends in CRLF was converted that way (a patch of a CRLF file
/// has LF-only headers and CRLF only inside its content lines), so it is restored to LF.
fn clipboard_patch_bytes(text: &str) -> Result<Vec<u8>> {
    if text.len() > 16 * 1024 * 1024 {
        return Err(fail("Patch exceeds the 16 MiB limit"));
    }
    let text = text.trim_start_matches('\u{feff}');
    let looks_like_patch = text.lines().any(|line| {
        line.starts_with("diff --git ")
            || line.starts_with("--- ")
            || line.starts_with("@@ ")
            || line.starts_with("GIT binary patch")
    });
    if !looks_like_patch {
        return Err(fail("The clipboard does not contain a patch"));
    }
    let first_line_is_crlf = text
        .split_once('\n')
        .is_some_and(|(line, _)| line.ends_with('\r'));
    let mut bytes = if first_line_is_crlf {
        text.replace("\r\n", "\n").into_bytes()
    } else {
        text.as_bytes().to_vec()
    };
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    Ok(bytes)
}

/// Checks the exact bytes against the index and working folder, then applies them to both. The
/// reviewed bytes stay immutable between `--check` and `--index`: a file supplied by another
/// application may be rewritten while Git runs.
fn apply_reviewed_patch(
    p: &Path,
    bytes: &[u8],
    expected: &str,
    invoke: &dyn Fn(&[&str]) -> Result<String>,
) -> Result<String> {
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
        .write_all(bytes)
        .map_err(|e| fail(e.to_string()))?;
    drop(temporary);
    let name = scratch
        .to_str()
        .ok_or_else(|| fail("Git directory must be UTF-8"))?;
    let name = super::gitexec::path_arg(p, Path::new(name))
        .to_string_lossy()
        .into_owned();
    let name = name.as_str();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn init(path: &Path) {
        crate::fs::hermetic_git();
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
    fn main_worktree_config_preserves_quoted_paths_and_rejects_ambiguity() {
        assert_eq!(
            configured_worktree("[core]\nworktree = ../../../module with spaces č\n"),
            Some("../../../module with spaces č".into())
        );
        assert_eq!(
            configured_worktree("[Core]\nworktree = \"../space \\\"quote\\\" \\t č \" ; comment\n"),
            Some("../space \"quote\" \t č ".into())
        );
        assert_eq!(
            configured_worktree("[core]\nworktree = \"C:\\\\source\\\\module\"\n"),
            Some("C:\\source\\module".into())
        );
        for config in [
            "[core]\nworktree = \"unterminated\n",
            "[core]\nworktree = ../bad\\q\n",
            "[core]\nworktree = ../one\n[include]\npath = other\n",
        ] {
            assert_eq!(configured_worktree(config), None);
        }
    }

    #[test]
    fn separate_git_directory_lists_its_actual_main_worktree() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("working folder");
        fs::create_dir(&root).unwrap();
        init(&root);
        fs::write(root.join("file.txt"), "content\n").unwrap();
        commit(&root, "First");
        let metadata = reported(directory.path()).join("separate metadata");
        git(
            &root,
            &[
                "init",
                "-q",
                "--separate-git-dir",
                metadata.to_str().unwrap(),
            ],
        )
        .unwrap();
        let trees = worktrees(&reported(&root)).unwrap();
        assert_eq!(trees.len(), 1);
        assert_eq!(Path::new(&trees[0].path), reported(&root));
    }

    #[test]
    fn a_separate_git_directory_named_dot_git_is_not_taken_for_the_main_checkout() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("working folder");
        fs::create_dir(&root).unwrap();
        init(&root);
        fs::write(root.join("file.txt"), "content\n").unwrap();
        commit(&root, "First");
        let store = directory.path().join("store");
        fs::create_dir(&store).unwrap();
        let metadata = reported(&store).join(".git");
        git(
            &root,
            &[
                "init",
                "-q",
                "--separate-git-dir",
                metadata.to_str().unwrap(),
            ],
        )
        .unwrap();
        let root = reported(&root);
        // The folder that holds the metadata is not the checkout.
        assert_eq!(main_and_common(&root).unwrap().0, root);
        let trees = worktrees(&root).unwrap();
        assert!(trees.iter().any(|tree| Path::new(&tree.path) == root));
        assert!(!trees
            .iter()
            .any(|tree| Path::new(&tree.path) == reported(&store)));
    }

    #[test]
    fn submodule_worktree_paths_identify_actual_folders_from_main_and_linked_trees() {
        let source = tempfile::tempdir().unwrap();
        init(source.path());
        fs::write(source.path().join("file.txt"), "content\n").unwrap();
        commit(source.path(), "Source");
        let parent = tempfile::tempdir().unwrap();
        init(parent.path());
        git(
            parent.path(),
            &[
                "-c",
                "protocol.file.allow=always",
                "-c",
                "core.autocrlf=false",
                "submodule",
                "add",
                "--",
                source.path().to_str().unwrap(),
                "module with spaces č",
            ],
        )
        .unwrap();
        commit(parent.path(), "Add submodule");
        let module = reported(&parent.path().join("module with spaces č"));
        git(&module, &["config", "core.autocrlf", "false"]).unwrap();
        let trees = worktrees(&module).unwrap();
        assert_eq!(trees.len(), 1);
        assert_eq!(Path::new(&trees[0].path), module);
        assert!(act(
            &module,
            Request::RemoveWorktree {
                destination: trees[0].path.clone(),
            },
        )
        .unwrap_err()
        .to_string()
        .contains("current or main worktree"));
        let linked = reported(parent.path()).join("linked module");
        git(
            &module,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "linked",
                linked.to_str().unwrap(),
            ],
        )
        .unwrap();
        for current in [&module, &linked] {
            let trees = worktrees(current).unwrap();
            assert_eq!(trees.len(), 2);
            assert_eq!(Path::new(&trees[0].path), module);
            assert_eq!(Path::new(&trees[1].path), linked);
        }
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

    /// Git 2.51 dropped the quotes around the term ("first bad commit").
    fn found_first_bad(log: &str) -> bool {
        log.contains("first 'bad' commit") || log.contains("first bad commit")
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
            if found_first_bad(&state.log) {
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
            found_first_bad(&state.log),
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
    fn bisect_moves_parent_gitlinks_without_rewriting_the_initialized_submodule() {
        let source = tempfile::tempdir().unwrap();
        init(source.path());
        fs::write(source.path().join("file.txt"), "first\n").unwrap();
        let first = commit(source.path(), "First module commit");
        fs::write(source.path().join("file.txt"), "second\n").unwrap();
        let second = commit(source.path(), "Second module commit");

        let parent = tempfile::tempdir().unwrap();
        let root = parent.path();
        init(root);
        let module_name = "module with spaces";
        git(
            root,
            &[
                "-c",
                "protocol.file.allow=always",
                "-c",
                "core.autocrlf=false",
                "submodule",
                "add",
                "--",
                source.path().to_str().unwrap(),
                module_name,
            ],
        )
        .unwrap();
        let module = root.join(module_name);
        git(&module, &["config", "core.autocrlf", "false"]).unwrap();
        fs::write(root.join("parent.txt"), "good\n").unwrap();
        let good = commit(root, "Good parent");
        git(&module, &["switch", "--detach", &first]).unwrap();
        fs::write(root.join("parent.txt"), "middle\n").unwrap();
        let middle = commit(root, "Middle parent");
        git(&module, &["switch", "--detach", &second]).unwrap();
        fs::write(root.join("parent.txt"), "bad\n").unwrap();
        let bad = commit(root, "Bad parent");
        git(root, &["config", "submodule.recurse", "true"]).unwrap();
        fs::write(module.join("file.txt"), "local module edit\n").unwrap();

        act(
            root,
            Request::BisectStart {
                good,
                bad: bad.clone(),
            },
        )
        .unwrap();
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), middle);
        assert_eq!(
            git(root, &["rev-parse", ":module with spaces"])
                .unwrap()
                .trim(),
            first
        );
        assert_eq!(git(&module, &["rev-parse", "HEAD"]).unwrap().trim(), second);
        assert_eq!(
            fs::read_to_string(module.join("file.txt")).unwrap(),
            "local module edit\n"
        );

        fs::write(root.join("parent.txt"), "local parent edit\n").unwrap();
        assert!(act(root, Request::BisectReset)
            .unwrap_err()
            .to_string()
            .contains("parent.txt"));
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), middle);
        assert!(snapshot(root, None).unwrap().bisect.is_some());
        assert_eq!(
            fs::read_to_string(root.join("parent.txt")).unwrap(),
            "local parent edit\n"
        );
        assert_eq!(
            fs::read_to_string(module.join("file.txt")).unwrap(),
            "local module edit\n"
        );

        fs::write(root.join("parent.txt"), "middle\n").unwrap();
        act(root, Request::BisectReset).unwrap();
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), bad);
        assert_eq!(
            git(root, &["rev-parse", ":module with spaces"])
                .unwrap()
                .trim(),
            second
        );
        assert_eq!(
            fs::read_to_string(module.join("file.txt")).unwrap(),
            "local module edit\n"
        );
    }

    #[test]
    fn bisect_native_checkout_uses_filters_and_preserves_overlapping_local_changes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        fs::write(root.join(".gitattributes"), "file.txt filter=roundtrip\n").unwrap();
        fs::write(root.join("notes.txt"), "original\n").unwrap();
        let mut ids = Vec::new();
        for version in 0..3 {
            fs::write(root.join("file.txt"), format!("repository {version}\n")).unwrap();
            ids.push(commit(root, &format!("Version {version}")));
        }
        for (key, value) in [
            ("filter.roundtrip.clean", "sed 's/working/repository/g'"),
            ("filter.roundtrip.smudge", "sed 's/repository/working/g'"),
            ("filter.roundtrip.required", "true"),
        ] {
            git(root, &["config", key, value]).unwrap();
        }
        fs::write(root.join("notes.txt"), "kept local\n").unwrap();
        fs::write(root.join("build.out"), "untracked\n").unwrap();
        act(
            root,
            Request::BisectStart {
                good: ids[0].clone(),
                bad: ids[2].clone(),
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "working 1\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("notes.txt")).unwrap(),
            "kept local\n"
        );

        fs::write(root.join("file.txt"), "working local\n").unwrap();
        let error = act(root, Request::BisectReset).unwrap_err();
        assert!(error.to_string().contains("file.txt"));
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "working local\n"
        );
        let active = snapshot(root, None).unwrap().bisect.unwrap();
        assert_eq!(active.current, ids[1]);

        fs::write(root.join("file.txt"), "working 1\n").unwrap();
        act(root, Request::BisectReset).unwrap();
        assert!(snapshot(root, None).unwrap().bisect.is_none());
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "working 2\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("build.out")).unwrap(),
            "untracked\n"
        );
    }

    /// A bisect leaves build output and edits behind; neither may stop the next step (lazygit and Git
    /// let them be), and a bisect step that would overwrite one is refused by Git, not by Gall.
    #[test]
    fn bisect_steps_run_with_build_output_and_unrelated_local_changes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        fs::write(root.join("notes.txt"), "notes\n").unwrap();
        let mut commits = Vec::new();
        for index in 0..6 {
            fs::write(root.join("file.txt"), format!("version {index}\n")).unwrap();
            commits.push(commit(root, &format!("Version {index}")));
        }
        fs::write(root.join("build.log"), "output of a test run\n").unwrap();
        fs::create_dir(root.join("target-dir")).unwrap();
        fs::write(root.join("target-dir").join("out.bin"), "artifact\n").unwrap();
        fs::write(root.join("notes.txt"), "my notes, edited\n").unwrap();
        act(
            root,
            Request::BisectStart {
                good: commits[0].clone(),
                bad: commits[5].clone(),
            },
        )
        .unwrap_or_else(|e| panic!("start: {e}"));
        for _ in 0..8 {
            let state = snapshot(root, None).unwrap().bisect.unwrap();
            if found_first_bad(&state.log) {
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
            .unwrap_or_else(|e| panic!("step at {position}: {e}"));
        }
        let state = snapshot(root, None).unwrap().bisect.unwrap();
        assert!(found_first_bad(&state.log), "{}", state.log);
        assert_eq!(state.current, commits[3]);
        act(root, Request::BisectReset).unwrap_or_else(|e| panic!("reset: {e}"));
        assert!(snapshot(root, None).unwrap().bisect.is_none());
        assert_eq!(
            fs::read_to_string(root.join("notes.txt")).unwrap(),
            "my notes, edited\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("build.log")).unwrap(),
            "output of a test run\n"
        );
        assert!(root.join("target-dir").join("out.bin").exists());
        // A step that would overwrite an untracked file is refused by Git and nothing moves.
        fs::write(root.join("future.txt"), "mine\n").unwrap();
        let bad = commits[5].clone();
        git(root, &["checkout", "-q", "-b", "later"]).unwrap();
        fs::remove_file(root.join("future.txt")).unwrap();
        fs::write(root.join("future.txt"), "tracked later\n").unwrap();
        git(root, &["add", "future.txt"]).unwrap();
        git(root, &["commit", "-qm", "adds future"]).unwrap();
        let tip = git(root, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        git(root, &["checkout", "-q", &bad]).unwrap();
        fs::write(root.join("future.txt"), "mine\n").unwrap();
        let started = act(
            root,
            Request::BisectStart {
                good: commits[0].clone(),
                bad: tip,
            },
        );
        assert_eq!(
            fs::read_to_string(root.join("future.txt")).unwrap(),
            "mine\n",
            "{started:?}"
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
        // Several commits, in the given order, in one file.
        let series = format!("{patch}.series");
        let export = |commits: Vec<String>, destination: &str| {
            act(
                root,
                Request::ExportPatches {
                    commits,
                    destination: destination.into(),
                },
            )
        };
        export(vec![first.clone(), second.clone()], &series).unwrap();
        let text = fs::read_to_string(&series).unwrap();
        assert_eq!(text.matches("\nSubject: ").count(), 2);
        assert!(text.find(first.as_str()).unwrap() < text.find(second.as_str()).unwrap());
        let refused = format!("{patch}.refused");
        assert!(export(vec![second.clone(), "0".repeat(40)], &refused).is_err());
        assert!(!Path::new(&refused).exists() && export(vec![], &refused).is_err());
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
    fn copied_commit_patch_applies_from_clipboard_text() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        fs::write(root.join("file.txt"), "first\n").unwrap();
        let first = commit(root, "First");
        fs::write(root.join("file.txt"), "second\n").unwrap();
        fs::write(root.join("added.txt"), "new\n").unwrap();
        let second = commit(root, "Second");
        let text = super::super::gitinspect::commit_patch_text(root, &second).unwrap();
        assert!(text.contains("Subject: [PATCH] Second") && text.contains("+second"));
        // A root commit has a patch too.
        assert!(super::super::gitinspect::commit_patch_text(root, &first)
            .unwrap()
            .contains("+first"));

        // Not a patch: refused before anything changes.
        git(root, &["checkout", "--detach", "-q", &first]).unwrap();
        assert!(act(
            root,
            Request::ApplyPatchText {
                text: "just some text".into()
            }
        )
        .is_err());
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "first\n"
        );

        // A clipboard that turned every line break into CRLF still applies.
        act(
            root,
            Request::ApplyPatchText {
                text: text.replace('\n', "\r\n"),
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "second\n"
        );
        assert_eq!(fs::read_to_string(root.join("added.txt")).unwrap(), "new\n");
        assert!(git(root, &["diff", "--cached", "--no-ext-diff"])
            .unwrap()
            .contains("+second"));
        assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), first);
        // The same patch cannot be applied twice.
        assert!(act(root, Request::ApplyPatchText { text }).is_err());
    }

    #[test]
    fn indexed_patch_updates_gitlinks_without_checkout_and_rejects_a_stale_pointer() {
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
                "-c",
                "core.autocrlf=false",
                "submodule",
                "add",
                "--",
                source.path().to_str().unwrap(),
                "module",
            ],
        )
        .unwrap();
        let module = root.join("module");
        git(&module, &["config", "core.autocrlf", "false"]).unwrap();
        let base = commit(root, "Parent at second");
        git(root, &["config", "submodule.recurse", "true"]).unwrap();
        fs::write(module.join("file.txt"), "local module content\n").unwrap();
        let patch = format!(
            "diff --git a/module b/module\nindex {second}..{first} 160000\n--- a/module\n+++ b/module\n@@ -1 +1 @@\n-Subproject commit {second}\n+Subproject commit {first}\n"
        );
        let patch_path = reported(source.path()).join("gitlink.patch");
        fs::write(&patch_path, &patch).unwrap();
        let file = || Request::ApplyPatch {
            file: patch_path.to_str().unwrap().into(),
        };
        let clipboard = || Request::ApplyPatchText {
            text: patch.clone(),
        };
        let assert_checkout_unchanged = || {
            assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), base);
            assert_eq!(git(&module, &["rev-parse", "HEAD"]).unwrap().trim(), second);
            assert_eq!(
                fs::read_to_string(module.join("file.txt")).unwrap(),
                "local module content\n"
            );
        };

        for request in [file(), clipboard()] {
            git(
                root,
                &[
                    "update-index",
                    "--cacheinfo",
                    &format!("160000,{second},module"),
                ],
            )
            .unwrap();
            act(root, request).unwrap();
            assert_eq!(git(root, &["rev-parse", ":module"]).unwrap().trim(), first);
            assert_checkout_unchanged();
        }
        for request in [file(), clipboard()] {
            assert!(act(root, request)
                .unwrap_err()
                .to_string()
                .contains("module"));
            assert_eq!(git(root, &["rev-parse", ":module"]).unwrap().trim(), first);
            assert_checkout_unchanged();
        }
    }

    #[test]
    fn indexed_patch_applies_filters_and_refuses_to_overwrite_edited_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        fs::write(root.join(".gitattributes"), "file.txt filter=roundtrip\n").unwrap();
        fs::write(root.join("file.txt"), "repository before\n").unwrap();
        let head = commit(root, "Base");
        for (key, value) in [
            ("filter.roundtrip.clean", "sed 's/working/repository/g'"),
            ("filter.roundtrip.smudge", "sed 's/repository/working/g'"),
            ("filter.roundtrip.required", "true"),
        ] {
            git(root, &["config", key, value]).unwrap();
        }
        let patch = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-repository before\n+repository after\n";
        let patch_path = reported(directory.path()).join("change.patch");
        fs::write(&patch_path, patch).unwrap();
        let file = || Request::ApplyPatch {
            file: patch_path.to_str().unwrap().into(),
        };
        let clipboard = || Request::ApplyPatchText { text: patch.into() };
        fs::write(root.join("unrelated.txt"), "keep me\n").unwrap();

        for request in [file(), clipboard()] {
            fs::write(root.join("file.txt"), "working before\n").unwrap();
            // As after a real checkout: the index remembers the working file (its smudged form), so Git does not take
            // its different size for a change; the blob stays the same because the clean filter maps it back.
            git(root, &["add", "--", "file.txt"]).unwrap();
            act(root, request).unwrap();
            assert_eq!(
                git(root, &["show", ":file.txt"]).unwrap(),
                "repository after\n"
            );
            assert_eq!(
                fs::read_to_string(root.join("file.txt")).unwrap(),
                "working after\n"
            );
            assert_eq!(git(root, &["rev-parse", "HEAD"]).unwrap().trim(), head);
            git(root, &["reset", "--hard", &head]).unwrap();
        }

        fs::write(root.join("file.txt"), "working local edit\n").unwrap();
        for request in [file(), clipboard()] {
            assert!(act(root, request)
                .unwrap_err()
                .to_string()
                .contains("file.txt"));
            assert_eq!(
                git(root, &["show", ":file.txt"]).unwrap(),
                "repository before\n"
            );
            assert_eq!(
                fs::read_to_string(root.join("file.txt")).unwrap(),
                "working local edit\n"
            );
        }
        assert_eq!(
            fs::read_to_string(root.join("unrelated.txt")).unwrap(),
            "keep me\n"
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
    fn submodule_tree_lists_nested_submodules_depth_first() {
        let allow = [
            "-c",
            "protocol.file.allow=always",
            "-c",
            "core.autocrlf=false",
        ];
        let add = |host: &Path, source: &Path, name: &str| {
            let mut args = allow.to_vec();
            args.extend(["submodule", "add", "--", source.to_str().unwrap(), name]);
            git(host, &args).unwrap();
        };
        let leaf = tempfile::tempdir().unwrap();
        init(leaf.path());
        fs::write(leaf.path().join("file.txt"), "leaf\n").unwrap();
        commit(leaf.path(), "Leaf");
        let middle = tempfile::tempdir().unwrap();
        init(middle.path());
        add(middle.path(), leaf.path(), "inner");
        commit(middle.path(), "Middle");
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path();
        init(root);
        add(root, middle.path(), "mid");
        add(root, leaf.path(), "zlast");
        commit(root, "Outer");
        let mut update = allow.to_vec();
        update.extend(["submodule", "update", "--init"]);
        git(&root.join("mid"), &update).unwrap();
        let tree = submodule_tree(root).unwrap();
        let listed: Vec<_> = tree
            .entries
            .iter()
            .map(|e| (e.label.as_str(), e.name.as_str(), e.depth))
            .collect();
        assert_eq!(
            listed,
            [
                ("mid", "mid", 0),
                ("mid/inner", "inner", 1),
                ("zlast", "zlast", 0)
            ]
        );
        assert!(tree.entries[1].path.ends_with("inner"));
        assert!(submodule_tree(outer.path().join("zlast").as_path())
            .unwrap()
            .entries
            .is_empty());
    }

    #[test]
    fn submodule_update_preserves_local_edits() {
        let source = tempfile::tempdir().unwrap();
        init(source.path());
        fs::write(source.path().join("file.txt"), "first\n").unwrap();
        fs::write(source.path().join("notes.txt"), "original module notes\n").unwrap();
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
        fs::write(root.join("parent-notes.txt"), "original\n").unwrap();
        commit(root, "Submodule");
        fs::write(root.join("parent-notes.txt"), "local parent edit\n").unwrap();
        fs::write(root.join("build output"), "keep\n").unwrap();
        git(root, &["config", "protocol.file.allow", "always"]).unwrap();
        let module = root.join("module with spaces");
        // The module is a clone: it does not inherit the repository-local setting.
        git(&module, &["config", "core.autocrlf", "false"]).unwrap();
        git(&module, &["checkout", "--detach", "-q", &first]).unwrap();
        assert_eq!(
            snapshot(root, None).unwrap().submodules[0].state,
            "differentCommit"
        );
        fs::write(module.join("notes.txt"), "unrelated module edit\n").unwrap();
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
        assert_eq!(
            fs::read_to_string(module.join("notes.txt")).unwrap(),
            "unrelated module edit\n"
        );
        fs::write(module.join("notes.txt"), "original module notes\n").unwrap();
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
        assert_eq!(
            fs::read_to_string(root.join("parent-notes.txt")).unwrap(),
            "local parent edit\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("build output")).unwrap(),
            "keep\n"
        );
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
        fs::write(exclude, "cache\ntree\nflat/\nbuild.log\n").unwrap();
        fs::write(module.join("build.log"), "unrelated ignored output").unwrap();
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
            assert!(error.contains(file.split('/').next().unwrap()), "{error}");
            assert_eq!(fs::read_to_string(&location).unwrap(), value);
            assert_eq!(git(&module, &["rev-parse", "HEAD"]).unwrap().trim(), first);
            fs::remove_file(location).unwrap();
            if file.contains('/') {
                fs::remove_dir(module.join("flat")).unwrap();
            }
        }
        fs::write(module.join("unrelated untracked"), "keep it").unwrap();
        act(
            p,
            Request::UpdateSubmodule {
                file: "module".into(),
            },
        )
        .unwrap();
        assert_eq!(git(&module, &["rev-parse", "HEAD"]).unwrap().trim(), target);
        assert_eq!(
            fs::read_to_string(module.join("build.log")).unwrap(),
            "unrelated ignored output"
        );
        assert_eq!(
            fs::read_to_string(module.join("unrelated untracked")).unwrap(),
            "keep it"
        );
    }

    #[test]
    fn submodule_update_fetches_missing_gitlink_before_a_guarded_checkout() {
        let source = tempfile::tempdir().unwrap();
        init(source.path());
        fs::write(source.path().join("base.txt"), "base\n").unwrap();
        let first = commit(source.path(), "Initial");
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path();
        init(root);
        let source_path = reported(source.path());
        git(
            root,
            &[
                "-c",
                "protocol.file.allow=always",
                "-c",
                "core.autocrlf=false",
                "submodule",
                "add",
                "--",
                source_path.to_str().unwrap(),
                "module",
            ],
        )
        .unwrap();
        commit(root, "Parent");
        let module = root.join("module");
        git(&module, &["config", "core.autocrlf", "false"]).unwrap();
        git(&module, &["config", "protocol.file.allow", "always"]).unwrap();
        fs::write(source.path().join("cache.txt"), "tracked now\n").unwrap();
        let target = commit(source.path(), "Track cache");
        assert!(concrete_commit(&module, &target).is_err());
        git(
            root,
            &[
                "update-index",
                "--cacheinfo",
                &format!("160000,{target},module"),
            ],
        )
        .unwrap();
        fs::write(
            git_path(&module, "info/exclude").unwrap(),
            "cache.txt\nbuild.log\n",
        )
        .unwrap();
        fs::write(module.join("cache.txt"), "valuable ignored contents\n").unwrap();
        fs::write(module.join("build.log"), "unrelated output\n").unwrap();
        let update = || {
            act(
                root,
                Request::UpdateSubmodule {
                    file: "module".into(),
                },
            )
        };

        let error = update().unwrap_err().to_string();
        assert!(error.contains("cache.txt"), "{error}");
        assert!(concrete_commit(&module, &target).is_ok());
        assert_eq!(git(&module, &["rev-parse", "HEAD"]).unwrap().trim(), first);
        assert_eq!(
            fs::read_to_string(module.join("cache.txt")).unwrap(),
            "valuable ignored contents\n"
        );

        fs::rename(module.join("cache.txt"), module.join("saved cache.txt")).unwrap();
        update().unwrap();
        assert_eq!(git(&module, &["rev-parse", "HEAD"]).unwrap().trim(), target);
        assert_eq!(
            fs::read_to_string(module.join("cache.txt")).unwrap(),
            "tracked now\n"
        );
        assert_eq!(
            fs::read_to_string(module.join("saved cache.txt")).unwrap(),
            "valuable ignored contents\n"
        );
        assert_eq!(
            fs::read_to_string(module.join("build.log")).unwrap(),
            "unrelated output\n"
        );
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

    fn quick(path: &Path, source: WorktreeSource, template: &str) -> Result<QuickResult> {
        quick_with_links(path, source, template, Default::default())
    }

    fn quick_with_links(
        path: &Path,
        source: WorktreeSource,
        template: &str,
        links: std::collections::BTreeMap<String, bool>,
    ) -> Result<QuickResult> {
        let request = Request::QuickWorktree(QuickWorktree {
            source,
            template: template.into(),
            templates: Default::default(),
            destination: None,
            commands: Default::default(),
            links,
        });
        execute(path, request, "", &AtomicBool::new(false), |_| {})
            .map(|json| serde_json::from_str(&json).unwrap())
    }

    fn undo(path: &Path, destination: &str) -> Result<String> {
        let request = Request::UndoWorktree {
            destination: destination.into(),
        };
        execute(path, request, "", &AtomicBool::new(false), |_| {})
    }

    /// A repository `app` with one commit and the branch `feature/x`; returns the commit.
    fn quick_repository(folder: &Path) -> (PathBuf, String) {
        let root = reported(folder).join("app");
        fs::create_dir(&root).unwrap();
        init(&root);
        fs::write(root.join("file.txt"), "one\n").unwrap();
        let id = commit(&root, "first");
        git(&root, &["branch", "feature/x"]).unwrap();
        (root, id)
    }

    fn branch_source(name: &str, id: &str) -> WorktreeSource {
        WorktreeSource::Branch {
            name: name.into(),
            id: id.into(),
        }
    }

    fn status_of(path: &Path) -> String {
        git(path, &["status", "--porcelain", "--untracked-files=all"]).unwrap()
    }

    #[test]
    fn worktree_creation_during_bisect_leaves_the_original_session_in_place() {
        let directory = tempfile::tempdir().unwrap();
        let (root, good) = quick_repository(directory.path());
        fs::write(root.join("file.txt"), "two\n").unwrap();
        commit(&root, "middle");
        fs::write(root.join("file.txt"), "three\n").unwrap();
        let bad = commit(&root, "last");
        act(
            &root,
            Request::BisectStart {
                good: good.clone(),
                bad,
            },
        )
        .unwrap();
        let before = snapshot(&root, None).unwrap().bisect.unwrap();

        let destination = reported(directory.path()).join("bisect companion");
        act(
            &root,
            Request::CreateWorktree {
                destination: destination.to_str().unwrap().into(),
                target: good.clone(),
                branch: None,
            },
        )
        .unwrap();
        let quick_result = quick(
            &root,
            WorktreeSource::Commit {
                id: good.clone(),
                label: Some("bisect quick companion".into()),
            },
            "",
        )
        .unwrap();
        for checkout in [&destination, Path::new(&quick_result.path)] {
            assert_eq!(git(checkout, &["rev-parse", "HEAD"]).unwrap().trim(), good);
            assert!(!git_path(checkout, "BISECT_START").unwrap().exists());
        }
        let after = snapshot(&root, None).unwrap().bisect.unwrap();
        assert_eq!(after.current, before.current);
        assert_eq!(after.candidate, before.candidate);
        assert_eq!(after.log, before.log);
        assert_eq!(
            fs::read_to_string(root.join("file.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "two\n"
        );
        act(&root, Request::BisectReset).unwrap();
    }

    #[test]
    fn quick_worktree_uses_the_preset_folder_and_stays_out_of_the_status() {
        let t = tempfile::tempdir().unwrap();
        let (root, id) = quick_repository(t.path());
        let made = quick(&root, branch_source("feature/x", &id), "").unwrap();
        let place = root.join(".worktrees").join("feature-x");
        assert_eq!(Path::new(&made.path), place);
        assert!(!made.existing && made.warnings.is_empty());
        assert_eq!(
            git(&place, &["symbolic-ref", "HEAD"]).unwrap().trim(),
            "refs/heads/feature/x"
        );
        // Not untracked content of the main worktree, and no tracked file was touched.
        assert_eq!(status_of(&root), "");
        assert!(!root.join(".gitignore").exists());
        let changes = crate::fs::gitworktree::workspace(&root).unwrap();
        assert!(changes.changes.is_empty());
        let exclude = || fs::read_to_string(root.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude().matches("/.worktrees/").count(), 1);
        // The same branch again answers with the worktree it has.
        let again = quick(&root, branch_source("feature/x", &id), "").unwrap();
        assert!(again.existing);
        assert_eq!(Path::new(&again.path), place);
        // From the linked worktree the folder is still the one of the main worktree.
        let detached = || WorktreeSource::Commit {
            id: id.clone(),
            label: Some("v1.0/rc".into()),
        };
        let second = quick(&place, detached(), "").unwrap();
        assert_eq!(
            Path::new(&second.path),
            root.join(".worktrees").join("v1.0-rc")
        );
        let third = quick(&root, detached(), "").unwrap();
        assert_eq!(
            Path::new(&third.path),
            root.join(".worktrees").join("v1.0-rc-2")
        );
        assert_eq!(status_of(&root), "");
        assert_eq!(exclude().matches(".worktrees").count(), 1);
    }

    #[test]
    fn explicit_and_quick_worktree_creation_and_undo_apply_configured_filters() {
        let directory = tempfile::tempdir().unwrap();
        let (root, _) = quick_repository(directory.path());
        fs::write(root.join(".gitattributes"), "file.txt filter=roundtrip\n").unwrap();
        fs::write(root.join("file.txt"), "repository bytes\n").unwrap();
        let id = commit(&root, "filtered file");
        git(
            &root,
            &[
                "config",
                "filter.roundtrip.clean",
                "sed '/^private/d; s/working/repository/g'",
            ],
        )
        .unwrap();
        git(
            &root,
            &[
                "config",
                "filter.roundtrip.smudge",
                "sed 's/repository/working/g'",
            ],
        )
        .unwrap();
        git(&root, &["config", "filter.roundtrip.required", "true"]).unwrap();
        fs::write(root.join("unrelated.tmp"), "keep me").unwrap();

        let destination = reported(directory.path()).join("filtered checkout");
        let create = |path: &Path| {
            act(
                &root,
                Request::CreateWorktree {
                    destination: path.to_str().unwrap().into(),
                    target: id.clone(),
                    branch: None,
                },
            )
        };
        create(&destination).unwrap();
        let source = |label: &str| WorktreeSource::Commit {
            id: id.clone(),
            label: Some(label.into()),
        };
        let quick_result = quick(&root, source("filtered quick checkout"), "").unwrap();
        for checkout in [&destination, Path::new(&quick_result.path)] {
            assert_eq!(
                fs::read_to_string(checkout.join("file.txt"))
                    .unwrap()
                    .replace("\r\n", "\n"),
                "working bytes\n"
            );
            assert!(status_of(checkout).is_empty());
        }
        assert_eq!(
            fs::read_to_string(root.join("unrelated.tmp")).unwrap(),
            "keep me"
        );

        let quick_path = Path::new(&quick_result.path);
        let filtered_file = quick_path.join("file.txt");
        let index = git(quick_path, &["ls-files", "--stage", "-z"]).unwrap();
        let filtered = filtered_index_files(quick_path, &index).unwrap();
        assert_eq!(
            filtered.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            ["file.txt"]
        );
        let cancelled = AtomicBool::new(true);
        assert!(verify_filtered_checkout_bytes(quick_path, &cancelled).is_err());
        assert!(quick_path.exists());
        let hidden = "working bytes\nprivate notes\n";
        fs::write(&filtered_file, hidden).unwrap();
        // Git must take this raw edit for a clean file (its clean filter drops it): refreshing the index records the
        // new stat the way a real checkout would, because Git 2.5x no longer compares by content when only the size of
        // an entry with a stat differs.
        git(quick_path, &["add", "--", "file.txt"]).unwrap();
        assert!(status_of(quick_path).is_empty());
        assert!(undo(&root, &quick_result.path)
            .unwrap_err()
            .to_string()
            .contains("file.txt"));
        assert_eq!(fs::read_to_string(&filtered_file).unwrap(), hidden);
        fs::write(&filtered_file, "edited working bytes\n").unwrap();
        let refused = undo(&root, &quick_result.path).unwrap_err();
        assert!(refused.to_string().contains("file.txt"));
        assert_eq!(
            fs::read_to_string(&filtered_file).unwrap(),
            "edited working bytes\n"
        );
        assert!(worktrees(&root)
            .unwrap()
            .iter()
            .any(|tree| tree.path == quick_result.path));

        fs::write(&filtered_file, "working bytes\n").unwrap();
        // The file is back to what the checkout wrote: refresh the index entry as the checkout had.
        git(quick_path, &["add", "--", "file.txt"]).unwrap();
        undo(&root, &quick_result.path).unwrap();
        assert!(!quick_path.exists());

        let explicit_path = destination.to_str().unwrap().to_owned();
        let explicit_file = destination.join("file.txt");
        let remove = || {
            act(
                &root,
                Request::RemoveWorktree {
                    destination: explicit_path.clone(),
                },
            )
        };
        let remove_with_recovery = || {
            act(
                &root,
                Request::RemoveWorktreeWithSnapshot {
                    destination: explicit_path.clone(),
                    ignored: false,
                },
            )
        };
        fs::write(&explicit_file, hidden).unwrap();
        git(&destination, &["add", "--", "file.txt"]).unwrap();
        assert!(status_of(&destination).is_empty());
        assert!(remove().unwrap_err().to_string().contains("file.txt"));
        assert!(remove_with_recovery()
            .unwrap_err()
            .to_string()
            .contains("file.txt"));
        assert_eq!(fs::read_to_string(&explicit_file).unwrap(), hidden);
        fs::write(&explicit_file, "edited working bytes\n").unwrap();
        assert!(remove().unwrap_err().to_string().contains("file.txt"));
        assert!(remove_with_recovery()
            .unwrap_err()
            .to_string()
            .contains("file.txt"));
        assert_eq!(
            fs::read_to_string(&explicit_file).unwrap(),
            "edited working bytes\n"
        );
        fs::write(&explicit_file, "working bytes\n").unwrap();
        git(&destination, &["add", "--", "file.txt"]).unwrap();
        remove_with_recovery().unwrap();
        assert!(!destination.exists());

        create(&destination).unwrap();
        remove().unwrap();
        assert!(!destination.exists());

        // Git rejects an occupied destination and a required filter failure. The main
        // worktree and the already-created worktree remain recoverable in both cases.
        let occupied = reported(directory.path()).join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("keep"), "untouched").unwrap();
        assert!(create(&occupied).is_err());
        assert_eq!(
            fs::read_to_string(occupied.join("keep")).unwrap(),
            "untouched"
        );

        git(&root, &["config", "filter.roundtrip.smudge", "false"]).unwrap();
        let registered = worktrees(&root).unwrap().len();
        assert!(create(&reported(directory.path()).join("failed checkout")).is_err());
        assert!(quick(&root, source("failed quick checkout"), "").is_err());
        assert_eq!(worktrees(&root).unwrap().len(), registered);
        assert_eq!(
            fs::read_to_string(root.join("unrelated.tmp")).unwrap(),
            "keep me"
        );
    }

    #[test]
    fn quick_worktree_creates_branches_and_refuses_a_moved_reference() {
        let t = tempfile::tempdir().unwrap();
        let (root, id) = quick_repository(t.path());
        // A remote branch without a remote server: the reference is all that is needed.
        git(
            &root,
            &["remote", "add", "origin", "https://example.invalid/app.git"],
        )
        .unwrap();
        git(&root, &["update-ref", "refs/remotes/origin/topic/a", &id]).unwrap();
        let made = quick(
            &root,
            WorktreeSource::Remote {
                full_name: "refs/remotes/origin/topic/a".into(),
                name: "topic/a".into(),
                id: id.clone(),
            },
            "../{repo}-{branch}",
        )
        .unwrap();
        assert_eq!(
            Path::new(&made.path),
            root.parent().unwrap().join("app-topic-a")
        );
        assert_eq!(
            git(&root, &["rev-parse", "--abbrev-ref", "topic/a@{upstream}"])
                .unwrap()
                .trim(),
            "origin/topic/a"
        );
        // Outside the repository nothing is excluded.
        let exclude = fs::read_to_string(root.join(".git/info/exclude")).unwrap_or_default();
        assert!(!exclude.contains("topic"));
        let fresh = |name: &str, id: &str| WorktreeSource::NewBranch {
            name: name.into(),
            id: id.into(),
        };
        let made = quick(&root, fresh("try/one", &id), "").unwrap();
        assert!(Path::new(&made.path).ends_with("try-one"));
        for (source, expected) in [
            (fresh("try/one", &id), "already exists"),
            (fresh("--bad", &id), "valid branch name"),
            (branch_source("feature/x", &"0".repeat(40)), "moved"),
        ] {
            let error = quick(&root, source, "").unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
        // A refused creation leaves no folder behind.
        for name in ["try-one-2", "bad", "feature-x"] {
            assert!(!root.join(".worktrees").join(name).exists(), "{name}");
        }
        for (template, expected) in [
            ("{repo}/.git/x/{branch}", "Git metadata"),
            ("{repo}/.worktrees/try-one/{branch}", "another worktree"),
            ("{repo}/x", "needs {branch}"),
        ] {
            let error = quick(&root, branch_source("feature/x", &id), template)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn a_failed_quick_worktree_removes_the_folders_it_made() {
        let t = tempfile::tempdir().unwrap();
        let (root, id) = quick_repository(t.path());
        let source = WorktreeSource::NewBranch {
            name: "feature/x".into(),
            id,
        };
        assert!(quick(&root, source, "{repo}/a/b/{branch}").is_err());
        assert!(!root.join("a").exists());
    }

    #[test]
    fn the_exclude_rule_never_hides_other_files_of_a_folder() {
        let t = tempfile::tempdir().unwrap();
        let (root, id) = quick_repository(t.path());
        fs::create_dir(root.join("docs")).unwrap();
        fs::write(root.join("docs/note.txt"), "mine\n").unwrap();
        quick(
            &root,
            branch_source("feature/x", &id),
            "{repo}/docs/{branch}",
        )
        .unwrap();
        assert_eq!(status_of(&root), "?? docs/note.txt\n");
    }

    #[test]
    fn undo_removes_only_an_unchanged_worktree_and_its_new_branch() {
        let t = tempfile::tempdir().unwrap();
        let (root, id) = quick_repository(t.path());
        let fresh = |name: &str| WorktreeSource::NewBranch {
            name: name.into(),
            id: id.clone(),
        };
        let has = |name: &str| {
            git(
                &root,
                &["rev-parse", "--verify", &format!("refs/heads/{name}")],
            )
            .is_ok()
        };
        let made = quick(&root, fresh("one"), "").unwrap();
        undo(&root, &made.path).unwrap();
        assert!(!root.join(".worktrees").exists() && !has("one"));
        assert!(undo(&root, &made.path).is_err(), "only once");
        // An existing branch is kept.
        let made = quick(&root, branch_source("feature/x", &id), "").unwrap();
        undo(&root, &made.path).unwrap();
        assert!(has("feature/x"));

        let made = quick(&root, fresh("two"), "").unwrap();
        let place = PathBuf::from(&made.path);
        let refused = || {
            assert!(undo(&root, &made.path).is_err());
            assert!(place.join("file.txt").exists() && has("two"));
        };
        fs::write(place.join("new.txt"), "x").unwrap();
        refused();
        fs::remove_file(place.join("new.txt")).unwrap();
        fs::write(place.join("file.txt"), "edited\n").unwrap();
        refused();
        fs::write(place.join("file.txt"), "one\n").unwrap();
        // An ignored file that Gall did not copy is the user's.
        let common = root.join(".git/info/exclude");
        let mut rules = fs::read_to_string(&common).unwrap();
        rules.push_str("\n*.log\n");
        fs::write(&common, rules).unwrap();
        fs::write(place.join("mine.log"), "x").unwrap();
        refused();
        fs::remove_file(place.join("mine.log")).unwrap();
        git(&place, &["commit", "-q", "--allow-empty", "-m", "work"]).unwrap();
        refused();
        git(&place, &["reset", "-q", "--hard", &id]).unwrap();
        undo(&root, &made.path).unwrap();
        assert!(!place.exists() && !has("two"));
    }

    fn ignored_folder_repository(folder: &Path) -> (PathBuf, String) {
        let (root, _) = quick_repository(folder);
        fs::write(root.join(".gitignore"), "cache/\n").unwrap();
        fs::write(root.join(INCLUDE_FILE), "cache/\n").unwrap();
        let id = commit(&root, "include ignored folder");
        fs::create_dir_all(root.join("cache/deep")).unwrap();
        fs::write(root.join("cache/deep/keep.txt"), "shared\n").unwrap();
        (root, id)
    }

    fn quick_linked(root: &Path, id: &str, label: &str) -> QuickResult {
        let links = [(root.to_string_lossy().into_owned(), true)].into();
        quick_with_links(
            root,
            WorktreeSource::Commit {
                id: id.into(),
                label: Some(label.into()),
            },
            "",
            links,
        )
        .unwrap()
    }

    #[test]
    fn worktreeinclude_links_ignored_folders_and_undo_preserves_source() {
        let t = tempfile::tempdir().unwrap();
        let (root, id) = ignored_folder_repository(t.path());
        let made = quick_linked(&root, &id, "linked");
        let place = PathBuf::from(&made.path);

        assert_eq!((made.copied, made.linked), (0, 1), "{made:?}");
        assert!(is_link(&place.join("cache")));
        assert_eq!(
            fs::read_to_string(place.join("cache/deep/keep.txt")).unwrap(),
            "shared\n"
        );
        assert_eq!(status_of(&root), "");
        assert_eq!(status_of(&place), "");
        #[cfg(unix)]
        {
            // Git reads `info/exclude` only from the common directory; a linked worktree has none.
            let shared_exclude = fs::read_to_string(root.join(".git/info/exclude")).unwrap();
            assert!(shared_exclude.contains("/cache"));
            assert!(!worktree_git_dir(&place)
                .unwrap()
                .join("info/exclude")
                .exists());
        }

        undo(&root, &made.path).unwrap();
        assert!(!place.exists());
        assert_eq!(
            fs::read_to_string(root.join("cache/deep/keep.txt")).unwrap(),
            "shared\n"
        );
        assert_eq!(status_of(&root), "");
    }

    #[cfg(windows)]
    #[test]
    fn unsafe_junction_path_falls_back_to_copy_and_undo_preserves_edits() {
        let t = tempfile::tempdir().unwrap();
        let folder = t.path().join("unsafe&path");
        fs::create_dir(&folder).unwrap();
        let (root, id) = ignored_folder_repository(&folder);
        let made = quick_linked(&root, &id, "fallback");
        let place = PathBuf::from(&made.path);

        assert_eq!((made.copied, made.linked), (1, 0));
        assert!(!is_link(&place.join("cache")));
        fs::write(place.join("cache/deep/keep.txt"), "local edit\n").unwrap();
        assert!(undo(&root, &made.path).is_err());
        assert_eq!(
            fs::read_to_string(place.join("cache/deep/keep.txt")).unwrap(),
            "local edit\n"
        );

        fs::write(place.join("cache/deep/keep.txt"), "shared\n").unwrap();
        undo(&root, &made.path).unwrap();
        assert!(!place.exists());
        assert_eq!(
            fs::read_to_string(root.join("cache/deep/keep.txt")).unwrap(),
            "shared\n"
        );
    }

    #[test]
    fn removing_linked_worktree_preserves_ignored_folder_in_main() {
        let t = tempfile::tempdir().unwrap();
        let (root, id) = ignored_folder_repository(t.path());
        let made = quick_linked(&root, &id, "remove-linked");
        let place = PathBuf::from(&made.path);
        assert!(is_link(&place.join("cache")));

        act(
            &root,
            Request::RemoveWorktree {
                destination: made.path,
            },
        )
        .unwrap();
        assert!(!place.exists());
        assert_eq!(
            fs::read_to_string(root.join("cache/deep/keep.txt")).unwrap(),
            "shared\n"
        );
        assert_eq!(status_of(&root), "");
    }

    #[test]
    fn worktree_removal_only_unlinks_shared_paths_and_restores_them_on_failure() {
        let t = tempfile::tempdir().unwrap();
        let main = reported(t.path()).join("main");
        fs::create_dir(&main).unwrap();
        init(&main);
        fs::create_dir_all(main.join("cache")).unwrap();
        fs::write(main.join("cache/keep.txt"), "keep\n").unwrap();
        fs::write(main.join(".worktreeinclude"), "cache\n").unwrap();
        commit(&main, "main");

        let outside = reported(t.path()).join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "outside\n").unwrap();
        let place = reported(t.path()).join("place");
        fs::create_dir(&place).unwrap();
        init(&place);
        fs::write(place.join(".gitignore"), "cache\nexternal\n").unwrap();
        commit(&place, "place");
        make_link(&main.join("cache"), &place.join("cache")).unwrap();
        make_link(&outside, &place.join("external")).unwrap();

        let error = remove_without_included_links(&main, &place, || {
            assert!(!is_link(&place.join("cache")));
            assert!(is_link(&place.join("external")));
            Err::<(), _>(fail("Git refused removal"))
        })
        .unwrap_err();
        assert!(error.to_string().contains("Git refused removal"));
        assert!(is_link(&place.join("cache")));
        remove_without_included_links(&main, &place, || Ok(())).unwrap();
        assert!(place.join("cache").symlink_metadata().is_err());
        assert!(is_link(&place.join("external")));
        assert!(main.join("cache/keep.txt").exists());
        assert!(outside.join("keep.txt").exists());
    }

    #[test]
    fn worktreeinclude_copies_ignored_files_only() {
        let t = tempfile::tempdir().unwrap();
        let (root, _) = quick_repository(t.path());
        fs::write(root.join(".gitignore"), ".env\ncache/\nsecret.txt\n").unwrap();
        fs::write(
            root.join(".worktreeinclude"),
            "# copied\n.env\ncache/keep/\nplain.txt\n",
        )
        .unwrap();
        let id = commit(&root, "rules");
        fs::write(root.join(".env"), "KEY=1\n").unwrap();
        fs::write(root.join("secret.txt"), "no").unwrap();
        fs::create_dir_all(root.join("cache/keep/deep")).unwrap();
        fs::write(root.join("cache/keep/deep/a"), "a").unwrap();
        fs::write(root.join("cache/other"), "b").unwrap();
        // Named, but a new file rather than an ignored one.
        fs::write(root.join("plain.txt"), "new").unwrap();
        let source = || WorktreeSource::Commit {
            id: id.clone(),
            label: None,
        };
        let made = quick(&root, source(), "").unwrap();
        let place = PathBuf::from(&made.path);
        assert!(made.warnings.is_empty(), "{:?}", made.warnings);
        assert_eq!(made.copied, 2);
        assert_eq!(fs::read_to_string(place.join(".env")).unwrap(), "KEY=1\n");
        assert!(place.join("cache/keep/deep/a").exists());
        for left in ["secret.txt", "cache/other", "plain.txt"] {
            assert!(!place.join(left).exists(), "{left}");
        }
        assert_eq!(status_of(&place), "");
        // Untouched copies do not prevent the undo, a changed one does.
        fs::write(place.join(".env"), "KEY=22\n").unwrap();
        assert!(undo(&root, &made.path).is_err());
        assert!(place.join(".env").exists());
        let second = quick(&root, source(), "").unwrap();
        undo(&root, &second.path).unwrap();
        assert!(!Path::new(&second.path).exists());
        assert_eq!(fs::read_to_string(root.join(".env")).unwrap(), "KEY=1\n");

        // At a commit without the rules the copies would be changes: they are not kept.
        let first = git(&root, &["rev-parse", "HEAD~1"]).unwrap();
        let old = WorktreeSource::Commit {
            id: first.trim().into(),
            label: Some("old".into()),
        };
        let made = quick(&root, old, "").unwrap();
        assert_eq!(made.copied, 0);
        assert_eq!(made.warnings.len(), 2);
        assert_eq!(status_of(Path::new(&made.path)), "");
    }

    #[cfg(unix)]
    #[test]
    fn worktreeinclude_never_writes_through_a_link_of_the_commit() {
        let t = tempfile::tempdir().unwrap();
        let (root, _) = quick_repository(t.path());
        let outside = reported(t.path()).join("outside");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("conf")).unwrap();
        fs::write(root.join(".gitignore"), "*.key\n").unwrap();
        let id = commit(&root, "link");
        fs::remove_file(root.join("conf")).unwrap();
        fs::create_dir(root.join("conf")).unwrap();
        fs::write(root.join("conf/a.key"), "k").unwrap();
        fs::write(root.join(".worktreeinclude"), "*.key\n").unwrap();
        let source = WorktreeSource::Commit { id, label: None };
        let made = quick(&root, source, "").unwrap();
        assert_eq!((made.copied, made.warnings.len()), (0, 1));
        assert!(!outside.join("a.key").exists());
    }

    #[test]
    fn a_location_is_previewed_without_creating_anything() {
        let t = tempfile::tempdir().unwrap();
        let (root, id) = quick_repository(t.path());
        let none = Default::default();
        let at = |template: &str| worktree_location(&root, template, &none, "feature/x").unwrap();
        let path = |place: WorktreeLocation| PathBuf::from(place.path.unwrap());
        assert_eq!(path(at("")), root.join(".worktrees").join("feature-x"));
        assert!(at("").inside && !root.join(".worktrees").exists());
        assert!(!at("../{repo}-{branch}").inside);
        let refused = at("{repo}/.git/{branch}");
        assert!(refused.path.is_none() && refused.error.is_some());
        assert_eq!(Path::new(&refused.main), root);
        // A template of this repository wins over the general one.
        let own: std::collections::BTreeMap<String, String> =
            [(root.to_string_lossy().into_owned(), "../{branch}".into())].into();
        let place = worktree_location(&root, "", &own, "y").unwrap();
        assert_eq!(path(place), root.parent().unwrap().join("y"));
        let request = |destination: Option<String>, name: &str| {
            Request::QuickWorktree(QuickWorktree {
                source: WorktreeSource::NewBranch {
                    name: name.into(),
                    id: id.clone(),
                },
                template: String::new(),
                templates: own.clone(),
                destination,
                commands: Default::default(),
                links: Default::default(),
            })
        };
        let run = |request| -> Result<QuickResult> {
            execute(&root, request, "", &AtomicBool::new(false), |_| {})
                .map(|json| serde_json::from_str(&json).unwrap())
        };
        let made = run(request(None, "own")).unwrap();
        assert_eq!(Path::new(&made.path), root.parent().unwrap().join("own"));
        // A typed folder is used exactly, and never replaced by another name.
        let typed = root.parent().unwrap().join("typed");
        let typed = typed.to_string_lossy().into_owned();
        assert_eq!(
            run(request(Some(typed.clone()), "typed")).unwrap().path,
            typed
        );
        assert!(run(request(Some(typed), "again")).is_err());
        assert!(run(request(Some("relative".into()), "again")).is_err());

        let made = quick(&root, branch_source("feature/x", &id), "").unwrap();
        assert!(path(at("")).ends_with("feature-x-2"));
        // The same answer from inside the linked worktree.
        let inner = worktree_location(Path::new(&made.path), "", &none, "y").unwrap();
        assert_eq!(path(inner), root.join(".worktrees").join("y"));
        // Outside a repository the folder itself is the example.
        let plain = reported(t.path()).join("plain");
        fs::create_dir(&plain).unwrap();
        let example = worktree_location(&plain, "", &none, "y").unwrap();
        assert_eq!(path(example), plain.join(".worktrees").join("y"));
    }

    #[test]
    fn searches_treat_a_nested_worktree_as_a_separate_checkout() {
        let t = tempfile::tempdir().unwrap();
        let (root, id) = quick_repository(t.path());
        let made = quick(
            &root,
            branch_source("feature/x", &id),
            "{repo}/trees/{branch}",
        )
        .unwrap();
        let filter = |respect| crate::fs::search_filter::FilterConfig {
            respect_gitignore: respect,
            ..Default::default()
        };
        let hidden = |from: &Path, file: &Path| {
            crate::fs::search_filter::FilterMatcher::new(from, filter(true))
                .is_path_hidden(file, false)
        };
        let nested = Path::new(&made.path).join("file.txt");
        assert!(hidden(&root, &nested), "not found from the main worktree");
        assert!(!hidden(&root, &root.join("file.txt")));
        assert!(
            !hidden(Path::new(&made.path), &nested),
            "found inside its own worktree"
        );
        // The real walk agrees with the matcher.
        let names = |from: &Path| {
            let builder = ignore::WalkBuilder::new(from);
            builder
                .build()
                .flatten()
                .filter(|e| e.file_name() == "file.txt")
                .count()
        };
        assert_eq!(names(&root), 1);
        assert_eq!(names(Path::new(&made.path)), 1);
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
    fn recovery_removal_allows_uninitialized_gitlinks_and_keeps_initialized_submodule_edits() {
        let source = tempfile::tempdir().unwrap();
        init(source.path());
        fs::write(source.path().join("source.txt"), "submodule\n").unwrap();
        commit(source.path(), "Module");
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path();
        init(root);
        git(
            root,
            &[
                "-c",
                "protocol.file.allow=always",
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
        fs::write(root.join("parent.txt"), "parent\n").unwrap();
        let head = commit(root, "Parent");
        let outside = tempfile::tempdir().unwrap();
        let place = reported(outside.path()).join("linked");
        let destination = place.to_str().unwrap().to_owned();
        act(
            root,
            Request::CreateWorktree {
                destination: destination.clone(),
                target: head,
                branch: None,
            },
        )
        .unwrap();
        let module = place.join("module with spaces");
        git(
            &place,
            &[
                "-c",
                "protocol.file.allow=always",
                "-c",
                "core.autocrlf=false",
                "submodule",
                "update",
                "--init",
                "--checkout",
                "--",
                "module with spaces",
            ],
        )
        .unwrap();
        let moved = reported(outside.path()).join("moved");
        let move_to = |destination: &Path, to: &Path| {
            act(
                root,
                Request::MoveWorktree {
                    destination: destination.to_str().unwrap().to_owned(),
                    to: to.to_str().unwrap().to_owned(),
                },
            )
        };
        fs::write(module.join("source.txt"), "local module edit\n").unwrap();
        let remove = || {
            act(
                root,
                Request::RemoveWorktreeWithSnapshot {
                    destination: destination.clone(),
                    ignored: false,
                },
            )
        };
        assert!(move_to(&place, &moved).is_err());
        assert!(!moved.exists());
        assert!(remove()
            .unwrap_err()
            .to_string()
            .contains("module with spaces"));
        assert_eq!(
            fs::read_to_string(module.join("source.txt")).unwrap(),
            "local module edit\n"
        );
        assert!(worktrees(root)
            .unwrap()
            .iter()
            .any(|tree| tree.path == destination));
        fs::write(module.join("source.txt"), "submodule\n").unwrap();
        let deinitialize = || {
            act(
                &place,
                Request::DeinitSubmodule {
                    file: "module with spaces".into(),
                },
            )
        };
        fs::write(module.join("source.txt"), "local module edit\n").unwrap();
        assert!(deinitialize()
            .unwrap_err()
            .to_string()
            .contains("source.txt"));
        fs::write(module.join("source.txt"), "submodule\n").unwrap();
        let module_git = worktree_git_dir(&module).unwrap();
        fs::create_dir_all(module_git.join("info")).unwrap();
        fs::write(module_git.join("info/exclude"), "local-cache\n").unwrap();
        fs::write(module.join("local-cache"), "keep me\n").unwrap();
        assert!(deinitialize()
            .unwrap_err()
            .to_string()
            .contains("local-cache"));
        assert_eq!(
            fs::read_to_string(module.join("local-cache")).unwrap(),
            "keep me\n"
        );
        fs::remove_file(module.join("local-cache")).unwrap();
        deinitialize().unwrap();
        let metadata = worktree_git_dir(&place).unwrap().join("modules");
        let module_metadata = metadata.join("module with spaces");
        assert!(module_metadata.join("HEAD").is_file());
        let refused = with_dormant_module_metadata(&place, WorktreeRelocation::Remove, || {
            Err(fail("Git refused removal"))
        });
        assert!(refused
            .unwrap_err()
            .to_string()
            .contains("Git refused removal"));
        assert!(module_metadata.join("HEAD").is_file());
        fs::write(place.join("parent.txt"), "local parent edit\n").unwrap();
        move_to(&place, &moved).unwrap();
        assert!(!place.exists());
        assert!(module_metadata.join("HEAD").is_file());
        assert_eq!(
            fs::read_to_string(moved.join("parent.txt")).unwrap(),
            "local parent edit\n"
        );
        move_to(&moved, &place).unwrap();
        let result = remove().unwrap();
        let recovered_modules = result
            .lines()
            .find_map(|line| line.strip_prefix("Submodule Git data is kept in "))
            .unwrap();
        assert!(Path::new(recovered_modules)
            .join("module with spaces/HEAD")
            .is_file());
        let id = result.rsplit('/').next().unwrap().trim();
        assert!(!place.exists());
        assert_eq!(
            git(
                root,
                &["show", &format!("refs/gall/recovery/{id}:parent.txt")]
            )
            .unwrap(),
            "local parent edit\n"
        );
    }

    #[test]
    fn a_changed_worktree_is_removed_only_after_its_changes_are_kept() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        init(p);
        fs::write(p.join("file.txt"), "first\n").unwrap();
        commit(p, "First");
        let token = || snapshot(p, None).unwrap().token;
        let act = |request| execute(p, request, &token(), &AtomicBool::new(false), |_| {});
        let outside = tempfile::tempdir().unwrap();
        let place = crate::nav::path::canonicalize(outside.path())
            .unwrap()
            .join("tree");
        git(
            p,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "kept",
                &place.to_string_lossy(),
            ],
        )
        .unwrap();
        let destination = snapshot(p, None)
            .unwrap()
            .worktrees
            .iter()
            .find(|tree| tree.branch.as_deref() == Some("kept"))
            .unwrap()
            .path
            .clone();
        let remove = |ignored| {
            act(Request::RemoveWorktreeWithSnapshot {
                destination: destination.clone(),
                ignored,
            })
        };
        fs::write(p.join(".git/info/exclude"), "ignored.txt\n").unwrap();
        std::fs::write(place.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(place.join("new.txt"), "new\n").unwrap();
        std::fs::write(place.join("ignored.txt"), "cache\n").unwrap();
        // Ignored files are in no snapshot: refused before anything changes.
        assert!(remove(false)
            .unwrap_err()
            .to_string()
            .contains("ignored files are present"));
        assert!(place.join("new.txt").exists() && place.join(".gitignore").exists());
        // Locked: refused as well.
        act(Request::LockWorktree {
            destination: destination.clone(),
        })
        .unwrap();
        assert!(remove(true).unwrap_err().to_string().contains("locked"));
        act(Request::UnlockWorktree {
            destination: destination.clone(),
        })
        .unwrap();
        let message = remove(true).unwrap();
        assert!(!place.exists());
        let id = message.rsplit('/').next().unwrap().trim().to_owned();
        let files = git(p, &["ls-tree", "-r", "--name-only", &format!("{id}^3")]).unwrap();
        assert!(files.contains("new.txt") && files.contains(".gitignore"));
        assert!(git(
            p,
            &["rev-parse", "--verify", &format!("refs/gall/recovery/{id}")]
        )
        .is_ok());
        assert!(git(p, &["stash", "list"]).unwrap().is_empty());
        assert!(git(p, &["rev-parse", "--verify", "refs/heads/kept"]).is_ok());
        // Never the main worktree.
        let main = snapshot(p, None).unwrap().worktrees[0].path.clone();
        assert!(act(Request::RemoveWorktreeWithSnapshot {
            destination: main,
            ignored: true,
        })
        .is_err());
    }

    #[test]
    fn worktrees_move_are_repaired_and_pruned() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        init(p);
        fs::write(p.join("file.txt"), "first\n").unwrap();
        commit(p, "First");
        let token = || snapshot(p, None).unwrap().token;
        let act = |request| execute(p, request, &token(), &AtomicBool::new(false), |_| {});
        let outside = tempfile::tempdir().unwrap();
        let base = crate::nav::path::canonicalize(outside.path()).unwrap();
        let (first, second, third) = (base.join("a"), base.join("b"), base.join("c"));
        git(
            p,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "moved",
                &first.to_string_lossy(),
            ],
        )
        .unwrap();
        let listed = || {
            snapshot(p, None)
                .unwrap()
                .worktrees
                .into_iter()
                .find(|tree| tree.branch.as_deref() == Some("moved"))
        };
        std::fs::write(first.join("work.txt"), "mine\n").unwrap();
        // Never over something that exists.
        std::fs::create_dir(&second).unwrap();
        let go = |to: &Path| {
            act(Request::MoveWorktree {
                destination: listed().unwrap().path,
                to: to.to_string_lossy().into_owned(),
            })
        };
        assert!(go(&second).is_err() && first.join("work.txt").exists());
        std::fs::remove_dir(&second).unwrap();
        go(&second).unwrap();
        assert!(!first.exists() && second.join("work.txt").exists());
        // Moved by hand: repaired from its new folder.
        std::fs::rename(&second, &third).unwrap();
        act(Request::RepairWorktree {
            destination: third.to_string_lossy().into_owned(),
        })
        .unwrap();
        assert!(listed().unwrap().path.ends_with('c'));
        // Neither a plain folder, nor a main worktree, nor one of another repository.
        let other = tempfile::tempdir().unwrap();
        init(other.path());
        fs::write(other.path().join("file.txt"), "first\n").unwrap();
        commit(other.path(), "First");
        let foreign = base.join("foreign");
        git(
            other.path(),
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &foreign.to_string_lossy(),
            ],
        )
        .unwrap();
        for wrong in [&base, &other.path().to_path_buf(), &foreign] {
            assert!(act(Request::RepairWorktree {
                destination: wrong.to_string_lossy().into_owned(),
            })
            .is_err());
        }
        std::fs::remove_dir_all(&third).unwrap();
        act(Request::PruneWorktrees).unwrap();
        assert!(listed().is_none());
    }

    #[test]
    fn worktree_removal_with_ignored_files_still_refuses_other_untracked_files() {
        let directory = tempfile::tempdir().unwrap();
        let p = directory.path();
        init(p);
        fs::write(p.join(".gitignore"), "cache\n").unwrap();
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
        fs::write(tree.join("cache"), "ignored cache\n").unwrap();
        fs::write(tree.join("notes.txt"), "not ignored\n").unwrap();
        assert!(act(
            p,
            Request::RemoveWorktreeWithIgnored {
                destination: destination.clone()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("Commit or stash local changes"));
        assert!(tree.join("cache").is_file() && tree.join("notes.txt").is_file());
        let plain = act(
            p,
            Request::RemoveWorktree {
                destination: destination.clone(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(plain.contains("Untracked files are present"), "{plain}");
        assert!(!plain.contains("ignored"), "{plain}");
        fs::remove_file(tree.join("notes.txt")).unwrap();
        act(p, Request::RemoveWorktreeWithIgnored { destination }).unwrap();
        assert!(!tree.exists());
        assert_eq!(snapshot(p, None).unwrap().worktrees.len(), 1);
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
