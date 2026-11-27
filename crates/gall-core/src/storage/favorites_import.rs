//! Read-only importers of the places other file managers keep (WN-56 framework, WN-58 Windows adapters).
//!
//! Every source implements [`Source`]: `detect` is a cheap check that does not read the other manager's data, `read`
//! returns its places in the order that manager shows them. A place that cannot be taken over is returned as
//! [`Kind::Skipped`] with the reason, never dropped silently. Nothing here writes to the other manager's files, and
//! every path and label is data: it is never interpreted, resolved through a shell or executed.
//!
//! Windows adapters:
//! * Quick access: the pinned folders (`System.Home.IsPinned`, which does not depend on the display language, tells them
//!   from the automatically listed frequent folders and recent files). Read with one fixed PowerShell script through
//!   the shell namespace; the script contains no user data.
//! * `%USERPROFILE%\Links`: the shortcuts of the old Explorer favourites. A `.lnk` is parsed as bytes (MS-SHLLINK),
//!   never opened through the shell, so nothing runs and no target is touched.
//!
//! Paths are kept as written (a mapped drive stays `Z:\..`, a UNC path stays `\\server\share\..`, OneDrive folders keep
//! their names) and are not canonicalized, since `canonicalize` would turn a mapped drive into its UNC target.
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The most places one source returns, and the largest shortcut file that is read.
pub const MAX_PLACES: usize = 500;
pub const MAX_LINK_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Folder,
    File,
    /// Not taken over; the reason is shown to the user.
    Skipped(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub path: String,
    pub label: String,
    pub hidden: bool,
    pub kind: Kind,
    /// The path does not exist (now): the checklist greys it out. Only set where the source was read
    /// with `Source::read`; the pure parsers leave it false.
    #[serde(default)]
    pub missing: bool,
}

/// One other file manager (or one list of it) that places can be read from.
pub trait Source {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    /// Whether this source exists on this computer, without reading its data.
    fn detect(&self) -> bool;
    fn read(&self) -> Result<Vec<Place>>;
}

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

fn skipped(label: &str, shown: &str, reason: &str) -> Place {
    Place {
        path: shown.to_owned(),
        label: label.to_owned(),
        hidden: false,
        kind: Kind::Skipped(reason.to_owned()),
        missing: false,
    }
}

/// A virtual folder has no path on disk (`::{CLSID}`, `shell:…`, `This PC`, `Libraries`, the Recycle Bin).
fn virtual_name(path: &str) -> bool {
    let lower = path.trim().to_ascii_lowercase();
    lower.starts_with("::")
        || lower.starts_with("shell:")
        || lower.starts_with("\\\\?\\guid")
        || [
            "this pc",
            "libraries",
            "recycle bin",
            "network",
            "control panel",
        ]
        .contains(&lower.as_str())
}

/// The path as written, tidied: verbatim prefixes removed, a trailing separator dropped (not from `C:\`). `None` for
/// what is no absolute path of a file or folder: empty, NUL, relative, or with a `..` part.
pub fn plain_path(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() || text.contains('\0') {
        return None;
    }
    let unverbatim = text
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| text.to_owned());
    let bytes = unverbatim.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    let unc =
        unverbatim.starts_with(r"\\") && unverbatim.len() > 2 && !unverbatim.starts_with(r"\\.\");
    let unix = unverbatim.starts_with('/');
    if !(drive || unc || unix) {
        return None;
    }
    if unverbatim.split(['\\', '/']).any(|part| part == "..") {
        return None;
    }
    let mut result = unverbatim;
    while result.len() > 3
        && (result.ends_with('\\') || (result.ends_with('/') && result.len() > 1))
    {
        result.pop();
    }
    Some(result)
}

/// A folder, a file or neither; a path that cannot be examined within a moment (a server that does not answer) counts as
/// a folder, which is what a place is. A missing path also counts as a folder: the checklist marks it as missing.
fn kind_of(path: &str) -> Kind {
    let (sender, receiver) = std::sync::mpsc::channel();
    let probe = PathBuf::from(path);
    std::thread::spawn(move || {
        let _ = sender.send(std::fs::metadata(&probe).map(|m| m.is_file()));
    });
    match receiver.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(Ok(true)) => Kind::File,
        _ => Kind::Folder,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Quick access

#[derive(Deserialize)]
struct Pinned {
    #[serde(default)]
    path: String,
    #[serde(default)]
    name: String,
}

/// Places from the JSON the Quick access script prints: an array, or one object (PowerShell prints a single item as an
/// object), or nothing. Virtual folders are reported as skipped.
pub fn parse_quick_access(json: &str) -> Result<Vec<Place>> {
    let text = json.trim().trim_start_matches('\u{feff}');
    if text.is_empty() || text == "null" {
        return Ok(Vec::new());
    }
    let items: Vec<Pinned> = if text.starts_with('[') {
        serde_json::from_str(text)
            .map_err(|e| fail(format!("Quick access could not be read: {e}")))?
    } else {
        vec![serde_json::from_str(text)
            .map_err(|e| fail(format!("Quick access could not be read: {e}")))?]
    };
    let mut places = Vec::new();
    for item in items.into_iter().take(MAX_PLACES) {
        let label = if item.name.trim().is_empty() {
            item.path
                .rsplit(['\\', '/'])
                .find(|part| !part.is_empty())
                .unwrap_or("")
                .to_owned()
        } else {
            item.name.trim().to_owned()
        };
        if virtual_name(&item.path) {
            places.push(skipped(
                &label,
                &item.path,
                "a virtual folder (it has no path on disk)",
            ));
            continue;
        }
        match plain_path(&item.path) {
            Some(path) => {
                let kind = kind_of(&path);
                places.push(Place {
                    path,
                    label,
                    hidden: false,
                    kind,
                    missing: false,
                });
            }
            None => places.push(skipped(
                &label,
                &item.path,
                "not an absolute path of a file or folder",
            )),
        }
    }
    Ok(places)
}

/// The script that lists the pinned items; it has no input of any kind. Output is UTF-8 JSON.
#[cfg(windows)]
const QUICK_ACCESS_SCRIPT: &str = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$ErrorActionPreference = 'Stop'
$items = (New-Object -ComObject Shell.Application).Namespace('shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}').Items()
$out = @()
for ($k = 0; $k -lt $items.Count; $k++) {
  $item = $items.Item($k)
  if ($item.ExtendedProperty('System.Home.IsPinned')) { $out += [pscustomobject]@{ path = [string]$item.Path; name = [string]$item.Name } }
}
ConvertTo-Json -InputObject @($out) -Compress
"#;

/// The encoded form `powershell -EncodedCommand` takes: base64 of the UTF-16LE text.
pub fn encoded_command(script: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Shortcuts (.lnk, MS-SHLLINK)

/// What a shortcut points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A path of a file or folder (local, or UNC for a network location).
    Path(String),
    /// A shell item without a file path (Desktop, Downloads, This PC as ID lists), which is not resolved.
    Shell,
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}
/// A NUL-terminated string at `at`: UTF-16LE when `wide`, else bytes of the system code page (read as Latin-1, which is
/// right for ASCII; modern shortcuts also carry the Unicode form, which is preferred).
fn text_at(data: &[u8], at: usize, wide: bool) -> Option<String> {
    if wide {
        let mut units = Vec::new();
        let mut i = at;
        loop {
            let unit = u16_at(data, i)?;
            if unit == 0 {
                break;
            }
            units.push(unit);
            i += 2;
            if units.len() > 32_768 {
                return None;
            }
        }
        String::from_utf16(&units).ok()
    } else {
        let rest = data.get(at..)?;
        let end = rest.iter().position(|b| *b == 0)?;
        Some(rest[..end].iter().map(|b| char::from(*b)).collect())
    }
}

/// The target of a shortcut file's bytes. Only the LinkInfo block is read (local base path or network share plus the
/// common suffix); the ID list is skipped, so a shortcut to a special folder reports [`Target::Shell`]. Every offset is
/// checked, so a damaged or hostile file is an error, never a panic or an out-of-range read.
pub fn parse_lnk(data: &[u8]) -> Result<Target> {
    let bad = || fail("Not a valid shortcut file");
    if u32_at(data, 0) != Some(0x4C)
        || data.get(4..20)
            != Some(
                &[
                    0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x46,
                ][..],
            )
    {
        return Err(bad());
    }
    let flags = u32_at(data, 20).ok_or_else(bad)?;
    let (has_list, has_info) = (flags & 1 != 0, flags & 2 != 0);
    let mut at = 0x4C;
    if has_list {
        at += 2 + usize::from(u16_at(data, at).ok_or_else(bad)?);
    }
    if !has_info {
        return Ok(Target::Shell);
    }
    let base = at;
    let size = u32_at(data, base).ok_or_else(bad)? as usize;
    if size < 0x1C || base.checked_add(size).is_none_or(|end| end > data.len()) {
        return Err(bad());
    }
    let info = &data[base..base + size];
    let header = u32_at(info, 4).ok_or_else(bad)? as usize;
    let info_flags = u32_at(info, 8).ok_or_else(bad)?;
    let (local, network, suffix) = (
        u32_at(info, 16).ok_or_else(bad)? as usize,
        u32_at(info, 20).ok_or_else(bad)? as usize,
        u32_at(info, 24).ok_or_else(bad)? as usize,
    );
    let (local_wide, suffix_wide) = if header >= 0x24 {
        (
            u32_at(info, 28).unwrap_or(0) as usize,
            u32_at(info, 32).unwrap_or(0) as usize,
        )
    } else {
        (0, 0)
    };
    let tail = if suffix_wide != 0 {
        text_at(info, suffix_wide, true)
    } else {
        text_at(info, suffix, false)
    }
    .unwrap_or_default();
    let join = |head: String| -> String {
        if tail.is_empty() {
            head
        } else if head.ends_with('\\') {
            format!("{head}{tail}")
        } else {
            format!("{head}\\{tail}")
        }
    };
    if info_flags & 1 != 0 {
        let head = if local_wide != 0 {
            text_at(info, local_wide, true)
        } else {
            text_at(info, local, false)
        };
        return head.map(|head| Target::Path(join(head))).ok_or_else(bad);
    }
    if info_flags & 2 != 0 && network != 0 {
        let net = info.get(network..).ok_or_else(bad)?;
        let name_offset = u32_at(net, 8).ok_or_else(bad)? as usize;
        let wide_offset = if name_offset > 0x14 {
            u32_at(net, 20).unwrap_or(0) as usize
        } else {
            0
        };
        let head = if wide_offset != 0 {
            text_at(net, wide_offset, true)
        } else {
            text_at(net, name_offset, false)
        };
        return head.map(|head| Target::Path(join(head))).ok_or_else(bad);
    }
    Ok(Target::Shell)
}

/// The place a shortcut file stands for.
pub fn place_of_link(file_name: &str, data: &[u8], hidden: bool) -> Place {
    let label = file_name
        .strip_suffix(".lnk")
        .or_else(|| file_name.strip_suffix(".LNK"))
        .unwrap_or(file_name)
        .to_owned();
    match parse_lnk(data) {
        Ok(Target::Path(text)) => match plain_path(&text) {
            Some(path) if !virtual_name(&path) => {
                let kind = kind_of(&path);
                Place {
                    path,
                    label,
                    hidden,
                    kind,
                    missing: false,
                }
            }
            _ => skipped(&label, &text, "not an absolute path of a file or folder"),
        },
        Ok(Target::Shell) => skipped(
            &label,
            file_name,
            "a special folder or shell item (Desktop, Downloads…) without a path in the shortcut",
        ),
        Err(error) => skipped(&label, file_name, &error.to_string()),
    }
}

/// Places of the shortcuts in `folder`, in alphabetical order, each file read once and never followed or run.
pub fn read_links_folder(folder: &Path) -> Result<Vec<Place>> {
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(folder)
        .map_err(|e| fail(format!("{} could not be read: {e}", folder.display())))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.to_ascii_lowercase()
                .ends_with(".lnk")
                .then(|| (name, entry.path()))
        })
        .collect();
    files.sort_by_key(|(name, _)| name.to_lowercase());
    let mut places = Vec::new();
    for (name, file) in files.into_iter().take(MAX_PLACES) {
        let label = name
            .trim_end_matches(".lnk")
            .trim_end_matches(".LNK")
            .to_owned();
        let data = match std::fs::symlink_metadata(&file) {
            Ok(meta) if meta.is_file() && meta.len() <= MAX_LINK_BYTES => {
                std::fs::read(&file).map_err(|e| e.to_string())
            }
            Ok(_) => Err("not a regular shortcut file, or too large".to_owned()),
            Err(error) => Err(error.to_string()),
        };
        match data {
            Ok(bytes) => places.push(place_of_link(&name, &bytes, hidden_file(&file))),
            Err(reason) => places.push(skipped(&label, &name, &reason)),
        }
    }
    Ok(places)
}

#[cfg(windows)]
fn hidden_file(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.file_attributes() & 0x2 != 0)
}
#[cfg(not(windows))]
fn hidden_file(_path: &Path) -> bool {
    false
}

// ---------------------------------------------------------------------------------------------------------------
// The Windows sources

#[cfg(windows)]
pub struct QuickAccess;
#[cfg(windows)]
impl Source for QuickAccess {
    fn id(&self) -> &'static str {
        "windows-quick-access"
    }
    fn label(&self) -> &'static str {
        "Quick access (pinned folders)"
    }
    /// Quick access exists on every Windows with Explorer; whether anything is pinned is only known by reading it.
    fn detect(&self) -> bool {
        true
    }
    fn read(&self) -> Result<Vec<Place>> {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        let system = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let program = system.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let child = Command::new(program)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
                &encoded_command(QUICK_ACCESS_SCRIPT),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| {
                fail(format!(
                    "Quick access could not be read (PowerShell did not start): {e}"
                ))
            })?;
        let id = child.id();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(child.wait_with_output());
        });
        let output = match receiver.recv_timeout(std::time::Duration::from_secs(20)) {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(fail(format!("Quick access could not be read: {e}"))),
            Err(_) => {
                let _ = Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &id.to_string()])
                    .creation_flags(0x0800_0000)
                    .output();
                return Err(fail("Quick access could not be read in time"));
            }
        };
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr);
            return Err(fail(format!(
                "Quick access could not be read: {}",
                message.trim().lines().next().unwrap_or("PowerShell failed")
            )));
        }
        parse_quick_access(&String::from_utf8_lossy(&output.stdout))
    }
}

#[cfg(windows)]
pub struct Links;
#[cfg(windows)]
impl Links {
    fn folder() -> Option<PathBuf> {
        std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Links"))
    }
}
#[cfg(windows)]
impl Source for Links {
    fn id(&self) -> &'static str {
        "windows-links"
    }
    fn label(&self) -> &'static str {
        "Links (Explorer favourites folder)"
    }
    fn detect(&self) -> bool {
        Self::folder().is_some_and(|folder| folder.is_dir())
    }
    fn read(&self) -> Result<Vec<Place>> {
        let folder = Self::folder().ok_or_else(|| fail("The user profile folder is unknown"))?;
        read_links_folder(&folder)
    }
}

/// The sources of the Windows host.
#[cfg(windows)]
pub fn windows_sources() -> Vec<Box<dyn Source>> {
    vec![Box::new(QuickAccess), Box::new(Links)]
}
#[cfg(not(windows))]
pub fn windows_sources() -> Vec<Box<dyn Source>> {
    Vec::new()
}

// ---------------------------------------------------------------------------------------------------------------
// Linux: GTK bookmarks and KDE Places

/// The most characters of a label that is taken over.
const MAX_LABEL: usize = 256;

/// `%XX` escapes decoded as UTF-8 bytes. An escape that is not two hex digits, a NUL, or bytes that are not
/// UTF-8 make the whole text unusable: a path is never guessed.
fn percent_decode(text: &str) -> std::result::Result<String, &'static str> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let pair = bytes
                .get(at + 1..at + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or("a broken %-escape in the address")?;
            out.push(pair);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    if out.contains(&0) {
        return Err("a NUL in the address");
    }
    String::from_utf8(out).map_err(|_| "an address that is not valid UTF-8")
}

/// A label made of plain text: control characters become spaces, the ends are trimmed, the length is capped.
fn clean_label(label: &str) -> String {
    let spaced: String = label
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    spaced.trim().chars().take(MAX_LABEL).collect()
}

/// The last part of a path, or `/` for the root.
fn last_part(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("/")
        .to_owned()
}

/// One bookmark address and label as a place: a `file:` address of this computer becomes a folder or file place, every
/// other address is a skipped place with the reason (`trash:/`, `remote:/`, `sftp://…`, searches…).
fn place_of_uri(uri: &str, label: &str, hidden: bool) -> Place {
    let label = clean_label(label);
    let shown = clean_label(uri);
    let skip = |reason: &str| {
        let name = if label.is_empty() {
            shown.clone()
        } else {
            label.clone()
        };
        skipped(&name, &shown, reason)
    };
    let lower = uri.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("file:").map(|_| &uri[5..]) else {
        let scheme = uri.split(':').next().unwrap_or("");
        return skip(&format!(
            "not a folder on this computer ({}:)",
            clean_label(scheme)
        ));
    };
    // `file:///path`, `file://localhost/path` and `file:/path`; a host that names another computer is not ours.
    let path_part = match rest.strip_prefix("//") {
        Some(after) => {
            let slash = after.find('/').unwrap_or(after.len());
            let host = &after[..slash];
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                return skip("a folder on another computer");
            }
            &after[slash..]
        }
        None => rest,
    };
    let decoded = match percent_decode(path_part) {
        Ok(path) => path,
        Err(reason) => return skip(reason),
    };
    // `file:///C:/folder` names the Windows drive path `C:/folder`.
    let decoded = match decoded.as_bytes() {
        [b'/', drive, b':', b'/' | b'\\', ..] if drive.is_ascii_alphabetic() => {
            decoded[1..].to_owned()
        }
        _ => decoded,
    };
    let Some(path) = plain_path(&decoded) else {
        return skip("not an absolute path without .. parts");
    };
    let label = if label.is_empty() {
        last_part(&path)
    } else {
        label
    };
    Place {
        path,
        label,
        hidden,
        kind: Kind::Folder,
        missing: false,
    }
}

fn too_many() -> Place {
    skipped(
        "",
        "",
        &format!("more than {MAX_PLACES} entries: the rest is not listed"),
    )
}

/// The places of a GTK bookmarks file (`~/.config/gtk-3.0/bookmarks`): one `address label` per line, the label is
/// optional. Used by Files (Nautilus), Thunar, Nemo, Caja and others. Empty lines and `#` lines are ignored.
pub fn parse_gtk_bookmarks(text: &str) -> Vec<Place> {
    let mut places = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if places.len() >= MAX_PLACES {
            places.push(too_many());
            break;
        }
        let (uri, label) = line
            .split_once(char::is_whitespace)
            .map_or((line, ""), |(uri, label)| (uri, label));
        places.push(place_of_uri(uri, label, false));
    }
    places
}

/// A DOCTYPE with nothing in it (`<!DOCTYPE xbel>`, which KDE writes) is removed so the parser, which refuses every
/// DTD, can read the file. One with declarations, entities or an external id is refused: it is not what KDE writes.
fn without_plain_doctype(
    xml: &str,
) -> std::result::Result<std::borrow::Cow<'_, str>, &'static str> {
    let Some(start) = xml.find("<!DOCTYPE") else {
        return Ok(std::borrow::Cow::Borrowed(xml));
    };
    let end = xml[start..]
        .find('>')
        .map(|n| start + n + 1)
        .ok_or("an unfinished DOCTYPE")?;
    let declaration = &xml[start..end];
    let inner = &declaration["<!DOCTYPE".len()..declaration.len() - 1];
    if inner.contains(['[', '<', '"', '\'']) || inner.contains("SYSTEM") || inner.contains("PUBLIC")
    {
        return Err("a DOCTYPE with declarations or an external id is not accepted");
    }
    Ok(std::borrow::Cow::Owned(format!(
        "{}{}",
        &xml[..start],
        &xml[end..]
    )))
}

/// The places of KDE's `user-places.xbel` (Dolphin, the Places panel of file dialogs): the `bookmark` elements in
/// order. A bookmark flagged `IsHidden` is returned hidden; separators and other elements are not places.
pub fn parse_kde_places(xml: &str) -> Result<Vec<Place>> {
    if xml.trim().is_empty() {
        return Ok(Vec::new());
    }
    let xml = without_plain_doctype(xml).map_err(|e| fail(format!("user-places.xbel: {e}")))?;
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: 200_000,
    };
    let doc = roxmltree::Document::parse_with_options(&xml, options)
        .map_err(|e| fail(format!("user-places.xbel is not valid XML: {e}")))?;
    let named =
        |node: &roxmltree::Node, name: &str| node.is_element() && node.tag_name().name() == name;
    let mut places = Vec::new();
    for bookmark in doc.descendants().filter(|n| named(n, "bookmark")) {
        if places.len() >= MAX_PLACES {
            places.push(too_many());
            break;
        }
        let title = bookmark
            .children()
            .find(|child| named(child, "title"))
            .and_then(|title| title.text())
            .unwrap_or("");
        let hidden = bookmark
            .descendants()
            .any(|node| named(&node, "IsHidden") && node.text().map(str::trim) == Some("true"));
        match bookmark.attribute("href") {
            Some(href) => places.push(place_of_uri(href, title, hidden)),
            None => places.push(skipped(
                &clean_label(title),
                "",
                "a bookmark without an address",
            )),
        }
    }
    Ok(places)
}

/// The largest bookmarks file that is read.
pub const MAX_LIST_BYTES: u64 = 1024 * 1024;

/// A text file of at most [`MAX_LIST_BYTES`] bytes, read as UTF-8.
fn read_capped(file: &Path) -> Result<String> {
    use std::io::Read;
    let handle = std::fs::File::open(file).map_err(|e| fail(format!("{}: {e}", file.display())))?;
    let mut text = String::new();
    handle
        .take(MAX_LIST_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| fail(format!("{}: {e}", file.display())))?;
    if text.len() as u64 > MAX_LIST_BYTES {
        return Err(fail(format!(
            "{} is larger than {} KiB and is not read",
            file.display(),
            MAX_LIST_BYTES / 1024
        )));
    }
    Ok(text)
}

/// Places probed at the same time, and how long one batch is waited for. A path that does not answer in that time (a
/// dead network mount) is neither missing nor examined: it stays a folder place, and the other places are not held up
/// by it for longer than one batch.
const PROBE_BATCH: usize = 64;
const PROBE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Fills in what only the disk knows: whether each taken-over place is a folder or a file and whether it exists. The
/// places are asked in batches at the same time, each on its own short-lived thread.
fn examine(mut places: Vec<Place>) -> Vec<Place> {
    let asked: Vec<usize> = places
        .iter()
        .enumerate()
        .filter(|(_, place)| place.kind == Kind::Folder)
        .map(|(index, _)| index)
        .collect();
    for batch in asked.chunks(PROBE_BATCH) {
        let (sender, receiver) = std::sync::mpsc::channel();
        for &index in batch {
            let sender = sender.clone();
            let path = PathBuf::from(&places[index].path);
            std::thread::spawn(move || {
                let _ = sender.send((index, std::fs::metadata(&path).map(|m| m.is_file())));
            });
        }
        drop(sender);
        let deadline = std::time::Instant::now() + PROBE_WAIT;
        for _ in batch {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok((index, found)) = receiver.recv_timeout(left) else {
                break;
            };
            match found {
                Ok(true) => places[index].kind = Kind::File,
                Ok(false) => {}
                Err(e) => places[index].missing = e.kind() == std::io::ErrorKind::NotFound,
            }
        }
    }
    places
}

/// A bookmarks file of another manager at a fixed place; `detect` only asks whether the file exists.
pub struct FileSource {
    id: &'static str,
    label: &'static str,
    file: PathBuf,
    parse: fn(&str) -> Result<Vec<Place>>,
}

impl Source for FileSource {
    fn id(&self) -> &'static str {
        self.id
    }
    fn label(&self) -> &'static str {
        self.label
    }
    fn detect(&self) -> bool {
        self.file.is_file()
    }
    fn read(&self) -> Result<Vec<Place>> {
        Ok(examine((self.parse)(&read_capped(&self.file)?)?))
    }
}

/// GTK bookmarks (Files/Nautilus, Thunar, Nemo, Caja…) and KDE Places (Dolphin) of the current user.
pub fn linux_sources() -> Vec<Box<dyn Source>> {
    #[cfg(target_os = "linux")]
    {
        let mut sources: Vec<Box<dyn Source>> = Vec::new();
        if let Some(config) = dirs::config_dir() {
            sources.push(Box::new(FileSource {
                id: "gtk-bookmarks",
                label: "GTK bookmarks",
                file: config.join("gtk-3.0").join("bookmarks"),
                parse: |text| Ok(parse_gtk_bookmarks(text)),
            }));
        }
        if let Some(data) = dirs::data_dir() {
            sources.push(Box::new(FileSource {
                id: "kde-places",
                label: "KDE Places",
                file: data.join("user-places.xbel"),
                parse: parse_kde_places,
            }));
        }
        sources
    }
    #[cfg(not(target_os = "linux"))]
    Vec::new()
}

// ---------------------------------------------------------------------------------------------------------------
// macOS: Finder sidebar (`FavoriteItems.sfl3`)

/// One item decoded from a Finder `FavoriteItems.sfl3` NSKeyedArchiver plist before its
/// bookmark bytes are resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinderSidebarItem {
    pub name: String,
    pub bookmark: Option<Vec<u8>>,
    pub hidden: bool,
    pub special_id: Option<String>,
}

/// Resolves a `plist::Uid` index against the `$objects` table of an `NSKeyedArchiver` archive,
/// treating `"$null"` as `None`.
fn sfl_resolve<'a>(objects: &'a [plist::Value], val: &'a plist::Value) -> Option<&'a plist::Value> {
    let target = match val {
        plist::Value::Uid(uid) => objects.get(uid.get() as usize)?,
        other => other,
    };
    if matches!(target, plist::Value::String(s) if s == "$null") {
        None
    } else {
        Some(target)
    }
}

fn sfl_as_string(objects: &[plist::Value], val: &plist::Value) -> Option<String> {
    let target = sfl_resolve(objects, val)?;
    match target {
        plist::Value::String(s) => Some(s.clone()),
        plist::Value::Dictionary(d) => d
            .get("NS.string")
            .and_then(|v| sfl_resolve(objects, v))
            .and_then(|v| v.as_string())
            .map(str::to_owned),
        _ => None,
    }
}

fn sfl_as_data(objects: &[plist::Value], val: &plist::Value) -> Option<Vec<u8>> {
    let target = sfl_resolve(objects, val)?;
    match target {
        plist::Value::Data(b) => Some(b.clone()),
        plist::Value::Dictionary(d) => d
            .get("NS.data")
            .and_then(|v| sfl_resolve(objects, v))
            .and_then(|v| v.as_data())
            .map(|b| b.to_vec()),
        _ => None,
    }
}

fn sfl_as_array<'a>(
    objects: &'a [plist::Value],
    val: &'a plist::Value,
) -> Option<&'a [plist::Value]> {
    let target = sfl_resolve(objects, val)?;
    match target {
        plist::Value::Array(arr) => Some(arr.as_slice()),
        plist::Value::Dictionary(d) => d
            .get("NS.objects")
            .and_then(|v| sfl_resolve(objects, v))
            .and_then(|v| v.as_array())
            .map(Vec::as_slice),
        _ => None,
    }
}

/// Looks up `key` in either a plain plist dictionary or an archived `NSDictionary`
/// (`NS.keys` + `NS.objects` parallel arrays).
fn sfl_dict_get<'a>(
    objects: &'a [plist::Value],
    val: &'a plist::Value,
    key: &str,
) -> Option<&'a plist::Value> {
    let dict = sfl_resolve(objects, val)?.as_dictionary()?;
    if let Some(direct) = dict.get(key) {
        return Some(direct);
    }
    let keys = dict.get("NS.keys").and_then(|v| sfl_as_array(objects, v))?;
    let vals = dict
        .get("NS.objects")
        .and_then(|v| sfl_as_array(objects, v))?;
    for (k, v) in keys.iter().zip(vals.iter()) {
        if sfl_as_string(objects, k).as_deref() == Some(key) {
            return Some(v);
        }
    }
    None
}

/// Parses a macOS Finder `FavoriteItems.sfl3` (NSKeyedArchiver plist) byte stream into
/// ordered [`FinderSidebarItem`] entries. Never executes or resolves anything on disk.
pub fn parse_finder_sfl(bytes: &[u8]) -> Result<Vec<FinderSidebarItem>> {
    if bytes.len() as u64 > MAX_LIST_BYTES {
        return Err(fail(format!(
            "Finder sidebar file is larger than {} KiB and is not read",
            MAX_LIST_BYTES / 1024
        )));
    }
    let root = plist::from_bytes::<plist::Value>(bytes)
        .map_err(|e| fail(format!("Finder sidebar file is not a valid plist: {e}")))?;
    let top_dict = root
        .as_dictionary()
        .ok_or_else(|| fail("Finder sidebar file has no top-level dictionary"))?;
    let objects = top_dict
        .get("$objects")
        .and_then(|v| v.as_array())
        .ok_or_else(|| fail("Finder sidebar archive is missing $objects"))?;
    let top = top_dict
        .get("$top")
        .and_then(|v| v.as_dictionary())
        .ok_or_else(|| fail("Finder sidebar archive is missing $top"))?;
    let root_uid = top
        .get("root")
        .ok_or_else(|| fail("Finder sidebar archive is missing $top.root"))?;
    let Some(items_val) = sfl_dict_get(objects, root_uid, "items") else {
        return Ok(Vec::new());
    };
    let Some(items) = sfl_as_array(objects, items_val) else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for entry in items.iter().take(MAX_PLACES + 1) {
        let name = sfl_dict_get(objects, entry, "Name")
            .and_then(|v| sfl_as_string(objects, v))
            .map(|s| clean_label(&s))
            .unwrap_or_default();
        let bookmark =
            sfl_dict_get(objects, entry, "Bookmark").and_then(|v| sfl_as_data(objects, v));
        let visibility = sfl_dict_get(objects, entry, "Visibility")
            .and_then(|v| sfl_resolve(objects, v))
            .and_then(|v| v.as_signed_integer());
        let custom = sfl_dict_get(objects, entry, "CustomItemProperties");
        let special_id = custom
            .and_then(|c| {
                sfl_dict_get(
                    objects,
                    c,
                    "com.apple.LSSharedFileList.SpecialItemIdentifier",
                )
            })
            .and_then(|v| sfl_as_string(objects, v))
            .or_else(|| {
                custom
                    .and_then(|c| sfl_dict_get(objects, c, "com.apple.LSSharedFileList.IsAirDrop"))
                    .and_then(|v| sfl_resolve(objects, v))
                    .and_then(|v| v.as_boolean())
                    .filter(|&b| b)
                    .map(|_| "com.apple.LSSharedFileList.IsAirDrop".to_owned())
            })
            .or_else(|| {
                custom
                    .and_then(|c| {
                        sfl_dict_get(objects, c, "com.apple.LSSharedFileList.IsMeetingRoom")
                    })
                    .and_then(|v| sfl_resolve(objects, v))
                    .and_then(|v| v.as_boolean())
                    .filter(|&b| b)
                    .map(|_| "com.apple.LSSharedFileList.IsMeetingRoom".to_owned())
            });
        // In Finder's SFL archive `Visibility == 1` marks a sidebar item hidden by the user.
        let hidden = visibility == Some(1);
        out.push(FinderSidebarItem {
            name,
            bookmark,
            hidden,
            special_id,
        });
    }
    Ok(out)
}

/// Identifies Finder sidebar pseudo-entries (AirDrop, Recents, iCloud Drive, saved searches)
/// that have no ordinary local folder path to import, returning the user-facing skip reason.
fn finder_pseudo_reason(
    name: &str,
    special_id: Option<&str>,
    resolved_path: Option<&str>,
) -> Option<&'static str> {
    if let Some(id) = special_id {
        let lower = id.to_ascii_lowercase();
        if lower.contains("airdrop") || lower.contains("meetingroom") {
            return Some("AirDrop is a Finder feature, not a folder on disk");
        }
        if lower.contains("recent") || lower.contains("allmyfiles") {
            return Some("Recents is a Finder smart view, not a folder on disk");
        }
        if lower.contains("icloud") || lower.contains("clouddocs") {
            return Some("iCloud Drive is a Finder virtual entry");
        }
    }
    if let Some(path) = resolved_path {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".cannedsearch") || lower.contains("mydocuments.cannedsearch") {
            return Some("Recents is a Finder smart view, not a folder on disk");
        }
        if lower.contains("airdrop.app") || lower.starts_with("nwnode://") {
            return Some("AirDrop is a Finder feature, not a folder on disk");
        }
        if lower.ends_with("/library/mobile documents/com~apple~clouddocs") {
            return Some("iCloud Drive is a Finder virtual entry");
        }
    }
    let name_lower = name.trim().to_ascii_lowercase();
    if name_lower == "airdrop" {
        return Some("AirDrop is a Finder feature, not a folder on disk");
    }
    if matches!(name_lower.as_str(), "recents" | "all my files") {
        return Some("Recents is a Finder smart view, not a folder on disk");
    }
    if name_lower == "icloud drive" {
        return Some("iCloud Drive is a Finder virtual entry");
    }
    None
}

/// Converts decoded Finder sidebar items into [`Place`] values using `resolve_bookmark` to
/// turn raw bookmark bytes into a local file path (never guessing when resolution fails).
pub fn places_from_finder_items<F>(
    items: Vec<FinderSidebarItem>,
    mut resolve_bookmark: F,
) -> Vec<Place>
where
    F: FnMut(&[u8]) -> std::result::Result<String, String>,
{
    let mut places = Vec::new();
    for item in items {
        if places.len() >= MAX_PLACES {
            places.push(too_many());
            break;
        }
        if let Some(reason) = finder_pseudo_reason(&item.name, item.special_id.as_deref(), None) {
            let label = if item.name.is_empty() {
                item.special_id
                    .clone()
                    .unwrap_or_else(|| "Finder item".into())
            } else {
                item.name
            };
            places.push(skipped(&label, "", reason));
            continue;
        }
        let Some(bookmark) = item.bookmark.as_deref() else {
            let label = if item.name.is_empty() {
                "Finder item".to_owned()
            } else {
                item.name
            };
            places.push(skipped(
                &label,
                "",
                "Finder sidebar entry has no bookmark data",
            ));
            continue;
        };
        match resolve_bookmark(bookmark) {
            Ok(raw_path) => {
                if let Some(reason) =
                    finder_pseudo_reason(&item.name, item.special_id.as_deref(), Some(&raw_path))
                {
                    let label = if item.name.is_empty() {
                        last_part(&raw_path)
                    } else {
                        item.name
                    };
                    places.push(skipped(&label, &raw_path, reason));
                    continue;
                }
                let Some(path) = plain_path(&raw_path) else {
                    let label = if item.name.is_empty() {
                        clean_label(&raw_path)
                    } else {
                        item.name
                    };
                    places.push(skipped(
                        &label,
                        &raw_path,
                        "not an absolute path of a file or folder",
                    ));
                    continue;
                };
                let label = if item.name.is_empty() {
                    last_part(&path)
                } else {
                    item.name
                };
                places.push(Place {
                    path,
                    label,
                    hidden: item.hidden,
                    kind: Kind::Folder,
                    missing: false,
                });
            }
            Err(reason) => {
                let label = if item.name.is_empty() {
                    "Finder item".to_owned()
                } else {
                    item.name
                };
                places.push(skipped(&label, "", &reason));
            }
        }
    }
    places
}

/// Resolves macOS bookmark bytes to a POSIX path without mounting volumes, opening files,
/// or triggering any permission/UI dialog.
#[cfg(target_os = "macos")]
pub fn resolve_macos_bookmark(bookmark: &[u8]) -> std::result::Result<String, String> {
    use objc2::runtime::Bool;
    use objc2_foundation::{NSData, NSURLBookmarkResolutionOptions, NSURL};

    let data = NSData::with_bytes(bookmark);
    let options =
        NSURLBookmarkResolutionOptions::WithoutUI | NSURLBookmarkResolutionOptions::WithoutMounting;
    let mut is_stale = Bool::NO;
    // SAFETY: `is_stale` is a valid stack pointer; `WithoutUI | WithoutMounting` prevents
    // dialogs and volume mounts.
    let url = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            &data,
            options,
            None,
            &mut is_stale,
        )
    }
    .map_err(|err| {
        format!(
            "Finder bookmark could not be resolved without mounting ({})",
            err.localizedDescription()
        )
    })?;
    if !url.isFileURL() {
        return Err("Finder bookmark does not point to a local file or folder".into());
    }
    let Some(path) = url.path() else {
        return Err("Finder bookmark has no local path".into());
    };
    let text = path.to_string();
    if text.is_empty() {
        Err("Finder bookmark resolved to an empty path".into())
    } else {
        Ok(text)
    }
}

#[cfg(target_os = "macos")]
pub struct FinderSidebar;

#[cfg(target_os = "macos")]
impl FinderSidebar {
    fn sharedfilelist_dir() -> Option<PathBuf> {
        dirs::home_dir().map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("com.apple.sharedfilelist")
        })
    }

    fn candidate_files() -> Vec<PathBuf> {
        let Some(dir) = Self::sharedfilelist_dir() else {
            return Vec::new();
        };
        [
            "com.apple.LSSharedFileList.FavoriteItems.sfl4",
            "com.apple.LSSharedFileList.FavoriteItems.sfl3",
            "com.apple.LSSharedFileList.FavoriteItems.sfl2",
        ]
        .into_iter()
        .map(|name| dir.join(name))
        .collect()
    }
}

#[cfg(target_os = "macos")]
impl Source for FinderSidebar {
    fn id(&self) -> &'static str {
        "macos-finder-sidebar"
    }

    fn label(&self) -> &'static str {
        "Finder sidebar"
    }

    fn detect(&self) -> bool {
        let Some(dir) = Self::sharedfilelist_dir() else {
            return false;
        };
        if Self::candidate_files().iter().any(|p| p.is_file()) {
            return true;
        }
        // When Full Disk Access is absent, macOS blocks `com.apple.sharedfilelist` with
        // PermissionDenied (EPERM/EACCES): still report the source so `read()` can explain
        // the Full Disk Access requirement clearly instead of hiding Finder altogether.
        matches!(
            std::fs::read_dir(&dir),
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied
        )
    }

    fn read(&self) -> Result<Vec<Place>> {
        let fda_err = || {
            fail(
                "Finder sidebar favorites could not be read: macOS requires Full Disk Access \
                 (System Settings › Privacy & Security › Full Disk Access) to read \
                 ~/Library/Application Support/com.apple.sharedfilelist",
            )
        };
        let Some(dir) = Self::sharedfilelist_dir() else {
            return Err(fail("The home folder is unknown"));
        };
        if let Err(err) = std::fs::read_dir(&dir) {
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(fda_err());
            }
            return Err(fail(format!("{}: {err}", dir.display())));
        }
        let Some(file) = Self::candidate_files().into_iter().find(|p| p.is_file()) else {
            return Err(fail(
                "Finder sidebar favorites file (FavoriteItems.sfl3) was not found",
            ));
        };
        read_finder_sfl_file(&file)
    }
}

/// Reads and resolves a Finder `.sfl3` file from disk, reporting Full Disk Access clearly on
/// `PermissionDenied` and never guessing a path when data cannot be read.
#[cfg(target_os = "macos")]
pub fn read_finder_sfl_file(file: &Path) -> Result<Vec<Place>> {
    use std::io::Read;
    let handle = std::fs::File::open(file).map_err(|err| {
        if err.kind() == std::io::ErrorKind::PermissionDenied {
            fail(
                "Finder sidebar favorites could not be read: macOS requires Full Disk Access \
                 (System Settings › Privacy & Security › Full Disk Access) to read \
                 ~/Library/Application Support/com.apple.sharedfilelist",
            )
        } else {
            fail(format!("{}: {err}", file.display()))
        }
    })?;
    let mut bytes = Vec::new();
    handle
        .take(MAX_LIST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| fail(format!("{}: {e}", file.display())))?;
    let items = parse_finder_sfl(&bytes)?;
    Ok(examine(places_from_finder_items(
        items,
        resolve_macos_bookmark,
    )))
}

/// The sources of the macOS host.
#[cfg(target_os = "macos")]
pub fn macos_sources() -> Vec<Box<dyn Source>> {
    vec![Box::new(FinderSidebar)]
}
#[cfg(not(target_os = "macos"))]
pub fn macos_sources() -> Vec<Box<dyn Source>> {
    Vec::new()
}

// ---------------------------------------------------------------------------------------------------------------
// Applying an import

use super::favorites::Favorites;

/// Every source of this platform, in the order the importer lists them.
pub fn all_sources() -> Vec<Box<dyn Source>> {
    let mut sources = windows_sources();
    sources.extend(linux_sources());
    sources.extend(macos_sources());
    sources
}

/// A path as it is compared for duplicates: tidied as `plain_path` does, normalized for
/// macOS Unicode decomposition (NFD vs NFC), and case-folded where file names are not
/// case-sensitive.
fn same_place_key(path: &str) -> String {
    same_place_key_for(
        path,
        cfg!(any(windows, target_os = "macos")),
        cfg!(target_os = "macos"),
    )
}

fn same_place_key_for(path: &str, case_insensitive: bool, normalize_unicode: bool) -> String {
    let mut tidy = plain_path(path).unwrap_or_else(|| path.to_owned());
    if normalize_unicode {
        use unicode_normalization::UnicodeNormalization;
        tidy = tidy.nfc().collect();
    }
    if case_insensitive {
        tidy.to_lowercase()
    } else {
        tidy
    }
}

/// Whether the path is already a favorite, in any group.
pub fn already_in(favorites: &Favorites, path: &str) -> bool {
    let key = same_place_key(path);
    favorites
        .groups
        .iter()
        .flat_map(|group| &group.items)
        .any(|item| same_place_key(&item.path) == key)
}

/// One place the user ticked, with the label to show for it (`None`: the folder name).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItem {
    pub path: String,
    #[serde(default)]
    pub alias: Option<String>,
}

/// The ticked places of one source; they become one group named `Imported from <source>`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportChoice {
    pub source: String,
    pub items: Vec<ImportItem>,
}

/// What an import added: enough to take exactly that back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReceipt {
    pub groups: Vec<ImportedGroup>,
    /// Ticked places that were left out: already a favorite, or not an absolute path.
    pub left_out: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedGroup {
    pub id: u64,
    pub paths: Vec<String>,
}

/// Adds the ticked places as one new group per source, after the existing groups, in the order given. Nothing that is
/// already there is changed, moved or removed, a place that is already a favorite (in any group, also from an earlier
/// import) is left out, and a source with nothing left makes no group, so importing again changes nothing. A path
/// that is not absolute, or has a NUL or a `..` part, is left out as well.
pub fn apply_import(favorites: &mut Favorites, choices: &[ImportChoice]) -> ImportReceipt {
    let mut receipt = ImportReceipt::default();
    for choice in choices {
        let mut fresh: Vec<(String, Option<String>)> = Vec::new();
        for item in choice.items.iter().take(MAX_PLACES) {
            let Some(path) = plain_path(&item.path) else {
                receipt.left_out += 1;
                continue;
            };
            let key = same_place_key(&path);
            if already_in(favorites, &path) || fresh.iter().any(|(p, _)| same_place_key(p) == key) {
                receipt.left_out += 1;
                continue;
            }
            let alias = item
                .alias
                .as_deref()
                .map(clean_label)
                .filter(|a| !a.is_empty() && *a != last_part(&path));
            fresh.push((path, alias));
        }
        if fresh.is_empty() {
            continue;
        }
        let name = format!("Imported from {}", clean_label(&choice.source));
        let id = favorites.add_group(&name);
        let mut paths = Vec::new();
        for (index, (path, alias)) in fresh.into_iter().enumerate() {
            if favorites.add_item(id, &path, None) {
                if alias.is_some() {
                    favorites.set_alias(id, index, alias);
                }
                paths.push(path);
            }
        }
        receipt.groups.push(ImportedGroup { id, paths });
    }
    receipt
}

/// Takes back what `apply_import` added: its places are removed from the groups it made, and a group that is then
/// empty is removed. A group the user has since filled with places of their own stays, with those places. Returns the
/// number of places removed.
pub fn undo_import(favorites: &mut Favorites, receipt: &ImportReceipt) -> usize {
    let mut removed = 0;
    for imported in &receipt.groups {
        let Some(group) = favorites.groups.iter_mut().find(|g| g.id == imported.id) else {
            continue;
        };
        let before = group.items.len();
        group
            .items
            .retain(|item| !imported.paths.contains(&item.path));
        removed += before - group.items.len();
    }
    let made: Vec<u64> = receipt.groups.iter().map(|g| g.id).collect();
    favorites
        .groups
        .retain(|group| !(made.contains(&group.id) && group.items.is_empty()));
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal shortcut: header, optional ID list and a LinkInfo with the given parts.
    fn lnk(
        id_list: bool,
        local: Option<(&str, &str)>,
        network: Option<(&str, &str)>,
        wide: bool,
    ) -> Vec<u8> {
        let mut data = vec![0u8; 0x4C];
        data[0..4].copy_from_slice(&0x4Cu32.to_le_bytes());
        data[4..20].copy_from_slice(&[
            0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ]);
        let has_info = local.is_some() || network.is_some();
        let flags = u32::from(id_list) | (u32::from(has_info) << 1);
        data[20..24].copy_from_slice(&flags.to_le_bytes());
        if id_list {
            data.extend_from_slice(&4u16.to_le_bytes());
            data.extend_from_slice(&[0, 0, 0, 0]);
        }
        if !has_info {
            return data;
        }
        let header = if wide { 0x24u32 } else { 0x1Cu32 };
        let ansi = |text: &str| -> Vec<u8> { text.bytes().chain([0]).collect() };
        let utf16 = |text: &str| -> Vec<u8> {
            text.encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect()
        };
        let (head, tail, is_local) = match (local, network) {
            (Some((head, tail)), _) => (head, tail, true),
            (None, Some((head, tail))) => (head, tail, false),
            _ => unreachable!(),
        };
        let mut body: Vec<u8> = Vec::new();
        let base = header as usize;
        let mut local_off = 0usize;
        let mut net_off = 0usize;
        if is_local {
            local_off = base + body.len();
            body.extend(ansi(head));
        } else {
            net_off = base + body.len();
            // CommonNetworkRelativeLink: size, flags, NetNameOffset (0x14), DeviceNameOffset, provider, then the name.
            let mut block = Vec::new();
            block.extend_from_slice(&0x14u32.wrapping_add(head.len() as u32 + 1).to_le_bytes());
            block.extend_from_slice(&0u32.to_le_bytes());
            block.extend_from_slice(&0x14u32.to_le_bytes());
            block.extend_from_slice(&0u32.to_le_bytes());
            block.extend_from_slice(&0u32.to_le_bytes());
            block.extend(ansi(head));
            body.extend(block);
        }
        let suffix_off = base + body.len();
        body.extend(ansi(tail));
        let (mut local_wide, mut suffix_wide) = (0usize, 0usize);
        if wide {
            if is_local {
                local_wide = base + body.len();
                body.extend(utf16(head));
            }
            suffix_wide = base + body.len();
            body.extend(utf16(tail));
        }
        let total = base + body.len();
        let mut info = Vec::new();
        info.extend_from_slice(&(total as u32).to_le_bytes());
        info.extend_from_slice(&header.to_le_bytes());
        info.extend_from_slice(&(if is_local { 1u32 } else { 2u32 }).to_le_bytes());
        info.extend_from_slice(&0u32.to_le_bytes());
        info.extend_from_slice(&(local_off as u32).to_le_bytes());
        info.extend_from_slice(&(net_off as u32).to_le_bytes());
        info.extend_from_slice(&(suffix_off as u32).to_le_bytes());
        if wide {
            info.extend_from_slice(&(local_wide as u32).to_le_bytes());
            info.extend_from_slice(&(suffix_wide as u32).to_le_bytes());
        }
        info.extend(body);
        data.extend(info);
        data
    }

    #[test]
    fn a_shortcut_to_a_local_folder_gives_its_path_with_the_suffix() {
        for wide in [false, true] {
            let data = lnk(true, Some((r"C:\Users\me", r"Projects\gall")), None, wide);
            assert_eq!(
                parse_lnk(&data).unwrap(),
                Target::Path(r"C:\Users\me\Projects\gall".into()),
                "wide={wide}"
            );
        }
        // No suffix: the base path alone, and a root keeps its separator.
        assert_eq!(
            parse_lnk(&lnk(false, Some((r"D:\", "")), None, true)).unwrap(),
            Target::Path(r"D:\".into())
        );
    }

    #[test]
    fn a_shortcut_to_a_network_share_gives_the_unc_path() {
        let data = lnk(true, None, Some((r"\\server\share", r"team\docs")), false);
        assert_eq!(
            parse_lnk(&data).unwrap(),
            Target::Path(r"\\server\share\team\docs".into())
        );
    }

    #[test]
    fn a_shortcut_without_a_file_path_is_a_shell_item_and_unicode_names_survive() {
        assert_eq!(
            parse_lnk(&lnk(true, None, None, false)).unwrap(),
            Target::Shell
        );
        let data = lnk(
            false,
            Some((r"C:\Uporabniki\Žan", r"Dokumenti\čšž")),
            None,
            true,
        );
        assert_eq!(
            parse_lnk(&data).unwrap(),
            Target::Path(r"C:\Uporabniki\Žan\Dokumenti\čšž".into())
        );
    }

    #[test]
    fn damaged_or_hostile_shortcuts_are_errors_not_panics() {
        assert!(parse_lnk(&[]).is_err());
        assert!(
            parse_lnk(b"MZ not a shortcut at all, but long enough to have a header....").is_err()
        );
        let good = lnk(true, Some((r"C:\x", "y")), None, true);
        // Every truncation either parses or reports an error; none panics.
        for cut in 0..good.len() {
            let _ = parse_lnk(&good[..cut]);
        }
        // Offsets pointing far outside the file.
        let mut wild = good.clone();
        let at = 0x4C + 6 + 16;
        wild[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let _ = parse_lnk(&wild);
        let mut size = good.clone();
        size[0x4C + 6..0x4C + 10].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_lnk(&size).is_err());
    }

    #[test]
    fn paths_are_kept_as_written_and_odd_ones_are_refused() {
        assert_eq!(plain_path(r"C:\Users\me\").as_deref(), Some(r"C:\Users\me"));
        assert_eq!(plain_path(r"C:\").as_deref(), Some(r"C:\"));
        assert_eq!(
            plain_path(r"Z:\mapped\folder").as_deref(),
            Some(r"Z:\mapped\folder")
        );
        assert_eq!(
            plain_path(r"\\server\share\dir").as_deref(),
            Some(r"\\server\share\dir")
        );
        assert_eq!(
            plain_path(r"\\?\C:\long\path").as_deref(),
            Some(r"C:\long\path")
        );
        assert_eq!(
            plain_path(r"\\?\UNC\server\share\x").as_deref(),
            Some(r"\\server\share\x")
        );
        assert_eq!(plain_path("/home/me/").as_deref(), Some("/home/me"));
        for bad in [
            "",
            "  ",
            "relative\\dir",
            r"C:\a\..\b",
            r"..\x",
            "C:\\a\0b",
            r"\\.\pipe\x",
            "C:",
        ] {
            assert_eq!(plain_path(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn quick_access_lists_pinned_places_and_reports_virtual_folders() {
        let json = r#"[{"path":"C:\\Projects","name":"Projects"},{"path":"::{645FF040-5081-101B-9F08-00AA002F954E}","name":"Recycle Bin"},{"path":"C:\\Users\\me\\OneDrive - Firm\\Namizje","name":"Namizje"},{"path":"relative","name":"odd"}]"#;
        let places = parse_quick_access(json).unwrap();
        assert_eq!(places.len(), 4);
        assert_eq!(
            (places[0].path.as_str(), places[0].label.as_str()),
            (r"C:\Projects", "Projects")
        );
        assert!(matches!(places[0].kind, Kind::Folder));
        assert!(matches!(&places[1].kind, Kind::Skipped(reason) if reason.contains("virtual")));
        assert_eq!(places[2].label, "Namizje");
        assert!(matches!(&places[3].kind, Kind::Skipped(_)));
    }

    #[test]
    fn quick_access_output_forms_of_powershell_all_parse() {
        // One item is printed as an object, none as an empty array, and a byte order mark may lead.
        assert_eq!(
            parse_quick_access(r#"{"path":"C:\\a","name":"a"}"#)
                .unwrap()
                .len(),
            1
        );
        assert!(parse_quick_access("[]").unwrap().is_empty());
        assert!(parse_quick_access("").unwrap().is_empty());
        assert!(parse_quick_access("\u{feff}[]").unwrap().is_empty());
        assert!(parse_quick_access("not json").is_err());
        // A name that is missing falls back to the last part of the path.
        assert_eq!(
            parse_quick_access(r#"[{"path":"C:\\x\\last"}]"#).unwrap()[0].label,
            "last"
        );
    }

    #[test]
    fn the_encoded_command_is_base64_of_utf16() {
        // "A" is 41 00 in UTF-16LE: QQA=
        assert_eq!(encoded_command("A"), "QQA=");
        assert_eq!(encoded_command("AB"), "QQBCAA==");
        assert_eq!(encoded_command("ABC"), "QQBCAEMA");
    }

    #[test]
    fn a_folder_of_shortcuts_is_read_in_order_without_running_anything() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("target");
        std::fs::create_dir(&real).unwrap();
        let target = real.to_string_lossy().into_owned();
        // The shortcut format keeps Windows paths; any absolute spelling is accepted as written.
        let windows_like = lnk(true, Some((r"C:\Does\Not\Exist", "")), None, true);
        std::fs::write(temp.path().join("b-missing.lnk"), &windows_like).unwrap();
        std::fs::write(
            temp.path().join("A-special.lnk"),
            lnk(true, None, None, false),
        )
        .unwrap();
        std::fs::write(temp.path().join("c-broken.lnk"), b"garbage").unwrap();
        std::fs::write(temp.path().join("notes.txt"), b"ignored").unwrap();
        let local = lnk(false, Some((&target, "")), None, true);
        std::fs::write(temp.path().join("d-real.LNK"), &local).unwrap();
        let places = read_links_folder(temp.path()).unwrap();
        let labels: Vec<_> = places.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["A-special", "b-missing", "c-broken", "d-real"]);
        assert!(
            matches!(&places[0].kind, Kind::Skipped(reason) if reason.contains("special folder"))
        );
        // A missing target is still a folder place (the checklist marks it missing); a broken file says why.
        assert!(matches!(places[1].kind, Kind::Folder) || cfg!(not(windows)));
        assert!(
            matches!(&places[2].kind, Kind::Skipped(reason) if reason.contains("valid shortcut"))
        );
        if cfg!(windows) {
            assert_eq!(places[3].path, target.trim_end_matches('\\'));
            assert!(matches!(places[3].kind, Kind::Folder));
        }
        // Nothing in the source folder changed.
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 6);
    }

    #[test]
    fn the_number_of_places_and_the_size_of_a_shortcut_are_capped() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..(MAX_PLACES + 20) {
            std::fs::write(
                temp.path().join(format!("{index:04}.lnk")),
                lnk(true, None, None, false),
            )
            .unwrap();
        }
        assert_eq!(read_links_folder(temp.path()).unwrap().len(), MAX_PLACES);
        let big = tempfile::tempdir().unwrap();
        std::fs::write(
            big.path().join("huge.lnk"),
            vec![0u8; MAX_LINK_BYTES as usize + 1],
        )
        .unwrap();
        let places = read_links_folder(big.path()).unwrap();
        assert!(matches!(&places[0].kind, Kind::Skipped(reason) if reason.contains("too large")));
    }

    #[test]
    fn virtual_folders_are_recognised_by_name() {
        for name in [
            "::{645FF040-5081-101B-9F08-00AA002F954E}",
            "shell:Downloads",
            "This PC",
            "Libraries",
            "Recycle Bin",
        ] {
            assert!(virtual_name(name), "{name}");
        }
        assert!(!virtual_name(r"C:\Projects"));
    }

    fn summary(places: &[Place]) -> Vec<(String, String, bool, bool)> {
        places
            .iter()
            .map(|p| {
                (
                    p.path.clone(),
                    p.label.clone(),
                    p.hidden,
                    matches!(p.kind, Kind::Skipped(_)),
                )
            })
            .collect()
    }

    fn reason(place: &Place) -> String {
        match &place.kind {
            Kind::Skipped(reason) => reason.clone(),
            other => panic!("not skipped: {other:?}"),
        }
    }

    #[test]
    fn gtk_bookmarks_keep_order_labels_spaces_unicode_and_escapes() {
        let text = "# a comment\nfile:///home/ana/Documents Documents\n\nfile:///home/ana/My%20Pictures My pictures here\n\
                    file:///srv/%C4%8Dasi%20in%20prostor\nfile://localhost/var/log/ Logs\r\nfile:/opt/single Single slash\n\
                    file:///\n   file:///home/ana/Music    Tab\tand  spaces  \nfile:///C:/Users/ana/Work%20Files Work\n";
        let places = parse_gtk_bookmarks(text);
        assert_eq!(
            summary(&places),
            vec![
                (
                    "/home/ana/Documents".into(),
                    "Documents".into(),
                    false,
                    false
                ),
                (
                    "/home/ana/My Pictures".into(),
                    "My pictures here".into(),
                    false,
                    false
                ),
                // No label: the last part of the decoded path.
                (
                    "/srv/časi in prostor".into(),
                    "časi in prostor".into(),
                    false,
                    false
                ),
                // A trailing slash is dropped, CRLF does not stay in the label.
                ("/var/log".into(), "Logs".into(), false, false),
                ("/opt/single".into(), "Single slash".into(), false, false),
                ("/".into(), "/".into(), false, false),
                (
                    "/home/ana/Music".into(),
                    "Tab and  spaces".into(),
                    false,
                    false
                ),
                // The slash before a drive letter is not part of the Windows path.
                (
                    "C:/Users/ana/Work Files".into(),
                    "Work".into(),
                    false,
                    false
                ),
            ]
        );
    }

    #[test]
    fn gtk_bookmarks_report_what_they_skip_and_why() {
        let text = "sftp://host/share Remote\nsmb://nas/media NAS\ntrash:/// Trash\nfile://otherhost/data Elsewhere\n\
                    file:///home/../etc Up\nfile://relative/path Odd\nfile:///bad%zzescape Broken\nfile:///nul%00byte Nul\n\
                    file:///bad%FFutf8 Latin\nnonsense\n";
        let places = parse_gtk_bookmarks(text);
        assert_eq!(places.len(), 10);
        assert!(
            places.iter().all(|p| matches!(p.kind, Kind::Skipped(_))),
            "{places:?}"
        );
        assert!(reason(&places[0]).contains("sftp:"));
        assert!(reason(&places[2]).contains("trash:"));
        assert!(reason(&places[3]).contains("another computer"));
        assert!(reason(&places[4]).contains(".."));
        // `file://relative/path` names a host called "relative", not a relative path.
        assert!(reason(&places[5]).contains("another computer"));
        assert!(reason(&places[6]).contains("%-escape"));
        assert!(reason(&places[7]).contains("NUL"));
        assert!(reason(&places[8]).contains("UTF-8"));
        assert!(reason(&places[9]).contains("not a folder on this computer"));
        // The label of a skipped entry is what the user knows it by.
        assert_eq!(places[0].label, "Remote");
    }

    #[test]
    fn gtk_bookmarks_cap_the_entries_and_accept_empty_input() {
        assert!(parse_gtk_bookmarks("").is_empty());
        assert!(parse_gtk_bookmarks("\n  \n# only a comment\n").is_empty());
        let many: String = (0..MAX_PLACES + 20)
            .map(|i| format!("file:///d/{i} L{i}\n"))
            .collect();
        let places = parse_gtk_bookmarks(&many);
        assert_eq!(places.len(), MAX_PLACES + 1);
        assert!(reason(places.last().unwrap()).contains("more than"));
        assert_eq!(places[MAX_PLACES - 1].label, format!("L{}", MAX_PLACES - 1));
    }

    const KDE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xbel>\n\
<xbel xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\" xmlns:mime=\"http://www.freedesktop.org/standards/shared-mime-info\" xmlns:kdepriv=\"http://www.kde.org/kdepriv\">\n\
 <bookmark href=\"file:///home/ana\"><title>Home</title><info><metadata owner=\"http://freedesktop.org\"><bookmark:icon name=\"user-home\"/></metadata><metadata owner=\"http://www.kde.org\"><ID>1</ID><isSystemItem>true</isSystemItem></metadata></info></bookmark>\n\
 <bookmark href=\"file:///home/ana/Prostor%20in%20%C4%8Das\"><title>Prostor in čas</title></bookmark>\n\
 <separator/>\n\
 <bookmark href=\"trash:/\"><title>Trash</title></bookmark>\n\
 <bookmark href=\"remote:/\"><title>Network</title></bookmark>\n\
 <bookmark href=\"recentlyused:/files\"><title>Recent Files</title></bookmark>\n\
 <bookmark href=\"timeline:/today\"><title>Today</title></bookmark>\n\
 <bookmark href=\"search:/?kfind=1\"><title>Saved search</title></bookmark>\n\
 <bookmark href=\"file:///mnt/old\"><title>Old disk</title><info><metadata owner=\"http://www.kde.org\"><IsHidden>true</IsHidden></metadata></info></bookmark>\n\
 <bookmark href=\"file:///mnt/shown\"><info><metadata owner=\"http://www.kde.org\"><IsHidden>false</IsHidden></metadata></info></bookmark>\n\
 <bookmark><title>No address</title></bookmark>\n\
</xbel>\n";

    #[test]
    fn kde_places_read_the_real_layout_with_hidden_entries_and_remote_urls() {
        let places = parse_kde_places(KDE).unwrap();
        let s = summary(&places);
        assert_eq!(s[0], ("/home/ana".into(), "Home".into(), false, false));
        assert_eq!(
            s[1],
            (
                "/home/ana/Prostor in čas".into(),
                "Prostor in čas".into(),
                false,
                false
            )
        );
        // The separator is no place: the next entry follows directly.
        assert_eq!(s[2].1, "Trash");
        for (i, scheme) in [
            (2, "trash:"),
            (3, "remote:"),
            (4, "recentlyused:"),
            (5, "timeline:"),
            (6, "search:"),
        ] {
            assert!(s[i].3, "{:?}", s[i]);
            assert!(
                reason(&places[i]).contains(scheme),
                "{}",
                reason(&places[i])
            );
        }
        // Hidden is carried over, a hidden flag set to false is not hidden, a missing title uses the path.
        assert_eq!(s[7], ("/mnt/old".into(), "Old disk".into(), true, false));
        assert_eq!(s[8], ("/mnt/shown".into(), "shown".into(), false, false));
        assert!(s[9].3 && reason(&places[9]).contains("without an address"));
        assert_eq!(places.len(), 10);
    }

    #[test]
    fn kde_places_refuse_malformed_and_dangerous_input_and_accept_empty() {
        assert!(parse_kde_places("").unwrap().is_empty());
        assert!(parse_kde_places("  \n").unwrap().is_empty());
        assert!(parse_kde_places("<xbel><bookmark href=").is_err());
        assert!(parse_kde_places("not xml at all").is_err());
        // A DOCTYPE with declarations (entity expansion), or an external id, is not what KDE writes.
        let bomb = "<?xml version=\"1.0\"?>\n<!DOCTYPE xbel [<!ENTITY a \"aaaa\"><!ENTITY b \"&a;&a;&a;&a;\">]>\n<xbel><bookmark href=\"file:///x\"><title>&b;</title></bookmark></xbel>";
        assert!(parse_kde_places(bomb)
            .unwrap_err()
            .to_string()
            .contains("DOCTYPE"));
        let external = "<!DOCTYPE xbel SYSTEM \"file:///etc/passwd\">\n<xbel><bookmark href=\"file:///x\"/></xbel>";
        assert!(parse_kde_places(external)
            .unwrap_err()
            .to_string()
            .contains("DOCTYPE"));
        assert!(parse_kde_places("<!DOCTYPE xbel").is_err());
        // A file with no bookmarks is a valid, empty list.
        assert!(parse_kde_places("<xbel></xbel>").unwrap().is_empty());
    }

    #[test]
    fn kde_places_cap_the_entries_and_clean_labels() {
        let mut xml = String::from("<xbel>");
        for i in 0..MAX_PLACES + 5 {
            xml.push_str(&format!(
                "<bookmark href=\"file:///d/{i}\"><title>T{i}</title></bookmark>"
            ));
        }
        xml.push_str("</xbel>");
        let places = parse_kde_places(&xml).unwrap();
        assert_eq!(places.len(), MAX_PLACES + 1);
        assert!(reason(places.last().unwrap()).contains("more than"));
        // Control characters of a title become spaces, and a long title is cut.
        let long = "x".repeat(MAX_LABEL + 50);
        let xml = format!("<xbel><bookmark href=\"file:///a\"><title>one&#10;two\tthree</title></bookmark><bookmark href=\"file:///b\"><title>{long}</title></bookmark></xbel>");
        let places = parse_kde_places(&xml).unwrap();
        assert_eq!(places[0].label, "one two three");
        assert_eq!(places[1].label.chars().count(), MAX_LABEL);
    }

    #[test]
    fn percent_decoding_rejects_what_it_cannot_decode() {
        assert_eq!(percent_decode("a%20b%2Fc").unwrap(), "a b/c");
        assert_eq!(percent_decode("%C4%8D").unwrap(), "č");
        assert!(percent_decode("%").is_err());
        assert!(percent_decode("%4").is_err());
        assert!(percent_decode("%g0").is_err());
        assert!(percent_decode("%00").is_err());
        assert!(percent_decode("%C4").is_err());
    }

    #[test]
    fn a_source_file_is_read_with_its_places_examined_and_oversized_input_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real folder");
        std::fs::create_dir(&real).unwrap();
        let file = temp.path().join("a file.txt");
        std::fs::write(&file, "x").unwrap();
        let list = temp.path().join("bookmarks");
        let uri = |p: &Path| {
            let path = p.to_string_lossy().replace('\\', "/").replace(' ', "%20");
            let slash = if path.starts_with('/') { "" } else { "/" };
            format!("file://{slash}{path}")
        };
        std::fs::write(
            &list,
            format!(
                "{} Real\n{} AFile\n{} Gone\nsftp://h/x Remote\n",
                uri(&real),
                uri(&file),
                uri(&temp.path().join("not here"))
            ),
        )
        .unwrap();
        let source = FileSource {
            id: "gtk-bookmarks",
            label: "test",
            file: list.clone(),
            parse: |text| Ok(parse_gtk_bookmarks(text)),
        };
        assert!(source.detect());
        let places = source.read().unwrap();
        assert_eq!(places[0].kind, Kind::Folder);
        assert!(!places[0].missing);
        assert_eq!(places[1].kind, Kind::File);
        // A missing folder is still a folder place that is marked missing; a skipped entry is not examined.
        assert_eq!(places[2].kind, Kind::Folder);
        assert!(places[2].missing);
        assert!(matches!(places[3].kind, Kind::Skipped(_)) && !places[3].missing);
        // Nothing is read for a source that is not there, and a file over the limit is refused.
        let absent = FileSource {
            file: temp.path().join("nothing"),
            ..source
        };
        assert!(!absent.detect());
        let big = temp.path().join("big");
        std::fs::write(&big, vec![b'a'; MAX_LIST_BYTES as usize + 1]).unwrap();
        let error = read_capped(&big).unwrap_err().to_string();
        assert!(error.contains("larger than"), "{error}");
        // A file of exactly the limit is read.
        std::fs::write(&big, vec![b'a'; MAX_LIST_BYTES as usize]).unwrap();
        assert_eq!(read_capped(&big).unwrap().len() as u64, MAX_LIST_BYTES);
    }

    fn group_paths(f: &Favorites, name: &str) -> Vec<(String, Option<String>)> {
        f.groups
            .iter()
            .find(|g| g.name == name)
            .map(|g| {
                g.items
                    .iter()
                    .map(|i| (i.path.clone(), i.alias.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn choice(source: &str, items: &[(&str, Option<&str>)]) -> ImportChoice {
        ImportChoice {
            source: source.into(),
            items: items
                .iter()
                .map(|(path, alias)| ImportItem {
                    path: (*path).into(),
                    alias: alias.map(str::to_owned),
                })
                .collect(),
        }
    }

    #[test]
    fn an_import_adds_one_group_per_source_after_the_existing_ones_and_never_touches_them() {
        let mut f = Favorites::default();
        let mine = f.add_group("Mine");
        f.add_item(mine, "/home/ana/work", None);
        f.set_alias(mine, 0, Some("Work".into()));
        let before = f.groups.clone();
        let receipt = apply_import(
            &mut f,
            &[
                choice(
                    "GTK bookmarks",
                    &[
                        ("/srv/data", Some("Data")),
                        ("/home/ana/music/", None),
                        ("/opt/tools", Some("tools")),
                    ],
                ),
                choice("KDE Places", &[("/mnt/disk", None)]),
            ],
        );
        // Existing groups are the same, in the same place; the new ones follow in order.
        assert_eq!(f.groups[..1], before[..]);
        assert_eq!(f.groups.len(), 3);
        assert_eq!(f.groups[1].name, "Imported from GTK bookmarks");
        assert_eq!(f.groups[2].name, "Imported from KDE Places");
        // Order kept, the trailing slash tidied, an alias equal to the folder name is not stored.
        assert_eq!(
            group_paths(&f, "Imported from GTK bookmarks"),
            vec![
                ("/srv/data".into(), Some("Data".into())),
                ("/home/ana/music".into(), None),
                ("/opt/tools".into(), None),
            ]
        );
        assert_eq!(receipt.groups.len(), 2);
        assert_eq!(receipt.left_out, 0);
    }

    #[test]
    fn importing_again_is_idempotent_and_duplicates_are_left_out_after_normalization() {
        let mut f = Favorites::default();
        let mine = f.add_group("Mine");
        f.add_item(mine, "/home/ana/work", None);
        let items = [
            ("/home/ana/work/", None),
            ("/srv/a", None),
            ("/srv/a", Some("again")),
            ("relative/x", None),
            ("/up/../x", None),
            ("/nul\0x", None),
        ];
        let first = apply_import(&mut f, &[choice("Src", &items)]);
        // Already a favorite (with a trailing slash), a repeat within the batch, and three unusable paths are left out.
        assert_eq!(
            group_paths(&f, "Imported from Src"),
            vec![("/srv/a".into(), None)]
        );
        assert_eq!(first.left_out, 5);
        let snapshot = f.clone();
        let second = apply_import(&mut f, &[choice("Src", &items)]);
        assert_eq!(f, snapshot, "a second import changes nothing");
        assert!(second.groups.is_empty());
        assert_eq!(second.left_out, 6);
        assert!(already_in(&f, "/srv/a/"));
        assert!(!already_in(&f, "/srv/b"));
    }

    #[test]
    fn a_source_with_nothing_left_makes_no_group_and_empty_input_does_nothing() {
        let mut f = Favorites::default();
        let g = f.add_group("Mine");
        f.add_item(g, "/a", None);
        let snapshot = f.clone();
        assert!(apply_import(&mut f, &[]).groups.is_empty());
        assert!(apply_import(&mut f, &[choice("Src", &[])])
            .groups
            .is_empty());
        assert!(apply_import(&mut f, &[choice("Src", &[("/a", None)])])
            .groups
            .is_empty());
        assert_eq!(f, snapshot);
    }

    #[test]
    fn text_is_taken_as_data_and_the_batch_is_capped() {
        let mut f = Favorites::default();
        let items: Vec<(String, Option<String>)> = (0..MAX_PLACES + 30)
            .map(|i| (format!("/d/{i}"), Some(format!("<b>x</b>\n{i}"))))
            .collect();
        let refs: Vec<(&str, Option<&str>)> = items
            .iter()
            .map(|(p, a)| (p.as_str(), a.as_deref()))
            .collect();
        apply_import(&mut f, &[choice("<script>alert(1)</script>", &refs)]);
        assert_eq!(f.groups.len(), 1);
        // Names and aliases are stored as typed text: no markup is interpreted, newlines are spaces.
        assert_eq!(f.groups[0].name, "Imported from <script>alert(1)</script>");
        assert_eq!(f.groups[0].items.len(), MAX_PLACES);
        assert_eq!(f.groups[0].items[3].alias.as_deref(), Some("<b>x</b> 3"));
    }

    #[test]
    fn undo_takes_back_exactly_the_batch_and_keeps_what_the_user_added_since() {
        let mut f = Favorites::default();
        let mine = f.add_group("Mine");
        f.add_item(mine, "/home/ana/work", None);
        let receipt = apply_import(
            &mut f,
            &[
                choice("A", &[("/a/1", None), ("/a/2", None)]),
                choice("B", &[("/b/1", None)]),
            ],
        );
        // The user adds a place of their own to an imported group, and moves nothing else.
        let a = receipt.groups[0].id;
        f.add_item(a, "/mine/extra", None);
        let removed = undo_import(&mut f, &receipt);
        assert_eq!(removed, 3);
        // Group B had only imported places and is gone; group A stays with the user's place.
        assert_eq!(f.groups.len(), 2);
        assert_eq!(
            group_paths(&f, "Mine"),
            vec![("/home/ana/work".into(), None)]
        );
        assert_eq!(
            group_paths(&f, "Imported from A"),
            vec![("/mine/extra".into(), None)]
        );
        assert!(f.groups.iter().all(|g| g.name != "Imported from B"));
        // Undoing again, or with nothing to undo, does nothing.
        let again = f.clone();
        assert_eq!(undo_import(&mut f, &receipt), 0);
        assert_eq!(f, again);
        assert_eq!(undo_import(&mut f, &ImportReceipt::default()), 0);
    }

    #[test]
    fn undo_does_not_touch_a_group_that_the_user_deleted_or_renamed() {
        let mut f = Favorites::default();
        let receipt = apply_import(&mut f, &[choice("A", &[("/a/1", None)])]);
        let id = receipt.groups[0].id;
        f.rename_group(id, "My renamed group");
        assert_eq!(undo_import(&mut f, &receipt), 1);
        // Emptied by the undo, it goes even when renamed: it only ever held what the import put there.
        assert!(f.groups.is_empty());
        let receipt = apply_import(&mut f, &[choice("A", &[("/a/1", None)])]);
        f.delete_group(receipt.groups[0].id);
        assert_eq!(undo_import(&mut f, &receipt), 0);
        assert!(f.groups.is_empty());
    }

    struct TestSflEntry<'a> {
        name: &'a str,
        bookmark: Option<&'a [u8]>,
        visibility: Option<i64>,
        special_id: Option<&'a str>,
    }

    /// Builds a binary `NSKeyedArchiver` `.sfl3` archive matching macOS Finder's layout.
    fn build_test_sfl3(entries: &[TestSflEntry<'_>]) -> Vec<u8> {
        use plist::{Dictionary, Uid, Value};
        let mut objects: Vec<Value> = vec![Value::String("$null".into())];
        let mut push = |v: Value| -> Uid {
            let id = Uid::new(objects.len() as u64);
            objects.push(v);
            id
        };

        // Reserve slot 1 for the root NSDictionary.
        let _root_slot = push(Value::String("$null".into()));
        let key_items = push(Value::String("items".into()));
        let mut item_uids = Vec::new();

        for entry in entries {
            let mut item = Dictionary::new();
            if !entry.name.is_empty() {
                let u = push(Value::String(entry.name.into()));
                item.insert("Name".into(), Value::Uid(u));
            }
            if let Some(bm) = entry.bookmark {
                let u = push(Value::Data(bm.to_vec()));
                item.insert("Bookmark".into(), Value::Uid(u));
            }
            if let Some(vis) = entry.visibility {
                item.insert("Visibility".into(), Value::Integer(vis.into()));
            }
            if let Some(sid) = entry.special_id {
                let k_uid = push(Value::String(
                    "com.apple.LSSharedFileList.SpecialItemIdentifier".into(),
                ));
                let v_uid = push(Value::String(sid.into()));
                let mut custom = Dictionary::new();
                custom.insert("NS.keys".into(), Value::Array(vec![Value::Uid(k_uid)]));
                custom.insert("NS.objects".into(), Value::Array(vec![Value::Uid(v_uid)]));
                let c_uid = push(Value::Dictionary(custom));
                item.insert("CustomItemProperties".into(), Value::Uid(c_uid));
            }
            let item_uid = push(Value::Dictionary(item));
            item_uids.push(Value::Uid(item_uid));
        }

        let mut ns_array = Dictionary::new();
        ns_array.insert("NS.objects".into(), Value::Array(item_uids));
        let arr_uid = push(Value::Dictionary(ns_array));

        let mut root_dict = Dictionary::new();
        root_dict.insert("NS.keys".into(), Value::Array(vec![Value::Uid(key_items)]));
        root_dict.insert("NS.objects".into(), Value::Array(vec![Value::Uid(arr_uid)]));
        objects[1] = Value::Dictionary(root_dict);

        let mut top = Dictionary::new();
        top.insert("root".into(), Value::Uid(Uid::new(1)));

        let mut archive = Dictionary::new();
        archive.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
        archive.insert("$version".into(), Value::Integer(100_000.into()));
        archive.insert("$top".into(), Value::Dictionary(top));
        archive.insert("$objects".into(), Value::Array(objects));

        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(archive)).unwrap();
        out
    }

    #[test]
    fn finder_sfl_parses_order_labels_hidden_and_skips_pseudo_and_broken_entries() {
        let bytes = build_test_sfl3(&[
            TestSflEntry {
                name: "AirDrop",
                bookmark: Some(b"bm-airdrop"),
                visibility: Some(0),
                special_id: Some("com.apple.LSSharedFileList.IsAirDrop"),
            },
            TestSflEntry {
                name: "Recents",
                bookmark: Some(b"bm-recents"),
                visibility: Some(0),
                special_id: Some("com.apple.LSSharedFileList.RecentDocuments"),
            },
            TestSflEntry {
                name: "iCloud Drive",
                bookmark: Some(b"bm-icloud"),
                visibility: Some(0),
                special_id: Some("com.apple.CloudDocs"),
            },
            TestSflEntry {
                name: "Projects",
                bookmark: Some(b"bm-projects"),
                visibility: Some(0),
                special_id: None,
            },
            TestSflEntry {
                name: "Old Archive",
                bookmark: Some(b"bm-hidden"),
                visibility: Some(1),
                special_id: None,
            },
            TestSflEntry {
                name: "",
                bookmark: Some(b"bm-nolabel"),
                visibility: Some(0),
                special_id: None,
            },
            TestSflEntry {
                name: "Broken Entry",
                bookmark: Some(b"bm-bad"),
                visibility: Some(0),
                special_id: None,
            },
            TestSflEntry {
                name: "No Bookmark",
                bookmark: None,
                visibility: Some(0),
                special_id: None,
            },
        ]);
        let items = parse_finder_sfl(&bytes).unwrap();
        assert_eq!(items.len(), 8);

        let places = places_from_finder_items(items, |bm| match bm {
            b"bm-projects" => Ok("/Users/ana/Projects".into()),
            b"bm-hidden" => Ok("/Users/ana/Old Archive".into()),
            b"bm-nolabel" => Ok("/Users/ana/Downloads".into()),
            b"bm-bad" => Err("bookmark could not be resolved".into()),
            _ => unreachable!("pseudo entries must be skipped before bookmark resolution"),
        });

        assert_eq!(places.len(), 8);
        assert!(reason(&places[0]).contains("AirDrop"));
        assert!(reason(&places[1]).contains("Recents"));
        assert!(reason(&places[2]).contains("iCloud"));
        assert_eq!(
            (
                places[3].path.as_str(),
                places[3].label.as_str(),
                places[3].hidden
            ),
            ("/Users/ana/Projects", "Projects", false)
        );
        assert_eq!(
            (
                places[4].path.as_str(),
                places[4].label.as_str(),
                places[4].hidden
            ),
            ("/Users/ana/Old Archive", "Old Archive", true)
        );
        assert_eq!(
            (
                places[5].path.as_str(),
                places[5].label.as_str(),
                places[5].hidden
            ),
            ("/Users/ana/Downloads", "Downloads", false)
        );
        assert!(reason(&places[6]).contains("could not be resolved"));
        assert!(reason(&places[7]).contains("no bookmark"));
    }

    #[test]
    fn finder_sfl_rejects_corrupt_and_oversized_archives() {
        assert!(parse_finder_sfl(b"not a plist").is_err());
        assert!(parse_finder_sfl(&vec![0u8; MAX_LIST_BYTES as usize + 1]).is_err());
    }

    #[test]
    fn nfd_and_nfc_paths_match_as_duplicates_on_macos_rules() {
        let nfc = "/Users/ana/Café/Čas";
        let nfd = "/Users/ana/Cafe\u{0301}/C\u{030c}as";
        assert_eq!(
            same_place_key_for(nfc, true, true),
            same_place_key_for(nfd, true, true)
        );
        assert_eq!(
            same_place_key_for("/Users/Ana/Projects", true, true),
            same_place_key_for("/users/ana/projects/", true, true)
        );
        if cfg!(target_os = "macos") {
            let mut f = Favorites::default();
            let g = f.add_group("Mine");
            f.add_item(g, nfc, None);
            assert!(already_in(&f, nfd));
            let receipt = apply_import(&mut f, &[choice("Finder sidebar", &[(nfd, Some("Čas"))])]);
            assert!(receipt.groups.is_empty());
            assert_eq!(receipt.left_out, 1);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_finder_sfl3_resolves_real_bookmarks_and_reports_unreadable_fda() {
        use objc2_foundation::{NSString, NSURLBookmarkCreationOptions, NSURL};
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("My Projects");
        let unicode = dir.path().join("Cafe\u{0301} Čšž");
        std::fs::create_dir(&proj).unwrap();
        std::fs::create_dir(&unicode).unwrap();

        let make_bookmark = |p: &Path| -> Vec<u8> {
            let url = NSURL::fileURLWithPath(&NSString::from_str(p.to_str().unwrap()));
            let data = url
                .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                    NSURLBookmarkCreationOptions::MinimalBookmark,
                    None,
                    None,
                )
                .expect("create macOS bookmark data");
            data.to_vec()
        };

        let bm_proj = make_bookmark(&proj);
        let bm_uni = make_bookmark(&unicode);
        let sfl_bytes = build_test_sfl3(&[
            TestSflEntry {
                name: "AirDrop",
                bookmark: Some(b"unused"),
                visibility: Some(0),
                special_id: Some("com.apple.LSSharedFileList.IsAirDrop"),
            },
            TestSflEntry {
                name: "Projects",
                bookmark: Some(&bm_proj),
                visibility: Some(0),
                special_id: None,
            },
            TestSflEntry {
                name: "",
                bookmark: Some(&bm_uni),
                visibility: Some(0),
                special_id: None,
            },
            TestSflEntry {
                name: "Damaged",
                bookmark: Some(b"not-a-bookmark"),
                visibility: Some(0),
                special_id: None,
            },
        ]);
        let sfl_path = dir
            .path()
            .join("com.apple.LSSharedFileList.FavoriteItems.sfl3");
        std::fs::write(&sfl_path, &sfl_bytes).unwrap();

        let places = read_finder_sfl_file(&sfl_path).unwrap();
        assert_eq!(places.len(), 4);
        assert!(reason(&places[0]).contains("AirDrop"));
        assert_eq!(places[1].label, "Projects");
        assert_eq!(places[1].kind, Kind::Folder);
        assert!(!places[1].missing);
        assert_eq!(
            std::fs::canonicalize(&places[1].path).unwrap(),
            std::fs::canonicalize(&proj).unwrap()
        );
        assert_eq!(places[2].kind, Kind::Folder);
        assert!(!places[2].missing);
        assert_eq!(
            std::fs::canonicalize(&places[2].path).unwrap(),
            std::fs::canonicalize(&unicode).unwrap()
        );
        assert!(reason(&places[3]).contains("could not be resolved"));

        // Unreadable file (0000): must report Full Disk Access clearly, never guess.
        let locked = dir.path().join("locked.sfl3");
        std::fs::write(&locked, &sfl_bytes).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let err = read_finder_sfl_file(&locked).unwrap_err().to_string();
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644));
        assert!(
            err.contains("Full Disk Access"),
            "expected Full Disk Access guidance, got: {err}"
        );
    }
}
