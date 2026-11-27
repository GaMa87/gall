//! Commands that run without a window: `gall search|grep|selection|skill`. They run in `pre_run`,
//! before Tauri starts, so they are quick and work over SSH or from scripts and agents.
//! Errors go to stderr and the exit code is 1; `--json` gives machine-readable output.

use gall_core::fs::grep::{self, CaseMode, FileMatches, GrepOptions};
use gall_core::fs::index::{FileIndex, IndexOptions};
use gall_core::util::appdirs::AppDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// The agent skill bundled in the binary.
pub const SKILL_MD: &str = include_str!("../../crates/gall-core/src/cli_skill.md");

/// What `gall selection` reports: written by the running app, read by the command.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SelectionSnapshot {
    pub pid: u32,
    pub window: String,
    pub folder: String,
    pub selection: Vec<String>,
    pub focused: Option<String>,
    /// Unix epoch milliseconds.
    pub updated_at: u64,
}

pub fn selection_file(dirs: &AppDirs) -> PathBuf {
    dirs.local_data.join("gall").join("selection.json")
}

/// Writes the snapshot atomically (temp file + rename).
pub fn write_selection(dirs: &AppDirs, snap: &SelectionSnapshot) -> std::io::Result<()> {
    let path = selection_file(dirs);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::pick::write_private(&path, &serde_json::to_vec(snap).unwrap_or_default())
}

pub fn remove_selection(dirs: &AppDirs) {
    let _ = std::fs::remove_file(selection_file(dirs));
}

fn process_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

/// The last published selection, if the app that wrote it is still running.
pub fn read_selection(dirs: &AppDirs) -> Result<SelectionSnapshot, String> {
    let path = selection_file(dirs);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| "Gall is not running (no selection published)".to_string())?;
    let snap: SelectionSnapshot =
        serde_json::from_str(&text).map_err(|e| format!("unreadable selection file: {e}"))?;
    if !process_alive(snap.pid) {
        return Err("Gall is not running (the last window closed)".into());
    }
    Ok(snap)
}

/// Options shared by `search` and `grep`.
#[derive(Debug, Default, PartialEq, Eq)]
struct Flags {
    json: bool,
    dir: Option<String>,
    limit: Option<usize>,
    regex: bool,
    word: bool,
    case: Option<CaseMode>,
    globs: Vec<String>,
    all: bool,
    positional: Vec<String>,
}

fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut it = args.iter();
    let value = |it: &mut std::slice::Iter<'_, String>, name: &str| {
        it.next()
            .cloned()
            .ok_or_else(|| format!("{name} needs a value"))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--" => {
                // Everything after `--` is the query, even when it starts with dashes.
                f.positional.extend(it.by_ref().cloned());
                break;
            }
            "--json" => f.json = true,
            "--regex" => f.regex = true,
            "--word" => f.word = true,
            "--all" => f.all = true,
            "--case" => f.case = Some(CaseMode::Sensitive),
            "--ignore-case" => f.case = Some(CaseMode::Insensitive),
            "--dir" => f.dir = Some(value(&mut it, "--dir")?),
            "--glob" => f.globs.push(value(&mut it, "--glob")?),
            "--limit" => {
                let v = value(&mut it, "--limit")?;
                f.limit = Some(
                    v.parse()
                        .map_err(|_| format!("--limit: not a number: {v}"))?,
                );
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => f.positional.push(a.clone()),
        }
    }
    Ok(f)
}

fn resolve_dir(dir: &Option<String>) -> Result<PathBuf, String> {
    let raw = match dir {
        Some(d) => gall_core::nav::path::expand(&gall_core::nav::path::file_uri_or_raw(d)),
        None => std::env::current_dir()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned(),
    };
    let p = PathBuf::from(raw);
    if !p.is_dir() {
        return Err(format!("not a folder: {}", p.display()));
    }
    Ok(gall_core::nav::path::canonicalize(&p).unwrap_or(p))
}

fn search(args: &[String], out: &mut dyn FnMut(String)) -> Result<(), String> {
    let f = parse_flags(args)?;
    let query = f.positional.join(" ");
    if query.trim().is_empty() {
        return Err("usage: gall search <query> [--dir <folder>] [--limit N] [--json]".into());
    }
    let dir = resolve_dir(&f.dir)?;
    let index = FileIndex::new();
    let opts = IndexOptions {
        everything: f.all,
        ..IndexOptions::default()
    };
    index.rebuild(std::slice::from_ref(&dir), &opts, &AtomicBool::new(false));
    let res = index.query(&query, Some(&dir), f.limit.unwrap_or(50));
    if f.json {
        let hits: Vec<_> = res
            .hits
            .iter()
            .map(|h| serde_json::json!({"path": h.path, "name": h.name, "isDir": h.is_dir, "score": h.score}))
            .collect();
        out(serde_json::to_string(&hits).unwrap_or_default());
    } else {
        for h in &res.hits {
            out(h.path.clone());
        }
    }
    Ok(())
}

fn grep_cmd(args: &[String], out: &mut dyn FnMut(String)) -> Result<(), String> {
    let f = parse_flags(args)?;
    let pattern = f.positional.join(" ");
    if pattern.is_empty() {
        return Err("usage: gall grep <pattern> [--dir <folder>] [--regex] [--json]".into());
    }
    let dir = resolve_dir(&f.dir)?;
    let opts = GrepOptions {
        case: f.case.unwrap_or_default(),
        whole_word: f.word,
        regex: f.regex,
        include: f.globs.clone(),
        everything: f.all,
        max_matches: f.limit.unwrap_or(2000),
        ..GrepOptions::default()
    };
    let files: Mutex<Vec<FileMatches>> = Mutex::new(Vec::new());
    let stats = grep::grep(&dir, &pattern, &opts, &AtomicBool::new(false), &|fm| {
        files.lock().unwrap().push(fm);
    })?;
    let mut files = files.into_inner().unwrap();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if f.json {
        let value = serde_json::json!({
            "files": files.iter().map(|fm| serde_json::json!({
                "path": fm.path,
                "matches": fm.matches.iter().map(|m| serde_json::json!({"line": m.line, "text": m.text})).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "stats": stats,
        });
        out(serde_json::to_string(&value).unwrap_or_default());
    } else {
        for fm in &files {
            for m in &fm.matches {
                out(format!("{}:{}:{}", fm.path, m.line, m.text));
            }
        }
    }
    Ok(())
}

fn selection_cmd(
    args: &[String],
    dirs: &AppDirs,
    out: &mut dyn FnMut(String),
) -> Result<(), String> {
    let f = parse_flags(args)?;
    let snap = read_selection(dirs)?;
    if f.json {
        out(serde_json::to_string(&snap).unwrap_or_default());
    } else if snap.selection.is_empty() {
        out(snap.folder);
    } else {
        for p in &snap.selection {
            out(p.clone());
        }
    }
    Ok(())
}

/// Where the skill is linked for Claude Code and other agents.
fn skill_links(home: &Path) -> [PathBuf; 2] {
    [
        home.join(".claude/skills/gall"),
        home.join(".agents/skills/gall"),
    ]
}

/// Writes the skill to the data folder and links it into the agent skill folders.
/// Returns the paths it created.
pub fn install_skill(dirs: &AppDirs, home: &Path, force: bool) -> Result<Vec<PathBuf>, String> {
    let dir = dirs.data.join("agent-skill").join("gall");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(dir.join("SKILL.md"), SKILL_MD).map_err(|e| e.to_string())?;
    let mut made = vec![dir.join("SKILL.md")];
    for link in skill_links(home) {
        let Some(parent) = link.parent() else {
            continue;
        };
        // Only link into agent folders that exist or can be created under the home folder.
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        match std::fs::symlink_metadata(&link) {
            Ok(meta) if meta.file_type().is_symlink() => {
                std::fs::remove_file(&link).map_err(|e| e.to_string())?;
            }
            Ok(_) if !force => {
                return Err(format!(
                    "{} exists and is not a link; use --force to replace it",
                    link.display()
                ));
            }
            Ok(meta) => {
                if meta.is_dir() {
                    std::fs::remove_dir_all(&link)
                } else {
                    std::fs::remove_file(&link)
                }
                .map_err(|e| e.to_string())?;
            }
            Err(_) => {}
        }
        link_dir(&dir, &link).map_err(|e| format!("{}: {e}", link.display()))?;
        made.push(link);
    }
    Ok(made)
}

#[cfg(unix)]
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    // Windows symlinks need privileges: copy the skill instead.
    std::fs::create_dir_all(link)?;
    std::fs::copy(target.join("SKILL.md"), link.join("SKILL.md")).map(|_| ())
}

fn skill_cmd(args: &[String], dirs: &AppDirs, out: &mut dyn FnMut(String)) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("install") => {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .ok_or("cannot find the home folder")?;
            let made = install_skill(dirs, &home, args.iter().any(|a| a == "--force"))?;
            for p in made {
                out(format!("installed {}", p.display()));
            }
            Ok(())
        }
        Some("print") => {
            out(SKILL_MD.trim_end().to_string());
            Ok(())
        }
        _ => Err("usage: gall skill install [--force] | gall skill print".into()),
    }
}

/// True when `args[1]` is one of the window-less subcommands (and not a path like `./search`).
pub fn is_headless(args: &[String]) -> bool {
    matches!(
        args.get(1).map(String::as_str),
        Some("search" | "grep" | "selection" | "skill")
    )
}

/// Runs a window-less subcommand, returning its exit code, or `None` for a normal launch.
pub fn run(args: &[String], dirs: &AppDirs) -> Option<i32> {
    if !is_headless(args) {
        return None;
    }
    let rest = &args[2..];
    // A closed pipe (`gall search x | head`) ends the command quietly instead of panicking in println!.
    let mut out = |line: String| {
        use std::io::Write;
        if writeln!(std::io::stdout().lock(), "{line}").is_err() {
            std::process::exit(0);
        }
    };
    let result = match args[1].as_str() {
        "search" => search(rest, &mut out),
        "grep" => grep_cmd(rest, &mut out),
        "selection" => selection_cmd(rest, dirs, &mut out),
        _ => skill_cmd(rest, dirs, &mut out),
    };
    match result {
        Ok(()) => Some(0),
        Err(e) => {
            eprintln!("gall: {e}");
            Some(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn dirs(root: &Path) -> AppDirs {
        AppDirs {
            config: root.join("config"),
            data: root.join("data"),
            local_data: root.join("local"),
            portable: false,
        }
    }

    fn collect(
        f: impl FnOnce(&mut dyn FnMut(String)) -> Result<(), String>,
    ) -> Result<Vec<String>, String> {
        let mut lines = Vec::new();
        f(&mut |l| lines.push(l))?;
        Ok(lines)
    }

    #[test]
    fn recognises_only_the_windowless_subcommands() {
        assert!(is_headless(&s(&["gall", "search", "x"])));
        assert!(is_headless(&s(&["gall", "selection"])));
        assert!(!is_headless(&s(&["gall", "./search"])));
        assert!(!is_headless(&s(&["gall", "--new-window"])));
        assert!(
            !is_headless(&s(&["gall", "diff", "a", "b"])),
            "diff opens a window"
        );
        assert!(!is_headless(&s(&["gall"])));
    }

    #[test]
    fn parses_flags_and_rejects_unknown_ones() {
        let f = parse_flags(&s(&[
            "needle", "--dir", "/x", "--limit", "5", "--json", "--regex", "--glob", "*.rs",
        ]))
        .unwrap();
        assert_eq!(f.positional, ["needle"]);
        assert_eq!(
            (f.dir.as_deref(), f.limit, f.json, f.regex),
            (Some("/x"), Some(5), true, true)
        );
        assert_eq!(f.globs, ["*.rs"]);
        let dashed = parse_flags(&s(&["--json", "--", "--foo", "-x"])).unwrap();
        assert!(dashed.json);
        assert_eq!(dashed.positional, ["--foo", "-x"]);
        assert!(parse_flags(&s(&["--bogus"])).is_err());
        assert!(parse_flags(&s(&["--limit", "many"])).is_err());
        assert!(parse_flags(&s(&["--dir"])).is_err());
    }

    #[test]
    fn grep_finds_lines_as_text_and_json() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a.txt"), "one\nneedle here\nthree\n").unwrap();
        std::fs::write(t.path().join("b.txt"), "nothing\n").unwrap();
        let dir = t.path().to_string_lossy().into_owned();
        let lines = collect(|o| grep_cmd(&s(&["needle", "--dir", &dir]), o)).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with("a.txt:2:needle here"), "{lines:?}");
        let json = collect(|o| grep_cmd(&s(&["needle", "--dir", &dir, "--json"]), o)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json[0]).unwrap();
        assert_eq!(v["files"][0]["matches"][0]["line"], 2);
        assert_eq!(v["stats"]["matches"], 1);
        assert!(
            collect(|o| grep_cmd(&s(&["--dir", &dir]), o)).is_err(),
            "pattern is required"
        );
        assert!(collect(|o| grep_cmd(&s(&["x", "--dir", "/no/such/folder"]), o)).is_err());
    }

    #[test]
    fn search_lists_matching_names() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("sub")).unwrap();
        std::fs::write(t.path().join("sub/report-final.txt"), "x").unwrap();
        std::fs::write(t.path().join("other.txt"), "x").unwrap();
        let dir = t.path().to_string_lossy().into_owned();
        let lines = collect(|o| search(&s(&["report", "--dir", &dir]), o)).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with("report-final.txt"), "{lines:?}");
        let json = collect(|o| search(&s(&["report", "--dir", &dir, "--json", "--limit", "1"]), o))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json[0]).unwrap();
        assert_eq!(v[0]["name"], "report-final.txt");
        assert_eq!(v[0]["isDir"], false);
    }

    #[test]
    fn selection_round_trips_and_needs_a_live_app() {
        let t = tempfile::tempdir().unwrap();
        let d = dirs(t.path());
        assert!(read_selection(&d).is_err(), "nothing published yet");
        let snap = SelectionSnapshot {
            pid: std::process::id(),
            window: "main".into(),
            folder: "/home/u".into(),
            selection: vec!["/home/u/a b.txt".into(), "/home/u/\u{010d}.md".into()],
            focused: Some("/home/u/a b.txt".into()),
            updated_at: 1,
        };
        write_selection(&d, &snap).unwrap();
        assert_eq!(read_selection(&d).unwrap(), snap);
        let lines = collect(|o| selection_cmd(&s(&["--json"]), &d, o)).unwrap();
        assert_eq!(
            serde_json::from_str::<SelectionSnapshot>(&lines[0]).unwrap(),
            snap
        );
        let plain = collect(|o| selection_cmd(&[], &d, o)).unwrap();
        assert_eq!(plain, snap.selection);
        #[cfg(target_os = "linux")]
        {
            write_selection(
                &d,
                &SelectionSnapshot {
                    pid: u32::MAX - 1,
                    ..snap
                },
            )
            .unwrap();
            assert!(
                read_selection(&d).is_err(),
                "a dead pid means Gall is not running"
            );
        }
        remove_selection(&d);
        assert!(read_selection(&d).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn skill_install_links_both_agent_folders_and_protects_real_folders() {
        let t = tempfile::tempdir().unwrap();
        let d = dirs(t.path());
        let home = t.path().join("home");
        let made = install_skill(&d, &home, false).unwrap();
        assert_eq!(made.len(), 3);
        for link in skill_links(&home) {
            let text = std::fs::read_to_string(link.join("SKILL.md")).unwrap();
            assert!(text.starts_with("---\nname: gall"));
        }
        // Reinstalling replaces the links.
        install_skill(&d, &home, false).unwrap();
        // A real folder of the user is kept unless forced.
        let real = home.join(".claude/skills/gall");
        std::fs::remove_file(&real).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("mine.txt"), "x").unwrap();
        assert!(install_skill(&d, &home, false).is_err());
        assert!(real.join("mine.txt").exists());
        install_skill(&d, &home, true).unwrap();
        assert!(real.join("SKILL.md").exists());
    }

    #[test]
    fn the_bundled_skill_names_every_command() {
        for word in [
            "gall search",
            "gall grep",
            "gall selection",
            "gall select",
            "gall diff",
            "gall git",
            "skill install",
        ] {
            assert!(SKILL_MD.contains(word), "{word}");
        }
    }
}
