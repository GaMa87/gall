//! Extensible user/distro context actions (`actions.d/*.json`), `zoxide` integration,
//! and active-terminal CWD lookup (`omarchy-cmd-terminal-cwd`).

use crate::error::{CoreError, Result};
use crate::util::appdirs::AppDirs;
use crate::util::cmdline;
use crate::util::terminal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SelectionMode {
    #[default]
    Any,
    Files,
    Dirs,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActionTerminal {
    #[default]
    None,
    Terminal,
    FloatingPresentation,
}

impl Serialize for ActionTerminal {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::None => serializer.serialize_bool(false),
            Self::Terminal => serializer.serialize_bool(true),
            Self::FloatingPresentation => serializer.serialize_str("floating-presentation"),
        }
    }
}

impl<'de> Deserialize<'de> for ActionTerminal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ActionTerminal;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("boolean or \"floating-presentation\"")
            }

            fn visit_bool<E: serde::de::Error>(
                self,
                v: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(if v {
                    ActionTerminal::Terminal
                } else {
                    ActionTerminal::None
                })
            }

            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                match v.trim() {
                    "floating-presentation" | "floatingPresentation" => {
                        Ok(ActionTerminal::FloatingPresentation)
                    }
                    "terminal" | "true" => Ok(ActionTerminal::Terminal),
                    "none" | "false" => Ok(ActionTerminal::None),
                    other => Err(E::unknown_variant(
                        other,
                        &["false", "true", "floating-presentation"],
                    )),
                }
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActionCommand {
    Argv(Vec<String>),
    Template(String),
}

impl Default for ActionCommand {
    fn default() -> Self {
        Self::Argv(Vec::new())
    }
}

impl ActionCommand {
    fn is_empty(&self) -> bool {
        match self {
            Self::Argv(v) => v.is_empty() || v.iter().all(|s| s.trim().is_empty()),
            Self::Template(s) => s.trim().is_empty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WhenCommand {
    One(String),
    All(Vec<String>),
}

impl WhenCommand {
    fn commands(&self) -> Vec<&str> {
        match self {
            Self::One(s) => vec![s.as_str()],
            Self::All(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionDef {
    pub id: String,
    #[serde(default, alias = "name")]
    pub label: String,
    #[serde(default, alias = "labelMulti", skip_serializing_if = "Option::is_none")]
    pub plural_label: Option<String>,
    #[serde(default, alias = "exec")]
    pub command: ActionCommand,
    #[serde(
        default,
        alias = "when_command",
        skip_serializing_if = "Option::is_none"
    )]
    pub when_command: Option<WhenCommand>,
    #[serde(default)]
    pub selection: SelectionMode,
    #[serde(default)]
    pub single: bool,
    #[serde(default, alias = "mime_prefixes")]
    pub mime_prefixes: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub terminal: ActionTerminal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionTarget {
    pub path: PathBuf,
    pub is_dir: bool,
    pub ext: String,
    pub mime: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ActionFile {
    One(ActionDef),
    Many(Vec<ActionDef>),
}

/// System-wide `actions.d` directories (lowest-to-highest priority).
pub fn system_actions_dirs() -> Vec<PathBuf> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let raw = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        actions_dirs_from_xdg(&raw)
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        Vec::new()
    }
}

/// `gall/actions.d` under each absolute entry of an `XDG_DATA_DIRS` value, lowest priority first.
/// Relative entries are ignored (the spec says so): they would resolve against the current folder and
/// let any folder define commands. Earlier entries have the higher priority.
#[cfg(any(all(unix, not(target_os = "macos")), all(unix, test)))]
fn actions_dirs_from_xdg(raw: &str) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut dirs: Vec<PathBuf> = raw
        .split(':')
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("gall/actions.d"))
        .filter(|d| seen.insert(d.clone()))
        .collect();
    dirs.reverse();
    dirs
}

/// Loads and merges `*.json` action definitions from `dirs` in lowest-to-highest priority.
/// Within each directory, files are sorted lexicographically by filename.
/// Duplicate `id`s replace the earlier entry; `"enabled": false` removes/disables an `id`.
pub fn load_actions_from_dirs(dirs: &[PathBuf]) -> Vec<ActionDef> {
    let mut out: Vec<ActionDef> = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            })
            .collect();
        files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        for file in files {
            let text = match std::fs::read_to_string(&file) {
                Ok(t) => t,
                Err(e) => {
                    log::warn!("cannot read action file {}: {e}", file.display());
                    continue;
                }
            };
            let parsed = match serde_json::from_str::<ActionFile>(&text) {
                Ok(ActionFile::One(a)) => vec![a],
                Ok(ActionFile::Many(v)) => v,
                Err(e) => {
                    log::warn!("ignoring malformed action file {}: {e}", file.display());
                    continue;
                }
            };
            for mut action in parsed {
                action.id = action.id.trim().to_string();
                if action.id.is_empty() {
                    log::warn!("ignoring action without id in {}", file.display());
                    continue;
                }
                if !action.enabled {
                    out.retain(|existing| existing.id != action.id);
                    continue;
                }
                if action.label.trim().is_empty() || action.command.is_empty() {
                    log::warn!(
                        "ignoring incomplete action '{}' in {}",
                        action.id,
                        file.display()
                    );
                    continue;
                }
                action.extensions = action
                    .extensions
                    .into_iter()
                    .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                    .filter(|e| !e.is_empty())
                    .collect();
                if let Some(pos) = out.iter().position(|existing| existing.id == action.id) {
                    out[pos] = action;
                } else {
                    out.push(action);
                }
            }
        }
    }
    out
}

/// Loads actions from system `actions.d` directories and the user's `actions.d` folder.
pub fn load_actions(config_dir: Option<&Path>) -> Vec<ActionDef> {
    let mut dirs = system_actions_dirs();
    let user_dir = config_dir
        .map(|d| d.join("actions.d"))
        .unwrap_or_else(|| AppDirs::resolve(None).actions());
    dirs.push(user_dir);
    load_actions_from_dirs(&dirs)
}

fn normalized_ext(target: &ActionTarget) -> String {
    let raw = target.ext.trim().trim_start_matches('.');
    if !raw.is_empty() {
        return raw.to_ascii_lowercase();
    }
    target
        .path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

fn infer_mime_from_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "heic" => "image/heic",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "jxl" => "image/jxl",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mpeg" | "mpg" => "video/mpeg",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "m4a" | "aac" => "audio/mp4",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" | "csv" => "text/plain",
        "json" => "application/json",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        _ => return None,
    })
}

fn target_mime(target: &ActionTarget, ext: &str) -> Option<String> {
    if target.is_dir {
        return Some("inode/directory".into());
    }
    target
        .mime
        .clone()
        .filter(|m| !m.is_empty())
        .or_else(|| infer_mime_from_ext(ext).map(str::to_string))
}

/// Filters `actions` against `targets` using injectable command lookup `has_cmd`.
pub fn filter_actions_with<F: Fn(&str) -> bool>(
    actions: &[ActionDef],
    targets: &[ActionTarget],
    has_cmd: F,
) -> Vec<ActionDef> {
    actions
        .iter()
        .filter(|action| {
            if !action.enabled {
                return false;
            }
            if let Some(ref wc) = action.when_command {
                let cmds = wc.commands();
                if cmds.is_empty()
                    || !cmds
                        .iter()
                        .all(|c| !c.trim().is_empty() && has_cmd(c.trim()))
                {
                    return false;
                }
            }
            let selection_ok = match action.selection {
                SelectionMode::None => targets.is_empty(),
                SelectionMode::Any => !targets.is_empty(),
                SelectionMode::Files => !targets.is_empty() && targets.iter().all(|t| !t.is_dir),
                SelectionMode::Dirs => !targets.is_empty() && targets.iter().all(|t| t.is_dir),
            };
            if !selection_ok {
                return false;
            }
            if action.single && targets.len() != 1 {
                return false;
            }
            if !action.mime_prefixes.is_empty() || !action.extensions.is_empty() {
                if targets.is_empty() {
                    return false;
                }
                let types_ok = targets.iter().all(|t| {
                    let ext = normalized_ext(t);
                    let ext_ok = !action.extensions.is_empty()
                        && action
                            .extensions
                            .iter()
                            .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext));
                    let mime = target_mime(t, &ext);
                    let mime_ok = !action.mime_prefixes.is_empty()
                        && mime
                            .as_deref()
                            .is_some_and(|m| action.mime_prefixes.iter().any(|p| m.starts_with(p)));
                    ext_ok || mime_ok
                });
                if !types_ok {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

/// Filters `actions` against `targets` using `cmdline::which` for `whenCommand` checks.
pub fn filter_actions(actions: &[ActionDef], targets: &[ActionTarget]) -> Vec<ActionDef> {
    filter_actions_with(actions, targets, cmdline::which)
}

fn replace_placeholders_single_pass(
    token: &str,
    path_val: Option<&str>,
    paths_val: Option<&str>,
    dir_val: &str,
) -> String {
    let mut out = String::with_capacity(token.len());
    let mut rest = token;
    while let Some(c) = rest.chars().next() {
        if let Some(p) = paths_val {
            if let Some(tail) = rest.strip_prefix("{paths}") {
                out.push_str(p);
                rest = tail;
                continue;
            }
        }
        if let Some(p) = path_val {
            if let Some(tail) = rest.strip_prefix("{path}") {
                out.push_str(p);
                rest = tail;
                continue;
            }
        }
        if let Some(tail) = rest.strip_prefix("{dir}") {
            out.push_str(dir_val);
            rest = tail;
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Most processes one action may start for a selection when its command takes a single `{path}`.
pub const MAX_INVOCATIONS: usize = 20;

/// Expands `{path}`, `{paths}`, and `{dir}` placeholders into concrete `Vec<Vec<String>>`
/// invocations without shell interpolation.
/// - `{paths}` expands all `paths` as separate arguments in a single invocation.
/// - `{path}` without `{paths}` and `paths.len() > 1` produces one invocation per path
///   (at most [`MAX_INVOCATIONS`]).
/// - When a command uses both, `{path}` in tokens without `{paths}` is the first selected item
///   (or `dir` for an empty selection); only `{paths}` tokens cover the whole selection.
pub fn expand_action_argv(
    cmd: &ActionCommand,
    paths: &[PathBuf],
    dir: &Path,
) -> Result<Vec<Vec<String>>> {
    let raw_tokens: Vec<String> = match cmd {
        ActionCommand::Argv(v) => v.clone(),
        ActionCommand::Template(s) => cmdline::split_command(s),
    };
    if raw_tokens.is_empty() || raw_tokens.first().is_none_or(|s| s.trim().is_empty()) {
        return Err(CoreError::Other("empty action command".into()));
    }
    let dir_str = dir.to_string_lossy();
    let has_paths = raw_tokens.iter().any(|t| t.contains("{paths}"));
    let has_path = raw_tokens.iter().any(|t| t.contains("{path}"));

    if !has_paths && has_path && paths.len() > 1 {
        // One process (or terminal window) per item: refuse a selection that would flood the desktop.
        if paths.len() > MAX_INVOCATIONS {
            return Err(CoreError::Other(format!(
                "this action runs once per item: select at most {MAX_INVOCATIONS} items ({} are selected)",
                paths.len()
            )));
        }
        let mut invocations = Vec::with_capacity(paths.len());
        for p in paths {
            let p_str = p.to_string_lossy();
            let argv: Vec<String> = raw_tokens
                .iter()
                .map(|t| replace_placeholders_single_pass(t, Some(&p_str), None, &dir_str))
                .collect();
            invocations.push(argv);
        }
        return Ok(invocations);
    }

    let first_path = paths
        .first()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir_str.clone().into_owned());
    let mut argv = Vec::with_capacity(raw_tokens.len() + paths.len());
    for t in &raw_tokens {
        if t == "{paths}" {
            for p in paths {
                argv.push(p.to_string_lossy().into_owned());
            }
        } else if t.contains("{paths}") {
            for p in paths {
                let p_str = p.to_string_lossy();
                argv.push(replace_placeholders_single_pass(
                    t,
                    Some(&p_str),
                    Some(&p_str),
                    &dir_str,
                ));
            }
        } else {
            argv.push(replace_placeholders_single_pass(
                t,
                Some(&first_path),
                None,
                &dir_str,
            ));
        }
    }
    if argv.is_empty() {
        return Err(CoreError::Other("empty action command".into()));
    }
    Ok(vec![argv])
}

/// Strictly single-quotes `s` for POSIX `sh` / `bash -c` evaluation (`'` -> `'\''`).
pub fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Joins one or more argv invocations into a single shell-safe presentation command string
/// for `omarchy-launch-floating-terminal-with-presentation`.
pub fn presentation_command(invocations: &[Vec<String>]) -> String {
    invocations
        .iter()
        .map(|argv| {
            argv.iter()
                .map(|a| shell_quote(a))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Executes `action` for `paths` inside `dir`, respecting `action.terminal` and falling back
/// from `omarchy-launch-floating-terminal-with-presentation` to `run_command_in_terminal`.
pub fn run_action(
    action: &ActionDef,
    paths: &[PathBuf],
    dir: &Path,
    configured_terminal: &str,
) -> Result<()> {
    let invocations = expand_action_argv(&action.command, paths, dir)?;
    match action.terminal {
        ActionTerminal::FloatingPresentation => {
            if cmdline::which("omarchy-launch-floating-terminal-with-presentation") {
                let cmd_str = presentation_command(&invocations);
                return cmdline::spawn(
                    &[
                        "omarchy-launch-floating-terminal-with-presentation".into(),
                        cmd_str,
                    ],
                    Some(dir),
                );
            }
            for inv in invocations {
                terminal::run_command_in_terminal(&inv, Some(dir), configured_terminal)?;
            }
            Ok(())
        }
        ActionTerminal::Terminal => {
            for inv in invocations {
                terminal::run_command_in_terminal(&inv, Some(dir), configured_terminal)?;
            }
            Ok(())
        }
        ActionTerminal::None => {
            for inv in invocations {
                cmdline::spawn(&inv, Some(dir))?;
            }
            Ok(())
        }
    }
}

/// True when `zoxide` is available on `$PATH`.
pub fn zoxide_available() -> bool {
    cmdline::which("zoxide")
}

/// Pure parser for `zoxide query -l` stdout (preserves frecency order, deduplicates, trims `\r`).
pub fn parse_zoxide_output(stdout: &str) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in stdout.lines() {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        let pb = PathBuf::from(line);
        if seen.insert(pb.clone()) {
            out.push(pb);
        }
    }
    out
}

/// Queries `zoxide query -l`. Returns `Ok(Vec::new())` if `zoxide` is absent or exits
/// non-zero (e.g. uninitialized database), ignoring stderr diagnostics.
pub fn zoxide_dirs() -> Result<Vec<PathBuf>> {
    if !zoxide_available() {
        return Ok(Vec::new());
    }
    let out = std::process::Command::new("zoxide")
        .args(["query", "-l"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| CoreError::Other(format!("cannot run zoxide: {e}")))?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(parse_zoxide_output(&stdout))
}

/// Pure parser for `omarchy-cmd-terminal-cwd` stdout.
pub fn parse_terminal_cwd_output(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .find(|l| !l.is_empty())
        .map(PathBuf::from)
}

/// Returns the active terminal's CWD via `omarchy-cmd-terminal-cwd` on Linux,
/// or `None` if unavailable or not a valid directory.
pub fn omarchy_terminal_cwd() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        if !cmdline::which("omarchy-cmd-terminal-cwd") {
            return None;
        }
        let out = std::process::Command::new("omarchy-cmd-terminal-cwd")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let path = parse_terminal_cwd_output(&String::from_utf8_lossy(&out.stdout))?;
        path.is_dir().then_some(path)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_per_item_action_refuses_a_flood_of_processes() {
        let cmd = ActionCommand::Argv(vec!["tool".into(), "{path}".into()]);
        let many: Vec<PathBuf> = (0..=MAX_INVOCATIONS)
            .map(|i| PathBuf::from(format!("/d/f{i}")))
            .collect();
        assert!(expand_action_argv(&cmd, &many, Path::new("/d")).is_err());
        let ok = expand_action_argv(&cmd, &many[..MAX_INVOCATIONS], Path::new("/d")).unwrap();
        assert_eq!(ok.len(), MAX_INVOCATIONS);
        // `{paths}` is one process however many items.
        let all = ActionCommand::Argv(vec!["tool".into(), "{paths}".into()]);
        assert_eq!(
            expand_action_argv(&all, &many, Path::new("/d"))
                .unwrap()
                .len(),
            1
        );
    }

    /// `XDG_DATA_DIRS` is a Unix list (`:`-separated absolute paths).
    #[cfg(unix)]
    #[test]
    fn xdg_data_dirs_entries_must_be_absolute_and_keep_their_priority() {
        let d = super::actions_dirs_from_xdg(
            "/home/u/.local/share:rel/dir:./x::/usr/local/share:/usr/share:/home/u/.local/share",
        );
        let names: Vec<_> = d.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert_eq!(
            names,
            ["/usr/share/gall/actions.d", "/usr/local/share/gall/actions.d", "/home/u/.local/share/gall/actions.d"],
            "lowest priority first, relative entries dropped, a repeated entry keeps its first (highest) place"
        );
    }

    use super::*;

    #[test]
    fn loads_merges_overrides_and_disables_actions_from_dirs() {
        let t = tempfile::tempdir().unwrap();
        let sys = t.path().join("sys");
        let user = t.path().join("user");
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::create_dir_all(&user).unwrap();

        // Single-object JSON, array JSON, and malformed JSON (skipped cleanly).
        std::fs::write(
            sys.join("10-first.json"),
            r#"{"id":"a.one","label":"One","command":["echo","{path}"]}"#,
        )
        .unwrap();
        std::fs::write(
            sys.join("50-omarchy.json"),
            r#"[
                {"id":"omarchy.transcode","label":"Transcode","command":["omarchy-transcode","{path}"],"extensions":[".JPG","png"],"terminal":"floating-presentation"},
                {"id":"omarchy.localsend","label":"Send","command":["localsend","--headless","send","{paths}"]}
            ]"#,
        )
        .unwrap();
        std::fs::write(sys.join("99-broken.json"), "{ invalid json").unwrap();

        // User overrides `a.one` and disables `omarchy.transcode` without repeating command/label.
        std::fs::write(
            user.join("20-override.json"),
            r#"[
                {"id":"a.one","label":"One Custom","command":"custom {path}","terminal":true},
                {"id":"omarchy.transcode","enabled":false}
            ]"#,
        )
        .unwrap();

        let actions = load_actions_from_dirs(&[sys, user]);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].id, "a.one");
        assert_eq!(actions[0].label, "One Custom");
        assert_eq!(actions[0].terminal, ActionTerminal::Terminal);
        assert_eq!(actions[1].id, "omarchy.localsend");
    }

    #[test]
    fn filters_by_when_command_selection_single_mime_and_ext() {
        let transcode: ActionDef = serde_json::from_str(
            r#"{
                "id": "omarchy.transcode",
                "label": "Transcode",
                "command": ["omarchy-transcode", "{path}"],
                "whenCommand": ["omarchy-transcode", "omarchy-launch-floating-terminal-with-presentation"],
                "selection": "files",
                "mimePrefixes": ["image/", "video/"],
                "extensions": ["jpg", "mp4"],
                "terminal": "floating-presentation"
            }"#,
        )
        .unwrap();
        let wallpaper: ActionDef = serde_json::from_str(
            r#"{
                "id": "omarchy.wallpaper",
                "label": "Set Wallpaper",
                "command": ["omarchy-theme-bg-set", "{path}"],
                "whenCommand": "omarchy-theme-bg-set",
                "selection": "files",
                "single": true,
                "mimePrefixes": ["image/"]
            }"#,
        )
        .unwrap();
        let folder_only: ActionDef = serde_json::from_str(
            r#"{
                "id": "custom.dir",
                "label": "Dir",
                "command": ["ls", "{path}"],
                "selection": "dirs"
            }"#,
        )
        .unwrap();
        let none_sel: ActionDef = serde_json::from_str(
            r#"{
                "id": "custom.bg",
                "label": "Bg",
                "command": ["pwd"],
                "selection": "none"
            }"#,
        )
        .unwrap();

        let all = [transcode, wallpaper, folder_only, none_sel];
        let img = ActionTarget {
            path: PathBuf::from("/tmp/photo.JPG"),
            is_dir: false,
            ext: "JPG".into(),
            mime: None,
        };
        let img2 = ActionTarget {
            path: PathBuf::from("/tmp/other.png"),
            is_dir: false,
            ext: ".png".into(),
            mime: None,
        };
        let txt = ActionTarget {
            path: PathBuf::from("/tmp/note.txt"),
            is_dir: false,
            ext: "txt".into(),
            mime: None,
        };
        let dir = ActionTarget {
            path: PathBuf::from("/tmp/folder"),
            is_dir: true,
            ext: String::new(),
            mime: None,
        };

        let has_all = |_: &str| true;
        let ids = |targets: &[ActionTarget]| -> Vec<String> {
            filter_actions_with(&all, targets, has_all)
                .into_iter()
                .map(|a| a.id)
                .collect()
        };

        assert_eq!(
            ids(std::slice::from_ref(&img)),
            ["omarchy.transcode", "omarchy.wallpaper"]
        );
        // Multi-file selection drops `single: true` wallpaper.
        assert_eq!(ids(&[img.clone(), img2]), ["omarchy.transcode"]);
        assert!(ids(&[txt]).is_empty());
        assert_eq!(ids(&[dir]), ["custom.dir"]);
        assert_eq!(ids(&[]), ["custom.bg"]);

        // Missing whenCommand hides action.
        let filtered = filter_actions_with(&all, &[img], |cmd| cmd == "omarchy-theme-bg-set");
        assert_eq!(
            filtered.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["omarchy.wallpaper"]
        );
    }

    #[test]
    fn expands_argv_and_shell_quotes_adversarial_filenames() {
        let dir = Path::new("/tmp/work dir");
        let p1 = PathBuf::from("/tmp/work dir/$(touch /tmp/pwned); 'quoted' {dir}.jpg");
        let p2 = PathBuf::from("/tmp/work dir/$HOME `id` & 100%.png");

        // `{paths}` expands N paths as separate arguments in one invocation.
        let multi_argv = expand_action_argv(
            &ActionCommand::Argv(vec![
                "localsend".into(),
                "--headless".into(),
                "send".into(),
                "{paths}".into(),
            ]),
            &[p1.clone(), p2.clone()],
            dir,
        )
        .unwrap();
        assert_eq!(multi_argv.len(), 1);
        assert_eq!(
            multi_argv[0],
            vec![
                "localsend",
                "--headless",
                "send",
                "/tmp/work dir/$(touch /tmp/pwned); 'quoted' {dir}.jpg",
                "/tmp/work dir/$HOME `id` & 100%.png",
            ]
        );

        // `{path}` without `{paths}` and N>1 paths produces N separate invocations.
        let per_file = expand_action_argv(
            &ActionCommand::Template("omarchy-transcode --in={dir} {path}".into()),
            &[p1.clone(), p2.clone()],
            dir,
        )
        .unwrap();
        assert_eq!(per_file.len(), 2);
        assert_eq!(
            per_file[0],
            vec![
                "omarchy-transcode",
                "--in=/tmp/work dir",
                "/tmp/work dir/$(touch /tmp/pwned); 'quoted' {dir}.jpg",
            ]
        );

        // Presentation command strictly single-quotes every argument.
        let pres = presentation_command(&per_file);
        assert!(pres.contains("'\\''quoted'\\''"));
        assert!(pres.contains("$(touch /tmp/pwned)"));
        assert!(pres.contains("'; '"));
        assert!(!pres.contains("\"/tmp"));

        // `sh -c '...' _ {paths}` style (as in 50-omarchy.json transcode) keeps adversarial args safe.
        let sh_argv = expand_action_argv(
            &ActionCommand::Argv(vec![
                "sh".into(),
                "-c".into(),
                "for f in \"$@\"; do omarchy-transcode -- \"$f\"; done".into(),
                "_".into(),
                "{paths}".into(),
            ]),
            &[p1, p2],
            dir,
        )
        .unwrap();
        assert_eq!(sh_argv.len(), 1);
        assert_eq!(sh_argv[0].len(), 6);
        assert!(expand_action_argv(&ActionCommand::Argv(vec!["".into()]), &[], dir).is_err());
    }

    #[test]
    fn path_next_to_paths_is_the_first_item_in_one_invocation() {
        let dir = Path::new("/d");
        let a = PathBuf::from("/d/a");
        let b = PathBuf::from("/d/b");
        let out = expand_action_argv(
            &ActionCommand::Argv(vec!["tool".into(), "--ref={path}".into(), "{paths}".into()]),
            &[a, b],
            dir,
        )
        .unwrap();
        assert_eq!(out, vec![vec!["tool", "--ref=/d/a", "/d/a", "/d/b"]]);
    }

    #[test]
    fn plural_label_is_canonical_and_accepts_legacy_alias() {
        let canonical: ActionDef = serde_json::from_str(
            r#"{"id":"x","label":"Send","pluralLabel":"Send {count}","command":["echo","{paths}"]}"#,
        )
        .unwrap();
        assert_eq!(canonical.plural_label.as_deref(), Some("Send {count}"));
        let json = serde_json::to_string(&canonical).unwrap();
        assert!(json.contains(r#""pluralLabel":"Send {count}""#));

        let legacy: ActionDef = serde_json::from_str(
            r#"{"id":"x","label":"Send","labelMulti":"Send {count}","command":["echo","{paths}"]}"#,
        )
        .unwrap();
        assert_eq!(legacy.plural_label.as_deref(), Some("Send {count}"));
    }

    #[test]
    fn parses_zoxide_and_terminal_cwd_outputs() {
        let z = parse_zoxide_output("/home/u/proj\r\n\n/home/u/docs\n/home/u/proj\n");
        assert_eq!(
            z,
            vec![PathBuf::from("/home/u/proj"), PathBuf::from("/home/u/docs")]
        );
        assert!(parse_zoxide_output("\n  \r\n").is_empty());

        assert_eq!(
            parse_terminal_cwd_output("\n  /home/u/code\r\n"),
            Some(PathBuf::from("/home/u/code"))
        );
        assert_eq!(parse_terminal_cwd_output("   \n"), None);
    }
}
