//! Whether merging a branch would conflict, without doing it (`git merge-tree --write-tree`, Git
//! 2.38). No work tree, index or reference is touched; Git only writes the objects of the result,
//! which nothing refers to.
use super::gitexec;
use super::gitgraph::root;
use crate::{CoreError, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// A merge that takes longer is not forecast.
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_OUTPUT: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Forecast {
    /// False: this Git or repository cannot tell. Nothing is shown then.
    pub known: bool,
    /// Files that would conflict; empty for a clean merge.
    pub files: Vec<String>,
}

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// Runs Git and returns the exit code and what it printed. None when it was cancelled or took too
/// long.
fn run(
    root: &Path,
    args: &[&str],
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<(Option<i32>, Vec<u8>)>> {
    let mut command = gitexec::command(root, super::gitstatus::program());
    command
        .args(["--no-pager", "-c", "core.fsmonitor=false", "-C"])
        .arg(gitexec::dir_arg(root))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1");
    gitexec::forward_env(&mut command);
    let mut child = command.spawn().map_err(|e| fail(e.to_string()))?;
    let stdout = child.stdout.take().ok_or_else(|| fail("No Git output"))?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(MAX_OUTPUT).read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    loop {
        match child.try_wait().map_err(|e| fail(e.to_string()))? {
            Some(status) => {
                let bytes = reader.join().unwrap_or_default();
                return Ok(Some((status.code(), bytes)));
            }
            None if cancelled() || started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

type DriverFact = (Option<SystemTime>, bool);

/// Whether a merge driver is configured: it would run a command, which a forecast in the background
/// must not. Asked once, and again when the configuration of the repository changes.
fn has_drivers(root: &Path, cancelled: &dyn Fn() -> bool) -> Result<bool> {
    static KNOWN: Mutex<Option<HashMap<PathBuf, DriverFact>>> = Mutex::new(None);
    let stamp = super::gitadvanced::common_dir(root)
        .ok()
        .and_then(|dir| dir.join("config").metadata().ok()?.modified().ok());
    if let Ok(known) = KNOWN.lock() {
        if let Some((at, has)) = known.as_ref().and_then(|known| known.get(root)) {
            if *at == stamp {
                return Ok(*has);
            }
        }
    }
    let has = match run(
        root,
        &[
            "config",
            "--name-only",
            "--get-regexp",
            r"^merge\..*\.driver$",
        ],
        cancelled,
    )? {
        Some((Some(0), names)) => !names.is_empty(),
        // Nothing matches.
        Some((Some(1), _)) => false,
        // It could not be asked: nothing is run then.
        _ => return Ok(true),
    };
    if let Ok(mut known) = KNOWN.lock() {
        let known = known.get_or_insert_with(HashMap::new);
        if known.len() >= 256 {
            known.clear();
        }
        known.insert(root.to_path_buf(), (stamp, has));
    }
    Ok(has)
}

/// What merging `theirs` into `ours` would do. Both are commits.
pub fn forecast(
    path: &Path,
    ours: &str,
    theirs: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<Forecast> {
    for id in [ours, theirs] {
        if !matches!(id.len(), 40 | 64) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(fail("A full Git commit ID is required"));
        }
    }
    let root = root(path)?;
    if cancelled() || has_drivers(&root, cancelled)? {
        return Ok(Forecast::default());
    }
    let answer = run(
        &root,
        &[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "-z",
            ours,
            theirs,
        ],
        cancelled,
    )?;
    Ok(match answer {
        Some((Some(0), bytes)) if has_tree(&bytes) => Forecast {
            known: true,
            files: Vec::new(),
        },
        Some((Some(1), bytes)) if has_tree(&bytes) => Forecast {
            known: true,
            files: conflicted(&bytes),
        },
        // 129: a Git before 2.38. Others: unrelated histories, a missing object of a partial clone.
        _ => Forecast::default(),
    })
}

fn has_tree(bytes: &[u8]) -> bool {
    bytes
        .split(|byte| matches!(byte, b'\0' | b'\n'))
        .next()
        .is_some_and(|id| matches!(id.len(), 40 | 64) && id.iter().all(u8::is_ascii_hexdigit))
}

/// The names after the tree.
fn conflicted(bytes: &[u8]) -> Vec<String> {
    let mut names: Vec<String> = bytes
        .split(|b| *b == 0)
        .skip(1)
        .take_while(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::super::gitgraph::git;
    use super::*;

    fn repository() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "Test"],
            &["config", "user.email", "test@example.com"],
            &["config", "core.autocrlf", "false"],
        ] {
            git(d.path(), args).unwrap();
        }
        d
    }

    fn commit(p: &Path, files: &[(&str, &str)]) -> String {
        for (file, text) in files {
            std::fs::write(p.join(file), text).unwrap();
        }
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "change"]).unwrap();
        git(p, &["rev-parse", "HEAD"]).unwrap().trim().into()
    }

    #[test]
    fn conflicts_are_named_and_nothing_is_touched() {
        let d = repository();
        let p = d.path();
        commit(p, &[("a.txt", "a\n"), ("ž b.txt", "b\n"), ("c.txt", "c\n")]);
        git(p, &["switch", "-qc", "clean"]).unwrap();
        let clean = commit(p, &[("new.txt", "new\n")]);
        git(p, &["switch", "-q", "main"]).unwrap();
        git(p, &["switch", "-qc", "side"]).unwrap();
        let side = commit(p, &[("a.txt", "side\n"), ("ž b.txt", "side\n")]);
        git(p, &["switch", "-q", "main"]).unwrap();
        let main = commit(p, &[("a.txt", "main\n"), ("ž b.txt", "main\n")]);
        std::fs::write(p.join("c.txt"), "local\n").unwrap();
        let before = (
            git(p, &["status", "--porcelain"]).unwrap(),
            git(p, &["for-each-ref"]).unwrap(),
        );
        let ask = |theirs: &str| forecast(p, &main, theirs, &|| false).unwrap();
        // A Git before 2.38 knows nothing, which is an answer too.
        let found = ask(&side);
        if found.known {
            assert_eq!(found.files, ["a.txt", "ž b.txt"]);
            assert_eq!(ask(&clean).files, Vec::<String>::new());
            assert!(ask(&clean).known);
        }
        assert_eq!(
            before,
            (
                git(p, &["status", "--porcelain"]).unwrap(),
                git(p, &["for-each-ref"]).unwrap()
            )
        );
        assert!(!p.join(".git/MERGE_HEAD").exists());
        // Cancelled and wrong questions.
        assert!(!forecast(p, &main, &side, &|| true).unwrap().known);
        assert!(forecast(p, "main", &side, &|| false).is_err());
        assert!(!ask(&"1".repeat(40)).known);
    }

    #[test]
    fn a_merge_driver_is_never_run() {
        let d = repository();
        let p = d.path();
        commit(
            p,
            &[("a.txt", "a\n"), (".gitattributes", "*.txt merge=mine\n")],
        );
        git(p, &["switch", "-qc", "side"]).unwrap();
        let side = commit(p, &[("a.txt", "side\n")]);
        git(p, &["switch", "-q", "main"]).unwrap();
        let main = commit(p, &[("a.txt", "main\n")]);
        let marker = p.join("ran");
        git(
            p,
            &[
                "config",
                "merge.mine.driver",
                &format!("echo x > '{}'", marker.to_string_lossy().replace('\\', "/")),
            ],
        )
        .unwrap();
        assert!(!forecast(p, &main, &side, &|| false).unwrap().known);
        assert!(!marker.exists());
    }

    #[test]
    fn names_end_at_the_empty_field() {
        assert_eq!(
            conflicted(b"tree\0a\0a\0b c\0\0informational"),
            ["a", "b c"]
        );
        assert_eq!(conflicted(b"tree\0"), Vec::<String>::new());
    }
}
