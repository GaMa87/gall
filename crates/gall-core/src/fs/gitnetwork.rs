//! Cancellable remote operations using the user's installed Git and credentials.
use super::gitgraph::{git, has_content_filters, root};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkAction {
    Fetch,
    Pull,
    Push,
    ForcePush,
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

/// `lease` is the reviewed remote branch SHA, never an implicit moving tracking ref.
pub fn run(
    path: &Path,
    remote: &str,
    action: NetworkAction,
    expected: &str,
    lease: Option<&str>,
    cancel: &AtomicBool,
    progress: impl Fn(String),
) -> Result<String> {
    let root = root(path)?;
    let state = super::gitrepository::snapshot(&root)?;
    if state.token != expected {
        return Err(failure(
            "Repository changed. Refresh and review the remote operation again.",
        ));
    }
    if !state.remotes.iter().any(|r| r.name == remote) || remote.starts_with('-') {
        return Err(failure("Select an existing remote."));
    }
    if state.operation.is_some() {
        return Err(failure("Finish or abort the current Git operation first."));
    }
    let branch = state.branch.as_deref();
    let mut pull_target = None;
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
        NetworkAction::Pull => {
            if root.join(".gitmodules").exists() || has_content_filters(&root) {
                return Err(failure("Pulling repositories with submodules or content filters requires the terminal."));
            }
            if !git(&root, &["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
                return Err(failure("Commit or stash local changes before pulling."));
            }
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
                "--set-upstream".into(),
            ];
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
    let mut command = Command::new("git");
    command
        .args([
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
            "-C",
        ])
        .arg(&root)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", if cfg!(windows) { "" } else { "/bin/false" });
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
    loop {
        for _ in 0..128 {
            let Ok(chunk) = rx.try_recv() else {
                break;
            };
            last_activity = Instant::now();
            progress(chunk.clone());
            output.push_str(&chunk);
            if output.len() > 128 * 1024 {
                let cut = output
                    .char_indices()
                    .find(|(i, _)| *i >= output.len() - 64 * 1024)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                output.drain(..cut);
            }
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
                        || !git(&root, &["status", "--porcelain", "--untracked-files=all"])?
                            .is_empty()
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
                    output.push_str(&git(
                        &root,
                        &[
                            "-c",
                            "merge.autoStash=false",
                            "merge",
                            "--ff-only",
                            "--no-overwrite-ignore",
                            "--no-autostash",
                            id.trim(),
                        ],
                    )?);
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
        std::thread::sleep(Duration::from_millis(40));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            &cancel,
            |_| {}
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(p.join("file")).unwrap(), "first");
    }
}
