//! Folder comparison: walks two trees and classifies every relative path as
//! same / different / left-only / right-only. Quick mode trusts size + mtime;
//! content mode compares bytes of equally sized files. Folders take the status
//! of their contents, so the UI can collapse identical subtrees.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering as CmpOrdering;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, UNIX_EPOCH};

use crate::{CoreError, Result};

/// Stop after this many paths per side (keeps the UI responsive on huge trees).
pub const MAX_ENTRIES: usize = 300_000;
/// mtimes within this many seconds count as equal (FAT, network shares, copies).
const MTIME_SLACK: i64 = 2;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DirCmpOptions {
    /// Compare file bytes when sizes match (slower, exact).
    pub content: bool,
    pub hidden: bool,
    pub respect_gitignore: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub is_dir: bool,
    pub size: u64,
    /// Seconds since the epoch.
    pub mtime: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Same,
    Different,
    LeftOnly,
    RightOnly,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Relative path with "/" separators.
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub status: Status,
    /// Why two files differ: "content" | "size" | "date" | "type".
    pub reason: Option<&'static str>,
    /// Which side has the newer modification time: "left" | "right".
    pub newer: Option<&'static str>,
    pub left: Option<Meta>,
    pub right: Option<Meta>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub same: usize,
    pub different: usize,
    pub left_only: usize,
    pub right_only: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirCmpResult {
    pub left: PathBuf,
    pub right: PathBuf,
    pub items: Vec<Item>,
    /// File counts (folders are not counted).
    pub counts: Counts,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

fn meta_of(m: &std::fs::Metadata) -> Meta {
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Meta {
        is_dir: m.is_dir(),
        size: if m.is_dir() { 0 } else { m.len() },
        mtime,
    }
}

/// rel path → metadata for everything below `root`.
fn walk(
    root: &Path,
    opts: DirCmpOptions,
    cancel: &AtomicBool,
    truncated: &mut bool,
) -> Result<BTreeMap<String, Meta>> {
    if !root.is_dir() {
        return Err(CoreError::Other(format!(
            "{} is not a folder",
            root.display()
        )));
    }
    let mut out = BTreeMap::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(!opts.hidden)
        .git_ignore(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .git_global(false)
        .ignore(opts.respect_gitignore)
        .parents(opts.respect_gitignore)
        .follow_links(false)
        .build();
    for e in walker.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let Ok(rel) = e.path().strip_prefix(root) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        if out.len() >= MAX_ENTRIES {
            *truncated = true;
            break;
        }
        let Ok(m) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        out.insert(rel.to_string_lossy().replace('\\', "/"), meta_of(&m));
    }
    Ok(out)
}

/// Tree order: a folder's children come right after it, siblings case-insensitively.
fn tree_cmp(a: &str, b: &str) -> CmpOrdering {
    let (mut x, mut y) = (a.split('/'), b.split('/'));
    loop {
        match (x.next(), y.next()) {
            (Some(p), Some(q)) => match p
                .to_lowercase()
                .cmp(&q.to_lowercase())
                .then_with(|| p.cmp(q))
            {
                CmpOrdering::Equal => continue,
                o => return o,
            },
            (None, Some(_)) => return CmpOrdering::Less,
            (Some(_), None) => return CmpOrdering::Greater,
            (None, None) => return CmpOrdering::Equal,
        }
    }
}

fn same_bytes(a: &Path, b: &Path, cancel: &AtomicBool) -> bool {
    let (Ok(fa), Ok(fb)) = (super::open_regular(a), super::open_regular(b)) else {
        return false;
    };
    let (mut ra, mut rb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let (Ok(na), Ok(nb)) = (fill(&mut ra, &mut ba), fill(&mut rb, &mut bb)) else {
            return false;
        };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

fn fill(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = r.read(&mut buf[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

pub fn compare_dirs(
    left: &Path,
    right: &Path,
    opts: DirCmpOptions,
    cancel: &AtomicBool,
) -> Result<DirCmpResult> {
    let t0 = Instant::now();
    if left == right {
        return Err(CoreError::Other("both sides are the same folder".into()));
    }
    let mut truncated = false;
    let l = walk(left, opts, cancel, &mut truncated)?;
    let r = walk(right, opts, cancel, &mut truncated)?;
    let mut keys: Vec<&String> = l
        .keys()
        .chain(r.keys().filter(|k| !l.contains_key(*k)))
        .collect();
    keys.sort_by(|a, b| tree_cmp(a, b));

    let mut items: Vec<Item> = Vec::with_capacity(keys.len());
    let mut counts = Counts::default();
    for k in keys {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let (lm, rm) = (l.get(k).copied(), r.get(k).copied());
        let is_dir = lm.map(|m| m.is_dir).unwrap_or(false) || rm.map(|m| m.is_dir).unwrap_or(false);
        let newer = match (lm, rm) {
            (Some(a), Some(b)) if (a.mtime - b.mtime).abs() > MTIME_SLACK => {
                Some(if a.mtime > b.mtime { "left" } else { "right" })
            }
            _ => None,
        };
        let (status, reason) = match (lm, rm) {
            (Some(_), None) => (Status::LeftOnly, None),
            (None, Some(_)) => (Status::RightOnly, None),
            (Some(a), Some(b)) if a.is_dir != b.is_dir => (Status::Different, Some("type")),
            (Some(a), Some(_)) if a.is_dir => (Status::Same, None), // resolved from children below
            (Some(a), Some(b)) if a.size != b.size => (Status::Different, Some("size")),
            (Some(_), Some(_)) if opts.content => {
                if same_bytes(&left.join(k), &right.join(k), cancel) {
                    (Status::Same, None)
                } else {
                    (Status::Different, Some("content"))
                }
            }
            (Some(_), Some(_)) if newer.is_some() => (Status::Different, Some("date")),
            _ => (Status::Same, None),
        };
        if !is_dir {
            match status {
                Status::Same => counts.same += 1,
                Status::Different => counts.different += 1,
                Status::LeftOnly => counts.left_only += 1,
                Status::RightOnly => counts.right_only += 1,
            }
        }
        items.push(Item {
            rel: k.clone(),
            name: k.rsplit('/').next().unwrap_or(k).to_string(),
            depth: k.matches('/').count(),
            is_dir,
            status,
            reason,
            newer: if status == Status::Different || is_dir {
                newer
            } else {
                None
            },
            left: lm,
            right: rm,
        });
    }

    // A folder present on both sides is "different" if anything inside it is.
    for i in (0..items.len()).rev() {
        if !(items[i].is_dir && items[i].status == Status::Same) {
            continue;
        }
        let prefix = format!("{}/", items[i].rel);
        let changed = items[i + 1..]
            .iter()
            .take_while(|c| c.rel.starts_with(&prefix))
            .any(|c| c.status != Status::Same);
        if changed {
            items[i].status = Status::Different;
            items[i].reason = Some("content");
        }
    }

    Ok(DirCmpResult {
        left: left.to_path_buf(),
        right: right.to_path_buf(),
        items,
        counts,
        truncated,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ops::{apply_sync, SyncOp, UndoJournal};
    use std::fs;

    fn setup() -> (crate::fs::ops::test_support::TestDir, PathBuf, PathBuf) {
        let t = crate::fs::ops::test_support::test_dir();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        for d in [&l, &r] {
            fs::create_dir_all(d.join("sub/deep")).unwrap();
            fs::write(d.join("same.txt"), "same").unwrap();
            fs::write(d.join("sub/deep/also-same.txt"), "x").unwrap();
        }
        fs::write(l.join("changed.txt"), "left version").unwrap();
        fs::write(r.join("changed.txt"), "right v").unwrap();
        fs::write(l.join("samesize.txt"), "aaaa").unwrap();
        fs::write(r.join("samesize.txt"), "bbbb").unwrap();
        fs::write(l.join("only-left.txt"), "L").unwrap();
        fs::create_dir_all(r.join("only-right-dir")).unwrap();
        fs::write(r.join("only-right-dir/f.txt"), "R").unwrap();
        fs::write(l.join("sub/new.txt"), "new").unwrap();
        // equal mtimes so quick mode cannot see the same-size change
        let t0 = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        for p in [l.join("samesize.txt"), r.join("samesize.txt")] {
            fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(t0)
                .unwrap();
        }
        (t, l, r)
    }

    fn st<'a>(r: &'a DirCmpResult, rel: &str) -> &'a Item {
        r.items
            .iter()
            .find(|i| i.rel == rel)
            .unwrap_or_else(|| panic!("{rel} missing"))
    }

    #[test]
    fn classifies_and_orders_as_a_tree() {
        let (_t, l, r) = setup();
        let res = compare_dirs(&l, &r, DirCmpOptions::default(), &AtomicBool::new(false)).unwrap();
        assert_eq!(st(&res, "same.txt").status, Status::Same);
        assert_eq!(st(&res, "changed.txt").status, Status::Different);
        assert_eq!(st(&res, "changed.txt").reason, Some("size"));
        assert_eq!(st(&res, "only-left.txt").status, Status::LeftOnly);
        assert_eq!(st(&res, "only-right-dir").status, Status::RightOnly);
        assert_eq!(st(&res, "only-right-dir/f.txt").status, Status::RightOnly);
        assert_eq!(
            st(&res, "sub").status,
            Status::Different,
            "folder with a new file inside"
        );
        assert_eq!(st(&res, "sub/deep").status, Status::Same);
        assert_eq!(
            st(&res, "samesize.txt").status,
            Status::Same,
            "quick mode: same size + mtime"
        );
        // children follow their parent
        let pos = |rel: &str| res.items.iter().position(|i| i.rel == rel).unwrap();
        assert!(pos("sub") < pos("sub/deep") && pos("sub/deep") < pos("sub/new.txt"));
        assert_eq!(res.counts.left_only, 2);
        assert_eq!(res.counts.right_only, 1);
    }

    #[test]
    fn content_mode_sees_same_size_changes() {
        let (_t, l, r) = setup();
        let o = DirCmpOptions {
            content: true,
            ..Default::default()
        };
        let res = compare_dirs(&l, &r, o, &AtomicBool::new(false)).unwrap();
        assert_eq!(st(&res, "samesize.txt").status, Status::Different);
        assert_eq!(st(&res, "samesize.txt").reason, Some("content"));
    }

    #[test]
    fn tree_order() {
        let mut v = vec!["a.txt", "a/x", "B", "a", "b/c"];
        v.sort_by(|a, b| tree_cmp(a, b));
        assert_eq!(v, ["a", "a/x", "a.txt", "B", "b/c"]);
    }

    #[test]
    fn mirror_makes_folders_identical() {
        let (_t, l, r) = setup();
        let j = UndoJournal::default();
        let before =
            compare_dirs(&l, &r, DirCmpOptions::default(), &AtomicBool::new(false)).unwrap();
        let ops: Vec<SyncOp> = before
            .items
            .iter()
            .filter(|i| !i.is_dir || i.status != Status::Different)
            .filter_map(|i| match i.status {
                Status::LeftOnly | Status::Different => Some(SyncOp::Copy {
                    src: l.join(&i.rel),
                    dest: r.join(&i.rel),
                }),
                Status::RightOnly => Some(SyncOp::Trash {
                    path: r.join(&i.rel),
                }),
                Status::Same => None,
            })
            .collect();
        // keep top-most only
        let mut top: Vec<SyncOp> = Vec::new();
        for op in ops {
            let p = match &op {
                SyncOp::Copy { dest, .. } => dest.clone(),
                SyncOp::Trash { path } => path.clone(),
            };
            if !top.iter().any(|t| match t {
                SyncOp::Copy { dest, .. } => p.starts_with(dest) && p != *dest,
                SyncOp::Trash { path } => p.starts_with(path) && p != *path,
            }) {
                top.push(op);
            }
        }
        apply_sync(&top, &j, &AtomicBool::new(false)).unwrap();
        let after =
            compare_dirs(&l, &r, DirCmpOptions::default(), &AtomicBool::new(false)).unwrap();
        let left: Vec<_> = after
            .items
            .iter()
            .filter(|i| i.status != Status::Same)
            .map(|i| {
                format!(
                    "{} {:?} {:?} {:?}/{:?}",
                    i.rel,
                    i.status,
                    i.reason,
                    i.left.map(|m| m.mtime),
                    i.right.map(|m| m.mtime)
                )
            })
            .collect();
        assert!(left.is_empty(), "still different after mirror: {left:?}");
    }

    #[test]
    fn sync_mirror_then_undo() {
        let (_t, l, r) = setup();
        let j = UndoJournal::default();
        let ops = vec![
            SyncOp::Copy {
                src: l.join("changed.txt"),
                dest: r.join("changed.txt"),
            },
            SyncOp::Copy {
                src: l.join("sub/new.txt"),
                dest: r.join("sub/new.txt"),
            },
            SyncOp::Copy {
                src: l.join("only-left.txt"),
                dest: r.join("x/y/only-left.txt"),
            },
            SyncOp::Trash {
                path: r.join("only-right-dir"),
            },
        ];
        let rep = apply_sync(&ops, &j, &AtomicBool::new(false)).unwrap();
        assert_eq!((rep.copied, rep.replaced, rep.trashed), (3, 1, 1));
        assert_eq!(
            fs::read_to_string(r.join("changed.txt")).unwrap(),
            "left version"
        );
        assert!(r.join("x/y/only-left.txt").exists());
        assert!(!r.join("only-right-dir").exists());
        assert_eq!(j.peek_description().as_deref(), Some("sync of 4 items"));
        // undo restores the replaced file, removes the copies and created folders
        match j.undo() {
            Ok(_) => {
                assert_eq!(
                    fs::read_to_string(r.join("changed.txt")).unwrap(),
                    "right v"
                );
                assert!(!r.join("sub/new.txt").exists());
                assert!(!r.join("x").exists());
                assert!(r.join("only-right-dir/f.txt").exists());
            }
            Err(e) => eprintln!("undo not verified (no trash here): {e}"),
        }
    }
}
