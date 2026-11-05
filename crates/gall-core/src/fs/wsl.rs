//! Windows Subsystem for Linux: the installed distributions as locations
//! (`\\wsl.localhost\Ubuntu\`, `\\wsl$\Ubuntu\` on older Windows), conversion between
//! those UNC paths and Linux paths, and the checks that keep a copy or move between
//! Linux and Windows from losing or merging files.
//!
//! Everything that parses or decides is plain logic, tested on every platform. Only
//! asking Windows (the registry, `wsl.exe`) is behind `cfg(windows)`; without WSL the
//! feature is simply absent: no `wsl.exe` is started when no distribution is registered.
//!
//! [`register_root`] makes any folder count as the root of a distribution, so copying,
//! moving and deleting "inside WSL" can be tested with temporary folders.

use super::drives::Drive;
use super::ops::TaskKind;
use crate::error::{CoreError, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

// ───────────────────────────── distributions ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DistroState {
    Running,
    Stopped,
    /// `wsl.exe` did not answer (in time).
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Distro {
    pub name: String,
    /// WSL 1 or 2 (0: not known).
    pub version: u32,
    /// The distribution `wsl.exe` starts without `-d`.
    pub default: bool,
    pub state: DistroState,
    /// `\\wsl.localhost\<name>\` (or `\\wsl$\<name>\`).
    pub root: String,
}

/// The UNC host Windows serves the distributions under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// `\\wsl.localhost` (Windows 11, and Windows 10 with WSL from the Store).
    Localhost,
    /// `\\wsl$` (every Windows with WSL; the only one on older Windows 10).
    Dollar,
}

impl Host {
    pub fn as_str(self) -> &'static str {
        match self {
            Host::Localhost => "wsl.localhost",
            Host::Dollar => "wsl$",
        }
    }

    pub fn other(self) -> Host {
        match self {
            Host::Localhost => Host::Dollar,
            Host::Dollar => Host::Localhost,
        }
    }
}

/// The host to try first on Windows build `build`: `\\wsl.localhost` from Windows 11 on,
/// `\\wsl$` before (it works everywhere; see [`detect`] for the check that switches).
pub fn host_for_build(build: u32) -> Host {
    if build >= 22000 {
        Host::Localhost
    } else {
        Host::Dollar
    }
}

/// `\\wsl.localhost\Ubuntu\`.
pub fn root(distro: &str, host: Host) -> String {
    format!(r"\\{}\{}\", host.as_str(), distro)
}

/// Distribution names are letters, digits, `.`, `-` and `_` (what `wsl --import` accepts);
/// used to tell names from messages in `wsl.exe` output.
pub fn valid_distro_name(name: &str) -> bool {
    // Starts with a letter or digit: a leading `-` would read as a wsl.exe option.
    name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Docker Desktop's internal distributions: no shell, nothing to browse, and opening one
/// only fails.
pub fn hidden_distro(name: &str) -> bool {
    name.eq_ignore_ascii_case("docker-desktop") || name.eq_ignore_ascii_case("docker-desktop-data")
}

/// Text written by `wsl.exe`: UTF-16LE (with or without a byte-order mark) by default,
/// UTF-8 when `WSL_UTF8=1` is set.
pub fn decode_output(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8]| {
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    };
    let text = if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        utf16(rest)
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        // Without a mark: text that is mostly ASCII has a zero in every second byte.
        let odd = bytes.len() / 2;
        let zeros = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
        if odd > 0 && zeros * 2 >= odd {
            utf16(bytes)
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        }
    };
    text.replace(['\0', '\u{feff}'], "")
}

/// Distribution names in `wsl.exe --list --quiet` output (one per line). Messages such as
/// "There are no running distributions." are not names and are left out.
pub fn parse_list_quiet(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\0' || c == '\u{feff}'))
        .filter(|l| valid_distro_name(l))
        .map(str::to_string)
        .collect()
}

/// One row of `wsl.exe --list --verbose`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub default: bool,
    /// Some(true) running, Some(false) stopped, None: another or a translated state.
    pub running: Option<bool>,
    pub version: u32,
}

/// Rows of `wsl.exe --list --verbose`:
/// ```text
///   NAME      STATE           VERSION
/// * Ubuntu    Running         2
///   Debian    Stopped         2
/// ```
/// The header and the state are translated on other Windows languages, so the name is
/// the first word, the version the last, and the state is only read in English.
pub fn parse_list_verbose(text: &str) -> Vec<Listed> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\0' || c == '\u{feff}');
        let (default, rest) = match line.strip_prefix('*') {
            Some(r) => (true, r.trim_start()),
            None => (false, line),
        };
        let words: Vec<&str> = rest.split_whitespace().collect();
        if words.len() < 3 || !valid_distro_name(words[0]) {
            continue;
        }
        let Ok(version) = words[words.len() - 1].parse::<u32>() else {
            continue; // the header ("NAME STATE VERSION")
        };
        let state = words[1..words.len() - 1].join(" ");
        out.push(Listed {
            name: words[0].to_string(),
            default,
            running: match state.as_str() {
                "Running" => Some(true),
                "Stopped" => Some(false),
                _ => None,
            },
            version,
        });
    }
    out
}

/// A distribution as registered under `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss\{guid}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegDistro {
    pub guid: String,
    pub name: String,
    /// 1 = installed; others are being installed, converted or removed.
    pub state: u32,
    pub version: u32,
}

/// The distributions to show: registered ones that are installed, default first, then
/// by name. `default_guid` is the `DefaultDistribution` value; `running`: names from
/// `wsl.exe --list --running --quiet`, None when that did not answer.
pub fn distros_from_registry(
    entries: &[RegDistro],
    default_guid: &str,
    running: Option<&[String]>,
    host: Host,
) -> Vec<Distro> {
    let mut out: Vec<Distro> = entries
        .iter()
        .filter(|e| e.state == 1 && valid_distro_name(&e.name) && !hidden_distro(&e.name))
        .map(|e| Distro {
            name: e.name.clone(),
            version: e.version,
            default: e.guid.eq_ignore_ascii_case(default_guid),
            state: match running {
                Some(r) if r.iter().any(|n| n.eq_ignore_ascii_case(&e.name)) => {
                    DistroState::Running
                }
                Some(_) => DistroState::Stopped,
                None => DistroState::Unknown,
            },
            root: root(&e.name, host),
        })
        .collect();
    out.sort_by(|a, b| {
        b.default
            .cmp(&a.default)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

/// The distributions from `wsl.exe --list --verbose` (when the registry has none).
pub fn distros_from_list(rows: &[Listed], host: Host) -> Vec<Distro> {
    rows.iter()
        .filter(|r| !hidden_distro(&r.name))
        .map(|r| Distro {
            name: r.name.clone(),
            version: r.version,
            default: r.default,
            state: match r.running {
                Some(true) => DistroState::Running,
                Some(false) => DistroState::Stopped,
                None => DistroState::Unknown,
            },
            root: root(&r.name, host),
        })
        .collect()
}

/// A distribution counts as mounted (listed among the drives) once it runs, or once it
/// was started in this session: one that stops again when idle stays, so a browser in it
/// is not sent away. The others wait under "Not mounted" like unmounted volumes.
pub fn is_mounted(d: &Distro, started: &[String]) -> bool {
    d.state == DistroState::Running || started.iter().any(|s| s.eq_ignore_ascii_case(&d.name))
}

/// The mounted distributions (see [`is_mounted`]) as rows of the drive list (sidebar
/// Drives group, drives column). No size: asking for it could start a stopped one.
pub fn as_drives(distros: &[Distro], started: &[String]) -> Vec<Drive> {
    distros
        .iter()
        .filter(|d| is_mounted(d, started))
        .map(|d| Drive {
            short: d.name.clone(),
            label: "WSL".into(),
            mount: d.root.clone(),
            fs: "wsl".into(),
            // "Eject" stops the distribution (see [`terminate`]).
            removable: true,
            ..Default::default()
        })
        .collect()
}

/// The other distributions, for "Not mounted": clicking one starts it (see
/// [`super::volumes::mount_volume`]) and it moves up among the drives.
pub fn as_volumes(distros: &[Distro], started: &[String]) -> Vec<super::volumes::Volume> {
    distros
        .iter()
        .filter(|d| !is_mounted(d, started))
        .map(|d| super::volumes::Volume {
            id: d.root.clone(),
            label: d.name.clone(),
            size: 0,
            fs: "wsl".into(),
            locked: false,
            removable: false,
        })
        .collect()
}

// ───────────────────────────── paths ─────────────────────────────

/// A path inside a distribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslPath {
    pub distro: String,
    /// The Linux path: `/home/u/a:b` (characters Windows shows as private-use ones are
    /// turned back, see [`linux_name`]).
    pub linux: String,
    pub host: Host,
}

/// `\\wsl.localhost\Ubuntu\home\u` → Ubuntu, `/home/u`. Also `\\wsl$\…`, forward
/// slashes and the verbatim `\\?\UNC\wsl.localhost\…`. The host is matched without case.
pub fn parse_unc(path: &str) -> Option<WslPath> {
    let s = path.replace('/', "\\");
    let rest = s
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| s.strip_prefix(r"\\.\UNC\"))
        .or_else(|| s.strip_prefix(r"\\"))?;
    let mut parts = rest.splitn(3, '\\');
    let host = match parts.next()?.to_ascii_lowercase().as_str() {
        "wsl.localhost" => Host::Localhost,
        "wsl$" => Host::Dollar,
        _ => return None,
    };
    let distro = parts.next().filter(|d| !d.is_empty())?.to_string();
    let tail = parts.next().unwrap_or("");
    let linux: Vec<String> = tail
        .split('\\')
        .filter(|c| !c.is_empty())
        .map(linux_name)
        .collect();
    Some(WslPath {
        distro,
        linux: format!("/{}", linux.join("/")),
        host,
    })
}

/// Ubuntu, `/home/u` → `\\wsl.localhost\Ubuntu\home\u` (the inverse of [`parse_unc`]).
pub fn to_unc(distro: &str, linux: &str, host: Host) -> String {
    let rel: Vec<String> = linux
        .split('/')
        .filter(|c| !c.is_empty())
        .map(windows_name)
        .collect();
    format!("{}{}", root(distro, host), rel.join("\\"))
}

/// Folders that count as distribution roots besides `\\wsl.localhost\…` (see [`register_root`]).
static ROOTS: RwLock<Vec<(PathBuf, String)>> = RwLock::new(Vec::new());

/// Makes `dir` count as the root of distribution `distro`: everything below it is
/// treated as inside WSL (no Recycle Bin, Linux names). Tests use temporary folders.
pub fn register_root(dir: &Path, distro: &str) {
    ROOTS
        .write()
        .unwrap()
        .push((dir.to_path_buf(), distro.to_string()));
}

/// Where `p` is inside WSL, or None for every other path. UNC forms only count on
/// Windows (on Linux `\\wsl$\x` is an odd relative file name).
pub fn locate(p: &Path) -> Option<WslPath> {
    if cfg!(windows) {
        if let Some(w) = parse_unc(&p.to_string_lossy()) {
            return Some(w);
        }
    }
    let roots = ROOTS.read().unwrap();
    roots.iter().find_map(|(r, d)| {
        let rel = p.strip_prefix(r).ok()?;
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        Some(WslPath {
            distro: d.clone(),
            linux: format!("/{}", parts.join("/")),
            host: Host::Localhost,
        })
    })
}

/// True when `p` is inside a WSL distribution.
pub fn is_wsl_path(p: &Path) -> bool {
    locate(p).is_some()
}

/// The spelling of `p` to hand to the file system: a path inside WSL whose last name ends
/// with a dot or space gets the verbatim prefix (`\\?\UNC\wsl.localhost\…`), which keeps
/// Windows from dropping it (the plain path would name another file). Others unchanged.
pub fn io_path(p: &Path) -> std::borrow::Cow<'_, Path> {
    if cfg!(windows) {
        if let Some(v) = verbatim(&p.to_string_lossy()) {
            return std::borrow::Cow::Owned(PathBuf::from(v));
        }
    }
    std::borrow::Cow::Borrowed(p)
}

/// The verbatim spelling [`io_path`] uses, for a WSL UNC path whose last name ends with a
/// dot or space; None for every other path.
pub fn verbatim(path: &str) -> Option<String> {
    parse_unc(path)?;
    let rest = path
        .strip_prefix(r"\\")
        .filter(|r| !r.starts_with(['?', '.']))?;
    let last = rest
        .trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()?;
    unreachable_name(last).then(|| format!(r"\\?\UNC\{}", rest.replace('/', "\\")))
}

/// A path typed into the path bar: on Windows a Linux path (`/home/u`) means that path in
/// the distribution of `base` (the open folder), else in `default` (name and host).
/// None when `input` is no Linux path or there is no distribution to put it in.
pub fn resolve_typed(
    input: &str,
    base: Option<&str>,
    default: Option<(&str, Host)>,
) -> Option<String> {
    if !input.starts_with('/') || input.starts_with("//") {
        return None;
    }
    let (distro, host) = match base.and_then(parse_unc) {
        Some(w) => (w.distro, w.host),
        None => {
            let (d, h) = default?;
            (d.to_string(), h)
        }
    };
    Some(to_unc(&distro, input, host))
}

/// `path` as Linux sees it under WSL: a path inside a distribution as its Linux path,
/// a drive path as `/mnt/<letter>/…`, a Linux path as it is. None for other paths
/// (a network share).
pub fn linux_path_for(path: &str) -> Option<String> {
    if path.starts_with('/') && !path.starts_with("//") {
        return Some(path.to_string());
    }
    if let Some(w) = parse_unc(path) {
        return Some(w.linux);
    }
    let rest = path.strip_prefix(r"\\?\").unwrap_or(path);
    let b = rest.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        let parts: Vec<String> = rest[2..]
            .split(['\\', '/'])
            .filter(|c| !c.is_empty())
            .map(linux_name)
            .collect();
        let mut out = format!("/mnt/{}", (b[0] as char).to_ascii_lowercase());
        for p in parts {
            out.push('/');
            out.push_str(&p);
        }
        return Some(out);
    }
    None
}

/// "Terminal here" in a WSL folder: a shell of that distribution in the Linux folder,
/// in Windows Terminal when `wt` is installed. None for other folders.
pub fn terminal_argv(dir: &str, wt: bool) -> Option<Vec<String>> {
    let w = parse_unc(dir)?;
    let mut argv = Vec::new();
    let linux = if wt {
        argv.push("wt.exe".to_string());
        // Windows Terminal splits its command line at `;` (new tab) unless escaped.
        w.linux.replace(';', r"\;")
    } else {
        w.linux.clone()
    };
    argv.extend([
        "wsl.exe".into(),
        "-d".into(),
        w.distro,
        "--cd".into(),
        linux,
    ]);
    Some(argv)
}

// ───────────────────────────── names ─────────────────────────────

/// Characters Windows does not allow in names (besides control characters and `/`).
const ILLEGAL: &str = "\\:*?\"<>|";

/// Windows shows Linux names with characters it does not allow by moving them to the
/// private use area (`:` → U+F03A), the way WSL maps them for its own drives; this turns
/// one back.
pub fn linux_char(c: char) -> char {
    let u = c as u32;
    if (0xF001..=0xF07F).contains(&u) {
        if let Some(orig) = char::from_u32(u - 0xF000) {
            if (orig as u32) < 0x20 || ILLEGAL.contains(orig) {
                return orig;
            }
        }
    }
    c
}

/// A name as Linux has it (see [`linux_char`]).
pub fn linux_name(name: &str) -> String {
    name.chars().map(linux_char).collect()
}

/// A Linux name as Windows shows it (the inverse of [`linux_name`]).
pub fn windows_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if (c != '\0' && (c as u32) < 0x20) || ILLEGAL.contains(c) {
                char::from_u32(0xF000 + c as u32).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// Why a name from inside WSL cannot be used as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameProblem {
    /// A character Windows does not allow in names.
    Char(char),
    /// Ends with a dot or a space: Windows drops those from paths, so the path names
    /// another item (or none). Also a problem inside WSL itself.
    TrailingDotOrSpace,
    /// A device name Windows reserves in every folder (CON, NUL, COM1, …, also `nul.txt`).
    Reserved,
    /// Differs from this other name in the same folder only in case: Windows folders
    /// would hold one of them.
    CaseClash(String),
}

impl NameProblem {
    fn describe(&self) -> String {
        match self {
            NameProblem::Char(c) if (*c as u32) < 0x20 => {
                format!(
                    "control character U+{:04X} is not allowed on Windows",
                    *c as u32
                )
            }
            NameProblem::Char(c) => format!("“{c}” is not allowed on Windows"),
            NameProblem::TrailingDotOrSpace => {
                "ends with a dot or space, which Windows drops from paths".into()
            }
            NameProblem::Reserved => "a reserved device name on Windows".into(),
            NameProblem::CaseClash(other) => {
                format!("differs from “{other}” only in case; Windows would keep one of them")
            }
        }
    }
}

/// Device names Windows reserves in every folder, also with an extension (`NUL.txt`).
fn windows_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || (stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.as_bytes()[3].is_ascii_digit()
        && stem.as_bytes()[3] != b'0')
}

/// Names that cannot be reached through Windows paths at all, also inside WSL.
pub fn unreachable_name(name: &str) -> bool {
    name.ends_with(['.', ' ']) && name != "." && name != ".."
}

/// Why `name` (as listed inside WSL) cannot exist as is in a Windows folder; None when it can.
pub fn windows_name_problem(name: &str) -> Option<NameProblem> {
    let linux = linux_name(name);
    if let Some(c) = linux
        .chars()
        .find(|c| ILLEGAL.contains(*c) || (*c as u32) < 0x20)
    {
        return Some(NameProblem::Char(c));
    }
    if unreachable_name(&linux) {
        return Some(NameProblem::TrailingDotOrSpace);
    }
    if windows_reserved(&linux) {
        return Some(NameProblem::Reserved);
    }
    None
}

/// Names that differ only in case: (name, the earlier name it clashes with), in the
/// order given.
pub fn case_clashes(names: &[String]) -> Vec<(String, String)> {
    let mut seen: std::collections::HashMap<String, &String> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for n in names {
        let key = linux_name(n).to_lowercase();
        match seen.get(&key) {
            Some(first) if *first != n => out.push((n.clone(), (*first).clone())),
            Some(_) => {}
            None => {
                seen.insert(key, n);
            }
        }
    }
    out
}

// ───────────────────────────── checks before an operation ─────────────────────────────

/// An item inside WSL that an operation cannot handle as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: PathBuf,
    pub what: NameProblem,
}

/// Stop collecting after this many: the message names a few, and a tree full of them
/// need not be walked to the end.
const MAX_PROBLEMS: usize = 50;

/// Checks a file task before anything is changed. Free when no source is inside WSL.
/// Items inside WSL whose names Windows cannot address (trailing dot or space) are
/// refused for every task; for a copy or move out of WSL into a Windows folder also the
/// names Windows does not allow and names that differ only in case. The error names them
/// and says that nothing was done. What is below the sources is only walked when Windows
/// handles it item by item (permanent delete, out of WSL, archives); a copy, duplicate
/// or move that stays inside WSL is done by Linux (`cp -a`, `tar`, rename), which handles
/// any name, so only the sources' own names count there.
pub fn check_task(
    kind: TaskKind,
    sources: &[PathBuf],
    dest: Option<&Path>,
    cancel: &AtomicBool,
) -> Result<()> {
    if !sources.iter().any(|s| is_wsl_path(s)) {
        return Ok(());
    }
    let to_windows =
        matches!(kind, TaskKind::Copy | TaskKind::Move) && dest.is_some_and(|d| !is_wsl_path(d));
    let stays_in_wsl = match kind {
        TaskKind::Copy | TaskKind::Move => dest.is_some_and(is_wsl_path),
        TaskKind::Duplicate => true,
        _ => false,
    };
    let problems = scan(sources, to_windows, !stays_in_wsl, cancel)?;
    if problems.is_empty() {
        return Ok(());
    }
    Err(CoreError::Other(report(kind, &problems, to_windows)))
}

/// Refuses to rename (or otherwise change) an item inside WSL whose path Windows cannot
/// address: the change would hit another item.
pub fn check_path(p: &Path) -> Result<()> {
    match locate(p) {
        Some(w) if w.linux.split('/').any(unreachable_name) => Err(CoreError::Other(format!(
            "“{}” cannot be changed from Windows: a name in its path ends with a dot or space, \
             which Windows drops from paths. Rename it in Linux (in a WSL terminal).",
            w.linux
        ))),
        _ => Ok(()),
    }
}

/// The problems in `sources` (only those inside WSL) and everything below them.
fn scan(
    sources: &[PathBuf],
    to_windows: bool,
    deep: bool,
    cancel: &AtomicBool,
) -> Result<Vec<Problem>> {
    let mut out = Vec::new();
    let wsl: Vec<&PathBuf> = sources.iter().filter(|s| is_wsl_path(s)).collect();
    let check = |p: &Path, out: &mut Vec<Problem>| {
        let Some(name) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return;
        };
        let what = if to_windows {
            windows_name_problem(&name)
        } else {
            unreachable_name(&name).then_some(NameProblem::TrailingDotOrSpace)
        };
        if let Some(what) = what {
            out.push(Problem {
                path: p.to_path_buf(),
                what,
            });
        }
    };
    // The sources themselves land side by side in the destination folder.
    if to_windows {
        let names: Vec<String> = wsl
            .iter()
            .filter_map(|s| s.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        for (name, other) in case_clashes(&names) {
            if let Some(s) = wsl
                .iter()
                .find(|s| s.file_name().is_some_and(|n| n.to_string_lossy() == name))
            {
                out.push(Problem {
                    path: s.to_path_buf(),
                    what: NameProblem::CaseClash(other),
                });
            }
        }
    }
    for s in &wsl {
        check(s, &mut out);
        // A link is copied as a link: what it points to is not walked.
        if deep && std::fs::symlink_metadata(s).is_ok_and(|m| m.is_dir()) {
            scan_dir(s, to_windows, cancel, &mut out, &check)?;
        }
        if out.len() >= MAX_PROBLEMS {
            break;
        }
    }
    Ok(out)
}

fn scan_dir(
    dir: &Path,
    to_windows: bool,
    cancel: &AtomicBool,
    out: &mut Vec<Problem>,
    check: &dyn Fn(&Path, &mut Vec<Problem>),
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Cancelled);
    }
    if out.len() >= MAX_PROBLEMS {
        return Ok(());
    }
    // Folders that cannot be read are left to the operation, which says so.
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut children: Vec<(PathBuf, String, bool)> = rd
        .flatten()
        .map(|e| {
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            (
                e.path(),
                e.file_name().to_string_lossy().into_owned(),
                is_dir,
            )
        })
        .collect();
    children.sort_by(|a, b| a.1.cmp(&b.1));
    for (path, _, _) in &children {
        check(path, out);
    }
    if to_windows {
        let names: Vec<String> = children.iter().map(|c| c.1.clone()).collect();
        for (name, other) in case_clashes(&names) {
            out.push(Problem {
                path: dir.join(&name),
                what: NameProblem::CaseClash(other),
            });
        }
    }
    for (path, _, is_dir) in &children {
        if *is_dir {
            scan_dir(path, to_windows, cancel, out, check)?;
        }
    }
    Ok(())
}

/// The message for problems found before a task.
fn report(kind: TaskKind, problems: &[Problem], to_windows: bool) -> String {
    let (verb, done) = match kind {
        TaskKind::Copy => ("copy", "copied"),
        TaskKind::Move => ("move", "moved"),
        TaskKind::Delete => ("delete", "deleted"),
        TaskKind::Duplicate => ("duplicate", "duplicated"),
        TaskKind::Compress => ("compress", "compressed"),
        TaskKind::Extract => ("extract", "extracted"),
    };
    const SHOWN: usize = 5;
    let named: Vec<String> = problems
        .iter()
        .take(SHOWN)
        .map(|p| {
            let (name, folder) = match locate(&p.path) {
                Some(w) => {
                    let (dir, name) = w.linux.rsplit_once('/').unwrap_or(("", &w.linux));
                    (
                        name.to_string(),
                        if dir.is_empty() {
                            "/".to_string()
                        } else {
                            dir.to_string()
                        },
                    )
                }
                None => (
                    p.path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    p.path
                        .parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default(),
                ),
            };
            format!("“{name}” in {folder} ({})", p.what.describe())
        })
        .collect();
    let more = if problems.len() > SHOWN {
        format!(
            ", and {}{} more",
            if problems.len() >= MAX_PROBLEMS {
                "at least "
            } else {
                ""
            },
            problems.len() - SHOWN
        )
    } else {
        String::new()
    };
    let n = problems.len();
    let items = if n == 1 {
        "1 item".to_string()
    } else {
        format!("{n}{} items", if n >= MAX_PROBLEMS { "+" } else { "" })
    };
    let fix = if to_windows
        && problems
            .iter()
            .any(|p| !matches!(p.what, NameProblem::TrailingDotOrSpace))
    {
        "Rename them in Linux first, or copy them to a folder inside WSL."
    } else {
        "Rename them in Linux first (in a WSL terminal)."
    };
    format!(
        "Nothing was {done}: cannot {verb} {items} inside WSL with {}: {}{more}. {fix}",
        if n == 1 { "its name" } else { "their names" },
        named.join("; ")
    )
}

// ───────────────────────────── copies inside Linux ─────────────────────────────
//
// A copy from one place inside WSL to another, made through Windows, passes every byte
// through the 9P file server twice and loses what Windows cannot express: permissions
// (a script is no longer executable), owners, symbolic links. Linux does it instead:
// `cp -a` within a distribution, a `tar` pipe between two.

/// How a copy between two places inside WSL runs inside Linux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinuxCopy {
    /// Within one distribution: `cp -a --reflink=auto -T -- src dst`.
    Same {
        distro: String,
        src: String,
        dst: String,
    },
    /// Between two: `tar -c` of `name` in `src_dir` (distribution `from`), unpacked by
    /// `tar -x` into the new folder `tmp_dir` (distribution `to`), whose only item is then
    /// renamed to the target (which may have another name: "a (2)").
    Across {
        from: String,
        src_dir: String,
        name: String,
        to: String,
        tmp_dir: String,
    },
}

impl LinuxCopy {
    /// The commands to run: (distribution, argv) for each; two are piped together.
    pub fn commands(&self) -> Vec<(String, Vec<String>)> {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        match self {
            LinuxCopy::Same { distro, src, dst } => vec![(
                distro.clone(),
                v(&["cp", "-a", "--reflink=auto", "-T", "--", src, dst]),
            )],
            LinuxCopy::Across {
                from,
                src_dir,
                name,
                to,
                tmp_dir,
            } => vec![
                (
                    from.clone(),
                    v(&["tar", "-C", src_dir, "-cf", "-", "--", name]),
                ),
                (to.clone(), v(&["tar", "-C", tmp_dir, "-xpf", "-"])),
            ],
        }
    }
}

/// The Linux copy of `src` to the new path `dst` (both inside WSL); `tmp_name` names the
/// temporary folder a copy between distributions unpacks into. None when one of them is
/// not inside WSL.
pub fn linux_copy_plan(src: &Path, dst: &Path, tmp_name: &str) -> Option<LinuxCopy> {
    let (s, d) = (locate(src)?, locate(dst)?);
    if s.linux == "/" || d.linux == "/" {
        return None;
    }
    if s.distro.eq_ignore_ascii_case(&d.distro) {
        return Some(LinuxCopy::Same {
            distro: s.distro,
            src: s.linux,
            dst: d.linux,
        });
    }
    let (src_dir, name) = s.linux.rsplit_once('/')?;
    let (dst_dir, _) = d.linux.rsplit_once('/')?;
    let dir = |d: &str| {
        if d.is_empty() {
            "/".to_string()
        } else {
            d.to_string()
        }
    };
    Some(LinuxCopy::Across {
        from: s.distro.clone(),
        src_dir: dir(src_dir),
        name: name.to_string(),
        to: d.distro,
        tmp_dir: format!("{}/{tmp_name}", dst_dir.trim_end_matches('/')),
    })
}

/// A command run in `distro`: through `wsl.exe` for `\\wsl.localhost` paths; for a
/// folder registered with [`register_root`] (tests), directly, with Linux paths
/// (arguments starting with `/`) mapped below that folder.
fn linux_command(distro: &str, via_root: Option<&Path>, argv: &[String]) -> std::process::Command {
    match via_root {
        Some(root) => {
            let mut c = std::process::Command::new(&argv[0]);
            c.args(argv[1..].iter().map(|a| {
                if a.starts_with('/') {
                    under_root(root, a).into_os_string()
                } else {
                    a.into()
                }
            }));
            c
        }
        None => {
            #[allow(unused_mut)]
            let mut c = std::process::Command::new(wsl_exe());
            c.args(["-d", distro, "--exec"]).args(argv);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            c
        }
    }
}

fn wsl_exe() -> PathBuf {
    let sys = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let p = sys.join("System32").join("wsl.exe");
    if p.is_file() {
        p
    } else {
        PathBuf::from("wsl.exe")
    }
}

/// The registered root `p` is under (None for `\\wsl.localhost` paths).
fn registered_root(p: &Path) -> Option<PathBuf> {
    if cfg!(windows) && parse_unc(&p.to_string_lossy()).is_some() {
        return None;
    }
    ROOTS
        .read()
        .unwrap()
        .iter()
        .find(|(r, _)| p.starts_with(r))
        .map(|(r, _)| r.clone())
}

static COPY_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Copies made by Linux so far (tests check that Linux, not Windows, copied).
#[cfg(test)]
static LINUX_COPIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Copies `src` to the new path `dst` inside Linux when both are inside WSL (see
/// [`LinuxCopy`]): faster, and permissions, owners and links stay. None when it does not
/// apply, or when Linux could not even start (no GNU `cp`, as on Alpine; nothing was
/// created): the caller then copies through Windows. Progress: a single file is
/// followed by its size; a folder counts when it is done. Cancel stops the copy and
/// removes what was copied.
pub fn copy_inside_linux(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    tick: &mut dyn FnMut(u64),
    durable: bool,
) -> Option<Result<()>> {
    // Windows: through wsl.exe (`\\wsl.localhost` paths). Elsewhere only for folders
    // registered as distribution roots (tests), run directly.
    let (rs, rd) = (registered_root(src), registered_root(dst));
    let usable = if cfg!(windows) {
        rs.is_none() && rd.is_none()
    } else {
        rs.is_some() && rd.is_some()
    };
    if !usable {
        return None;
    }
    let tmp_name = format!(
        ".gall-copy-{}-{}",
        std::process::id(),
        COPY_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let plan = linux_copy_plan(src, dst, &tmp_name)?;
    let meta = std::fs::symlink_metadata(src).ok()?;
    let tmp = dst.parent()?.join(&tmp_name);
    if matches!(plan, LinuxCopy::Across { .. }) && std::fs::create_dir(&tmp).is_err() {
        return None;
    }
    let cleanup = |tmp_too: bool| {
        if std::fs::symlink_metadata(dst).is_ok() {
            let _ = if std::fs::symlink_metadata(dst).is_ok_and(|m| m.is_dir()) {
                std::fs::remove_dir_all(dst)
            } else {
                std::fs::remove_file(dst)
            };
        }
        if tmp_too {
            let _ = std::fs::remove_dir_all(&tmp);
        }
    };
    let across = matches!(plan, LinuxCopy::Across { .. });
    let result = run_copy(&plan, src, dst, cancel, tick, meta.is_file());
    match result {
        Ok(()) => {}
        Err(CopyFailure::NotStarted) => {
            cleanup(across);
            return None;
        }
        Err(CopyFailure::Cancelled) => {
            cleanup(across);
            return Some(Err(CoreError::Cancelled));
        }
        Err(CopyFailure::Failed(why)) => {
            cleanup(across);
            return Some(Err(CoreError::Other(format!(
                "copying “{}” inside WSL failed: {why}",
                linux_name(
                    &src.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )
            ))));
        }
    }
    if let LinuxCopy::Across { .. } = plan {
        let moved = match src.file_name() {
            Some(name) => std::fs::rename(tmp.join(name), dst),
            None => Err(std::io::Error::other("no file name")),
        };
        let _ = std::fs::remove_dir_all(&tmp);
        if let Err(e) = moved {
            cleanup(false);
            return Some(Err(CoreError::io("copy", dst, e)));
        }
    }
    if let (true, Some(w)) = (durable, locate(dst)) {
        let distro = w.distro;
        let _ = linux_command(
            &distro,
            registered_root(dst).as_deref(),
            &["sync".to_string()],
        )
        .status();
    }
    // What the copy counts as done (bytes and items), as `measure` counted them.
    if meta.is_dir() {
        for e in walkdir::WalkDir::new(src)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            if let Ok(m) = e.metadata() {
                if m.is_file() {
                    tick(m.len());
                }
            }
            tick(0);
        }
    } else {
        tick(0);
    }
    #[cfg(test)]
    LINUX_COPIES.fetch_add(1, Ordering::Relaxed);
    Some(Ok(()))
}

enum CopyFailure {
    /// Linux did not start the copy and nothing was created.
    NotStarted,
    Cancelled,
    Failed(String),
}

fn run_copy(
    plan: &LinuxCopy,
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    tick: &mut dyn FnMut(u64),
    single_file: bool,
) -> std::result::Result<(), CopyFailure> {
    use std::process::Stdio;
    let cmds = plan.commands();
    let roots = [registered_root(src), registered_root(dst)];
    let mut children = Vec::new();
    let mut prev_out: Option<std::process::ChildStdout> = None;
    for (i, (distro, argv)) in cmds.iter().enumerate() {
        let root = if i == 0 {
            roots[0].as_deref()
        } else {
            roots[1].as_deref()
        };
        // `sh` says the process id first (on stderr), so Cancel can stop the copy inside
        // Linux too: killing wsl.exe alone may leave it running.
        let mut wrapped: Vec<String> = ["sh", "-c", "echo $$ >&2; exec \"$@\"", "gall-copy"]
            .map(String::from)
            .to_vec();
        wrapped.extend(argv.iter().cloned());
        let mut c = linux_command(distro, root, &wrapped);
        c.stderr(Stdio::piped());
        match prev_out.take() {
            Some(out) => c.stdin(Stdio::from(out)),
            None => c.stdin(Stdio::null()),
        };
        c.stdout(if i + 1 < cmds.len() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = match c.spawn() {
            Ok(ch) => ch,
            Err(_) => {
                for mut ch in children {
                    let _ = std::process::Child::kill(&mut ch);
                    let _ = ch.wait();
                }
                return Err(CopyFailure::NotStarted);
            }
        };
        prev_out = child.stdout.take();
        children.push(child);
    }
    // The process ids inside Linux (first stderr line), then the error text, read on the
    // side so a full pipe never blocks the copy.
    let mut pids: Vec<Option<String>> = Vec::new();
    let readers: Vec<_> = children
        .iter_mut()
        .map(|c| {
            use std::io::{BufRead, Read};
            let mut e = c.stderr.take().map(std::io::BufReader::new);
            let mut first = String::new();
            if let Some(e) = e.as_mut() {
                let _ = e.read_line(&mut first);
            }
            let pid = first.trim();
            let is_pid = !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit());
            pids.push(is_pid.then(|| pid.to_string()));
            let early = if is_pid { String::new() } else { first };
            std::thread::spawn(move || {
                let mut s = early;
                if let Some(e) = e.as_mut() {
                    let _ = e.read_to_string(&mut s);
                }
                s
            })
        })
        .collect();
    let mut seen = 0u64;
    let mut statuses = vec![None; children.len()];
    loop {
        if cancel.load(Ordering::Relaxed) {
            // Stop the copy inside Linux, let wsl.exe end with it (a few seconds at
            // most), then make sure: the caller removes the partial target next.
            for (i, pid) in pids.iter().enumerate() {
                if let Some(pid) = pid {
                    let (distro, _) = &cmds[i];
                    let root = if i == 0 {
                        roots[0].as_deref()
                    } else {
                        roots[1].as_deref()
                    };
                    let kill = ["kill", "-TERM", pid.as_str()].map(String::from);
                    let _ = linux_command(distro, root, &kill)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
            let t0 = std::time::Instant::now();
            for c in children.iter_mut() {
                while matches!(c.try_wait(), Ok(None))
                    && t0.elapsed() < std::time::Duration::from_secs(3)
                {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                let _ = c.kill();
                let _ = c.wait();
            }
            for r in readers {
                let _ = r.join();
            }
            return Err(CopyFailure::Cancelled);
        }
        for (i, c) in children.iter_mut().enumerate() {
            if statuses[i].is_none() {
                if let Ok(Some(st)) = c.try_wait() {
                    statuses[i] = Some(st);
                }
            }
        }
        if single_file {
            // A large file: its growing size is the progress.
            if let Ok(m) = std::fs::metadata(dst) {
                if m.len() > seen {
                    tick(m.len() - seen);
                    seen = m.len();
                }
            }
        }
        if statuses.iter().all(Option::is_some) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let errors: Vec<String> = readers
        .into_iter()
        .map(|r| r.join().unwrap_or_default())
        .collect();
    if statuses.iter().all(|s| s.is_some_and(|s| s.success())) {
        // Keep the byte count right for the file (the caller ticks the item).
        if single_file {
            if let Ok(m) = std::fs::metadata(dst) {
                if m.len() > seen {
                    tick(m.len() - seen);
                }
            }
        }
        return Ok(());
    }
    let created = std::fs::symlink_metadata(dst).is_ok()
        || matches!(plan, LinuxCopy::Across { .. })
            && dst
                .parent()
                .and_then(|p| std::fs::read_dir(p.join(tmp_of(plan))).ok())
                .is_some_and(|mut d| d.next().is_some());
    if !created {
        return Err(CopyFailure::NotStarted);
    }
    let why = errors
        .iter()
        .flat_map(|e| e.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("exit status not 0")
        .to_string();
    Err(CopyFailure::Failed(why))
}

fn tmp_of(plan: &LinuxCopy) -> String {
    match plan {
        LinuxCopy::Across { tmp_dir, .. } => tmp_dir.rsplit('/').next().unwrap_or("").to_string(),
        LinuxCopy::Same { .. } => String::new(),
    }
}

// ───────────────────────────── the Linux trash ─────────────────────────────
//
// Windows has no Recycle Bin for `\\wsl` paths (and deletes for good when asked to
// recycle there). Items inside WSL go to the distribution's own trash instead, the one
// Linux file managers use (freedesktop.org: `~/.local/share/Trash/{files,info}`), so
// "move to trash" and its undo work as everywhere else, and Nautilus in WSLg sees them.

/// Home folders (Linux paths) of the default user per distribution.
static HOMES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Sets the Linux home folder of `distro`'s default user. On Windows it is found in the
/// distribution's /etc/passwd; tests set it for their stand-in roots.
pub fn set_home(distro: &str, home: &str) {
    let mut h = HOMES.lock().unwrap();
    h.retain(|(d, _)| !d.eq_ignore_ascii_case(distro));
    h.push((distro.to_string(), home.to_string()));
}

/// The home folder of user id `uid` in /etc/passwd text (`name:x:uid:gid:gecos:home:shell`).
pub fn home_from_passwd(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 6 && f[2].trim().parse::<u32>() == Ok(uid) && f[5].starts_with('/'))
            .then(|| f[5].trim_end_matches('/').to_string())
            .map(|h| if h.is_empty() { "/".to_string() } else { h })
    })
}

/// `p` inside WSL with the Windows path of its distribution's root.
fn with_root(p: &Path) -> Option<(PathBuf, WslPath)> {
    if cfg!(windows) {
        if let Some(w) = parse_unc(&p.to_string_lossy()) {
            return Some((PathBuf::from(root(&w.distro, w.host)), w));
        }
    }
    let roots = ROOTS.read().unwrap();
    let (r, _) = roots.iter().find(|(r, _)| p.starts_with(r))?;
    let r = r.clone();
    drop(roots);
    locate(p).map(|w| (r, w))
}

/// The Windows path of Linux path `linux` under distribution root `root`.
fn under_root(root: &Path, linux: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    for c in linux.split('/').filter(|c| !c.is_empty()) {
        out.push(if cfg!(windows) {
            windows_name(c)
        } else {
            c.to_string()
        });
    }
    out
}

/// The default user's home folder in the distribution at `root`.
fn home(distro: &str, root: &Path) -> Option<String> {
    if let Some((_, h)) = HOMES
        .lock()
        .unwrap()
        .iter()
        .find(|(d, _)| d.eq_ignore_ascii_case(distro))
    {
        return Some(h.clone());
    }
    #[cfg(windows)]
    {
        let uid = win::default_uid(distro).unwrap_or(1000);
        let passwd = std::fs::read_to_string(under_root(root, "/etc/passwd")).ok()?;
        let h = home_from_passwd(&passwd, uid)?;
        set_home(distro, &h);
        Some(h)
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        None
    }
}

/// The trash of the distribution `p` is in: (its `files` and `info` folders as Windows
/// paths, `p` as Linux path), or why `p` cannot go there.
fn trash_for(p: &Path) -> std::result::Result<(PathBuf, PathBuf, WslPath), String> {
    let Some((root, w)) = with_root(p) else {
        return Err(format!("“{}” is not inside WSL", p.display()));
    };
    let name = w
        .linux
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or(&w.linux)
        .to_string();
    if w.linux.starts_with("/mnt/") {
        return Err(format!(
            "“{name}” is on a Windows drive mounted in WSL ({}); open it through its drive \
             letter to use the Recycle Bin",
            w.linux.split('/').take(3).collect::<Vec<_>>().join("/")
        ));
    }
    let Some(home) = home(&w.distro, &root) else {
        return Err(format!(
            "“{name}” is inside WSL ({}), which has no Recycle Bin, and its Linux trash \
             was not found",
            w.distro
        ));
    };
    let trash = format!("{}/.local/share/Trash", home.trim_end_matches('/'));
    let inside = |dir: &str| {
        w.linux == dir
            || w.linux
                .starts_with(&format!("{}/", dir.trim_end_matches('/')))
    };
    if w.linux == "/"
        || inside(&trash)
        || home.starts_with(&format!("{}/", w.linux))
        || w.linux == home
    {
        return Err(format!("“{name}” cannot be moved to the Linux trash"));
    }
    Ok((
        under_root(&root, &format!("{trash}/files")),
        under_root(&root, &format!("{trash}/info")),
        w,
    ))
}

/// Why `p` cannot go to the trash of its distribution; None when it can or is not in WSL.
pub fn no_trash_reason(p: &Path) -> Option<String> {
    locate(p)?;
    trash_for(p).err()
}

/// `%`-encodes a Linux path for a `.trashinfo` file (RFC 2396, `/` kept).
pub fn encode_trash_path(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The inverse of [`encode_trash_path`].
pub fn decode_trash_path(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `.trashinfo` text for Linux path `path` deleted at local time `when`.
pub fn trashinfo(path: &str, when: chrono::NaiveDateTime) -> String {
    format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        encode_trash_path(path),
        when.format("%Y-%m-%dT%H:%M:%S")
    )
}

/// (Linux path, deletion time) from `.trashinfo` text.
pub fn parse_trashinfo(text: &str) -> Option<(String, chrono::NaiveDateTime)> {
    let mut path = None;
    let mut date = None;
    for l in text.lines() {
        if let Some(v) = l.strip_prefix("Path=") {
            path = Some(decode_trash_path(v.trim()));
        } else if let Some(v) = l.strip_prefix("DeletionDate=") {
            date = chrono::NaiveDateTime::parse_from_str(v.trim(), "%Y-%m-%dT%H:%M:%S").ok();
        }
    }
    Some((path?, date?))
}

/// Moves `paths` (all inside WSL) to their distribution's trash. Stops at the first item
/// that cannot go there; those moved before stay in the trash.
pub fn trash(paths: &[PathBuf]) -> Result<()> {
    for p in paths {
        let (files, info, w) = trash_for(p).map_err(CoreError::Trash)?;
        let fail = |what: &str, e: std::io::Error| {
            CoreError::Trash(format!(
                "cannot move “{}” to the Linux trash ({what}: {e})",
                w.linux
            ))
        };
        std::fs::create_dir_all(&files).map_err(|e| fail("create trash", e))?;
        std::fs::create_dir_all(&info).map_err(|e| fail("create trash", e))?;
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| CoreError::Trash(format!("“{}” has no name", w.linux)))?;
        let (stem, ext) = super::ops::split_ext(&name);
        let when = chrono::Local::now();
        let mut text = trashinfo(&w.linux, when.naive_local());
        if let Some(nanos) = when.timestamp_nanos_opt() {
            text.push_str(&format!("X-Gall-DeletionTimeNs={nanos}\n"));
        }
        for n in 1.. {
            let candidate = if n == 1 {
                name.clone()
            } else {
                format!("{stem}.{n}{ext}")
            };
            let info_file = info.join(format!("{candidate}.trashinfo"));
            // The info file is created first, exclusively: it reserves the name.
            let created = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&info_file);
            let mut f = match created {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(fail("write trash info", e)),
            };
            let target = files.join(&candidate);
            if std::fs::symlink_metadata(&target).is_ok() {
                drop(f);
                let _ = std::fs::remove_file(&info_file);
                continue;
            }
            use std::io::Write;
            let written = f.write_all(text.as_bytes()).and_then(|()| f.sync_all());
            drop(f);
            let moved = written.and_then(|()| std::fs::rename(p, &target));
            if let Err(e) = moved {
                let _ = std::fs::remove_file(&info_file);
                return Err(fail("move", e));
            }
            break;
        }
    }
    Ok(())
}

/// Puts items trashed by [`trash`] between `at` and `until` (epoch milliseconds) back.
/// For each path the newest matching item is restored. A path that is taken again does
/// not stop the others: they are restored, then the first taken one is reported.
pub fn restore(paths: &[PathBuf], at: i64, until: i64) -> Result<()> {
    use chrono::TimeZone;
    let mut missing = 0;
    let mut taken: Option<PathBuf> = None;
    for p in paths {
        let Ok((files, info, w)) = trash_for(p) else {
            missing += 1;
            continue;
        };
        let mut best: Option<(i64, String)> = None;
        for e in std::fs::read_dir(&info).into_iter().flatten().flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            let Some(name) = file.strip_suffix(".trashinfo") else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            let Some((path, date)) = parse_trashinfo(&text) else {
                continue;
            };
            // An hour that happens twice (end of summer time) may be either one.
            let local = chrono::Local.from_local_datetime(&date);
            let in_window = |t: i64| t >= at - 2000 && t <= until + 2000;
            let Some(t) = [local.earliest(), local.latest()]
                .into_iter()
                .flatten()
                .map(|t| t.timestamp_millis())
                .find(|t| in_window(*t))
                .or_else(|| local.earliest().map(|t| t.timestamp_millis()))
            else {
                continue;
            };
            let order = text
                .lines()
                .find_map(|line| line.strip_prefix("X-Gall-DeletionTimeNs="))
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or_else(|| t.saturating_mul(1_000_000));
            // Standard entries have second precision; Gall's extra field breaks ties.
            if path == w.linux && in_window(t) && best.as_ref().is_none_or(|(b, _)| order > *b) {
                best = Some((order, name.to_string()));
            }
        }
        let Some((_, name)) = best else {
            missing += 1;
            continue;
        };
        if std::fs::symlink_metadata(p).is_ok() {
            taken.get_or_insert_with(|| p.clone());
            continue;
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::io("create folder", parent, e))?;
        }
        std::fs::rename(files.join(&name), p).map_err(|e| CoreError::io("restore", p, e))?;
        let _ = std::fs::remove_file(info.join(format!("{name}.trashinfo")));
    }
    if let Some(p) = taken {
        return Err(CoreError::AlreadyExists(p));
    }
    if missing > 0 {
        return Err(CoreError::Trash(format!(
            "{} no longer in the Linux trash",
            if missing == 1 {
                "1 item is".to_string()
            } else {
                format!("{missing} items are")
            }
        )));
    }
    Ok(())
}

// ───────────────────────────── history ─────────────────────────────

/// False for a path in a distribution that is not known to be running: looking at it
/// would start the distribution (slow), so background checks (history) leave it alone.
pub fn may_touch(p: &Path) -> bool {
    locate(p).is_none_or(|w| !cfg!(windows) || is_running(&w.distro))
}

// ───────────────────────────── Gall itself inside WSL ─────────────────────────────

/// Mounts WSL adds inside a distribution for its own use (GPU drivers, WSLg): no
/// storage for the user. Gall running inside WSL leaves them out of the drive list.
pub fn hidden_mount(mount: &str) -> bool {
    ["/mnt/wsl", "/mnt/wslg", "/usr/lib/wsl", "/init"]
        .iter()
        .any(|p| mount == *p || mount.starts_with(&format!("{p}/")))
}

/// `/mnt/c` mounted by WSL (9p or drvfs) → "C:", how the drive is known on Windows.
pub fn drvfs_letter(mount: &str, fs: &str) -> Option<String> {
    if !matches!(fs, "9p" | "drvfs" | "v9fs") {
        return None;
    }
    let letter = mount.strip_prefix("/mnt/")?;
    let mut chars = letter.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(format!("{}:", c.to_ascii_uppercase())),
        _ => None,
    }
}

// ───────────────────────────── detection (cached) ─────────────────────────────

/// How long a detection stays fresh. The drive list asks every few seconds; WSL is only
/// asked again in the background once this has passed.
#[cfg(windows)]
const FRESH: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Default)]
struct Cache {
    distros: Vec<Distro>,
    #[cfg(windows)]
    at: Option<std::time::Instant>,
    #[cfg(windows)]
    refreshing: bool,
    /// Distributions started (or seen running) in this session.
    started: Vec<String>,
}

static CACHE: std::sync::Mutex<Cache> = std::sync::Mutex::new(Cache {
    distros: Vec::new(),
    #[cfg(windows)]
    at: None,
    #[cfg(windows)]
    refreshing: false,
    started: Vec::new(),
});

/// The installed distributions, as last detected. Never waits for WSL: the first call
/// (and any call once the result is old) starts a detection in the background and
/// returns what is known, so the drive list stays fast. Empty without WSL and on other
/// systems.
pub fn distros() -> Vec<Distro> {
    #[cfg(windows)]
    {
        let mut c = CACHE.lock().unwrap();
        let stale = c.at.is_none_or(|t| t.elapsed() >= FRESH);
        if stale && !c.refreshing {
            c.refreshing = true;
            let spawned = std::thread::Builder::new()
                .name("wsl-detect".into())
                .spawn(|| {
                    let found = detect();
                    let mut c = CACHE.lock().unwrap();
                    for d in found.iter().filter(|d| d.state == DistroState::Running) {
                        if !c.started.iter().any(|s| s.eq_ignore_ascii_case(&d.name)) {
                            c.started.push(d.name.clone());
                        }
                    }
                    c.distros = found;
                    c.at = Some(std::time::Instant::now());
                    c.refreshing = false;
                });
            if spawned.is_err() {
                c.refreshing = false;
            }
        }
        c.distros.clone()
    }
    #[cfg(not(windows))]
    {
        CACHE.lock().unwrap().distros.clone()
    }
}

/// The mounted distributions as rows of the drive list.
pub fn drives() -> Vec<Drive> {
    let all = distros();
    as_drives(&all, &CACHE.lock().unwrap().started)
}

/// The distributions that are not mounted yet, for "Not mounted".
pub fn volumes() -> Vec<super::volumes::Volume> {
    let all = distros();
    as_volumes(&all, &CACHE.lock().unwrap().started)
}

/// The default distribution (name, host), for Linux paths typed in the path bar.
pub fn default_distro() -> Option<(String, Host)> {
    let c = CACHE.lock().unwrap();
    c.distros
        .iter()
        .find(|d| d.default)
        .or(c.distros.first())
        .and_then(|d| parse_unc(&d.root).map(|w| (d.name.clone(), w.host)))
}

fn is_running(distro: &str) -> bool {
    let c = CACHE.lock().unwrap();
    c.started.iter().any(|s| s.eq_ignore_ascii_case(distro))
        || c.distros
            .iter()
            .any(|d| d.name.eq_ignore_ascii_case(distro) && d.state == DistroState::Running)
}

/// After a distribution was stopped: back under "Not mounted".
#[cfg_attr(not(windows), allow(dead_code))]
fn mark_stopped(distro: &str) {
    let mut c = CACHE.lock().unwrap();
    c.started.retain(|s| !s.eq_ignore_ascii_case(distro));
    for d in c
        .distros
        .iter_mut()
        .filter(|d| d.name.eq_ignore_ascii_case(distro))
    {
        d.state = DistroState::Stopped;
    }
}

/// "Eject" on a distribution: stops it (`wsl --terminate`), like unmounting a volume.
/// It moves back under "Not mounted"; opening it again starts it.
pub fn terminate(distro: &str) -> Result<()> {
    if !valid_distro_name(distro) {
        return Err(CoreError::Other(format!(
            "“{distro}” is not a WSL distribution name"
        )));
    }
    #[cfg(windows)]
    {
        match win::run(&["--terminate", distro], std::time::Duration::from_secs(20)) {
            Ok((true, _, _)) => {}
            Ok((false, out, err)) => {
                let why = if err.trim().is_empty() { out } else { err };
                let why = why.split_whitespace().collect::<Vec<_>>().join(" ");
                return Err(CoreError::Other(format!(
                    "WSL distribution “{distro}” could not be stopped: {why}"
                )));
            }
            Err(e) => {
                return Err(CoreError::Other(format!(
                    "WSL distribution “{distro}” could not be stopped: {e}"
                )))
            }
        }
        mark_stopped(distro);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(CoreError::Other(format!(
            "WSL distribution “{distro}” can only be stopped on Windows"
        )))
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn mark_started(distro: &str) {
    let mut c = CACHE.lock().unwrap();
    if !c.started.iter().any(|s| s.eq_ignore_ascii_case(distro)) {
        c.started.push(distro.to_string());
    }
    for d in c
        .distros
        .iter_mut()
        .filter(|d| d.name.eq_ignore_ascii_case(distro))
    {
        d.state = DistroState::Running;
    }
}

/// How long starting a distribution may take before Gall gives up with a message.
pub const START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Before listing `path`: when it is in a distribution that is not running, starts it
/// (at most [`START_TIMEOUT`]) so the user gets a clear message instead of a listing
/// that hangs or fails with "network path not found". Free for every other path.
pub fn prepare(path: &Path) -> Result<()> {
    if !cfg!(windows) {
        return Ok(());
    }
    match parse_unc(&path.to_string_lossy()) {
        Some(w) if !is_running(&w.distro) => start(&w.distro),
        _ => Ok(()),
    }
}

/// Starts distribution `distro` (does nothing when it already runs).
pub fn start(distro: &str) -> Result<()> {
    // The name comes from a path or the UI: never hand wsl.exe an option (`--help`).
    if !valid_distro_name(distro) {
        return Err(CoreError::Other(format!(
            "“{distro}” is not a WSL distribution name"
        )));
    }
    #[cfg(windows)]
    {
        win::start(distro, START_TIMEOUT)?;
        mark_started(distro);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(CoreError::Other(format!(
            "WSL distribution “{distro}” can only be started on Windows"
        )))
    }
}

/// Asks Windows for the installed distributions now (registry, then `wsl.exe` for which
/// ones run, with a timeout). Empty when WSL is not installed or not on Windows.
pub fn detect() -> Vec<Distro> {
    #[cfg(windows)]
    {
        win::detect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const LXSS: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";

    /// `wsl.exe --list --running --quiet` is quick once WSL runs; the first call of a
    /// session can take a moment while the service starts.
    const LIST_TIMEOUT: Duration = Duration::from_secs(5);

    /// Runs `wsl.exe args`, without a console window, for at most `timeout`:
    /// (exit code 0, stdout, stderr), or a message when it could not start or took too long.
    pub(super) fn run(
        args: &[&str],
        timeout: Duration,
    ) -> std::result::Result<(bool, String, String), String> {
        let mut child = Command::new(wsl_exe())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("cannot run wsl.exe: {e}"))?;
        // Read both pipes on their own threads: a full pipe would block wsl.exe.
        let pipe = |r: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if let Some(mut r) = r {
                    let _ = r.read_to_end(&mut buf);
                }
                buf
            })
        };
        let out = pipe(
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
        );
        let err = pipe(
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
        );
        let t0 = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if t0.elapsed() < timeout => std::thread::sleep(Duration::from_millis(25)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("timeout".into());
                }
                Err(e) => return Err(format!("wsl.exe: {e}")),
            }
        };
        let out = decode_output(&out.join().unwrap_or_default());
        let err = decode_output(&err.join().unwrap_or_default());
        Ok((status.success(), out, err))
    }

    fn build_number() -> u32 {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// The registered distributions and the default's GUID; None when WSL has never
    /// been set up for this user (no Lxss key).
    fn registry() -> Option<(Vec<RegDistro>, String)> {
        let lxss = RegKey::predef(HKEY_CURRENT_USER).open_subkey(LXSS).ok()?;
        let default: String = lxss.get_value("DefaultDistribution").unwrap_or_default();
        let entries = lxss
            .enum_keys()
            .flatten()
            .filter_map(|guid| {
                let k = lxss.open_subkey(&guid).ok()?;
                Some(RegDistro {
                    name: k.get_value("DistributionName").ok()?,
                    state: k.get_value("State").unwrap_or(1),
                    version: k.get_value("Version").unwrap_or(0),
                    guid,
                })
            })
            .collect();
        Some((entries, default))
    }

    /// The user id `wsl.exe` logs in as in `distro` (`DefaultUid`, set by the installer).
    pub(super) fn default_uid(distro: &str) -> Option<u32> {
        let lxss = RegKey::predef(HKEY_CURRENT_USER).open_subkey(LXSS).ok()?;
        lxss.enum_keys().flatten().find_map(|guid| {
            let k = lxss.open_subkey(&guid).ok()?;
            let name: String = k.get_value("DistributionName").ok()?;
            if name.eq_ignore_ascii_case(distro) {
                k.get_value("DefaultUid").ok()
            } else {
                None
            }
        })
    }

    /// The host that works: `\\wsl.localhost` or `\\wsl$`. Checked once on a running
    /// distribution (looking at a stopped one would start it); until then by build.
    fn host(running: Option<&str>) -> Host {
        static CHECKED: std::sync::OnceLock<Host> = std::sync::OnceLock::new();
        if let Some(h) = CHECKED.get() {
            return *h;
        }
        let guess = host_for_build(build_number());
        let Some(name) = running else {
            return guess;
        };
        let ok = |h: Host| Path::new(&root(name, h)).is_dir();
        let h = if ok(guess) {
            guess
        } else if ok(guess.other()) {
            guess.other()
        } else {
            return guess;
        };
        *CHECKED.get_or_init(|| h)
    }

    pub(super) fn detect() -> Vec<Distro> {
        let Some((entries, default)) = registry() else {
            return Vec::new(); // WSL not set up: nothing is started
        };
        if entries.is_empty() {
            return Vec::new();
        }
        let running = match run(&["--list", "--running", "--quiet"], LIST_TIMEOUT) {
            // Exit code 1 with a message when none runs.
            Ok((_, out, _)) => Some(parse_list_quiet(&out)),
            Err(e) => {
                log::info!("wsl.exe --list --running: {e}");
                None
            }
        };
        let first_running = running.as_ref().and_then(|r| r.first().cloned());
        let host = host(first_running.as_deref());
        let found = distros_from_registry(&entries, &default, running.as_deref(), host);
        if !found.is_empty() {
            return found;
        }
        // Registered under another layout: ask wsl.exe for the list itself.
        match run(&["--list", "--verbose"], LIST_TIMEOUT) {
            Ok((true, out, _)) => distros_from_list(&parse_list_verbose(&out), host),
            _ => Vec::new(),
        }
    }

    pub(super) fn start(distro: &str, timeout: Duration) -> Result<()> {
        if let Some((entries, _)) = registry() {
            if !entries.is_empty() && !entries.iter().any(|e| e.name.eq_ignore_ascii_case(distro)) {
                return Err(CoreError::Other(format!(
                    "WSL distribution “{distro}” is not installed (anymore)"
                )));
            }
        }
        match run(
            &["-d", distro, "--exec", "/bin/sh", "-c", "exit 0"],
            timeout,
        ) {
            Ok((true, _, _)) => Ok(()),
            Ok((false, out, err)) => {
                let why = if err.trim().is_empty() { out } else { err };
                let why = why.split_whitespace().collect::<Vec<_>>().join(" ");
                Err(CoreError::Other(format!(
                    "WSL distribution “{distro}” could not be started: {why}"
                )))
            }
            Err(e) if e == "timeout" => Err(CoreError::Other(format!(
                "WSL distribution “{distro}” did not start within {} seconds. Start it once \
                 with “wsl -d {distro}” in a terminal, then open it again.",
                timeout.as_secs()
            ))),
            Err(e) => Err(CoreError::Other(format!(
                "WSL distribution “{distro}” could not be started: {e}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str, bom: bool) -> Vec<u8> {
        let mut v = if bom { vec![0xFF, 0xFE] } else { Vec::new() };
        for u in s.encode_utf16() {
            v.extend(u.to_le_bytes());
        }
        v
    }

    const VERBOSE: &str = "  NAME                   STATE           VERSION\r\n\
                           * Ubuntu-22.04           Running         2\r\n\
                           \x20 Debian                 Stopped         2\r\n\
                           \x20 docker-desktop-data    Stopped         2\r\n\
                           \x20 Legacy                 Stopped         1\r\n";

    #[test]
    fn decodes_utf16_with_and_without_bom_and_utf8() {
        let s = "Ubuntu\r\nDebian\r\n";
        assert_eq!(decode_output(&utf16(s, true)), s);
        assert_eq!(decode_output(&utf16(s, false)), s);
        assert_eq!(decode_output(s.as_bytes()), s);
        let mut bom8 = vec![0xEF, 0xBB, 0xBF];
        bom8.extend(s.as_bytes());
        assert_eq!(decode_output(&bom8), s);
        // Non-ASCII text (a translated message) in UTF-16 with a mark.
        assert_eq!(
            decode_output(&utf16("Wird ausgeführt", true)),
            "Wird ausgeführt"
        );
        assert_eq!(decode_output(b""), "");
        // An odd trailing byte is dropped, not a panic.
        let mut odd = utf16("Ubuntu", false);
        odd.push(b'x');
        assert_eq!(decode_output(&odd), "Ubuntu");
    }

    #[test]
    fn parses_quiet_list() {
        let out = decode_output(&utf16("Ubuntu\r\nDebian\r\n\r\nkali-linux\r\n", true));
        assert_eq!(parse_list_quiet(&out), ["Ubuntu", "Debian", "kali-linux"]);
        // No running distribution: a message, not names.
        assert!(parse_list_quiet("There are no running distributions.\r\n").is_empty());
        assert!(parse_list_quiet("").is_empty());
    }

    #[test]
    fn parses_verbose_list_in_any_language() {
        let rows = parse_list_verbose(&decode_output(&utf16(VERBOSE, false)));
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows[0],
            Listed {
                name: "Ubuntu-22.04".into(),
                default: true,
                running: Some(true),
                version: 2
            }
        );
        assert_eq!(rows[1].running, Some(false));
        assert_eq!(rows[3].version, 1);
        // German: header and states translated, states with spaces.
        let de = "  NAME      STATUS           VERSION\n* Ubuntu    Wird ausgeführt  2\n  Debian    Beendet          2\n";
        let rows = parse_list_verbose(de);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].default, rows[0].running), (true, None));
        let ds = distros_from_list(&parse_list_verbose(VERBOSE), Host::Dollar);
        assert_eq!(ds.len(), 3, "docker-desktop-data is hidden");
        assert_eq!(ds[0].root, r"\\wsl$\Ubuntu-22.04\");
        assert_eq!(ds[0].state, DistroState::Running);
    }

    #[test]
    fn registry_entries_become_distros() {
        let e = |guid: &str, name: &str, state: u32| RegDistro {
            guid: guid.into(),
            name: name.into(),
            state,
            version: 2,
        };
        let entries = [
            e("{b}", "Debian", 1),
            e("{a}", "Ubuntu", 1),
            e("{c}", "Installing", 3),
            e("{d}", "docker-desktop", 1),
            e("{e}", "bad name", 1),
        ];
        let running = vec!["ubuntu".to_string()];
        let ds = distros_from_registry(&entries, "{A}", Some(&running), Host::Localhost);
        let names: Vec<_> = ds.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Ubuntu", "Debian"], "default first");
        assert!(ds[0].default);
        assert_eq!(ds[0].state, DistroState::Running);
        assert_eq!(ds[1].state, DistroState::Stopped);
        assert_eq!(ds[0].root, r"\\wsl.localhost\Ubuntu\");
        let unknown = distros_from_registry(&entries, "", None, Host::Dollar);
        assert!(unknown
            .iter()
            .all(|d| d.state == DistroState::Unknown && !d.default));
        // Running: a drive row (no size). Stopped: under "Not mounted" until started.
        let rows = as_drives(&ds, &[]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].removable, "Eject stops it");
        assert_eq!(rows[0].short, "Ubuntu");
        assert_eq!(rows[0].mount, r"\\wsl.localhost\Ubuntu\");
        assert_eq!((rows[0].total, rows[0].fs.as_str()), (0, "wsl"));
        let waiting = as_volumes(&ds, &[]);
        assert_eq!(waiting.len(), 1);
        assert_eq!(
            (waiting[0].label.as_str(), waiting[0].id.as_str()),
            ("Debian", r"\\wsl.localhost\Debian\")
        );
        assert_eq!(waiting[0].fs, "wsl");
        // Started in this session (also when it stopped again since): stays a drive.
        let started = vec!["debian".to_string()];
        assert_eq!(as_drives(&ds, &started).len(), 2);
        assert!(as_volumes(&ds, &started).is_empty());
        // State unknown (wsl.exe did not answer): not mounted.
        assert!(as_drives(&unknown, &[]).is_empty());
        assert_eq!(as_volumes(&unknown, &[]).len(), 2);
    }

    #[test]
    fn start_refuses_names_that_are_no_distribution() {
        for bad in ["--help", "-d", "a b", ""] {
            let err = start(bad).unwrap_err().to_string();
            assert!(err.contains("is not a WSL distribution name"), "{err}");
            let err = terminate(bad).unwrap_err().to_string();
            assert!(err.contains("is not a WSL distribution name"), "{err}");
        }
    }

    #[test]
    fn passwd_and_trashinfo() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nmatej:x:1000:1000:Matej,,,:/home/matej:/bin/zsh\nodd\n";
        assert_eq!(
            home_from_passwd(passwd, 1000).as_deref(),
            Some("/home/matej")
        );
        assert_eq!(home_from_passwd(passwd, 0).as_deref(), Some("/root"));
        assert_eq!(home_from_passwd(passwd, 1001), None);
        let path = "/home/u/čebela & co/50%.txt";
        let enc = encode_trash_path(path);
        assert_eq!(enc, "/home/u/%C4%8Debela%20%26%20co/50%25.txt");
        assert_eq!(decode_trash_path(&enc), path);
        assert_eq!(decode_trash_path("/a%2"), "/a%2", "a cut escape stays");
        assert_eq!(decode_trash_path("/%zz%C4%8D"), "/%zzč");
        let when = chrono::NaiveDate::from_ymd_opt(2026, 11, 5)
            .unwrap()
            .and_hms_opt(7, 30, 1)
            .unwrap();
        let text = trashinfo(path, when);
        assert_eq!(
            text,
            format!("[Trash Info]\nPath={enc}\nDeletionDate=2026-11-05T07:30:01\n")
        );
        assert_eq!(parse_trashinfo(&text), Some((path.to_string(), when)));
        assert_eq!(parse_trashinfo("[Trash Info]\nPath=/x\n"), None);
    }

    #[test]
    fn host_by_build() {
        assert_eq!(host_for_build(22631), Host::Localhost);
        assert_eq!(host_for_build(19045), Host::Dollar);
        assert_eq!(host_for_build(0), Host::Dollar);
        assert_eq!(Host::Dollar.other(), Host::Localhost);
    }

    #[test]
    fn unc_and_linux_paths_convert_both_ways() {
        let w = parse_unc(r"\\wsl.localhost\Ubuntu\home\u").unwrap();
        assert_eq!(
            (w.distro.as_str(), w.linux.as_str(), w.host),
            ("Ubuntu", "/home/u", Host::Localhost)
        );
        assert_eq!(parse_unc(r"\\wsl$\Debian").unwrap().linux, "/");
        assert_eq!(parse_unc(r"\\WSL$\Debian\").unwrap().linux, "/");
        assert_eq!(
            parse_unc("//wsl.localhost/Ubuntu/etc/").unwrap().linux,
            "/etc"
        );
        assert_eq!(
            parse_unc(r"\\?\UNC\wsl.localhost\Ubuntu\tmp")
                .unwrap()
                .linux,
            "/tmp"
        );
        assert_eq!(parse_unc(r"\\server\share\x"), None);
        assert_eq!(parse_unc(r"\\wsl.localhost\"), None);
        assert_eq!(parse_unc(r"C:\Users"), None);
        assert_eq!(parse_unc("/home/u"), None);
        assert_eq!(
            to_unc("Ubuntu", "/home/u", Host::Localhost),
            r"\\wsl.localhost\Ubuntu\home\u"
        );
        assert_eq!(to_unc("Ubuntu", "/", Host::Dollar), r"\\wsl$\Ubuntu\");
        assert_eq!(
            to_unc("Ubuntu", "/home//u/", Host::Dollar),
            r"\\wsl$\Ubuntu\home\u"
        );
        // Characters Windows does not allow travel as private-use ones.
        let unc = to_unc("Ubuntu", "/srv/a:b?", Host::Localhost);
        assert_eq!(unc, "\\\\wsl.localhost\\Ubuntu\\srv\\a\u{F03A}b\u{F03F}");
        assert_eq!(parse_unc(&unc).unwrap().linux, "/srv/a:b?");
    }

    #[test]
    fn names_ending_in_a_dot_get_the_verbatim_spelling() {
        assert_eq!(
            verbatim(r"\\wsl.localhost\Ubuntu\home\u\name.").as_deref(),
            Some(r"\\?\UNC\wsl.localhost\Ubuntu\home\u\name.")
        );
        assert_eq!(
            verbatim(r"\\wsl$\Ubuntu\x ").as_deref(),
            Some(r"\\?\UNC\wsl$\Ubuntu\x ")
        );
        assert_eq!(verbatim(r"\\wsl$\Ubuntu\home\u\name.txt"), None);
        assert_eq!(
            verbatim(r"\\?\UNC\wsl$\Ubuntu\name."),
            None,
            "already verbatim"
        );
        assert_eq!(verbatim(r"\\server\share\name."), None);
        assert_eq!(io_path(Path::new("/tmp/x.")), Path::new("/tmp/x."));
    }

    #[test]
    fn typed_linux_paths_resolve_in_the_open_distro() {
        let base = r"\\wsl$\Debian\home";
        assert_eq!(
            resolve_typed("/etc", Some(base), None).as_deref(),
            Some(r"\\wsl$\Debian\etc")
        );
        assert_eq!(
            resolve_typed("/etc", Some(r"C:\Users"), Some(("Ubuntu", Host::Localhost))).as_deref(),
            Some(r"\\wsl.localhost\Ubuntu\etc")
        );
        assert_eq!(resolve_typed("/etc", None, None), None);
        assert_eq!(resolve_typed(r"C:\x", Some(base), None), None);
        assert_eq!(resolve_typed("//srv/share", Some(base), None), None);
    }

    #[test]
    fn linux_copy_plans() {
        let a = fake_distro("PlanA");
        let b = fake_distro("PlanB");
        let same = linux_copy_plan(
            &a.path().join("home/u/x"),
            &a.path().join("srv/x (2)"),
            ".tmp",
        )
        .unwrap();
        assert_eq!(
            same.commands(),
            [(
                "PlanA".to_string(),
                [
                    "cp",
                    "-a",
                    "--reflink=auto",
                    "-T",
                    "--",
                    "/home/u/x",
                    "/srv/x (2)"
                ]
                .map(String::from)
                .to_vec()
            )]
        );
        let across =
            linux_copy_plan(&a.path().join("home/u/x"), &b.path().join("srv/y"), ".tmp").unwrap();
        assert_eq!(
            across,
            LinuxCopy::Across {
                from: "PlanA".into(),
                src_dir: "/home/u".into(),
                name: "x".into(),
                to: "PlanB".into(),
                tmp_dir: "/srv/.tmp".into()
            }
        );
        let cmds = across.commands();
        assert_eq!(cmds[0].1, ["tar", "-C", "/home/u", "-cf", "-", "--", "x"]);
        assert_eq!(
            cmds[1],
            (
                "PlanB".to_string(),
                ["tar", "-C", "/srv/.tmp", "-xpf", "-"]
                    .map(String::from)
                    .to_vec()
            )
        );
        // At the top level, and not both inside WSL: no plan.
        let top = linux_copy_plan(&a.path().join("x"), &b.path().join("x"), ".t").unwrap();
        assert!(
            matches!(top, LinuxCopy::Across { ref src_dir, ref tmp_dir, .. } if src_dir == "/" && tmp_dir == "/.t")
        );
        assert_eq!(linux_copy_plan(a.path(), &b.path().join("x"), ".t"), None);
        assert_eq!(
            linux_copy_plan(
                &std::env::temp_dir().join("not-wsl-gall"),
                &b.path().join("x"),
                ".t"
            ),
            None
        );
    }

    #[test]
    fn paths_as_linux_sees_them() {
        assert_eq!(
            linux_path_for(r"\\wsl.localhost\Ubuntu\home\u").as_deref(),
            Some("/home/u")
        );
        assert_eq!(linux_path_for(r"\\wsl$\Debian\").as_deref(), Some("/"));
        assert_eq!(
            linux_path_for(r"C:\Users\me\a b").as_deref(),
            Some("/mnt/c/Users/me/a b")
        );
        assert_eq!(linux_path_for(r"D:\").as_deref(), Some("/mnt/d"));
        assert_eq!(linux_path_for(r"\\?\E:\x").as_deref(), Some("/mnt/e/x"));
        assert_eq!(
            linux_path_for("C:\\a\u{F03A}b").as_deref(),
            Some("/mnt/c/a:b")
        );
        assert_eq!(linux_path_for("/home/u").as_deref(), Some("/home/u"));
        assert_eq!(linux_path_for(r"\\server\share\x"), None);
        assert_eq!(
            crate::nav::path::format_path(r"C:\Users", crate::nav::path::PathFormat::Linux),
            "/mnt/c/Users"
        );
        assert_eq!(
            crate::nav::path::format_path(r"\\srv\s", crate::nav::path::PathFormat::Linux),
            r"\\srv\s"
        );
    }

    #[test]
    fn terminal_opens_wsl_in_the_linux_folder() {
        let argv = terminal_argv(r"\\wsl.localhost\Ubuntu\home\u\my dir", false).unwrap();
        assert_eq!(argv, ["wsl.exe", "-d", "Ubuntu", "--cd", "/home/u/my dir"]);
        let argv = terminal_argv(r"\\wsl$\Debian\srv\a;b", true).unwrap();
        assert_eq!(
            argv,
            ["wt.exe", "wsl.exe", "-d", "Debian", "--cd", r"/srv/a\;b"]
        );
        assert_eq!(terminal_argv(r"C:\Users", true), None);
    }

    #[test]
    fn windows_name_problems() {
        assert_eq!(windows_name_problem("notes.txt"), None);
        assert_eq!(windows_name_problem("a:b"), Some(NameProblem::Char(':')));
        assert_eq!(
            windows_name_problem("a\u{F02A}b"),
            Some(NameProblem::Char('*')),
            "as Windows lists it"
        );
        for n in ["q?", "\"x\"", "<a>", "a|b", "back\\slash", "tab\there"] {
            assert!(
                matches!(windows_name_problem(n), Some(NameProblem::Char(_))),
                "{n}"
            );
        }
        assert_eq!(
            windows_name_problem("dots."),
            Some(NameProblem::TrailingDotOrSpace)
        );
        assert_eq!(
            windows_name_problem("space "),
            Some(NameProblem::TrailingDotOrSpace)
        );
        for n in ["CON", "con", "nul.txt", "Com1", "lpt9.log", "aux.tar.gz"] {
            assert_eq!(windows_name_problem(n), Some(NameProblem::Reserved), "{n}");
        }
        for n in ["COM0", "console", "CONx.txt", ".bashrc", "nul_", "x.con"] {
            assert_eq!(windows_name_problem(n), None, "{n}");
        }
        assert!(unreachable_name("a."));
        assert!(!unreachable_name(".hidden"));
        assert!(!unreachable_name(".."));
        assert_eq!(linux_name("a\u{F03A}b"), "a:b");
        assert_eq!(windows_name("a:b"), "a\u{F03A}b");
        assert_eq!(
            linux_name("\u{F041}"),
            "\u{F041}",
            "only characters Windows forbids"
        );
    }

    #[test]
    fn case_clashes_are_found() {
        let names: Vec<String> = ["README", "src", "Readme", "readme", "SRC2", "ŽABA", "žaba"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            case_clashes(&names),
            [
                ("Readme".to_string(), "README".to_string()),
                ("readme".to_string(), "README".to_string()),
                ("žaba".to_string(), "ŽABA".to_string()),
            ]
        );
        assert!(case_clashes(&["a".into(), "b".into()]).is_empty());
    }

    #[test]
    fn mounts_inside_wsl() {
        assert!(hidden_mount("/usr/lib/wsl/drivers"));
        assert!(hidden_mount("/mnt/wslg/distro"));
        assert!(hidden_mount("/mnt/wsl"));
        assert!(!hidden_mount("/mnt/c"));
        assert!(!hidden_mount("/mnt/wslbackup"));
        assert_eq!(drvfs_letter("/mnt/c", "9p").as_deref(), Some("C:"));
        assert_eq!(drvfs_letter("/mnt/d", "drvfs").as_deref(), Some("D:"));
        assert_eq!(drvfs_letter("/mnt/c", "ext4"), None);
        assert_eq!(drvfs_letter("/mnt/data", "9p"), None);
    }

    /// A temporary folder standing in for `\\wsl.localhost\<distro>\`.
    fn fake_distro(distro: &str) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        register_root(t.path(), distro);
        t
    }

    #[test]
    fn registered_roots_count_as_wsl() {
        let t = fake_distro("Fake");
        let p = t.path().join("home").join("u");
        let w = locate(&p).unwrap();
        assert_eq!((w.distro.as_str(), w.linux.as_str()), ("Fake", "/home/u"));
        assert_eq!(locate(t.path()).unwrap().linux, "/");
        assert!(!is_wsl_path(
            &std::env::temp_dir().join("elsewhere-gall-wsl")
        ));
        assert_eq!(
            no_trash_reason(&p).as_deref(),
            Some("“u” is inside WSL (Fake), which has no Recycle Bin, and its Linux trash was not found")
        );
        assert!(check_path(&p).is_ok());
        assert!(check_path(&t.path().join("x.")).is_err());
    }

    /// Linux-only: these names cannot be created on Windows or macOS test machines.
    #[cfg(target_os = "linux")]
    #[test]
    fn copy_out_of_wsl_refuses_names_windows_cannot_hold() {
        let wsl = fake_distro("Fake");
        let win = tempfile::tempdir().unwrap();
        let proj = wsl.path().join("proj");
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        for f in ["ok.txt", "a:b", "CON", "docs/README", "docs/Readme"] {
            std::fs::write(proj.join(f), f).unwrap();
        }
        let no = AtomicBool::new(false);
        let err = check_task(
            TaskKind::Copy,
            std::slice::from_ref(&proj),
            Some(win.path()),
            &no,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.starts_with("Nothing was copied: cannot copy 3 items inside WSL"),
            "{err}"
        );
        assert!(
            err.contains("“a:b” in /proj (“:” is not allowed on Windows)"),
            "{err}"
        );
        assert!(
            err.contains("“CON” in /proj (a reserved device name on Windows)"),
            "{err}"
        );
        assert!(
            err.contains("“Readme” in /proj/docs (differs from “README” only in case"),
            "{err}"
        );
        assert!(err.contains("copy them to a folder inside WSL"), "{err}");
        // Into another folder inside WSL (or another distribution) those names are fine.
        let other = fake_distro("Other");
        assert!(check_task(
            TaskKind::Copy,
            std::slice::from_ref(&proj),
            Some(other.path()),
            &no
        )
        .is_ok());
        assert!(check_task(
            TaskKind::Move,
            std::slice::from_ref(&proj),
            Some(wsl.path()),
            &no
        )
        .is_ok());
        assert!(check_task(TaskKind::Delete, std::slice::from_ref(&proj), None, &no).is_ok());
        // Sources side by side that differ only in case.
        let (a, b) = (proj.join("docs/README"), proj.join("docs/Readme"));
        let err = check_task(TaskKind::Move, &[a, b], Some(win.path()), &no)
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with("Nothing was moved: cannot move 1 item"),
            "{err}"
        );
        // Into WSL from Windows: nothing to check.
        let w = win.path().join("w.txt");
        std::fs::write(&w, "w").unwrap();
        assert!(check_task(TaskKind::Copy, &[w], Some(&proj), &no).is_ok());
        // Cancelled while checking.
        let yes = AtomicBool::new(true);
        assert!(matches!(
            check_task(
                TaskKind::Copy,
                std::slice::from_ref(&proj),
                Some(win.path()),
                &yes
            ),
            Err(CoreError::Cancelled)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn trailing_dot_names_are_refused_for_every_task() {
        let wsl = fake_distro("Fake");
        let dir = wsl.path().join("d");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("name."), "x").unwrap();
        let no = AtomicBool::new(false);
        let win = tempfile::tempdir().unwrap();
        for (kind, dest) in [(TaskKind::Delete, None), (TaskKind::Copy, Some(win.path()))] {
            let err = check_task(kind, std::slice::from_ref(&dir), dest, &no)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("“name.” in /d/sub (ends with a dot or space"),
                "{err}"
            );
            assert!(err.ends_with("(in a WSL terminal)."), "{err}");
        }
        // Linux copies, duplicates and moves inside WSL: a name below is no problem there.
        for (kind, dest) in [
            (TaskKind::Copy, Some(wsl.path())),
            (TaskKind::Move, Some(wsl.path())),
            (TaskKind::Duplicate, None),
        ] {
            assert!(
                check_task(kind, std::slice::from_ref(&dir), dest, &no).is_ok(),
                "{kind:?}"
            );
        }
        // The source's own name still counts.
        let dot = wsl.path().join("top.");
        std::fs::write(&dot, "x").unwrap();
        assert!(check_task(TaskKind::Duplicate, &[dot], None, &no).is_err());
    }

    mod operations {
        //! Copy, move, delete and undo with a temporary folder as the distribution root:
        //! the same code paths as `\\wsl.localhost\…` on Windows (no Recycle Bin there).
        use super::*;
        use crate::fs::ops::{self, TaskEvent, TaskManager, UndoJournal};
        use std::sync::Arc;

        struct Tasks {
            m: Arc<TaskManager>,
            rx: crossbeam_channel::Receiver<TaskEvent>,
        }

        impl Tasks {
            fn new() -> Self {
                let (tx, rx) = crossbeam_channel::unbounded();
                let on_event = Arc::new(move |e| {
                    let _ = tx.send(e);
                });
                Tasks {
                    m: Arc::new(TaskManager::new(Arc::new(UndoJournal::default()), on_event)),
                    rx,
                }
            }

            fn journal(&self) -> &UndoJournal {
                self.m.journal()
            }

            /// Runs a task to its end: Ok(items done) or Err(the failure message).
            fn run(&self, req: serde_json::Value) -> std::result::Result<u64, String> {
                let id = self
                    .m
                    .start(serde_json::from_value(req).unwrap())
                    .map_err(|e| e.to_string())?;
                loop {
                    match self
                        .rx
                        .recv_timeout(std::time::Duration::from_secs(20))
                        .unwrap()
                    {
                        TaskEvent::Done { id: i, items, .. } if i == id => return Ok(items),
                        TaskEvent::Failed { id: i, error, .. } if i == id => return Err(error),
                        TaskEvent::Cancelled { .. } => return Err("cancelled".into()),
                        _ => {}
                    }
                }
            }
        }

        fn read(p: &Path) -> String {
            std::fs::read_to_string(p).unwrap()
        }

        #[test]
        fn copy_into_wsl_and_undo_without_a_recycle_bin() {
            let wsl = fake_distro("Fake");
            let win = tempfile::tempdir().unwrap();
            let src = win.path().join("report.txt");
            std::fs::write(&src, "r").unwrap();
            let t = Tasks::new();
            let req = serde_json::json!({"kind": "copy", "sources": [src], "dest": wsl.path()});
            assert_eq!(t.run(req), Ok(1));
            let copy = wsl.path().join("report.txt");
            assert_eq!(read(&copy), "r");
            // Undo cannot put the copy in a trash: it says so and keeps the step.
            let err = t.journal().undo().unwrap_err().to_string();
            assert!(
                err.starts_with("no-trash: “report.txt” cannot be moved to the trash"),
                "{err}"
            );
            assert!(
                err.contains("inside WSL (Fake), which has no Recycle Bin"),
                "{err}"
            );
            assert!(copy.exists());
            assert_eq!(t.journal().len(), 1);
            // Agreed to delete for good: the message says so, not "in the trash".
            assert_eq!(
                t.journal().undo_with(true).unwrap(),
                "Deleted “report.txt” for good"
            );
            assert!(!copy.exists() && src.exists());
        }

        #[test]
        fn move_out_of_wsl_and_between_distros_with_undo() {
            let a = fake_distro("A");
            let b = fake_distro("B");
            let win = tempfile::tempdir().unwrap();
            let dir = a.path().join("proj");
            std::fs::create_dir_all(dir.join("src")).unwrap();
            std::fs::write(dir.join("src").join("main.rs"), "fn main() {}").unwrap();
            let t = Tasks::new();
            // WSL → Windows.
            let req = serde_json::json!({"kind": "move", "sources": [dir], "dest": win.path()});
            assert_eq!(t.run(req), Ok(1));
            assert!(!dir.exists());
            assert_eq!(read(&win.path().join("proj/src/main.rs")), "fn main() {}");
            t.journal().undo().unwrap();
            assert_eq!(read(&dir.join("src/main.rs")), "fn main() {}");
            // WSL → WSL (another distribution).
            let req = serde_json::json!({"kind": "move", "sources": [dir], "dest": b.path()});
            assert_eq!(t.run(req), Ok(1));
            assert!(b.path().join("proj/src/main.rs").exists() && !dir.exists());
            assert!(t
                .journal()
                .undo()
                .unwrap()
                .starts_with("Moved “proj” back to"));
            assert!(dir.join("src/main.rs").exists());
        }

        /// A stand-in distribution with a home folder, so its Linux trash works.
        /// Each test uses its own name: the home is remembered per distribution.
        fn distro_with_home(name: &str) -> (tempfile::TempDir, PathBuf) {
            let t = fake_distro(name);
            set_home(name, "/home/u");
            let home = t.path().join("home").join("u");
            std::fs::create_dir_all(&home).unwrap();
            (t, home)
        }

        #[test]
        fn trash_inside_wsl_uses_the_linux_trash_and_undo_restores() {
            let (wsl, home) = distro_with_home("Trash1");
            let docs = home.join("docs");
            std::fs::create_dir(&docs).unwrap();
            let f = docs.join("a b.txt");
            std::fs::write(&f, "one").unwrap();
            let t = Tasks::new();
            ops::trash_items(std::slice::from_ref(&f), t.journal()).unwrap();
            assert!(!f.exists());
            let trash = home.join(".local/share/Trash");
            assert_eq!(read(&trash.join("files/a b.txt")), "one");
            let info = read(&trash.join("info/a b.txt.trashinfo"));
            assert!(
                info.starts_with("[Trash Info]\nPath=/home/u/docs/a%20b.txt\nDeletionDate=20"),
                "{info}"
            );
            // A second item with the same name gets its own name in the trash.
            std::fs::write(&f, "two").unwrap();
            ops::trash_items(std::slice::from_ref(&f), t.journal()).unwrap();
            assert_eq!(read(&trash.join("files/a b.2.txt")), "two");
            // Force the same standard deletion second for both entries.
            let when = chrono::Local::now();
            let nanos = when.timestamp_nanos_opt().unwrap();
            for (name, order) in [("a b.txt", nanos), ("a b.2.txt", nanos + 1)] {
                let text = format!(
                    "{}X-Gall-DeletionTimeNs={order}\n",
                    trashinfo("/home/u/docs/a b.txt", when.naive_local())
                );
                std::fs::write(trash.join("info").join(format!("{name}.trashinfo")), text).unwrap();
            }
            // Undo brings back the newest first, then the older one.
            assert_eq!(t.journal().undo().unwrap(), "Restored “a b.txt”");
            assert_eq!(read(&f), "two");
            assert!(!trash.join("info/a b.2.txt.trashinfo").exists());
            std::fs::remove_file(&f).unwrap();
            t.journal().undo().unwrap();
            assert_eq!(read(&f), "one");
            assert_eq!(std::fs::read_dir(trash.join("files")).unwrap().count(), 0);
            // A folder, and the restore recreates a parent that is gone.
            ops::trash_items(std::slice::from_ref(&docs), t.journal()).unwrap();
            assert!(!docs.exists());
            t.journal().undo().unwrap();
            assert_eq!(read(&f), "one");
            let _ = wsl;
        }

        /// Undo of two trashed items where one name is taken again: the other still
        /// comes back, and the taken one is reported.
        #[test]
        fn restore_goes_on_past_a_taken_name() {
            let (_wsl, home) = distro_with_home("Trash4");
            let (a, b) = (home.join("a.txt"), home.join("b.txt"));
            std::fs::write(&a, "a").unwrap();
            std::fs::write(&b, "b").unwrap();
            let at = chrono::Utc::now().timestamp_millis();
            trash(&[a.clone(), b.clone()]).unwrap();
            let until = chrono::Utc::now().timestamp_millis();
            std::fs::write(&a, "new").unwrap();
            let r = restore(&[a.clone(), b.clone()], at, until);
            assert!(
                matches!(r, Err(CoreError::AlreadyExists(ref p)) if *p == a),
                "{r:?}"
            );
            assert_eq!(read(&a), "new");
            assert_eq!(read(&b), "b");
        }

        #[test]
        fn undo_and_replace_inside_wsl_use_the_linux_trash() {
            let (_wsl, home) = distro_with_home("Trash2");
            let win = tempfile::tempdir().unwrap();
            let src = win.path().join("a.txt");
            std::fs::write(&src, "new").unwrap();
            let old = home.join("a.txt");
            std::fs::write(&old, "old").unwrap();
            let t = Tasks::new();
            let req = serde_json::json!({"kind": "copy", "sources": [src], "dest": home, "conflict": "overwrite"});
            assert_eq!(t.run(req), Ok(1));
            assert_eq!(read(&old), "new");
            // Undo: the copy goes to the Linux trash, the replaced file comes back from it.
            t.journal().undo().unwrap();
            assert_eq!(read(&old), "old");
            // Undo of a plain copy into WSL: the copy goes to the trash.
            let req =
                serde_json::json!({"kind": "copy", "sources": [src], "dest": home.join("..")});
            assert_eq!(t.run(req), Ok(1));
            assert_eq!(
                t.journal().undo().unwrap(),
                "Removed “a.txt” (in the trash)"
            );
            assert!(!home.join("../a.txt").exists());
        }

        #[test]
        fn what_cannot_go_to_the_linux_trash() {
            let (wsl, home) = distro_with_home("Trash3");
            let t = Tasks::new();
            let c = wsl.path().join("mnt/c/Users");
            std::fs::create_dir_all(&c).unwrap();
            let err = ops::trash_items(std::slice::from_ref(&c), t.journal())
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("is on a Windows drive mounted in WSL (/mnt/c)"),
                "{err}"
            );
            for p in [
                home.clone(),
                wsl.path().join("home"),
                home.join(".local/share/Trash"),
            ] {
                std::fs::create_dir_all(&p).unwrap();
                let err = ops::trash_items(std::slice::from_ref(&p), t.journal())
                    .unwrap_err()
                    .to_string();
                assert!(err.ends_with("cannot be moved to the Linux trash"), "{err}");
                assert!(p.exists());
            }
            assert!(t.journal().is_empty());
        }

        #[test]
        fn delete_inside_wsl_is_permanent_and_not_undoable() {
            let wsl = fake_distro("Fake");
            let f = wsl.path().join("old.log");
            std::fs::write(&f, "x").unwrap();
            let t = Tasks::new();
            // Without a Linux trash (no home folder known) the trash refuses up front;
            // nothing is deleted, nothing recorded.
            let err = ops::trash_items(std::slice::from_ref(&f), t.journal())
                .unwrap_err()
                .to_string();
            assert_eq!(
                err,
                "trash: “old.log” is inside WSL (Fake), which has no Recycle Bin, and its Linux trash was not found"
            );
            assert!(f.exists());
            assert!(t.journal().is_empty());
            // The permanent delete the UI then offers: gone, and no undo step pretends otherwise.
            let req = serde_json::json!({"kind": "delete", "sources": [f]});
            assert_eq!(t.run(req), Ok(1));
            assert!(!f.exists());
            assert!(t.journal().is_empty());
        }

        #[test]
        fn replacing_inside_wsl_asks_before_deleting_for_good() {
            let wsl = fake_distro("Fake");
            let win = tempfile::tempdir().unwrap();
            let src = win.path().join("a.txt");
            std::fs::write(&src, "new").unwrap();
            let old = wsl.path().join("a.txt");
            std::fs::write(&old, "old").unwrap();
            let t = Tasks::new();
            let req = |policy: &str| serde_json::json!({"kind": "copy", "sources": [src], "dest": wsl.path(), "conflict": policy});
            let err = t.run(req("overwrite")).unwrap_err();
            assert!(
                err.starts_with("no-trash: “a.txt” was not replaced"),
                "{err}"
            );
            assert_eq!(read(&old), "old");
            assert_eq!(t.run(req("overwritePermanently")), Ok(1));
            assert_eq!(read(&old), "new");
            // Keep both and rename still work as anywhere else.
            assert_eq!(t.run(req("keepBoth")), Ok(1));
            assert_eq!(read(&wsl.path().join("a (2).txt")), "new");
            let renamed = ops::rename(&old, "b.txt", t.journal()).unwrap();
            assert_eq!(read(&renamed), "new");
            t.journal().undo().unwrap();
            assert!(old.exists());
        }

        /// A tree with what a copy through Windows loses: an executable script, a
        /// private file and a symbolic link.
        #[cfg(target_os = "linux")]
        fn linux_tree(dir: &Path) {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            let run = dir.join("bin/run.sh");
            std::fs::write(&run, "#!/bin/sh\necho hi\n").unwrap();
            std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
            let key = dir.join("key");
            std::fs::write(&key, "secret").unwrap();
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::os::unix::fs::symlink("bin/run.sh", dir.join("run")).unwrap();
            std::fs::write(dir.join("a:b"), "linux name").unwrap();
        }

        #[cfg(target_os = "linux")]
        fn assert_linux_tree(dir: &Path) {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |p: &str| std::fs::metadata(dir.join(p)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode("bin/run.sh"), 0o755, "still executable");
            assert_eq!(mode("key"), 0o600, "still private");
            assert_eq!(
                std::fs::read_link(dir.join("run")).unwrap(),
                Path::new("bin/run.sh")
            );
            assert_eq!(read(&dir.join("a:b")), "linux name");
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn copies_inside_a_distribution_keep_permissions_and_links() {
            let wsl = fake_distro("Copy1");
            let proj = wsl.path().join("home/u/proj");
            linux_tree(&proj);
            let t = Tasks::new();
            let dest = wsl.path().join("srv");
            std::fs::create_dir(&dest).unwrap();
            let before = LINUX_COPIES.load(Ordering::Relaxed);
            let req = serde_json::json!({"kind": "copy", "sources": [proj], "dest": dest});
            assert_eq!(t.run(req), Ok(1));
            assert!(
                LINUX_COPIES.load(Ordering::Relaxed) > before,
                "copied by Linux"
            );
            assert_linux_tree(&dest.join("proj"));
            assert_linux_tree(&proj);
            // Next to itself: "proj (2)" (the target has another name than the source).
            let req = serde_json::json!({"kind": "copy", "sources": [proj], "dest": proj.parent().unwrap()});
            assert_eq!(t.run(req), Ok(1));
            assert_linux_tree(&wsl.path().join("home/u/proj (2)"));
            // A single file, and undo as for any copy.
            let script = proj.join("bin/run.sh");
            let req = serde_json::json!({"kind": "copy", "sources": [script], "dest": dest});
            assert_eq!(t.run(req), Ok(1));
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dest.join("run.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
            t.journal().undo_with(true).unwrap();
            assert!(!dest.join("run.sh").exists());
            assert!(std::fs::read_dir(&dest).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".gall-copy")));
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn a_cancelled_linux_copy_leaves_nothing_behind() {
            let a = fake_distro("CancelA");
            let b = fake_distro("CancelB");
            let proj = a.path().join("proj");
            linux_tree(&proj);
            let yes = AtomicBool::new(true);
            for dst in [a.path().join("copy"), b.path().join("copy")] {
                let r = copy_inside_linux(&proj, &dst, &yes, &mut |_| {}, false);
                assert!(matches!(r, Some(Err(CoreError::Cancelled))));
                assert!(!dst.exists());
                let parent = dst.parent().unwrap();
                let left: Vec<_> = std::fs::read_dir(parent)
                    .unwrap()
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with(".gall-copy"))
                    .collect();
                assert!(left.is_empty(), "{left:?}");
            }
            assert_linux_tree(&proj);
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn copies_between_distributions_keep_permissions_and_links() {
            let a = fake_distro("CopyA");
            let b = fake_distro("CopyB");
            let proj = a.path().join("proj");
            linux_tree(&proj);
            let dest = b.path().join("home");
            std::fs::create_dir(&dest).unwrap();
            std::fs::create_dir(dest.join("proj")).unwrap();
            let t = Tasks::new();
            // "proj" exists there: kept both, so the unpacked folder is renamed.
            let before = LINUX_COPIES.load(Ordering::Relaxed);
            let req = serde_json::json!({"kind": "copy", "sources": [proj], "dest": dest});
            assert_eq!(t.run(req), Ok(1));
            assert!(
                LINUX_COPIES.load(Ordering::Relaxed) > before,
                "copied by Linux"
            );
            assert_linux_tree(&dest.join("proj (2)"));
            let names: Vec<String> = std::fs::read_dir(&dest)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert!(
                names.iter().all(|n| !n.starts_with(".gall-copy")),
                "{names:?}"
            );
            // A move between distributions copies the same way, then removes the source.
            let req = serde_json::json!({"kind": "move", "sources": [proj], "dest": b.path()});
            assert_eq!(t.run(req), Ok(1));
            assert_linux_tree(&b.path().join("proj"));
            assert!(!proj.exists());
            t.journal().undo().unwrap();
            assert!(proj.exists());
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn copy_with_linux_only_names_changes_nothing() {
            let wsl = fake_distro("Fake");
            let win = tempfile::tempdir().unwrap();
            let dir = wsl.path().join("d");
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("ok.txt"), "x").unwrap();
            std::fs::write(dir.join("why?.txt"), "x").unwrap();
            let t = Tasks::new();
            for kind in ["copy", "move"] {
                let req = serde_json::json!({"kind": kind, "sources": [dir], "dest": win.path()});
                let err = t.run(req).unwrap_err();
                assert!(
                    err.contains("“why?.txt” in /d (“?” is not allowed on Windows)"),
                    "{err}"
                );
                assert_eq!(std::fs::read_dir(win.path()).unwrap().count(), 0);
                assert!(dir.join("ok.txt").exists());
            }
            assert!(t.journal().is_empty());
            // A rename of an item Windows cannot address is refused too.
            let dot = dir.join("trailing.");
            std::fs::write(&dot, "x").unwrap();
            assert!(ops::rename(&dot, "fixed", t.journal()).is_err());
            assert!(dot.exists());
        }
    }

    #[test]
    fn many_problems_are_summed_up() {
        let problems: Vec<Problem> = (0..MAX_PROBLEMS)
            .map(|i| Problem {
                path: PathBuf::from(format!("/x/f{i}")),
                what: NameProblem::Reserved,
            })
            .collect();
        let msg = report(TaskKind::Copy, &problems, true);
        assert!(msg.contains("cannot copy 50+ items"), "{msg}");
        assert!(msg.contains(", and at least 45 more."), "{msg}");
    }
}
