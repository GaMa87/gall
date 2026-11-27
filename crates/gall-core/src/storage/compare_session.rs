//! Comparison workspaces that survive a restart: the tabs of the comparison window with what is
//! needed to reopen each one (where its two sides come from, the view, the options, a logical
//! scroll anchor and the active hunk).
//!
//! The record never holds text of the comparison itself. A side that has no place to be read
//! again (clipboard text, an edited buffer) is stored as "not saved" unless the caller asks
//! otherwise, so unsaved edits never reach the disk by accident. Restoring checks every file
//! first and says what it found (ready, changed, missing, not saved); it never reads or writes
//! the compared files.

use super::JsonStore;
use crate::fs::diff::{DiffOptions, DiffSource};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Layout of the record. A file of another version is not read (and then not overwritten
/// without the caller having looked at the result of `load`).
pub const VERSION: u32 = 1;
/// Most tabs kept; a longer record is cut when it is loaded.
pub const MAX_TABS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ViewMode {
    #[default]
    Split,
    Unified,
}

/// A place in the text that does not depend on the window size: a line of one side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollAnchor {
    /// `true`: a line of the left side, `false`: of the right one.
    pub left: bool,
    pub line: usize,
}

/// One side of a tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    /// What to read again; `None` for text that was not saved (and is not kept).
    pub source: Option<DiffSource>,
    /// Name shown in the tab, also when the source is gone.
    pub label: String,
    /// Size and modification time of a file when the tab was saved (`ops::file_revision`).
    #[serde(default)]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceState {
    /// Reads as it did.
    Ready,
    /// A file that changed since the tab was saved.
    Changed,
    /// A file that is not there (moved, renamed or deleted).
    Missing,
    /// Text that was not saved with the tab.
    NotSaved,
}

impl SourceRecord {
    /// The record of a source. `keep_text` stores `Text` sources too (off by default: unsaved
    /// buffers stay out of persistence).
    pub fn capture(source: &DiffSource, label: &str, keep_text: bool) -> Self {
        let source_kept = match source {
            DiffSource::Text { .. } if !keep_text => None,
            other => Some(other.clone()),
        };
        let revision = match &source_kept {
            Some(DiffSource::File { path }) => crate::fs::ops::file_revision(path),
            _ => None,
        };
        Self {
            source: source_kept,
            label: label.to_string(),
            revision,
        }
    }

    pub fn state(&self) -> SourceState {
        match &self.source {
            None => SourceState::NotSaved,
            Some(DiffSource::File { path }) => match crate::fs::ops::file_revision(path) {
                None => SourceState::Missing,
                Some(now) if self.revision.as_deref().is_some_and(|was| was != now) => {
                    SourceState::Changed
                }
                Some(_) => SourceState::Ready,
            },
            // A blob of a commit never changes, but its repository must still be there.
            Some(DiffSource::GitBlob { root, .. }) if !root.exists() => SourceState::Missing,
            // HEAD, a commit of a file and a blob: the object must still exist (checked through Git, without
            // reading the content), else the tab is shown as missing and not as ready.
            Some(source) if !crate::fs::diff::git_source_present(source) => SourceState::Missing,
            Some(_) => SourceState::Ready,
        }
    }

    /// Points a file below `from` to the same place below `to`. Returns whether it changed.
    fn relink(&mut self, from: &Path, to: &Path) -> bool {
        let Some(DiffSource::File { path }) = &mut self.source else {
            return false;
        };
        match strip_prefix_folded(path, from) {
            Some(rest) => {
                *path = to.join(rest);
                self.revision = crate::fs::ops::file_revision(path);
                true
            }
            None => false,
        }
    }
}

/// One path component as it is compared: on Windows and macOS (case-blind volumes, and NFD names
/// on macOS) case and Unicode form do not tell two names apart; elsewhere the name is exact.
fn folded(c: &std::ffi::OsStr) -> String {
    use unicode_normalization::UnicodeNormalization;
    let name = c.to_string_lossy();
    if cfg!(any(windows, target_os = "macos")) {
        name.nfc().collect::<String>().to_lowercase()
    } else {
        name.into_owned()
    }
}

/// `path` below `from` as the rest of the path, comparing component by component (saved paths may
/// no longer exist, so this is lexical, never through the file system).
fn strip_prefix_folded(path: &Path, from: &Path) -> Option<PathBuf> {
    let mut rest = path.components();
    for want in from.components() {
        let have = rest.next()?;
        if have != want && folded(have.as_os_str()) != folded(want.as_os_str()) {
            return None;
        }
    }
    Some(rest.as_path().to_path_buf())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabRecord {
    pub left: SourceRecord,
    pub right: SourceRecord,
    #[serde(default)]
    pub mode: ViewMode,
    #[serde(default)]
    pub options: DiffOptions,
    #[serde(default)]
    pub scroll: Option<ScrollAnchor>,
    /// Index of the change the user stepped to.
    #[serde(default)]
    pub active_hunk: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareSessions {
    pub version: u32,
    pub tabs: Vec<TabRecord>,
    /// The selected tab.
    pub active: usize,
}

impl Default for CompareSessions {
    fn default() -> Self {
        Self {
            version: VERSION,
            tabs: Vec::new(),
            active: 0,
        }
    }
}

/// What opening one saved tab would find.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabCheck {
    pub index: usize,
    pub left: SourceState,
    pub right: SourceState,
    /// Both sides can be read again (changed files compare as they are now).
    pub openable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub sessions: CompareSessions,
    pub checks: Vec<TabCheck>,
}

impl CompareSessions {
    /// Keeps the selection in range and unsaved text out, whatever the record says: a caller
    /// that hands over a `Text` source (an edited buffer, the clipboard) does not get it stored.
    pub fn normalize(&mut self) {
        self.tabs.truncate(MAX_TABS);
        for tab in &mut self.tabs {
            for side in [&mut tab.left, &mut tab.right] {
                if matches!(side.source, Some(DiffSource::Text { .. })) {
                    side.source = None;
                    side.revision = None;
                }
            }
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
    }

    /// Checks every tab; the sessions come back unchanged but normalized.
    pub fn restore(mut self) -> Restored {
        self.normalize();
        let checks = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let (left, right) = (tab.left.state(), tab.right.state());
                let readable = |s| matches!(s, SourceState::Ready | SourceState::Changed);
                TabCheck {
                    index,
                    left,
                    right,
                    openable: readable(left) && readable(right),
                }
            })
            .collect();
        Restored {
            sessions: self,
            checks,
        }
    }

    /// A folder was renamed or moved: every file below `from` is looked for below `to`. Returns
    /// how many sides were changed.
    pub fn relink(&mut self, from: &Path, to: &Path) -> usize {
        let mut n = 0;
        for tab in &mut self.tabs {
            n += usize::from(tab.left.relink(from, to));
            n += usize::from(tab.right.relink(from, to));
        }
        n
    }
}

/// Reads the record. A missing, corrupt or foreign-version file gives an empty one, and the
/// second value says whether a file of another version was found (so it is not overwritten
/// unnoticed).
pub fn load(path: impl Into<PathBuf>) -> (CompareSessions, bool) {
    let store = JsonStore::new(path);
    // The version is read first, so a file of another layout is recognised even when its tabs
    // could not be parsed with this one.
    let (raw, _) = store.load::<serde_json::Value>();
    if let Some(v) = raw.get("version").and_then(|v| v.as_u64()) {
        if v != u64::from(VERSION) {
            return (CompareSessions::default(), true);
        }
    }
    match serde_json::from_value::<CompareSessions>(raw) {
        Ok(mut loaded) => {
            loaded.normalize();
            (loaded, false)
        }
        Err(_) => (CompareSessions::default(), false),
    }
}

pub fn save(path: impl Into<PathBuf>, sessions: &CompareSessions) -> crate::Result<()> {
    let mut clean = sessions.clone();
    clean.version = VERSION;
    clean.normalize();
    JsonStore::new(path).save(&clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(left: &Path, right: &Path) -> TabRecord {
        let rec = |p: &Path| {
            SourceRecord::capture(
                &DiffSource::File {
                    path: p.to_path_buf(),
                },
                &p.file_name().unwrap().to_string_lossy(),
                false,
            )
        };
        TabRecord {
            left: rec(left),
            right: rec(right),
            mode: ViewMode::Unified,
            options: DiffOptions {
                ignore_case: true,
                moves_edited: true,
                ..DiffOptions::default()
            },
            scroll: Some(ScrollAnchor {
                left: false,
                line: 120,
            }),
            active_hunk: Some(3),
        }
    }

    fn two_files(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
        let (a, b) = (
            dir.join(format!("{name}-a.txt")),
            dir.join(format!("{name}-b.txt")),
        );
        std::fs::write(&a, "one\n").unwrap();
        std::fs::write(&b, "two\n").unwrap();
        (a, b)
    }

    #[test]
    fn git_sources_that_are_gone_are_missing_on_restore_and_present_ones_are_ready() {
        let d = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .current_dir(d.path())
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "core.autocrlf", "false"]);
        std::fs::write(d.path().join("a.txt"), "one\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "one"]);
        let state = |source: DiffSource| SourceRecord::capture(&source, "x", false).state();
        let present = d.path().join("a.txt");
        let absent = d.path().join("nowhere.txt");
        assert_eq!(
            state(DiffSource::GitHead {
                path: present.clone()
            }),
            SourceState::Ready
        );
        assert_eq!(
            state(DiffSource::GitHead {
                path: absent.clone()
            }),
            SourceState::Missing
        );
        let bogus = "0123456789abcdef0123456789abcdef01234567".to_string();
        assert_eq!(
            state(DiffSource::GitCommit {
                path: present,
                commit: bogus.clone()
            }),
            SourceState::Missing
        );
        assert_eq!(
            state(DiffSource::GitBlob {
                root: d.path().to_path_buf(),
                file: "a.txt".into(),
                commit: bogus
            }),
            SourceState::Missing
        );
    }

    #[test]
    fn several_tabs_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = two_files(d.path(), "x");
        let (c, e) = two_files(d.path(), "y");
        let sessions = CompareSessions {
            version: VERSION,
            tabs: vec![tab(&a, &b), tab(&c, &e)],
            active: 1,
        };
        let file = d.path().join("cmp.json");
        save(&file, &sessions).unwrap();
        let (back, foreign) = load(&file);
        assert!(!foreign);
        assert_eq!(back, sessions);
        let restored = back.restore();
        assert!(restored.checks.iter().all(|c| c.openable));
        assert_eq!(restored.sessions.active, 1);
    }

    #[test]
    fn missing_changed_and_unsaved_sides_are_told_apart() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = two_files(d.path(), "m");
        let mut t = tab(&a, &b);
        // Unsaved text is not kept.
        let text = DiffSource::Text {
            label: "clipboard".into(),
            text: "secret edits".into(),
        };
        let unsaved = SourceRecord::capture(&text, "clipboard", false);
        assert_eq!(unsaved.state(), SourceState::NotSaved);
        let json = serde_json::to_string(&unsaved).unwrap();
        assert!(!json.contains("secret edits"));
        assert!(SourceRecord::capture(&text, "clipboard", true)
            .source
            .is_some());

        t.left.revision = Some("0:0".into());
        std::fs::remove_file(&b).unwrap();
        assert_eq!(t.left.state(), SourceState::Changed);
        assert_eq!(t.right.state(), SourceState::Missing);
        let r = CompareSessions {
            version: VERSION,
            tabs: vec![t],
            active: 0,
        }
        .restore();
        assert!(!r.checks[0].openable);
    }

    #[test]
    fn a_renamed_folder_is_followed() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("old");
        std::fs::create_dir(&old).unwrap();
        let (a, b) = two_files(&old, "r");
        let mut s = CompareSessions {
            version: VERSION,
            tabs: vec![tab(&a, &b)],
            active: 0,
        };
        let new = d.path().join("new");
        std::fs::rename(&old, &new).unwrap();
        assert!(!s.clone().restore().checks[0].openable);
        assert_eq!(s.relink(&old, &new), 2);
        assert!(s.restore().checks[0].openable);
    }

    #[test]
    fn a_prefix_is_matched_by_component_and_folded_only_where_names_are_blind() {
        let rest = |p: &str, f: &str| strip_prefix_folded(Path::new(p), Path::new(f));
        assert_eq!(rest("/a/old/x.txt", "/a/old"), Some(PathBuf::from("x.txt")));
        // A longer name that merely starts with the same letters is another folder.
        assert_eq!(rest("/a/older/x.txt", "/a/old"), None);
        let folded_case = rest("/a/OLD/x.txt", "/a/old");
        if cfg!(any(windows, target_os = "macos")) {
            assert_eq!(folded_case, Some(PathBuf::from("x.txt")));
            // NFD on one side, NFC on the other.
            assert!(rest("/a/Cafe\u{301}/x", "/a/Caf\u{e9}").is_some());
        } else {
            assert_eq!(folded_case, None);
        }
    }

    #[test]
    fn selection_is_kept_in_range_and_long_records_are_cut() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = two_files(d.path(), "n");
        let mut s = CompareSessions {
            version: VERSION,
            tabs: (0..MAX_TABS + 5).map(|_| tab(&a, &b)).collect(),
            active: 999,
        };
        s.normalize();
        assert_eq!((s.tabs.len(), s.active), (MAX_TABS, MAX_TABS - 1));
        let empty = CompareSessions::default().restore();
        assert!(empty.checks.is_empty() && empty.sessions.active == 0);
    }

    #[test]
    fn unsaved_text_never_reaches_the_file() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = two_files(d.path(), "u");
        let mut t = tab(&a, &b);
        t.right.source = Some(DiffSource::Text {
            label: "clipboard".into(),
            text: "secret edits".into(),
        });
        let file = d.path().join("cmp.json");
        let sessions = CompareSessions {
            version: VERSION,
            tabs: vec![t],
            active: 0,
        };
        save(&file, &sessions).unwrap();
        assert!(!std::fs::read_to_string(&file)
            .unwrap()
            .contains("secret edits"));
        assert_eq!(load(&file).0.tabs[0].right.state(), SourceState::NotSaved);
    }

    #[test]
    fn a_foreign_version_of_another_shape_is_recognised() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("cmp.json");
        std::fs::write(
            &file,
            r#"{"version": 2, "tabs": [{"left": "x"}], "active": 0}"#,
        )
        .unwrap();
        let (s, foreign) = load(&file);
        assert!(foreign && s.tabs.is_empty());
    }

    #[test]
    fn a_blob_of_a_removed_repository_is_missing() {
        let d = tempfile::tempdir().unwrap();
        let rec = SourceRecord::capture(
            &DiffSource::GitBlob {
                root: d.path().join("gone"),
                file: "a.txt".into(),
                commit: "0".repeat(40),
            },
            "a.txt",
            false,
        );
        assert_eq!(rec.state(), SourceState::Missing);
    }

    #[test]
    fn a_foreign_version_or_corrupt_file_is_not_read() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("cmp.json");
        std::fs::write(&file, r#"{"version": 99, "tabs": [], "active": 0}"#).unwrap();
        let (s, foreign) = load(&file);
        assert!(foreign && s.tabs.is_empty());
        std::fs::write(&file, "{ not json").unwrap();
        let (s, foreign) = load(&file);
        assert!(!foreign && s == CompareSessions::default());
    }
}
