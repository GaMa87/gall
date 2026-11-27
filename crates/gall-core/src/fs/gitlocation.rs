//! Where a new worktree goes: the location template, the folder name of a branch, what is
//! refused, and the local exclude rule that keeps a nested worktree out of the status.
//! Everything here reads files only and starts no Git process.
use crate::{CoreError, Result};
use std::path::{Component, Path, PathBuf};

/// Inside the main worktree, in a folder that the repository's local excludes hide.
pub const DEFAULT_TEMPLATE: &str = "{repo}/.worktrees/{branch}";

const MAX_NAME: usize = 80;

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// The folder name for a branch or commit: one path component that every platform accepts.
pub fn slug(name: &str) -> String {
    let mut result = String::new();
    for c in name.chars() {
        let c = if c.is_control()
            || c.is_whitespace()
            || matches!(
                c,
                '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*' | '%' | '~'
            ) {
            '-'
        } else {
            c
        };
        // `feature//x` and `a: b` need only one dash.
        if c != '-' || !result.ends_with('-') {
            result.push(c);
        }
    }
    let mut result = result
        .trim_start_matches(['-', '.'])
        .trim_end_matches(['-', '.'])
        .to_string();
    if result.len() > MAX_NAME {
        let mut end = MAX_NAME;
        while !result.is_char_boundary(end) {
            end -= 1;
        }
        result.truncate(end);
        result.truncate(result.trim_end_matches(['-', '.']).len());
    }
    // Windows reserves these names, also with an extension.
    let stem = result
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let reserved = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    if result.is_empty() {
        "worktree".into()
    } else if reserved {
        format!("_{result}")
    } else {
        result
    }
}

/// Removes `.` and resolves `..` without looking at the disk.
fn tidy(path: &Path) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(result.components().next_back(), Some(Component::Normal(_))) {
                    return None;
                }
                result.pop();
            }
            other => result.push(other),
        }
    }
    Some(result)
}

/// The folder `template` names for `branch`. `{repo}` at the start is the folder of the main
/// worktree, elsewhere its name; a relative template starts at that folder, `~/` at home.
pub fn expand(template: &str, main: &Path, branch: &str) -> Result<PathBuf> {
    let template = match template.trim() {
        "" => DEFAULT_TEMPLATE,
        value => value,
    };
    if template.chars().any(char::is_control) {
        return Err(fail(
            "The worktree location must not contain control characters",
        ));
    }
    if !template.contains("{branch}") {
        return Err(fail("The worktree location needs {branch}"));
    }
    let rest = template.replace("{branch}", "").replace("{repo}", "");
    if rest.contains(['{', '}']) {
        return Err(fail("The worktree location knows only {repo} and {branch}"));
    }
    let name = main
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repository".into());
    let fill = |text: &str| {
        text.replace("{repo}", &name)
            .replace("{branch}", &slug(branch))
    };
    let separator = |c: char| c == '/' || (cfg!(windows) && c == '\\');
    let path = if let Some(tail) = template
        .strip_prefix("{repo}")
        .filter(|tail| tail.starts_with(separator))
    {
        main.join(fill(tail.trim_start_matches(separator)))
    } else if let Some(tail) = template
        .strip_prefix('~')
        .filter(|tail| tail.starts_with(separator))
    {
        dirs::home_dir()
            .ok_or_else(|| fail("The home folder is unknown"))?
            .join(fill(tail.trim_start_matches(separator)))
    } else {
        main.join(fill(template))
    };
    // `join` keeps the `/` of the template, which Windows paths elsewhere do not have.
    let path: PathBuf = path.components().collect();
    tidy(&path)
        .filter(|path| path.is_absolute() && path.file_name().is_some())
        .ok_or_else(|| fail("The worktree location is not a valid folder"))
}

/// `path` with the part that exists resolved (links, 8.3 names, letter case).
fn resolved(path: &Path) -> PathBuf {
    for existing in path.ancestors() {
        if let Ok(real) = crate::nav::path::canonicalize(existing) {
            return match path.strip_prefix(existing) {
                Ok(rest) if !rest.as_os_str().is_empty() => real.join(rest),
                _ => real,
            };
        }
    }
    path.to_path_buf()
}

/// The nearest folder of `path` that exists.
fn existing(path: &Path) -> &Path {
    path.ancestors().find(|p| p.is_dir()).unwrap_or(path)
}

/// Refuses a location in Git metadata or in another checkout. A repository that surrounds the
/// main worktree itself (a home folder under version control) is no reason to refuse.
pub fn check(main: &Path, common: &Path, destination: &Path) -> Result<()> {
    let destination = resolved(destination);
    let main = resolved(main);
    if destination
        .components()
        .any(|part| part.as_os_str().eq_ignore_ascii_case(".git"))
        || destination.starts_with(resolved(common))
    {
        return Err(fail(
            "A worktree cannot be inside the Git metadata of a repository",
        ));
    }
    if main.starts_with(&destination) {
        return Err(fail(
            "Choose a folder that does not contain the main worktree",
        ));
    }
    // From the parent: a taken name is a worktree itself, and `free` picks the next one.
    let parent = destination.parent().unwrap_or(&destination);
    if let Some(tree) = super::gitstatus::work_tree(existing(parent)) {
        let tree = resolved(&tree);
        if tree != main && !main.starts_with(&tree) {
            return Err(fail(format!(
                "A worktree cannot be inside another worktree or repository: {}",
                tree.display()
            )));
        }
    }
    Ok(())
}

/// `path`, or the first of `path-2`, `path-3`, … that is free on disk and not `taken`.
pub fn free(path: &Path, taken: impl Fn(&Path) -> bool) -> Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| fail("The worktree location is not a valid folder"))?
        .to_string_lossy()
        .into_owned();
    (1..1000)
        .map(|n| match n {
            1 => path.to_path_buf(),
            n => path.with_file_name(format!("{name}-{n}")),
        })
        .find(|candidate| candidate.symlink_metadata().is_err() && !taken(candidate))
        .ok_or_else(|| fail("No free folder name is left at this worktree location"))
}

/// The local exclude rule for a worktree at `destination` inside `main`; None when it is
/// outside. The folder that collects the worktrees is excluded as a whole only while it holds
/// nothing else (`only_worktrees`), so new files next to a worktree stay visible in the status.
pub fn exclude_rule(main: &Path, destination: &Path, only_worktrees: bool) -> Option<String> {
    let inside = resolved(destination);
    let relative = inside.strip_prefix(resolved(main)).ok()?.to_path_buf();
    let folder = match relative.parent() {
        Some(parent) if only_worktrees && !parent.as_os_str().is_empty() => parent,
        _ => &relative,
    };
    let mut rule = String::new();
    for part in folder.components() {
        rule.push('/');
        rule.push_str(&super::gitworktree::escape_rule(
            &part.as_os_str().to_string_lossy(),
        ));
    }
    rule.push('/');
    Some(rule)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_are_valid_everywhere() {
        for (branch, expected) in [
            ("main", "main"),
            ("feature/login", "feature-login"),
            ("feature//x", "feature-x"),
            ("a\\b:c*d?e\"f<g>h|i", "a-b-c-d-e-f-g-h-i"),
            ("with space", "with-space"),
            (".hidden", "hidden"),
            ("..", "worktree"),
            ("-dash-", "dash"),
            ("ends.", "ends"),
            ("CON", "_CON"),
            ("nul.txt", "_nul.txt"),
            ("com1", "_com1"),
            ("common", "common"),
            ("čšž/ž", "čšž-ž"),
            ("", "worktree"),
        ] {
            assert_eq!(slug(branch), expected, "{branch}");
        }
        let long = slug(&"ž".repeat(200));
        assert!(long.len() <= MAX_NAME && long.chars().all(|c| c == 'ž'));
    }

    #[test]
    fn templates_expand_inside_beside_and_elsewhere() {
        let base = if cfg!(windows) { "C:\\work" } else { "/work" };
        let main = Path::new(base).join("app");
        let at = |template: &str| expand(template, &main, "feature/x").unwrap();
        assert_eq!(at(""), main.join(".worktrees").join("feature-x"));
        assert_eq!(
            at(DEFAULT_TEMPLATE),
            main.join(".worktrees").join("feature-x")
        );
        assert_eq!(
            at("../{repo}-{branch}"),
            Path::new(base).join("app-feature-x")
        );
        assert_eq!(at("{repo}-{branch}"), main.join("app-feature-x"));
        assert_eq!(
            at("~/trees/{repo}/{branch}"),
            dirs::home_dir()
                .unwrap()
                .join("trees")
                .join("app")
                .join("feature-x")
        );
        // Windows paths come out with one kind of separator.
        assert!(!cfg!(windows) || !at("").to_string_lossy().contains('/'));
        for bad in [
            "{repo}/x",
            "{repo}/{branch}/{user}",
            "{branch",
            "../../../../../../{branch}",
            "{repo}/{branch}\n",
        ] {
            // A template with a trailing line break is trimmed and then valid.
            assert_eq!(
                expand(bad, &main, "b").is_err(),
                !bad.ends_with('\n'),
                "{bad}"
            );
        }
    }

    #[test]
    fn taken_names_get_a_number() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("x");
        assert_eq!(free(&path, |_| false).unwrap(), path);
        std::fs::create_dir(&path).unwrap();
        assert_eq!(free(&path, |_| false).unwrap(), t.path().join("x-2"));
        // Registered but missing on disk: Git would refuse the name.
        assert_eq!(
            free(&path, |p| p.ends_with("x-2")).unwrap(),
            t.path().join("x-3")
        );
    }

    #[test]
    fn metadata_and_other_checkouts_are_refused() {
        let t = tempfile::tempdir().unwrap();
        let main = t.path().join("app");
        let common = main.join(".git");
        std::fs::create_dir_all(&common).unwrap();
        let linked = main.join(".worktrees").join("a");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(linked.join(".git"), "gitdir: elsewhere\n").unwrap();
        let other = t.path().join("other");
        std::fs::create_dir_all(other.join(".git")).unwrap();
        for good in [
            main.join(".worktrees/b"),
            main.join("x/y/b"),
            t.path().join("app-b"),
        ] {
            assert!(check(&main, &common, &good).is_ok(), "{good:?}");
        }
        for bad in [
            main.join(".git/worktrees/b"),
            linked.join("b"),
            linked.join("deep/b"),
            other.join("b"),
            main.clone(),
            t.path().to_path_buf(),
        ] {
            assert!(check(&main, &common, &bad).is_err(), "{bad:?}");
        }
        // A repository around everything is not another checkout.
        std::fs::create_dir(t.path().join(".git")).unwrap();
        assert!(check(&main, &common, &t.path().join("app-b")).is_ok());
    }

    #[test]
    fn exclude_rules_cover_only_worktrees() {
        let t = tempfile::tempdir().unwrap();
        let main = t.path().join("app");
        std::fs::create_dir(&main).unwrap();
        let rule = |p: &str, all| exclude_rule(&main, &main.join(p), all);
        assert_eq!(rule(".worktrees/a", true).unwrap(), "/.worktrees/");
        assert_eq!(rule("docs/a", false).unwrap(), "/docs/a/");
        assert_eq!(rule("a", true).unwrap(), "/a/");
        assert_eq!(rule("a [1]/b", true).unwrap(), "/a\\ \\[1\\]/");
        assert_eq!(exclude_rule(&main, &t.path().join("app-a"), true), None);
    }
}
