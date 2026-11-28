//! Whole-system name lookup through the operating system's own indexer, used by Quick Open's
//! "everywhere" search when one is installed: `plocate`/`locate` on Linux, Spotlight (`mdfind`) on
//! macOS and Everything's `es` command line on Windows. Without one (or on any failure) callers
//! fall back to the polite home-folder index.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn on_path(tool: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|d| {
        let f = d.join(tool);
        f.is_file() || (cfg!(windows) && d.join(format!("{tool}.exe")).is_file())
    })
}

/// The command (program and arguments) that lists up to `limit` paths whose name matches `query`.
fn command_for(
    os: &str,
    query: &str,
    limit: usize,
    has: impl Fn(&str) -> bool,
) -> Option<Vec<String>> {
    let q = query.trim();
    if q.is_empty() || q.starts_with('-') {
        return None;
    }
    let n = limit.to_string();
    match os {
        "linux" => ["plocate", "locate"].into_iter().find(|t| has(t)).map(|t| {
            let mut c = vec![t.to_string(), "-i".into(), "-b".into(), "-l".into(), n];
            c.extend(["--".into(), q.to_string()]);
            c
        }),
        "macos" => has("mdfind").then(|| vec!["mdfind".into(), "-name".into(), q.to_string()]),
        "windows" => has("es").then(|| vec!["es".into(), "-n".into(), n, q.to_string()]),
        _ => None,
    }
}

/// Existing absolute paths from the tool's output, at most `limit`, other folders' hidden or
/// system content left out.
fn parse_output(out: &str, limit: usize) -> Vec<PathBuf> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.exists())
        .take(limit)
        .collect()
}

/// Names matching `query` anywhere on the system, or `None` when no system indexer is available
/// or it failed. Hidden folders (any component starting with a dot) are dropped, like the
/// home-folder index does.
pub fn search(query: &str, limit: usize) -> Option<Vec<PathBuf>> {
    let cmd = command_for(
        std::env::consts::OS,
        query,
        limit.saturating_mul(4).max(1),
        on_path,
    )?;
    let out = Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let found = parse_output(&String::from_utf8_lossy(&out.stdout), usize::MAX);
    Some(
        found
            .into_iter()
            .filter(|p| !is_hidden_path(p))
            .take(limit)
            .collect(),
    )
}

fn is_hidden_path(p: &Path) -> bool {
    p.components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_first_installed_tool_per_platform() {
        let all = |_: &str| true;
        let none = |_: &str| false;
        let locate_only = |t: &str| t == "locate";
        assert_eq!(command_for("linux", "a.rs", 5, all).unwrap()[0], "plocate");
        assert_eq!(
            command_for("linux", "a.rs", 5, locate_only).unwrap()[0],
            "locate"
        );
        assert_eq!(
            command_for("macos", "a", 5, all).unwrap()[..2],
            ["mdfind", "-name"]
        );
        assert_eq!(command_for("windows", "a", 5, all).unwrap()[0], "es");
        assert!(command_for("linux", "a", 5, none).is_none());
        assert!(command_for("freebsd", "a", 5, all).is_none());
    }

    #[test]
    fn refuses_empty_queries_and_option_lookalikes() {
        assert!(command_for("linux", "  ", 5, |_| true).is_none());
        assert!(command_for("linux", "--help", 5, |_| true).is_none());
    }

    #[test]
    fn keeps_only_existing_absolute_paths_up_to_the_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        let out = format!(
            "{}\nrelative.txt\n{}\n\n{}\n",
            a.display(),
            tmp.path().join("gone").display(),
            b.display()
        );
        assert_eq!(parse_output(&out, 10), vec![a.clone(), b]);
        assert_eq!(parse_output(&out, 1), vec![a]);
    }

    #[test]
    fn hidden_components_are_detected() {
        assert!(is_hidden_path(Path::new("/home/u/.cache/x")));
        assert!(!is_hidden_path(Path::new("/home/u/docs/x")));
    }
}
