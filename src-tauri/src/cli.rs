//! Command-line startup options (X-01).
//!
//! ```text
//! gall [PATH...]                      open folder(s), or parent folder + select file(s)
//! gall -p PATH | --path PATH          browser 1
//! gall -p2 PATH                       browser 2
//! gall -o PATH [--new-tab|--newtab]   open in running instance (same or new tab)
//! gall -w | --new-window [PATH...]    open in a new window
//! gall --select PATH | -s PATH        select item(s) after opening (repeatable)
//! gall --compare A B                  open compare view for A and B (also `gall diff A B`)
//! gall --compare -- A B               the same, A and B taken literally (a name that starts with dashes)
//! gall diff --wait A B                the caller waits until the compare window is closed
//! gall compare-files [-- FILE]         choose the input files in a native dialog
//! gall merge BASE LOCAL REMOTE OUTPUT three-way merge; the caller waits and exits 0 accepted, 1 cancelled, 2 error
//!                                      (BASE may be an empty argument: the file has no common ancestor)
//! gall --git REPO                     open git view for REPO
//! gall --nowindow | --daemon          start without creating an initial window
//! ```

use std::path::{Path, PathBuf};

pub(crate) const HANDOFF_FLAG: &str = "--gall-compare-result=";

/// One file-type filter of a picker: `Images|*.png;*.jpg`.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickFilter {
    pub name: String,
    /// Globs such as `*.png`; empty means every file.
    pub patterns: Vec<String>,
}

/// An extra option of a picker (the portal's `choices`): a checkbox (no `options`; the value is
/// `"true"` or `"false"`) or a combo box (the value is one of the option ids).
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickChoice {
    pub id: String,
    pub label: String,
    /// `(id, label)` pairs.
    #[serde(default)]
    pub options: Vec<(String, String)>,
    #[serde(default)]
    pub initial: String,
}

/// `--pick-choices '<json>'`: a list of [`PickChoice`]; malformed or duplicate entries are dropped.
fn parse_pick_choices(raw: &str) -> Vec<PickChoice> {
    let list: Vec<serde_json::Value> = serde_json::from_str(raw).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    list.into_iter()
        .filter_map(|v| serde_json::from_value::<PickChoice>(v).ok())
        .filter(|c| !c.id.is_empty() && seen.insert(c.id.clone()))
        .collect()
}

/// `gall --pick open|save|dir`: a window that returns the user's choice to the caller (the
/// `xdg-desktop-portal` FileChooser backend) through a result file.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickArgs {
    /// `open` (files), `dir` (folders) or `save`.
    pub mode: String,
    /// JSON file written when the user decides: `{"status":0|1,"paths":[…],"filter":n}`.
    pub result: String,
    pub multiple: bool,
    pub title: Option<String>,
    /// Suggested file name (save).
    pub name: Option<String>,
    pub accept: Option<String>,
    pub filters: Vec<PickFilter>,
    pub choices: Vec<PickChoice>,
}

fn parse_pick_filter(raw: &str) -> PickFilter {
    let (name, globs) = raw.split_once('|').unwrap_or((raw, ""));
    PickFilter {
        name: name.trim().to_string(),
        patterns: globs
            .split(';')
            .map(str::trim)
            .filter(|g| !g.is_empty())
            .map(String::from)
            .collect(),
    }
}

/// The operands of `gall merge`, with relative ones made absolute against the caller's folder.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeArgs {
    /// None when the base operand is empty: the file was added on both sides.
    pub base: Option<String>,
    pub local: String,
    pub remote: String,
    /// The file that the accepted result is written to; it is never touched otherwise.
    pub output: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliArgs {
    pub path1: Option<String>,
    pub path2: Option<String>,
    pub paths: Vec<String>,
    pub select_paths: Vec<String>,
    pub new_tab: bool,
    pub new_window: bool,
    pub no_window: bool,
    pub daemon: bool,
    pub compare: Option<(String, String)>,
    /// `gall merge BASE LOCAL REMOTE OUTPUT`: a merge that the caller waits for (see `gall_core::handoff`).
    pub merge: Option<MergeArgs>,
    /// `--wait`: the caller does not return before the user is done with the compare window.
    pub wait: bool,
    /// Private result path of a child launch. Never send it to the webview.
    #[serde(skip)]
    pub handoff: Option<PathBuf>,
    pub compare_picker: bool,
    pub compare_seed: Option<String>,
    pub git_repo: Option<String>,
    /// Debug builds: run the UI self-test (GALL_SELFTEST=1 or --selftest).
    pub selftest: bool,
    /// "demo" = README screenshots instead of the test suite.
    pub selftest_mode: String,
    /// Picker window (see [`PickArgs`]); implies `new_window`.
    pub pick: Option<PickArgs>,
}

impl CliArgs {
    /// True when the invocation specifies at least one folder, selection, compare, or git target.
    pub fn has_launch_action(&self) -> bool {
        self.path1.is_some()
            || self.path2.is_some()
            || !self.paths.is_empty()
            || !self.select_paths.is_empty()
            || self.compare.is_some()
            || self.compare_picker
            || self.merge.is_some()
            || self.git_repo.is_some()
    }
}

/// A usage error in the raw arguments (`--compare` needs two paths), reported on stderr with a
/// non-zero exit before any window or single-instance forwarding starts.
/// `gall diff|git|select ...` are the subcommand spellings of `--compare`, `--git` and `--select`.
/// Only the first argument counts; write `./diff` for a folder of that name.
pub fn expand_subcommand<S: AsRef<str>>(args: &[S]) -> Vec<String> {
    let mut out: Vec<String> = args.iter().map(|a| a.as_ref().to_string()).collect();
    if matches!(out.get(1..3), Some([flag, pid]) if flag == gall_core::updater::WAIT_PID_FLAG && pid.parse::<u32>().is_ok())
    {
        out.drain(1..3);
    }
    let flag = match out.get(1).map(String::as_str) {
        Some("diff") => "--compare",
        Some("merge") => "--merge",
        Some("git") => "--git",
        Some("select") => "--select",
        _ => return out,
    };
    out[1] = flag.to_string();
    // `gall diff --wait A B`: the flag belongs to the command, not to the operands that follow it, so it moves in
    // front of the expanded flag. Operands after a lone `--` are left alone.
    if flag == "--compare" || flag == "--merge" {
        let stop = out
            .iter()
            .skip(2)
            .position(|a| a == "--")
            .map_or(out.len(), |at| at + 2);
        let mut waits = false;
        let mut kept: Vec<String> = Vec::with_capacity(out.len());
        for (at, a) in out.into_iter().enumerate() {
            if at >= 2 && at < stop && a == "--wait" {
                waits = true;
            } else {
                kept.push(a);
            }
        }
        if waits {
            kept.insert(1, "--wait".to_string());
        }
        return kept;
    }
    out
}

/// The `n` operands of a flag. After a lone `--` they are taken literally, whatever they start with (a name such as
/// `--odd` or `-x`); without it an operand that starts with `--` is the next flag, not an operand. `None`: too few.
fn take_operands<T: AsRef<str>>(it: &mut impl Iterator<Item = T>, n: usize) -> Option<Vec<T>> {
    let first = it.next()?;
    let literal = first.as_ref() == "--";
    let mut got = if literal { Vec::new() } else { vec![first] };
    while got.len() < n {
        got.push(it.next()?);
    }
    (literal || got.iter().all(|v| !v.as_ref().starts_with("--"))).then_some(got)
}

pub fn usage_error<S: AsRef<str>>(args: &[S]) -> Option<String> {
    let expanded = expand_subcommand(args);
    if expanded.get(1).is_some_and(|a| a == "compare-files") {
        return compare_seed(&expanded[2..]).err().map(str::to_string);
    }
    let mut it = expanded.iter().skip(1).map(String::as_str);
    let (mut waits, mut waitable) = (false, false);
    while let Some(a) = it.next() {
        if let Some(result) = a.strip_prefix(HANDOFF_FLAG) {
            if !crate::compare_wait::is_result_path(Path::new(result)) {
                return Some("gall: invalid private comparison result path".into());
            }
        }
        waits |= a == "--wait";
        waitable |= a == "--compare" || a == "--merge";
        if a == "--compare" && take_operands(&mut it, 2).is_none() {
            return Some(
                "gall: --compare requires two paths: gall --compare <left> <right>".into(),
            );
        }
        if a == "--merge" && take_operands(&mut it, 4).is_none() {
            return Some(
                "gall: merge requires four paths: gall merge <base> <local> <remote> <output> (an empty <base> means none; put -- before names that start with dashes)".into(),
            );
        }
    }
    if waits && !waitable {
        return Some(
            "gall: --wait applies to diff and merge: gall diff --wait <left> <right>".into(),
        );
    }
    None
}

fn compare_seed(args: &[String]) -> Result<Option<&str>, &'static str> {
    // A previously installed launcher may still append its process-wait switch.
    let args = match args {
        [operands @ .., flag, pid]
            if flag == gall_core::updater::WAIT_PID_FLAG && pid.parse::<u32>().is_ok() =>
        {
            operands
        }
        _ => args,
    };
    match args {
        [] => Ok(None),
        [seed] if !seed.starts_with("--") && !seed.is_empty() => Ok(Some(seed)),
        [separator, seed] if separator == "--" && !seed.is_empty() => Ok(Some(seed)),
        _ => Err("gall: compare-files accepts at most one file: gall compare-files [-- FILE]"),
    }
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> CliArgs {
    parse_in(args, None)
}

fn push_unique(list: &mut Vec<String>, val: String) {
    if !val.is_empty() && !list.contains(&val) {
        list.push(val);
    }
}

fn strip_windows_shell_dot(path: &str) -> String {
    if let Some(prefix) = path.strip_suffix(r"\.") {
        // Drive roots such as `C:\.` or `C:\\.` become `C:\`; other paths drop the trailing `\.`.
        let trimmed = prefix.trim_end_matches('\\');
        if trimmed.len() == 2 && trimmed.as_bytes()[1] == b':' {
            return format!("{trimmed}\\");
        }
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    path.to_string()
}

fn normalize_arg(raw: &str, cwd: Option<&Path>) -> String {
    let decoded = gall_core::nav::path::file_uri_or_raw(raw);
    let mut expanded = gall_core::nav::path::expand(&decoded);
    if let Some(cwd) = cwd.filter(|_| Path::new(&expanded).is_relative()) {
        expanded = cwd.join(&expanded).to_string_lossy().into_owned();
    }
    gall_core::nav::path::canonicalize(Path::new(&expanded))
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|_| strip_windows_shell_dot(&expanded))
}

fn is_existing_non_dir(path: &str) -> bool {
    let p = Path::new(path);
    std::fs::symlink_metadata(p).is_ok() && !p.is_dir()
}

fn parent_dir_or_self(path: &str) -> String {
    Path::new(path)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn is_selftest_mode_flag(flag: &str) -> bool {
    flag.starts_with("--e2e-")
        || matches!(
            flag,
            "--updates"
                | "--key-profiles"
                | "--key-profiles-reload"
                | "--key-help"
                | "--demo"
                | "--design"
                | "--audit"
                | "--mapped-drives"
                | "--themed-select"
                | "--git-rebase-submodules"
                | "--git-compare"
                | "--structural-diff"
                | "--git-workspace"
                | "--preview-shortcuts"
                | "--quick-open"
                | "--search-hidden"
                | "--notifications"
                | "--keyboard"
                | "--bug-report"
                | "--bug-report-entry"
                | "--bug-report-text"
                | "--omarchy"
                | "--adaptive-layout"
                | "--workspaces-pairs"
        )
}

/// As `parse`, with relative paths taken from `cwd`: the folder a second launch was
/// started in, which is not the folder of the running instance that reads its arguments.
pub fn parse_in<I: IntoIterator<Item = String>>(args: I, cwd: Option<&Path>) -> CliArgs {
    parse_in_with_env(args, cwd, std::env::var("GALL_SELFTEST").ok())
}

fn parse_in_with_env<I: IntoIterator<Item = String>>(
    args: I,
    cwd: Option<&Path>,
    env_selftest: Option<String>,
) -> CliArgs {
    let args = expand_subcommand(&args.into_iter().collect::<Vec<String>>());
    if args.get(1).is_some_and(|a| a == "compare-files") {
        return match compare_seed(&args[2..]) {
            Ok(seed) => CliArgs {
                compare_picker: true,
                compare_seed: seed.map(|path| normalize_arg(path, cwd)),
                ..CliArgs::default()
            },
            Err(_) => CliArgs::default(),
        };
    }
    let mut out = CliArgs::default();
    let mut raw_path1: Option<String> = None;
    let mut raw_path2: Option<String> = None;
    let mut raw_positional: Vec<String> = Vec::new();
    let mut raw_select: Vec<String> = Vec::new();
    let mut raw_compare: Option<(String, String)> = None;
    let mut raw_merge: Option<Vec<String>> = None;
    let mut raw_git: Option<String> = None;
    let mut cli_selftest = false;
    let mut cli_selftest_mode: Option<String> = None;
    let mut after_double_dash = false;
    let mut pick = PickArgs::default();
    let mut picking = false;

    let mut it = args.into_iter().skip(1);
    while let Some(a) = it.next() {
        if after_double_dash {
            raw_positional.push(a);
            continue;
        }
        match a.as_str() {
            "--" => after_double_dash = true,
            "-p" | "-path" | "--path" | "-o" | "--open" => {
                if let Some(v) = it.next() {
                    raw_path1 = Some(v);
                }
            }
            s if s.starts_with("--path=") => {
                raw_path1 = Some(s["--path=".len()..].to_string());
            }
            s if s.starts_with("--open=") => {
                raw_path1 = Some(s["--open=".len()..].to_string());
            }
            "-p2" | "--path2" => {
                if let Some(v) = it.next() {
                    raw_path2 = Some(v);
                }
            }
            s if s.starts_with("--path2=") => {
                raw_path2 = Some(s["--path2=".len()..].to_string());
            }
            "-newtab" | "--newtab" | "--new-tab" => out.new_tab = true,
            "--new-window" | "-newwindow" | "-w" => out.new_window = true,
            "-nowindow" | "--nowindow" => out.no_window = true,
            "--daemon" => {
                out.daemon = true;
                out.no_window = true;
            }
            "--select" | "-s" => {
                if let Some(v) = it.next() {
                    raw_select.push(v);
                }
            }
            s if s.starts_with("--select=") => {
                raw_select.push(s["--select=".len()..].to_string());
            }
            "--compare" => match take_operands(&mut it, 2) {
                Some(mut ops) => {
                    let b = ops.pop().unwrap_or_default();
                    raw_compare = Some((ops.pop().unwrap_or_default(), b));
                }
                None => log::warn!("--compare requires two paths"),
            },
            "--merge" => match take_operands(&mut it, 4) {
                Some(ops) => raw_merge = Some(ops),
                None => log::warn!("merge requires four paths"),
            },
            "--wait" => out.wait = true,
            s if s.starts_with(HANDOFF_FLAG) => {
                let path = PathBuf::from(&s[HANDOFF_FLAG.len()..]);
                if crate::compare_wait::is_result_path(&path) {
                    out.handoff = Some(path);
                }
            }
            "--git" => {
                if let Some(v) = it.next() {
                    raw_git = Some(v);
                }
            }
            s if s.starts_with("--git=") => {
                raw_git = Some(s["--git=".len()..].to_string());
            }
            "--pick" => {
                if let Some(v) = it.next() {
                    picking = true;
                    pick.mode = match v.as_str() {
                        "save" | "dir" => v,
                        _ => "open".to_string(),
                    };
                }
            }
            "--pick-result" => pick.result = it.next().unwrap_or_default(),
            "--pick-multiple" => pick.multiple = true,
            "--pick-title" => pick.title = it.next(),
            "--pick-name" => pick.name = it.next(),
            "--pick-accept" => pick.accept = it.next(),
            "--pick-filter" => {
                if let Some(v) = it.next() {
                    pick.filters.push(parse_pick_filter(&v));
                }
            }
            "--pick-choices" => {
                if let Some(v) = it.next() {
                    pick.choices = parse_pick_choices(&v);
                }
            }
            "--selftest" => cli_selftest = true,
            s if s.starts_with("--selftest=") => {
                cli_selftest = true;
                cli_selftest_mode = Some(s["--selftest=".len()..].to_string());
            }
            "--selftest-mode" => {
                if let Some(v) = it.next() {
                    cli_selftest = true;
                    cli_selftest_mode = Some(v);
                }
            }
            s if s.starts_with("--selftest-mode=") => {
                cli_selftest = true;
                cli_selftest_mode = Some(s["--selftest-mode=".len()..].to_string());
            }
            s if cli_selftest && cli_selftest_mode.is_none() && is_selftest_mode_flag(s) => {
                cli_selftest_mode = Some(s[2..].to_string());
            }
            "--gall-wait-pid" | "-gall-wait-pid" => {
                let _ = it.next();
            }
            s if s == "--gall-update-probe" || s.starts_with("--gall-wait-pid=") => {}
            s if s.starts_with('-') && s.len() > 1 && Path::new(&s[1..]).is_absolute() => {
                raw_positional.push(s[1..].to_string());
            }
            s if !s.starts_with('-') => raw_positional.push(s.to_string()),
            other => log::warn!("unknown argument {other}"),
        }
    }

    if picking && !pick.result.is_empty() {
        out.new_window = true;
        out.pick = Some(pick);
    }

    let mut apply_target = |raw: &str, set_primary: bool| {
        let norm = normalize_arg(raw, cwd);
        let folder = if is_existing_non_dir(&norm) {
            push_unique(&mut out.select_paths, norm.clone());
            parent_dir_or_self(&norm)
        } else {
            norm
        };
        push_unique(&mut out.paths, folder.clone());
        if set_primary || out.path1.is_none() {
            out.path1 = Some(folder);
        }
    };

    if let Some(p1) = raw_path1 {
        apply_target(&p1, true);
    }
    for pos in raw_positional {
        apply_target(&pos, false);
    }
    if let Some(p2) = raw_path2 {
        out.path2 = Some(normalize_arg(&p2, cwd));
    }
    for sel in raw_select {
        let norm = normalize_arg(&sel, cwd);
        push_unique(&mut out.select_paths, norm.clone());
        if out.path1.is_none() {
            let folder = if Path::new(&norm).is_dir() {
                norm
            } else {
                parent_dir_or_self(&norm)
            };
            push_unique(&mut out.paths, folder.clone());
            out.path1 = Some(folder);
        }
    }
    if let Some((a, b)) = raw_compare {
        out.compare = Some((normalize_arg(&a, cwd), normalize_arg(&b, cwd)));
    }
    if let Some(ops) = raw_merge {
        // Base, local, remote, output; an empty base means that the file has no common ancestor.
        let norm = |raw: &String| normalize_arg(raw, cwd);
        out.merge = Some(MergeArgs {
            base: (!ops[0].is_empty()).then(|| norm(&ops[0])),
            local: norm(&ops[1]),
            remote: norm(&ops[2]),
            output: norm(&ops[3]),
        });
    }
    if let Some(repo) = raw_git {
        let norm = normalize_arg(&repo, cwd);
        out.git_repo = Some(norm.clone());
        if out.path1.is_none() {
            out.path1 = Some(norm.clone());
            push_unique(&mut out.paths, norm);
        }
    }

    let env_selftest = env_selftest
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v != "0");
    out.selftest = cfg!(debug_assertions) && (cli_selftest || env_selftest.is_some());
    out.selftest_mode = if out.selftest {
        cli_selftest_mode.or(env_selftest).unwrap_or_default()
    } else {
        String::new()
    };

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> CliArgs {
        parse(
            std::iter::once("gall")
                .chain(v.iter().copied())
                .map(String::from),
        )
    }

    #[test]
    fn strip_windows_shell_dot_handles_drive_roots_and_folders() {
        assert_eq!(strip_windows_shell_dot(r"C:\."), r"C:\");
        assert_eq!(strip_windows_shell_dot(r"C:\\."), r"C:\");
        assert_eq!(strip_windows_shell_dot(r"D:\Work\Repo\."), r"D:\Work\Repo");
        assert_eq!(
            strip_windows_shell_dot(r"\\server\share\."),
            r"\\server\share"
        );
        assert_eq!(strip_windows_shell_dot("/tmp/folder"), "/tmp/folder");
    }

    #[test]
    fn pick_choices_are_read_from_json_and_cleaned() {
        let a = p(&[
            "--pick",
            "open",
            "--pick-result",
            "/r",
            "--pick-choices",
            r#"[{"id":"enc","label":"Encoding","options":[["u8","UTF-8"],["l1","Latin-1"]],"initial":"u8"},{"id":"ow","label":"Overwrite","options":[],"initial":"false"},{"id":"ow","label":"Dup","options":[],"initial":"true"},{"id":"","label":"No id"},{"nonsense":1}]"#,
        ]);
        let choices = a.pick.expect("pick").choices;
        assert_eq!(
            choices.len(),
            2,
            "a duplicate id and an entry without one are dropped"
        );
        assert_eq!(
            choices[0].options[1],
            ("l1".to_string(), "Latin-1".to_string())
        );
        assert!(choices[1].options.is_empty());
        assert_eq!(choices[1].initial, "false");
        let bad = p(&[
            "--pick",
            "open",
            "--pick-result",
            "/r",
            "--pick-choices",
            "not json",
        ]);
        assert!(bad.pick.expect("pick").choices.is_empty());
    }

    #[test]
    fn pick_arguments_describe_a_picker_window() {
        let a = p(&[
            "--pick",
            "save",
            "--pick-result",
            "/tmp/r.json",
            "--pick-title",
            "Save as",
            "--pick-name",
            "a b.txt",
            "--pick-multiple",
            "--pick-accept",
            "Save",
            "--pick-filter",
            "Images|*.png; *.jpg",
            "--pick-filter",
            "All",
            "/tmp",
        ]);
        let pick = a.pick.expect("pick");
        assert!(a.new_window, "a picker is always its own window");
        assert_eq!(
            (pick.mode.as_str(), pick.result.as_str()),
            ("save", "/tmp/r.json")
        );
        assert_eq!(
            (
                pick.title.as_deref(),
                pick.name.as_deref(),
                pick.accept.as_deref()
            ),
            (Some("Save as"), Some("a b.txt"), Some("Save"))
        );
        assert!(pick.multiple);
        assert_eq!(
            pick.filters[0],
            PickFilter {
                name: "Images".into(),
                patterns: vec!["*.png".into(), "*.jpg".into()]
            }
        );
        assert_eq!(
            pick.filters[1],
            PickFilter {
                name: "All".into(),
                patterns: vec![]
            }
        );
        assert!(a.path1.is_some(), "the folder to start in");
        assert_eq!(
            p(&["--pick", "weird", "--pick-result", "/r"])
                .pick
                .unwrap()
                .mode,
            "open"
        );
        assert!(
            p(&["--pick", "open"]).pick.is_none(),
            "no result file, no picker"
        );
        assert!(p(&["/tmp"]).pick.is_none());
    }

    #[test]
    fn subcommands_are_spellings_of_the_flags() {
        let d = std::env::temp_dir().join(format!("gall-cli-sub-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f1 = d.join("a.txt");
        let f2 = d.join("b.txt");
        std::fs::write(&f1, "x").unwrap();
        std::fs::write(&f2, "x").unwrap();
        let (a, b) = (
            f1.to_string_lossy().into_owned(),
            f2.to_string_lossy().into_owned(),
        );
        assert_eq!(
            p(&["diff", &a, &b]).compare,
            p(&["--compare", &a, &b]).compare
        );
        assert!(p(&["diff", &a, &b]).compare.is_some());
        let dir = d.to_string_lossy().into_owned();
        assert_eq!(p(&["git", &dir]).git_repo, p(&["--git", &dir]).git_repo);
        assert!(p(&["git", &dir]).git_repo.is_some());
        let sel = p(&["select", &a]);
        assert_eq!(sel.select_paths, p(&["--select", &a]).select_paths);
        assert!(!sel.select_paths.is_empty());
        // Only the first argument is a subcommand: a later word is a path.
        assert!(p(&[&dir, "git"]).git_repo.is_none());
        assert!(usage_error(&["gall", "diff", &a]).is_some());
        assert!(usage_error(&["gall", "diff", &a, &b]).is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn relative_paths_follow_the_given_folder() {
        let d = std::env::temp_dir().join(format!("gall-cli-cwd-{}", std::process::id()));
        let other = std::env::temp_dir().join(format!("gall-cli-other-{}", std::process::id()));
        std::fs::create_dir_all(d.join("docs")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let canonical = |p: &std::path::Path| {
            gall_core::nav::path::canonicalize(p)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        let args = |v: &[&str]| {
            parse_in(
                std::iter::once("gall")
                    .chain(v.iter().copied())
                    .map(String::from),
                Some(&d),
            )
        };
        let here = args(&["."]).path1;
        let docs = args(&["-p2", "docs"]).path2;
        // An absolute path is not moved under the folder.
        let absolute = args(&[other.to_str().unwrap()]).path1;
        let expected = (canonical(&d), canonical(&d.join("docs")), canonical(&other));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&other);
        assert_eq!(
            (here, docs, absolute),
            (Some(expected.0), Some(expected.1), Some(expected.2))
        );
    }

    #[cfg(unix)]
    #[test]
    fn variants() {
        assert_eq!(
            p(&["/tmp"]).path1.as_deref(),
            Some(std::fs::canonicalize("/tmp").unwrap().to_str().unwrap())
        );
        assert!(p(&["-o", "/", "-newtab"]).new_tab);
        assert!(p(&["--new-tab"]).new_tab);
        assert!(p(&["--new-window"]).new_window);
        assert!(p(&["-newwindow"]).new_window);
        assert!(p(&["-w"]).new_window);
        assert!(p(&["--nowindow"]).no_window);
        let daemon = p(&["--daemon"]);
        assert!(daemon.daemon);
        assert!(daemon.no_window);
        assert!(!daemon.has_launch_action());
        assert_eq!(p(&["-p2", "/"]).path2.as_deref(), Some("/"));
        assert_eq!(p(&["-/"]).path1.as_deref(), Some("/"));
        let filtered = p(&["--gall-wait-pid", "9876", "/tmp"]);
        assert_eq!(
            filtered.path1.as_deref(),
            Some(std::fs::canonicalize("/tmp").unwrap().to_str().unwrap())
        );
    }

    struct TestDir(std::path::PathBuf);
    impl TestDir {
        fn new(name: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("gall-{name}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn file_uris_and_positional_files_open_parent_and_select() {
        let dir = TestDir::new("cli-uri");
        let dir_canon = gall_core::nav::path::canonicalize(dir.path()).unwrap();
        let file = dir_canon.join("My Črka.txt");
        std::fs::write(&file, "hi").unwrap();
        let uri = gall_core::nav::path::encode_file_uri(&file);

        let a = p(&[&uri]);
        assert_eq!(a.path1.as_deref(), Some(dir_canon.to_str().unwrap()));
        assert_eq!(a.paths, vec![dir_canon.to_string_lossy().into_owned()]);
        assert_eq!(a.select_paths, vec![file.to_string_lossy().into_owned()]);

        // Multiple files in the same folder deduplicate `paths` and preserve both selections.
        let file2 = dir_canon.join("-dash-file.txt");
        std::fs::write(&file2, "yo").unwrap();
        let b = p(&["--", file.to_str().unwrap(), file2.to_str().unwrap()]);
        assert_eq!(b.paths, vec![dir_canon.to_string_lossy().into_owned()]);
        assert_eq!(
            b.select_paths,
            vec![
                file.to_string_lossy().into_owned(),
                file2.to_string_lossy().into_owned()
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn compare_without_two_paths_is_a_usage_error() {
        assert!(usage_error(&["gall", "--compare", "a"]).is_some());
        assert!(usage_error(&["gall", "--compare"]).is_some());
        assert!(usage_error(&["gall", "--compare", "a", "--git"]).is_some());
        assert!(usage_error(&["gall", "--compare", "a", "b"]).is_none());
        assert!(usage_error(&["gall", "/tmp"]).is_none());
    }

    #[test]
    fn select_compare_git_and_selftest_flags() {
        let dir = TestDir::new("cli-flags");
        let dir_canon = gall_core::nav::path::canonicalize(dir.path()).unwrap();
        let f1 = dir_canon.join("a.txt");
        let f2 = dir_canon.join("b.txt");
        std::fs::write(&f1, "1").unwrap();
        std::fs::write(&f2, "2").unwrap();

        let sel = p(&["-s", f1.to_str().unwrap(), "--select", f2.to_str().unwrap()]);
        assert_eq!(sel.path1.as_deref(), Some(dir_canon.to_str().unwrap()));
        assert_eq!(
            sel.select_paths,
            vec![
                f1.to_string_lossy().into_owned(),
                f2.to_string_lossy().into_owned()
            ]
        );

        let cmp = p(&["--compare", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(
            cmp.compare,
            Some((
                f1.to_string_lossy().into_owned(),
                f2.to_string_lossy().into_owned()
            ))
        );

        let git = p(&["--git", dir_canon.to_str().unwrap()]);
        assert_eq!(git.git_repo.as_deref(), Some(dir_canon.to_str().unwrap()));
        assert_eq!(git.path1.as_deref(), Some(dir_canon.to_str().unwrap()));

        let st = p(&["--selftest-mode", "design"]);
        if cfg!(debug_assertions) {
            assert!(st.selftest);
            assert_eq!(st.selftest_mode, "design");
        }
        let e2e = p(&["--selftest", "--e2e-cli-state"]);
        let e2e_eq = p(&["--selftest-mode=e2e-omarchy-dark"]);
        let updates = p(&["--selftest", "--updates"]);
        let key_profiles = p(&["--selftest", "--key-profiles"]);
        let key_help = p(&["--selftest", "--key-help"]);
        let design = p(&["--selftest", "--design"]);
        let audit = p(&["--selftest", "--audit"]);
        let omarchy = p(&["--selftest", "--omarchy"]);
        let adaptive = p(&["--selftest", "--adaptive-layout"]);
        if cfg!(debug_assertions) {
            assert_eq!(omarchy.selftest_mode, "omarchy");
            assert_eq!(adaptive.selftest_mode, "adaptive-layout");
            assert!(e2e.selftest);
            assert_eq!(e2e.selftest_mode, "e2e-cli-state");
            assert!(e2e_eq.selftest);
            assert_eq!(e2e_eq.selftest_mode, "e2e-omarchy-dark");
            assert!(updates.selftest);
            assert_eq!(updates.selftest_mode, "updates");
            assert!(key_profiles.selftest);
            assert_eq!(key_profiles.selftest_mode, "key-profiles");
            assert!(key_help.selftest);
            assert_eq!(key_help.selftest_mode, "key-help");
            assert!(design.selftest);
            assert_eq!(design.selftest_mode, "design");
            assert!(audit.selftest);
            assert_eq!(audit.selftest_mode, "audit");
        }
    }

    #[test]
    fn gall_selftest_env_empty_or_zero_disables_while_nonempty_enables() {
        let parse_with = |args: &[&str], env: Option<&str>| {
            parse_in_with_env(
                std::iter::once("gall")
                    .chain(args.iter().copied())
                    .map(String::from),
                None,
                env.map(str::to_string),
            )
        };

        let empty = parse_with(&[], Some(""));
        assert!(!empty.selftest);
        assert_eq!(empty.selftest_mode, "");

        let spaces = parse_with(&[], Some("   "));
        assert!(!spaces.selftest);

        let zero = parse_with(&[], Some("0"));
        assert!(!zero.selftest);
        assert_eq!(zero.selftest_mode, "");

        let one = parse_with(&[], Some("1"));
        let e2e = parse_with(&[], Some("e2e-omarchy-dark"));
        let overridden = parse_with(&["--selftest", "--updates"], Some("1"));
        if cfg!(debug_assertions) {
            assert!(one.selftest);
            assert_eq!(one.selftest_mode, "1");
            assert!(e2e.selftest);
            assert_eq!(e2e.selftest_mode, "e2e-omarchy-dark");
            assert!(overridden.selftest);
            assert_eq!(overridden.selftest_mode, "updates");
        }
    }

    #[cfg(windows)]
    #[test]
    fn variants_windows() {
        let tmp = std::env::temp_dir();
        let got = p(&[tmp.to_str().unwrap()]).path1.unwrap();
        assert!(!got.starts_with(r"\\?\"), "verbatim prefix leaked: {got}");
        assert!(std::path::Path::new(&got).is_dir());
        assert!(p(&["-o", r"C:\", "-newtab"]).new_tab);
        assert_eq!(p(&["-p2", r"C:\"]).path2.as_deref(), Some(r"C:\"));
    }

    #[test]
    fn merge_takes_four_operands_in_the_order_base_local_remote_output() {
        let a = p(&["merge", "/t/base", "/t/local", "/t/remote", "/t/out"]);
        assert_eq!(
            a.merge,
            Some(MergeArgs {
                base: Some("/t/base".into()),
                local: "/t/local".into(),
                remote: "/t/remote".into(),
                output: "/t/out".into(),
            })
        );
        assert!(a.has_launch_action());
        // The flag spelling is the same, and an empty base means no common ancestor.
        let b = p(&["--merge", "", "/t/l", "/t/r", "/t/o"]);
        assert_eq!(b.merge.as_ref().map(|m| m.base.clone()), Some(None));
        assert_eq!(b.merge.as_ref().map(|m| m.output.as_str()), Some("/t/o"));
        // Not the folders of a plain launch, and no wait unless asked for.
        assert!(a.paths.is_empty() && !a.wait);
        assert!(p(&["merge", "/a", "/b"]).merge.is_none());
    }

    #[test]
    fn merge_and_compare_take_names_that_start_with_dashes_after_a_double_dash() {
        let m = p(&["merge", "--", "-base", "--local", "-x/remote", "--out"]);
        let m = m.merge.unwrap();
        assert!(m.base.unwrap().ends_with("-base"));
        assert!(
            m.local.ends_with("--local")
                && m.remote.ends_with("-x/remote")
                && m.output.ends_with("--out")
        );
        let c = p(&["diff", "--", "--left", "-right"]).compare.unwrap();
        assert!(c.0.ends_with("--left") && c.1.ends_with("-right"));
        // Without the separator a name that starts with two dashes is a flag, and that is a usage error.
        assert!(usage_error(&["gall", "merge", "--base", "l", "r", "o"]).is_some());
        assert!(usage_error(&["gall", "diff", "--left", "right"]).is_some());
        assert!(usage_error(&["gall", "merge", "--", "--base", "l", "r", "o"]).is_none());
        assert!(usage_error(&["gall", "diff", "--", "--left", "right"]).is_none());
        // One dash is a path, as before.
        assert!(usage_error(&["gall", "diff", "-left", "-right"]).is_none());
    }

    #[test]
    fn merge_with_too_few_operands_is_a_usage_error_and_nothing_is_launched() {
        for args in [
            &["gall", "merge"][..],
            &["gall", "merge", "a", "b", "c"],
            &["gall", "--merge", "a", "b"],
            &["gall", "merge", "--", "a", "b", "c"],
        ] {
            let message = usage_error(args).unwrap_or_else(|| panic!("{args:?}"));
            assert!(message.contains("four paths"), "{message}");
        }
        assert!(usage_error(&["gall", "merge", "b", "l", "r", "o"]).is_none());
        assert!(usage_error(&["gall", "merge", "", "l", "r", "o"]).is_none());
        let none = p(&["merge", "a", "b", "c"]);
        assert!(none.merge.is_none() && !none.has_launch_action());
    }

    #[test]
    fn wait_belongs_to_diff_and_merge_only() {
        assert!(p(&["diff", "--wait", "/a", "/b"]).wait);
        assert!(p(&["--wait", "--compare", "/a", "/b"]).wait);
        assert!(p(&["merge", "--wait", "/b", "/l", "/r", "/o"]).wait);
        assert!(usage_error(&["gall", "diff", "--wait", "a", "b"]).is_none());
        let message = usage_error(&["gall", "--wait", "/tmp"]).unwrap();
        assert!(message.contains("--wait applies to diff and merge"));
        assert!(usage_error(&["gall", "--wait"]).is_some());
        assert!(!p(&["diff", "/a", "/b"]).wait);
    }

    #[test]
    fn relative_merge_operands_belong_to_the_folder_of_the_caller() {
        let dir = tempfile::tempdir().unwrap();
        let args = parse_in(
            [
                "gall",
                "merge",
                "base.txt",
                "local.txt",
                "remote.txt",
                "out.txt",
            ]
            .map(String::from),
            Some(dir.path()),
        );
        let m = args.merge.unwrap();
        let under = |name: &str| {
            std::path::Path::new(&m.output)
                .parent()
                .map(|p| p.join(name).to_string_lossy().into_owned())
        };
        assert!(std::path::Path::new(&m.local)
            .starts_with(std::path::Path::new(&m.output).parent().unwrap()));
        assert_eq!(under("out.txt").as_deref(), Some(m.output.as_str()));
        assert!(m.output.ends_with("out.txt") && m.base.unwrap().ends_with("base.txt"));
    }

    #[test]
    fn compare_picker_takes_one_literal_seed_or_no_paths() {
        assert!(p(&["compare-files"]).compare_picker);
        assert_eq!(p(&["compare-files"]).compare_seed, None);
        let a = p(&["compare-files", "--", "--a b.txt"]);
        assert!(a.compare_picker && a.has_launch_action());
        assert!(a.compare_seed.as_deref().unwrap().ends_with("--a b.txt"));
        assert!(usage_error(&["gall", "compare-files", "a", "b"]).is_some());
        let forwarded = p(&[
            "--gall-wait-pid",
            "1234",
            "compare-files",
            "--",
            "--a b.txt",
        ]);
        assert_eq!(forwarded, a);
    }

    #[test]
    fn wait_result_path_is_kept_on_the_native_side() {
        let dir = crate::compare_wait::result_dir().unwrap();
        let path = gall_core::handoff::new_result_path(dir.path());
        let arg = format!("{HANDOFF_FLAG}{}", path.display());
        let args = p(&[&arg, "--wait", "--compare", "a", "b"]);
        assert_eq!(args.handoff.as_deref(), Some(path.as_path()));
        assert!(args.wait && args.compare.is_some());
        assert!(!serde_json::to_string(&args).unwrap().contains("gall-wait-"));
        let bad = format!(
            "{HANDOFF_FLAG}{}",
            dir.path().join("unrelated.txt").display()
        );
        assert!(usage_error(&["gall", &bad, "--wait", "--compare", "a", "b"]).is_some());
        assert!(!p(&["diff", "--", "--wait", "merge"]).wait);
    }
}
