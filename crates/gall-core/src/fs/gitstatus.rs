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
use std::sync::OnceLock;
use std::time::Duration;

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

/// The key of a status entry: macOS volumes may hold decomposed names while Git prints them
/// composed, so there the key is composed and the UI looks names up composed. Elsewhere two
/// names that differ only in normalization are two files.
fn status_key(name: &str) -> String {
    if cfg!(target_os = "macos") {
        crate::util::unicode::nfc(name).into_owned()
    } else {
        name.to_string()
    }
}

/// The Git program to start for every command: the real executable from Git's own program
/// folder (`git --exec-path`), found once, or plain `git` from `PATH` when that fails.
///
/// The `git` on `PATH` is often only a launcher that starts the real program: `cmd\git.exe`
/// of Git for Windows, a Scoop shim in front of it, `xcrun` on macOS. Every hop is one more
/// process, which about doubles the cost of a call on Windows, and a view makes dozens of
/// them. The real program sets up its own environment (helper folders on `PATH`), so the
/// result is the same, credential helpers, `ssh` and LFS included. If it disappears (Git
/// was updated while the app runs), `git` from `PATH` is used again.
pub(super) fn program() -> &'static Path {
    static REAL: OnceLock<Option<PathBuf>> = OnceLock::new();
    match REAL.get_or_init(real_program) {
        Some(real) if real.is_file() => real,
        _ => Path::new("git"),
    }
}

fn real_program() -> Option<PathBuf> {
    let mut cmd = Command::new("git");
    cmd.arg("--exec-path")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok().filter(|out| out.status.success())?;
    let dir = PathBuf::from(
        String::from_utf8(out.stdout)
            .ok()?
            .trim_end_matches(['\r', '\n']),
    );
    let real = dir.join(format!("git{}", std::env::consts::EXE_SUFFIX));
    (dir.is_absolute() && real.is_file()).then_some(real)
}

/// A git command with the protections described in the module docs; `overrides` are
/// more `-c key=value` settings.
fn git(dir: &Path, overrides: &[String]) -> Command {
    // In WSL the command runs inside the distribution (see `gitexec`), where a folder of
    // the Windows temp directory does not exist and the null device does the same.
    let hooks = if super::gitexec::place(dir).is_some() {
        "core.hooksPath=/dev/null".to_string()
    } else {
        format!(
            "core.hooksPath={}",
            std::env::temp_dir().join("gall-no-git-hooks").display()
        )
    };
    let mut cmd = super::gitexec::command(dir, program());
    cmd.env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "core.fsmonitor=false", "-c"])
        .arg(hooks);
    for o in overrides {
        cmd.arg("-c").arg(o);
    }
    cmd.arg("-C")
        .arg(super::gitexec::dir_arg(dir))
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// Runs `cmd` and returns its output, or None on failure or after [`TIMEOUT`].
fn output(mut cmd: Command) -> Option<Vec<u8>> {
    super::gitexec::forward_env(&mut cmd);
    // For a repository in WSL: a shell that is already running in the distribution.
    if let Some(answer) = super::gitexec::run(&cmd, TIMEOUT) {
        return answer.ok().filter(|o| o.status.success()).map(|o| o.stdout);
    }
    let mut child = cmd.stdout(Stdio::piped()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // The reader reports the end of the output, so the result is there the moment git
    // finishes; polling the process added up to a sleep interval to every call.
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = done.send(stdout.read_to_end(&mut buf).map(|_| buf));
    });
    match finished.recv_timeout(TIMEOUT) {
        Ok(Ok(out)) => child.wait().ok()?.success().then_some(out),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

/// Status of the items in `dir`; None outside a repository or when git is missing.
pub fn status(dir: &Path) -> Option<GitStatus> {
    let root = work_tree(dir)?;
    // The repository's filter commands, to switch them off for this call. They are asked
    // for once and again when the configuration changes (see `gitfacts::filters`).
    let off: Vec<String> = super::gitfacts::filters(&root)
        .iter()
        .map(|(key, _)| format!("{key}="))
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
    let Some(out) = output(cmd) else {
        // No answer: Git may refuse the repository now (see `untrusted`).
        if let Ok(mut known) = trusted().lock() {
            known.remove(&root);
        }
        return None;
    };
    let rel = dir.strip_prefix(&root).ok()?;
    let mut prefix = rel.to_string_lossy().replace('\\', "/");
    if !prefix.is_empty() {
        prefix.push('/');
    }
    let mut st = parse(&out, &prefix);
    // The subject and, for a detached HEAD, the branches containing it depend only on the
    // commit: asked once per commit, not on every refresh and in every folder.
    let head = branch_oid(&out);
    let detached = st.branch.is_none();
    let about = head_facts(&root, &head, detached, || {
        std::thread::scope(|scope| {
            let branches = scope.spawn(|| {
                if !detached {
                    return Vec::new();
                }
                let mut branches = git(dir, &off);
                branches.args([
                    "for-each-ref",
                    "--contains=HEAD",
                    "--format=%(refname:short)",
                    "refs/heads/",
                ]);
                output(branches)
                    .unwrap_or_default()
                    .split(|b| *b == b'\n')
                    .filter(|line| !line.is_empty())
                    .map(|line| String::from_utf8_lossy(line).into_owned())
                    .collect()
            });
            let mut summary = git(dir, &off);
            summary.args(["log", "--no-show-signature", "-1", "--format=%s", "HEAD"]);
            let subject =
                output(summary).map(|out| String::from_utf8_lossy(&out).trim_end().to_string());
            (subject, branches.join().unwrap_or_default())
        })
    });
    (st.subject, st.containing_branches) = about;
    st.root = root;
    accept(&st.root);
    Some(st)
}

/// The commit in the `# branch.oid` header of a status (empty before the first commit).
fn branch_oid(out: &[u8]) -> String {
    out.split(|&b| b == 0)
        .take_while(|f| f.starts_with(b"# "))
        .find_map(|f| f.strip_prefix(b"# branch.oid "))
        .map(|oid| String::from_utf8_lossy(oid).into_owned())
        .unwrap_or_default()
}

type HeadFacts = (Option<String>, Vec<String>);

/// The subject of the HEAD commit and, when detached, the local branches containing it:
/// `ask` runs once per repository, commit and local branch set. The branch set is the
/// modification time of `refs/heads` and `packed-refs`, which change when a branch is
/// created, deleted or moved by Git.
fn head_facts(
    root: &Path,
    head: &str,
    detached: bool,
    ask: impl FnOnce() -> HeadFacts,
) -> HeadFacts {
    type Key = (String, bool, Vec<Option<std::time::SystemTime>>);
    static KNOWN: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, (Key, HeadFacts)>>> =
        std::sync::Mutex::new(None);
    // Branch tips only matter for the list of containing branches.
    let refs = if detached {
        heads_changed(root)
    } else {
        Vec::new()
    };
    let key = (head.to_owned(), detached, refs);
    if !head.is_empty() && head != "(initial)" {
        if let Ok(known) = KNOWN.lock() {
            if let Some((k, facts)) = known.as_ref().and_then(|k| k.get(root)) {
                if *k == key {
                    return facts.clone();
                }
            }
        }
    }
    let facts = ask();
    // A failed query (no subject) is asked again next time.
    if facts.0.is_some() {
        if let Ok(mut known) = KNOWN.lock() {
            let known = known.get_or_insert_with(Default::default);
            if known.len() >= 256 {
                known.clear();
            }
            known.insert(root.to_path_buf(), (key, facts.clone()));
        }
    }
    facts
}

/// When the local branches last changed, as far as the files tell: `packed-refs` and the
/// folders of `refs/heads` (a branch `feature/x` is a file in a folder), in the common Git
/// directory.
fn heads_changed(root: &Path) -> Vec<Option<std::time::SystemTime>> {
    let Some(mut dir) = super::gitfacts::git_dir(root) else {
        return vec![Some(std::time::SystemTime::now())]; // unknown: never equal
    };
    if let Ok(common) = std::fs::read_to_string(dir.join("commondir")) {
        dir = dir.join(common.trim());
    }
    let modified = |p: &Path| p.metadata().ok().and_then(|m| m.modified().ok());
    let mut times = vec![modified(&dir.join("packed-refs"))];
    let mut folders = vec![dir.join("refs").join("heads")];
    while let Some(folder) = folders.pop() {
        times.push(modified(&folder));
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut inner: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        inner.sort();
        folders.extend(inner);
    }
    times
}

/// Notes that Git answered a query about the work tree `root`: it accepts the repository.
pub(super) fn accept(root: &Path) {
    if let Ok(mut known) = trusted().lock() {
        if !known.contains(root) {
            if known.len() >= 1024 {
                known.clear();
            }
            known.insert(root.to_path_buf());
        }
    }
}

/// Whether Git answered a query about the work tree `root` and no status failed since.
pub(super) fn accepted(root: &Path) -> bool {
    trusted().lock().is_ok_and(|known| known.contains(root))
}

/// Work tree roots Git answered for: it accepts them, so [`untrusted`] has nothing to
/// ask. A status without an answer takes its root out again.
fn trusted() -> &'static std::sync::Mutex<std::collections::HashSet<PathBuf>> {
    static TRUSTED: OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        OnceLock::new();
    TRUSTED.get_or_init(Default::default)
}

/// The repository path as Git spells it, when Git refuses the repository around `dir`
/// because another user owns it ("dubious ownership": folders on a network share, a WSL
/// distribution opened from Windows, a repository copied from another account). None when
/// `dir` is outside a repository or Git accepts it. Git's own spelling is what
/// `safe.directory` has to contain, so it is read from the message.
pub fn untrusted(dir: &Path) -> Option<PathBuf> {
    // A repository in WSL is read by the Git inside the distribution, as its owner.
    if super::gitexec::place(dir).is_some() {
        return None;
    }
    let root = work_tree(dir)?;
    // Git answered a status here: it does not refuse the repository. This runs on every
    // navigation, and ownership does not change while the app runs.
    if accepted(&root) {
        return None;
    }
    let mut cmd = git(&root, &[]);
    // A fixed language: the message is parsed.
    cmd.args(["rev-parse", "--git-dir"])
        .env("LC_ALL", "C")
        .env("LANGUAGE", "C")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    super::gitexec::forward_env(&mut cmd);
    let out = cmd.output().ok()?;
    if out.status.success() {
        return None;
    }
    parse_untrusted(&String::from_utf8_lossy(&out.stderr))
}

/// The path in Git's "detected dubious ownership in repository at '<path>'" error.
fn parse_untrusted(stderr: &str) -> Option<PathBuf> {
    let line = stderr
        .lines()
        .find_map(|l| l.split_once("dubious ownership in repository at '"))?
        .1
        .trim_end();
    let path = line.strip_suffix('\'')?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Adds the repository around `dir` to the user's global `safe.directory`, only when Git
/// itself refuses it as untrusted; returns the trusted path. The path comes from Git's
/// message, never from the caller, so this cannot trust an arbitrary folder.
pub fn trust(dir: &Path) -> Result<PathBuf, String> {
    let path = untrusted(dir).ok_or("Git does not refuse this folder")?;
    let global = |args: &[&str]| {
        let mut cmd = Command::new(program());
        cmd.args(["config", "--global"])
            .args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.output().map_err(|e| e.to_string())
    };
    let known = global(&["--get-all", "safe.directory"])?;
    let wanted = path.to_string_lossy();
    if String::from_utf8_lossy(&known.stdout)
        .lines()
        .any(|l| l.trim_end() == wanted)
    {
        return Ok(path);
    }
    let added = global(&["--add", "safe.directory", &wanted])?;
    if added.status.success() {
        Ok(path)
    } else {
        Err(String::from_utf8_lossy(&added.stderr).trim().to_string())
    }
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
        let e = st.entries.entry(status_key(child)).or_insert(state);
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
    fn the_untrusted_path_is_read_from_gits_message() {
        let msg = "fatal: detected dubious ownership in repository at '//wsl.localhost/Ubuntu-26.04/home/u/SW/G800'\n'//wsl.localhost/Ubuntu-26.04/home/u/SW/G800' may refer to a non-local directory\nTo add an exception for this directory, call:\n\n\tgit config --global --add safe.directory ...\n";
        assert_eq!(
            parse_untrusted(msg),
            Some(PathBuf::from("//wsl.localhost/Ubuntu-26.04/home/u/SW/G800"))
        );
        assert_eq!(parse_untrusted("fatal: not a git repository\n"), None);
        assert_eq!(parse_untrusted(""), None);
    }

    #[test]
    fn an_accepted_or_missing_repository_is_not_untrusted() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(untrusted(t.path()), None, "outside a repository");
        let r = t.path().join("repo");
        std::fs::create_dir(&r).unwrap();
        let init = Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["init", "-q"])
            .status();
        if !init.is_ok_and(|s| s.success()) {
            eprintln!("git not installed: skipped");
            return;
        }
        assert_eq!(untrusted(&r), None, "own repository");
        assert!(trust(&r).is_err(), "nothing to trust");
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
