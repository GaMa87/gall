//! Commands the user defines for the Git workspace and the command that runs after a worktree was created (WN-47).
//!
//! A command is an argument list, never a shell string: the first entry is the program, the others its arguments, and
//! `{hash}`-style variables are replaced inside one argument at a time. A value therefore cannot add arguments, run a
//! second command or be interpreted by a shell, whatever a branch, file or tag is called. The program starts in the
//! repository (or the new worktree) with no input; its output (at most 1 MiB) comes back for the log pane. A run ends
//! after ten minutes or when it is cancelled, and the whole process tree is stopped.
use crate::{CoreError, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const MAX_OUTPUT: usize = 1024 * 1024;
pub const TIMEOUT: Duration = Duration::from_secs(600);
pub const MAX_ARGS: usize = 64;
pub const MAX_ARG: usize = 8192;

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// How a run ended.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// None when the program did not end by itself (cancelled or out of time).
    pub code: Option<i32>,
    /// Standard output, then standard error.
    pub output: String,
    pub timed_out: bool,
    pub cancelled: bool,
}

/// Refuses what cannot be a command: nothing, a program that is empty, text with NUL, too many or too long arguments.
pub fn check(argv: &[String]) -> Result<()> {
    let Some(program) = argv.first() else {
        return Err(fail("The command is empty"));
    };
    if program.trim().is_empty() {
        return Err(fail("The command has no program"));
    }
    if argv.len() > MAX_ARGS {
        return Err(fail("The command has too many arguments"));
    }
    if argv
        .iter()
        .any(|part| part.contains('\0') || part.len() > MAX_ARG)
    {
        return Err(fail("The command has an invalid argument"));
    }
    Ok(())
}

/// Replaces `{name}` in every argument by the variable's value. `{{` and `}}` stand for the braces themselves. An unknown
/// name is an error (a typo must not run with the text `{brnch}`), and the program (the first entry) takes none.
pub fn expand(argv: &[String], variables: &BTreeMap<String, String>) -> Result<Vec<String>> {
    check(argv)?;
    argv.iter()
        .enumerate()
        .map(|(index, part)| {
            let mut out = String::new();
            let mut chars = part.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '{' if chars.peek() == Some(&'{') => {
                        chars.next();
                        out.push('{');
                    }
                    '}' if chars.peek() == Some(&'}') => {
                        chars.next();
                        out.push('}');
                    }
                    '{' => {
                        let mut name = String::new();
                        loop {
                            match chars.next() {
                                Some('}') => break,
                                Some(next) => name.push(next),
                                None => {
                                    return Err(fail(format!(
                                        "A variable is not closed in “{part}”"
                                    )))
                                }
                            }
                        }
                        if index == 0 {
                            return Err(fail("The program of a command cannot use a variable"));
                        }
                        let value = variables
                            .get(&name)
                            .ok_or_else(|| fail(format!("Unknown variable {{{name}}}")))?;
                        out.push_str(value);
                    }
                    '}' => return Err(fail(format!("A stray }} in “{part}”"))),
                    other => out.push(other),
                }
            }
            Ok(out)
        })
        .collect()
}

static RUNNING: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
fn running<T>(f: impl FnOnce(&mut HashMap<String, Arc<AtomicBool>>) -> T) -> T {
    let mut map = RUNNING
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut map)
}

/// Runs `argv` in the repository of `path` and registers it under `id`, so [`cancel`] can stop it.
pub fn run(path: &Path, id: &str, argv: &[String]) -> Result<Outcome> {
    let root = super::gitgraph::root(path)?;
    let flag = Arc::new(AtomicBool::new(false));
    running(|map| map.insert(id.to_owned(), flag.clone()));
    let outcome = run_in(&root, argv, TIMEOUT, &|| flag.load(Ordering::Relaxed));
    running(|map| map.remove(id));
    outcome
}

/// Stops the run registered under `id`; true when there was one.
pub fn cancel(id: &str) -> bool {
    running(|map| {
        map.get(id)
            .map(|flag| flag.store(true, Ordering::Relaxed))
            .is_some()
    })
}

/// Runs `argv` with `dir` as its folder until it ends, `timeout` passes or `cancelled` says so.
pub fn run_in(
    dir: &Path,
    argv: &[String],
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Outcome> {
    check(argv)?;
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| fail(format!("{} could not start: {e}", argv[0])))?;
    let tracker = super::gitprogress::active();
    let drain =
        |stream: Option<Box<dyn Read + Send>>,
         progress: Option<Arc<Mutex<super::gitprogress::ProgressTracker>>>| {
            std::thread::spawn(move || {
                let mut kept = Vec::new();
                if let Some(mut stream) = stream {
                    let mut buffer = [0u8; 8192];
                    while let Ok(read) = stream.read(&mut buffer) {
                        if read == 0 {
                            break;
                        }
                        if let Some(ref tracker) = progress {
                            let text = String::from_utf8_lossy(&buffer[..read]);
                            if let Ok(mut guard) = tracker.lock() {
                                guard.feed_chunk(&text);
                            }
                        }
                        // Keep reading past the limit so the program never blocks on a full pipe.
                        let room = MAX_OUTPUT.saturating_sub(kept.len());
                        kept.extend_from_slice(&buffer[..read.min(room)]);
                    }
                }
                kept
            })
        };
    let out = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        None,
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        tracker,
    );
    let started = Instant::now();
    let (mut timed_out, mut stopped) = (false, false);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => return Err(fail(e.to_string())),
        }
        if cancelled() {
            stopped = true;
        } else if started.elapsed() >= timeout {
            timed_out = true;
        }
        if stopped || timed_out {
            end_tree(&mut child);
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let mut output = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let errors = err.join().unwrap_or_default();
    if !errors.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&String::from_utf8_lossy(&errors));
    }
    Ok(Outcome {
        code: status.and_then(|s| s.code()),
        output,
        timed_out,
        cancelled: stopped,
    })
}

/// Stops the program and what it started: the tree on Windows (`taskkill /T`), the process itself elsewhere.
fn end_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .creation_flags(0x0800_0000)
            .output();
    }
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }
    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).into()).collect()
    }

    #[test]
    fn a_variable_is_replaced_inside_one_argument_and_can_never_add_arguments() {
        let hostile = "feature/x; rm -rf / && $(evil) \"quoted\" 'single' `tick` --upload-pack=bad";
        let out = expand(
            &argv(&["git", "log", "--grep={branch}", "{hash}"]),
            &vars(&[("branch", hostile), ("hash", "abc123")]),
        )
        .unwrap();
        assert_eq!(
            out,
            argv(&["git", "log", &format!("--grep={hostile}"), "abc123"])
        );
        // Still four entries: nothing split, nothing joined.
        assert_eq!(out.len(), 4);
        // A value with a new line or spaces stays one argument.
        let out = expand(&argv(&["echo", "{file}"]), &vars(&[("file", "a b\nc.txt")])).unwrap();
        assert_eq!(out, argv(&["echo", "a b\nc.txt"]));
    }

    #[test]
    fn braces_can_be_written_and_mistakes_are_refused() {
        assert_eq!(
            expand(&argv(&["tool", "{{literal}}"]), &vars(&[])).unwrap(),
            argv(&["tool", "{literal}"])
        );
        assert!(expand(&argv(&["tool", "{brnch}"]), &vars(&[("branch", "x")])).is_err());
        assert!(expand(&argv(&["tool", "{open"]), &vars(&[])).is_err());
        assert!(expand(&argv(&["tool", "stray}"]), &vars(&[])).is_err());
        assert!(expand(&argv(&["{hash}"]), &vars(&[("hash", "x")])).is_err());
    }

    #[test]
    fn what_cannot_be_a_command_is_refused() {
        assert!(check(&[]).is_err());
        assert!(check(&argv(&[""])).is_err());
        assert!(check(&argv(&["  "])).is_err());
        assert!(check(&argv(&["tool", "a\0b"])).is_err());
        assert!(check(&vec!["x".to_owned(); MAX_ARGS + 1]).is_err());
        assert!(check(&["x".to_owned(), "y".repeat(MAX_ARG + 1)]).is_err());
        assert!(check(&argv(&["git", "status"])).is_ok());
    }

    /// A program that exists on every host the tests run on: Git itself.
    fn git_version() -> Vec<String> {
        argv(&["git", "--version"])
    }

    #[test]
    fn a_command_runs_in_the_folder_without_a_shell_and_its_output_comes_back() {
        let temp = tempfile::tempdir().unwrap();
        let out = run_in(
            temp.path(),
            &git_version(),
            Duration::from_secs(30),
            &|| false,
        )
        .unwrap();
        assert_eq!(out.code, Some(0));
        assert!(out.output.starts_with("git version"), "{}", out.output);
        assert!(!out.timed_out && !out.cancelled);
        // A failing run reports its code and the error text.
        let failed = run_in(
            temp.path(),
            &argv(&["git", "no-such-subcommand-xyz"]),
            Duration::from_secs(30),
            &|| false,
        )
        .unwrap();
        assert!(failed.code.is_some_and(|code| code != 0));
        assert!(!failed.output.is_empty());
        // A program that does not exist is an error, not a panic.
        assert!(run_in(
            temp.path(),
            &argv(&["no-such-program-xyz"]),
            Duration::from_secs(5),
            &|| false
        )
        .is_err());
        // Shell syntax is just text for the program: `;` and `&&` are arguments of git here.
        let text = run_in(
            temp.path(),
            &argv(&[
                "git",
                "rev-parse",
                "--verify",
                "x; echo pwned && echo pwned",
            ]),
            Duration::from_secs(30),
            &|| false,
        )
        .unwrap();
        assert!(text.code.is_some_and(|code| code != 0) && !text.output.starts_with("pwned"));
    }

    #[test]
    fn a_run_is_cancelled_and_a_run_out_of_time_is_stopped() {
        let temp = tempfile::tempdir().unwrap();
        // `git daemon`-free long runner: git waits for input that never comes only with a TTY, so use the host's sleep.
        let sleeper = if cfg!(windows) {
            argv(&["ping", "-n", "30", "127.0.0.1"])
        } else {
            argv(&["sleep", "30"])
        };
        let started = Instant::now();
        let out = run_in(temp.path(), &sleeper, Duration::from_millis(300), &|| false).unwrap();
        assert!(out.timed_out && out.code.is_none() && started.elapsed() < Duration::from_secs(10));
        let started = Instant::now();
        let out = run_in(temp.path(), &sleeper, Duration::from_secs(60), &|| true).unwrap();
        assert!(out.cancelled && started.elapsed() < Duration::from_secs(10));
        assert!(!cancel("nothing-runs-under-this-id"));
    }

    #[test]
    fn the_output_is_capped_but_always_read_to_the_end() {
        let temp = tempfile::tempdir().unwrap();
        // 3 MiB of output from git itself: the log of a generated file is not needed, `git hash-object --stdin-paths`
        // is not either; `git help -a` is long enough on every host, so repeat the test with the biggest output at hand.
        let out = run_in(
            temp.path(),
            &argv(&["git", "help", "-a"]),
            Duration::from_secs(60),
            &|| false,
        )
        .unwrap();
        assert!(out.output.len() <= 2 * MAX_OUTPUT);
        assert_eq!(out.code, Some(0));
    }
}
