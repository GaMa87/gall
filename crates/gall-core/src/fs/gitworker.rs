//! Long-lived shells inside a WSL distribution that run Git commands for `gitexec`.
//!
//! Starting `wsl.exe` costs about 70 ms every time, and a view asks Git a few dozen
//! questions (the history graph about twenty, the submodule list as many). A shell that is
//! already running answers in a few milliseconds. Each worker serves one request at a time;
//! concurrent requests use several workers. Idle workers end after [`IDLE`] (closing
//! their input ends the shell), so a distribution is not kept awake.
//!
//! A request is one line: the command as a shell line (`env 'K=V' git 'arg' …`, every word
//! single-quoted, so no argument is ever interpreted), base64-encoded to keep newlines and
//! binary-safe bytes out of the framing. The answer is `<exit code> <stdout bytes> <stderr
//! bytes>` and the two streams.

use super::wsl;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a worker may sit unused.
const IDLE: Duration = Duration::from_secs(30);
/// Idle workers kept per distribution.
const MAX_IDLE: usize = 16;

/// The shell loop (POSIX `sh`). The request is decoded and run in a subshell with its
/// output in files, so the sizes are known before the bytes are sent.
const SCRIPT: &str = r#"d=/tmp/.gall-$$; mkdir -p "$d"; while IFS= read -r req; do line=$(printf %s "$req" | base64 -d); (eval "$line") >"$d/o" 2>"$d/e" </dev/null; rc=$?; printf '%s %s %s\n' "$rc" "$(wc -c <"$d/o")" "$(wc -c <"$d/e")"; cat "$d/o" "$d/e"; done; rm -rf "$d""#;

type Reply = std::io::Result<(i32, Vec<u8>, Vec<u8>)>;

struct Worker {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<Reply>,
    used: Instant,
}

impl Worker {
    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static POOL: OnceLock<Mutex<HashMap<String, Vec<Worker>>>> = OnceLock::new();

fn pool() -> &'static Mutex<HashMap<String, Vec<Worker>>> {
    static REAPER: OnceLock<()> = OnceLock::new();
    REAPER.get_or_init(|| {
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_secs(10));
            if let Some(pool) = POOL.get() {
                if let Ok(mut pool) = pool.lock() {
                    for idle in pool.values_mut() {
                        // Dropping a worker closes its input: the shell ends.
                        idle.retain(|w| w.used.elapsed() < IDLE);
                    }
                }
            }
        });
    });
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn spawn(distro: &str) -> std::io::Result<Worker> {
    let mut cmd = Command::new(wsl::wsl_exe());
    cmd.args(["-d", distro, "--exec", "sh", "-c", SCRIPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("no stdout"))?;
    let (tx, replies) = channel();
    std::thread::spawn(move || {
        let mut out = BufReader::new(stdout);
        loop {
            let mut head = String::new();
            if !matches!(out.read_line(&mut head), Ok(n) if n > 0) {
                break; // the shell ended
            }
            let mut n = head.split_whitespace().map(|v| v.parse::<usize>());
            let (Some(Ok(code)), Some(Ok(o)), Some(Ok(e))) = (n.next(), n.next(), n.next()) else {
                let _ = tx.send(Err(std::io::Error::other(
                    "unexpected output of the worker",
                )));
                break;
            };
            let mut stdout = vec![0; o];
            let mut stderr = vec![0; e];
            if out.read_exact(&mut stdout).is_err() || out.read_exact(&mut stderr).is_err() {
                break;
            }
            if tx.send(Ok((code as i32, stdout, stderr))).is_err() {
                break;
            }
        }
    });
    Ok(Worker {
        child,
        stdin,
        replies,
        used: Instant::now(),
    })
}

fn take(distro: &str) -> std::io::Result<Worker> {
    let idle = pool()
        .lock()
        .ok()
        .and_then(|mut p| p.get_mut(&distro.to_lowercase()).and_then(Vec::pop));
    match idle {
        Some(w) => Ok(w),
        None => spawn(distro),
    }
}

fn give_back(distro: &str, mut w: Worker) {
    w.used = Instant::now();
    if let Ok(mut p) = pool().lock() {
        let idle = p.entry(distro.to_lowercase()).or_default();
        if idle.len() < MAX_IDLE {
            idle.push(w);
            return;
        }
    }
    drop(w); // closes its input
}

/// `'text'` for a POSIX shell: nothing in it is interpreted.
pub(super) fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Standard base64 (the shell decodes it with `base64 -d`).
pub(super) fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn exit_status(code: i32) -> ExitStatus {
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(code as u32)
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }
}

/// Runs `shell_line` (see the module docs) in `distro` on a pooled worker. None when no
/// worker could be used (the caller starts the command the ordinary way); a timeout is
/// an error, and the worker, which may still run Git, is ended.
pub(super) fn run(
    distro: &str,
    shell_line: &str,
    timeout: Duration,
) -> Option<std::io::Result<Output>> {
    let mut worker = take(distro).ok()?;
    let request = format!("{}\n", base64(shell_line.as_bytes()));
    if worker.stdin.write_all(request.as_bytes()).is_err() || worker.stdin.flush().is_err() {
        worker.kill();
        return None;
    }
    match worker.replies.recv_timeout(timeout) {
        Ok(Ok((code, stdout, stderr))) => {
            give_back(distro, worker);
            Some(Ok(Output {
                status: exit_status(code),
                stdout,
                stderr,
            }))
        }
        Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => {
            worker.kill();
            None
        }
        Err(RecvTimeoutError::Timeout) => {
            worker.kill();
            Some(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Git took too long inside WSL",
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_examples() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"a\nb"), "YQpi");
    }

    #[test]
    fn quoting_keeps_every_character_literal() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("a b;$(x)`y`\n*"), "'a b;$(x)`y`\n*'");
        assert_eq!(quote(""), "''");
    }

    /// The worker script and the framing, run with a local POSIX shell where there is one.
    #[cfg(unix)]
    #[test]
    fn the_script_answers_requests_in_order_with_exit_codes() {
        use std::io::Write;
        let mut child = Command::new("sh")
            .args(["-c", SCRIPT])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for line in [
            "printf 'out\\n'; printf 'err' >&2; exit 3",
            "env 'GALL_T=a b' sh -c 'printf %s \"$GALL_T\"'",
        ] {
            writeln!(stdin, "{}", base64(line.as_bytes())).unwrap();
        }
        drop(stdin);
        let mut output = BufReader::new(child.stdout.take().unwrap());
        for (code, stdout, stderr) in [(3, "out\n", "err"), (0, "a b", "")] {
            let mut header = String::new();
            output.read_line(&mut header).unwrap();
            // BSD wc pads its counts; the protocol uses numeric fields, then exact bytes.
            let fields: Vec<usize> = header
                .split_whitespace()
                .map(|field| field.parse().unwrap())
                .collect();
            assert_eq!(fields, [code, stdout.len(), stderr.len()]);
            let mut body = vec![0; stdout.len() + stderr.len()];
            output.read_exact(&mut body).unwrap();
            assert_eq!(body, format!("{stdout}{stderr}").as_bytes());
        }
        let mut trailing = Vec::new();
        output.read_to_end(&mut trailing).unwrap();
        assert!(trailing.is_empty());
        assert!(child.wait().unwrap().success());
    }
}
