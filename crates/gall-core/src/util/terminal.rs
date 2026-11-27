//! "Open terminal here" (X-02): the configured terminal, else the first one found.

use crate::error::{CoreError, Result};
use crate::util::cmdline::{build, spawn, which};
use crate::util::openwith::AppChoice;
use std::path::Path;

/// Known terminals: (display name, program checked on the PATH or an absolute path
/// to check, command template). Order = preference when none is configured.
#[cfg(target_os = "linux")]
const KNOWN: &[(&str, &str, &str)] = &[
    (
        "Default terminal",
        "xdg-terminal-exec",
        "xdg-terminal-exec --dir={dir}",
    ),
    (
        "System terminal",
        "x-terminal-emulator",
        "x-terminal-emulator",
    ),
    (
        "Ptyxis",
        "ptyxis",
        "ptyxis --new-window --working-directory={dir}",
    ),
    (
        "GNOME Terminal",
        "gnome-terminal",
        "gnome-terminal --working-directory={dir}",
    ),
    ("Konsole", "konsole", "konsole --workdir {dir}"),
    ("GNOME Console", "kgx", "kgx --working-directory={dir}"),
    (
        "Xfce Terminal",
        "xfce4-terminal",
        "xfce4-terminal --working-directory={dir}",
    ),
    (
        "WezTerm",
        "wezterm",
        "wezterm start --always-new-process --cwd {dir}",
    ),
    ("Kitty", "kitty", "kitty --directory {dir}"),
    ("Ghostty", "ghostty", "ghostty --working-directory={dir}"),
    (
        "Alacritty",
        "alacritty",
        "alacritty --working-directory {dir}",
    ),
    ("Foot", "foot", "foot --working-directory={dir}"),
    ("Tilix", "tilix", "tilix --working-directory={dir}"),
    (
        "Terminator",
        "terminator",
        "terminator --working-directory={dir}",
    ),
    ("XTerm", "xterm", "xterm"),
];

#[cfg(target_os = "macos")]
const KNOWN: &[(&str, &str, &str)] = &[
    (
        "Terminal",
        "/System/Applications/Utilities/Terminal.app",
        "open -a Terminal {dir}",
    ),
    ("iTerm", "/Applications/iTerm.app", "open -a iTerm {dir}"),
    (
        "Ghostty",
        "/Applications/Ghostty.app",
        "open -a Ghostty {dir}",
    ),
    (
        "WezTerm",
        "wezterm",
        "wezterm start --always-new-process --cwd {dir}",
    ),
    ("Kitty", "kitty", "kitty --directory {dir}"),
    (
        "Alacritty",
        "alacritty",
        "alacritty --working-directory {dir}",
    ),
];

/// Windows Terminal if installed (Windows 11 has it), then the shells. Each starts
/// directly in the folder: no `cmd /c start`, whose quoting breaks folders with spaces.
#[cfg(windows)]
const KNOWN: &[(&str, &str, &str)] = &[
    ("Windows Terminal", "wt", "wt.exe -d {dir}"),
    ("PowerShell 7", "pwsh", "pwsh.exe -NoLogo"),
    ("Windows PowerShell", "powershell", "powershell.exe -NoLogo"),
    ("Command Prompt", "cmd", "cmd.exe"),
    (
        "WezTerm",
        "wezterm",
        "wezterm.exe start --always-new-process --cwd {dir}",
    ),
    (
        "Alacritty",
        "alacritty",
        "alacritty.exe --working-directory {dir}",
    ),
    (
        "Git Bash",
        r"C:\Program Files\Git\git-bash.exe",
        r#""C:\Program Files\Git\git-bash.exe" --cd={dir}"#,
    ),
];

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
const KNOWN: &[(&str, &str, &str)] = &[];

/// Terminals installed on this computer, in preference order. `default` marks the one
/// used when none is configured.
pub fn terminal_apps() -> Vec<AppChoice> {
    let env_term = std::env::var("TERMINAL").ok();
    terminal_apps_with(which, env_term.as_deref())
}

/// Testable helper for [`terminal_apps`] with injectable binary lookup and `$TERMINAL`.
pub fn terminal_apps_with<F: Fn(&str) -> bool>(
    has_bin: F,
    env_terminal: Option<&str>,
) -> Vec<AppChoice> {
    let mut out: Vec<AppChoice> = KNOWN
        .iter()
        .filter(|(_, bin, _)| has_bin(bin))
        .map(|(name, _, cmd)| AppChoice {
            id: cmd.to_string(),
            name: name.to_string(),
            default: false,
            command: cmd.to_string(),
        })
        .collect();
    #[cfg(target_os = "linux")]
    if let Some(t) = env_terminal.map(str::trim).filter(|s| !s.is_empty()) {
        let bin = t.split_whitespace().next().unwrap_or("");
        let has_extra_args = t != bin;
        let bin_name = Path::new(bin)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(bin);
        if !has_extra_args {
            if let Some(pos) = out.iter().position(|a| {
                a.command
                    .split_whitespace()
                    .next()
                    .and_then(|c| Path::new(c).file_name().and_then(|s| s.to_str()))
                    == Some(bin_name)
            }) {
                if pos > 0 {
                    let item = out.remove(pos);
                    out.insert(0, item);
                }
            } else if !bin.is_empty() && has_bin(bin) {
                out.insert(
                    0,
                    AppChoice {
                        id: t.to_string(),
                        name: format!("{t} ($TERMINAL)"),
                        default: false,
                        command: t.to_string(),
                    },
                );
            }
        } else if !bin.is_empty() && has_bin(bin) && !out.iter().any(|a| a.command == t) {
            out.insert(
                0,
                AppChoice {
                    id: t.to_string(),
                    name: format!("{t} ($TERMINAL)"),
                    default: false,
                    command: t.to_string(),
                },
            );
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = env_terminal;
    if let Some(first) = out.first_mut() {
        first.default = true;
    }
    out
}

/// Runs a terminal command template in `dir`.
pub fn open_terminal_with(dir: &Path, template: &str) -> Result<()> {
    spawn(&build(template, dir, dir, false)?, Some(dir))
}

/// Opens a terminal in `dir`: `configured` (a template, may use `{dir}`) if set,
/// else the first installed terminal.
pub fn open_terminal(dir: &Path, configured: &str) -> Result<()> {
    // A folder inside WSL: a shell of that distribution, in the Linux folder.
    #[cfg(windows)]
    if let Some(argv) = crate::fs::wsl::terminal_argv(&dir.to_string_lossy(), which("wt")) {
        return spawn(&argv, None);
    }
    if !configured.trim().is_empty() {
        return open_terminal_with(dir, configured);
    }
    let first = terminal_apps().into_iter().next().ok_or_else(|| {
        CoreError::Other(
            "no terminal found; choose one with Terminal here with… or in Settings › Window".into(),
        )
    })?;
    open_terminal_with(dir, &first.command)
}

/// Runs `script` in a terminal (Linux), so its output is visible: the terminal's
/// command-line form of "run this program" differs per terminal. `configured` is the
/// terminal template from the settings, else the first installed terminal. `None`
/// when there is no terminal.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn run_script(script: &Path, configured: &str) -> Option<Result<()>> {
    let dir = script.parent().filter(|d| d.is_dir())?;
    let template = if configured.trim().is_empty() {
        terminal_apps().into_iter().next()?.command
    } else {
        configured.to_string()
    };
    Some(
        build(&template, dir, dir, false)
            .map(|argv| script_argv(argv, script))
            .and_then(|argv| spawn(&argv, Some(dir))),
    )
}

/// Extends a started `terminal` argv to execute `command` inside it.
/// `xdg-terminal-exec` accepts `-- [command args...]` without `-e`.
pub fn command_in_terminal_argv(mut terminal: Vec<String>, command: &[String]) -> Vec<String> {
    let program = terminal
        .first()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match program.as_str() {
        "xdg-terminal-exec" | "gnome-terminal" | "ptyxis" | "wezterm" => {
            terminal.push("--".into());
        }
        "xfce4-terminal" | "terminator" => terminal.push("-x".into()),
        // These take the program directly after the options.
        "kitty" | "foot" => {}
        // `-e` takes one command line (run by a shell), not separate arguments.
        "tilix" | "kgx" => {
            terminal.push("-e".into());
            terminal.push(
                command
                    .iter()
                    .map(|a| super::actions::shell_quote(a))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            return terminal;
        }
        _ => terminal.push("-e".into()),
    }
    terminal.extend(command.iter().cloned());
    terminal
}

/// Runs `command` (argv) inside a terminal window in `dir` (using `configured`
/// template if non-empty, else the first installed terminal).
pub fn run_command_in_terminal(
    command: &[String],
    dir: Option<&Path>,
    configured: &str,
) -> Result<()> {
    if command.is_empty() {
        return Err(CoreError::Other("empty command".into()));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let work_dir = dir.filter(|d| d.is_dir()).unwrap_or_else(|| Path::new("."));
        let template = if configured.trim().is_empty() {
            terminal_apps()
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CoreError::Other(
                        "no terminal found; choose one with Terminal here with… or in Settings › Window"
                            .into(),
                    )
                })?
                .command
        } else {
            configured.to_string()
        };
        let term_argv = build(&template, work_dir, work_dir, false)?;
        let argv = command_in_terminal_argv(term_argv, command);
        spawn(&argv, Some(work_dir))
    }
    #[cfg(windows)]
    {
        let _ = configured;
        spawn(command, dir)
    }
    #[cfg(target_os = "macos")]
    {
        if !configured.trim().is_empty() {
            let work_dir = dir.filter(|d| d.is_dir()).unwrap_or_else(|| Path::new("."));
            let term_argv = build(configured, work_dir, work_dir, false)?;
            let prog = term_argv
                .first()
                .and_then(|p| Path::new(p).file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if prog != "open" {
                let argv = command_in_terminal_argv(term_argv, command);
                return spawn(&argv, Some(work_dir));
            }
        }
        // No CLI terminal configured: Terminal.app runs the command through AppleScript.
        let work_dir = dir.filter(|d| d.is_dir()).unwrap_or_else(|| Path::new("."));
        let argv = vec![
            "osascript".to_string(),
            "-e".to_string(),
            macos_terminal_script(work_dir, command),
            "-e".to_string(),
            "tell application \"Terminal\" to activate".to_string(),
        ];
        spawn(&argv, None)
    }
}

/// AppleScript that opens a Terminal.app window running `command` in `dir`.
#[cfg(any(target_os = "macos", test))]
#[allow(dead_code)]
fn macos_terminal_script(dir: &Path, command: &[String]) -> String {
    use super::actions::shell_quote;
    let line = format!(
        "cd {} && exec {}",
        shell_quote(&dir.to_string_lossy()),
        command
            .iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ")
    );
    // Inside an AppleScript string only the backslash and the double quote are special.
    let escaped = line.replace('\\', "\\\\").replace('"', "\\\"");
    format!("tell application \"Terminal\" to do script \"{escaped}\"")
}

/// `terminal` (a started terminal command) extended to run `script` in it and wait for
/// Enter afterwards, so a short script's output does not vanish with the window.
#[cfg(all(unix, not(target_os = "macos")))]
fn script_argv(terminal: Vec<String>, script: &Path) -> Vec<String> {
    command_in_terminal_argv(
        terminal,
        &[
            "sh".into(),
            "-c".into(),
            r#""$0"; s=$?; printf '\n[exit status %s] Press Enter to close ' "$s"; read _"#.into(),
            script.to_string_lossy().into_owned(),
        ],
    )
}

// ───────────────────────────── editors ─────────────────────────────

/// TUI editor executable basenames that require a terminal when launched directly.
pub const TUI_EDITORS: &[&str] = &[
    "vi", "vim", "nvim", "nano", "micro", "hx", "helix", "kak", "ed", "fresh",
];

/// Returns true if `argv` invokes a TUI editor (including `emacs -nw` / `-t`).
pub fn is_tui_editor_argv(argv: &[String]) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };
    let bin = Path::new(first)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(first.as_str());
    let bin = bin.strip_suffix(".exe").unwrap_or(bin);
    if TUI_EDITORS.contains(&bin) {
        return true;
    }
    (bin == "emacs" || bin == "emacsclient")
        && argv
            .iter()
            .any(|a| a == "-nw" || a == "--no-window-system" || a == "-t")
}

/// Known text editors: (display name, program on the PATH or absolute path, template).
#[cfg(target_os = "linux")]
const EDITORS: &[(&str, &str, &str)] = &[
    ("VS Code", "code", "code {path}"),
    ("VSCodium", "codium", "codium {path}"),
    ("Zed", "zed", "zed {path}"),
    ("Zed", "zeditor", "zeditor {path}"),
    ("Sublime Text", "subl", "subl {path}"),
    ("Kate", "kate", "kate {path}"),
    ("KWrite", "kwrite", "kwrite {path}"),
    (
        "GNOME Text Editor",
        "gnome-text-editor",
        "gnome-text-editor {path}",
    ),
    ("gedit", "gedit", "gedit {path}"),
    ("Mousepad", "mousepad", "mousepad {path}"),
    ("Xed", "xed", "xed {path}"),
    ("Emacs", "emacs", "emacs {path}"),
    ("Neovim", "nvim", "nvim {path}"),
    ("Helix", "hx", "hx {path}"),
    ("Helix", "helix", "helix {path}"),
    ("Vim", "vim", "vim {path}"),
    ("Nano", "nano", "nano {path}"),
    ("Micro", "micro", "micro {path}"),
    ("Fresh", "fresh", "fresh {path}"),
    ("Kakoune", "kak", "kak {path}"),
];

#[cfg(target_os = "macos")]
const EDITORS: &[(&str, &str, &str)] = &[
    ("VS Code", "code", "code {path}"),
    ("Zed", "zed", "zed {path}"),
    ("Sublime Text", "subl", "subl {path}"),
    (
        "TextEdit",
        "/System/Applications/TextEdit.app",
        "open -a TextEdit {path}",
    ),
];

/// Bare names are looked up when run (PATH, App Paths, `.cmd`: see
/// [`crate::util::cmdline::resolve`]), so `code` starts `Code.exe` without a console.
#[cfg(windows)]
const EDITORS: &[(&str, &str, &str)] = &[
    ("VS Code", "code", "code {path}"),
    ("VSCodium", "codium", "codium {path}"),
    ("Zed", "zed", "zed {path}"),
    ("Sublime Text", "subl", "subl {path}"),
    ("Sublime Text", "sublime_text", "sublime_text {path}"),
    (
        "Sublime Text",
        r"C:\Program Files\Sublime Text\sublime_text.exe",
        r#""C:\Program Files\Sublime Text\sublime_text.exe" {path}"#,
    ),
    ("Notepad++", "notepad++", "notepad++ {path}"),
    (
        "Notepad++",
        r"C:\Program Files\Notepad++\notepad++.exe",
        r#""C:\Program Files\Notepad++\notepad++.exe" {path}"#,
    ),
    ("Notepad", "notepad", "notepad {path}"),
];

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
const EDITORS: &[(&str, &str, &str)] = &[];

/// Text editors installed on this computer (Edit with…), in preference order.
pub fn editor_apps() -> Vec<AppChoice> {
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    editor_apps_with(which, visual.as_deref(), editor.as_deref())
}

/// Testable helper for [`editor_apps`] with injectable binary lookup and `$VISUAL`/`$EDITOR`.
pub fn editor_apps_with<F: Fn(&str) -> bool>(
    has_bin: F,
    visual: Option<&str>,
    editor: Option<&str>,
) -> Vec<AppChoice> {
    let mut out: Vec<AppChoice> = Vec::new();
    #[cfg(target_os = "linux")]
    if has_bin("omarchy-launch-editor") {
        out.push(AppChoice {
            id: "omarchy-launch-editor {path}".into(),
            name: "Omarchy default editor".into(),
            default: true,
            command: "omarchy-launch-editor {path}".into(),
        });
    }
    for (name, bin, cmd) in EDITORS {
        if !out.iter().any(|a| a.name == *name) && has_bin(bin) {
            out.push(AppChoice {
                id: cmd.to_string(),
                name: name.to_string(),
                default: false,
                command: cmd.to_string(),
            });
        }
    }
    for (var, val) in [("VISUAL", visual), ("EDITOR", editor)] {
        let Some(e) = val.map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let bin = e.split_whitespace().next().unwrap_or("");
        if bin.is_empty() || !has_bin(bin) {
            continue;
        }
        let cmd = format!("{e} {{path}}");
        let dup = out.iter().any(|a| {
            a.command == cmd
                || (!e.contains(' ') && a.command.split_whitespace().next() == Some(bin))
        });
        if !dup {
            out.push(AppChoice {
                id: e.to_string(),
                name: format!("{e} (${var})"),
                default: false,
                command: cmd,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(unix, not(target_os = "macos")))]
    fn argv(template: &[&str]) -> Vec<String> {
        script_argv(
            template.iter().map(|s| s.to_string()).collect(),
            Path::new("/tmp/a b/run.sh"),
        )
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn macos_terminal_script_quotes_for_the_shell_and_for_applescript() {
        let script = macos_terminal_script(
            Path::new("/tmp/a b"),
            &["vim".to_string(), "it's \"x\\y\".txt".to_string()],
        );
        assert_eq!(
            script,
            r#"tell application "Terminal" to do script "cd '/tmp/a b' && exec 'vim' 'it'\\''s \"x\\y\".txt'""#
        );
    }

    #[test]
    fn tilix_and_kgx_get_one_quoted_command_line_after_dash_e() {
        let cmd = vec!["nano".to_string(), "a b.txt".to_string()];
        for term in ["tilix", "kgx"] {
            assert_eq!(
                command_in_terminal_argv(vec![term.into()], &cmd),
                vec![term, "-e", "'nano' 'a b.txt'"]
            );
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn script_runs_after_the_terminals_execute_flag() {
        let konsole = argv(&["konsole", "--workdir", "/tmp/a b"]);
        assert_eq!(&konsole[..4], ["konsole", "--workdir", "/tmp/a b", "-e"]);
        assert_eq!(konsole[4], "sh");
        // The script path is a separate argument, never part of the shell text.
        assert_eq!(konsole.last().unwrap(), "/tmp/a b/run.sh");
        assert!(konsole[6].contains("read"));
        assert_eq!(argv(&["gnome-terminal"])[1], "--");
        assert_eq!(argv(&["/usr/bin/xfce4-terminal"])[1], "-x");
        assert_eq!(argv(&["kitty", "--directory", "/x"])[3], "sh");
    }

    #[test]
    fn command_in_terminal_argv_handles_xdg_terminal_exec_and_flags() {
        let xte = command_in_terminal_argv(
            vec!["xdg-terminal-exec".into(), "--dir=/tmp/work".into()],
            &["nvim".into(), "-R".into(), "/tmp/a b.txt".into()],
        );
        assert_eq!(
            xte,
            vec![
                "xdg-terminal-exec",
                "--dir=/tmp/work",
                "--",
                "nvim",
                "-R",
                "/tmp/a b.txt"
            ]
        );
        // Must not insert `-e` for xdg-terminal-exec.
        assert!(!xte.iter().any(|a| a == "-e"));

        assert_eq!(
            command_in_terminal_argv(vec!["kitty".into()], &["hx".into(), "f".into()]),
            vec!["kitty", "hx", "f"]
        );
        assert_eq!(
            command_in_terminal_argv(vec!["foot".into()], &["vim".into(), "f".into()]),
            vec!["foot", "vim", "f"]
        );
        assert_eq!(
            command_in_terminal_argv(vec!["wezterm".into(), "start".into()], &["nano".into()]),
            vec!["wezterm", "start", "--", "nano"]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn terminal_apps_with_prefers_xdg_terminal_exec_and_env_terminal() {
        let apps = terminal_apps_with(
            |b| matches!(b, "xdg-terminal-exec" | "x-terminal-emulator" | "kitty"),
            None,
        );
        assert_eq!(apps[0].command, "xdg-terminal-exec --dir={dir}");
        assert!(apps[0].default);
        assert_eq!(apps[1].command, "x-terminal-emulator");
        assert!(!apps[1].default);

        let with_env = terminal_apps_with(
            |b| matches!(b, "xdg-terminal-exec" | "alacritty"),
            Some("alacritty"),
        );
        assert_eq!(with_env.len(), 2);
        assert_eq!(with_env[0].command, "alacritty --working-directory {dir}");
        assert!(with_env[0].default);
        assert_eq!(with_env[1].command, "xdg-terminal-exec --dir={dir}");
        assert!(!with_env[1].default);

        // Omarchy default: bare $TERMINAL=xdg-terminal-exec promotes existing entry with --dir={dir}
        // without duplicating it.
        let with_xte = terminal_apps_with(
            |b| matches!(b, "xdg-terminal-exec" | "kitty"),
            Some("xdg-terminal-exec"),
        );
        assert_eq!(with_xte.len(), 2);
        assert_eq!(with_xte[0].command, "xdg-terminal-exec --dir={dir}");
        assert!(with_xte[0].default);
        assert!(!with_xte[1].default);

        // Bare binary or explicit path for a KNOWN terminal with subcommands preserves its template.
        for term in ["wezterm", "/usr/bin/wezterm"] {
            let with_wez =
                terminal_apps_with(|b| matches!(b, "xdg-terminal-exec" | "wezterm"), Some(term));
            assert_eq!(with_wez.len(), 2);
            assert_eq!(
                with_wez[0].command,
                "wezterm start --always-new-process --cwd {dir}"
            );
            assert!(with_wez[0].default);
            assert!(!with_wez[1].default);
        }

        // Custom flags or unknown terminal binary inserts a dedicated ($TERMINAL) entry at index 0.
        let with_flags = terminal_apps_with(
            |b| matches!(b, "xdg-terminal-exec" | "kitty"),
            Some("kitty --single-instance"),
        );
        assert_eq!(with_flags.len(), 3);
        assert_eq!(with_flags[0].command, "kitty --single-instance");
        assert!(with_flags[0].default);
        assert!(!with_flags[1].default);

        let with_custom = terminal_apps_with(
            |b| matches!(b, "xdg-terminal-exec" | "custom-term"),
            Some("custom-term"),
        );
        assert_eq!(with_custom.len(), 2);
        assert_eq!(with_custom[0].command, "custom-term");
        assert!(with_custom[0].default);
        assert!(!with_custom[1].default);
    }

    #[test]
    fn tui_editor_detection_and_editor_apps() {
        for bin in [
            "nvim",
            "/usr/bin/hx",
            "helix",
            "vim",
            "nano",
            "micro",
            "fresh",
            "kak",
            "ed",
            "vi",
        ] {
            assert!(
                is_tui_editor_argv(&[bin.into(), "file.txt".into()]),
                "{bin} should be detected as TUI editor"
            );
        }
        assert!(is_tui_editor_argv(&[
            "emacs".into(),
            "-nw".into(),
            "a.txt".into()
        ]));
        assert!(is_tui_editor_argv(&[
            "emacsclient".into(),
            "-t".into(),
            "a.txt".into()
        ]));
        assert!(!is_tui_editor_argv(&["emacs".into(), "a.txt".into()]));
        assert!(!is_tui_editor_argv(&["code".into(), "a.txt".into()]));
        assert!(!is_tui_editor_argv(&[
            "omarchy-launch-editor".into(),
            "a.txt".into()
        ]));

        #[cfg(target_os = "linux")]
        {
            let editors = editor_apps_with(
                |b| matches!(b, "omarchy-launch-editor" | "nvim" | "hx" | "emacs" | "kak"),
                Some("hx"),
                Some("emacs -nw"),
            );
            assert_eq!(editors[0].command, "omarchy-launch-editor {path}");
            assert!(editors[0].default);
            assert!(editors.iter().any(|a| a.command == "nvim {path}"));
            assert!(editors.iter().any(|a| a.command == "hx {path}"));
            assert!(editors.iter().any(|a| a.command == "emacs -nw {path}"));
            assert!(editors.iter().any(|a| a.command == "kak {path}"));
        }
    }
}
