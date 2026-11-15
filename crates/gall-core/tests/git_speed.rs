//! Speed test of everything the app asks Git, in a heavy repository: 50 000 tracked
//! files, 20 000 commits, 300 branches, 200 tags, 30 stashes, 100 000 ignored files in
//! `node_modules`, `.cache` and `target`, 1 000 changed and 1 000 untracked files, a
//! submodule of 12 500 files with a submodule of its own, a small submodule, two linked
//! worktrees, a second repository without submodules and a bare remote.
//!
//! Every view and operation is checked two ways:
//! - **Git processes**: no more than listed in `views()`. This does not depend on the
//!   machine: a process start costs a few milliseconds on Linux and 40–80 ms on Windows,
//!   so a dozen processes in a row are slow there even when Linux does not show it. Lower
//!   a number when a view gets cheaper; raising one needs a reason.
//! - **Time**: the median of five runs against the limit of that view on that platform
//!   in `tests/git_speed_limits.tsv`, which is 150 % of the slowest run recorded there.
//!   Limits are kept as multiples of a reference (a plain `git status` of the test
//!   repository, measured right before each view), so a busy or slower machine moves the
//!   limit with it. The aim is 200 ms; rows above it are marked in the table. A platform without recorded
//!   limits is measured and reported, and only its process counts are checked.
//!
//! Not run by default (it builds the repositories, which takes a minute on Linux and
//! several on Windows), but it is part of the release checks on Linux, macOS and Windows:
//!
//! ```text
//! cargo test -p gall-core --release --test git_speed -- --ignored --nocapture
//! ```
//!
//! On Windows everything is measured a second time for repositories inside WSL (opened
//! by their `\\wsl.localhost\…` path, platform `windows-wsl`) when a distribution with
//! Git is installed.
//!
//! Environment:
//! - `GALL_SPEED_RECORD=1`: raise the limits of this platform to 150 % of the slowest run
//!   of this pass and write the file. Run it a few times on a new machine or platform.
//! - `GALL_SPEED_BUDGET_MS`: an additional limit for every view (not set by default).
//! - `GALL_SPEED_FILES`, `GALL_SPEED_COMMITS`, `GALL_SPEED_IGNORED`: size of the repository.
//! - `GALL_SPEED_DIR`: where to build the native repository (default: the temp folder).
//! - `GALL_SPEED_WSL_DISTRO`: the distribution to use (default: the first one listed).
//! - `GALL_SPEED_WSL_DIR`: the Linux folder to build the WSL repository in (default `/tmp`,
//!   which WSL empties when the distribution stops; use another one with `GALL_SPEED_KEEP`).
//! - `GALL_SPEED_REQUIRE_WSL=1`: fail instead of skipping when WSL cannot be used.
//! - `GALL_SPEED_KEEP=1`: keep the repositories after the run (their paths are printed).
//! - `GALL_SPEED_REPORT`: a file the result table is appended to (Markdown).
//! - `GALL_SPEED_ONLY`: comma-separated parts of view names; only those views are measured.
//! - `GALL_SPEED_FIXTURE`: a folder kept by an earlier run (`GALL_SPEED_KEEP=1`) to measure
//!   again without building it; native path only, WSL is skipped unless
//!   `GALL_SPEED_WSL_FIXTURE` names the Linux path of a kept WSL folder (Windows only).
use gall_core::fs::{
    diff, gitadvanced, gitexec, gitgraph, gitinspect, gitnetwork, gitrepository, gitrewrite,
    gitstatus, gitworktree, listing,
};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Measured runs per view (after one that is reported as "first" and not judged: it
/// fills the caches, starts the shell in WSL and reads files from disk).
const RUNS: usize = 5;

fn setting(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Where the repository is and how Git is run to build it.
struct Place {
    label: &'static str,
    /// The main repository as the app opens it; linked worktrees are next to it.
    dir: PathBuf,
    /// `(distribution, Linux path of the repository)` for a repository inside WSL.
    wsl: Option<(String, String)>,
}

impl Place {
    /// Creates the folder `sub` of the repository (and the repository folder itself).
    fn mkdir(&self, sub: &str) {
        match &self.wsl {
            Some((distro, linux)) => {
                let made = Command::new("wsl.exe")
                    .args([
                        "-d",
                        distro,
                        "--exec",
                        "mkdir",
                        "-p",
                        &format!("{linux}/{sub}"),
                    ])
                    .status();
                assert!(made.is_ok_and(|s| s.success()), "mkdir {sub} in WSL");
            }
            None => std::fs::create_dir_all(self.dir.join(sub)).expect("create folder"),
        }
    }

    /// The folder `sub` of the repository as the Git of this place spells it.
    fn path(&self, sub: &str) -> String {
        match &self.wsl {
            Some((_, linux)) => format!("{linux}/{sub}"),
            None => self.dir.join(sub).to_string_lossy().into_owned(),
        }
    }

    /// Runs Git in the folder `sub` of the repository ("" for the repository itself) with
    /// `env` set and `input` on its standard input.
    fn git(&self, sub: &str, args: &[&str], env: &[(&str, &str)], input: &[u8]) -> String {
        let mut cmd = match &self.wsl {
            Some((distro, linux)) => {
                let mut cmd = Command::new("wsl.exe");
                cmd.args(["-d", distro, "--exec", "env"]);
                for (key, value) in env {
                    cmd.arg(format!("{key}={value}"));
                }
                cmd.args(["git", "-C", &format!("{linux}/{sub}")]);
                cmd
            }
            None => {
                let mut cmd = Command::new("git");
                cmd.arg("-C").arg(self.dir.join(sub));
                cmd.envs(env.iter().copied());
                cmd
            }
        };
        let mut child = cmd
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start git");
        let mut stdin = child.stdin.take().unwrap();
        let out = std::thread::scope(|scope| {
            scope.spawn(move || {
                let _ = stdin.write_all(input);
            });
            child.wait_with_output().expect("run git")
        });
        assert!(
            out.status.success(),
            "git {args:?} failed in {}/{sub}: {}",
            self.dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
}

/// Deterministic pseudo-random numbers: the repository is the same on every run.
struct Rng(u64);
impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

fn tracked(i: usize) -> String {
    format!("src/mod{:03}/sub{}/file{i:06}.rs", i % 200, i % 7)
}

/// What one repository of the fixture holds.
struct Shape<'a> {
    files: usize,
    commits: usize,
    branches: usize,
    /// Files in ignored folders, put into the work tree only.
    ignored: usize,
    /// Tracked files changed in the work tree, and as many untracked ones.
    changed: usize,
    /// Submodules: `(path, commit)`.
    modules: &'a [(&'a str, String)],
}

/// Tracked files changed in the work tree: every second one from the start.
fn changed(shape: &Shape) -> impl Iterator<Item = usize> {
    (0..shape.changed * 2).step_by(2)
}

/// The history as a `git fast-import` stream, and after it one commit outside every
/// branch whose tree holds what is put into the work tree without being committed.
fn history(shape: &Shape) -> Vec<u8> {
    fn data(out: &mut Vec<u8>, text: &str) {
        write!(out, "data {}\n{text}\n", text.len()).unwrap();
    }
    fn file(out: &mut Vec<u8>, path: &str, text: &str) {
        writeln!(out, "M 100644 inline {path}").unwrap();
        data(out, text);
    }
    let (files, commits) = (shape.files, shape.commits);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut out = Vec::new();
    let mut tips: Vec<(String, usize)> = Vec::new();
    for c in 0..commits {
        let mark = c + 1;
        let branch = if c < 50 || rng.below(10) < 6 {
            "main".to_owned()
        } else {
            format!("feature/b{:03}", rng.below(shape.branches))
        };
        writeln!(out, "commit refs/heads/{branch}\nmark :{mark}").unwrap();
        let time = 1_700_000_000 + c * 600;
        writeln!(
            out,
            "author A U Thor <a{}@example.com> {time} +0100\ncommitter C O Mitter <c@example.com> {time} +0100",
            c % 40
        )
        .unwrap();
        data(
            &mut out,
            &format!("Commit {c}: change something useful\n\nBody of commit {c}."),
        );
        let known = tips.iter().position(|(name, _)| *name == branch);
        match known {
            // A new branch starts at the tip of main.
            None if !tips.is_empty() => writeln!(out, "from :{}", tips[0].1).unwrap(),
            Some(0) if tips.len() > 1 && rng.below(10) == 0 => {
                let other = 1 + rng.below(tips.len() - 1);
                writeln!(out, "merge :{}", tips[other].1).unwrap();
            }
            _ => {}
        }
        match known {
            Some(i) => tips[i].1 = mark,
            None => tips.push((branch, mark)),
        }
        if c == 0 {
            file(
                &mut out,
                ".gitignore",
                "node_modules/\ntarget/\n.cache/\n*.o\n",
            );
            if !shape.modules.is_empty() {
                let mut text = String::new();
                for (path, commit) in shape.modules {
                    writeln!(text, "[submodule \"{path}\"]\n\tpath = {path}\n\turl = https://example.invalid/{path}.git").unwrap();
                    writeln!(out, "M 160000 {commit} {path}").unwrap();
                }
                file(&mut out, ".gitmodules", text.trim_end());
            }
            for i in 0..files {
                let path = tracked(i);
                let body = format!("// {path}\nfn f() {{}}\n{}", "let x = 1;\n".repeat(20));
                file(&mut out, &path, &body);
            }
        } else {
            for _ in 0..3 {
                let path = tracked(rng.below(files));
                let body = format!(
                    "// {path} at {c}\n{}",
                    format!("let x = {c};\n").repeat(5 + rng.below(35))
                );
                file(&mut out, &path, &body);
            }
        }
        out.push(b'\n');
        if c % (commits / 200).max(1) == 0 {
            writeln!(out, "reset refs/tags/v0.{c}\nfrom :{mark}\n").unwrap();
        }
    }
    for (name, mark) in tips.iter().take(shape.branches / 2) {
        writeln!(out, "reset refs/remotes/origin/{name}\nfrom :{mark}\n").unwrap();
    }
    writeln!(out, "commit refs/fixture/extra\nmark :{}", commits + 1).unwrap();
    writeln!(out, "committer C O Mitter <c@example.com> 1700000000 +0100").unwrap();
    data(&mut out, "work tree content");
    for (folder, count) in [
        ("node_modules", shape.ignored * 2 / 3),
        (".cache", shape.ignored / 6),
        ("target", shape.ignored / 6),
    ] {
        for i in 0..count {
            let path = format!("{folder}/pkg{:03}/lib{}/f{i}.js", i % 500, i % 11);
            file(&mut out, &path, "x");
        }
    }
    for i in 0..shape.changed {
        file(&mut out, &format!("untracked/d{}/n{i}.txt", i % 10), "new");
    }
    for i in changed(shape) {
        let path = tracked(i);
        file(
            &mut out,
            &path,
            &format!("// {path}\nfn f() {{}}\n// changed\n"),
        );
    }
    out.push(b'\n');
    out
}

/// Builds one repository in the folder `sub` of `place`; returns its HEAD commit.
fn build_repository(place: &Place, sub: &str, shape: &Shape) -> String {
    place.mkdir(sub);
    place.git(sub, &["init", "-q", "-b", "main"], &[], b"");
    for (key, value) in [
        ("user.email", "t@example.com"),
        ("user.name", "t"),
        ("core.autocrlf", "false"),
        ("gc.auto", "0"),
        ("remote.origin.url", "https://example.invalid/fixture.git"),
        ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
        ("branch.main.remote", "origin"),
        ("branch.main.merge", "refs/heads/main"),
    ] {
        place.git(sub, &["config", key, value], &[], b"");
    }
    for (path, _) in shape.modules {
        let url = format!("https://example.invalid/{path}.git");
        place.git(
            sub,
            &["config", &format!("submodule.{path}.url"), &url],
            &[],
            b"",
        );
    }
    place.git(sub, &["fast-import", "--quiet"], &[], &history(shape));
    place.git(sub, &["checkout", "-q", "-f", "main"], &[], b"");
    // Upstream one commit behind, so the branch is "ahead 1".
    place.git(
        sub,
        &["update-ref", "refs/remotes/origin/main", "main~1"],
        &[],
        b"",
    );
    // Stashes: any merge commit has the shape of one.
    let merges = place.git(sub, &["rev-list", "--merges", "-n", "30", "main"], &[], b"");
    for (i, merge) in merges.lines().enumerate() {
        let message = format!("On main: saved work {i}");
        place.git(sub, &["stash", "store", "-m", &message, merge], &[], b"");
    }
    // The uncommitted content goes into the work tree through a second index, so the
    // real one never hears of it: ignored and untracked files, and changed tracked ones.
    let index = [("GIT_INDEX_FILE", ".git/gall-speed-index")];
    place.git(sub, &["read-tree", "refs/fixture/extra"], &index, b"");
    place.git(sub, &["checkout-index", "-a", "-f"], &index, b"");
    place.git(sub, &["update-ref", "-d", "refs/fixture/extra"], &[], b"");
    let staged: Vec<String> = changed(shape).take(25).map(tracked).collect();
    let mut add = vec!["add", "--"];
    add.extend(staged.iter().map(String::as_str));
    if !staged.is_empty() {
        place.git(sub, &add, &[], b"");
    }
    place.git(sub, &["rev-parse", "HEAD"], &[], b"")
}

/// Builds the fixture in `place`: a large repository with a large submodule that has a
/// submodule of its own, a small submodule, stashes, and two linked worktrees next to it.
fn build(place: &Place) {
    let files = setting("GALL_SPEED_FILES", 50_000);
    let commits = setting("GALL_SPEED_COMMITS", 20_000);
    let ignored = setting("GALL_SPEED_IGNORED", 100_000);
    let started = Instant::now();
    // Inside out: a repository records the commits of its submodules.
    let leaf = |files, commits| Shape {
        files,
        commits,
        branches: 5,
        ignored: 0,
        changed: 20,
        modules: &[],
    };
    let nested = build_repository(place, "vendor/big/deps/nested", &leaf(500, 100));
    let small = build_repository(place, "vendor/small", &leaf(200, 50));
    let big = build_repository(
        place,
        "vendor/big",
        &Shape {
            files: files / 4,
            commits: commits / 10,
            branches: 30,
            ignored: ignored / 10,
            changed: 100,
            modules: &[("deps/nested", nested)],
        },
    );
    build_repository(
        place,
        "",
        &Shape {
            files,
            commits,
            branches: 300,
            ignored,
            changed: 1000,
            modules: &[("vendor/big", big), ("vendor/small", small)],
        },
    );
    // The Git directories of the submodules move into the parent's, as after a clone.
    place.git("", &["submodule", "absorbgitdirs"], &[], b"");
    place.git(
        "",
        &["worktree", "add", "-q", "../wt-feature", "feature/b001"],
        &[],
        b"",
    );
    place.git(
        "",
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            "../wt-detached",
            "main~50",
        ],
        &[],
        b"",
    );
    // A second large repository without submodules and with a clean work tree: Gall
    // refuses stash, merge, rebase and cherry-pick where there are submodules.
    build_repository(
        place,
        "../plain",
        &Shape {
            files: files / 2,
            commits: commits / 4,
            branches: 20,
            ignored: 0,
            changed: 0,
            modules: &[],
        },
    );
    place.git("../plain", &["switch", "-q", "feature/b001"], &[], b"");
    // For merge, rebase and cherry-pick there: a branch that leaves `feature/b001`
    // three commits back and adds one file, so nothing can conflict.
    let side = "commit refs/heads/speed/side\ncommitter C O Mitter <c@example.com> 1800000000 +0100\ndata 9\nSide work\nfrom refs/heads/feature/b001~3\nM 100644 inline speed-side.txt\ndata 4\nside\n\n";
    place.git(
        "../plain",
        &["fast-import", "--quiet"],
        &[],
        side.as_bytes(),
    );
    // A remote to fetch from and push to: a bare repository next to this one.
    place.mkdir("../remote.git");
    place.git("../remote.git", &["init", "-q", "--bare"], &[], b"");
    let remote = place.path("../remote.git");
    place.git("", &["remote", "add", "local", &remote], &[], b"");
    place.git(
        "",
        &["push", "-q", "local", "feature/b001", "main"],
        &[],
        b"",
    );
    place.git("", &["fetch", "-q", "local"], &[], b"");
    let upstream = [
        "branch",
        "-q",
        "--set-upstream-to=local/feature/b001",
        "feature/b001",
    ];
    place.git("", &upstream, &[], b"");
    eprintln!(
        "{}: fixture built in {:.1} s ({files} files, {commits} commits, {ignored} ignored, 3 submodules, 2 worktrees, a second repository)",
        place.label,
        started.elapsed().as_secs_f32()
    );
}

struct Sample {
    time: Duration,
    processes: u64,
    ok: bool,
}

/// Runs `f` and reports its time and the Git processes it started.
fn timed(f: impl FnOnce() -> bool) -> Sample {
    let before = gitexec::started();
    let start = Instant::now();
    let ok = f();
    Sample {
        time: start.elapsed(),
        processes: gitexec::started() - before,
        ok,
    }
}

/// `(name, most Git processes per call, the call)`.
type View<'a> = (&'static str, u64, Box<dyn Fn() -> Sample + 'a>);

/// The views for the repository at `root`; `head` is its HEAD commit, `in_head` a file
/// that commit changed and `base` the tenth ancestor of the linked worktree's branch.
fn views<'a>(place: &'a Place, head: &'a str, in_head: &'a str, base: &'a str) -> Vec<View<'a>> {
    let root: &'a Path = &place.dir;
    let plain = "src/mod001/sub1/file000001.rs";
    let edited = "src/mod102/sub4/file000102.rs";
    let all = listing::ListOptions {
        show_hidden: true,
        hide_temp: false,
        dirs_only: false,
    };
    let visible = listing::ListOptions {
        show_hidden: false,
        ..all
    };
    let status = move |sub: &'static str| -> Box<dyn Fn() -> Sample + 'a> {
        Box::new(move || timed(|| gitstatus::status(&root.join(sub)).is_some()))
    };
    let list =
        move |sub: &'static str, opts: listing::ListOptions| -> Box<dyn Fn() -> Sample + 'a> {
            Box::new(move || {
                timed(|| listing::list_dir(&root.join(sub), opts).is_ok_and(|l| !l.is_empty()))
            })
        };
    macro_rules! view {
        ($call:expr) => {
            Box::new(move || timed(|| $call)) as Box<dyn Fn() -> Sample + 'a>
        };
    }
    // Gall reads HEAD, references, configuration and stashes from Git's files, which WSL
    // repositories leave to Git (`wsl` processes instead of `files`). With Git for Windows
    // on a Windows folder a full status is one process instead of two; `statuses` is how
    // many of them the call asks for.
    let n = |files: u64, wsl: u64, statuses: u64| {
        if place.wsl.is_some() {
            wsl
        } else if cfg!(windows) {
            files - statuses
        } else {
            files
        }
    };
    vec![
        // What every folder navigation asks: the listing, the root and the badges.
        ("list the root", 0, list("", visible)),
        (
            "list node_modules (500 folders)",
            0,
            list("node_modules", all),
        ),
        (
            "list .cache/pkg001/lib1",
            0,
            list(".cache/pkg001/lib1", all),
        ),
        (
            "git root of a folder",
            0,
            view!(diff::git_root(&root.join("src/mod001")).is_some()),
        ),
        ("status badges: root", 1, status("")),
        ("status badges: src/mod001", 1, status("src/mod001")),
        (
            "status badges: node_modules (ignored)",
            1,
            status("node_modules"),
        ),
        (
            "status badges: node_modules/pkg001",
            1,
            status("node_modules/pkg001"),
        ),
        (
            "status badges: .cache (hidden, ignored)",
            1,
            status(".cache"),
        ),
        ("status badges: untracked/d1", 1, status("untracked/d1")),
        (
            "untrusted repository check",
            0,
            view!(gitstatus::untrusted(root).is_none()),
        ),
        // The Git workspace.
        (
            "repository snapshot",
            n(5, 14, 1),
            view!(gitrepository::snapshot(root).is_ok()),
        ),
        (
            "graph view, 200 commits",
            n(7, 17, 1),
            view!(gitrepository::graph_view(root, false, false, 200, None).is_ok()),
        ),
        (
            "graph view, 2000 commits",
            n(7, 17, 1),
            view!(gitrepository::graph_view(root, false, false, 2000, None).is_ok()),
        ),
        (
            "graph view with the reflog",
            n(7, 17, 1),
            view!(gitrepository::graph_view(root, false, true, 200, None).is_ok()),
        ),
        (
            "history query",
            n(5, 6, 1),
            view!(gitgraph::query_history(root, false, false, 200, None).is_ok()),
        ),
        (
            "working changes",
            n(4, 5, 1),
            view!(gitworktree::workspace(root).is_ok()),
        ),
        (
            "patch of a changed file",
            n(3, 3, 1),
            view!(gitworktree::patch(root, edited, false).is_ok()),
        ),
        (
            "selectable patch of a changed file",
            n(4, 6, 1),
            view!(gitworktree::selection_patch(root, edited, false).is_ok()),
        ),
        (
            "worktrees, submodules, bisect",
            n(10, 28, 1),
            view!(gitadvanced::snapshot(root, None).is_ok()),
        ),
        (
            "the same with the notes of a commit",
            n(12, 30, 1),
            view!(gitadvanced::snapshot(root, Some(head)).is_ok()),
        ),
        (
            "submodule tree (3 submodules, one nested)",
            7,
            view!(gitadvanced::submodule_tree(root).is_ok_and(|tree| !tree.entries.is_empty())),
        ),
        (
            "commit details",
            3,
            view!(gitinspect::details(root, head, None).is_ok()),
        ),
        (
            "commit diff",
            1,
            view!(gitinspect::commit_diff(root, head).is_ok()),
        ),
        (
            "commit as a patch",
            1,
            view!(gitinspect::commit_patch_text(root, head).is_ok()),
        ),
        (
            "patch of one file of a commit",
            1,
            view!(gitinspect::patch(root, head, None, in_head).is_ok()),
        ),
        (
            "history of a file",
            3,
            view!(gitinspect::file_history(root, plain, 200, None).is_ok()),
        ),
        (
            "content of a file in a commit",
            2,
            view!(gitinspect::content(root, head, plain).is_ok()),
        ),
        (
            "blame",
            3,
            view!(gitinspect::blame(root, head, plain).is_ok()),
        ),
        (
            "commits for the diff picker",
            1,
            view!(diff::git_commits(&root.join(plain)).is_ok()),
        ),
        // Changing the index: stage one file, then take it back.
        (
            "stage a file",
            n(4, 5, 1),
            Box::new(move || index_action(root, gitworktree::IndexAction::Stage, edited)),
        ),
        (
            "unstage a file",
            n(4, 5, 1),
            Box::new(move || index_action(root, gitworktree::IndexAction::Unstage, edited)),
        ),
        // Submodules: a folder of them, and a large one opened as a repository.
        (
            "status badges: vendor (folder of submodules)",
            1,
            status("vendor"),
        ),
        (
            "status badges: vendor/big (a submodule)",
            1,
            status("vendor/big"),
        ),
        (
            "status badges: vendor/big/src/mod001",
            1,
            status("vendor/big/src/mod001"),
        ),
        (
            "submodule: graph view",
            n(7, 17, 1),
            view!(
                gitrepository::graph_view(&root.join("vendor/big"), false, false, 200, None)
                    .is_ok()
            ),
        ),
        (
            "submodule: working changes",
            n(4, 5, 1),
            view!(gitworktree::workspace(&root.join("vendor/big")).is_ok()),
        ),
        (
            "nested submodule: working changes",
            n(4, 5, 1),
            view!(gitworktree::workspace(&root.join("vendor/big/deps/nested")).is_ok()),
        ),
        // A linked worktree of the same repository.
        ("linked worktree: status badges", 1, status("../wt-feature")),
        (
            "linked worktree: graph view",
            n(8, 18, 1),
            view!(
                gitrepository::graph_view(&root.join("../wt-feature"), false, false, 200, None)
                    .is_ok()
            ),
        ),
        (
            "linked worktree: working changes",
            n(4, 5, 1),
            view!(gitworktree::workspace(&root.join("../wt-feature")).is_ok()),
        ),
        // Changing the repository: a branch, and the interactive rebase of ten commits up
        // to the point where Git would start rewriting (the plan and the conflict check).
        (
            "create a branch",
            n(4, 9, 1),
            Box::new(move || branch_action(root, true)),
        ),
        (
            "delete a branch",
            n(4, 11, 1),
            Box::new(move || branch_action(root, false)),
        ),
        (
            "interactive rebase: plan of 10 commits",
            n(8, 18, 1),
            view!(gitrewrite::plan(&root.join("../wt-feature"), base)
                .is_ok_and(|plan| !plan.commits.is_empty())),
        ),
        (
            "interactive rebase: conflict check of 10 commits",
            // One `merge-tree --stdin` for the plan; in WSL one per commit.
            n(2, 11, 0),
            Box::new(move || {
                let wt = root.join("../wt-feature");
                let Ok(plan) = gitrewrite::plan(&wt, base) else {
                    return FAILED;
                };
                let entries: Vec<gitrewrite::RewriteEntry> = plan
                    .commits
                    .into_iter()
                    .map(|commit| gitrewrite::RewriteEntry {
                        id: commit.id,
                        action: gitrewrite::RewriteAction::Pick,
                        message: String::new(),
                    })
                    .collect();
                timed(|| gitrewrite::predict_conflicts(&wt, base, &entries).is_ok())
            }),
        ),
        // Operations that change the repository. Each one is undone afterwards, outside
        // the measured time. The work tree of the main repository has a thousand changes;
        // checkout runs in the clean linked worktree, and stash, merge, rebase and
        // cherry-pick in the second repository, which has no submodules.
        (
            "commit the staged files",
            n(4, 6, 1),
            Box::new(move || {
                let Ok(seen) = gitworktree::workspace(root) else {
                    return FAILED;
                };
                let action = gitworktree::IndexAction::Commit;
                let done = timed(|| {
                    gitworktree::index_action(root, action, None, "Speed test", &seen.token).is_ok()
                });
                if done.ok {
                    place.git("", &["reset", "-q", "--soft", "HEAD~1"], &[], b"");
                }
                done
            }),
        ),
        (
            "stash a staged file",
            n(4, 10, 1),
            Box::new(move || {
                // Something to stash: the file of the side branch, staged.
                let take = ["checkout", "speed/side", "--", "speed-side.txt"];
                place.git("../plain", &take, &[], b"");
                let request = gitrepository::Request::StashSave {
                    message: "Speed test".into(),
                };
                let done = repository_action(&root.join("../plain"), request);
                if done.ok {
                    place.git("../plain", &["stash", "drop", "-q"], &[], b"");
                } else {
                    place.git("../plain", &["reset", "-q", "--hard"], &[], b"");
                }
                done
            }),
        ),
        (
            "checkout another branch",
            n(3, 5, 1),
            Box::new(move || {
                let wt = root.join("../wt-feature");
                // Fixture restoration can outlast the filter cache on Windows. As for
                // other mutations, prepare the displayed state outside the measurement.
                if gitrepository::snapshot(&wt).is_err() {
                    return FAILED;
                }
                let done = timed(|| {
                    gitgraph::switch(&wt, "feature/b002", true, "", "")
                        .inspect_err(|e| eprintln!("  refused in {}: {e}", wt.display()))
                        .is_ok()
                });
                place.git("../wt-feature", &["switch", "-q", "feature/b001"], &[], b"");
                // Files the restore just wrote are racily clean, so the next status hashes
                // them for seconds. Settle them here, or the snapshot above outlasts the
                // filter cache and its refresh lands in the measured switch of every other run.
                place.git(
                    "../wt-feature",
                    &["update-index", "-q", "--refresh"],
                    &[],
                    b"",
                );
                done
            }),
        ),
        (
            "merge a branch",
            n(6, 12, 1),
            Box::new(move || in_worktree(place, |target| gitrepository::Request::Merge { target })),
        ),
        (
            "rebase 3 commits onto a branch",
            n(9, 15, 1),
            Box::new(move || {
                in_worktree(place, |target| gitrepository::Request::Rebase { target })
            }),
        ),
        (
            "cherry-pick a commit",
            n(7, 14, 1),
            Box::new(move || {
                in_worktree(place, |target| gitrepository::Request::CherryPick {
                    target,
                })
            }),
        ),
        (
            "fetch (nothing new, local remote)",
            n(5, 9, 1),
            Box::new(move || network(root, gitnetwork::NetworkAction::Fetch)),
        ),
        (
            "push one commit (local remote)",
            n(6, 14, 1),
            Box::new(move || {
                let empty = ["commit", "-q", "--allow-empty", "-m", "Speed test"];
                place.git("../wt-feature", &empty, &[], b"");
                network(&root.join("../wt-feature"), gitnetwork::NetworkAction::Push)
            }),
        ),
    ]
}

const FAILED: Sample = Sample {
    time: Duration::ZERO,
    processes: 0,
    ok: false,
};

/// A repository action with the token of a fresh snapshot; only the action counts.
fn repository_action(root: &Path, request: gitrepository::Request) -> Sample {
    let Ok(seen) = gitrepository::snapshot(root) else {
        return FAILED;
    };
    timed(|| {
        gitrepository::execute(root, request, &seen.token)
            .inspect_err(|e| eprintln!("  refused in {}: {e}", root.display()))
            .is_ok()
    })
}

/// An action with the branch `speed/side` in the second repository (clean, no
/// submodules), which is put back to where its branch was afterwards.
fn in_worktree(place: &Place, request: impl FnOnce(String) -> gitrepository::Request) -> Sample {
    let plain = place.dir.join("../plain");
    let before = place.git("../plain", &["rev-parse", "HEAD"], &[], b"");
    let side = place.git("../plain", &["rev-parse", "speed/side"], &[], b"");
    let done = repository_action(&plain, request(side));
    place.git("../plain", &["reset", "-q", "--hard", &before], &[], b"");
    done
}

/// A fetch or push with the remote `local`.
fn network(root: &Path, action: gitnetwork::NetworkAction) -> Sample {
    let Ok(seen) = gitrepository::snapshot(root) else {
        return FAILED;
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    timed(|| {
        gitnetwork::run(
            root,
            "local",
            action,
            &seen.token,
            None,
            None,
            &cancel,
            |_| {},
        )
        .inspect_err(|e| eprintln!("  refused in {}: {e}", root.display()))
        .is_ok()
    })
}

/// Creates the branch `speed/new` at HEAD and deletes it again, each with the token of a
/// fresh snapshot; the time and processes of one of the two are reported.
fn branch_action(root: &Path, measure_create: bool) -> Sample {
    let act = |request: fn(String) -> gitrepository::Request| {
        let Ok(seen) = gitrepository::snapshot(root) else {
            return FAILED;
        };
        let request = request(seen.head.clone());
        timed(|| gitrepository::execute(root, request, &seen.token).is_ok())
    };
    let created = act(|target| gitrepository::Request::CreateBranch {
        name: "speed/new".into(),
        target,
    });
    let deleted = act(|_| gitrepository::Request::DeleteBranch {
        name: "speed/new".into(),
        force: true,
    });
    let (mut measured, other) = if measure_create {
        (created, deleted)
    } else {
        (deleted, created)
    };
    measured.ok &= other.ok;
    measured
}

/// An index action with the token of a fresh look at the changes; only the action counts.
fn index_action(root: &Path, action: gitworktree::IndexAction, file: &str) -> Sample {
    let Ok(seen) = gitworktree::workspace(root) else {
        return FAILED;
    };
    timed(|| gitworktree::index_action(root, action, Some(file), "", &seen.token).is_ok())
}

/// The time limits: `platform <tab> view <tab> factor` per line, in
/// `tests/git_speed_limits.tsv`. The factor is a multiple of the reference time, a plain
/// `git status` of the main repository measured right before the view: a busy or slower
/// machine slows both alike, so the limit moves with it. A factor is one and a half times
/// the slowest run ever recorded for that view on that platform, so an ordinary slow
/// moment does not fail the test while a view that got slower does.
struct Limits {
    file: PathBuf,
    values: std::collections::BTreeMap<(String, String), f64>,
}

/// No limit below this many milliseconds: a view of a few milliseconds varies by more
/// than half of itself.
const SMALLEST_LIMIT: f64 = 25.0;

impl Limits {
    fn load() -> Self {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/git_speed_limits.tsv");
        let values = std::fs::read_to_string(&file)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let mut fields = line.split('\t');
                let key = (fields.next()?.to_owned(), fields.next()?.to_owned());
                Some((key, fields.next()?.trim().parse().ok()?))
            })
            .collect();
        Limits { file, values }
    }

    /// The limit of `view` in milliseconds for a machine whose reference is `reference`.
    fn get(&self, platform: &str, view: &str, reference: f64) -> Option<u64> {
        self.values
            .get(&(platform.to_owned(), view.to_owned()))
            .map(|factor| (factor * reference).max(SMALLEST_LIMIT).ceil() as u64)
    }

    /// Raises the factor of `view` to one and a half times `slowest` if that is more.
    fn record(&mut self, platform: &str, view: &str, slowest: Duration, reference: f64) {
        let factor = slowest.as_secs_f64() * 1000.0 * 1.5 / reference;
        let entry = self
            .values
            .entry((platform.to_owned(), view.to_owned()))
            .or_insert(0.0);
        *entry = entry.max((factor * 100.0).ceil() / 100.0);
    }

    fn save(&self) {
        let mut text = String::from(
            "# Time limits of the Git speed test (crates/gall-core/tests/git_speed.rs), as multiples\n# of the reference time (a plain `git status` of the test repository, measured before each\n# view): 150 % of the slowest recorded run. Written by GALL_SPEED_RECORD=1; delete a line\n# to record that view again from scratch.\n",
        );
        for ((platform, view), factor) in &self.values {
            writeln!(text, "{platform}\t{view}\t{factor:.2}").unwrap();
        }
        std::fs::write(&self.file, text).expect("write the limits");
    }
}

/// The reference time in milliseconds: the median of three plain `git status` runs in the
/// main repository, by the Git of the place and without any of Gall's code.
fn reference(place: &Place) -> f64 {
    // Submodules are left out, as in Gall's own status.
    let status = ["status", "--porcelain=v1", "--ignore-submodules=all"];
    let mut times: Vec<f64> = (0..3)
        .map(|_| {
            let start = Instant::now();
            place.git("", &status, &[("GIT_OPTIONAL_LOCKS", "0")], b"");
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[1]
}

/// Measures every view in `place`; returns the table and what is over its limit.
fn measure(place: &Place, limits: &mut Limits) -> (String, Vec<String>) {
    let cap = std::env::var("GALL_SPEED_BUDGET_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    let record = std::env::var_os("GALL_SPEED_RECORD").is_some();
    let platform = format!(
        "{}{}",
        std::env::consts::OS,
        if place.wsl.is_some() { "-wsl" } else { "" }
    );
    let head = place.git("", &["rev-parse", "HEAD"], &[], b"");
    let mut table = format!(
        "\n### {} ({platform})\n\n| View | first ms | median ms | worst ms | limit ms | reference ms | git | allowed | |\n|---|---:|---:|---:|---:|---:|---:|---:|---|\n",
        place.label,
    );
    let mut failures = Vec::new();
    let mut unlimited = 0;
    let base = place.git("", &["rev-parse", "feature/b001~10"], &[], b"");
    let in_head = place.git(
        "",
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-m",
            &head,
        ],
        &[],
        b"",
    );
    let in_head = in_head.lines().next().expect("HEAD changes a file");
    let only = std::env::var("GALL_SPEED_ONLY").unwrap_or_default();
    let selected = |name: &str| {
        only.trim().is_empty()
            || only
                .split(',')
                .any(|part| !part.trim().is_empty() && name.contains(part.trim()))
    };
    for (name, allowed, run) in views(place, &head, in_head, &base) {
        if !selected(name) {
            continue;
        }
        let reference = reference(place);
        let first = run();
        let mut samples: Vec<Sample> = (0..RUNS).map(|_| run()).collect();
        // The usual number: a kept answer (the content filters) is asked for again every
        // ten seconds, which may add one process to a single run.
        let mut counts: Vec<u64> = samples.iter().map(|s| s.processes).collect();
        counts.sort_unstable();
        let processes = counts[RUNS / 2];
        let ok = first.ok && samples.iter().all(|s| s.ok);
        samples.sort_by_key(|s| s.time);
        let median = samples[RUNS / 2].time.as_millis() as u64;
        let worst = samples[RUNS - 1].time;
        if record && ok {
            limits.record(&platform, name, worst, reference);
        }
        let limit = limits.get(&platform, name, reference);
        let mut notes = Vec::new();
        if !ok {
            notes.push("the call failed".to_owned());
        }
        match limit {
            Some(limit) if median > limit => {
                notes.push(format!("{median} ms, limit {limit} ms"));
            }
            Some(_) => {}
            None => unlimited += 1,
        }
        if cap.is_some_and(|cap| median > cap) {
            notes.push(format!("{median} ms, over GALL_SPEED_BUDGET_MS"));
        }
        if processes > allowed {
            notes.push(format!("{processes} Git processes, {allowed} allowed"));
        }
        writeln!(
            table,
            "| {name} | {} | {median} | {} | {} | {reference:.0} | {processes} | {allowed} | {} |",
            first.time.as_millis(),
            worst.as_millis(),
            limit.map_or("-".to_owned(), |l| l.to_string()),
            if !notes.is_empty() {
                "**over**"
            } else if median > 200 {
                "above 200 ms"
            } else {
                ""
            }
        )
        .unwrap();
        if !notes.is_empty() {
            failures.push(format!("{} / {name}: {}", place.label, notes.join("; ")));
        }
    }
    if unlimited > 0 {
        writeln!(
            table,
            "\n{unlimited} views have no time limit for `{platform}` yet (only their Git processes were checked). Record them on this machine with `GALL_SPEED_RECORD=1`."
        )
        .unwrap();
    }
    (table, failures)
}

/// A place inside the first WSL distribution that has Git, or why there is none. With
/// `GALL_SPEED_WSL_FIXTURE` (the Linux path a run with `GALL_SPEED_KEEP=1` printed), the
/// folder kept by that run.
#[cfg(windows)]
fn wsl_place() -> Result<Place, String> {
    use gall_core::fs::wsl;
    let distro = match std::env::var("GALL_SPEED_WSL_DISTRO") {
        Ok(name) => name,
        Err(_) => {
            let listed = Command::new("wsl.exe")
                .args(["-l", "-q"])
                .output()
                .map_err(|e| format!("wsl.exe did not start: {e}"))?;
            wsl::parse_list_quiet(&wsl::decode_output(&listed.stdout))
                .into_iter()
                .find(|name| !wsl::hidden_distro(name))
                .ok_or("no WSL distribution is installed")?
        }
    };
    let inside = |args: &[&str]| {
        Command::new("wsl.exe")
            .args(["-d", &distro, "--exec"])
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    inside(&["git", "--version"]).ok_or(format!("no Git in the WSL distribution {distro}"))?;
    let linux = match std::env::var("GALL_SPEED_WSL_FIXTURE") {
        Ok(kept) => kept.trim_end_matches('/').to_owned(),
        Err(_) => {
            // `/tmp` is emptied whenever the distribution stops; a kept fixture needs
            // another folder.
            let base = std::env::var("GALL_SPEED_WSL_DIR").unwrap_or_else(|_| "/tmp".into());
            inside(&["mktemp", "-d", &format!("{base}/gall-speed-XXXXXX")])
                .ok_or(format!("no temp folder in the WSL distribution {distro}"))?
        }
    };
    let base =
        |host: &str| PathBuf::from(format!(r"\\{host}\{distro}{}", linux.replace('/', r"\")));
    let dir = [base("wsl.localhost"), base("wsl$")]
        .into_iter()
        .find(|dir| dir.is_dir())
        .ok_or(format!("{linux} of {distro} is not reachable from Windows"))?
        .join("repo");
    Ok(Place {
        label: "repository inside WSL",
        dir,
        wsl: Some((distro, format!("{linux}/repo"))),
    })
}

#[cfg(not(windows))]
fn wsl_place() -> Result<Place, String> {
    Err("WSL exists only on Windows".into())
}

#[test]
#[ignore = "builds a large repository; part of the release checks"]
fn git_views_answer_within_the_time_and_process_budget() {
    let reused = std::env::var_os("GALL_SPEED_FIXTURE").map(PathBuf::from);
    let base = std::env::var_os("GALL_SPEED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let temp = reused.is_none().then(|| {
        tempfile::Builder::new()
            .prefix("gall-speed-")
            .tempdir_in(base)
            .expect("temp folder")
    });
    let fixture = reused
        .clone()
        .unwrap_or_else(|| temp.as_ref().unwrap().path().to_path_buf());
    if std::env::var_os("GALL_SPEED_KEEP").is_some() && reused.is_none() {
        eprintln!("the fixture is kept in {}", fixture.display());
    }
    let mut places = vec![Place {
        label: "repository on a native path",
        dir: fixture.join("repo"),
        wsl: None,
    }];
    let mut skipped = String::new();
    let mut failures = Vec::new();
    let wsl_reused = std::env::var_os("GALL_SPEED_WSL_FIXTURE").is_some();
    let keep = std::env::var_os("GALL_SPEED_KEEP").is_some();
    let wsl = if reused.is_some() && !wsl_reused {
        Err("GALL_SPEED_FIXTURE".to_owned())
    } else {
        wsl_place()
    };
    match wsl {
        Ok(place) => places.push(place),
        Err(why)
            if cfg!(windows)
                && reused.is_none()
                && std::env::var_os("GALL_SPEED_REQUIRE_WSL").is_some() =>
        {
            failures.push(format!("WSL was required: {why}"))
        }
        Err(why) => skipped = format!("\nRepository inside WSL: **not measured** ({why}).\n"),
    }
    eprintln!("{skipped}");
    let mut report = skipped.clone();
    let mut limits = Limits::load();
    for place in &places {
        let built_before = match place.wsl {
            Some(_) => wsl_reused,
            None => reused.is_some(),
        };
        if !built_before {
            build(place);
            std::thread::sleep(Duration::from_secs(2));
        }
        // The app writes the commit-graph and refreshes the index in the background when
        // a Git view opens; here it is waited for, so every run measures the same state.
        // The index can only be refreshed a moment after the files were written (above).
        for repository in [
            "",
            "vendor/big",
            "vendor/big/deps/nested",
            "../wt-feature",
            "../plain",
        ] {
            gitrepository::prepare_history(&place.dir.join(repository)).expect("commit-graph");
        }
        let (table, over) = measure(place, &mut limits);
        eprintln!("{table}");
        report.push_str(&table);
        failures.extend(over);
        if let Some((distro, linux)) = &place.wsl {
            let folder = linux.trim_end_matches("/repo");
            if keep || wsl_reused {
                eprintln!("the WSL fixture is kept in {folder} (GALL_SPEED_WSL_FIXTURE)");
                continue;
            }
            let _ = Command::new("wsl.exe")
                .args([
                    "-d",
                    distro,
                    "--exec",
                    "rm",
                    "-rf",
                    linux.trim_end_matches("/repo"),
                ])
                .status();
        }
    }
    if std::env::var_os("GALL_SPEED_RECORD").is_some() {
        limits.save();
        eprintln!("limits written to {}", limits.file.display());
    }
    if let Some(temp) = temp.filter(|_| std::env::var_os("GALL_SPEED_KEEP").is_some()) {
        let _ = temp.keep();
    }
    if let Some(file) = std::env::var_os("GALL_SPEED_REPORT") {
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .and_then(|mut f| f.write_all(report.as_bytes()));
        if let Err(e) = written {
            eprintln!(
                "could not write the report to {}: {e}",
                Path::new(&file).display()
            );
        }
    }
    assert!(
        failures.is_empty(),
        "Git got slower or starts more processes:\n  {}",
        failures.join("\n  ")
    );
}
