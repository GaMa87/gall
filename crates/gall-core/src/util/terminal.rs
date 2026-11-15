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
    let mut out: Vec<AppChoice> = KNOWN
        .iter()
        .filter(|(_, bin, _)| which(bin))
        .map(|(name, _, cmd)| AppChoice {
            id: cmd.to_string(),
            name: name.to_string(),
            default: false,
            command: cmd.to_string(),
        })
        .collect();
    #[cfg(target_os = "linux")]
    if let Ok(t) = std::env::var("TERMINAL") {
        if which(&t) && !out.iter().any(|a| a.command == t) {
            out.insert(
                0,
                AppChoice {
                    id: t.clone(),
                    name: format!("{t} ($TERMINAL)"),
                    default: false,
                    command: t,
                },
            );
        }
    }
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

/// `terminal` (a started terminal command) extended to run `script` in it and wait for
/// Enter afterwards, so a short script's output does not vanish with the window.
#[cfg(all(unix, not(target_os = "macos")))]
fn script_argv(mut terminal: Vec<String>, script: &Path) -> Vec<String> {
    let program = terminal
        .first()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match program.as_str() {
        "gnome-terminal" | "ptyxis" | "wezterm" => terminal.push("--".into()),
        "xfce4-terminal" | "terminator" => terminal.push("-x".into()),
        // These take the program directly after the options.
        "kitty" | "foot" => {}
        _ => terminal.push("-e".into()),
    }
    terminal.extend([
        "sh".into(),
        "-c".into(),
        r#""$0"; s=$?; printf '\n[exit status %s] Press Enter to close ' "$s"; read _"#.into(),
        script.to_string_lossy().into_owned(),
    ]);
    terminal
}

// ───────────────────────────── editors ─────────────────────────────

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
    let mut out: Vec<AppChoice> = Vec::new();
    for (name, bin, cmd) in EDITORS {
        if !out.iter().any(|a| a.name == *name) && which(bin) {
            out.push(AppChoice {
                id: cmd.to_string(),
                name: name.to_string(),
                default: false,
                command: cmd.to_string(),
            });
        }
    }
    for var in ["VISUAL", "EDITOR"] {
        if let Ok(e) = std::env::var(var) {
            let bin = e.split_whitespace().next().unwrap_or("").to_string();
            // Terminal editors (vim, nano, …) need a terminal; only GUI ones are useful here.
            let gui = ![
                "vi",
                "vim",
                "nvim",
                "nano",
                "micro",
                "hx",
                "helix",
                "emacs -nw",
                "kak",
                "ed",
            ]
            .contains(&bin.as_str());
            if gui
                && which(&bin)
                && !out
                    .iter()
                    .any(|a| a.command.split_whitespace().next() == Some(bin.as_str()))
            {
                out.push(AppChoice {
                    id: e.clone(),
                    name: format!("{e} (${var})"),
                    default: false,
                    command: format!("{e} {{path}}"),
                });
            }
        }
    }
    out
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    fn argv(template: &[&str]) -> Vec<String> {
        script_argv(
            template.iter().map(|s| s.to_string()).collect(),
            Path::new("/tmp/a b/run.sh"),
        )
    }

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
}
