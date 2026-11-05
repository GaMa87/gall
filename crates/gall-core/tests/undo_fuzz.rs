//! Randomized undo test: random copies, moves (replace, merge, keep both, skip, replace
//! older), renames, batch renames, duplicates and deletes on a random tree, then undo
//! everything; the tree must be back byte for byte, and every single undo must restore
//! the state from before its operation.
//!
//! Uses the real trash, so it is not run by default:
//! `cargo test -p gall-core --test undo_fuzz -- --ignored --nocapture`
//! (`GALL_FUZZ_SEEDS=400 GALL_FUZZ_STEPS=60` for a longer run). Its trash items are
//! purged at the end.
// Restoring from the trash (and so undoing a delete) is not available on macOS.
#![cfg(any(windows, all(unix, not(target_os = "macos"))))]

use gall_core::fs::ops::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};

type Tree = BTreeMap<String, Option<Vec<u8>>>;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<T: Clone>(&mut self, v: &[T]) -> T {
        v[(self.next() % v.len() as u64) as usize].clone()
    }
    fn one_in(&mut self, n: u64) -> bool {
        self.next().is_multiple_of(n)
    }
}

fn snapshot(root: &Path) -> Tree {
    walkdir::WalkDir::new(root)
        .min_depth(1)
        .into_iter()
        .flatten()
        .map(|e| {
            let rel = e
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let data = if e.file_type().is_symlink() {
                let t = std::fs::read_link(e.path()).unwrap();
                Some(format!("-> {}", t.display()).into_bytes())
            } else {
                e.file_type()
                    .is_file()
                    .then(|| std::fs::read(e.path()).unwrap())
            };
            (rel, data)
        })
        .collect()
}

fn items(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut files = vec![];
    let mut dirs = vec![root.to_path_buf()];
    for e in walkdir::WalkDir::new(root)
        .min_depth(1)
        .into_iter()
        .flatten()
    {
        if e.file_type().is_dir() {
            dirs.push(e.path().to_path_buf());
        } else {
            files.push(e.path().to_path_buf());
        }
    }
    (files, dirs)
}

/// [`snapshot`] of several roots, told apart by their index.
fn snapshot_all(roots: &[PathBuf]) -> Tree {
    roots
        .iter()
        .enumerate()
        .flat_map(|(i, r)| {
            snapshot(r)
                .into_iter()
                .map(move |(k, v)| (format!("{i}/{k}"), v))
        })
        .collect()
}

/// [`items`] of several roots; the roots come first in the folders.
fn items_all(roots: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut files, mut dirs) = (vec![], roots.to_vec());
    for r in roots {
        let (f, d) = items(r);
        files.extend(f);
        dirs.extend(d.into_iter().skip(1));
    }
    (files, dirs)
}

/// A folder on another file system than `near` (/dev/shm), so that moves between the
/// two copy and delete instead of renaming. None where there is none.
fn other_file_system(near: &Path) -> Option<tempfile::TempDir> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let shm = Path::new("/dev/shm");
        let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
        if dev(shm).is_some() && dev(shm) != dev(near) {
            return tempfile::tempdir_in(shm).ok();
        }
    }
    let _ = near;
    None
}

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[test]
#[ignore = "uses the real trash; run with --ignored"]
fn random_operations_undo_completely() {
    let (seeds, steps) = (
        env("GALL_FUZZ_SEEDS", 60),
        env("GALL_FUZZ_STEPS", 40) as usize,
    );
    let base = tempfile::tempdir().unwrap();
    let other = other_file_system(base.path());
    let mut failures = vec![];
    for seed in 1..=seeds {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let root = base.path().join(format!("s{seed}"));
        std::fs::create_dir_all(&root).unwrap();
        let mut roots = vec![root.clone()];
        if let Some(o) = &other {
            roots.push(o.path().join(format!("s{seed}")));
            std::fs::create_dir_all(&roots[1]).unwrap();
        }
        let names = ["a", "b", "c", "Photos", "x.txt", "y.txt", "A.TXT", "z"];
        for _ in 0..25 {
            let (_, dirs) = items_all(&roots);
            let p = r.pick(&dirs).join(r.pick(&names));
            if p.symlink_metadata().is_ok() {
                continue;
            }
            if r.one_in(3) {
                std::fs::create_dir(&p).unwrap();
            } else if cfg!(unix) && r.one_in(4) {
                // Symlinks to a file, a folder or nothing (relative, as in real trees).
                #[cfg(unix)]
                std::os::unix::fs::symlink(r.pick(&names), &p).unwrap();
            } else {
                std::fs::write(&p, format!("{seed}-{}", r.next() % 1000)).unwrap();
            }
        }
        let start = snapshot_all(&roots);
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let journal = Arc::new(UndoJournal::default());
        let tasks = Arc::new(TaskManager::new(
            journal.clone(),
            Arc::new(move |e| {
                let _ = tx.lock().unwrap().send(format!("{e:?}"));
            }),
        ));
        let run = |req: serde_json::Value| -> bool {
            let req: TaskRequest = serde_json::from_value(req).unwrap();
            if tasks.start(req).is_err() {
                return false;
            }
            loop {
                let e = rx.recv().unwrap();
                if !e.starts_with("Progress") {
                    return e.starts_with("Done");
                }
            }
        };
        let mut log = vec![];
        // Journal length before each operation, with the tree at that point.
        let mut checkpoints: Vec<(usize, Tree, usize)> = vec![];
        for _ in 0..steps {
            let (files, dirs) = items_all(&roots);
            let all: Vec<PathBuf> = files
                .iter()
                .chain(dirs.iter().skip(roots.len()))
                .cloned()
                .collect();
            if all.is_empty() {
                break;
            }
            checkpoints.push((journal.len(), snapshot_all(&roots), log.len()));
            match r.next() % 6 {
                op @ (0 | 1) => {
                    let (src, dest) = (r.pick(&all), r.pick(&dirs));
                    let kind = if op == 0 { "copy" } else { "move" };
                    let conflict = r.pick(&["keepBoth", "overwrite", "skip", "overwriteOlder"]);
                    let merge = r.one_in(2);
                    log.push(format!(
                        "{kind} {src:?} -> {dest:?} {conflict} merge={merge}"
                    ));
                    run(
                        serde_json::json!({"kind": kind, "sources": [src], "dest": dest, "conflict": conflict, "merge": merge}),
                    );
                }
                2 => {
                    let (p, n) = (r.pick(&all), r.pick(&names));
                    log.push(format!("rename {p:?} {n}"));
                    let _ = rename(&p, n, &journal);
                }
                3 => {
                    let d = r.pick(&dirs);
                    let kids: Vec<PathBuf> = std::fs::read_dir(&d)
                        .unwrap()
                        .flatten()
                        .map(|e| e.path())
                        .collect();
                    if kids.len() >= 2 {
                        let n = kids.len();
                        let batch: Vec<(PathBuf, String)> = (0..n)
                            .map(|i| {
                                (
                                    kids[i].clone(),
                                    kids[(i + 1) % n]
                                        .file_name()
                                        .unwrap()
                                        .to_string_lossy()
                                        .into_owned(),
                                )
                            })
                            .collect();
                        log.push(format!("rotate names in {d:?}"));
                        let _ = batch_rename(&batch, &journal);
                    }
                }
                4 => {
                    let p = r.pick(&all);
                    log.push(format!("duplicate {p:?}"));
                    run(serde_json::json!({"kind": "duplicate", "sources": [p]}));
                }
                _ => {
                    let p = r.pick(&all);
                    log.push(format!("trash {p:?}"));
                    let _ = trash_items(&[p], &journal);
                }
            }
        }
        let mut problem = None;
        while !journal.is_empty() {
            let desc = journal.peek_description().unwrap();
            if let Err(e) = journal.undo() {
                problem = Some(format!("undo of '{desc}' failed: {e}"));
                break;
            }
            let n = journal.len();
            if let Some((_, tree, op)) = checkpoints.iter().rev().find(|(l, _, _)| *l == n) {
                if snapshot_all(&roots) != *tree {
                    problem = Some(format!(
                        "undo of '{desc}' did not restore the state before: {}",
                        log[*op]
                    ));
                    break;
                }
            }
        }
        if problem.is_none() && snapshot_all(&roots) != start {
            problem = Some("the tree differs after undoing everything".into());
        }
        if let Some(p) = problem {
            failures.push(format!("seed {seed}: {p}\n  {}", log.join("\n  ")));
        }
    }
    // Purge what this test put in the trash.
    let mine: Vec<_> = trash::os_limited::list()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| {
            let p = i.original_path();
            p.starts_with(base.path()) || other.as_ref().is_some_and(|o| p.starts_with(o.path()))
        })
        .collect();
    let _ = trash::os_limited::purge_all(mine);
    assert!(
        failures.is_empty(),
        "{} of {seeds} seeds failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
