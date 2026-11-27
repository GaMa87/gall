//! Git for repositories inside WSL runs in the distribution itself.
//!
//! Windows reads `\\wsl.localhost\<distro>\…` through the 9P file server, and Git stats
//! every file of the work tree: a status takes seconds (the first one tens of seconds)
//! where Git inside the distribution needs a fraction of a second. So for a repository
//! there, the Git commands run as `wsl.exe -d <distro> --exec git -C <linux path> …`.
//! As a side effect Git runs as the owner of the files and never meets "dubious
//! ownership".
//!
//! The callers keep building their command lines; this module supplies what changes:
//! the program ([`command`]), the repository argument ([`dir_arg`]), other paths given
//! to Git ([`path_arg`]), paths Git prints ([`host_path`]) and the null device
//! ([`null_device`]). On other platforms and for other folders nothing changes.

use super::wsl::{self, WslPath};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Environment variables Gall sets on Git commands. `wsl.exe` passes a variable into the
/// distribution only when `WSLENV` names it.
const FORWARDED: &[&str] = &[
    "GIT_OPTIONAL_LOCKS",
    "GIT_INDEX_FILE",
    "GIT_ATTR_SOURCE",
    "GIT_SEQUENCE_EDITOR",
    "GIT_EDITOR",
    "GIT_TERMINAL_PROMPT",
    "GIT_NO_LAZY_FETCH",
    "GIT_ASKPASS",
    "GIT_ALLOW_PROTOCOL",
    "GIT_SSH_COMMAND",
    "GCM_INTERACTIVE",
    "LC_ALL",
    "LANGUAGE",
];

/// Marks a command built by [`command`] for a distribution (see [`run`]).
const DISTRO_MARK: &str = "GALL_WSL_DISTRO";

/// Runs `cmd` (built by [`command`] and complete with arguments and environment) and
/// collects its output. A Git command for a repository in WSL goes to a long-lived shell
/// in the distribution (see `gitworker`): a few milliseconds instead of the 70 ms that
/// starting `wsl.exe` takes, which adds up to seconds over the dozens of questions a view
/// asks. None when `cmd` is no such command or no shell could be used; run it the ordinary
/// way then. Only for commands that need no input: stdin is closed.
pub(super) fn run(
    cmd: &Command,
    timeout: std::time::Duration,
) -> Option<std::io::Result<std::process::Output>> {
    let mut distro = None;
    let mut line = String::from("env");
    for (key, value) in cmd.get_envs() {
        let (key, Some(value)) = (key.to_string_lossy(), value) else {
            continue;
        };
        match &*key {
            DISTRO_MARK => distro = Some(value.to_string_lossy().into_owned()),
            "WSLENV" => {}
            _ => {
                line.push(' ');
                line.push_str(&super::gitworker::quote(&format!(
                    "{key}={}",
                    value.to_string_lossy()
                )));
            }
        }
    }
    line.push_str(" git");
    // The first four are `-d <distro> --exec git`, which the shell does not need.
    for arg in cmd.get_args().skip(4) {
        line.push(' ');
        line.push_str(&super::gitworker::quote(&arg.to_string_lossy()));
    }
    super::gitworker::run(&distro?, &line, timeout)
}

/// Where `dir` is inside a WSL distribution (Windows only), else None.
pub(super) fn place(dir: &Path) -> Option<WslPath> {
    if cfg!(windows) {
        wsl::parse_unc(&dir.to_string_lossy())
    } else {
        None
    }
}

/// Git commands built so far (see [`started`]).
static STARTED: AtomicU64 = AtomicU64::new(0);

/// How many Git commands were built since the app started. Each one is a process (or a
/// request to a shell in WSL), and on Windows a process start costs tens of milliseconds,
/// so the speed test keeps the number per view within a budget.
pub fn started() -> u64 {
    STARTED.load(Ordering::Relaxed)
}

/// The Git program for `dir`: `native` for ordinary folders, `git` inside the
/// distribution for a repository in WSL. Add `-C` [`dir_arg`] and the arguments next.
pub(super) fn command(dir: &Path, native: &Path) -> Command {
    command_with_index(dir, native, None)
}

pub(super) fn command_with_index(dir: &Path, native: &Path, index: Option<&Path>) -> Command {
    command_with_options(dir, native, index, None)
}

pub(super) fn command_with_options(
    dir: &Path,
    native: &Path,
    index: Option<&Path>,
    attr_source: Option<&str>,
) -> Command {
    STARTED.fetch_add(1, Ordering::Relaxed);
    let index = index.map(|index| path_arg(dir, index));
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = match place(dir) {
        Some(w) => wsl_git::<&str>(&w, &[]),
        None if crate::util::hostspawn::in_flatpak() => {
            return host_git(index.as_deref(), attr_source);
        }
        None => Command::new(native),
    };
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    if let Some(source) = attr_source {
        cmd.env("GIT_ATTR_SOURCE", source);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// `wsl.exe -d <distro> --exec <prefix…> git`. [`run`] expects an empty `prefix`.
fn wsl_git<S: AsRef<std::ffi::OsStr>>(w: &WslPath, prefix: &[S]) -> Command {
    let mut c = Command::new(wsl::wsl_exe());
    c.args(["-d", &w.distro, "--exec"]).args(prefix).arg("git");
    // Tells `run` and `forward_env` where this command belongs; not passed on to Git.
    c.env(DISTRO_MARK, &w.distro);
    c.env("WSLENV", std::env::var("WSLENV").unwrap_or_default());
    c
}

/// A Git command for a repository in WSL with `prefix` (a wrapper such as `timeout`) run
/// before `git` inside the distribution. None for other folders. Not for [`run`].
#[cfg(windows)]
pub(super) fn wsl_command<S: AsRef<std::ffi::OsStr>>(dir: &Path, prefix: &[S]) -> Option<Command> {
    let w = place(dir)?;
    STARTED.fetch_add(1, Ordering::Relaxed);
    let mut cmd = wsl_git(&w, prefix);
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    Some(cmd)
}

/// Git on the host, for a Flatpak sandbox that has none of its own: `flatpak-spawn --host git`.
/// The host process starts with the host's environment, so the variables every Gall Git command
/// sets (no optional locks, no terminal prompt, untranslated messages that `gitstatus::untrusted`
/// reads) go along as `--env`. Other variables a caller sets (credential helpers for network
/// operations) do not reach the host Git.
fn host_git(index: Option<&std::ffi::OsStr>, attr_source: Option<&str>) -> Command {
    let index = index.map(std::ffi::OsStr::to_string_lossy);
    let mut env = vec![
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("LC_ALL", "C"),
    ];
    if let Some(index) = &index {
        env.push(("GIT_INDEX_FILE", index.as_ref()));
    }
    if let Some(source) = attr_source {
        env.push(("GIT_ATTR_SOURCE", source));
    }
    let wrapped = crate::util::hostspawn::wrap_argv_if(true, vec!["git".to_string()], None, &env);
    let mut cmd = Command::new(&wrapped[0]);
    cmd.args(&wrapped[1..]);
    cmd
}

/// A Git command with no repository behind it (`git config --global`): the program as is, except in
/// a Flatpak sandbox, where it is the host's Git.
pub(super) fn plain(native: &Path) -> Command {
    if crate::util::hostspawn::in_flatpak() {
        host_git(None, None)
    } else {
        Command::new(native)
    }
}

/// Prepares WSL forwarding after the caller has set the command's environment.
/// WSL turns an absent variable named in WSLENV into an empty one; an empty
/// GIT_ALLOW_PROTOCOL forbids every transport.
pub(super) fn forward_env(cmd: &mut Command) {
    if !cmd.get_envs().any(|(key, _)| key == DISTRO_MARK) {
        return;
    }
    let existing = cmd
        .get_envs()
        .find(|(key, _)| *key == "WSLENV")
        .and_then(|(_, value)| value)
        .map(|value| value.to_string_lossy().into_owned());
    let present: Vec<&str> = FORWARDED
        .iter()
        .copied()
        .filter(|key| match cmd.get_envs().find(|(name, _)| name == key) {
            Some((_, value)) => value.is_some(),
            None => std::env::var_os(key).is_some(),
        })
        .collect();
    cmd.env("WSLENV", wslenv(existing.as_deref(), &present));
}

fn wslenv(existing: Option<&str>, forwarded: &[&str]) -> String {
    let mut names: Vec<&str> = existing
        .unwrap_or("")
        .split(':')
        .filter(|n| !n.is_empty())
        .collect();
    for f in forwarded {
        if !names.iter().any(|n| n.split('/').next() == Some(f)) {
            names.push(f);
        }
    }
    names.join(":")
}

/// The repository folder as the Git of [`command`] needs it after `-C`.
pub(super) fn dir_arg(dir: &Path) -> OsString {
    match place(dir) {
        Some(w) => w.linux.into(),
        None => dir.as_os_str().to_owned(),
    }
}

/// A path to hand to Git as an argument (a destination, a patch file): for a repository
/// in WSL its Linux spelling (a path in the distribution, a drive as `/mnt/c/…`).
pub(super) fn path_arg(dir: &Path, p: &Path) -> OsString {
    if let Some(repo) = place(dir) {
        // A path in another distribution is no path for this one: left as it is, Git
        // reports that it does not exist instead of writing somewhere else.
        let other = wsl::parse_unc(&p.to_string_lossy())
            .is_some_and(|w| !w.distro.eq_ignore_ascii_case(&repo.distro));
        if !other {
            if let Some(linux) = wsl::linux_path_for(&p.to_string_lossy()) {
                return linux.into();
            }
        }
    }
    p.as_os_str().to_owned()
}

/// Size, modification and change times (nanoseconds) of `names` (relative to the repository
/// `dir`) for a repository in WSL, read by one `stat` inside the distribution. Reading them
/// from Windows costs a round trip over 9P per file: thousands of untracked files took
/// seconds. None for other folders or when it could not run; files that vanished are left out.
pub(super) fn fingerprint(dir: &Path, names: &[String]) -> Option<Vec<u8>> {
    let w = place(dir)?;
    if names.is_empty() {
        return Some(Vec::new());
    }
    match fingerprint_in_shell(&w, names) {
        Some(out) => out,
        None => fingerprint_started(&w, names),
    }
}

/// The changed paths of the last token of each work tree, as a hint of what the next one
/// will list: the content or the fingerprint of those files can be read while Git lists
/// the changes, and is used only when the list turns out to be the same.
static LAST_CHANGED: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, Vec<String>>>> =
    std::sync::Mutex::new(None);

pub(super) fn last_changed(path: &Path) -> Option<Vec<String>> {
    let last = LAST_CHANGED.lock().ok()?;
    last.as_ref()?.get(path).cloned()
}

pub(super) fn remember_changed(path: &Path, names: Vec<String>) {
    if let Ok(mut last) = LAST_CHANGED.lock() {
        let last = last.get_or_insert_with(Default::default);
        if last.len() >= 64 {
            last.clear();
        }
        last.insert(path.to_path_buf(), names);
    }
}

/// [`fingerprint`] through a shell that is already running (see `gitworker`): starting
/// `wsl.exe` for it took 70–90 ms on every token. The names go in as quoted words of
/// `printf`, a shell builtin, so their number is not limited by the size of a command line.
/// None when no shell could be used.
fn fingerprint_in_shell(w: &WslPath, names: &[String]) -> Option<Option<Vec<u8>>> {
    let quote = super::gitworker::quote;
    let mut line = format!("cd {} || exit 2; printf '%s\\0'", quote(&w.linux));
    for name in names {
        line.push(' ');
        line.push_str(&quote(name));
    }
    line.push_str(" | xargs -0 -r stat -c '%s %.9Y %.9Z %n' --");
    let out = super::gitworker::run(&w.distro, &line, std::time::Duration::from_secs(120))?;
    Some(out.ok().filter(vanished_only).map(|out| out.stdout))
}

/// Whether the `xargs … stat` of a fingerprint succeeded, or failed only because a name
/// vanished meanwhile (`stat` exits with 1, which `xargs` reports as 123); those files are
/// left out, which is no failure here.
fn vanished_only(out: &std::process::Output) -> bool {
    out.status.code().is_some_and(|c| matches!(c, 0 | 1 | 123))
}

/// [`fingerprint`] by a `wsl.exe` of its own, with the names on its input.
fn fingerprint_started(w: &WslPath, names: &[String]) -> Option<Vec<u8>> {
    use std::io::Write;
    use std::process::Stdio;
    let mut cmd = Command::new(wsl::wsl_exe());
    cmd.args([
        "-d",
        &w.distro,
        "--exec",
        "sh",
        "-c",
        "cd \"$1\" || exit 2; xargs -0 -r stat -c '%s %.9Y %.9Z %n' --",
        "gall-fingerprint",
        &w.linux,
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let input: Vec<u8> = names.iter().flat_map(|n| n.bytes().chain([0])).collect();
    // Written on the side: the output may fill its pipe before all names are in.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().ok()?;
    let _ = writer.join();
    vanished_only(&out).then_some(out.stdout)
}

/// A path Git printed (`rev-parse --show-toplevel`, `worktree list`) as a path the app
/// can use: a Linux path of a repository in WSL becomes its `\\wsl.localhost\…` path.
/// Relative paths and other repositories' output are returned as they are.
pub(super) fn host_path(dir: &Path, printed: &str) -> PathBuf {
    match place(dir) {
        Some(w) if printed.starts_with('/') => {
            PathBuf::from(wsl::to_unc(&w.distro, printed, w.host))
        }
        _ => PathBuf::from(printed),
    }
}

/// The null device as the Git of [`command`] spells it (for `core.hooksPath`).
pub(super) fn null_device(dir: &Path) -> &'static str {
    if place(dir).is_none() && cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wslenv_adds_the_git_variables_and_keeps_the_users_own() {
        let v = wslenv(Some("MYVAR/p:GIT_SSH_COMMAND/u"), FORWARDED);
        let names: Vec<&str> = v.split(':').collect();
        assert!(names.contains(&"MYVAR/p"), "{v}");
        assert!(
            names.contains(&"GIT_SSH_COMMAND/u"),
            "kept as the user wrote it"
        );
        assert_eq!(
            names
                .iter()
                .filter(|n| n.starts_with("GIT_SSH_COMMAND"))
                .count(),
            1
        );
        assert!(names.contains(&"GIT_OPTIONAL_LOCKS") && names.contains(&"GIT_TERMINAL_PROMPT"));
        assert!(wslenv(None, FORWARDED).starts_with("GIT_OPTIONAL_LOCKS"));
    }

    #[test]
    fn wsl_forwarding_distinguishes_unset_and_explicit_protocol_limits() {
        let mut cmd = Command::new("wsl.exe");
        cmd.env(DISTRO_MARK, "Ubuntu-26.04").env("WSLENV", "");
        for key in FORWARDED {
            cmd.env_remove(key);
        }
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        forward_env(&mut cmd);
        let forwarded = |cmd: &Command| {
            cmd.get_envs()
                .find(|(key, _)| *key == "WSLENV")
                .and_then(|(_, value)| value)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(forwarded(&cmd), "GIT_TERMINAL_PROMPT");
        for protocols in ["https:ssh", ""] {
            cmd.env("GIT_ALLOW_PROTOCOL", protocols);
            forward_env(&mut cmd);
            assert!(forwarded(&cmd)
                .split(':')
                .any(|key| key == "GIT_ALLOW_PROTOCOL"));
        }
    }

    #[test]
    fn ordinary_folders_are_left_alone() {
        let d = Path::new("/home/u/repo");
        assert!(place(d).is_none());
        assert_eq!(dir_arg(d), d.as_os_str());
        assert_eq!(
            path_arg(d, Path::new("/tmp/x")),
            Path::new("/tmp/x").as_os_str()
        );
        assert_eq!(host_path(d, "/home/u/repo"), PathBuf::from("/home/u/repo"));
        assert_eq!(
            null_device(d),
            if cfg!(windows) { "NUL" } else { "/dev/null" }
        );
    }

    /// Needs a WSL distribution (`GALL_SPEED_WSL_DISTRO` or the first one listed):
    /// `cargo test -p gall-core --lib -- --ignored fingerprints_in_wsl`.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs WSL"]
    fn fingerprints_in_wsl_agree_through_the_shell_and_a_process_of_their_own() {
        let distro = std::env::var("GALL_SPEED_WSL_DISTRO").unwrap_or_else(|_| {
            let listed = Command::new("wsl.exe").args(["-l", "-q"]).output().unwrap();
            wsl::parse_list_quiet(&wsl::decode_output(&listed.stdout))
                .into_iter()
                .find(|name| !wsl::hidden_distro(name))
                .expect("a WSL distribution")
        });
        let inside = |script: &str| {
            let out = Command::new("wsl.exe")
                .args(["-d", &distro, "--exec", "sh", "-c", script])
                .output()
                .unwrap();
            assert!(out.status.success(), "{script}");
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        let linux = inside("d=$(mktemp -d) && cd \"$d\" && mkdir 'a b' && printf x > 'a b/it'\\''s' && printf yy > 'ž č.txt' && printf z > '-dash' && echo \"$d\"");
        let w = WslPath {
            distro: distro.clone(),
            linux: linux.clone(),
            host: wsl::Host::Localhost,
        };
        let names: Vec<String> = ["a b/it's", "ž č.txt", "-dash", "vanished", "a b"]
            .map(String::from)
            .to_vec();
        let shell = fingerprint_in_shell(&w, &names).expect("a shell in the distribution");
        let started = fingerprint_started(&w, &names);
        inside(&format!("rm -rf '{linux}'"));
        assert!(shell.as_ref().is_some_and(|out| !out.is_empty()));
        assert_eq!(shell, started);
    }

    #[cfg(windows)]
    #[test]
    fn a_repository_in_wsl_gets_linux_paths_and_gives_back_unc_paths() {
        let d = Path::new(r"\\wsl.localhost\Ubuntu-26.04\home\u\repo");
        assert_eq!(dir_arg(d), OsString::from("/home/u/repo"));
        assert_eq!(
            path_arg(d, Path::new(r"\\wsl.localhost\Ubuntu-26.04\home\u\wt")),
            OsString::from("/home/u/wt")
        );
        assert_eq!(
            path_arg(d, Path::new(r"C:\Temp\a.patch")),
            OsString::from("/mnt/c/Temp/a.patch")
        );
        assert_eq!(
            host_path(d, "/home/u/repo"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu-26.04\home\u\repo")
        );
        assert_eq!(host_path(d, ".git"), PathBuf::from(".git"));
        assert_eq!(null_device(d), "/dev/null");
        assert_eq!(null_device(Path::new(r"C:\repo")), "NUL");
        // Another distribution is not reachable from this one.
        let other = Path::new(r"\\wsl.localhost\Debian\home\u\wt");
        assert_eq!(path_arg(d, other), other.as_os_str());
    }
}

#[cfg(test)]
mod flatpak_tests {
    use super::*;

    #[test]
    fn flatpak_passes_the_pinned_attribute_source_to_the_host_git() {
        let cmd = host_git(None, Some("0123456789abcdef"));
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let source = args
            .iter()
            .position(|arg| arg == "--env=GIT_ATTR_SOURCE=0123456789abcdef")
            .unwrap();
        let git = args.iter().position(|arg| arg == "git").unwrap();
        assert!(source < git);
        assert!(!host_git(None, None)
            .get_args()
            .any(|arg| arg.to_string_lossy().starts_with("--env=GIT_ATTR_SOURCE=")));
    }

    #[test]
    fn git_in_a_flatpak_sandbox_runs_on_the_host_with_gall_s_environment() {
        let cmd = host_git(None, None);
        assert_eq!(cmd.get_program(), "flatpak-spawn");
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--host",
                "--env=GIT_OPTIONAL_LOCKS=0",
                "--env=GIT_TERMINAL_PROMPT=0",
                "--env=LC_ALL=C",
                "git"
            ]
        );
    }

    #[test]
    fn outside_a_sandbox_git_starts_as_before() {
        // The suite does not run in a Flatpak sandbox; a repository's folder changes nothing.
        if crate::util::hostspawn::in_flatpak() {
            return;
        }
        let native = Path::new("/usr/bin/git");
        for cmd in [command(Path::new("/tmp"), native), plain(native)] {
            assert_eq!(cmd.get_program(), native.as_os_str());
            assert_eq!(cmd.get_args().count(), 0);
        }
    }
}
