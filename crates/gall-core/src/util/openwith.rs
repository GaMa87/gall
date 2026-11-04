//! "Open with…": choose the application for a file.
//!
//! - Linux (freedesktop): the applications registered for the file's MIME type,
//!   read like desktop environments do (shared-mime-info globs, `mimeinfo.cache`,
//!   `mimeapps.list` defaults and associations, `.desktop` files).
//! - Windows: the programs registered for the extension in the registry
//!   (`OpenWithProgids`, `OpenWithList`, `UserChoice`); the system dialog stays available.
//! - Everywhere: a custom command template (see [`crate::util::cmdline`]).
//!
//! Every choice carries a `command` that [`run_choice`] runs and that Gall stores
//! when the user makes it the default for a file type.

use crate::error::{CoreError, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppChoice {
    /// Desktop file id, e.g. `org.gnome.TextEditor.desktop`.
    pub id: String,
    pub name: String,
    /// The system's default application for this type.
    pub default: bool,
    /// What to run: `desktop:<id>` for a freedesktop application, `system` for the
    /// system default, otherwise a command template (`zed {path}`).
    pub command: String,
}

/// Runs a choice's `command` for `path` (see [`AppChoice::command`]).
pub fn run_choice(path: &Path, command: &str) -> Result<()> {
    let command = command.trim();
    if command == "system" {
        return match open_default(path) {
            Some(r) => r,
            None => open_system(path),
        };
    }
    if let Some(id) = command.strip_prefix("desktop:") {
        return open_with_app(path, id);
    }
    let dir = path.parent().unwrap_or(path);
    crate::util::cmdline::spawn(
        &crate::util::cmdline::build(command, path, dir, true)?,
        Some(dir),
    )
}

/// The platform opener (xdg-open / open) for systems without [`open_default`].
fn open_system(path: &Path) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    crate::util::cmdline::spawn(
        &[opener.to_string(), path.to_string_lossy().into_owned()],
        path.parent(),
    )
}

/// Applications that can open `path`, default first. Empty where we read no
/// registry (macOS): the palette then offers the system default and custom commands.
pub fn apps_for(path: &Path) -> Vec<AppChoice> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dirs = xdg::Dirs::from_env();
        xdg::apps_for(path, &dirs)
    }
    #[cfg(windows)]
    {
        win::apps_for(path)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = path;
        Vec::new()
    }
}

/// Opens `path` with the application `id` from [`apps_for`].
pub fn open_with_app(path: &Path, id: &str) -> Result<()> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dirs = xdg::Dirs::from_env();
        let entry = dirs
            .find_desktop(id)
            .and_then(|f| xdg::DesktopEntry::read(&f))
            .ok_or_else(|| CoreError::Other(format!("application {id} not found")))?;
        spawn(&expand_exec(&entry.exec, path))
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = (path, id);
        Err(CoreError::Other(
            "choose the application in the system dialog".into(),
        ))
    }
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

/// Opens `path` with its default application. Windows: ShellExecute directly (the
/// `opener` plugin goes through `cmd /c start`, so the app was not started by the
/// foreground process and opened in the background). Elsewhere: `None`, the caller
/// uses the platform opener.
pub fn open_default(path: &Path) -> Option<Result<()>> {
    #[cfg(windows)]
    {
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
        let dir: Option<Vec<u16>> = path
            .parent()
            .map(|d| d.as_os_str().encode_wide().chain(Some(0)).collect());
        // SAFETY: NUL-terminated UTF-16 strings that outlive the call; a null verb
        // means the file type's default verb.
        let r = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                std::ptr::null(),
                wide.as_ptr(),
                std::ptr::null(),
                dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
                SW_SHOWNORMAL,
            )
        };
        Some(if r > 32 {
            Ok(())
        } else {
            Err(CoreError::Other(match r {
                2 | 3 => format!("'{}' not found", path.display()),
                5 => "access denied".into(),
                31 => format!(
                    "no application is set to open '{}'; use Open with…",
                    path.display()
                ),
                _ => format!("cannot open '{}' (error {r})", path.display()),
            }))
        })
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
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

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn(argv: &[String]) -> Result<()> {
    crate::util::cmdline::spawn(argv, None)
}

/// Splits a desktop-entry `Exec` value into arguments and fills in the file
/// (`%f %F %u %U`); other field codes are dropped, `%%` is a percent sign. Without
/// a file code the file is appended.
pub fn expand_exec(exec: &str, path: &Path) -> Vec<String> {
    let file = path.to_string_lossy().into_owned();
    let mut out = Vec::new();
    let mut used = false;
    for tok in split_exec(exec) {
        match tok.as_str() {
            "%f" | "%F" | "%u" | "%U" => {
                out.push(file.clone());
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
                            s.push_str(&file);
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
    if !used {
        out.push(file);
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

    pub struct Dirs {
        /// `$XDG_DATA_HOME` first, then `$XDG_DATA_DIRS`.
        pub data: Vec<PathBuf>,
        /// `$XDG_CONFIG_HOME` first, then `$XDG_CONFIG_DIRS`.
        pub config: Vec<PathBuf>,
    }

    impl Dirs {
        pub fn from_env() -> Self {
            let home = dirs::home_dir().unwrap_or_default();
            let list = |var: &str, home_var: &str, home_default: PathBuf, default: &str| {
                let mut v = vec![std::env::var_os(home_var)
                    .map(PathBuf::from)
                    .unwrap_or(home_default)];
                let rest =
                    std::env::var("XDG_".to_string() + var).unwrap_or_else(|_| default.into());
                v.extend(rest.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
                v
            };
            Dirs {
                data: list(
                    "DATA_DIRS",
                    "XDG_DATA_HOME",
                    home.join(".local/share"),
                    "/usr/local/share:/usr/share",
                ),
                config: list(
                    "CONFIG_DIRS",
                    "XDG_CONFIG_HOME",
                    home.join(".config"),
                    "/etc/xdg",
                ),
            }
        }

        pub fn find_desktop(&self, id: &str) -> Option<PathBuf> {
            self.data
                .iter()
                .map(|d| d.join("applications").join(id))
                .find(|p| p.is_file())
        }
    }

    pub struct DesktopEntry {
        pub name: String,
        pub exec: String,
        pub hidden: bool,
    }

    impl DesktopEntry {
        pub fn read(file: &Path) -> Option<Self> {
            Self::parse(&std::fs::read_to_string(file).ok()?)
        }

        /// The `[Desktop Entry]` group: Name (localised for $LANG when present), Exec,
        /// NoDisplay / Hidden.
        pub fn parse(text: &str) -> Option<Self> {
            let lang = std::env::var("LANG").unwrap_or_default();
            let lang = lang.split(['.', '@']).next().unwrap_or("");
            let short = lang.split('_').next().unwrap_or("");
            let (mut name, mut local, mut exec, mut hidden) = (None, None, None, false);
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
                    "NoDisplay" | "Hidden" if v.trim() == "true" => hidden = true,
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
                hidden,
            })
        }
    }

    /// MIME type of `path` from shared-mime-info `globs2` files (`weight:type:glob`),
    /// matching `*.ext` globs case-insensitively; directories are `inode/directory`.
    pub fn mime_type(path: &Path, dirs: &Dirs) -> Option<String> {
        if path.is_dir() {
            return Some("inode/directory".into());
        }
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        let mut best: Option<(u32, usize, String)> = None;
        for d in &dirs.data {
            let Ok(text) = std::fs::read_to_string(d.join("mime/globs2")) else {
                continue;
            };
            for line in text.lines().filter(|l| !l.starts_with('#')) {
                let mut parts = line.splitn(4, ':');
                let (Some(w), Some(t), Some(g)) = (parts.next(), parts.next(), parts.next()) else {
                    continue;
                };
                let Some(suffix) = g.strip_prefix('*') else {
                    continue;
                };
                if suffix.contains(['*', '?', '[']) || !name.ends_with(&suffix.to_lowercase()) {
                    continue;
                }
                let w: u32 = w.parse().unwrap_or(50);
                // Higher weight wins, then the longer (more specific) suffix.
                if best
                    .as_ref()
                    .is_none_or(|(bw, bl, _)| (w, suffix.len()) > (*bw, *bl))
                {
                    best = Some((w, suffix.len(), t.to_string()));
                }
            }
        }
        best.map(|b| b.2)
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
        while i < out.len() && out.len() < 8 {
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

    pub fn apps_for(path: &Path, dirs: &Dirs) -> Vec<AppChoice> {
        let Some(mime) = mime_type(path, dirs) else {
            return Vec::new();
        };
        let types = with_parents(&mime, dirs);
        let lists: Vec<PathBuf> = dirs
            .config
            .iter()
            .map(|d| d.join("mimeapps.list"))
            .chain(
                dirs.data
                    .iter()
                    .map(|d| d.join("applications/mimeapps.list")),
            )
            .collect();
        let lists: Vec<String> = lists
            .iter()
            .filter_map(|f| std::fs::read_to_string(f).ok())
            .collect();
        let pick = |group: &str| -> Vec<String> {
            let mut v = Vec::new();
            for text in &lists {
                for (k, ids) in ini_group(text, group) {
                    if k == mime {
                        v.extend(ids);
                    }
                }
            }
            v
        };
        let default = pick("Default Applications").into_iter().next();
        let removed = pick("Removed Associations");
        let mut ids: Vec<String> = default.iter().cloned().collect();
        ids.extend(pick("Added Associations"));
        for t in &types {
            for d in &dirs.data {
                if let Ok(text) = std::fs::read_to_string(d.join("applications/mimeinfo.cache")) {
                    for (k, v) in ini_group(&text, "MIME Cache") {
                        if &k == t {
                            ids.extend(v);
                        }
                    }
                }
            }
        }
        let mut out: Vec<AppChoice> = Vec::new();
        for id in ids {
            if removed.contains(&id) || out.iter().any(|a| a.id == id) {
                continue;
            }
            let Some(e) = dirs.find_desktop(&id).and_then(|f| DesktopEntry::read(&f)) else {
                continue;
            };
            if e.hidden {
                continue;
            }
            out.push(AppChoice {
                default: default.as_deref() == Some(id.as_str()),
                command: format!("desktop:{id}"),
                id,
                name: e.name,
            });
        }
        out
    }
}

/// Windows registry: programs registered for a file extension.
#[cfg(windows)]
mod win {
    use super::AppChoice;
    use std::path::Path;
    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
    use winreg::RegKey;

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

    pub fn apps_for(path: &Path) -> Vec<AppChoice> {
        let Some(ext) = path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        else {
            return Vec::new();
        };
        let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let user_exts = hkcu
            .open_subkey(format!(
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{ext}"
            ))
            .ok();
        // The system default: the user's choice, else the extension's ProgID.
        let default_progid: Option<String> = user_exts
            .as_ref()
            .and_then(|k| k.open_subkey("UserChoice").ok())
            .and_then(|k| k.get_value::<String, _>("ProgId").ok())
            .or_else(|| {
                hkcr.open_subkey(&ext)
                    .ok()
                    .and_then(|k| k.get_value::<String, _>("").ok())
            });

        let mut progids: Vec<String> = default_progid.iter().cloned().collect();
        let mut exes: Vec<String> = Vec::new();
        for root in [hkcr.open_subkey(&ext).ok(), user_exts]
            .into_iter()
            .flatten()
        {
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
            let Some(cmd) = open_command(&hkcr, &p) else {
                continue;
            };
            let friendly = hkcr
                .open_subkey(format!(r"{p}\Application"))
                .ok()
                .and_then(|k| k.get_value::<String, _>("ApplicationName").ok())
                .and_then(display_name);
            let name = friendly.unwrap_or_else(|| program_name(&cmd));
            push(
                p.clone(),
                name,
                cmd,
                default_progid.as_deref() == Some(p.as_str()),
            );
        }
        for exe in exes {
            let key = format!(r"Applications\{exe}");
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
    fn desktop_entry() {
        let e = xdg::DesktopEntry::parse(
            "[Desktop Entry]\nName=Text Editor\nExec=gnome-text-editor %U\n[Desktop Action new]\nName=New\nExec=x\n",
        )
        .unwrap();
        assert_eq!(
            (e.name.as_str(), e.exec.as_str(), e.hidden),
            ("Text Editor", "gnome-text-editor %U", false)
        );
        assert!(
            xdg::DesktopEntry::parse("[Desktop Entry]\nName=A\nExec=a\nNoDisplay=true\n")
                .unwrap()
                .hidden
        );
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
}
