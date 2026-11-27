//! "Open with…": open files with their default application or any installed program.
//!
//! - Linux (freedesktop): Shared MIME-info (`globs2`, `aliases`, `subclasses`, content
//!   sniffing), desktop-specific `mimeapps.list`, `mimeinfo.cache`, `.desktop` files
//!   (including `Terminal=true`, `Path=`, `%F`/`%U` batching, Flatpak/Snap export dirs,
//!   and updating `~/.config/mimeapps.list` for OS-wide defaults).
//! - Windows: `ShellExecuteW` (default verb and `runas` for elevation), extension +
//!   `PerceivedType` + `SystemFileAssociations` + `*` registry discovery, plus all
//!   registered `HKCR\Applications` and the native Windows "Open with" dialog.
//! - macOS: LaunchServices (`LSCopyDefaultApplicationURLForURL`, `LSCopyApplicationURLsForURL`)
//!   combined with `.app` bundle `Info.plist` scanning and `/usr/bin/open` (including
//!   multi-file batching).
//! - Everywhere: custom command templates (see [`crate::util::cmdline`]).
//!
//! Every choice carries a `command` that [`run_choice`] / [`run_choice_many_in`] runs and
//! that Gall stores when the user makes it the default for a file type.

use crate::error::{CoreError, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;

/// Error prefix used when no default application is registered for a file type.
/// The UI matches this to automatically open the "Open with…" chooser, like OS file managers.
pub const NO_DEFAULT_APP_PREFIX: &str = "no default application";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppChoice {
    /// Desktop file id (`org.gnome.TextEditor.desktop`), Windows ProgID/exe, or macOS bundle path.
    pub id: String,
    pub name: String,
    /// The system's default application for this type.
    pub default: bool,
    /// What to run: `desktop:<id>` for a freedesktop application, `macos:<bundle>` for a
    /// macOS `.app` bundle, `system` for the system default, otherwise a command template (`zed {path}`).
    pub command: String,
}

/// Runs a choice's `command` for `path` (see [`AppChoice::command`]).
pub fn run_choice(path: &Path, command: &str) -> Result<()> {
    run_choice_in(path, command, "")
}

/// Pure routing helper for custom command choices (`"terminal:"` prefix and TUI editors).
/// Returns `(needs_terminal, argv, work_dir)`.
pub fn plan_choice_command(path: &Path, command: &str) -> Result<(bool, Vec<String>, PathBuf)> {
    let command = command.trim();
    let (force_term, tmpl) = match command.strip_prefix("terminal:") {
        Some(rest) => (true, rest.trim()),
        None => (false, command),
    };
    let dir = work_dir_for(path);
    let argv = crate::util::cmdline::build(tmpl, path, &dir, true)?;
    let needs_term = force_term || crate::util::terminal::is_tui_editor_argv(&argv);
    Ok((needs_term, argv, dir))
}

fn work_dir_for(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }
}

/// Runs a choice's `command` for `path`, using `terminal` template when a terminal is needed.
pub fn run_choice_in(path: &Path, command: &str, terminal: &str) -> Result<()> {
    run_choice_many_in(&[path.to_path_buf()], command, terminal)
}

/// Runs a choice's `command` for one or more `paths`, batching them into a single invocation
/// when the application supports multiple files (`%F`/`%U` on Linux, `open -a` on macOS).
pub fn run_choice_many_in(paths: &[PathBuf], command: &str, terminal: &str) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let command = command.trim();
    if command == "system" {
        return open_default_many_in(paths, terminal);
    }
    if let Some(id) = command.strip_prefix("desktop:") {
        return open_with_app_many_in(paths, id, terminal);
    }
    if let Some(bundle) = command.strip_prefix("macos:") {
        return open_with_macos_bundle(paths, bundle);
    }
    for path in paths {
        let (needs_term, argv, dir) = plan_choice_command(path, command)?;
        if needs_term {
            crate::util::terminal::run_command_in_terminal(&argv, Some(&dir), terminal)?;
        } else {
            crate::util::cmdline::spawn(&argv, Some(&dir))?;
        }
    }
    Ok(())
}

/// Opens `paths` with a macOS `.app` bundle via `/usr/bin/open -a <bundle>`.
fn open_with_macos_bundle(paths: &[PathBuf], bundle: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut argv = vec!["/usr/bin/open".to_string(), "-a".to_string(), bundle.into()];
        for p in paths {
            argv.push(safe_open_arg(p));
        }
        crate::util::cmdline::spawn(&argv, paths.first().map(|p| work_dir_for(p)).as_deref())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (paths, bundle);
        Err(CoreError::Other(
            "macOS application bundles are only supported on macOS".into(),
        ))
    }
}

/// Prefixes a relative path starting with `-` with `./` so `/usr/bin/open` / `gio` / `xdg-open`
/// cannot mistake it for an option flag.
#[cfg_attr(windows, allow(dead_code))]
fn safe_open_arg(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    if s.starts_with('-') {
        format!("./{s}")
    } else {
        s
    }
}

/// Fallback platform opener (`gio open` / `xdg-open` on Linux, `/usr/bin/open` on macOS).
#[cfg_attr(windows, allow(dead_code))]
fn open_system(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let arg = safe_open_arg(path);
        let status = std::process::Command::new("/usr/bin/open")
            .arg(&arg)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| CoreError::Other(format!("cannot run open: {e}")))?;
        if status.status.success() {
            return Ok(());
        }
        let msg = String::from_utf8_lossy(&status.stderr).trim().to_string();
        if msg.to_ascii_lowercase().contains("no application") {
            return Err(no_default_err(path));
        }
        Err(CoreError::Other(if msg.is_empty() {
            format!("cannot open '{}'", path.display())
        } else {
            msg
        }))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let arg = safe_open_arg(path);
        if crate::util::cmdline::which("gio") {
            return crate::util::cmdline::spawn(
                &["gio".to_string(), "open".to_string(), arg],
                path.parent(),
            );
        }
        crate::util::cmdline::spawn(&["xdg-open".to_string(), arg], path.parent())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(no_default_err(path))
    }
}

fn no_default_err(path: &Path) -> CoreError {
    CoreError::Other(format!(
        "{NO_DEFAULT_APP_PREFIX} is set to open '{}'; use Open with…",
        path.display()
    ))
}

/// Applications registered for `path`, default first.
pub fn apps_for(path: &Path) -> Vec<AppChoice> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dirs = xdg::Dirs::from_env();
        let desktops = xdg::desktops_from_env();
        xdg::apps_for_desktops(path, &dirs, &desktops)
    }
    #[cfg(windows)]
    {
        win::apps_for(path)
    }
    #[cfg(target_os = "macos")]
    {
        macos::apps_for(path)
    }
}

/// All installed GUI/desktop applications on this OS (for "Show all applications" in Open with…),
/// sorted alphabetically by display name.
pub fn all_apps() -> Vec<AppChoice> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dirs = xdg::Dirs::from_env();
        xdg::all_apps(&dirs)
    }
    #[cfg(windows)]
    {
        win::all_apps()
    }
    #[cfg(target_os = "macos")]
    {
        macos::all_apps()
    }
}

/// MIME type of `path` on Linux (used by the UI to describe OS default updates).
pub fn mime_type_of(path: &Path) -> Option<String> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dirs = xdg::Dirs::from_env();
        xdg::mime_type(path, &dirs)
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = path;
        None
    }
}

/// Updates the OS-wide default application for `path`'s file type.
/// - Linux: writes `$XDG_CONFIG_HOME/mimeapps.list` `[Default Applications]` for the file's MIME type.
/// - macOS: sets the LaunchServices default handler when a bundle identifier is available.
/// - Windows: Windows protects `UserChoice` with a system hash; callers should open [`system_dialog`].
pub fn set_system_default(path: &Path, id: &str) -> Result<String> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dirs = xdg::Dirs::from_env();
        let mime = xdg::mime_type(path, &dirs).ok_or_else(|| {
            CoreError::Other(format!(
                "cannot determine MIME type for '{}'",
                path.display()
            ))
        })?;
        let desktop_id = id.strip_prefix("desktop:").unwrap_or(id).trim();
        if desktop_id.is_empty() || !desktop_id.ends_with(".desktop") {
            return Err(CoreError::Other(
                "only desktop applications can be set as the system default".into(),
            ));
        }
        let mimeapps = dirs
            .config
            .first()
            .cloned()
            .unwrap_or_else(|| dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")))
            .join("mimeapps.list");
        xdg::set_default_in_file(&mimeapps, &mime, desktop_id)?;
        Ok(mime)
    }
    #[cfg(target_os = "macos")]
    {
        macos::set_system_default(path, id)
    }
    #[cfg(windows)]
    {
        let _ = (path, id);
        Err(CoreError::Other(
            "Windows manages system defaults through the Windows \"Open with\" dialog".into(),
        ))
    }
}

/// Opens `path` with the application `id` from [`apps_for`].
pub fn open_with_app(path: &Path, id: &str) -> Result<()> {
    open_with_app_in(path, id, "")
}

/// Opens `path` with the application `id` from [`apps_for`], using `terminal` for `Terminal=true` entries.
pub fn open_with_app_in(path: &Path, id: &str, terminal: &str) -> Result<()> {
    open_with_app_many_in(&[path.to_path_buf()], id, terminal)
}

/// Opens one or more `paths` with the application `id`, passing them together when the
/// desktop entry supports `%F`/`%U` (or launching once per path for `%f`/`%u`).
pub fn open_with_app_many_in(paths: &[PathBuf], id: &str, terminal: &str) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dirs = xdg::Dirs::from_env();
        let entry = dirs
            .find_desktop(id)
            .and_then(|f| xdg::DesktopEntry::read(&f))
            .ok_or_else(|| CoreError::Other(format!("application {id} not found")))?;
        launch_desktop_entry(&entry, paths, terminal)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = terminal;
        let bundle = id.strip_prefix("macos:").unwrap_or(id);
        open_with_macos_bundle(paths, bundle)
    }
    #[cfg(not(unix))]
    {
        let _ = (paths, id, terminal);
        Err(CoreError::Other(
            "choose the application in the system dialog".into(),
        ))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn launch_desktop_entry(
    entry: &xdg::DesktopEntry,
    paths: &[PathBuf],
    terminal: &str,
) -> Result<()> {
    let default_dir = paths.first().map(|p| work_dir_for(p));
    let entry_dir = entry
        .path
        .as_deref()
        .map(Path::new)
        .filter(|d| d.is_dir())
        .map(Path::to_path_buf);
    let cwd = entry_dir.or(default_dir);
    for argv in expand_exec_many(&entry.exec, paths) {
        if entry.terminal {
            crate::util::terminal::run_command_in_terminal(&argv, cwd.as_deref(), terminal)?;
        } else {
            crate::util::cmdline::spawn(&argv, cwd.as_deref())?;
        }
    }
    Ok(())
}

/// Lets the program we are about to start (or hand a file to) come to the front.
/// Windows only lets the foreground process raise windows; a program that is already
/// running, or one started via another process, would otherwise open behind Gall and
/// just flash in the taskbar. Explorer does the same before opening files.
pub fn allow_foreground() {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn AllowSetForegroundWindow(process_id: u32) -> i32;
        }
        const ASFW_ANY: u32 = u32::MAX;
        // SAFETY: plain Win32 call without pointers.
        unsafe {
            AllowSetForegroundWindow(ASFW_ANY);
        }
    }
}

/// Opens `path` with its default application using the auto-detected terminal when needed.
pub fn open_default(path: &Path) -> Option<Result<()>> {
    open_default_in(path, "")
}

/// Opens `path` with its system default application, using `terminal` when the Linux
/// desktop entry sets `Terminal=true`.
///
/// - Linux (outside Flatpak): resolves the real Freedesktop default `.desktop` entry
///   (Shared MIME-info `globs2` + `aliases` + content sniffing + `subclasses` +
///   desktop-specific `mimeapps.list` + `mimeinfo.cache`), launching `Terminal=true` apps
///   inside a terminal and GUI apps cleanly (fixing `xdg-open` on Hyprland/Omarchy/Sway/etc.).
///   Falls back to `gio open` / `xdg-open` if no desktop entry matched.
/// - Windows: `ShellExecuteW` + `AllowSetForegroundWindow`.
/// - macOS: `/usr/bin/open` (LaunchServices).
pub fn open_default_in(path: &Path, terminal: &str) -> Option<Result<()>> {
    #[cfg(windows)]
    {
        let _ = terminal;
        Some(win_shell_execute(path, None))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if crate::util::hostspawn::in_flatpak() {
            // Inside Flatpak, host `.desktop` files live outside the sandbox runtime; let the
            // portal / opener plugin handle default opening.
            return None;
        }
        let dirs = xdg::Dirs::from_env();
        let desktops = xdg::desktops_from_env();
        if let Some((_, entry)) = xdg::default_entry_for_desktops(path, &dirs, &desktops) {
            return Some(launch_desktop_entry(
                &entry,
                &[path.to_path_buf()],
                terminal,
            ));
        }
        // No desktop entry resolved: fall back to `gio open` if installed, else report
        // no default application so the UI can open the "Open with…" chooser.
        if crate::util::cmdline::which("gio") {
            return Some(gio_open_checked(path));
        }
        Some(Err(no_default_err(path)))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = terminal;
        Some(open_system(path))
    }
}

/// Opens multiple `paths` with their default applications, grouping files that share a
/// multi-file-capable desktop entry (`%F`/`%U` on Linux) into a single launch.
pub fn open_default_many_in(paths: &[PathBuf], terminal: &str) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if !crate::util::hostspawn::in_flatpak() && paths.len() > 1 {
        let dirs = xdg::Dirs::from_env();
        let desktops = xdg::desktops_from_env();
        let mut groups: Vec<(Option<String>, Option<xdg::DesktopEntry>, Vec<PathBuf>)> = Vec::new();
        for p in paths {
            match xdg::default_entry_for_desktops(p, &dirs, &desktops) {
                Some((id, entry)) if exec_supports_multiple(&entry.exec) => {
                    if let Some(g) = groups.iter_mut().find(|g| g.0.as_deref() == Some(&id)) {
                        g.2.push(p.clone());
                    } else {
                        groups.push((Some(id), Some(entry), vec![p.clone()]));
                    }
                }
                Some((id, entry)) => {
                    groups.push((Some(id), Some(entry), vec![p.clone()]));
                }
                None => {
                    groups.push((None, None, vec![p.clone()]));
                }
            }
        }
        for (_, entry, batch) in groups {
            if let Some(entry) = entry {
                launch_desktop_entry(&entry, &batch, terminal)?;
            } else {
                for p in &batch {
                    if crate::util::cmdline::which("gio") {
                        gio_open_checked(p)?;
                    } else {
                        return Err(no_default_err(p));
                    }
                }
            }
        }
        return Ok(());
    }
    for p in paths {
        match open_default_in(p, terminal) {
            Some(r) => r?,
            None => open_system(p)?,
        }
    }
    Ok(())
}

/// Runs `gio open <path>` and checks whether it failed with "No application is registered".
#[cfg(all(unix, not(target_os = "macos")))]
fn gio_open_checked(path: &Path) -> Result<()> {
    let arg = safe_open_arg(path);
    let out = std::process::Command::new("gio")
        .args(["open", &arg])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| CoreError::Other(format!("cannot run gio: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let lower = msg.to_ascii_lowercase();
    if lower.contains("no application") || lower.contains("not supported") {
        return Err(no_default_err(path));
    }
    Err(CoreError::Other(if msg.is_empty() {
        format!("cannot open '{}'", path.display())
    } else {
        msg
    }))
}

#[cfg(windows)]
fn win_shell_execute(path: &Path, verb: Option<&str>) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut std::ffi::c_void,
            op: *const u16,
            file: *const u16,
            params: *const u16,
            dir: *const u16,
            show: i32,
        ) -> isize;
    }
    const SW_SHOWNORMAL: i32 = 1;
    allow_foreground();
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let op: Option<Vec<u16>> = verb.map(|v| v.encode_utf16().chain(Some(0)).collect());
    let dir: Option<Vec<u16>> = path
        .parent()
        .map(|d| d.as_os_str().encode_wide().chain(Some(0)).collect());
    // SAFETY: NUL-terminated UTF-16 strings that outlive the call; a null verb
    // means the file type's default verb.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            wide.as_ptr(),
            std::ptr::null(),
            dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    if r > 32 {
        Ok(())
    } else {
        Err(match r {
            2 | 3 => CoreError::Other(format!("'{}' not found", path.display())),
            5 => CoreError::Other("access denied".into()),
            31 => no_default_err(path),
            _ => CoreError::Other(format!("cannot open '{}' (error {r})", path.display())),
        })
    }
}

/// Runs `path` with administrator privileges on Windows (`ShellExecuteW` `"runas"` verb).
pub fn open_elevated(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        win_shell_execute(path, Some("runas"))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(CoreError::Other(
            "Run as administrator is only available on Windows".into(),
        ))
    }
}

/// What a file manager starts instead of handing the file to the MIME handler.
#[cfg(all(unix, not(target_os = "macos")))]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Launcher {
    /// A native program or an AppImage: run directly.
    Program,
    /// A script with a `#!` line: run in a terminal, so its output is visible.
    Script,
    /// A `.desktop` launcher.
    Desktop,
}

/// How `path` is started when it is opened, if it is something to run: a regular file
/// with the execute bit that is an AppImage or ELF program (not a shared library), a
/// `#!` script or a `.desktop` file. Anything else is opened by its MIME handler.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn launcher(path: &Path) -> Option<Launcher> {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        return None;
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext.as_deref() {
        Some("appimage") => return Some(Launcher::Program),
        Some("desktop") => return Some(Launcher::Desktop),
        _ => {}
    }
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(18)
        .read_to_end(&mut head)
        .ok()?;
    if head.starts_with(b"#!") {
        return Some(Launcher::Script);
    }
    if head.len() < 18 || !head.starts_with(b"\x7fELF") {
        return None;
    }
    // AppImage: `AI` and a type byte at offset 8 of the ELF header.
    if &head[8..10] == b"AI" && matches!(head[10], 1 | 2) {
        return Some(Launcher::Program);
    }
    // e_type: executable (2) or position independent executable / library (3).
    let e_type = if head[5] == 2 {
        u16::from_be_bytes([head[16], head[17]])
    } else {
        u16::from_le_bytes([head[16], head[17]])
    };
    let name = path.file_name()?.to_string_lossy();
    let library = name.ends_with(".so") || name.contains(".so.");
    (matches!(e_type, 2 | 3) && !library).then_some(Launcher::Program)
}

/// Opens `path` the way a file manager does when it is something to run (see
/// [`launcher`]); `None` for any other file, which goes through the default opener.
/// `terminal` is the configured terminal template, used for scripts. Windows and macOS start
/// programs and scripts themselves, so they return `None`.
pub fn open_executable(path: &Path, terminal: &str) -> Option<Result<()>> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let kind = launcher(path)?;
        let dir = path.parent().filter(|d| d.is_dir());
        match kind {
            Launcher::Program => Some(spawn_program_argv(
                &[path.to_string_lossy().into_owned()],
                path,
                dir,
            )),
            Launcher::Desktop => {
                if let Some(entry) = xdg::DesktopEntry::read(path) {
                    return Some(launch_desktop_entry(&entry, &[], terminal));
                }
                if !crate::util::cmdline::which("gio") {
                    return None;
                }
                let argv = vec![
                    "gio".to_string(),
                    "launch".to_string(),
                    path.to_string_lossy().into_owned(),
                ];
                Some(spawn_program_argv(&argv, path, dir))
            }
            Launcher::Script => match crate::util::terminal::run_script(path, terminal) {
                Some(r) => Some(r),
                // No terminal installed: run it without one.
                None => Some(spawn_program_argv(
                    &[path.to_string_lossy().into_owned()],
                    path,
                    dir,
                )),
            },
        }
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = (path, terminal);
        None
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_program_argv(argv: &[String], path: &Path, dir: Option<&Path>) -> Result<()> {
    // In a Flatpak sandbox the program lives on the host: start it there (see `hostspawn`).
    let sandboxed = crate::util::hostspawn::in_flatpak();
    let wrapped = if sandboxed {
        crate::util::hostspawn::wrap_argv(argv.to_vec(), dir, &[])
    } else {
        crate::util::cmdline::wrap_uwsm(argv.to_vec())
    };
    let (bin, args) = wrapped
        .split_first()
        .ok_or_else(|| CoreError::Other("empty command".into()))?;
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args);
    if let Some(dir) = dir.filter(|_| !sandboxed) {
        cmd.current_dir(dir);
    }
    // When Gall itself runs from an AppImage these describe Gall's image; a started
    // AppImage would take them for its own.
    for var in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
        cmd.env_remove(var);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| CoreError::Other(format!("cannot start '{}': {e}", path.display())))?;
    // Reap it when it exits so no zombie is left behind.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Shows the system's "Open with" dialog. Returns false where there is none.
pub fn system_dialog(path: &Path) -> Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        allow_foreground();
        // OpenAs_RunDLL takes the rest of the command line verbatim: no quotes.
        Command::new("rundll32.exe")
            .raw_arg(format!("shell32.dll,OpenAs_RunDLL {}", path.display()))
            .spawn()
            .map_err(|e| CoreError::Other(format!("cannot show the Open with dialog: {e}")))?;
        Ok(true)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(false)
    }
}

/// Runs a user-typed command template for `path` (`{file}` is accepted for `{path}`).
pub fn open_with_command(path: &Path, command: &str) -> Result<()> {
    if command.trim().is_empty() {
        return Err(CoreError::Other("empty command".into()));
    }
    run_choice(path, &command.replace("{file}", "{path}"))
}

/// Returns true when a desktop entry `Exec=` line accepts multiple files in a single launch (`%F` / `%U`).
pub fn exec_supports_multiple(exec: &str) -> bool {
    split_exec(exec)
        .iter()
        .any(|tok| tok == "%F" || tok == "%U")
}

/// Expands a desktop-entry `Exec` value for zero, one, or many `paths`.
/// `%F`/`%U` expand to all paths in a single argv; `%f`/`%u` (or no file code) produce one argv per path.
pub fn expand_exec_many(exec: &str, paths: &[PathBuf]) -> Vec<Vec<String>> {
    if paths.is_empty() {
        let out = expand_exec_tokens(exec, &[]);
        return if out.is_empty() { vec![] } else { vec![out] };
    }
    if paths.len() == 1 || exec_supports_multiple(exec) {
        let files: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        return vec![expand_exec_tokens(exec, &files)];
    }
    paths.iter().map(|p| expand_exec(exec, p)).collect()
}

/// Splits a desktop-entry `Exec` value into arguments and fills in the file
/// (`%f %F %u %U`); other field codes are dropped, `%%` is a percent sign. Without
/// a file code the file is appended.
pub fn expand_exec(exec: &str, path: &Path) -> Vec<String> {
    expand_exec_tokens(exec, &[path.to_string_lossy().into_owned()])
}

fn expand_exec_tokens(exec: &str, files: &[String]) -> Vec<String> {
    let first = files.first().map(String::as_str).unwrap_or("");
    let mut out = Vec::new();
    let mut used = false;
    for tok in split_exec(exec) {
        match tok.as_str() {
            "%F" | "%U" => {
                out.extend(files.iter().cloned());
                used = true;
            }
            "%f" | "%u" => {
                if !first.is_empty() {
                    out.push(first.to_string());
                }
                used = true;
            }
            "%i" | "%c" | "%k" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
            _ => {
                let mut s = String::new();
                let mut chars = tok.chars().peekable();
                while let Some(c) = chars.next() {
                    if c != '%' {
                        s.push(c);
                        continue;
                    }
                    match chars.next() {
                        Some('%') => s.push('%'),
                        Some('f' | 'F' | 'u' | 'U') => {
                            s.push_str(first);
                            used = true;
                        }
                        _ => {}
                    }
                }
                if !s.is_empty() {
                    out.push(s);
                }
            }
        }
    }
    if !used && !files.is_empty() {
        out.extend(files.iter().cloned());
    }
    out
}

/// Desktop-entry quoting: whitespace separates, double quotes group, `\` escapes
/// inside quotes.
fn split_exec(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut has = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_q = !in_q;
                has = true;
            }
            '\\' if in_q => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !in_q => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Freedesktop registries (pure functions over a set of directories, so they are
/// testable on any OS).
pub mod xdg {
    use super::*;
    use std::collections::HashSet;

    pub struct Dirs {
        /// `$XDG_DATA_HOME` first, then `$XDG_DATA_DIRS` (plus Flatpak/Snap export dirs if present).
        pub data: Vec<PathBuf>,
        /// `$XDG_CONFIG_HOME` first, then `$XDG_CONFIG_DIRS`.
        pub config: Vec<PathBuf>,
    }

    /// Lower-cased desktop names from `$XDG_CURRENT_DESKTOP` (e.g. `Hyprland` -> `["hyprland"]`).
    pub fn desktops_from_env() -> Vec<String> {
        parse_desktops(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
    }

    pub fn parse_desktops(raw: &str) -> Vec<String> {
        let mut out = Vec::new();
        for part in raw.split(':').map(str::trim).filter(|s| !s.is_empty()) {
            let lower = part.to_ascii_lowercase();
            if !out.contains(&lower) {
                out.push(lower);
            }
        }
        out
    }

    impl Dirs {
        pub fn from_env() -> Self {
            let home = dirs::home_dir().unwrap_or_default();
            let list = |var: &str, home_var: &str, home_default: PathBuf, default: &str| {
                let mut v = vec![std::env::var_os(home_var)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or(home_default)];
                let rest = std::env::var("XDG_".to_string() + var)
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| default.into());
                for p in rest.split(':').filter(|s| !s.is_empty()).map(PathBuf::from) {
                    if !v.contains(&p) {
                        v.push(p);
                    }
                }
                v
            };
            let mut data = list(
                "DATA_DIRS",
                "XDG_DATA_HOME",
                home.join(".local/share"),
                "/usr/local/share:/usr/share",
            );
            // Flatpak and Snap export `.desktop` files here even when a compositor session
            // did not add them to `$XDG_DATA_DIRS`.
            for extra in [
                home.join(".local/share/flatpak/exports/share"),
                PathBuf::from("/var/lib/flatpak/exports/share"),
                PathBuf::from("/var/lib/snapd/desktop"),
            ] {
                if extra.is_dir() && !data.contains(&extra) {
                    data.push(extra);
                }
            }
            Dirs {
                data,
                config: list(
                    "CONFIG_DIRS",
                    "XDG_CONFIG_HOME",
                    home.join(".config"),
                    "/etc/xdg",
                ),
            }
        }

        /// Finds a `.desktop` file by id, supporting vendor subdirectories
        /// (`foo-bar.desktop` -> `applications/foo/bar.desktop`) per the Desktop Entry Spec.
        pub fn find_desktop(&self, id: &str) -> Option<PathBuf> {
            if id.is_empty() || id.contains("..") || id.starts_with('/') || id.contains('\\') {
                return None;
            }
            for d in &self.data {
                let apps = d.join("applications");
                let direct = apps.join(id);
                if direct.is_file() {
                    return Some(direct);
                }
                // Try replacing `-` with `/` for vendor-prefixed ids (e.g. `kde4-kate.desktop`).
                if id.contains('-') {
                    let mut prefix = 0;
                    while let Some(rel) = id[prefix..].find('-') {
                        let split = prefix + rel;
                        let candidate = apps.join(&id[..split]).join(&id[split + 1..]);
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                        prefix = split + 1;
                    }
                    let nested = apps.join(id.replace('-', "/"));
                    if nested.is_file() {
                        return Some(nested);
                    }
                }
            }
            None
        }
    }

    pub struct DesktopEntry {
        pub name: String,
        pub exec: String,
        /// True when `NoDisplay=true` or `Hidden=true` (omitted from application menus/lists).
        pub hidden: bool,
        /// True when `Hidden=true` (deleted/disabled; unusable even as a MIME default).
        pub deleted: bool,
        pub terminal: bool,
        /// `Type=` (`Application` when omitted).
        pub entry_type: String,
        /// `Path=` (optional working directory).
        pub path: Option<String>,
        /// `MimeType=` (types the program says it can open).
        pub mime_types: Vec<String>,
        /// `TryExec=`: the entry is unusable when this program is not installed.
        pub try_exec: Option<String>,
    }

    impl DesktopEntry {
        /// False when `TryExec` names a program that is not installed (the entry is then ignored).
        pub fn installed(&self) -> bool {
            if self.entry_type != "Application" {
                return false;
            }
            match self
                .try_exec
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                None => true,
                Some(t) if Path::new(t).is_absolute() => crate::util::cmdline::resolve(t).is_some(),
                Some(t) => crate::util::cmdline::which(t),
            }
        }

        pub fn read(file: &Path) -> Option<Self> {
            Self::parse(&std::fs::read_to_string(file).ok()?)
        }

        /// The `[Desktop Entry]` group: Name (localised for $LANG when present), Exec,
        /// NoDisplay / Hidden, Terminal, Path, Type, MimeType, TryExec.
        pub fn parse(text: &str) -> Option<Self> {
            let lang = std::env::var("LANG").unwrap_or_default();
            let lang = lang.split(['.', '@']).next().unwrap_or("");
            let short = lang.split('_').next().unwrap_or("");
            let (mut name, mut local, mut exec) = (None, None, None);
            let (mut no_display, mut deleted, mut terminal) = (false, false, false);
            let mut entry_type = String::from("Application");
            let mut path: Option<String> = None;
            let mut mime_types: Vec<String> = Vec::new();
            let mut try_exec: Option<String> = None;
            let mut in_group = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_group = line == "[Desktop Entry]";
                    continue;
                }
                let Some((k, v)) = line.split_once('=') else {
                    continue;
                };
                if !in_group {
                    continue;
                }
                match k.trim() {
                    "Name" => name = Some(v.trim().to_string()),
                    "Exec" => exec = Some(v.trim().to_string()),
                    "Type" => entry_type = v.trim().to_string(),
                    "Path" if !v.trim().is_empty() => path = Some(v.trim().to_string()),
                    "NoDisplay" if v.trim().eq_ignore_ascii_case("true") => no_display = true,
                    "Hidden" if v.trim().eq_ignore_ascii_case("true") => deleted = true,
                    "Terminal" if v.trim().eq_ignore_ascii_case("true") => terminal = true,
                    "TryExec" => try_exec = Some(v.trim().to_string()),
                    "MimeType" => {
                        mime_types = v
                            .split(';')
                            .map(str::trim)
                            .filter(|m| !m.is_empty())
                            .map(String::from)
                            .collect();
                    }
                    k if !lang.is_empty()
                        && (k == format!("Name[{lang}]") || k == format!("Name[{short}]")) =>
                    {
                        local = Some(v.trim().to_string());
                    }
                    _ => {}
                }
            }
            Some(DesktopEntry {
                name: local.or(name)?,
                exec: exec?,
                hidden: no_display || deleted,
                deleted,
                terminal,
                entry_type,
                path,
                mime_types,
                try_exec,
            })
        }
    }

    /// Simple glob matching (`*`, `?`, `[abc]`/`[a-z]`) for shared-mime-info `globs2` patterns.
    fn glob_match(pattern: &str, text: &str) -> bool {
        let p: Vec<char> = pattern.chars().collect();
        let t: Vec<char> = text.chars().collect();
        fn rec(p: &[char], t: &[char]) -> bool {
            match (p.first(), t.first()) {
                (None, None) => true,
                (Some('*'), _) => rec(&p[1..], t) || (!t.is_empty() && rec(p, &t[1..])),
                (Some('?'), Some(_)) => rec(&p[1..], &t[1..]),
                (Some('['), Some(&tc)) => {
                    let Some(end) = p[1..].iter().position(|&c| c == ']') else {
                        return tc == '[' && rec(&p[1..], &t[1..]);
                    };
                    let class = &p[1..1 + end];
                    let (neg, body) = match class.first() {
                        Some('!' | '^') => (true, &class[1..]),
                        _ => (false, class),
                    };
                    let mut matched = false;
                    let mut i = 0;
                    while i < body.len() {
                        if i + 2 < body.len() && body[i + 1] == '-' {
                            if tc >= body[i] && tc <= body[i + 2] {
                                matched = true;
                            }
                            i += 3;
                        } else {
                            if tc == body[i] {
                                matched = true;
                            }
                            i += 1;
                        }
                    }
                    (matched != neg) && rec(&p[2 + end..], &t[1..])
                }
                (Some(&pc), Some(&tc)) if pc == tc => rec(&p[1..], &t[1..]),
                _ => false,
            }
        }
        rec(&p, &t)
    }

    /// Reads `mime/aliases` (`alias canonical`) across `dirs.data`.
    pub fn read_aliases(dirs: &Dirs) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for d in &dirs.data {
            let Ok(text) = std::fs::read_to_string(d.join("mime/aliases")) else {
                continue;
            };
            for line in text.lines() {
                if let Some((alias, canon)) = line.trim().split_once(' ') {
                    let (alias, canon) = (alias.trim(), canon.trim());
                    if !alias.is_empty() && !canon.is_empty() {
                        map.entry(alias.to_string())
                            .or_insert_with(|| canon.to_string());
                    }
                }
            }
        }
        map
    }

    /// Canonical MIME type plus all aliases that map to it, so entries registered under an
    /// alias (such as `text/x-markdown` for `text/markdown`) are always matched.
    fn mime_with_aliases(mime: &str, aliases: &HashMap<String, String>) -> Vec<String> {
        let canon = aliases
            .get(mime)
            .map(String::as_str)
            .unwrap_or(mime)
            .to_string();
        let mut out = vec![canon.clone()];
        if mime != canon {
            out.push(mime.to_string());
        }
        for (a, c) in aliases {
            if c == &canon && !out.contains(a) {
                out.push(a.clone());
            }
        }
        out
    }

    /// Content-based MIME sniffing for files without a matching glob (extensionless files,
    /// scripts, text notes, or common binary formats).
    pub fn sniff_content_mime(path: &Path) -> Option<String> {
        use std::io::Read;
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        if meta.len() == 0 {
            return Some("text/plain".into());
        }
        let mut buf = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(4096)
            .read_to_end(&mut buf)
            .ok()?;
        sniff_bytes_mime(&buf)
    }

    /// Pure byte-header MIME sniffer (unit-tested directly).
    pub fn sniff_bytes_mime(buf: &[u8]) -> Option<String> {
        if buf.is_empty() {
            return Some("text/plain".into());
        }
        if buf.starts_with(b"%PDF-") {
            return Some("application/pdf".into());
        }
        if buf.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some("image/png".into());
        }
        if buf.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some("image/jpeg".into());
        }
        if buf.starts_with(b"GIF87a") || buf.starts_with(b"GIF89a") {
            return Some("image/gif".into());
        }
        if buf.len() >= 12 && &buf[0..4] == b"RIFF" {
            match &buf[8..12] {
                b"WEBP" => return Some("image/webp".into()),
                b"WAVE" => return Some("audio/wav".into()),
                b"AVI " => return Some("video/x-msvideo".into()),
                _ => {}
            }
        }
        if buf.starts_with(b"PK\x03\x04") {
            return Some("application/zip".into());
        }
        if buf.starts_with(&[0x1F, 0x8B]) {
            return Some("application/gzip".into());
        }
        if buf.starts_with(b"BZh") {
            return Some("application/x-bzip2".into());
        }
        if buf.starts_with(b"\xFD7zXZ\x00") {
            return Some("application/x-xz".into());
        }
        if buf.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
            return Some("application/zstd".into());
        }
        if buf.starts_with(b"7z\xBC\xAF\x27\x1C") {
            return Some("application/x-7z-compressed".into());
        }
        if buf.starts_with(b"SQLite format 3\0") {
            return Some("application/vnd.sqlite3".into());
        }
        if buf.starts_with(b"ID3")
            || (buf.len() >= 2 && buf[0] == 0xFF && matches!(buf[1], 0xFB | 0xF3 | 0xF2))
        {
            return Some("audio/mpeg".into());
        }
        if buf.starts_with(b"OggS") {
            return Some("application/ogg".into());
        }
        if buf.starts_with(b"fLaC") {
            return Some("audio/flac".into());
        }
        if buf.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            return Some("video/x-matroska".into());
        }
        if buf.len() >= 12 && &buf[4..8] == b"ftyp" {
            return Some("video/mp4".into());
        }
        if buf.starts_with(b"#!") {
            let line = buf
                .split(|&b| b == b'\n' || b == b'\r')
                .next()
                .and_then(|l| std::str::from_utf8(l).ok())
                .unwrap_or("");
            let lower = line.to_ascii_lowercase();
            if lower.contains("python") {
                return Some("text/x-python".into());
            }
            if lower.contains("bash")
                || lower.contains("/sh")
                || lower.ends_with(" sh")
                || lower.contains("zsh")
                || lower.contains("dash")
                || lower.contains("fish")
            {
                return Some("application/x-shellscript".into());
            }
            if lower.contains("node") || lower.contains("deno") || lower.contains("bun") {
                return Some("text/javascript".into());
            }
            if lower.contains("ruby") {
                return Some("application/x-ruby".into());
            }
            if lower.contains("perl") {
                return Some("application/x-perl".into());
            }
            return Some("text/plain".into());
        }
        // Check UTF-8 / printable text (Shared MIME-info spec fallback to text/plain).
        if buf.contains(&0) {
            return None;
        }
        let text = match std::str::from_utf8(buf) {
            Ok(s) => s,
            Err(e) if e.valid_up_to() > 0 && buf.len() - e.valid_up_to() < 4 => {
                // Truncated multi-byte UTF-8 sequence right at the 4 KiB read boundary.
                std::str::from_utf8(&buf[..e.valid_up_to()]).ok()?
            }
            Err(_) => return None,
        };
        let has_binary_ctrl = text
            .bytes()
            .any(|b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B));
        if has_binary_ctrl {
            return None;
        }
        let trimmed = text.trim_start_matches([' ', '\t', '\r', '\n', '\u{feff}']);
        let head_lower = trimmed
            .chars()
            .take(64)
            .collect::<String>()
            .to_ascii_lowercase();
        if head_lower.starts_with("<!doctype html") || head_lower.starts_with("<html") {
            return Some("text/html".into());
        }
        if head_lower.starts_with("<svg")
            || (head_lower.starts_with("<?xml") && text.contains("<svg"))
        {
            return Some("image/svg+xml".into());
        }
        if head_lower.starts_with("<?xml") {
            return Some("application/xml".into());
        }
        Some("text/plain".into())
    }

    /// MIME type of `path` from shared-mime-info `globs2` (`weight:type:glob[:flags]`),
    /// matching suffix globs (`*.md`, `*.tar.gz`), exact filenames (`Makefile`, `README`),
    /// wildcard globs, aliases, and content sniffing when no glob matches.
    pub fn mime_type(path: &Path, dirs: &Dirs) -> Option<String> {
        if path.is_dir() {
            return Some("inode/directory".into());
        }
        let raw_name = path.file_name()?.to_string_lossy();
        let lower_name = raw_name.to_lowercase();
        let aliases = read_aliases(dirs);
        // Score: (weight, exact_literal, pattern_len)
        let mut best: Option<(u32, bool, usize, String)> = None;
        for d in &dirs.data {
            let Ok(text) = std::fs::read_to_string(d.join("mime/globs2")) else {
                continue;
            };
            for line in text.lines().filter(|l| !l.starts_with('#')) {
                let mut parts = line.splitn(4, ':');
                let (Some(w), Some(t), Some(g)) = (parts.next(), parts.next(), parts.next()) else {
                    continue;
                };
                let flags = parts.next().unwrap_or("");
                let cs = flags.split(',').any(|f| f == "cs");
                let has_wild = g.contains(['*', '?', '[']);
                let matched = if !has_wild {
                    if cs {
                        raw_name == g
                    } else {
                        lower_name == g.to_lowercase()
                    }
                } else if let Some(suffix) = g.strip_prefix('*') {
                    if suffix.contains(['*', '?', '[']) {
                        if cs {
                            glob_match(g, &raw_name)
                        } else {
                            glob_match(&g.to_lowercase(), &lower_name)
                        }
                    } else if cs {
                        raw_name.ends_with(suffix)
                    } else {
                        lower_name.ends_with(&suffix.to_lowercase())
                    }
                } else if cs {
                    glob_match(g, &raw_name)
                } else {
                    glob_match(&g.to_lowercase(), &lower_name)
                };
                if !matched {
                    continue;
                }
                let w: u32 = w.parse().unwrap_or(50);
                let exact = !has_wild;
                let plen = g.trim_start_matches('*').len();
                if best
                    .as_ref()
                    .is_none_or(|(bw, be, bl, _)| (w, exact, plen) > (*bw, *be, *bl))
                {
                    best = Some((w, exact, plen, t.to_string()));
                }
            }
        }
        let mime = best.map(|b| b.3).or_else(|| sniff_content_mime(path))?;
        Some(aliases.get(&mime).cloned().unwrap_or(mime))
    }

    /// `type` and its parent types (`mime/subclasses`: "child parent"), e.g.
    /// text/x-python → text/plain, so plain text editors are offered for code too.
    pub fn with_parents(mime: &str, dirs: &Dirs) -> Vec<String> {
        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        for d in &dirs.data {
            if let Ok(text) = std::fs::read_to_string(d.join("mime/subclasses")) {
                for line in text.lines() {
                    if let Some((c, p)) = line.split_once(' ') {
                        parents
                            .entry(c.to_string())
                            .or_default()
                            .push(p.trim().to_string());
                    }
                }
            }
        }
        let mut out = vec![mime.to_string()];
        let mut i = 0;
        while i < out.len() && out.len() < 12 {
            for p in parents.get(&out[i]).cloned().unwrap_or_default() {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
            i += 1;
        }
        out
    }

    /// Entries of `[group]` in an ini-style file: `type=a.desktop;b.desktop;`.
    pub fn ini_group(text: &str, group: &str) -> Vec<(String, Vec<String>)> {
        let mut out = Vec::new();
        let mut in_group = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_group = line == format!("[{group}]");
                continue;
            }
            if let (true, Some((k, v))) = (in_group, line.split_once('=')) {
                let ids = v
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                out.push((k.trim().to_string(), ids));
            }
        }
        out
    }

    /// `mimeapps.list` files in Freedesktop MIME Apps 1.0.1 precedence order.
    fn read_mimeapps_lists(dirs: &Dirs, desktops: &[String]) -> Vec<String> {
        let mut paths = Vec::new();
        for c in &dirs.config {
            for de in desktops {
                paths.push(c.join(format!("{de}-mimeapps.list")));
            }
            paths.push(c.join("mimeapps.list"));
        }
        for d in &dirs.data {
            let apps = d.join("applications");
            for de in desktops {
                paths.push(apps.join(format!("{de}-mimeapps.list")));
            }
            paths.push(apps.join("mimeapps.list"));
        }
        paths
            .into_iter()
            .filter_map(|f| std::fs::read_to_string(f).ok())
            .collect()
    }

    /// Collects desktop file ids from uncached `applications/` directories matching any of `keys`.
    fn uncached_ids_for_mimes(dirs: &Dirs, keys: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for (i, d) in dirs.data.iter().enumerate() {
            let apps = d.join("applications");
            if i > 0 && apps.join("mimeinfo.cache").is_file() {
                continue;
            }
            for (id, file) in scan_desktop_files(&apps) {
                if out.contains(&id) {
                    continue;
                }
                if dirs.find_desktop(&id).as_deref() != Some(file.as_path()) {
                    continue;
                }
                let Some(e) = DesktopEntry::read(&file) else {
                    continue;
                };
                if e.hidden || !e.installed() || !e.mime_types.iter().any(|m| keys.contains(m)) {
                    continue;
                }
                out.push(id);
            }
        }
        out
    }

    /// Scans `applications/` (and one level of vendor subdirectories) for `*.desktop` files,
    /// returning `(desktop_id, path)` sorted by id.
    fn scan_desktop_files(apps_dir: &Path) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(apps_dir) else {
            return out;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().is_some_and(|x| x == "desktop") {
                if let Some(name) = p.file_name().map(|n| n.to_string_lossy().into_owned()) {
                    out.push((name, p));
                }
            } else if p.is_dir() {
                let Some(prefix) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                    continue;
                };
                if let Ok(sub) = std::fs::read_dir(&p) {
                    for sub_entry in sub.flatten() {
                        let sp = sub_entry.path();
                        if sp.is_file() && sp.extension().is_some_and(|x| x == "desktop") {
                            if let Some(name) =
                                sp.file_name().map(|n| n.to_string_lossy().into_owned())
                            {
                                out.push((format!("{prefix}-{name}"), sp));
                            }
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Resolves the effective Freedesktop default `(desktop_id, DesktopEntry)` for `path`,
    /// following the same precedence as GLib/GIO and Nautilus:
    /// for each MIME type from most specific to parent subclasses, check
    /// `[Default Applications]` -> `[Added Associations]` -> `mimeinfo.cache` -> uncached `.desktop`.
    pub fn default_entry_for_desktops(
        path: &Path,
        dirs: &Dirs,
        desktops: &[String],
    ) -> Option<(String, DesktopEntry)> {
        let mime = mime_type(path, dirs)?;
        let aliases = read_aliases(dirs);
        let types = with_parents(&mime, dirs);
        let lists = read_mimeapps_lists(dirs, desktops);
        let caches: Vec<String> = dirs
            .data
            .iter()
            .filter_map(|d| std::fs::read_to_string(d.join("applications/mimeinfo.cache")).ok())
            .collect();

        for t in &types {
            let keys = mime_with_aliases(t, &aliases);
            let pick_group = |group: &str| -> Vec<String> {
                let mut v = Vec::new();
                for text in &lists {
                    for (k, ids) in ini_group(text, group) {
                        if keys.contains(&k) {
                            v.extend(ids);
                        }
                    }
                }
                v
            };
            // 1. Explicit [Default Applications]: NoDisplay=true is still valid as a default handler;
            //    only Hidden=true (`deleted`) or missing `TryExec` disqualifies it.
            for id in pick_group("Default Applications") {
                if let Some(e) = dirs.find_desktop(&id).and_then(|f| DesktopEntry::read(&f)) {
                    if !e.deleted && e.installed() {
                        return Some((id, e));
                    }
                }
            }
            let removed = pick_group("Removed Associations");
            // 2. [Added Associations]
            for id in pick_group("Added Associations") {
                if removed.contains(&id) {
                    continue;
                }
                if let Some(e) = dirs.find_desktop(&id).and_then(|f| DesktopEntry::read(&f)) {
                    if !e.deleted && e.installed() {
                        return Some((id, e));
                    }
                }
            }
            // 3. mimeinfo.cache for this exact MIME type (before falling back to parent types)
            for text in &caches {
                for (k, ids) in ini_group(text, "MIME Cache") {
                    if !keys.contains(&k) {
                        continue;
                    }
                    for id in ids {
                        if removed.contains(&id) {
                            continue;
                        }
                        if let Some(e) = dirs.find_desktop(&id).and_then(|f| DesktopEntry::read(&f))
                        {
                            if !e.deleted && !e.hidden && e.installed() {
                                return Some((id, e));
                            }
                        }
                    }
                }
            }
            // 4. Uncached `.desktop` files declaring this MIME type
            for id in uncached_ids_for_mimes(dirs, &keys) {
                if removed.contains(&id) {
                    continue;
                }
                if let Some(e) = dirs.find_desktop(&id).and_then(|f| DesktopEntry::read(&f)) {
                    if !e.deleted && !e.hidden && e.installed() {
                        return Some((id, e));
                    }
                }
            }
        }
        None
    }

    pub fn apps_for(path: &Path, dirs: &Dirs) -> Vec<AppChoice> {
        apps_for_desktops(path, dirs, &[])
    }

    pub fn apps_for_desktops(path: &Path, dirs: &Dirs, desktops: &[String]) -> Vec<AppChoice> {
        let Some(mime) = mime_type(path, dirs) else {
            return Vec::new();
        };
        let aliases = read_aliases(dirs);
        let types = with_parents(&mime, dirs);
        let all_keys: Vec<String> = types
            .iter()
            .flat_map(|t| mime_with_aliases(t, &aliases))
            .collect();
        let lists = read_mimeapps_lists(dirs, desktops);
        let pick_for = |group: &str, keys: &[String]| -> Vec<String> {
            let mut v = Vec::new();
            for text in &lists {
                for (k, ids) in ini_group(text, group) {
                    if keys.contains(&k) {
                        v.extend(ids);
                    }
                }
            }
            v
        };
        let default_id = default_entry_for_desktops(path, dirs, desktops).map(|(id, _)| id);
        let primary_keys = mime_with_aliases(&mime, &aliases);
        let removed = pick_for("Removed Associations", &primary_keys);
        let mut ids: Vec<String> = default_id.iter().cloned().collect();
        for t in &types {
            let keys = mime_with_aliases(t, &aliases);
            ids.extend(pick_for("Default Applications", &keys));
            ids.extend(pick_for("Added Associations", &keys));
        }
        for t in &types {
            let keys = mime_with_aliases(t, &aliases);
            for d in &dirs.data {
                if let Ok(text) = std::fs::read_to_string(d.join("applications/mimeinfo.cache")) {
                    for (k, v) in ini_group(&text, "MIME Cache") {
                        if keys.contains(&k) {
                            ids.extend(v);
                        }
                    }
                }
            }
        }
        ids.extend(uncached_ids_for_mimes(dirs, &all_keys));
        let mut out: Vec<AppChoice> = Vec::new();
        for id in ids {
            if removed.contains(&id) || out.iter().any(|a| a.id == id) {
                continue;
            }
            let Some(e) = dirs.find_desktop(&id).and_then(|f| DesktopEntry::read(&f)) else {
                continue;
            };
            if e.hidden || !e.installed() {
                continue;
            }
            out.push(AppChoice {
                default: default_id.as_deref() == Some(id.as_str()),
                command: format!("desktop:{id}"),
                id,
                name: e.name,
            });
        }
        out
    }

    /// All visible, installed `Type=Application` desktop entries across `dirs.data`,
    /// deduplicated by id and sorted alphabetically by display name.
    pub fn all_apps(dirs: &Dirs) -> Vec<AppChoice> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for d in &dirs.data {
            let apps = d.join("applications");
            for (id, file) in scan_desktop_files(&apps) {
                if !seen.insert(id.clone()) {
                    continue;
                }
                let Some(e) = DesktopEntry::read(&file) else {
                    continue;
                };
                if e.hidden || !e.installed() {
                    continue;
                }
                out.push(AppChoice {
                    default: false,
                    command: format!("desktop:{id}"),
                    id,
                    name: e.name,
                });
            }
        }
        out.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }

    /// Sets `mime=desktop_id;` under `[Default Applications]` in `mimeapps_path` (e.g.
    /// `~/.config/mimeapps.list`), preserving other sections and comments, and removes
    /// `desktop_id` from `[Removed Associations]` for `mime` if present.
    pub fn set_default_in_file(mimeapps_path: &Path, mime: &str, desktop_id: &str) -> Result<()> {
        let existing = std::fs::read_to_string(mimeapps_path).unwrap_or_default();
        let updated = update_mimeapps_text(&existing, mime, desktop_id);
        if let Some(parent) = mimeapps_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CoreError::Other(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let tmp = mimeapps_path.with_extension("list.gall-tmp");
        if std::fs::write(&tmp, &updated).is_ok() && std::fs::rename(&tmp, mimeapps_path).is_ok() {
            return Ok(());
        }
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(mimeapps_path, updated)
            .map_err(|e| CoreError::Other(format!("cannot write {}: {e}", mimeapps_path.display())))
    }

    pub fn update_mimeapps_text(text: &str, mime: &str, desktop_id: &str) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut group = String::new();
        let mut found_default_section = false;
        let mut replaced_default = false;
        let new_entry = format!("{mime}={desktop_id};");

        for raw in text.lines() {
            let trimmed = raw.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                if group == "[Default Applications]" && !replaced_default {
                    lines.push(new_entry.clone());
                    replaced_default = true;
                }
                group = trimmed.to_string();
                if group == "[Default Applications]" {
                    found_default_section = true;
                }
                lines.push(raw.to_string());
                continue;
            }
            if group == "[Default Applications]" {
                if let Some((k, _)) = trimmed.split_once('=') {
                    if k.trim() == mime {
                        if !replaced_default {
                            lines.push(new_entry.clone());
                            replaced_default = true;
                        }
                        continue;
                    }
                }
            } else if group == "[Removed Associations]" {
                if let Some((k, v)) = trimmed.split_once('=') {
                    if k.trim() == mime {
                        let kept: Vec<&str> = v
                            .split(';')
                            .map(str::trim)
                            .filter(|s| !s.is_empty() && *s != desktop_id)
                            .collect();
                        if !kept.is_empty() {
                            lines.push(format!("{mime}={};", kept.join(";")));
                        }
                        continue;
                    }
                }
            }
            lines.push(raw.to_string());
        }

        if !replaced_default {
            if !found_default_section {
                if !lines.is_empty() && !lines.last().is_some_and(|l| l.is_empty()) {
                    lines.push(String::new());
                }
                lines.push("[Default Applications]".into());
            }
            lines.push(new_entry);
        }

        let mut out = lines.join("\n");
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

/// macOS `.app` bundle parsing (pure XML Info.plist helpers tested on all OSes) and
/// native LaunchServices app enumeration on macOS.
pub mod macos_bundle {
    use super::AppChoice;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct BundleInfo {
        pub name: String,
        pub bundle_id: Option<String>,
        pub background_only: bool,
        pub extensions: Vec<String>,
        pub utis: Vec<String>,
    }

    /// Parses an XML `Info.plist` document into [`BundleInfo`].
    pub fn parse_info_plist(xml: &str) -> Option<BundleInfo> {
        let doc = roxmltree::Document::parse(xml).ok()?;
        let root_dict = doc
            .root_element()
            .children()
            .find(|n| n.is_element() && n.has_tag_name("dict"))?;
        let map = plist_dict_entries(root_dict);
        let display_name = map
            .get("CFBundleDisplayName")
            .and_then(|n| plist_string(*n))
            .filter(|s| !s.is_empty());
        let bundle_name = map
            .get("CFBundleName")
            .and_then(|n| plist_string(*n))
            .filter(|s| !s.is_empty());
        let name = display_name.or(bundle_name)?;
        let bundle_id = map
            .get("CFBundleIdentifier")
            .and_then(|n| plist_string(*n))
            .filter(|s| !s.is_empty());
        let background_only = map.get("LSBackgroundOnly").is_some_and(|n| plist_bool(*n))
            || map.get("LSUIElement").is_some_and(|n| plist_bool(*n));

        let mut extensions = Vec::new();
        let mut utis = Vec::new();
        if let Some(types_node) = map.get("CFBundleDocumentTypes") {
            if types_node.has_tag_name("array") {
                for item in types_node
                    .children()
                    .filter(|n| n.is_element() && n.has_tag_name("dict"))
                {
                    let d = plist_dict_entries(item);
                    if let Some(role) = d.get("CFBundleTypeRole").and_then(|n| plist_string(*n)) {
                        if role.eq_ignore_ascii_case("None") {
                            continue;
                        }
                    }
                    if let Some(arr) = d.get("CFBundleTypeExtensions") {
                        for ext in plist_string_array(*arr) {
                            let clean = ext.trim().trim_start_matches('.').to_ascii_lowercase();
                            if !clean.is_empty() && !extensions.contains(&clean) {
                                extensions.push(clean);
                            }
                        }
                    }
                    if let Some(arr) = d.get("LSItemContentTypes") {
                        for uti in plist_string_array(*arr) {
                            let clean = uti.trim().to_ascii_lowercase();
                            if !clean.is_empty() && !utis.contains(&clean) {
                                utis.push(clean);
                            }
                        }
                    }
                }
            }
        }
        Some(BundleInfo {
            name,
            bundle_id,
            background_only,
            extensions,
            utis,
        })
    }

    /// True if `bundle` declares support for files with extension `ext` (lowercased, no dot).
    pub fn matches_extension(bundle: &BundleInfo, ext: &str) -> bool {
        let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
        if bundle.extensions.iter().any(|e| e == &ext || e == "*") {
            return true;
        }
        let target_utis = utis_for_extension(&ext);
        bundle
            .utis
            .iter()
            .any(|u| target_utis.contains(&u.as_str()))
    }

    /// Maps common file extensions to macOS Uniform Type Identifiers (plus their supertypes).
    pub fn utis_for_extension(ext: &str) -> &'static [&'static str] {
        match ext {
            "md" | "markdown" | "mdown" => &[
                "net.daringfireball.markdown",
                "public.plain-text",
                "public.text",
                "public.data",
                "public.item",
            ],
            "txt" | "text" | "log" | "csv" | "tsv" | "ini" | "cfg" | "conf" | "toml" | "yaml"
            | "yml" | "env" => &[
                "public.plain-text",
                "public.text",
                "public.data",
                "public.item",
            ],
            "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "c" | "h" | "cpp" | "cc" | "hpp"
            | "java" | "kt" | "swift" | "rb" | "sh" | "zsh" | "bash" | "css" | "scss" | "sql"
            | "lua" => &[
                "public.source-code",
                "public.plain-text",
                "public.text",
                "public.data",
                "public.item",
            ],
            "json" => &[
                "public.json",
                "public.plain-text",
                "public.text",
                "public.data",
                "public.item",
            ],
            "xml" => &[
                "public.xml",
                "public.plain-text",
                "public.text",
                "public.data",
                "public.item",
            ],
            "html" | "htm" => &["public.html", "public.text", "public.data", "public.item"],
            "pdf" => &["com.adobe.pdf", "public.data", "public.item"],
            "png" => &["public.png", "public.image", "public.data", "public.item"],
            "jpg" | "jpeg" => &["public.jpeg", "public.image", "public.data", "public.item"],
            "gif" => &[
                "com.compuserve.gif",
                "public.image",
                "public.data",
                "public.item",
            ],
            "webp" => &[
                "org.webmproject.webp",
                "public.image",
                "public.data",
                "public.item",
            ],
            "svg" => &[
                "public.svg-image",
                "public.image",
                "public.xml",
                "public.data",
                "public.item",
            ],
            "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" => &[
                "public.movie",
                "public.mpeg-4",
                "public.audiovisual-content",
                "public.data",
                "public.item",
            ],
            "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => {
                &["public.audio", "public.mp3", "public.data", "public.item"]
            }
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" => &[
                "public.archive",
                "public.zip-archive",
                "public.data",
                "public.item",
            ],
            _ => &["public.data", "public.item"],
        }
    }

    pub fn app_choice(bundle_path: &std::path::Path, name: String, default: bool) -> AppChoice {
        let p = bundle_path.to_string_lossy().into_owned();
        AppChoice {
            id: p.clone(),
            name,
            default,
            command: format!("macos:{p}"),
        }
    }

    fn plist_dict_entries<'a, 'input>(
        dict: roxmltree::Node<'a, 'input>,
    ) -> std::collections::HashMap<&'a str, roxmltree::Node<'a, 'input>> {
        let mut out = std::collections::HashMap::new();
        let mut elems = dict.children().filter(|n| n.is_element());
        while let Some(k) = elems.next() {
            if k.has_tag_name("key") {
                if let (Some(name), Some(val)) = (k.text(), elems.next()) {
                    out.insert(name.trim(), val);
                }
            }
        }
        out
    }

    fn plist_string(node: roxmltree::Node<'_, '_>) -> Option<String> {
        node.has_tag_name("string")
            .then(|| node.text().unwrap_or("").trim().to_string())
    }

    fn plist_bool(node: roxmltree::Node<'_, '_>) -> bool {
        if node.has_tag_name("true") {
            return true;
        }
        if node.has_tag_name("string") {
            return node
                .text()
                .is_some_and(|t| matches!(t.trim(), "1" | "true" | "TRUE" | "YES"));
        }
        false
    }

    fn plist_string_array(node: roxmltree::Node<'_, '_>) -> Vec<String> {
        if !node.has_tag_name("array") {
            return Vec::new();
        }
        node.children()
            .filter(|n| n.is_element())
            .filter_map(plist_string)
            .collect()
    }
}

/// macOS LaunchServices + `.app` bundle discovery.
#[cfg(target_os = "macos")]
mod macos {
    use super::macos_bundle::{self, BundleInfo};
    use super::AppChoice;
    use crate::error::{CoreError, Result};
    use std::ffi::{c_void, CStr};
    use std::path::{Path, PathBuf};

    type CFAllocatorRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFURLRef = *const c_void;
    type CFTypeRef = *const c_void;
    type CFIndex = isize;
    type LSRolesMask = u32;
    const K_LS_ROLES_ALL: LSRolesMask = 0xFFFF_FFFF;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_CF_URL_POSIX_PATH_STYLE: CFIndex = 0;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: CFAllocatorRef,
            buffer: *const u8,
            buf_len: CFIndex,
            is_directory: bool,
        ) -> CFURLRef;
        fn CFURLCopyFileSystemPath(an_url: CFURLRef, path_style: CFIndex) -> CFStringRef;
        fn CFStringGetCString(
            the_string: CFStringRef,
            buffer: *mut std::ffi::c_char,
            buffer_size: CFIndex,
            encoding: u32,
        ) -> bool;
        fn CFStringCreateWithBytes(
            alloc: CFAllocatorRef,
            bytes: *const u8,
            num_bytes: CFIndex,
            encoding: u32,
            is_external_representation: bool,
        ) -> CFStringRef;
    }

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn LSCopyDefaultApplicationURLForURL(
            in_url: CFURLRef,
            in_role_mask: LSRolesMask,
            out_error: *mut CFTypeRef,
        ) -> CFURLRef;
        fn LSCopyApplicationURLsForURL(in_url: CFURLRef, in_role_mask: LSRolesMask) -> CFArrayRef;
        fn LSCopyDefaultRoleHandlerForContentType(
            in_content_type: CFStringRef,
            in_role: LSRolesMask,
        ) -> CFStringRef;
        fn UTTypeCreatePreferredIdentifierForTag(
            in_tag_class: CFStringRef,
            in_tag: CFStringRef,
            in_conforming_to_uti: CFStringRef,
        ) -> CFStringRef;
        fn LSSetDefaultRoleHandlerForContentType(
            in_content_type: CFStringRef,
            in_role: LSRolesMask,
            in_handler_bundle_id: CFStringRef,
        ) -> i32;
    }

    fn cfurl_to_path(url: CFURLRef) -> Option<PathBuf> {
        if url.is_null() {
            return None;
        }
        // SAFETY: `url` is a valid non-null CFURLRef.
        let cfstr = unsafe { CFURLCopyFileSystemPath(url, K_CF_URL_POSIX_PATH_STYLE) };
        if cfstr.is_null() {
            return None;
        }
        let mut buf = vec![0i8; 4096];
        // SAFETY: `cfstr` is a valid CFStringRef and `buf` has space for 4096 bytes.
        let ok = unsafe {
            CFStringGetCString(
                cfstr,
                buf.as_mut_ptr(),
                buf.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
            )
        };
        // SAFETY: `cfstr` was created by Copy rule.
        unsafe { CFRelease(cfstr) };
        if !ok {
            return None;
        }
        // SAFETY: `CFStringGetCString` writes a NUL-terminated UTF-8 C string on success.
        let s = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_str().ok()?;
        Some(PathBuf::from(s))
    }

    fn cfstring_to_string(cfstr: CFStringRef) -> Option<String> {
        if cfstr.is_null() {
            return None;
        }
        let mut buf = vec![0i8; 4096];
        // SAFETY: `cfstr` is a valid non-null CFStringRef and `buf` has space for 4096 bytes.
        let ok = unsafe {
            CFStringGetCString(
                cfstr,
                buf.as_mut_ptr(),
                buf.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
            )
        };
        if !ok {
            return None;
        }
        // SAFETY: `CFStringGetCString` writes a NUL-terminated UTF-8 C string on success.
        unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_str()
            .ok()
            .map(str::to_string)
    }

    fn cfstring_from_str(s: &str) -> CFStringRef {
        // SAFETY: valid UTF-8 byte slice.
        unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                s.as_ptr(),
                s.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
                false,
            )
        }
    }

    fn launch_services_apps(path: &Path) -> (Option<PathBuf>, Vec<PathBuf>) {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: valid byte slice for CFURL creation.
        let url = unsafe {
            CFURLCreateFromFileSystemRepresentation(
                std::ptr::null(),
                bytes.as_ptr(),
                bytes.len() as CFIndex,
                path.is_dir(),
            )
        };
        if url.is_null() {
            return (None, Vec::new());
        }
        // SAFETY: `url` is a valid CFURLRef.
        let default_url =
            unsafe { LSCopyDefaultApplicationURLForURL(url, K_LS_ROLES_ALL, std::ptr::null_mut()) };
        let default = if default_url.is_null() {
            None
        } else {
            let p = cfurl_to_path(default_url);
            // SAFETY: created by Copy rule.
            unsafe { CFRelease(default_url) };
            p
        };
        // SAFETY: `url` is a valid CFURLRef.
        let arr = unsafe { LSCopyApplicationURLsForURL(url, K_LS_ROLES_ALL) };
        let mut all = Vec::new();
        if !arr.is_null() {
            // SAFETY: `arr` is a valid CFArrayRef of CFURLRef.
            let count = unsafe { CFArrayGetCount(arr) };
            for i in 0..count {
                let item = unsafe { CFArrayGetValueAtIndex(arr, i) } as CFURLRef;
                if let Some(p) = cfurl_to_path(item) {
                    if !all.contains(&p) {
                        all.push(p);
                    }
                }
            }
            // SAFETY: created by Copy rule.
            unsafe { CFRelease(arr) };
        }
        // SAFETY: `url` was created by Create rule.
        unsafe { CFRelease(url) };
        (default, all)
    }

    fn read_bundle_info(bundle: &Path) -> Option<BundleInfo> {
        let plist = bundle.join("Contents/Info.plist");
        if !plist.is_file() {
            return None;
        }
        let xml = match std::fs::read_to_string(&plist) {
            Ok(s) if s.trim_start().starts_with('<') => s,
            _ => {
                // Binary plist: convert via built-in `plutil`.
                let out = std::process::Command::new("/usr/bin/plutil")
                    .args(["-convert", "xml1", "-o", "-"])
                    .arg(&plist)
                    .output()
                    .ok()?;
                if !out.status.success() {
                    return None;
                }
                String::from_utf8(out.stdout).ok()?
            }
        };
        macos_bundle::parse_info_plist(&xml)
    }

    fn display_name_for_bundle(bundle: &Path) -> String {
        read_bundle_info(bundle).map(|b| b.name).unwrap_or_else(|| {
            bundle
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| bundle.to_string_lossy().into_owned())
        })
    }

    fn scan_app_bundles() -> Vec<PathBuf> {
        let mut roots = vec![
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications"),
            PathBuf::from("/System/Applications/Utilities"),
        ];
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        let mut out = Vec::new();
        for root in roots {
            let Ok(rd) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in rd.flatten() {
                let p = entry.path();
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) && p.is_dir() {
                    if !out.contains(&p) {
                        out.push(p);
                    }
                } else if p.is_dir() {
                    // One level down (e.g. `/Applications/Utilities`, `/Applications/Setapp`).
                    if let Ok(sub) = std::fs::read_dir(&p) {
                        for se in sub.flatten() {
                            let sp = se.path();
                            if sp
                                .extension()
                                .is_some_and(|e| e.eq_ignore_ascii_case("app"))
                                && sp.is_dir()
                                && !out.contains(&sp)
                            {
                                out.push(sp);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    pub fn apps_for(path: &Path) -> Vec<AppChoice> {
        fn push_bundle(out: &mut Vec<AppChoice>, bundle: PathBuf, is_default: bool) {
            let key = bundle.to_string_lossy().into_owned();
            if out.iter().any(|a| a.id == key) {
                return;
            }
            let name = display_name_for_bundle(&bundle);
            out.push(macos_bundle::app_choice(&bundle, name, is_default));
        }
        let (default_path, ls_paths) = launch_services_apps(path);
        let mut out: Vec<AppChoice> = Vec::new();
        if let Some(ref d) = default_path {
            push_bundle(&mut out, d.clone(), true);
        }
        for p in ls_paths {
            let is_def = default_path.as_ref().is_some_and(|d| d == &p);
            if !is_def && read_bundle_info(&p).is_some_and(|info| info.background_only) {
                continue;
            }
            push_bundle(&mut out, p, is_def);
        }
        if out.is_empty() {
            if let Some(ext) = path.extension().map(|e| e.to_string_lossy().to_lowercase()) {
                for bundle in scan_app_bundles() {
                    if let Some(info) = read_bundle_info(&bundle) {
                        if !info.background_only && macos_bundle::matches_extension(&info, &ext) {
                            push_bundle(&mut out, bundle, false);
                        }
                    }
                }
            }
        }
        out
    }

    pub fn all_apps() -> Vec<AppChoice> {
        let mut out: Vec<AppChoice> = Vec::new();
        for bundle in scan_app_bundles() {
            let info = read_bundle_info(&bundle);
            if info.as_ref().is_some_and(|i| i.background_only) {
                continue;
            }
            let name = info
                .map(|i| i.name)
                .unwrap_or_else(|| display_name_for_bundle(&bundle));
            let key = bundle.to_string_lossy().into_owned();
            if !out.iter().any(|a| a.id == key) {
                out.push(macos_bundle::app_choice(&bundle, name, false));
            }
        }
        out.sort_by_key(|a| a.name.to_lowercase());
        out
    }

    pub fn set_system_default(path: &Path, id: &str) -> Result<String> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .ok_or_else(|| CoreError::Other("file has no extension for macOS default".into()))?;
        let bundle = Path::new(id.strip_prefix("macos:").unwrap_or(id));
        let bundle_id = read_bundle_info(bundle)
            .and_then(|i| i.bundle_id)
            .ok_or_else(|| CoreError::Other("application bundle identifier not found".into()))?;
        let tag_class = cfstring_from_str("public.filename-extension");
        let tag = cfstring_from_str(&ext);
        let handler = cfstring_from_str(&bundle_id);
        // SAFETY: valid CFStringRefs.
        let uti =
            unsafe { UTTypeCreatePreferredIdentifierForTag(tag_class, tag, std::ptr::null()) };
        let status = if !uti.is_null() {
            // SAFETY: valid CFStringRefs.
            unsafe { LSSetDefaultRoleHandlerForContentType(uti, K_LS_ROLES_ALL, handler) }
        } else {
            -1
        };
        // SAFETY: release owned CF objects.
        unsafe {
            if !uti.is_null() {
                CFRelease(uti);
            }
            CFRelease(tag_class);
            CFRelease(tag);
            CFRelease(handler);
        }
        if status == 0 {
            Ok(format!(".{ext}"))
        } else {
            Err(CoreError::Other(format!(
                "cannot update macOS default for .{ext} (status {status})"
            )))
        }
    }

    pub fn set_uti_handler(uti: &str, bundle_id: &str) -> i32 {
        let cf_uti = cfstring_from_str(uti);
        let cf_handler = cfstring_from_str(bundle_id);
        // SAFETY: valid CFStringRefs.
        let status =
            unsafe { LSSetDefaultRoleHandlerForContentType(cf_uti, K_LS_ROLES_ALL, cf_handler) };
        // SAFETY: release owned CF objects.
        unsafe {
            CFRelease(cf_uti);
            CFRelease(cf_handler);
        }
        status
    }

    pub fn default_uti_handler(uti: &str) -> Option<String> {
        let cf_uti = cfstring_from_str(uti);
        if cf_uti.is_null() {
            return None;
        }
        // SAFETY: `cf_uti` is a valid CFStringRef.
        let handler = unsafe { LSCopyDefaultRoleHandlerForContentType(cf_uti, K_LS_ROLES_ALL) };
        // SAFETY: `cf_uti` was created by Create rule.
        unsafe { CFRelease(cf_uti) };
        if handler.is_null() {
            return None;
        }
        let bundle_id = cfstring_to_string(handler);
        // SAFETY: `handler` was returned by a Copy function.
        unsafe { CFRelease(handler) };
        bundle_id.filter(|id| !id.trim().is_empty())
    }
}

/// Sets the macOS LaunchServices default role handler for a UTI (e.g. `"public.folder"`).
#[cfg(target_os = "macos")]
pub(crate) fn macos_set_uti_handler(uti: &str, bundle_id: &str) -> i32 {
    macos::set_uti_handler(uti, bundle_id)
}

/// Reads the macOS LaunchServices default role handler for a UTI (e.g. `"public.folder"`).
#[cfg(target_os = "macos")]
pub(crate) fn macos_default_uti_handler(uti: &str) -> Option<String> {
    macos::default_uti_handler(uti)
}

/// Windows registry: programs registered for a file extension, `PerceivedType`,
/// `SystemFileAssociations`, universal `*`, and all openable `HKCR\Applications`.
#[cfg(windows)]
mod win {
    use super::AppChoice;
    use std::path::Path;
    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
    use winreg::RegKey;

    fn has_no_open_with(root: &RegKey, key: &str) -> bool {
        root.open_subkey(key)
            .ok()
            .is_some_and(|k| k.get_raw_value("NoOpenWith").is_ok())
    }

    fn open_command(root: &RegKey, key: &str) -> Option<String> {
        let k = root
            .open_subkey(format!(r"{key}\shell\open\command"))
            .ok()?;
        let v: String = k.get_value("").ok()?;
        // `%I` needs an Explorer item-ID list (`Explorer.exe /idlist,%I,%L`): can't run it.
        (!v.trim().is_empty() && !v.contains("%I") && !v.contains("%i"))
            .then(|| super::win_names::expand_env(&v))
    }

    #[link(name = "shlwapi")]
    extern "system" {
        fn SHLoadIndirectString(
            source: *const u16,
            out: *mut u16,
            out_len: u32,
            reserved: *mut *mut std::ffi::c_void,
        ) -> i32;
    }

    /// A display name from the registry; `@{Package?ms-resource://…}` and `@dll,-id`
    /// names are looked up the way Explorer does.
    fn display_name(raw: String) -> Option<String> {
        if !raw.starts_with('@') {
            return (!raw.trim().is_empty()).then_some(raw);
        }
        use std::os::windows::ffi::OsStrExt;
        let src: Vec<u16> = std::ffi::OsStr::new(&raw)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut buf = [0u16; 512];
        // SAFETY: NUL-terminated input; output buffer and its length match.
        let hr = unsafe {
            SHLoadIndirectString(
                src.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                std::ptr::null_mut(),
            )
        };
        if hr != 0 {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]);
        (!s.trim().is_empty()).then_some(s)
    }

    /// The name Explorer shows for a program ("Word" for WINWORD.EXE), from its cache
    /// of executable descriptions; else one made from the file name.
    fn program_name(command: &str) -> String {
        let exe = crate::util::cmdline::split_command(command)
            .into_iter()
            .next()
            .unwrap_or_default();
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(
                r"Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache",
            )
            .ok()
            .and_then(|k| {
                k.enum_values().flatten().find_map(|(n, v)| {
                    let path = n.strip_suffix(".FriendlyAppName")?;
                    path.eq_ignore_ascii_case(&exe).then(|| v.to_string())
                })
            })
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| super::win_names::app_name(command))
    }

    fn collect_open_with_subkeys(root: &RegKey, progids: &mut Vec<String>, exes: &mut Vec<String>) {
        if let Ok(k) = root.open_subkey("OpenWithProgids") {
            progids.extend(k.enum_values().flatten().map(|(n, _)| n));
        }
        if let Ok(k) = root.open_subkey("OpenWithList") {
            // HKCR: subkeys named after the exe; HKCU: values a, b, c… holding exe names.
            exes.extend(k.enum_keys().flatten());
            exes.extend(
                k.enum_values()
                    .flatten()
                    .filter(|(n, _)| n.len() == 1)
                    .map(|(_, v)| v.to_string()),
            );
        }
    }

    pub fn apps_for(path: &Path) -> Vec<AppChoice> {
        let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let ext = path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()));
        let ext_key = ext.as_deref().and_then(|e| hkcr.open_subkey(e).ok());
        let user_exts = ext.as_deref().and_then(|e| {
            hkcu.open_subkey(format!(
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{e}"
            ))
            .ok()
        });
        // The system default: the user's choice, else the extension's ProgID.
        let default_progid: Option<String> = user_exts
            .as_ref()
            .and_then(|k| k.open_subkey("UserChoice").ok())
            .and_then(|k| k.get_value::<String, _>("ProgId").ok())
            .or_else(|| {
                ext_key
                    .as_ref()
                    .and_then(|k| k.get_value::<String, _>("").ok())
            });

        let mut progids: Vec<String> = default_progid.iter().cloned().collect();
        let mut exes: Vec<String> = Vec::new();
        if let Some(ref k) = ext_key {
            collect_open_with_subkeys(k, &mut progids, &mut exes);
        }
        if let Some(ref k) = user_exts {
            collect_open_with_subkeys(k, &mut progids, &mut exes);
        }
        // SystemFileAssociations for the extension and its PerceivedType (e.g. `text`, `image`).
        if let Some(ref e) = ext {
            if let Ok(k) = hkcr.open_subkey(format!(r"SystemFileAssociations\{e}")) {
                collect_open_with_subkeys(&k, &mut progids, &mut exes);
            }
        }
        if let Some(perceived) = ext_key
            .as_ref()
            .and_then(|k| k.get_value::<String, _>("PerceivedType").ok())
            .filter(|p| !p.trim().is_empty())
        {
            if let Ok(k) = hkcr.open_subkey(format!(r"SystemFileAssociations\{perceived}")) {
                collect_open_with_subkeys(&k, &mut progids, &mut exes);
            }
        }
        // Universal handlers registered under `HKCR\*`.
        if let Ok(k) = hkcr.open_subkey("*") {
            collect_open_with_subkeys(&k, &mut progids, &mut exes);
        }

        let mut out: Vec<AppChoice> = Vec::new();
        let mut push = |id: String, name: String, command: String, default: bool| {
            if !out.iter().any(|a| a.command.eq_ignore_ascii_case(&command)) {
                out.push(AppChoice {
                    id,
                    name,
                    default,
                    command,
                });
            }
        };
        for p in progids {
            let is_default = default_progid.as_deref() == Some(p.as_str());
            if !is_default && has_no_open_with(&hkcr, &p) {
                continue;
            }
            let Some(cmd) = open_command(&hkcr, &p) else {
                continue;
            };
            let friendly = hkcr
                .open_subkey(format!(r"{p}\Application"))
                .ok()
                .and_then(|k| k.get_value::<String, _>("ApplicationName").ok())
                .and_then(display_name);
            let name = friendly.unwrap_or_else(|| program_name(&cmd));
            push(p, name, cmd, is_default);
        }
        for exe in exes {
            let key = format!(r"Applications\{exe}");
            if has_no_open_with(&hkcr, &key) {
                continue;
            }
            let Some(cmd) = open_command(&hkcr, &key) else {
                continue;
            };
            let name = hkcr
                .open_subkey(&key)
                .ok()
                .and_then(|k| k.get_value::<String, _>("FriendlyAppName").ok())
                .and_then(display_name)
                .unwrap_or_else(|| program_name(&cmd));
            push(exe, name, cmd, false);
        }
        out.sort_by_key(|a| !a.default);
        out
    }

    /// All openable applications registered under `HKCR\Applications\*` (without `NoOpenWith`).
    pub fn all_apps() -> Vec<AppChoice> {
        let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
        let Ok(apps_key) = hkcr.open_subkey("Applications") else {
            return Vec::new();
        };
        let mut out: Vec<AppChoice> = Vec::new();
        for exe in apps_key.enum_keys().flatten() {
            let key = format!(r"Applications\{exe}");
            if has_no_open_with(&hkcr, &key) {
                continue;
            }
            let Some(cmd) = open_command(&hkcr, &key) else {
                continue;
            };
            if out.iter().any(|a| a.command.eq_ignore_ascii_case(&cmd)) {
                continue;
            }
            let name = hkcr
                .open_subkey(&key)
                .ok()
                .and_then(|k| k.get_value::<String, _>("FriendlyAppName").ok())
                .and_then(display_name)
                .unwrap_or_else(|| program_name(&cmd));
            out.push(AppChoice {
                id: exe,
                name,
                default: false,
                command: cmd,
            });
        }
        out.sort_by_key(|a| a.name.to_lowercase());
        out
    }
}

/// Helpers for Windows registry values, pure so they are tested everywhere.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) mod win_names {
    /// `%SystemRoot%\notepad.exe` → `C:\Windows\notepad.exe` (unknown variables stay).
    pub fn expand_env(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(i) = rest.find('%') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            match after.find('%') {
                Some(j)
                    if j > 0
                        && after[..j]
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_' || c == '(' || c == ')') =>
                {
                    let var = &after[..j];
                    match std::env::var(var) {
                        Ok(v) => out.push_str(&v),
                        Err(_) => out.push_str(&rest[i..i + j + 2]),
                    }
                    rest = &after[j + 1..];
                }
                _ => {
                    out.push('%');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// A readable name from an open command: `"C:\…\Notepad++\notepad++.exe" "%1"` → `Notepad++`.
    pub fn app_name(command: &str) -> String {
        let exe = crate::util::cmdline::split_command(command)
            .into_iter()
            .next()
            .unwrap_or_default();
        let stem = std::path::Path::new(&exe.replace('\\', "/"))
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or(exe);
        let mut c = stem.chars();
        match c.next() {
            Some(f) => f.to_uppercase().chain(c).collect(),
            None => stem,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn launcher_picks_what_a_file_manager_runs() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let make = |name: &str, bytes: &[u8], mode: u32| {
            let p = t.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p
        };
        // ELF header: class, data, version, ABI, 4 pad bytes, then e_type at offset 16.
        let elf = |kind: u8| {
            let mut h = b"\x7fELF\x02\x01\x01\x00\0\0\0\0\0\0\0\0".to_vec();
            h.extend([kind, 0, 0x3e, 0]);
            h
        };
        let mut app_image = elf(2);
        app_image[8..11].copy_from_slice(b"AI\x02");
        let run = Some(Launcher::Program);
        assert_eq!(launcher(&make("a.AppImage", b"x", 0o755)), run);
        assert_eq!(launcher(&make("noext", &app_image, 0o755)), run);
        assert_eq!(launcher(&make("pie", &elf(3), 0o755)), run);
        assert_eq!(launcher(&make("old", &elf(2), 0o755)), run);
        assert_eq!(launcher(&make("libx.so", &elf(3), 0o755)), None);
        assert_eq!(launcher(&make("libx.so.6", &elf(3), 0o755)), None);
        assert_eq!(launcher(&make("obj.o", &elf(1), 0o755)), None);
        assert_eq!(
            launcher(&make("run.sh", b"#!/bin/sh\necho hi\n", 0o755)),
            Some(Launcher::Script)
        );
        assert_eq!(
            launcher(&make("app.desktop", b"[Desktop Entry]\n", 0o755)),
            Some(Launcher::Desktop)
        );
        // No execute bit, no marker, not a regular file: opened by its MIME handler.
        assert_eq!(launcher(&make("b.AppImage", b"x", 0o644)), None);
        assert_eq!(launcher(&make("p.sh", b"#!/bin/sh\n", 0o644)), None);
        assert_eq!(launcher(&make("notes.txt", b"hello", 0o755)), None);
        assert_eq!(launcher(t.path()), None);
    }

    #[test]
    fn windows_registry_helpers() {
        std::env::set_var("GALL_TEST_ROOT", r"C:\Windows");
        assert_eq!(
            win_names::expand_env(r"%GALL_TEST_ROOT%\notepad.exe %1"),
            r"C:\Windows\notepad.exe %1"
        );
        assert_eq!(
            win_names::expand_env("100% %NOPE_NOT_SET% x"),
            "100% %NOPE_NOT_SET% x"
        );
        assert_eq!(
            win_names::app_name(r#""C:\Program Files\Notepad++\notepad++.exe" "%1""#),
            "Notepad++"
        );
        assert_eq!(
            win_names::app_name(r"C:\Windows\system32\mspaint.exe %1"),
            "Mspaint"
        );
    }

    #[test]
    fn exec_field_codes() {
        let p = Path::new("/home/u/My File.txt");
        assert_eq!(expand_exec("gedit %U", p), ["gedit", "/home/u/My File.txt"]);
        assert_eq!(
            expand_exec("code --new-window %F", p),
            ["code", "--new-window", "/home/u/My File.txt"]
        );
        assert_eq!(
            expand_exec("vlc --started-from-file", p),
            ["vlc", "--started-from-file", "/home/u/My File.txt"]
        );
        assert_eq!(
            expand_exec("app %i %c --x", p),
            ["app", "--x", "/home/u/My File.txt"]
        );
        assert_eq!(
            expand_exec(r#"sh -c "echo \"hi\" 100%%" %f"#, p),
            ["sh", "-c", r#"echo "hi" 100%"#, "/home/u/My File.txt"]
        );
    }

    #[test]
    fn exec_many_batches_percent_f_u_and_splits_single_file_codes() {
        let paths = vec![PathBuf::from("/tmp/a.md"), PathBuf::from("/tmp/b.md")];
        assert!(exec_supports_multiple("wordcraft %F"));
        assert!(exec_supports_multiple("mpv -- %U"));
        assert!(!exec_supports_multiple("omawrite %f"));
        assert!(!exec_supports_multiple("firefox %u"));
        assert!(!exec_supports_multiple("viewer"));

        assert_eq!(
            expand_exec_many("wordcraft %F", &paths),
            vec![vec!["wordcraft", "/tmp/a.md", "/tmp/b.md"]]
        );
        assert_eq!(
            expand_exec_many("omawrite %f", &paths),
            vec![vec!["omawrite", "/tmp/a.md"], vec!["omawrite", "/tmp/b.md"],]
        );
    }

    #[test]
    fn desktop_entry() {
        let e = xdg::DesktopEntry::parse(
            "[Desktop Entry]\nName=Text Editor\nExec=gnome-text-editor %U\nPath=/tmp\n[Desktop Action new]\nName=New\nExec=x\n",
        )
        .unwrap();
        assert_eq!(
            (
                e.name.as_str(),
                e.exec.as_str(),
                e.hidden,
                e.deleted,
                e.terminal,
                e.path.as_deref()
            ),
            (
                "Text Editor",
                "gnome-text-editor %U",
                false,
                false,
                false,
                Some("/tmp")
            )
        );
        let nd =
            xdg::DesktopEntry::parse("[Desktop Entry]\nName=A\nExec=a\nNoDisplay=true\n").unwrap();
        assert!(nd.hidden);
        assert!(!nd.deleted);
        let del =
            xdg::DesktopEntry::parse("[Desktop Entry]\nName=B\nExec=b\nHidden=true\n").unwrap();
        assert!(del.hidden);
        assert!(del.deleted);
        let term =
            xdg::DesktopEntry::parse("[Desktop Entry]\nName=Neovim\nExec=nvim %F\nTerminal=true\n")
                .unwrap();
        assert!(term.terminal);
        let action_term = xdg::DesktopEntry::parse(
            "[Desktop Entry]\nName=App\nExec=app %f\n[Desktop Action Term]\nName=Term\nExec=app --cli\nTerminal=true\n",
        )
        .unwrap();
        assert!(!action_term.terminal);
    }

    #[test]
    fn choice_command_plan_routes_terminal_prefix_and_tui_editors() {
        let f = Path::new("/tmp/notes.md");
        let (term, argv, dir) = plan_choice_command(f, "nvim {path}").unwrap();
        assert!(term);
        assert_eq!(argv, vec!["nvim", "/tmp/notes.md"]);
        assert_eq!(dir, Path::new("/tmp"));

        let (term, argv, _) = plan_choice_command(f, "omarchy-launch-editor {path}").unwrap();
        assert!(!term, "omarchy-launch-editor wraps internally");
        assert_eq!(argv, vec!["omarchy-launch-editor", "/tmp/notes.md"]);

        let (term, argv, _) = plan_choice_command(f, "terminal: less {path}").unwrap();
        assert!(term);
        assert_eq!(argv, vec!["less", "/tmp/notes.md"]);

        let (term, argv, _) = plan_choice_command(f, "code {path}").unwrap();
        assert!(!term);
        assert_eq!(argv, vec!["code", "/tmp/notes.md"]);

        let (term, argv, rel_dir) =
            plan_choice_command(Path::new("notes.md"), "nvim {path}").unwrap();
        assert!(term);
        assert_eq!(argv, vec!["nvim", "notes.md"]);
        assert_eq!(rel_dir, Path::new("."));
    }

    /// A `.desktop` file dropped into the user's data dir (no `update-desktop-database`) is
    /// offered through its `MimeType=`; NoDisplay, Hidden and a missing `TryExec` are not.
    #[test]
    fn apps_from_uncached_user_desktop_files() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("share");
        std::fs::create_dir_all(data.join("mime")).unwrap();
        std::fs::create_dir_all(data.join("applications")).unwrap();
        std::fs::write(data.join("mime/globs2"), "50:text/plain:*.txt\n").unwrap();
        for (id, body) in [
            (
                "good.desktop",
                "Name=Good\nExec=good %f\nMimeType=text/plain;",
            ),
            (
                "term.desktop",
                "Name=Term\nExec=tui %F\nTerminal=true\nMimeType=text/plain;",
            ),
            (
                "nodisplay.desktop",
                "Name=NoDisp\nExec=x\nNoDisplay=true\nMimeType=text/plain;",
            ),
            (
                "hidden.desktop",
                "Name=Hid\nExec=x\nHidden=true\nMimeType=text/plain;",
            ),
            (
                "tryexec.desktop",
                "Name=Try\nExec=x\nTryExec=definitely-missing-e2e-binary\nMimeType=text/plain;",
            ),
            ("other.desktop", "Name=Other\nExec=o\nMimeType=image/png;"),
        ] {
            std::fs::write(
                data.join("applications").join(id),
                format!("[Desktop Entry]\nType=Application\n{body}\n"),
            )
            .unwrap();
        }
        let dirs = xdg::Dirs {
            data: vec![data],
            config: vec![t.path().join("config")],
        };
        let file = t.path().join("a.txt");
        std::fs::write(&file, "").unwrap();
        let names: Vec<_> = xdg::apps_for(&file, &dirs)
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["Good", "Term"]);
    }

    /// A tiny freedesktop tree: globs, subclasses, two editors, a default.
    #[test]
    fn apps_from_registries() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("share");
        let config = t.path().join("config");
        std::fs::create_dir_all(data.join("mime")).unwrap();
        std::fs::create_dir_all(data.join("applications")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            data.join("mime/globs2"),
            "50:text/x-python:*.py\n50:text/plain:*.txt\n",
        )
        .unwrap();
        std::fs::write(data.join("mime/subclasses"), "text/x-python text/plain\n").unwrap();
        std::fs::write(
            data.join("applications/mimeinfo.cache"),
            "[MIME Cache]\ntext/plain=editor.desktop;hidden.desktop;\ntext/x-python=ide.desktop;\n",
        )
        .unwrap();
        for (id, body) in [
            ("editor.desktop", "Name=Editor\nExec=editor %f"),
            ("ide.desktop", "Name=IDE\nExec=ide %U"),
            ("hidden.desktop", "Name=Hidden\nExec=h\nNoDisplay=true"),
        ] {
            std::fs::write(
                data.join("applications").join(id),
                format!("[Desktop Entry]\n{body}\n"),
            )
            .unwrap();
        }
        std::fs::write(
            config.join("mimeapps.list"),
            "[Default Applications]\ntext/x-python=editor.desktop\n",
        )
        .unwrap();
        let dirs = xdg::Dirs {
            data: vec![data],
            config: vec![config],
        };
        let file = t.path().join("script.PY");
        std::fs::write(&file, "").unwrap();
        assert_eq!(
            xdg::mime_type(&file, &dirs).as_deref(),
            Some("text/x-python")
        );
        let apps = xdg::apps_for(&file, &dirs);
        let names: Vec<_> = apps.iter().map(|a| (a.name.as_str(), a.default)).collect();
        assert_eq!(names, [("Editor", true), ("IDE", false)]);
    }

    /// Reproduces the Omarchy `.md` and `.txt` opening scenario:
    /// - `.md` has no explicit `[Default Applications]`, but `~/.local/share/applications/mimeinfo.cache`
    ///   lists `wordcraft.desktop`, while system `mimeapps.list` sets `text/plain=nvim.desktop` (`Terminal=true`).
    /// - `.md` MUST resolve to `wordcraft.desktop` (not `text/plain` -> headless `nvim`)!
    /// - `.txt` MUST resolve to `nvim.desktop` with `terminal == true`!
    /// - Desktop-specific `hyprland-mimeapps.list` overrides generic `mimeapps.list`!
    /// - `NoDisplay=true` default handler (like `omarchy-browser.desktop`) is still usable as default!
    #[test]
    fn omarchy_markdown_terminal_and_desktop_mimeapps_resolution() {
        let t = tempfile::tempdir().unwrap();
        let user_data = t.path().join("user_share");
        let sys_data = t.path().join("sys_share");
        let config = t.path().join("config");
        for d in [&user_data, &sys_data] {
            std::fs::create_dir_all(d.join("mime")).unwrap();
            std::fs::create_dir_all(d.join("applications")).unwrap();
        }
        std::fs::create_dir_all(&config).unwrap();

        std::fs::write(
            sys_data.join("mime/globs2"),
            "50:text/markdown:*.md\n10:application/x-genesis-rom:*.md\n50:text/plain:*.txt\n50:text/x-makefile:Makefile:cs\n50:text/html:*.html\n",
        )
        .unwrap();
        std::fs::write(
            sys_data.join("mime/subclasses"),
            "text/markdown text/plain\ntext/x-makefile text/plain\n",
        )
        .unwrap();
        std::fs::write(
            sys_data.join("mime/aliases"),
            "text/x-markdown text/markdown\n",
        )
        .unwrap();
        std::fs::write(
            user_data.join("applications/mimeinfo.cache"),
            "[MIME Cache]\ntext/markdown=wordcraft.desktop;\n",
        )
        .unwrap();
        std::fs::write(
            sys_data.join("applications/mimeinfo.cache"),
            "[MIME Cache]\ntext/markdown=obsidian.desktop;\ntext/x-markdown=omawrite.desktop;\ntext/plain=nvim.desktop;\n",
        )
        .unwrap();
        std::fs::write(
            sys_data.join("applications/mimeapps.list"),
            "[Default Applications]\ntext/plain=nvim.desktop\ntext/html=firefox.desktop\n",
        )
        .unwrap();
        std::fs::write(
            config.join("hyprland-mimeapps.list"),
            "[Default Applications]\ntext/html=omarchy-browser.desktop\n",
        )
        .unwrap();

        std::fs::write(
            user_data.join("applications/wordcraft.desktop"),
            "[Desktop Entry]\nType=Application\nName=WordCraft\nExec=/opt/wordcraft %F\nTerminal=false\nMimeType=text/markdown;\n",
        )
        .unwrap();
        std::fs::write(
            user_data.join("applications/omarchy-browser.desktop"),
            "[Desktop Entry]\nType=Application\nName=Browser\nExec=omarchy-launch-browser %U\nNoDisplay=true\nMimeType=text/html;\n",
        )
        .unwrap();
        for (id, body) in [
            (
                "obsidian.desktop",
                "Name=Obsidian\nExec=/usr/bin/obsidian %U\nMimeType=text/markdown;",
            ),
            (
                "omawrite.desktop",
                "Name=Omawrite\nExec=omawrite %f\nMimeType=text/x-markdown;",
            ),
            (
                "nvim.desktop",
                "Name=Neovim\nExec=nvim %F\nTerminal=true\nMimeType=text/plain;",
            ),
            (
                "firefox.desktop",
                "Name=Firefox\nExec=firefox %u\nMimeType=text/html;",
            ),
        ] {
            std::fs::write(
                sys_data.join("applications").join(id),
                format!("[Desktop Entry]\nType=Application\n{body}\n"),
            )
            .unwrap();
        }

        let dirs = xdg::Dirs {
            data: vec![user_data, sys_data],
            config: vec![config.clone()],
        };
        let desktops = xdg::parse_desktops("Hyprland");

        // 1. Markdown file with spaces, Unicode, and commas in Documents
        let md = t.path().join("Reši tri naloge, test.md");
        std::fs::write(&md, "# Naslov\nBesedilo\n").unwrap();
        assert_eq!(xdg::mime_type(&md, &dirs).as_deref(), Some("text/markdown"));
        let (def_id, def_entry) = xdg::default_entry_for_desktops(&md, &dirs, &desktops).unwrap();
        assert_eq!(def_id, "wordcraft.desktop");
        assert!(!def_entry.terminal);
        let apps = xdg::apps_for_desktops(&md, &dirs, &desktops);
        assert_eq!(apps[0].id, "wordcraft.desktop");
        assert!(apps[0].default);
        // Alias `text/x-markdown` (`omawrite.desktop`) and subclass `text/plain` (`nvim.desktop`) are both offered.
        assert!(apps.iter().any(|a| a.id == "obsidian.desktop"));
        assert!(apps.iter().any(|a| a.id == "omawrite.desktop"));
        assert!(apps.iter().any(|a| a.id == "nvim.desktop"));

        // 2. Plain text file resolves to Terminal=true nvim.desktop
        let txt = t.path().join("notes.txt");
        std::fs::write(&txt, "hello\n").unwrap();
        let (txt_id, txt_entry) = xdg::default_entry_for_desktops(&txt, &dirs, &desktops).unwrap();
        assert_eq!(txt_id, "nvim.desktop");
        assert!(txt_entry.terminal);

        // 3. Exact filename glob (`Makefile`) and extensionless plain text file (`NOTES`)
        let mk = t.path().join("Makefile");
        std::fs::write(&mk, "all:\n\techo ok\n").unwrap();
        assert_eq!(
            xdg::mime_type(&mk, &dirs).as_deref(),
            Some("text/x-makefile")
        );
        let notes = t.path().join("NOTES");
        std::fs::write(&notes, "plain utf-8 note without extension\n").unwrap();
        assert_eq!(xdg::mime_type(&notes, &dirs).as_deref(), Some("text/plain"));
        assert_eq!(
            xdg::default_entry_for_desktops(&notes, &dirs, &desktops)
                .unwrap()
                .0,
            "nvim.desktop"
        );

        // 4. Desktop-specific `hyprland-mimeapps.list` wins even when `NoDisplay=true`
        let html = t.path().join("page.html");
        std::fs::write(&html, "<html></html>").unwrap();
        assert_eq!(
            xdg::default_entry_for_desktops(&html, &dirs, &desktops)
                .unwrap()
                .0,
            "omarchy-browser.desktop"
        );
        assert_eq!(
            xdg::default_entry_for_desktops(&html, &dirs, &[])
                .unwrap()
                .0,
            "firefox.desktop"
        );

        // 5. `all_apps` returns all visible installed apps sorted alphabetically, skipping NoDisplay
        let all = xdg::all_apps(&dirs);
        let all_names: Vec<_> = all.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            all_names,
            ["Firefox", "Neovim", "Obsidian", "Omawrite", "WordCraft"]
        );

        // 6. `set_default_in_file` updates mimeapps.list cleanly and changes the default
        let user_mimeapps = config.join("mimeapps.list");
        xdg::set_default_in_file(&user_mimeapps, "text/markdown", "omawrite.desktop").unwrap();
        assert_eq!(
            xdg::default_entry_for_desktops(&md, &dirs, &desktops)
                .unwrap()
                .0,
            "omawrite.desktop"
        );
    }

    #[test]
    fn mimeapps_updater_preserves_comments_and_clears_removed() {
        let initial = "\
# User MIME preferences
[Default Applications]
text/html=firefox.desktop;
text/markdown=wordcraft.desktop;

[Removed Associations]
text/markdown=omawrite.desktop;obsidian.desktop;
";
        let updated = xdg::update_mimeapps_text(initial, "text/markdown", "omawrite.desktop");
        assert!(updated.contains("# User MIME preferences"));
        assert!(updated.contains("text/html=firefox.desktop;"));
        assert!(updated.contains("text/markdown=omawrite.desktop;"));
        assert!(!updated.contains("text/markdown=wordcraft.desktop;"));
        assert!(updated.contains("[Removed Associations]\ntext/markdown=obsidian.desktop;"));
    }

    #[test]
    fn content_sniffer_detects_magic_shebangs_and_utf8_text() {
        assert_eq!(
            xdg::sniff_bytes_mime(b"%PDF-1.7\n").as_deref(),
            Some("application/pdf")
        );
        assert_eq!(
            xdg::sniff_bytes_mime(b"\x89PNG\r\n\x1a\nxxxx").as_deref(),
            Some("image/png")
        );
        assert_eq!(
            xdg::sniff_bytes_mime(b"#!/usr/bin/env python3\nprint(1)\n").as_deref(),
            Some("text/x-python")
        );
        assert_eq!(
            xdg::sniff_bytes_mime(b"#!/bin/bash\necho hi\n").as_deref(),
            Some("application/x-shellscript")
        );
        assert_eq!(
            xdg::sniff_bytes_mime(b"Pozdravljeni, to je navadno besedilo!\n").as_deref(),
            Some("text/plain")
        );
        assert_eq!(xdg::sniff_bytes_mime(b"\x00\x01\x02\x03\xFF"), None);
    }

    #[test]
    fn macos_info_plist_parses_bundle_and_matches_extensions() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>Zed</string>
  <key>CFBundleIdentifier</key>
  <string>dev.zed.Zed</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeRole</key>
      <string>Editor</string>
      <key>CFBundleTypeExtensions</key>
      <array>
        <string>md</string>
        <string>rs</string>
      </array>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.plain-text</string>
      </array>
    </dict>
  </array>
</dict>
</plist>"#;
        let info = macos_bundle::parse_info_plist(xml).unwrap();
        assert_eq!(info.name, "Zed");
        assert_eq!(info.bundle_id.as_deref(), Some("dev.zed.Zed"));
        assert!(!info.background_only);
        assert!(macos_bundle::matches_extension(&info, "md"));
        assert!(macos_bundle::matches_extension(&info, "txt"));
        assert!(macos_bundle::matches_extension(&info, "rs"));
        assert!(!macos_bundle::matches_extension(&info, "png"));
    }

    #[test]
    fn vendor_subdir_desktop_lookup_works() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("share");
        let sub = data.join("applications/kde4");
        std::fs::create_dir_all(&sub).unwrap();
        let kate = sub.join("kate.desktop");
        std::fs::write(&kate, "[Desktop Entry]\nName=Kate\nExec=kate %U\n").unwrap();
        let dirs = xdg::Dirs {
            data: vec![data],
            config: vec![],
        };
        assert_eq!(dirs.find_desktop("kde4-kate.desktop"), Some(kate));
        assert_eq!(dirs.find_desktop("../evil.desktop"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_enumerates_installed_and_registered_apps() {
        let d = tempfile::tempdir().unwrap();
        let txt = d.path().join("sample.txt");
        std::fs::write(&txt, "hello").unwrap();

        let registered = apps_for(&txt);
        assert!(
            !registered.is_empty(),
            "expected at least one registered macOS app for .txt"
        );
        assert!(
            registered[0].default,
            "default application should be listed first: {registered:?}"
        );
        assert!(
            registered.iter().all(|a| a.command.starts_with("macos:")),
            "every macOS choice should use the macos: command prefix"
        );

        let all = all_apps();
        assert!(
            all.len() >= 5,
            "expected macOS system applications to be enumerated, got {}",
            all.len()
        );
        assert!(
            all.iter()
                .any(|a| a.id.ends_with(".app") && a.name.eq_ignore_ascii_case("TextEdit")),
            "expected TextEdit in all_apps()"
        );
    }
}
