//! Parent process and private result directory for CLI comparisons that wait for a window.
use crate::cli;
use gall_core::handoff::{self, Outcome};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessStatus, ProcessesToUpdate, System};

const POLL: Duration = Duration::from_millis(100);
const STARTUP_LIMIT: Duration = Duration::from_secs(30);
const DIRECTORY_PREFIX: &str = "gall-compare-wait-";
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

fn companion(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

pub(crate) fn is_result_path(path: &Path) -> bool {
    let result_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            name.strip_prefix("gall-wait-")
                .and_then(|name| name.strip_suffix(".json"))
                .is_some_and(|id| {
                    let mut fields = id.split('-');
                    fields.clone().count() == 3
                        && fields.all(|field| {
                            !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit())
                        })
                })
        });
    path.is_absolute()
        && result_name
        && path.parent().is_some_and(|dir| {
            dir.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with(DIRECTORY_PREFIX))
                && fs::symlink_metadata(dir).is_ok_and(|meta| {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if meta.permissions().mode() & 0o077 != 0 {
                            return false;
                        }
                    }
                    meta.is_dir()
                })
        })
}

pub(crate) fn result_dir() -> io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(DIRECTORY_PREFIX);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    builder.tempdir()
}

/// The parent and the window take this ticket before withdrawing or completing the request.
/// Once completion starts, an interrupt waits for its outcome instead of reporting a cancelled save.
pub(crate) fn reserve_result(path: &Path) -> io::Result<bool> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(companion(path, ".decision"))
    {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err),
    }
}

pub(crate) fn mark_started(path: &Path) -> io::Result<()> {
    fs::write(companion(path, ".started"), std::process::id().to_string())
}

/// A caller that withdrew has left its private directory for the window to remove.
pub(crate) fn clean_cancelled(path: &Path) {
    let marker = handoff::cancel_marker(path);
    if !marker.exists() {
        return;
    }
    for file in [
        path.to_path_buf(),
        companion(path, ".started"),
        companion(path, ".decision"),
        marker,
    ] {
        let _ = fs::remove_file(file);
    }
    if let Some(dir) = path.parent().filter(|dir| {
        dir.file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with(DIRECTORY_PREFIX))
    }) {
        let _ = fs::remove_dir(dir);
    }
}

fn child_operands(mut args: cli::CliArgs) -> Option<Vec<OsString>> {
    if let Some(merge) = args.merge.take() {
        // Git supplies the name of an absent base for an add/add conflict.
        let base = merge
            .base
            .filter(|path| !fs::metadata(path).is_err_and(|err| err.kind() == ErrorKind::NotFound));
        return Some(
            [
                "--merge".into(),
                "--".into(),
                base.unwrap_or_default(),
                merge.local,
                merge.remote,
                merge.output,
            ]
            .map(OsString::from)
            .into(),
        );
    }
    let (left, right) = args.compare.filter(|_| args.wait)?;
    Some(
        ["--compare".to_string(), "--".to_string(), left, right]
            .map(OsString::from)
            .into(),
    )
}

fn error(message: impl Into<String>) -> Outcome {
    Outcome::Error {
        message: message.into(),
    }
}

fn run(operands: &[OsString]) -> io::Result<Outcome> {
    let result_dir = result_dir()?;
    let path = handoff::new_result_path(result_dir.path());
    #[cfg(windows)]
    let watch_interrupt = {
        // A VCS or terminal can supply a console to the bundled Windows GUI binary.
        use windows::Win32::System::Console::{AttachConsole, GetConsoleCP, ATTACH_PARENT_PROCESS};
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
        unsafe { GetConsoleCP() != 0 }
    };
    #[cfg(not(windows))]
    let watch_interrupt = true;
    if watch_interrupt {
        ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::Relaxed))
            .map_err(io::Error::other)?;
    }

    let mut request = OsString::from(cli::HANDOFF_FLAG);
    request.push(path.as_os_str());
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(request)
        .arg("--wait")
        .args(operands)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Terminal Ctrl+C must signal the waiter. The native window withdraws through the same
    // cancellation path for both a first instance and a request forwarded to a running Gall.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows::Win32::System::Threading::CREATE_NO_WINDOW;
        command.creation_flags(CREATE_NO_WINDOW.0);
    }
    let mut child = command.spawn()?;
    let started_at = Instant::now();
    let started = companion(&path, ".started");
    let mut problem = None;
    let mut withdrawn = false;
    let mut launch_failed = false;
    let mut process = System::new().ok();
    let mut receiver = None;
    let mut last_probe = started_at;
    let outcome = handoff::wait(&path, POLL, None, || {
        if problem.is_none() {
            if receiver.is_none() {
                receiver = fs::read_to_string(&started)
                    .ok()
                    .and_then(|pid| pid.parse::<u32>().ok())
                    .filter(|pid| *pid != 0)
                    .map(Pid::from_u32);
            }
            match child.try_wait() {
                Ok(Some(status)) if !status.success() => {
                    problem = Some(format!("Gall failed to start the comparison ({status})"));
                    launch_failed = true;
                }
                Err(err) => problem = Some(format!("Cannot check the comparison process: {err}")),
                _ if receiver.is_none() && started_at.elapsed() >= STARTUP_LIMIT => {
                    problem = Some("Gall did not open the comparison window in time".into());
                }
                _ => {}
            }
            if let Some((pid, process)) = receiver
                .filter(|_| last_probe.elapsed() >= Duration::from_secs(1))
                .zip(process.as_mut())
            {
                last_probe = Instant::now();
                process.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
                if process.process(pid).is_none_or(|p| {
                    matches!(p.status(), ProcessStatus::Zombie | ProcessStatus::Dead)
                }) && handoff::read_outcome(&path).is_none()
                {
                    problem = Some("Gall closed before returning the comparison result".into());
                    launch_failed = true;
                }
            }
        }
        if problem.is_none() && !INTERRUPTED.load(Ordering::Relaxed) {
            return false;
        }
        if handoff::read_outcome(&path).is_some() {
            return false;
        }
        match reserve_result(&path) {
            Ok(false) if !launch_failed => return false,
            Ok(_) => {}
            Err(err) => problem = Some(format!("Cannot reserve the comparison result: {err}")),
        }
        withdrawn = true;
        true
    });
    if withdrawn && !launch_failed {
        // The receiver also consumes a marker if the request was withdrawn before it opened.
        let _ = result_dir.keep();
    }
    Ok(if withdrawn {
        problem.map_or(outcome, error)
    } else {
        outcome
    })
}

pub(crate) fn run_caller(raw: &[OsString], text: &[String]) -> Option<i32> {
    let cwd = std::env::current_dir().ok();
    let args = cli::parse_in(text.iter().cloned(), cwd.as_deref());
    if args.handoff.is_some() {
        return None;
    }
    let operands = child_operands(args)?;
    let outcome = if raw.iter().any(|arg| arg.to_str().is_none()) {
        error("Comparison paths must be valid Unicode")
    } else {
        run(&operands).unwrap_or_else(|err| error(format!("Cannot launch Gall: {err}")))
    };
    if let Outcome::Error { message } = &outcome {
        eprintln!("gall: {message}");
    }
    Some(outcome.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comparison_result_has_one_terminal_ticket() {
        let dir = tempfile::tempdir().unwrap();
        let path = handoff::new_result_path(dir.path());
        assert!(reserve_result(&path).unwrap());
        assert!(!reserve_result(&path).unwrap());
        let another = handoff::new_result_path(dir.path());
        assert!(reserve_result(&another).unwrap());
    }

    #[test]
    fn a_merge_with_a_missing_base_keeps_the_literal_operands() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing base");
        let args = cli::CliArgs {
            merge: Some(cli::MergeArgs {
                base: Some(missing.to_string_lossy().into_owned()),
                local: "--has spaces".into(),
                remote: "remote".into(),
                output: "out".into(),
            }),
            ..Default::default()
        };
        let out = child_operands(args).unwrap();
        assert_eq!(
            out,
            ["--merge", "--", "", "--has spaces", "remote", "out"].map(OsString::from)
        );
    }
}
