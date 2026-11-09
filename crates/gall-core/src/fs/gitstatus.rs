//! Git status of the items in a folder (badges in the file list) and the branch.
//!
//! Safe in repositories one did not create (a downloaded project with its `.git`): a plain
//! `git status` can run commands configured in the repository: clean filters while it
//! hashes changed files, the post-index-change hook when it refreshes the index, an
//! fsmonitor, and the same again inside submodules. Here the repository's filters are
//! switched off for the call, hooks point nowhere, optional locks are off (status never
//! writes the index, which also keeps it out of the way of a git running at the same
//! time) and submodules are not entered.

use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What Git says about an item; for a folder, the most important state inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitState {
    /// Ignored (.gitignore); only for the item itself, not for folders with ignored files.
    Ignored,
    /// New, not added.
    Untracked,
    /// Changes added to the index (staged).
    Staged,
    /// Changed in the work tree (also deleted or renamed inside a folder).
    Modified,
    /// Merge conflict.
    Conflict,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// Work tree root.
    pub root: PathBuf,
    /// Current branch; None when the HEAD is detached.
    pub branch: Option<String>,
    /// Local branches whose history contains HEAD, including detached revisions.
    pub containing_branches: Vec<String>,
    pub subject: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// State of each item of the folder that has one, by name.
    pub entries: BTreeMap<String, GitState>,
    /// The folder itself is untracked or ignored: every item in it is.
    pub all: Option<GitState>,
}

/// How long a status may take (a huge repository on a slow disk) before it is given up.
const TIMEOUT: Duration = Duration::from_secs(8);

/// The work tree containing `dir`: the nearest folder with a `.git` (a folder, or a file
/// for worktrees and submodules). Found without starting git, so folders outside a
/// repository cost nothing. None inside `.git` itself.
pub fn work_tree(dir: &Path) -> Option<PathBuf> {
    if dir.components().any(|c| c.as_os_str() == ".git") {
        return None;
    }
    dir.ancestors()
        .find(|d| d.join(".git").symlink_metadata().is_ok())
        .map(Path::to_path_buf)
}

/// A git command with the protections described in the module docs; `overrides` are
/// more `-c key=value` settings.
fn git(dir: &Path, overrides: &[String]) -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "core.fsmonitor=false", "-c"])
        .arg(format!(
            "core.hooksPath={}",
            std::env::temp_dir().join("gall-no-git-hooks").display()
        ));
    for o in overrides {
        cmd.arg("-c").arg(o);
    }
    cmd.arg("-C")
        .arg(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Runs `cmd` and returns its output, or None on failure or after [`TIMEOUT`].
fn output(mut cmd: Command) -> Option<Vec<u8>> {
    let mut child = cmd.stdout(Stdio::piped()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().ok()?.ok()?;
                return status.success().then_some(out);
            }
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Status of the items in `dir`; None outside a repository or when git is missing.
pub fn status(dir: &Path) -> Option<GitStatus> {
    let root = work_tree(dir)?;
    // The repository's filter commands, to switch them off for this call.
    let mut cfg = git(dir, &[]);
    cfg.args(["config", "-z", "--name-only", "--get-regexp"])
        .arg(r"^filter\..*\.(clean|smudge|process)$");
    let filters = output(cfg).unwrap_or_default();
    let off: Vec<String> = filters
        .split(|&b| b == 0)
        .filter(|k| !k.is_empty())
        .map(|k| format!("{}=", String::from_utf8_lossy(k)))
        .collect();
    let mut cmd = git(dir, &off);
    cmd.args([
        "status",
        "--porcelain=v2",
        "-z",
        "--branch",
        "--untracked-files=normal",
        "--ignored=matching",
        "--ignore-submodules=all",
        "--",
        ".",
    ]);
    let out = output(cmd)?;
    let rel = dir.strip_prefix(&root).ok()?;
    let mut prefix = rel.to_string_lossy().replace('\\', "/");
    if !prefix.is_empty() {
        prefix.push('/');
    }
    let mut st = parse(&out, &prefix);
    let mut summary = git(dir, &off);
    summary.args(["log", "--no-show-signature", "-1", "--format=%s", "HEAD"]);
    st.subject = output(summary).map(|out| String::from_utf8_lossy(&out).trim_end().to_string());
    if st.branch.is_none() {
        let mut branches = git(dir, &off);
        branches.args([
            "for-each-ref",
            "--contains=HEAD",
            "--format=%(refname:short)",
            "refs/heads/",
        ]);
        st.containing_branches = output(branches)
            .unwrap_or_default()
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| String::from_utf8_lossy(line).into_owned())
            .collect();
    }
    st.root = root;
    Some(st)
}

/// Parses `git status --porcelain=v2 -z --branch` output for the folder whose path in
/// the repository is `prefix` ("" for the root, else ending in "/").
pub fn parse(out: &[u8], prefix: &str) -> GitStatus {
    let mut st = GitStatus::default();
    let mut fields = out.split(|&b| b == 0).map(String::from_utf8_lossy);
    let put = |path: &str, state: GitState, st: &mut GitStatus| {
        let Some(rest) = path.strip_prefix(prefix) else {
            // The folder itself is untracked or ignored ("dir/" above us or equal to it).
            if state <= GitState::Untracked && prefix.starts_with(path) && path.ends_with('/') {
                st.all = Some(state);
            }
            return;
        };
        let trimmed = rest.trim_end_matches('/');
        if trimmed.is_empty() {
            if state <= GitState::Untracked {
                st.all = Some(state);
            }
            return;
        }
        let child = trimmed.split('/').next().unwrap_or(trimmed);
        // A folder is ignored only when it is ignored itself, not for files inside it.
        if state == GitState::Ignored && child != trimmed {
            return;
        }
        let e = st.entries.entry(child.to_string()).or_insert(state);
        *e = (*e).max(state);
    };
    while let Some(f) = fields.next() {
        let f = f.as_ref();
        if let Some(h) = f.strip_prefix("# branch.head ") {
            st.branch = (h != "(detached)").then(|| h.to_string());
        } else if let Some(ab) = f.strip_prefix("# branch.ab ") {
            let mut it = ab.split(' ');
            st.ahead = it
                .next()
                .and_then(|a| a.trim_start_matches('+').parse().ok())
                .unwrap_or(0);
            st.behind = it
                .next()
                .and_then(|b| b.trim_start_matches('-').parse().ok())
                .unwrap_or(0);
        } else if let Some(p) = f.strip_prefix("? ") {
            put(p, GitState::Untracked, &mut st);
        } else if let Some(p) = f.strip_prefix("! ") {
            put(p, GitState::Ignored, &mut st);
        } else if f.starts_with("1 ") || f.starts_with("2 ") || f.starts_with("u ") {
            let (n, conflict) = match f.as_bytes()[0] {
                b'1' => (9, false),
                b'2' => (10, false),
                _ => (11, true),
            };
            let parts: Vec<&str> = f.splitn(n, ' ').collect();
            let (Some(xy), Some(path)) = (parts.get(1), parts.get(n - 1)) else {
                continue;
            };
            let xy = xy.as_bytes();
            let state = if conflict {
                GitState::Conflict
            } else if xy.get(1).is_some_and(|&y| y != b'.') {
                GitState::Modified
            } else {
                GitState::Staged
            };
            put(path, state, &mut st);
            if f.starts_with("2 ") {
                fields.next(); // the original path of a rename
            }
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(lines: &[&str]) -> Vec<u8> {
        lines.join("\0").into_bytes()
    }

    #[test]
    fn parses_branch_and_entries_of_the_root() {
        let o = out(&[
            "# branch.oid abc",
            "# branch.head main",
            "# branch.ab +2 -1",
            "1 .M N... 100644 100644 100644 a b src/main.rs",
            "1 A. N... 000000 100644 100644 a b new.txt",
            "1 M. N... 100644 100644 100644 a b src/lib.rs",
            "2 R. N... 100644 100644 100644 a b R100 docs/b.md",
            "docs/a.md",
            "u UU N... 100644 100644 100644 100644 a b c conflict.txt",
            "? notes/",
            "? src/scratch.rs",
            "! target/",
            "! src/gen.o",
        ]);
        let st = parse(&o, "");
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        let e = |n: &str| st.entries.get(n).copied();
        assert_eq!(
            e("src"),
            Some(GitState::Modified),
            "worst state inside wins"
        );
        assert_eq!(e("new.txt"), Some(GitState::Staged));
        assert_eq!(e("docs"), Some(GitState::Staged));
        assert_eq!(e("conflict.txt"), Some(GitState::Conflict));
        assert_eq!(e("notes"), Some(GitState::Untracked));
        assert_eq!(e("target"), Some(GitState::Ignored));
        assert_eq!(e("a.md"), None, "the old name of a rename is not an entry");
        assert_eq!(st.all, None);
    }

    #[test]
    fn parses_a_subfolder_and_untracked_folders() {
        let o = out(&[
            "# branch.head (detached)",
            "1 .M N... 1 1 1 a b src/ui/app.ts",
            "! src/gen.o",
            "? src/new/",
        ]);
        let st = parse(&o, "src/");
        assert_eq!(st.branch, None);
        assert_eq!(st.entries.get("ui"), Some(&GitState::Modified));
        assert_eq!(st.entries.get("gen.o"), Some(&GitState::Ignored));
        assert_eq!(st.entries.get("new"), Some(&GitState::Untracked));
        // Inside an untracked folder everything is untracked.
        let st = parse(&out(&["? src/new/"]), "src/new/deep/");
        assert_eq!(st.all, Some(GitState::Untracked));
        let st = parse(&out(&["! build/"]), "build/");
        assert_eq!(st.all, Some(GitState::Ignored));
    }

    #[test]
    fn work_tree_is_found_without_git() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("repo/.git")).unwrap();
        std::fs::create_dir_all(t.path().join("repo/a/b")).unwrap();
        assert_eq!(
            work_tree(&t.path().join("repo/a/b")),
            Some(t.path().join("repo"))
        );
        assert_eq!(work_tree(&t.path().join("repo/.git")), None);
    }

    /// End to end with the real git, when it is installed: badges, and the repository's
    /// own filter and hook commands are not run.
    #[test]
    fn status_of_a_real_repository_runs_none_of_its_commands() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path().join("repo");
        std::fs::create_dir(&r).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&r)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        if !run(&["init", "-q"]) {
            eprintln!("git not installed: skipped");
            return;
        }
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(r.join("f.txt"), "a\n").unwrap();
        std::fs::write(r.join(".gitignore"), "build/\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "init"]);
        // A hostile configuration: a clean filter and a hook that leave a marker.
        let marker = t.path().join("ran");
        std::fs::write(r.join(".gitattributes"), "* filter=evil\n").unwrap();
        let touch = format!("sh -c 'touch \"{}\"; cat'", marker.display());
        run(&["config", "filter.evil.clean", &touch]);
        std::fs::create_dir(r.join("hooks")).unwrap();
        run(&["config", "core.hooksPath", "hooks"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let hook = r.join("hooks/post-index-change");
            std::fs::write(
                &hook,
                format!("#!/bin/sh\ntouch \"{}\"\n", marker.display()),
            )
            .unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(r.join("f.txt"), "b\n").unwrap();
        std::fs::create_dir(r.join("build")).unwrap();
        std::fs::write(r.join("new.txt"), "n").unwrap();
        let st = status(&r).expect("status");
        assert_eq!(st.entries.get("f.txt"), Some(&GitState::Modified));
        assert_eq!(st.entries.get("new.txt"), Some(&GitState::Untracked));
        assert_eq!(st.entries.get("build"), Some(&GitState::Ignored));
        assert!(st.branch.is_some());
        assert_eq!(st.subject.as_deref(), Some("init"));
        assert!(run(&[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "filter.evil.clean=",
            "checkout",
            "--detach",
            "HEAD"
        ]));
        let detached = status(&r).unwrap();
        assert!(detached.branch.is_none());
        assert!(!detached.containing_branches.is_empty());
        assert_eq!(detached.subject.as_deref(), Some("init"));
        assert!(!marker.exists(), "a command from the repository ran");
    }
}
