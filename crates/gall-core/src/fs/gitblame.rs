//! Who last changed each line, streamed as Git finds it (`git blame --incremental`).
//!
//! Commits listed in `blame.ignoreRevsFile` or in `.git-blame-ignore-revs` at the top of the work
//! tree (formatting only, by convention) are skipped.
use super::gitgraph::root;
use super::gitinspect::{commit_id, read_git, relative_file, PATCH_LIMIT};
use crate::{CoreError, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Lines looked at; the rest of a longer file is left out, and the answer says so.
pub const MAX_LINES: usize = 10_000;
const IGNORE_REVS: &str = ".git-blame-ignore-revs";

/// A commit that lines come from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameCommit {
    /// Only zeros: not committed yet.
    pub id: String,
    pub author: String,
    /// Seconds since 1970.
    pub time: i64,
    pub subject: String,
    /// The commit and name of the file before, to look further back ("peel"). None where the
    /// file begins.
    pub previous: Option<(String, String)>,
}

/// `count` lines from `line` on (1-based) come from `commit`, where they begin at `original`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameRange {
    pub commit: String,
    pub line: u32,
    pub count: u32,
    pub original: u32,
    /// The name of the file in that commit.
    pub file: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlamePart {
    /// Commits named for the first time.
    pub commits: Vec<BlameCommit>,
    pub ranges: Vec<BlameRange>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameEnd {
    /// Only the first [`MAX_LINES`] lines were looked at.
    pub limited: bool,
    /// Commits were skipped by a list of the repository.
    pub ignored: bool,
}

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

/// Git's answer, record by record: a header, the facts of a commit the first time it is named, and
/// `filename` as the last line of every record.
#[derive(Default)]
pub(super) struct Parser {
    commits: HashMap<String, BlameCommit>,
    sent: std::collections::HashSet<String>,
    open: Option<BlameRange>,
}

impl Parser {
    pub(super) fn feed(&mut self, text: &str) -> BlamePart {
        let mut part = BlamePart::default();
        for line in text.lines() {
            let Some(range) = &mut self.open else {
                let mut fields = line.split(' ');
                let id = fields.next().unwrap_or_default();
                let mut number = || fields.next().and_then(|n| n.parse::<u32>().ok());
                if let (Some(original), Some(line), Some(count)) = (number(), number(), number()) {
                    self.commits
                        .entry(id.to_owned())
                        .or_insert_with(|| BlameCommit {
                            id: id.to_owned(),
                            ..Default::default()
                        });
                    self.open = Some(BlameRange {
                        commit: id.to_owned(),
                        line,
                        count,
                        original,
                        file: String::new(),
                    });
                }
                continue;
            };
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            if key == "filename" {
                range.file = unquote(value);
                if self.sent.insert(range.commit.clone()) {
                    part.commits
                        .extend(self.commits.get(&range.commit).cloned());
                }
                part.ranges.extend(self.open.take());
                continue;
            }
            let Some(commit) = self.commits.get_mut(&range.commit) else {
                continue;
            };
            match key {
                "author" => commit.author = value.to_owned(),
                "author-time" => commit.time = value.parse().unwrap_or_default(),
                "summary" => commit.subject = value.to_owned(),
                "previous" => {
                    commit.previous = value
                        .split_once(' ')
                        .map(|(id, file)| (id.to_owned(), unquote(file)));
                }
                _ => {}
            }
        }
        part
    }
}

/// A name as Git prints it: in quotes with escapes when it has special characters.
fn unquote(name: &str) -> String {
    let Some(inner) = name
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return name.to_owned();
    };
    let mut bytes = Vec::new();
    let mut rest = inner.bytes();
    while let Some(byte) = rest.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match rest.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'a') => bytes.push(7),
            Some(b'b') => bytes.push(8),
            Some(b'v') => bytes.push(11),
            Some(b'f') => bytes.push(12),
            Some(digit @ b'0'..=b'3') => {
                let octal = [Some(digit), rest.next(), rest.next()]
                    .into_iter()
                    .flatten()
                    .fold(0u32, |value, digit| {
                        value * 8 + u32::from(digit.wrapping_sub(b'0'))
                    });
                bytes.push(octal as u8);
            }
            Some(other) => bytes.push(other),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Where a file on disk is in its repository: the work tree and the name in it.
pub fn locate(file: &Path) -> Result<(PathBuf, String)> {
    let folder = file.parent().ok_or_else(|| fail("Choose a file"))?;
    let root = root(folder)?;
    let name = file
        .strip_prefix(&root)
        .map(Path::to_path_buf)
        .or_else(|_| {
            let canonical =
                crate::nav::path::canonicalize(file).map_err(|e| fail(e.to_string()))?;
            let top = crate::nav::path::canonicalize(&root).map_err(|e| fail(e.to_string()))?;
            canonical
                .strip_prefix(&top)
                .map(Path::to_path_buf)
                .map_err(|_| fail("The file is not in this repository"))
        })?;
    let parts: Vec<_> = name
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect();
    Ok((root, parts.join("/")))
}

/// Blame of a file in a commit, or of the file on disk without one. `part` gets what Git found so
/// far; stops silently when `cancelled` says so.
pub fn blame(
    path: &Path,
    commit: Option<&str>,
    file: &str,
    cancelled: &dyn Fn() -> bool,
    mut part: impl FnMut(BlamePart),
) -> Result<BlameEnd> {
    relative_file(file)?;
    let root = root(path)?;
    super::gitfacts::keep_commit_graph(&root);
    // Reject binary content and large files before asking Git to attribute lines.
    let (commit, content, large) = match commit {
        Some(commit) => {
            let commit = commit_id(&root, commit)?;
            let spec = format!("{commit}:{file}");
            let (content, large) = read_git(&root, &["cat-file", "blob", &spec], PATCH_LIMIT)?;
            (Some(commit), content, large)
        }
        None => {
            let place = root.join(file);
            let meta = place
                .symlink_metadata()
                .map_err(|e| fail(format!("{file}: {e}")))?;
            if !meta.is_file() {
                return Err(fail("Blame is available for files"));
            }
            let large = meta.len() > PATCH_LIMIT as u64;
            let content = if large {
                Vec::new()
            } else {
                std::fs::read(&place).map_err(|e| fail(format!("{file}: {e}")))?
            };
            (None, content, large)
        }
    };
    if large {
        return Err(fail("Blame preview is limited to files up to 1 MiB"));
    }
    if content.contains(&0) {
        return Err(fail("Blame is not available for binary files"));
    }
    if content.is_empty() {
        return Ok(BlameEnd::default());
    }
    let raw_lines = content.iter().filter(|byte| **byte == b'\n').count()
        + usize::from(content.last() != Some(&b'\n'));
    let limited = content.iter().filter(|byte| **byte == b'\n').count() > MAX_LINES;
    let total_lines = raw_lines.min(MAX_LINES) as u64;
    let listed = root.join(IGNORE_REVS).is_file();
    let run = |ignoring: bool, part: &mut dyn FnMut(BlamePart)| {
        let mut args = ["blame", "--incremental", "--no-textconv"]
            .map(String::from)
            .to_vec();
        if !ignoring {
            args.push("--ignore-revs-file=".into());
        }
        if ignoring && listed {
            args.push(format!("--ignore-revs-file={IGNORE_REVS}"));
        }
        if limited {
            args.push(format!("-L1,{MAX_LINES}"));
        }
        args.extend(commit.clone());
        args.extend(["--".into(), file.to_owned()]);
        let mut parser = Parser::default();
        let mut blamed_lines = 0u64;
        super::gitprogress::report_counts(
            "Blaming file",
            0,
            Some(total_lines),
            "lines",
            Some(file.to_owned()),
        );
        super::gitsearch::stream(&root, &args, cancelled, |bytes| {
            let found = parser.feed(&String::from_utf8_lossy(&bytes));
            if !found.ranges.is_empty() {
                blamed_lines = blamed_lines
                    .saturating_add(found.ranges.iter().map(|r| u64::from(r.count)).sum::<u64>());
                super::gitprogress::report_counts(
                    "Blaming file",
                    blamed_lines.min(total_lines),
                    Some(total_lines),
                    "lines",
                    Some(file.to_owned()),
                );
                part(found);
            }
            true
        })
    };
    let mut ignored = listed;
    if let Err(error) = run(true, &mut part) {
        // Retry without Gall's optional list. Git still honors its own configured lists.
        let text = error.to_string();
        if !["object name", "ignore"]
            .iter()
            .any(|hint| text.contains(hint))
        {
            return Err(error);
        }
        ignored = false;
        run(false, &mut part).map_err(|retry| {
            fail(format!(
                "Git could not read blame. Check the configured blame.ignoreRevsFile: {retry}"
            ))
        })?;
    }
    Ok(BlameEnd { limited, ignored })
}

#[cfg(test)]
mod tests {
    use super::super::gitgraph::git;
    use super::*;

    fn repository() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "Test"],
            &["config", "user.email", "test@example.com"],
            &["config", "core.autocrlf", "false"],
        ] {
            git(d.path(), args).unwrap();
        }
        d
    }

    fn commit(p: &Path, file: &str, text: &str, message: &str) -> String {
        std::fs::write(p.join(file), text).unwrap();
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", message]).unwrap();
        git(p, &["rev-parse", "HEAD"]).unwrap().trim().into()
    }

    /// Commit of each line, and the commits by id.
    fn collect(
        p: &Path,
        commit: Option<&str>,
        file: &str,
    ) -> (Vec<String>, HashMap<String, BlameCommit>, BlameEnd) {
        let (mut lines, mut commits) = (Vec::new(), HashMap::new());
        let end = blame(p, commit, file, &|| false, |part| {
            commits.extend(part.commits.into_iter().map(|c| (c.id.clone(), c)));
            for range in part.ranges {
                let last = (range.line + range.count - 1) as usize;
                if lines.len() < last {
                    lines.resize(last, String::new());
                }
                for line in range.line - 1..range.line + range.count - 1 {
                    lines[line as usize] = range.commit.clone();
                }
            }
        })
        .unwrap();
        (lines, commits, end)
    }

    #[test]
    fn lines_agree_with_git_and_can_be_peeled() {
        let d = repository();
        let p = d.path();
        let one = commit(p, "old ž.txt", "a\nb\nc\n", "one");
        let two = commit(p, "old ž.txt", "a\nB\nc\n", "two");
        git(p, &["mv", "old ž.txt", "new ž.txt"]).unwrap();
        git(p, &["commit", "-qm", "rename"]).unwrap();
        let three = commit(p, "new ž.txt", "a\nBB\nc\nd\n", "three");
        let (lines, commits, end) = collect(p, Some(&three), "new ž.txt");
        assert_eq!(
            lines,
            [one.as_str(), three.as_str(), one.as_str(), three.as_str()]
        );
        assert_eq!(end, BlameEnd::default());
        assert_eq!(commits[&three].subject, "three");
        assert_eq!(commits[&three].author, "Test");
        // Back to what was there before.
        let (id, name) = commits[&three].previous.clone().unwrap();
        assert_eq!(name, "new ž.txt");
        let (lines, commits, _) = collect(p, Some(&id), &name);
        assert_eq!(lines, [one.as_str(), two.as_str(), one.as_str()]);
        let (id, name) = commits[&two].previous.clone().unwrap();
        assert_eq!((id.as_str(), name.as_str()), (one.as_str(), "old ž.txt"));
        let (lines, commits, _) = collect(p, Some(&id), &name);
        assert_eq!(lines, [one.as_str(), one.as_str(), one.as_str()]);
        assert_eq!(commits[&one].previous, None);
        // The file on disk, with a line that is in no commit.
        std::fs::write(p.join("new ž.txt"), "a\nBB\nmine\nd\n").unwrap();
        let (lines, _, _) = collect(p, None, "new ž.txt");
        assert!(lines[2].bytes().all(|b| b == b'0'));
        assert_eq!(lines[1], three);
        let (found, file) = locate(&p.join("new ž.txt")).unwrap();
        assert_eq!(file, "new ž.txt");
        assert!(root(&found).is_ok());
    }

    #[test]
    fn listed_commits_are_skipped_and_a_wrong_list_is_not_fatal() {
        let d = repository();
        let p = d.path();
        let one = commit(p, "f.txt", "a\nb\n", "one");
        let format = commit(p, "f.txt", "  a\n  b\n", "format");
        assert_eq!(
            collect(p, Some(&format), "f.txt").0,
            [format.as_str(), format.as_str()]
        );
        std::fs::write(p.join(IGNORE_REVS), format!("# formatting\n{format}\n")).unwrap();
        let (lines, _, end) = collect(p, Some(&format), "f.txt");
        assert_eq!(lines, [one.as_str(), one.as_str()]);
        assert!(end.ignored);
        std::fs::write(p.join(IGNORE_REVS), "this is no commit\n").unwrap();
        let (lines, _, end) = collect(p, Some(&format), "f.txt");
        assert_eq!(lines, [format.as_str(), format.as_str()]);
        assert!(!end.ignored);
        std::fs::remove_file(p.join(IGNORE_REVS)).unwrap();
        git(p, &["config", "blame.ignoreRevsFile", "missing-list"]).unwrap();
        let error = blame(p, Some(&format), "f.txt", &|| false, |_| {}).unwrap_err();
        assert!(error.to_string().contains("blame.ignoreRevsFile"));
    }

    #[test]
    fn limits_and_cancelling() {
        let d = repository();
        let p = d.path();
        let long = commit(p, "long.txt", &"x\n".repeat(MAX_LINES + 5), "long");
        let (lines, _, end) = collect(p, Some(&long), "long.txt");
        assert!(end.limited);
        assert_eq!(lines.len(), MAX_LINES);
        let empty = commit(p, "empty.txt", "", "empty");
        assert!(collect(p, Some(&empty), "empty.txt").0.is_empty());
        let mut parts = 0;
        blame(p, Some(&long), "long.txt", &|| true, |_| parts += 1).unwrap();
        assert_eq!(parts, 0);
        std::fs::write(p.join("big.txt"), vec![b'x'; PATCH_LIMIT + 1]).unwrap();
        assert!(blame(p, None, "big.txt", &|| false, |_| {}).is_err());
        assert!(blame(p, None, "../outside.txt", &|| false, |_| {}).is_err());
    }

    #[test]
    fn records_split_anywhere_are_read_and_names_unquoted() {
        assert_eq!(unquote(r#""a\tb\305\276\"c""#), "a\tbž\"c");
        assert_eq!(unquote("plain name.txt"), "plain name.txt");
        let id = "a".repeat(40);
        let mut parser = Parser::default();
        let first = parser.feed(&format!(
            "{id} 1 2 3\nauthor A B\nauthor-time 12\nsummary filename is here\n"
        ));
        assert_eq!(first, BlamePart::default());
        let part = parser.feed(&format!("filename f.txt\n{id} 9 8 1\nfilename f.txt\n"));
        assert_eq!(part.commits.len(), 1);
        assert_eq!(part.commits[0].subject, "filename is here");
        assert_eq!(part.ranges.len(), 2);
        assert_eq!((part.ranges[1].line, part.ranges[1].original), (8, 9));
    }
}
