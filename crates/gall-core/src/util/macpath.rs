//! macOS: a GUI app started from Finder, the Dock or Spotlight gets the system's minimal
//! `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`), not the one of the user's shell. Homebrew,
//! `/usr/local/bin` and the `code` shim are then invisible, so terminals and editors are
//! not offered and Git comes from the Xcode shim. This builds the PATH the user would
//! see in a terminal.

/// Directories that tools are commonly installed to on a Mac.
#[cfg(target_os = "macos")]
const COMMON: [&str; 3] = ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"];

/// `current` followed by the `extra` lists, without duplicates and keeping the first
/// occurrence, so what the process already had keeps its priority.
pub fn merge(current: &str, extra: &[&str]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for dir in std::iter::once(current)
        .chain(extra.iter().copied())
        .flat_map(|list| list.split(':'))
    {
        if !dir.is_empty() && !out.contains(&dir) {
            out.push(dir);
        }
    }
    out.join(":")
}

/// The value printed by `path_helper -s` (`PATH="…"; export PATH;`).
pub fn parse_path_helper(output: &str) -> Option<String> {
    let rest = output.split("PATH=\"").nth(1)?;
    Some(rest.split('"').next()?.to_string())
}

/// Adds the directories a terminal would have (`/etc/paths`, `/etc/paths.d`, Homebrew) to
/// this process's `PATH`. Nothing the process had is removed or reordered.
#[cfg(target_os = "macos")]
pub fn fix_path() {
    let current = std::env::var("PATH").unwrap_or_default();
    let helper = std::process::Command::new("/usr/libexec/path_helper")
        .arg("-s")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| parse_path_helper(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    let existing: Vec<&str> = COMMON
        .iter()
        .copied()
        .filter(|d| std::path::Path::new(d).is_dir())
        .collect();
    let merged = merge(&current, &[&helper, &existing.join(":")]);
    if merged != current {
        std::env::set_var("PATH", merged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_order_and_drops_duplicates() {
        assert_eq!(
            merge(
                "/usr/bin:/bin",
                &["/opt/homebrew/bin:/usr/bin", "", "/bin:/x"]
            ),
            "/usr/bin:/bin:/opt/homebrew/bin:/x"
        );
        assert_eq!(merge("", &["/a::/b"]), "/a:/b");
    }

    #[test]
    fn path_helper_output_is_parsed() {
        let out = "PATH=\"/usr/local/bin:/usr/bin:/bin\"; export PATH;\n";
        assert_eq!(
            parse_path_helper(out).as_deref(),
            Some("/usr/local/bin:/usr/bin:/bin")
        );
        assert_eq!(parse_path_helper("nothing"), None);
    }
}
