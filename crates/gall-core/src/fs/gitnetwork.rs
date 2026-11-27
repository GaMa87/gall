//! Cancellable remote operations using the user's installed Git and credentials.
use super::gitgraph::{git, root};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
#[cfg(windows)]
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkAction {
    Fetch,
    Pull,
    PullMerge,
    PullRebase,
    Push,
    PushTag,
    DeleteTag,
    ForcePush,
    /// Deletes a branch on the remote (`git push <remote> --delete`).
    DeleteBranch,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub id: String,
    pub text: String,
}

fn failure(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// A tag advertised by one remote. Annotated tags carry the peeled target separately.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTag {
    pub name: String,
    pub id: String,
    pub peeled: Option<String>,
}

/// List a remote's tags without fetching or changing references.
pub fn remote_tags(path: &Path, remote: &str, cancel: &AtomicBool) -> Result<Vec<RemoteTag>> {
    let root = root(path)?;
    let state = super::gitrepository::checked(&root, false)?;
    if remote.starts_with('-') || !state.remotes.iter().any(|item| item.name == remote) {
        return Err(failure("Select an existing remote."));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(failure("Remote operation cancelled before starting."));
    }
    super::gitprogress::report_phase("Listing remote tags", Some(remote.to_owned()));
    // Stopping wsl.exe does not stop Git and SSH inside the distribution, so there the
    // lookup runs as its own bounded process group that Gall can end (see `WslLookup`).
    #[cfg(windows)]
    let wsl = super::gitexec::place(&root).map(|place| WslLookup::new(place.distro));
    #[cfg(windows)]
    let mut command = match &wsl {
        Some(lookup) => super::gitexec::wsl_command(&root, &lookup.prefix())
            .ok_or_else(|| failure("Remote tag lookup is unavailable."))?,
        None => super::gitexec::command(&root, super::gitstatus::program()),
    };
    #[cfg(not(windows))]
    let mut command = super::gitexec::command(&root, super::gitstatus::program());
    command
        .args(["-c", "core.fsmonitor=false", "-c"])
        .arg(format!(
            "core.hooksPath={}",
            super::gitexec::null_device(&root)
        ))
        .args(["-c", "protocol.ext.allow=never", "-C"])
        .arg(super::gitexec::dir_arg(&root))
        .args(["ls-remote", "--tags", "--", remote])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env(
            "GIT_ASKPASS",
            if cfg!(windows) && super::gitexec::place(&root).is_none() {
                ""
            } else {
                "/bin/false"
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
    #[cfg(windows)]
    if wsl.is_some() {
        command.stderr(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|error| failure(error.to_string()))?;
    #[cfg(windows)]
    let mut wsl = wsl;
    #[cfg(windows)]
    if let (Some(lookup), Some(stderr)) = (wsl.as_mut(), child.stderr.take()) {
        lookup.watch(stderr);
    }
    let stop = |child: &mut std::process::Child| {
        #[cfg(windows)]
        if let Some(lookup) = &wsl {
            lookup.stop(child);
        }
        stop_remote_tag_child(child);
    };
    let Some(mut output) = child.stdout.take() else {
        stop(&mut child);
        return Err(failure("Remote output unavailable."));
    };
    let (tx, rx) = mpsc::channel();
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = (&mut output)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(30) {
            stop(&mut child);
            return Err(failure("Remote tag lookup cancelled or timed out."));
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                stop(&mut child);
                return Err(failure(error.to_string()));
            }
        };
        if let Some(status) = status {
            if !status.success() {
                return Err(failure("Remote tag lookup failed."));
            }
            let bytes = rx
                .recv_timeout(Duration::from_secs(1))
                .map_err(|_| failure("Remote tag output did not close."))?
                .map_err(|error| failure(error.to_string()))?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(failure("Remote tag list exceeds the size limit."));
            }
            return parse_remote_tags(&bytes);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A remote tag lookup inside a WSL distribution. The command runs as
/// `env GALL_REMOTE_TAGS=<token> sh -c 'echo $$ >&2; exec timeout …' git …`: the shell
/// reports its process ID and becomes `timeout`, which leads the process group of Git and
/// SSH and ends it after `LIMIT` seconds. Cancel kills that group, after checking the token
/// in the leader's environment so a reused process ID is never signalled.
#[cfg(windows)]
struct WslLookup {
    distro: String,
    token: String,
    pid: Option<mpsc::Receiver<u32>>,
}

#[cfg(windows)]
impl WslLookup {
    const START: &'static str = "echo $$ >&2; exec timeout -k 2 25 \"$@\"";
    const STOP: &'static str = "tr '\\0' '\\n' < \"/proc/$1/environ\" 2>/dev/null \
        | grep -qx \"GALL_REMOTE_TAGS=$2\" || exit 0; \
        kill -KILL -- \"-$1\" 2>/dev/null || kill -KILL \"$1\"";

    fn new(distro: String) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or_default();
        let token = format!(
            "{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            distro,
            token,
            pid: None,
        }
    }

    fn prefix(&self) -> [String; 6] {
        [
            "env".into(),
            format!("GALL_REMOTE_TAGS={}", self.token),
            "sh".into(),
            "-c".into(),
            Self::START.into(),
            "gall-remote-tags".into(),
        ]
    }

    /// Reads the process ID from the first line of `stderr` and discards the rest.
    fn watch(&mut self, stderr: std::process::ChildStderr) {
        let (tx, rx) = mpsc::channel();
        self.pid = Some(rx);
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(stderr);
            let mut line = String::new();
            if reader.read_line(&mut line).is_ok() {
                if let Ok(pid) = line.trim().parse::<u32>() {
                    let _ = tx.send(pid);
                }
            }
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
        });
    }

    /// Ends the process group inside the distribution and waits briefly for `child`
    /// (wsl.exe) to follow; the caller then ends wsl.exe itself.
    fn stop(&self, child: &mut std::process::Child) {
        let Some(rx) = &self.pid else { return };
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid = loop {
            match rx.try_recv() {
                Ok(pid) => break Some(pid),
                Err(mpsc::TryRecvError::Disconnected) => break None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if !matches!(child.try_wait(), Ok(None)) || Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let Some(pid) = pid else { return };
        use std::os::windows::process::CommandExt;
        let killer = Command::new(super::wsl::wsl_exe())
            .args(["-d", &self.distro, "--exec", "sh", "-c", Self::STOP])
            .args(["gall-remote-tags-stop", &pid.to_string(), &self.token])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn();
        if let Ok(mut killer) = killer {
            wait_bounded(&mut killer, Duration::from_secs(5));
            let _ = killer.kill();
            let _ = killer.wait();
        }
        wait_bounded(child, Duration::from_secs(3));
    }
}

#[cfg(windows)]
fn wait_bounded(child: &mut std::process::Child, limit: Duration) {
    let started = Instant::now();
    while matches!(child.try_wait(), Ok(None)) && started.elapsed() < limit {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stop_remote_tag_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let mut killer = Command::new("taskkill");
        use std::os::windows::process::CommandExt;
        let _ = killer
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x08000000)
            .output();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn parse_remote_tags(bytes: &[u8]) -> Result<Vec<RemoteTag>> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| failure("Remote tag output is not UTF-8."))?;
    let mut tags = std::collections::BTreeMap::<String, RemoteTag>::new();
    for line in text.lines() {
        let (id, reference) = line
            .split_once('\t')
            .ok_or_else(|| failure("Invalid remote tag output."))?;
        if !matches!(id.len(), 40 | 64) || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(failure("Invalid remote tag object ID."));
        }
        let name = reference
            .strip_prefix("refs/tags/")
            .ok_or_else(|| failure("Invalid remote tag reference."))?;
        let peeled = name.ends_with("^{}");
        let name = name.strip_suffix("^{}").unwrap_or(name);
        if name.is_empty() {
            return Err(failure("Invalid remote tag name."));
        }
        let entry = tags.entry(name.to_owned()).or_insert_with(|| RemoteTag {
            name: name.to_owned(),
            id: String::new(),
            peeled: None,
        });
        if peeled {
            if entry.peeled.is_some() {
                return Err(failure("Duplicate remote tag output."));
            }
            entry.peeled = Some(id.to_owned());
        } else {
            if !entry.id.is_empty() {
                return Err(failure("Duplicate remote tag output."));
            }
            entry.id = id.to_owned();
        }
    }
    if tags.values().any(|tag| tag.id.is_empty()) {
        return Err(failure("Incomplete remote tag output."));
    }
    Ok(tags.into_values().collect())
}

/// `lease` is the reviewed remote branch SHA, never an implicit moving tracking ref.
/// `branch_name` is the short name of the remote branch that `DeleteBranch` removes.
#[allow(clippy::too_many_arguments)]
pub fn run(
    path: &Path,
    remote: &str,
    action: NetworkAction,
    expected: &str,
    lease: Option<&str>,
    branch_name: Option<&str>,
    cancel: &AtomicBool,
    progress: impl Fn(String),
) -> Result<String> {
    let operation = matches!(
        action,
        NetworkAction::Pull | NetworkAction::PullMerge | NetworkAction::PullRebase
    )
    .then(|| (super::gitjournal::Kind::Pull, remote.to_owned()));
    super::gitjournal::around(path, operation, || {
        run_now(
            path,
            remote,
            action,
            expected,
            lease,
            branch_name,
            cancel,
            progress,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn run_now(
    path: &Path,
    remote: &str,
    action: NetworkAction,
    expected: &str,
    lease: Option<&str>,
    branch_name: Option<&str>,
    cancel: &AtomicBool,
    progress: impl Fn(String),
) -> Result<String> {
    let _busy = super::gitfacts::mutating();
    let root = root(path)?;
    // A fetch looks only at the remotes; the others also at references.
    let state = super::gitrepository::checked(&root, !matches!(action, NetworkAction::Fetch))?;
    if state.token != expected {
        return Err(failure(
            "Repository changed. Refresh and review the remote operation again.",
        ));
    }
    if !state.remotes.iter().any(|r| r.name == remote) || remote.starts_with('-') {
        return Err(failure("Select an existing remote."));
    }
    if state.operation.is_some() && !matches!(action, NetworkAction::DeleteTag) {
        return Err(failure("Finish or abort the current Git operation first."));
    }
    let branch = state.branch.as_deref();
    let mut pull_target = None;
    // Tracked changes before a pull; they must be the same when the fetch is done.
    let mut status_before = String::new();
    let mut args: Vec<String>;
    match action {
        NetworkAction::Fetch => {
            args = vec![
                "fetch".into(),
                "--progress".into(),
                "--prune".into(),
                "--no-recurse-submodules".into(),
                "--".into(),
                remote.into(),
            ]
        }
        NetworkAction::Pull | NetworkAction::PullMerge | NetworkAction::PullRebase => {
            // Local changes, submodules and content filters do not block a pull, as in Git and lazygit: the
            // fast-forward below is `merge --ff-only`, which refuses, without changing anything,
            // when a changed file would be overwritten, and it leaves submodule folders alone.
            status_before = git(&root, &["status", "--porcelain", "--untracked-files=no"])?;
            let current =
                branch.ok_or_else(|| failure("Attach HEAD to a branch before pulling."))?;
            let reference = state
                .refs
                .iter()
                .find(|r| r.kind == "branch" && r.name == current)
                .ok_or_else(|| failure("Current branch no longer exists."))?;
            let upstream = reference
                .upstream
                .as_deref()
                .ok_or_else(|| failure("Set an upstream before pulling."))?;
            let prefix = format!("{remote}/");
            let destination = upstream.strip_prefix(&prefix).ok_or_else(|| {
                failure("Choose the remote configured as this branch's upstream.")
            })?;
            pull_target = Some(format!("refs/remotes/{remote}/{destination}"));
            args = vec![
                "fetch".into(),
                "--no-recurse-submodules".into(),
                "--progress".into(),
                "--".into(),
                remote.into(),
            ];
        }
        NetworkAction::DeleteBranch => {
            let name = branch_name
                .filter(|n| !n.is_empty() && !n.starts_with('-') && *n != "HEAD")
                .ok_or_else(|| failure("Select a remote branch to delete."))?;
            if !state.refs.iter().any(|r| {
                r.kind == "remote" && r.full_name == format!("refs/remotes/{remote}/{name}")
            }) {
                return Err(failure(
                    "The remote branch is no longer known. Fetch and review again.",
                ));
            }
            args = vec![
                "push".into(),
                "--progress".into(),
                "--no-recurse-submodules".into(),
                "--delete".into(),
                "--".into(),
                remote.into(),
                format!("refs/heads/{name}"),
            ];
        }
        NetworkAction::PushTag | NetworkAction::DeleteTag => {
            let name = branch_name
                .filter(|name| !name.is_empty())
                .ok_or_else(|| failure("Select a local tag."))?;
            let tag = format!("refs/tags/{name}");
            if !state
                .refs
                .iter()
                .any(|reference| reference.kind == "tag" && reference.full_name == tag)
            {
                return Err(failure("The selected local tag no longer exists."));
            }
            let refspec = if matches!(action, NetworkAction::DeleteTag) {
                format!(":{tag}")
            } else {
                // Preserve the annotation by pushing the tag object itself.
                let id = git(&root, &["rev-parse", "--verify", &tag])?;
                format!("{}:{tag}", id.trim())
            };
            args = vec![
                "push".into(),
                "--progress".into(),
                "--no-recurse-submodules".into(),
                "--no-follow-tags".into(),
                "--".into(),
                remote.into(),
                refspec,
            ];
        }
        NetworkAction::Push | NetworkAction::ForcePush => {
            let branch =
                branch.ok_or_else(|| failure("Attach HEAD to a branch before pushing."))?;
            let reference = state
                .refs
                .iter()
                .find(|r| r.kind == "branch" && r.name == branch);
            let prefix = format!("{remote}/");
            let destination = reference
                .and_then(|r| r.upstream.as_deref())
                .and_then(|u| u.strip_prefix(&prefix))
                .unwrap_or(branch);
            args = vec![
                "push".into(),
                "--progress".into(),
                "--no-recurse-submodules".into(),
            ];
            // A first push records where the branch goes. A branch that already has an
            // upstream keeps it, also when it is pushed to another remote.
            if reference.is_none_or(|r| r.upstream.is_none()) {
                args.push("--set-upstream".into());
            }
            if matches!(action, NetworkAction::ForcePush) {
                let lease = lease.ok_or_else(|| {
                    failure("An explicit reviewed remote commit is required for force-with-lease.")
                })?;
                if !(lease.len() == 40 || lease.len() == 64)
                    || !lease.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(failure("Invalid lease commit."));
                }
                let known = state
                    .refs
                    .iter()
                    .find(|r| r.full_name == format!("refs/remotes/{remote}/{destination}"));
                if known.is_none_or(|r| r.id != lease) {
                    return Err(failure(
                        "Remote tracking commit changed. Fetch and review before force-with-lease.",
                    ));
                }
                args.push(format!(
                    "--force-with-lease=refs/heads/{destination}:{lease}"
                ));
            }
            args.extend([
                "--".into(),
                remote.into(),
                format!("HEAD:refs/heads/{destination}"),
            ]);
        }
    }
    // For a repository in WSL this runs the Git (and the credentials and SSH keys) of the
    // distribution, as in a terminal there.
    let mut command = super::gitexec::command(&root, super::gitstatus::program());
    command
        .args(["-c", "core.fsmonitor=false", "-c"])
        .arg(format!(
            "core.hooksPath={}",
            super::gitexec::null_device(&root)
        ))
        .args(["-c", "protocol.ext.allow=never", "-C"])
        .arg(super::gitexec::dir_arg(&root))
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env(
            "GIT_ASKPASS",
            if cfg!(windows) && super::gitexec::place(&root).is_none() {
                ""
            } else {
                "/bin/false"
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
    if cancel.load(Ordering::Relaxed) {
        return Err(failure("Remote operation cancelled before starting."));
    }
    super::gitexec::forward_env(&mut command);
    let mut child = command.spawn().map_err(|e| failure(e.to_string()))?;
    let (tx, rx) = mpsc::channel();
    for mut pipe in [
        Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().unwrap()),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(n) = pipe.read(&mut bytes) {
                if n == 0
                    || tx
                        .send(String::from_utf8_lossy(&bytes[..n]).into_owned())
                        .is_err()
                {
                    break;
                }
            }
        });
    }
    drop(tx);
    // Inactivity deadline: progress output proves the transfer is advancing.
    let mut last_activity = Instant::now();
    let mut output = String::new();
    let mut closed = false;
    loop {
        for _ in 0..128 {
            let Ok(chunk) = rx.try_recv() else {
                break;
            };
            last_activity = Instant::now();
            super::gitprogress::feed_stderr(&chunk);
            progress(chunk.clone());
            keep_output(&mut output, &chunk);
        }
        if cancel.load(Ordering::Relaxed) || last_activity.elapsed() > Duration::from_secs(300) {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            #[cfg(windows)]
            {
                let mut killer = Command::new("taskkill");
                use std::os::windows::process::CommandExt;
                let _ = killer
                    .args(["/PID", &child.id().to_string(), "/T", "/F"])
                    .creation_flags(0x08000000)
                    .output();
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(failure("Remote operation stopped. Refresh to inspect any transfer that already completed; cancellation cannot undo a push."));
        }
        if let Some(status) = child.try_wait().map_err(|e| failure(e.to_string()))? {
            // Readers finish after the process closes its pipes; bounded wait also handles credential helpers retaining a pipe.
            for _ in 0..20 {
                match rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(chunk) => {
                        super::gitprogress::feed_stderr(&chunk);
                        progress(chunk.clone());
                        output.push_str(&chunk);
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(_) => {}
                }
            }
            return if status.success() {
                if let Some(target) = &pull_target {
                    let latest = super::gitrepository::snapshot(&root)?;
                    if latest.head != state.head
                        || latest.branch != state.branch
                        || latest.operation.is_some()
                        || latest.bisect_active
                        || git(&root, &["status", "--porcelain", "--untracked-files=no"])?
                            != status_before
                    {
                        return Err(failure("Fetch completed, but the working repository changed. Review it before fast-forwarding."));
                    }
                    if cancel.load(Ordering::Relaxed) {
                        return Err(failure("Fetch completed; fast-forward cancelled."));
                    }
                    let id = git(
                        &root,
                        &["rev-parse", "--verify", &format!("{target}^{{commit}}")],
                    )?;
                    let config =
                        |key: &str| git(&root, &["config", "--get", key]).unwrap_or_default();
                    let ff = config("pull.ff");
                    // Diverged: HEAD is neither behind nor level with the fetched commit. Like `git pull`,
                    // follow the configured strategy; with none, say how to go on in the app.
                    let head_id = git(&root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
                    let base = git(&root, &["merge-base", head_id.trim(), id.trim()])?;
                    if base.trim() != head_id.trim() && base.trim() != id.trim() {
                        let strategy = match action {
                            NetworkAction::PullMerge => "false".to_owned(),
                            NetworkAction::PullRebase => "true".to_owned(),
                            _ if ff.trim() == "only" => String::new(),
                            _ => state
                                .branch
                                .as_ref()
                                .map(|branch| config(&format!("branch.{branch}.rebase")))
                                .filter(|value| !value.trim().is_empty())
                                .unwrap_or_else(|| config("pull.rebase")),
                        };
                        if matches!(strategy.trim(), "true" | "yes" | "on" | "1") {
                            super::gitprogress::report_phase("Rebasing", None);
                            output.push_str(&super::gitrepository::execute(
                                &root,
                                super::gitrepository::Request::Rebase {
                                    target: id.trim().into(),
                                },
                                &latest.token,
                            )?);
                            output.push_str(&super::gitrewrite::submodule_note(&root));
                            return Ok(output);
                        }
                        if !matches!(strategy.trim(), "false" | "no" | "off" | "0") {
                            return Err(failure(format!(
                                "Fetch completed, but {current} and {target} have diverged. Choose Merge or Rebase in the Pull dialog.",
                                current = state.branch.as_deref().unwrap_or("HEAD"),
                                target = target.trim_start_matches("refs/remotes/"),
                            )));
                        }
                        // An untracked file (an ignored one too) at a path the merge writes is refused by name and keeps
                        // its bytes: Git's own `--no-overwrite-ignore` is not honoured by every merge strategy.
                        super::gitrepository::protect_untracked(&root, &[id.trim()], None)?;
                        super::gitprogress::report_phase("Merging", None);
                        let mut merge_args = vec![
                            "-c",
                            "submodule.recurse=false",
                            "merge",
                            if ff.trim() == "false" {
                                "--no-ff"
                            } else {
                                "--ff"
                            },
                            "--no-edit",
                            "--no-autostash",
                            "--no-overwrite-ignore",
                        ];
                        if super::gitprogress::is_active() {
                            merge_args.push("--progress");
                        }
                        merge_args.push(id.trim());
                        output.push_str(&git(&root, &merge_args)?);
                        output.push_str(&super::gitrewrite::submodule_note(&root));
                        return Ok(output);
                    }
                    let ff_mode =
                        if matches!(action, NetworkAction::Pull | NetworkAction::PullMerge)
                            && matches!(ff.trim(), "false" | "no" | "off" | "0")
                        {
                            "--no-ff"
                        } else {
                            "--ff-only"
                        };
                    super::gitrepository::protect_untracked(&root, &[id.trim()], None)?;
                    super::gitprogress::report_phase("Merging", None);
                    let mut ff_args = vec![
                        "-c",
                        "merge.autoStash=false",
                        "-c",
                        "submodule.recurse=false",
                        "merge",
                        ff_mode,
                        "--no-edit",
                        "--no-overwrite-ignore",
                        "--no-autostash",
                    ];
                    if super::gitprogress::is_active() {
                        ff_args.push("--progress");
                    }
                    ff_args.push(id.trim());
                    output.push_str(&git(&root, &ff_args)?);
                    output.push_str(&super::gitrewrite::submodule_note(&root));
                }
                Ok(if output.trim().is_empty() {
                    "Remote operation completed.".into()
                } else {
                    output
                })
            } else {
                Err(failure(output.trim().to_owned()))
            };
        }
        // Once both pipes are closed Git is ending: look for its exit often, still minding
        // cancel. Until then wake for output right away, otherwise to look at cancel.
        if closed {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(chunk) => {
                last_activity = Instant::now();
                super::gitprogress::feed_stderr(&chunk);
                progress(chunk.clone());
                keep_output(&mut output, &chunk);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => closed = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Appends `chunk` to the collected output, keeping only its last 64 KiB once it exceeds
/// 128 KiB.
fn keep_output(output: &mut String, chunk: &str) {
    output.push_str(chunk);
    if output.len() > 128 * 1024 {
        let cut = output
            .char_indices()
            .find(|(i, _)| *i >= output.len() - 64 * 1024)
            .map(|(i, _)| i)
            .unwrap_or(0);
        output.drain(..cut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_tags_preserves_names_and_peeled_targets() {
        let first = "1".repeat(40);
        let second = "2".repeat(40);
        let third = "3".repeat(40);
        let output = format!(
            "{first}\trefs/tags/release/a\n{second}\trefs/tags/annotated\n{third}\trefs/tags/annotated^{{}}\n"
        );
        let tags = parse_remote_tags(output.as_bytes()).unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name, "annotated");
        assert_eq!(tags[0].id, second);
        assert_eq!(tags[0].peeled.as_deref(), Some(third.as_str()));
        assert_eq!(tags[1].name, "release/a");
        assert_eq!(tags[1].id, first);
        assert_eq!(tags[1].peeled, None);
        assert!(parse_remote_tags(b"invalid\trefs/tags/v1\n").is_err());
        assert!(parse_remote_tags(format!("{first}\trefs/tags/v1^{{}}\n").as_bytes()).is_err());
    }

    #[test]
    fn remote_tags_reads_a_local_remote_and_rejects_cancellation() {
        let repo = tempfile::tempdir().unwrap();
        let bare = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]).unwrap();
        git(repo.path(), &["config", "core.autocrlf", "false"]).unwrap();
        let excludes = repo.path().join(".git").join("empty-excludes");
        std::fs::write(&excludes, "").unwrap();
        git(
            repo.path(),
            &["config", "core.excludesFile", excludes.to_str().unwrap()],
        )
        .unwrap();
        git(repo.path(), &["config", "user.name", "Test"]).unwrap();
        git(repo.path(), &["config", "user.email", "test@example.com"]).unwrap();
        std::fs::write(repo.path().join("file"), "first").unwrap();
        git(repo.path(), &["add", "."]).unwrap();
        git(repo.path(), &["commit", "-qm", "first"]).unwrap();
        git(repo.path(), &["tag", "light"]).unwrap();
        git(repo.path(), &["tag", "v1"]).unwrap();
        git(repo.path(), &["tag", "-a", "annotated", "-m", "tag"]).unwrap();
        git(bare.path(), &["init", "--bare", "-q"]).unwrap();
        git(
            repo.path(),
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        )
        .unwrap();
        git(repo.path(), &["push", "-q", "origin", "--tags"]).unwrap();
        std::fs::write(repo.path().join("file"), "second").unwrap();
        git(repo.path(), &["commit", "-qam", "second"]).unwrap();
        git(repo.path(), &["branch", "tags/v1"]).unwrap();
        git(
            repo.path(),
            &[
                "push",
                "-q",
                "origin",
                "refs/heads/tags/v1:refs/heads/tags/v1",
            ],
        )
        .unwrap();
        git(repo.path(), &["fetch", "-q", "origin", "--tags"]).unwrap();
        let branch = git(repo.path(), &["rev-parse", "refs/remotes/origin/tags/v1"]).unwrap();
        let cancel = AtomicBool::new(false);
        let tags = remote_tags(repo.path(), "origin", &cancel).unwrap();
        assert_eq!(
            tags.iter().map(|tag| tag.name.as_str()).collect::<Vec<_>>(),
            ["annotated", "light", "v1"]
        );
        let head = git(repo.path(), &["rev-parse", "HEAD"]).unwrap();
        let tagged = git(repo.path(), &["rev-parse", "HEAD^"]).unwrap();
        assert_eq!(branch.trim(), head.trim());
        assert_eq!(tags[0].peeled.as_deref(), Some(tagged.trim()));
        assert_eq!(tags[1].id, tagged.trim());
        assert_eq!(tags[2].id, tagged.trim());
        cancel.store(true, Ordering::Relaxed);
        assert!(remote_tags(repo.path(), "origin", &cancel).is_err());
        assert!(remote_tags(repo.path(), "-invalid", &AtomicBool::new(false)).is_err());
    }

    /// Needs a WSL distribution (`GALL_SPEED_WSL_DISTRO` or the first one listed) and
    /// reaches out to an unroutable address:
    /// `cargo test -p gall-core --lib -- --ignored remote_tags_in_wsl`.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs WSL"]
    fn remote_tags_in_wsl_lists_fails_offline_and_cancels_without_leftovers() {
        use super::super::wsl;
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
        const OFFLINE: &str = "10.255.255.1";
        let linux = inside(&format!(
            "d=$(mktemp -d) && cd \"$d\" && git init -q --bare b.git && git init -q -b main r \
             && cd r && git -c user.name=T -c user.email=t@example.com commit -q --allow-empty -m one \
             && git tag v1 && git remote add origin ../b.git && git push -q origin main --tags \
             && git remote add offline 'ssh://git@{OFFLINE}/x.git' && echo \"$d\""
        ));
        let repo = std::path::PathBuf::from(wsl::to_unc(
            &distro,
            &format!("{linux}/r"),
            wsl::Host::Localhost,
        ));
        let leftovers = || {
            inside(&format!(
                "ps -eo args | grep -c '[{}]{}' || true",
                &OFFLINE[..1],
                &OFFLINE[1..]
            ))
        };

        let tags = remote_tags(&repo, "origin", &AtomicBool::new(false));
        let started = Instant::now();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            flag.store(true, Ordering::Relaxed);
        });
        let cancelled = remote_tags(&repo, "offline", &cancel);
        let cancel_time = started.elapsed();
        canceller.join().unwrap();
        let after_cancel = leftovers();
        let failed = remote_tags(&repo, "offline", &AtomicBool::new(false));
        let after_failure = leftovers();
        inside(&format!("rm -rf '{linux}'"));

        let tags = tags.unwrap();
        assert_eq!(
            tags.iter().map(|tag| tag.name.as_str()).collect::<Vec<_>>(),
            ["v1"]
        );
        assert!(cancelled.is_err());
        assert!(cancel_time < Duration::from_secs(10), "{cancel_time:?}");
        assert_eq!(after_cancel, "0");
        assert!(failed.is_err());
        assert_eq!(after_failure, "0");
    }

    #[test]
    fn local_remote_round_trip_and_stale_lease() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.com"]).unwrap();
        std::fs::write(p.join("file"), "first").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "first"]).unwrap();
        let bare = tempfile::tempdir().unwrap();
        git(bare.path(), &["init", "--bare", "-q"]).unwrap();
        git(
            p,
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        )
        .unwrap();
        let s = super::super::gitrepository::snapshot(p).unwrap();
        let cancel = AtomicBool::new(false);
        run(
            p,
            "origin",
            NetworkAction::Push,
            &s.token,
            None,
            None,
            &cancel,
            |_| {},
        )
        .unwrap();
        let s = super::super::gitrepository::snapshot(p).unwrap();
        run(
            p,
            "origin",
            NetworkAction::Fetch,
            &s.token,
            None,
            None,
            &cancel,
            |_| {},
        )
        .unwrap();
        let s = super::super::gitrepository::snapshot(p).unwrap();
        run(
            p,
            "origin",
            NetworkAction::Pull,
            &s.token,
            None,
            None,
            &cancel,
            |_| {},
        )
        .unwrap();
        assert!(run(
            p,
            "origin",
            NetworkAction::Push,
            "stale",
            None,
            None,
            &cancel,
            |_| {}
        )
        .is_err());
        assert!(run(
            p,
            "origin",
            NetworkAction::ForcePush,
            &s.token,
            Some(&"0".repeat(40)),
            None,
            &cancel,
            |_| {}
        )
        .is_err());
        cancel.store(true, Ordering::Relaxed);
        assert!(run(
            p,
            "origin",
            NetworkAction::Fetch,
            &s.token,
            None,
            None,
            &cancel,
            |_| {}
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(p.join("file")).unwrap(), "first");
    }

    /// What lazygit allows: a pull with local changes in files the update does not touch, and in a
    /// repository that has a `.gitmodules` file. An update that would overwrite a changed file is
    /// refused and the change stays.
    #[test]
    fn a_pull_runs_with_unrelated_local_changes_and_refuses_to_overwrite_one() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("work");
        let other = d.path().join("other");
        let bare = d.path().join("bare.git");
        std::fs::create_dir(&p).unwrap();
        git(&p, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(&p, &["config", key, value]).unwrap();
        }
        std::fs::write(p.join("mine.txt"), "base\n").unwrap();
        std::fs::write(p.join("shared.txt"), "base\n").unwrap();
        std::fs::write(p.join(".gitmodules"), "").unwrap();
        git(&p, &["add", "."]).unwrap();
        git(&p, &["commit", "-qm", "first"]).unwrap();
        std::fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "--bare", "-q", "-b", "main"]).unwrap();
        git(&p, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();
        git(&p, &["push", "-q", "-u", "origin", "main"]).unwrap();
        git(
            d.path(),
            &[
                "-c",
                "core.autocrlf=false",
                "clone",
                "-q",
                bare.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        )
        .unwrap();
        for (key, value) in [
            ("user.name", "Other"),
            ("user.email", "other@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(&other, &["config", key, value]).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let pull = || {
            let s = super::super::gitrepository::snapshot(&p).unwrap();
            run(
                &p,
                "origin",
                NetworkAction::Pull,
                &s.token,
                None,
                None,
                &cancel,
                |_| {},
            )
        };
        let text = |file: &str| std::fs::read_to_string(p.join(file)).unwrap();
        // The other side changes `new.txt` only; here `mine.txt` is edited, once also staged.
        std::fs::write(other.join("new.txt"), "new\n").unwrap();
        git(&other, &["add", "."]).unwrap();
        git(&other, &["commit", "-qm", "new file"]).unwrap();
        git(&other, &["push", "-q"]).unwrap();
        // A content filter does not stop the pull either (the other filter-free paths stay as they are).
        git(&p, &["config", "filter.keep.clean", "cat"]).unwrap();
        git(&p, &["config", "filter.keep.smudge", "cat"]).unwrap();
        std::fs::write(p.join(".git/info/attributes"), "*.txt filter=keep\n").unwrap();
        std::fs::write(p.join("mine.txt"), "local edit\n").unwrap();
        git(&p, &["add", "mine.txt"]).unwrap();
        std::fs::write(p.join("mine.txt"), "local edit, more\n").unwrap();
        pull().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(text("new.txt"), "new\n");
        assert_eq!(text("mine.txt"), "local edit, more\n");
        // The other side now changes the file that is edited here.
        std::fs::write(other.join("shared.txt"), "other change\n").unwrap();
        git(&other, &["commit", "-qam", "shared"]).unwrap();
        git(&other, &["push", "-q"]).unwrap();
        std::fs::write(p.join("shared.txt"), "my change\n").unwrap();
        let before = git(&p, &["rev-parse", "HEAD"]).unwrap();
        assert!(pull().is_err());
        assert_eq!(text("shared.txt"), "my change\n");
        assert_eq!(git(&p, &["rev-parse", "HEAD"]).unwrap(), before);
    }

    #[test]
    fn a_diverged_pull_says_how_to_go_on_and_merges_when_configured() {
        crate::fs::hermetic_git();
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("work");
        let other = d.path().join("other");
        let bare = d.path().join("bare.git");
        std::fs::create_dir(&p).unwrap();
        git(&p, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(&p, &["config", key, value]).unwrap();
        }
        std::fs::write(p.join("base.txt"), "base\n").unwrap();
        git(&p, &["add", "."]).unwrap();
        git(&p, &["commit", "-qm", "first"]).unwrap();
        std::fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "--bare", "-q", "-b", "main"]).unwrap();
        git(&p, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();
        git(&p, &["push", "-q", "-u", "origin", "main"]).unwrap();
        git(
            d.path(),
            &[
                "-c",
                "core.autocrlf=false",
                "clone",
                "-q",
                bare.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        )
        .unwrap();
        for (key, value) in [
            ("user.name", "Other"),
            ("user.email", "other@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(&other, &["config", key, value]).unwrap();
        }
        std::fs::write(other.join("theirs.txt"), "theirs\n").unwrap();
        git(&other, &["add", "."]).unwrap();
        git(&other, &["commit", "-qm", "theirs"]).unwrap();
        git(&other, &["push", "-q"]).unwrap();
        std::fs::write(p.join("ours.txt"), "ours\n").unwrap();
        git(&p, &["add", "."]).unwrap();
        git(&p, &["commit", "-qm", "ours"]).unwrap();
        let cancel = AtomicBool::new(false);
        let pull = |action| {
            let s = super::super::gitrepository::snapshot(&p).unwrap();
            run(&p, "origin", action, &s.token, None, None, &cancel, |_| {})
        };
        // No strategy configured: nothing changes, and the message names the next step in the app.
        let before = git(&p, &["rev-parse", "HEAD"]).unwrap();
        let error = pull(NetworkAction::Pull).unwrap_err().to_string();
        assert!(
            error.contains("diverged") && error.contains("Rebase"),
            "{error}"
        );
        assert_eq!(git(&p, &["rev-parse", "HEAD"]).unwrap(), before);
        assert!(!p.join("theirs.txt").exists());
        git(&p, &["config", "pull.rebase", "false"]).unwrap();
        git(&p, &["config", "branch.main.rebase", "true"]).unwrap();
        git(&p, &["config", "pull.ff", "only"]).unwrap();
        assert!(pull(NetworkAction::Pull)
            .unwrap_err()
            .to_string()
            .contains("diverged"));
        assert_eq!(git(&p, &["rev-parse", "HEAD"]).unwrap(), before);
        git(&p, &["config", "--unset", "pull.ff"]).unwrap();
        pull(NetworkAction::Pull).unwrap();
        assert_eq!(
            git(&p, &["rev-parse", "HEAD^"]).unwrap(),
            git(&p, &["rev-parse", "origin/main"]).unwrap()
        );
        git(&p, &["reset", "--hard", before.trim()]).unwrap();
        git(&p, &["config", "--unset", "branch.main.rebase"]).unwrap();

        pull(NetworkAction::PullRebase).unwrap();
        assert_eq!(
            git(&p, &["rev-parse", "HEAD^"]).unwrap(),
            git(&p, &["rev-parse", "origin/main"]).unwrap()
        );
        assert!(p.join("theirs.txt").exists() && p.join("ours.txt").exists());
        git(&p, &["reset", "--hard", before.trim()]).unwrap();

        std::fs::write(p.join(".git/info/exclude"), "theirs.txt\n").unwrap();
        std::fs::write(p.join("theirs.txt"), "local ignored bytes\n").unwrap();
        let blocked = pull(NetworkAction::PullMerge).unwrap_err().to_string();
        assert!(blocked.contains("theirs.txt"), "{blocked}");
        assert_eq!(git(&p, &["rev-parse", "HEAD"]).unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(p.join("theirs.txt")).unwrap(),
            "local ignored bytes\n"
        );
        std::fs::remove_file(p.join("theirs.txt")).unwrap();
        git(&p, &["config", "merge.ff", "only"]).unwrap();
        pull(NetworkAction::PullMerge).unwrap();
        assert_eq!(
            git(&p, &["rev-list", "--parents", "-n", "1", "HEAD"])
                .unwrap()
                .split_whitespace()
                .count(),
            3
        );
        git(&p, &["reset", "--hard", before.trim()]).unwrap();
        git(&p, &["config", "--unset", "merge.ff"]).unwrap();
        // `pull.rebase=false` asks for a merge, as with `git pull`.
        git(&p, &["config", "pull.rebase", "false"]).unwrap();
        pull(NetworkAction::Pull).unwrap_or_else(|e| panic!("{e}"));
        assert!(p.join("theirs.txt").exists() && p.join("ours.txt").exists());
        assert_eq!(
            git(&p, &["rev-list", "--parents", "-n", "1", "HEAD"])
                .unwrap()
                .split_whitespace()
                .count(),
            3
        );
        // Git's pull.ff=false creates a merge commit even when it could fast-forward.
        git(&p, &["reset", "--hard", &format!("{}^", before.trim())]).unwrap();
        git(&p, &["config", "pull.ff", "false"]).unwrap();
        pull(NetworkAction::Pull).unwrap();
        assert_eq!(
            git(&p, &["rev-list", "--parents", "-n", "1", "HEAD"])
                .unwrap()
                .split_whitespace()
                .count(),
            3
        );
    }

    #[test]
    fn push_to_another_remote_keeps_the_upstream() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.com"]).unwrap();
        git(p, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(p.join("file"), "first").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "first"]).unwrap();
        let origin = tempfile::tempdir().unwrap();
        let fork = tempfile::tempdir().unwrap();
        for (name, bare) in [("origin", &origin), ("fork", &fork)] {
            git(bare.path(), &["init", "--bare", "-q"]).unwrap();
            git(p, &["remote", "add", name, bare.path().to_str().unwrap()]).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let push = |remote: &str| {
            let s = super::super::gitrepository::snapshot(p).unwrap();
            run(
                p,
                remote,
                NetworkAction::Push,
                &s.token,
                None,
                None,
                &cancel,
                |_| {},
            )
            .unwrap();
        };
        // The first push sets the upstream, a push elsewhere leaves it alone.
        push("origin");
        assert_eq!(
            git(p, &["config", "branch.main.remote"]).unwrap().trim(),
            "origin"
        );
        push("fork");
        assert_eq!(
            git(p, &["config", "branch.main.remote"]).unwrap().trim(),
            "origin"
        );
        assert_eq!(
            git(p, &["rev-parse", "refs/remotes/fork/main"])
                .unwrap()
                .trim(),
            git(p, &["rev-parse", "HEAD"]).unwrap().trim()
        );
    }

    #[test]
    fn push_tag_publishes_only_the_selected_annotation_and_never_replaces_a_remote_tag() {
        let d = tempfile::tempdir().unwrap();
        let work = d.path().join("work");
        let bare = d.path().join("bare.git");
        std::fs::create_dir(&work).unwrap();
        std::fs::create_dir(&bare).unwrap();
        git(&work, &["init", "-q", "-b", "main"]).unwrap();
        git(&bare, &["init", "-q", "--bare", "-b", "main"]).unwrap();
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(&work, &["config", key, value]).unwrap();
        }
        git(&work, &["commit", "--allow-empty", "-qm", "initial"]).unwrap();
        git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();
        git(&work, &["config", "push.followTags", "true"]).unwrap();
        git(&work, &["tag", "-a", "release", "-m", "annotation"]).unwrap();
        git(
            &work,
            &["tag", "-a", "unselected", "-m", "other annotation"],
        )
        .unwrap();
        let push = |tag: &str| {
            let state = super::super::gitrepository::snapshot(&work).unwrap();
            run(
                &work,
                "origin",
                NetworkAction::PushTag,
                &state.token,
                None,
                Some(tag),
                &AtomicBool::new(false),
                |_| {},
            )
        };
        assert!(push("missing").is_err());
        push("release").unwrap();
        let id = git(&bare, &["rev-parse", "refs/tags/release"]).unwrap();
        assert_eq!(id, git(&work, &["rev-parse", "refs/tags/release"]).unwrap());
        assert_eq!(
            git(&bare, &["cat-file", "-t", "refs/tags/release"])
                .unwrap()
                .trim(),
            "tag"
        );
        assert!(git(&bare, &["show-ref", "--verify", "refs/tags/unselected"]).is_err());
        assert!(git(&bare, &["show-ref", "--verify", "refs/heads/main"]).is_err());
        git(&work, &["tag", "-fa", "release", "-m", "new annotation"]).unwrap();
        assert!(push("release").is_err());
        assert_eq!(git(&bare, &["rev-parse", "refs/tags/release"]).unwrap(), id);
    }

    #[test]
    fn deletes_only_the_named_remote_tag() {
        let work = tempfile::tempdir().unwrap();
        let bare = tempfile::tempdir().unwrap();
        let p = work.path();
        let remote = bare.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        git(remote, &["init", "-q", "--bare"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.com"]).unwrap();
        git(p, &["config", "core.autocrlf", "false"]).unwrap();
        git(p, &["commit", "--allow-empty", "-qm", "initial"]).unwrap();
        git(p, &["remote", "add", "origin", remote.to_str().unwrap()]).unwrap();
        git(p, &["tag", "release"]).unwrap();
        git(p, &["tag", "other"]).unwrap();
        git(
            p,
            &[
                "push",
                "origin",
                "HEAD:refs/heads/release",
                "refs/tags/release",
                "refs/tags/other",
            ],
        )
        .unwrap();
        let delete = |name| {
            let state = super::super::gitrepository::snapshot(p).unwrap();
            run(
                p,
                "origin",
                NetworkAction::DeleteTag,
                &state.token,
                None,
                Some(name),
                &AtomicBool::new(false),
                |_| {},
            )
        };
        assert!(delete("missing").is_err());
        std::fs::write(p.join("untracked"), "local").unwrap();
        delete("release").unwrap();
        assert!(git(remote, &["show-ref", "--verify", "refs/tags/release"]).is_err());
        git(remote, &["show-ref", "--verify", "refs/tags/other"]).unwrap();
        git(remote, &["show-ref", "--verify", "refs/heads/release"]).unwrap();
        git(p, &["show-ref", "--verify", "refs/tags/release"]).unwrap();
    }

    #[test]
    fn deletes_a_remote_branch() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q", "-b", "main"]).unwrap();
        git(p, &["config", "user.name", "Test"]).unwrap();
        git(p, &["config", "user.email", "test@example.com"]).unwrap();
        git(p, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(p.join("file"), "first").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-qm", "first"]).unwrap();
        let bare = tempfile::tempdir().unwrap();
        git(bare.path(), &["init", "--bare", "-q"]).unwrap();
        git(
            p,
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        )
        .unwrap();
        git(p, &["push", "-q", "origin", "main:main", "main:topic/one"]).unwrap();
        git(p, &["fetch", "-q", "origin"]).unwrap();
        let cancel = AtomicBool::new(false);
        let s = super::super::gitrepository::snapshot(p).unwrap();
        for bad in [None, Some("HEAD"), Some("--all"), Some("missing")] {
            assert!(run(
                p,
                "origin",
                NetworkAction::DeleteBranch,
                &s.token,
                None,
                bad,
                &cancel,
                |_| {}
            )
            .is_err());
        }
        run(
            p,
            "origin",
            NetworkAction::DeleteBranch,
            &s.token,
            None,
            Some("topic/one"),
            &cancel,
            |_| {},
        )
        .unwrap();
        let remote = git(bare.path(), &["--git-dir=.", "branch", "--list"]).unwrap();
        assert!(remote.contains("main") && !remote.contains("topic/one"));
        let s = super::super::gitrepository::snapshot(p).unwrap();
        assert!(s
            .refs
            .iter()
            .all(|r| r.full_name != "refs/remotes/origin/topic/one"));
    }
}
