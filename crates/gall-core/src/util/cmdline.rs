//! Command templates for "Open with" and "Terminal here": `zed {path}`,
//! `wezterm start --cwd {dir}`, `"C:\Program Files\App\app.exe" "%1"`.
//!
//! A template is split into words first (double quotes group words), then the
//! placeholders are filled in inside each word, so paths with spaces stay one argument.
//!
//! | Placeholder | Becomes |
//! |---|---|
//! | `{path}`, `%1`, `%L`, `%V`, `%f`, `%F`, `%u`, `%U` | the file or folder |
//! | `{dir}`, `%W` | its folder (the folder itself for "Terminal here") |
//! | `{name}` | its file name |
//!
//! Registry `%*` and `%2`…`%9` and desktop-entry `%i`, `%c`, `%k` are dropped.

use crate::error::{CoreError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Splits a command line into words; double quotes group words with spaces.
pub fn split_command(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

const PATH_CODES: &[&str] = &[
    "{path}", "%1", "%L", "%l", "%V", "%v", "%f", "%F", "%u", "%U",
];
/// Folder codes: Gall's `{dir}` and the registry's `%W` (working directory).
const DIR_CODES: &[&str] = &["{dir}", "%W", "%w"];

/// Program and arguments for `template` applied to `path` (with `dir` as `{dir}`).
/// When `append_path` is set and the template names no file, the path is added last.
pub fn build(template: &str, path: &Path, dir: &Path, append_path: bool) -> Result<Vec<String>> {
    let p = path.to_string_lossy();
    let d = dir.to_string_lossy();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut used = false;
    let mut argv: Vec<String> = split_command(template)
        .into_iter()
        .map(|w| {
            let mut w = w;
            for code in PATH_CODES {
                if w.contains(code) {
                    used = true;
                    w = w.replace(code, &p);
                }
            }
            for code in DIR_CODES {
                w = w.replace(code, &d);
            }
            w.replace("{name}", &name)
        })
        // Codes we don't fill are dropped: desktop-entry %i (icon), %c (name), %k (file);
        // registry %* (further files) and %2…%9.
        .filter(|w| {
            !matches!(w.as_str(), "%i" | "%c" | "%k" | "%*")
                && !(w.len() == 2
                    && w.starts_with('%')
                    && w[1..].chars().all(|c| c.is_ascii_digit()))
        })
        .collect();
    if argv.is_empty() {
        return Err(CoreError::Other("empty command".into()));
    }
    if append_path && !used {
        argv.push(p.into_owned());
    }
    Ok(argv)
}

/// Starts `argv` in `cwd` without waiting. Console programs (shells) get their own
/// window on Windows instead of sharing ours.
pub fn spawn(argv: &[String], cwd: Option<&Path>) -> Result<()> {
    let (bin, args) = argv
        .split_first()
        .ok_or_else(|| CoreError::Other("empty command".into()))?;
    // Windows: `code` is `code.cmd` on the PATH or `Code.exe` in App Paths; Command
    // itself only finds `.exe` files on the PATH.
    #[cfg(windows)]
    let program = {
        let p = resolve(bin).unwrap_or_else(|| std::path::PathBuf::from(bin));
        gui_twin(&p).unwrap_or(p)
    };
    #[cfg(not(windows))]
    let program = Path::new(bin);
    let mut cmd = Command::new(program.as_os_str());
    cmd.args(args);
    if let Some(d) = cwd.filter(|d| d.is_dir()) {
        cmd.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // A .cmd/.bat launcher (VS Code's `code.cmd`) would flash a console window.
        let script = program
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
        // Console programs (shells, TUI editors) need a window of their own; GUI programs
        // get none (it would only flash).
        let flags = if script {
            CREATE_NO_WINDOW
        } else if pe_subsystem(&program) == Some(PeSubsystem::Gui) {
            0
        } else {
            CREATE_NEW_CONSOLE
        };
        cmd.creation_flags(flags);
    }
    // Let the new window come to the front (Windows refuses otherwise).
    crate::util::openwith::allow_foreground();
    let mut child = cmd
        .spawn()
        .map_err(|e| CoreError::Other(format!("cannot start '{bin}': {e}")))?;
    // Reap it when it exits so no zombie is left behind.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum PeSubsystem {
    Gui,
    Console,
}

/// Whether a Windows executable is a GUI or a console program (the PE optional header's
/// Subsystem field). None for anything unreadable or not a PE image (store app aliases).
#[cfg_attr(not(windows), allow(dead_code))]
fn pe_subsystem(exe: &Path) -> Option<PeSubsystem> {
    use std::io::Read;
    let mut head = [0u8; 1024];
    let n = std::fs::File::open(exe).ok()?.read(&mut head).ok()?;
    pe_subsystem_of(&head[..n])
}

fn pe_subsystem_of(head: &[u8]) -> Option<PeSubsystem> {
    if head.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(head.get(0x3C..0x40)?.try_into().ok()?) as usize;
    if head.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    // COFF header (20 bytes) after the signature, then the optional header: Subsystem at +68.
    let at = pe + 4 + 20 + 68;
    match u16::from_le_bytes(head.get(at..at + 2)?.try_into().ok()?) {
        2 => Some(PeSubsystem::Gui),
        3 => Some(PeSubsystem::Console),
        _ => None,
    }
}

/// A console launcher's GUI build next to it: `wezterm.exe` (console, opens a console
/// window first) → `wezterm-gui.exe`, which takes the same `start --cwd …` arguments.
/// Also works for Scoop shims, which come in matching console/GUI pairs.
#[cfg_attr(not(windows), allow(dead_code))]
fn gui_twin(program: &Path) -> Option<PathBuf> {
    let stem = program.file_stem()?.to_str()?;
    if stem.to_ascii_lowercase().ends_with("-gui")
        || pe_subsystem(program) != Some(PeSubsystem::Console)
    {
        return None;
    }
    let twin = program.with_file_name(format!("{stem}-gui.exe"));
    (pe_subsystem(&twin) == Some(PeSubsystem::Gui)).then_some(twin)
}

/// True if `bin` can be started: an existing absolute path or a program on the PATH
/// (on Windows also `.cmd`/`.bat` and programs registered under App Paths).
pub fn which(bin: &str) -> bool {
    resolve(bin).is_some()
}

/// Where `bin` would be started from. Store apps such as Windows Terminal (`wt.exe`)
/// are zero-byte "app execution aliases" that can't be opened like a file, so the
/// existence of the entry is enough.
pub fn resolve(bin: &str) -> Option<PathBuf> {
    let p = Path::new(bin);
    if p.is_absolute() {
        return p.exists().then(|| p.to_path_buf());
    }
    let on_path = |name: &str| -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join(name))
            .find(|f| f.symlink_metadata().is_ok_and(|m| !m.is_dir()))
    };
    if !cfg!(windows) || p.extension().is_some() {
        return on_path(bin).or_else(|| app_path(bin));
    }
    // Windows without an extension: a real .exe first, then App Paths (VS Code's
    // Code.exe rather than its code.cmd launcher), then the scripts.
    on_path(&format!("{bin}.exe"))
        .or_else(|| app_path(&format!("{bin}.exe")))
        .or_else(|| {
            [".com", ".cmd", ".bat"]
                .iter()
                .find_map(|ext| on_path(&format!("{bin}{ext}")))
        })
}

/// Programs registered under `App Paths` (what Win+R finds without the PATH).
#[cfg(windows)]
fn app_path(exe: &str) -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let key = format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .find_map(|root| {
            let v: String = winreg::RegKey::predef(root)
                .open_subkey(&key)
                .ok()?
                .get_value("")
                .ok()?;
            let v = crate::util::openwith::win_names::expand_env(&v);
            let p = PathBuf::from(v.trim().trim_matches('"'));
            p.is_file().then_some(p)
        })
}

#[cfg(not(windows))]
fn app_path(_exe: &str) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(t: &str, path: &str, append: bool) -> Vec<String> {
        let p = Path::new(path);
        build(t, p, p.parent().unwrap_or(p), append).unwrap()
    }

    #[test]
    fn folder_with_spaces_stays_one_argument() {
        assert_eq!(
            b("wt.exe -d {dir}", r"/Ana Novak/My Files/x", false),
            vec!["wt.exe", "-d", "/Ana Novak/My Files"]
        );
    }

    #[test]
    fn quoted_program_and_placeholder_inside_a_word() {
        assert_eq!(
            b(
                r#""C:\Program Files\Git\git-bash.exe" --cd={dir}"#,
                "/home/a b/f",
                false
            ),
            vec![r"C:\Program Files\Git\git-bash.exe", "--cd=/home/a b"]
        );
    }

    #[test]
    fn registry_and_desktop_codes_and_appending() {
        assert_eq!(
            b(r#""C:\Apps\Code.exe" "%1""#, "/d/x y.md", true),
            vec![r"C:\Apps\Code.exe", "/d/x y.md"]
        );
        assert_eq!(
            b("gedit %U %i", "/d/a.txt", true),
            vec!["gedit", "/d/a.txt"]
        );
        assert_eq!(b("zed", "/d/a.md", true), vec!["zed", "/d/a.md"]);
        assert_eq!(b("zed", "/d/a.md", false), vec!["zed"]);
        assert_eq!(
            b("echo {name}", "/d/a.md", true),
            vec!["echo", "a.md", "/d/a.md"]
        );
        // Registry: further-files and working-directory codes.
        assert_eq!(
            b(r#""C:\Py\py.exe" "%L" %*"#, "/d/a b.py", true),
            vec![r"C:\Py\py.exe", "/d/a b.py"]
        );
        assert_eq!(
            b(r#"app.exe /dir:%W "%V" %2"#, "/d/a.txt", true),
            vec!["app.exe", "/dir:/d", "/d/a.txt"]
        );
    }

    /// A minimal PE header with the given Subsystem value.
    fn fake_pe(subsystem: u16) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[..2].copy_from_slice(b"MZ");
        b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        let at = 0x80 + 4 + 20 + 68;
        b[at..at + 2].copy_from_slice(&subsystem.to_le_bytes());
        b
    }

    #[test]
    fn reads_pe_subsystem() {
        assert_eq!(pe_subsystem_of(&fake_pe(2)), Some(PeSubsystem::Gui));
        assert_eq!(pe_subsystem_of(&fake_pe(3)), Some(PeSubsystem::Console));
        assert_eq!(pe_subsystem_of(&fake_pe(10)), None);
        assert_eq!(pe_subsystem_of(b"#!/bin/sh\n"), None);
        assert_eq!(pe_subsystem_of(b"MZ"), None);
    }

    #[test]
    fn console_launcher_prefers_its_gui_twin() {
        let d = tempfile::tempdir().unwrap();
        let cli = d.path().join("wezterm.exe");
        let gui = d.path().join("wezterm-gui.exe");
        std::fs::write(&cli, fake_pe(3)).unwrap();
        assert_eq!(gui_twin(&cli), None, "no twin yet");
        std::fs::write(&gui, fake_pe(2)).unwrap();
        assert_eq!(gui_twin(&cli), Some(gui.clone()));
        assert_eq!(gui_twin(&gui), None, "already the GUI build");
        // A GUI program is started as is.
        let alacritty = d.path().join("alacritty.exe");
        std::fs::write(&alacritty, fake_pe(2)).unwrap();
        std::fs::write(d.path().join("alacritty-gui.exe"), fake_pe(2)).unwrap();
        assert_eq!(gui_twin(&alacritty), None);
    }

    #[test]
    fn empty_command_is_an_error() {
        assert!(build("   ", Path::new("/"), Path::new("/"), true).is_err());
        assert_eq!(split_command(r#"a "" b"#), vec!["a", "", "b"]);
    }
}
