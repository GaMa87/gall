//! A caller that waits for the user: `gall diff --wait` (Git difftool, SVN and Mercurial diff commands) and
//! `gall merge` (Git mergetool and the like) hand the work to a window of a running Gall and must not exit until the
//! user is done, because the tool that called them deletes its temporary files, or reads the merged file, the moment
//! they return.
//!
//! The window reports through a small result file that the waiting process polls, as the picker window does; the
//! exit code of the caller tells three outcomes apart: accepted (0), cancelled (1) and error (2). Nothing here opens a
//! window. A merge writes its output file only when accepted, through [`save_output`], which replaces the file
//! atomically and keeps its permissions, so a cancelled or failed merge leaves the output exactly as it was.
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Exit codes of a waiting caller.
pub const EXIT_ACCEPTED: i32 = 0;
pub const EXIT_CANCELLED: i32 = 1;
pub const EXIT_ERROR: i32 = 2;

/// How a waiting request ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Outcome {
    /// The user finished: for a merge, the output file was written.
    Accepted,
    /// The user closed the window or declined: nothing was written.
    Cancelled,
    /// The window could not do it (a file could not be read, the window could not open, the output could not be
    /// written). Nothing was written.
    Error { message: String },
}

impl Outcome {
    pub fn exit_code(&self) -> i32 {
        match self {
            Outcome::Accepted => EXIT_ACCEPTED,
            Outcome::Cancelled => EXIT_CANCELLED,
            Outcome::Error { .. } => EXIT_ERROR,
        }
    }
}

/// A result path in `dir` that no other request uses: process ID, a counter and the clock.
pub fn new_result_path(dir: &Path) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    dir.join(format!(
        "gall-wait-{}-{seq}-{nanos}.json",
        std::process::id()
    ))
}

/// The file a caller creates to withdraw its request (it was interrupted): the window then closes as cancelled.
pub fn cancel_marker(result: &Path) -> PathBuf {
    let mut name = result.as_os_str().to_owned();
    name.push(".cancel");
    PathBuf::from(name)
}

/// A new file that only the user can read (mode 0600 on Unix), which must not exist yet: a planted file or link is
/// refused instead of followed.
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// A temporary name next to `path`, unique per call.
fn sibling_temp(path: &Path) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".gall-{}-{seq}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Reports an outcome: written to a private temporary file and renamed, so the waiting process never sees half of it.
pub fn write_outcome(path: &Path, outcome: &Outcome) -> std::io::Result<()> {
    let tmp = sibling_temp(path);
    let result = (|| {
        let mut file = create_private(&tmp)?;
        file.write_all(&serde_json::to_vec(outcome).map_err(std::io::Error::other)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The outcome in the result file; `None` while there is none yet. A file that cannot be understood is an error
/// outcome, not a missing one: the caller must not wait for ever.
pub fn read_outcome(path: &Path) -> Option<Outcome> {
    match std::fs::read(path) {
        Ok(bytes) => Some(
            serde_json::from_slice(&bytes).unwrap_or_else(|e| Outcome::Error {
                message: format!("The result of the window could not be read: {e}"),
            }),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(Outcome::Error {
            message: format!("The result of the window could not be read: {e}"),
        }),
    }
}

/// Waits for the result file. `interrupted` is asked at every poll: when it says so (the caller got a signal) the
/// request is withdrawn with a cancel marker and the outcome is `Cancelled`. After `timeout` (if any) it is an error.
/// The result file is removed once it has been read.
pub fn wait(
    path: &Path,
    poll: Duration,
    timeout: Option<Duration>,
    mut interrupted: impl FnMut() -> bool,
) -> Outcome {
    let started = Instant::now();
    loop {
        if let Some(outcome) = read_outcome(path) {
            let _ = std::fs::remove_file(path);
            return outcome;
        }
        if interrupted() {
            let _ = std::fs::write(cancel_marker(path), b"");
            return Outcome::Cancelled;
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            return Outcome::Error {
                message: "Gall did not answer in time".into(),
            };
        }
        std::thread::sleep(poll);
    }
}

/// Writes the output of a merge. The bytes go to a temporary file next to it and replace it by a rename, so the output
/// is either all old or all new; the permissions of the old file are kept. Called only for an accepted merge: a
/// cancelled or failed one never reaches it, which is what leaves the output intact.
pub fn save_output(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = sibling_temp(path);
    let result = (|| {
        let mut file = create_private(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Ok(existing) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, existing.permissions())?;
        }
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    const NEVER: &dyn Fn() -> bool = &|| false;
    const FAST: Duration = Duration::from_millis(5);

    #[test]
    fn the_three_outcomes_have_the_three_exit_codes() {
        assert_eq!(Outcome::Accepted.exit_code(), 0);
        assert_eq!(Outcome::Cancelled.exit_code(), 1);
        assert_eq!(
            Outcome::Error {
                message: "x".into()
            }
            .exit_code(),
            2
        );
        assert_eq!((EXIT_ACCEPTED, EXIT_CANCELLED, EXIT_ERROR), (0, 1, 2));
    }

    #[test]
    fn an_outcome_round_trips_through_the_result_file_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        for outcome in [
            Outcome::Accepted,
            Outcome::Cancelled,
            Outcome::Error {
                message: "no \u{10d}as \"quoted\"".into(),
            },
        ] {
            let path = new_result_path(dir.path());
            assert_eq!(read_outcome(&path), None);
            write_outcome(&path, &outcome).unwrap();
            assert_eq!(read_outcome(&path), Some(outcome));
        }
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.iter().all(|name| !name.ends_with(".tmp")), "{left:?}");
        // The file says the status by name, for the other programs that may read it.
        let path = new_result_path(dir.path());
        write_outcome(&path, &Outcome::Cancelled).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"status":"cancelled"}"#
        );
    }

    #[test]
    fn result_paths_are_unique_and_the_cancel_marker_sits_beside_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let a = new_result_path(dir.path());
        let b = new_result_path(dir.path());
        assert_ne!(a, b);
        assert_eq!(
            cancel_marker(Path::new("/run/u/r.json")),
            PathBuf::from("/run/u/r.json.cancel")
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_result_file_is_private_and_an_existing_link_is_not_followed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = new_result_path(dir.path());
        write_outcome(&path, &Outcome::Accepted).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // A planted link under the temporary name is refused (create_new), nothing is written through it.
        let target = dir.path().join("victim.txt");
        std::fs::write(&target, "keep").unwrap();
        let tmp_like = sibling_temp(&path);
        std::os::unix::fs::symlink(&target, &tmp_like).unwrap();
        assert!(create_private(&tmp_like).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    }

    #[test]
    fn waiting_returns_what_the_window_writes_and_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = new_result_path(dir.path());
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(40));
                write_outcome(&path, &Outcome::Accepted).unwrap();
            })
        };
        let outcome = wait(&path, FAST, Some(Duration::from_secs(10)), NEVER);
        writer.join().unwrap();
        assert_eq!(outcome, Outcome::Accepted);
        assert!(!path.exists(), "the result is consumed");
    }

    #[test]
    fn waiting_ends_with_an_error_after_the_timeout_and_for_a_result_it_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = new_result_path(dir.path());
        let outcome = wait(&path, FAST, Some(Duration::from_millis(30)), NEVER);
        assert!(matches!(outcome, Outcome::Error { .. }));
        assert_eq!(outcome.exit_code(), EXIT_ERROR);
        // A garbage result is an error at once, not a reason to wait on.
        std::fs::write(&path, b"{ not json").unwrap();
        let outcome = wait(&path, FAST, None, NEVER);
        assert!(
            matches!(&outcome, Outcome::Error { message } if message.contains("could not be read")),
            "{outcome:?}"
        );
        assert!(!path.exists());
    }

    #[test]
    fn an_interrupted_caller_withdraws_the_request_with_a_cancel_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = new_result_path(dir.path());
        let flag = AtomicBool::new(false);
        let interrupted: &dyn Fn() -> bool = &|| flag.load(Ordering::Relaxed);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(30));
                flag.store(true, Ordering::Relaxed);
            });
            let outcome = wait(&path, FAST, None, interrupted);
            assert_eq!(outcome, Outcome::Cancelled);
        });
        assert!(cancel_marker(&path).exists());
    }

    #[test]
    fn the_output_of_an_accepted_merge_replaces_the_file_atomically_and_keeps_its_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("merged file.txt");
        std::fs::write(&output, "old\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        save_output(&output, b"new\r\nbytes\xff").unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"new\r\nbytes\xff");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        // A new output file is made, and no temporary file stays behind.
        let fresh = dir.path().join("-leading dash.txt");
        save_output(&fresh, b"x").unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"x");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
    }

    #[test]
    fn a_failed_save_leaves_the_output_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out.txt");
        std::fs::write(&output, "keep me").unwrap();
        // The folder of the output does not exist for a second file: that save fails and touches nothing else.
        let missing = dir.path().join("nofolder").join("out.txt");
        assert!(save_output(&missing, b"x").is_err());
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "keep me");
        // The caller that is cancelled never calls `save_output`: the output is what it was.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
