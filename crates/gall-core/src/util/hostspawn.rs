//! Running programs of the host from inside a Flatpak sandbox.
//!
//! A Flatpak app sees only its own runtime: the terminal, the editor, `git`, `gio` and every program
//! "Open with" starts live on the host. Flathub accepts that for a file manager when the app starts
//! them with `flatpak-spawn --host` (the sandbox needs `--talk-name=org.freedesktop.Flatpak`, see
//! `packaging/flatpak`). Outside Flatpak nothing here changes a command.
//!
//! The wrapping is a pure function of a "sandboxed" flag, so it is unit-tested without a sandbox
//! (`wrap_argv_if`); [`in_flatpak`] reads the real flag.

use std::path::Path;
use std::sync::OnceLock;

/// The file Flatpak puts in every sandbox.
const FLATPAK_INFO: &str = "/.flatpak-info";

/// True when this process runs inside a Flatpak sandbox (Linux only).
pub fn in_flatpak() -> bool {
    static IN_FLATPAK: OnceLock<bool> = OnceLock::new();
    *IN_FLATPAK.get_or_init(|| cfg!(target_os = "linux") && Path::new(FLATPAK_INFO).exists())
}

/// `argv` as it must be started: unchanged outside the sandbox, else prefixed with
/// `flatpak-spawn --host`, the working folder as `--directory` (the new process is not a child of
/// this one, so `current_dir` would not reach it) and `env` as `--env` (the host process starts with
/// the host's environment, not ours). An empty `argv` stays empty.
pub fn wrap_argv_if(
    sandboxed: bool,
    argv: Vec<String>,
    cwd: Option<&Path>,
    env: &[(&str, &str)],
) -> Vec<String> {
    if !sandboxed || argv.is_empty() {
        return argv;
    }
    let mut out = vec!["flatpak-spawn".to_string(), "--host".to_string()];
    if let Some(dir) = cwd {
        out.push(format!("--directory={}", dir.display()));
    }
    for (key, value) in env {
        out.push(format!("--env={key}={value}"));
    }
    out.extend(argv);
    out
}

/// [`wrap_argv_if`] for the real sandbox state.
pub fn wrap_argv(argv: Vec<String>, cwd: Option<&Path>, env: &[(&str, &str)]) -> Vec<String> {
    wrap_argv_if(in_flatpak(), argv, cwd, env)
}

/// True when `program` can be started on the host. Only meaningful in the sandbox, where the
/// program is not on this process's `PATH`; it asks the host's shell.
pub fn host_has(program: &str) -> bool {
    if program.is_empty() || program.contains(['\0', '\n']) {
        return false;
    }
    std::process::Command::new("flatpak-spawn")
        .args([
            "--host",
            "sh",
            "-c",
            "command -v -- \"$1\" >/dev/null 2>&1",
            "sh",
            program,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn outside_flatpak_nothing_changes() {
        let a = argv(&["git", "-C", "/tmp/a b", "status"]);
        assert_eq!(
            wrap_argv_if(
                false,
                a.clone(),
                Some(Path::new("/tmp")),
                &[("LC_ALL", "C")]
            ),
            a
        );
        assert_eq!(wrap_argv_if(false, vec![], None, &[]), Vec::<String>::new());
    }

    #[test]
    fn inside_flatpak_the_command_runs_on_the_host() {
        assert_eq!(
            wrap_argv_if(true, argv(&["xdg-terminal-exec"]), None, &[]),
            argv(&["flatpak-spawn", "--host", "xdg-terminal-exec"])
        );
    }

    #[test]
    fn folder_and_environment_are_passed_as_options_before_the_program() {
        let wrapped = wrap_argv_if(
            true,
            argv(&["git", "status"]),
            Some(Path::new("/home/u/my repo")),
            &[("GIT_TERMINAL_PROMPT", "0"), ("LC_ALL", "C")],
        );
        assert_eq!(
            wrapped,
            argv(&[
                "flatpak-spawn",
                "--host",
                "--directory=/home/u/my repo",
                "--env=GIT_TERMINAL_PROMPT=0",
                "--env=LC_ALL=C",
                "git",
                "status"
            ])
        );
    }

    #[test]
    fn arguments_stay_separate_and_look_like_nothing_to_flatpak_spawn() {
        // Each argument is its own element: spaces, quotes, a leading dash and `--host` itself are data.
        let wrapped = wrap_argv_if(
            true,
            argv(&["editor", "--host", "-n", "a b", "it's"]),
            None,
            &[],
        );
        assert_eq!(&wrapped[..3], ["flatpak-spawn", "--host", "editor"]);
        assert_eq!(&wrapped[3..], ["--host", "-n", "a b", "it's"]);
    }

    #[test]
    fn an_empty_command_is_not_wrapped_into_a_bare_flatpak_spawn() {
        assert!(wrap_argv_if(true, vec![], Some(Path::new("/x")), &[("A", "b")]).is_empty());
    }

    #[test]
    fn a_program_name_that_could_break_the_host_shell_is_refused() {
        assert!(!host_has(""));
        assert!(!host_has("a\nb"));
        assert!(!host_has("a\0b"));
    }

    #[test]
    fn this_test_run_is_not_in_a_sandbox() {
        // The suite runs on an ordinary machine or CI runner; a sandboxed run would wrap everything.
        if !Path::new(FLATPAK_INFO).exists() {
            assert!(!in_flatpak());
        }
    }
}
