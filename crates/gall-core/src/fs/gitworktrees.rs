//! Worktrees as places to work in: the list for the switcher, the state of one worktree for a
//! visible row, and the ones that can be cleaned up.
//!
//! The list is read from Git's files and starts no process, so it may open on a key press. The
//! state of a row costs one `git status` and is asked only for rows on screen.
use super::gitadvanced::{main_and_common, worktree_git_dir};
use super::gitgraph::{git, root};
use crate::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// A worktree as Git's files describe it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub path: String,
    /// None for a detached HEAD.
    pub branch: Option<String>,
    /// Empty before the first commit.
    pub head: String,
    pub main: bool,
    pub current: bool,
    pub locked: bool,
    /// Its folder is gone.
    pub missing: bool,
    /// When Git last changed something there (milliseconds since 1970).
    pub used: Option<i64>,
}

/// The state of one worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub branch: Option<String>,
    /// Changed, staged, conflicted and new items.
    pub dirty: usize,
    pub conflicts: usize,
    pub ahead: u32,
    pub behind: u32,
    pub upstream: bool,
}

/// Why a worktree is offered for removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// Every commit of its branch is in the branch of the main worktree.
    Merged,
    /// The upstream branch was deleted on the remote.
    Gone,
    /// Its folder is gone.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    #[serde(flatten)]
    pub place: Place,
    pub reason: Reason,
}

/// The newest change of what Git writes when someone works in a worktree.
fn used(git_dir: &Path) -> Option<i64> {
    ["index", "HEAD", "logs/HEAD"]
        .iter()
        .filter_map(|name| git_dir.join(name).metadata().ok()?.modified().ok())
        .filter_map(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_millis() as i64)
        .max()
}

/// The branch and commit in a `HEAD` file. The commit stays empty where the files cannot tell
/// (a reftable repository): the row then shows the branch alone.
fn head(git_dir: &Path, common: &Path, packed: &str) -> (Option<String>, String) {
    let text = std::fs::read_to_string(git_dir.join("HEAD")).unwrap_or_default();
    let text = text.trim();
    let Some(name) = text.strip_prefix("ref: ") else {
        return (None, text.to_owned());
    };
    let id = std::fs::read_to_string(common.join(name))
        .ok()
        .map(|id| id.trim().to_owned())
        .or_else(|| {
            packed
                .lines()
                .find_map(|line| line.strip_suffix(name)?.strip_suffix(' '))
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let branch = name.strip_prefix("refs/heads/").unwrap_or(name);
    // The placeholder of a reftable repository.
    let branch = (branch != ".invalid").then(|| branch.to_owned());
    (branch, id)
}

/// Every worktree of the repository, the most recently used first, read from Git's files.
pub fn places(path: &Path) -> Result<Vec<Place>> {
    let root = root(path)?;
    let (main, common) = main_and_common(&root)?;
    let packed = std::fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
    let same = |a: &Path, b: &Path| {
        a == b
            || matches!((crate::nav::path::canonicalize(a), crate::nav::path::canonicalize(b)),
                (Ok(a), Ok(b)) if a == b)
    };
    let place = |folder: PathBuf, git_dir: &Path, main: bool| {
        let (branch, head) = head(git_dir, &common, &packed);
        Place {
            missing: !folder.is_dir(),
            current: same(&folder, &root),
            locked: !main && git_dir.join("locked").exists(),
            used: used(git_dir),
            path: folder.to_string_lossy().into_owned(),
            branch,
            head,
            main,
        }
    };
    let mut all = Vec::new();
    // A bare repository has no main worktree.
    if worktree_git_dir(&main).is_some() {
        all.push(place(main, &common, true));
    }
    if let Ok(linked) = std::fs::read_dir(common.join("worktrees")) {
        for entry in linked.flatten() {
            let git_dir = entry.path();
            let Ok(text) = std::fs::read_to_string(git_dir.join("gitdir")) else {
                continue;
            };
            // It names the `.git` file of the worktree, possibly relative to this folder.
            let dot = super::gitadvanced::worktree_metadata_path(&root, &git_dir, text.trim());
            if let Some(folder) = dot.parent() {
                all.push(place(folder.to_path_buf(), &git_dir, false));
            }
        }
    }
    all.sort_by(|a, b| b.used.cmp(&a.used).then_with(|| a.path.cmp(&b.path)));
    Ok(all)
}

/// The state of the worktree at `path`, with one process.
pub fn state(path: &Path) -> Result<State> {
    let output = git(
        path,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=all",
        ],
    )?;
    Ok(parse_state(&output))
}

fn parse_state(output: &str) -> State {
    let mut state = State::default();
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        if let Some(name) = field.strip_prefix("# branch.head ") {
            state.branch = (name != "(detached)").then(|| name.to_owned());
        } else if field.starts_with("# branch.upstream ") {
            state.upstream = true;
        } else if let Some(counts) = field.strip_prefix("# branch.ab ") {
            let mut counts = counts.split(' ');
            let mut count = |sign: char| {
                counts
                    .next()
                    .and_then(|text| text.strip_prefix(sign)?.parse().ok())
                    .unwrap_or_default()
            };
            state.ahead = count('+');
            state.behind = count('-');
        } else if field.starts_with("2 ") {
            state.dirty += 1;
            // The old name of a rename is a field of its own.
            fields.next();
        } else if field.starts_with("u ") {
            state.dirty += 1;
            state.conflicts += 1;
        } else if field.starts_with("1 ") || field.starts_with("? ") {
            state.dirty += 1;
        }
    }
    state
}

/// Worktrees that look finished. Nothing is removed here; whether one is clean is decided when
/// it is removed.
pub fn cleanup(path: &Path) -> Result<Vec<Candidate>> {
    let root = root(path)?;
    let places = places(&root)?;
    let target = places.iter().find(|place| place.main).and_then(|place| {
        place
            .branch
            .as_ref()
            .map(|branch| format!("refs/heads/{branch}"))
    });
    let listed = |extra: &[&str]| -> Result<Vec<(String, String)>> {
        let mut args = vec![
            "for-each-ref",
            "--format=%(refname:lstrip=2)%00%(upstream:track)",
        ];
        args.extend_from_slice(extra);
        args.push("refs/heads/");
        Ok(git(&root, &args)?
            .lines()
            .filter_map(|line| line.split_once('\0'))
            .map(|(name, track)| (name.to_owned(), track.to_owned()))
            .collect())
    };
    let gone = listed(&[])?;
    let merged = match &target {
        Some(target) => listed(&[&format!("--merged={target}")])?,
        None => Vec::new(),
    };
    Ok(places
        .into_iter()
        .filter(|place| !place.main && !place.current && !place.locked)
        .filter_map(|place| {
            let reason = if place.missing {
                Reason::Missing
            } else {
                // A detached worktree belongs to no branch, so nothing says it is finished.
                let branch = place.branch.as_ref()?;
                let is = |list: &[(String, String)], want: &str| {
                    list.iter()
                        .any(|(name, track)| name == branch && (want.is_empty() || track == want))
                };
                if is(&gone, "[gone]") {
                    Reason::Gone
                } else if is(&merged, "") {
                    Reason::Merged
                } else {
                    return None;
                }
            };
            Some(Candidate { place, reason })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("main");
        std::fs::create_dir(&p).unwrap();
        git(&p, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("core.autocrlf", "false"),
        ] {
            git(&p, &["config", key, value]).unwrap();
        }
        std::fs::write(p.join("file.txt"), "one\n").unwrap();
        git(&p, &["add", "--all"]).unwrap();
        git(&p, &["commit", "-qm", "one"]).unwrap();
        d
    }

    fn add(main: &Path, name: &str, args: &[&str]) -> PathBuf {
        let folder = main.parent().unwrap().join(name);
        let text = folder.to_string_lossy().into_owned();
        let mut all = vec!["worktree", "add", "-q"];
        all.extend_from_slice(args);
        all.push(&text);
        git(main, &all).unwrap();
        folder
    }

    #[test]
    fn the_list_agrees_with_git() {
        let d = repository();
        let main = d.path().join("main");
        let one = add(&main, "one", &["-b", "feature/one"]);
        add(&main, "two", &["--detach"]);
        git(&main, &["pack-refs", "--all"]).unwrap();
        for from in [&main, &one] {
            let found = places(from).unwrap();
            assert_eq!(found.len(), 3);
            assert_eq!(found.iter().filter(|place| place.current).count(), 1);
            let current = found.iter().find(|place| place.current).unwrap();
            assert_eq!(current.main, from == &main);
            let listed = super::super::gitadvanced::snapshot(from, None)
                .unwrap()
                .worktrees;
            for place in &found {
                let same = |path: &str| {
                    crate::nav::path::canonicalize(Path::new(path)).unwrap()
                        == crate::nav::path::canonicalize(Path::new(&place.path)).unwrap()
                };
                let listed = listed.iter().find(|tree| same(&tree.path)).unwrap();
                assert_eq!(
                    (&listed.head, &listed.branch),
                    (&place.head, &place.branch),
                    "{}",
                    place.path
                );
            }
        }
    }

    #[test]
    fn locked_and_missing_worktrees_are_marked() {
        let d = repository();
        let main = d.path().join("main");
        let one = add(&main, "one", &["-b", "one"]);
        git(&main, &["worktree", "lock", "--", &one.to_string_lossy()]).unwrap();
        let two = add(&main, "two", &["-b", "two"]);
        std::fs::remove_dir_all(&two).unwrap();
        let found = places(&main).unwrap();
        let by = |name: &str| {
            found
                .iter()
                .find(|place| place.branch.as_deref() == Some(name))
                .unwrap()
        };
        assert!(by("one").locked && !by("one").missing);
        assert!(by("two").missing && !by("two").locked);
        assert!(by("main").main && !by("main").locked);
    }

    #[test]
    fn the_state_counts_what_git_reports() {
        let d = repository();
        let main = d.path().join("main");
        let one = add(&main, "one", &["-b", "one"]);
        assert_eq!(
            state(&one).unwrap(),
            State {
                branch: Some("one".into()),
                ..State::default()
            }
        );
        std::fs::write(one.join("file.txt"), "changed\n").unwrap();
        std::fs::write(one.join("new ž.txt"), "new\n").unwrap();
        git(&one, &["add", "new ž.txt"]).unwrap();
        std::fs::write(one.join("other.txt"), "other\n").unwrap();
        assert_eq!(state(&one).unwrap().dirty, 3);
        // The other worktree is not touched by it.
        assert_eq!(state(&main).unwrap().dirty, 0);
        git(&one, &["checkout", "-q", "--detach", "-f"]).unwrap();
        assert_eq!(state(&one).unwrap().branch, None);
    }

    #[test]
    fn a_rename_counts_once_and_tracking_is_read() {
        let parsed = parse_state(
            "# branch.oid abc\0# branch.head x\0# branch.upstream origin/x\0# branch.ab +2 -3\0\
             2 R. N... 100644 100644 100644 a b R100 new.txt\0old.txt\0u UU N... 1 1 1 1 a b c f\0? n\0",
        );
        assert_eq!(
            parsed,
            State {
                branch: Some("x".into()),
                dirty: 3,
                conflicts: 1,
                ahead: 2,
                behind: 3,
                upstream: true,
            }
        );
    }

    #[test]
    fn cleanup_offers_merged_gone_and_missing_only() {
        let d = repository();
        let main = d.path().join("main");
        let commit = |folder: &Path, name: &str| {
            std::fs::write(folder.join(name), name).unwrap();
            git(folder, &["add", "--all"]).unwrap();
            git(folder, &["commit", "-qm", name]).unwrap();
        };
        let merged = add(&main, "merged", &["-b", "merged"]);
        commit(&merged, "m.txt");
        git(&main, &["merge", "-q", "--ff-only", "merged"]).unwrap();
        let open = add(&main, "open", &["-b", "open"]);
        commit(&open, "o.txt");
        let gone = add(&main, "gone", &["-b", "gone"]);
        commit(&gone, "g.txt");
        for (key, value) in [
            ("remote.origin.url", "../nowhere"),
            ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
            ("branch.gone.remote", "origin"),
            ("branch.gone.merge", "refs/heads/gone"),
        ] {
            git(&main, &["config", key, value]).unwrap();
        }
        add(&main, "detached", &["--detach"]);
        let locked = add(&main, "locked", &["-b", "locked"]);
        git(
            &main,
            &["worktree", "lock", "--", &locked.to_string_lossy()],
        )
        .unwrap();
        let missing = add(&main, "missing", &["-b", "missing"]);
        commit(&missing, "x.txt");
        std::fs::remove_dir_all(&missing).unwrap();
        let mut found: Vec<_> = cleanup(&main)
            .unwrap()
            .into_iter()
            .map(|c| (c.place.branch.unwrap(), c.reason))
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            found,
            [
                ("gone".to_owned(), Reason::Gone),
                ("merged".to_owned(), Reason::Merged),
                ("missing".to_owned(), Reason::Missing),
            ]
        );
        // Never the worktree one is in.
        assert!(cleanup(&merged)
            .unwrap()
            .iter()
            .all(|c| c.place.branch.as_deref() != Some("merged")));
    }
}
