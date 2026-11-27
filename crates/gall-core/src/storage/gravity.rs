//! Zero-config folder-pair affinity ("Pair Gravity") and structural Git worktree matching (WN-181).

use crate::error::Result;
use crate::nav::PairMode;
use crate::storage::Db;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Exponential recency half-life for learned folder-pair affinity (14 days).
pub const GRAVITY_HALF_LIFE_MS: f64 = 14.0 * 86_400_000.0;
/// Minimum decayed score required to surface a learned ghost tab suggestion.
pub const GRAVITY_THRESHOLD: f64 = 1.5;
/// Maximum stored affinity score to keep decay responsive.
pub const GRAVITY_MAX_SCORE: f64 = 100.0;
/// Confidence score assigned to a structural sibling Git worktree match.
pub const WORKTREE_STRUCTURAL_SCORE: f64 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GravityReason {
    Learned,
    Worktree,
}

/// High-confidence ghost tab partner suggested for the opposite TabBar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GravitySuggestion {
    pub source_path: String,
    pub path: String,
    pub reason: GravityReason,
    pub score: f64,
    pub suggested_mode: PairMode,
    /// Optional hint label (e.g. sibling worktree directory name).
    pub hint: Option<String>,
}

/// Normalizes a path string for consistent gravity storage (trims trailing separators except root).
pub fn normalize_gravity_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut s = trimmed.to_string();
    while s.len() > 1 && (s.ends_with('/') || s.ends_with('\\')) {
        // Keep Windows drive root like `C:\`.
        if s.len() == 3 && s.as_bytes()[1] == b':' {
            break;
        }
        s.pop();
    }
    s
}

/// Orders a pair symmetrically so `(A, B)` and `(B, A)` share one row.
pub fn canonical_pair(a: &str, b: &str) -> Option<(String, String)> {
    let na = normalize_gravity_path(a);
    let nb = normalize_gravity_path(b);
    if na.is_empty() || nb.is_empty() || na == nb {
        return None;
    }
    if na < nb {
        Some((na, nb))
    } else {
        Some((nb, na))
    }
}

/// Applies exponential recency decay to a stored affinity score.
pub fn decayed_score(score: f64, last_used_ms: i64, now_ms: i64) -> f64 {
    if score <= 0.0 {
        return 0.0;
    }
    let elapsed = (now_ms - last_used_ms).max(0) as f64;
    score * 0.5_f64.powf(elapsed / GRAVITY_HALF_LIFE_MS)
}

#[derive(Debug, Clone)]
struct WorktreeInfo {
    current_root: PathBuf,
    siblings: Vec<(PathBuf, Option<String>)>,
}

/// Pure filesystem Git worktree discovery: zero `git` subprocesses, sub-millisecond on disk.
/// Handles both main worktrees (`.git/` directory with `worktrees/*/gitdir`) and linked
/// worktrees (`.git` file containing `gitdir: <common>/worktrees/<name>`).
fn discover_worktree_siblings(path: &Path) -> Option<WorktreeInfo> {
    let mut cur = Some(path);
    while let Some(dir) = cur {
        let dot_git = dir.join(".git");
        let Ok(meta) = std::fs::symlink_metadata(&dot_git) else {
            cur = dir.parent();
            continue;
        };
        if meta.is_dir() {
            // Main worktree: list linked worktrees under `.git/worktrees/*/gitdir`.
            let siblings = read_worktrees_dir(&dot_git.join("worktrees"), dir);
            return Some(WorktreeInfo {
                current_root: dir.to_path_buf(),
                siblings,
            });
        }
        if meta.is_file() {
            let content = std::fs::read_to_string(&dot_git).ok()?;
            let raw = content
                .lines()
                .find_map(|l| l.trim().strip_prefix("gitdir:"))?
                .trim();
            let gitdir = if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                dir.join(raw)
            };
            // `<common>/worktrees/<name>` → common git dir is two parents up.
            let worktrees_dir = gitdir.parent()?;
            if worktrees_dir.file_name().and_then(|n| n.to_str()) != Some("worktrees") {
                return None;
            }
            let common_git = worktrees_dir.parent()?;
            let mut siblings = Vec::new();
            // Prefer the main worktree first when currently inside a linked worktree.
            if let Some(main_root) = common_git.parent() {
                if main_root != dir && main_root.join(".git").is_dir() {
                    let hint = main_root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned());
                    siblings.push((main_root.to_path_buf(), hint));
                }
            }
            siblings.extend(read_worktrees_dir(worktrees_dir, dir));
            return Some(WorktreeInfo {
                current_root: dir.to_path_buf(),
                siblings,
            });
        }
        cur = dir.parent();
    }
    None
}

fn read_worktrees_dir(worktrees_dir: &Path, exclude_root: &Path) -> Vec<(PathBuf, Option<String>)> {
    let Ok(entries) = std::fs::read_dir(worktrees_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let wt_meta_dir = entry.path();
        if !wt_meta_dir.is_dir() {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(wt_meta_dir.join("gitdir")) else {
            continue;
        };
        let gitfile = PathBuf::from(raw.trim());
        let Some(root) = gitfile.parent().map(Path::to_path_buf) else {
            continue;
        };
        if root == exclude_root || !root.is_dir() {
            continue;
        }
        let hint = entry
            .file_name()
            .to_str()
            .map(str::to_string)
            .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()));
        out.push((root, hint));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Computes the single best ghost-tab suggestion for `current_path`, combining learned
/// SQLite Pair Gravity affinity and zero-config structural Git worktree pairing.
///
/// Guarantees:
/// - Never suggests a path already open in `open_opposite_paths` or equal to `current_path`.
/// - Never suggests a missing/non-directory path on disk.
/// - Never suggests a pair dismissed/suppressed by the user.
pub fn suggest_partner(
    db: &Db,
    current_path: &str,
    open_opposite_paths: &[String],
    now_ms: i64,
) -> Result<Option<GravitySuggestion>> {
    let source = normalize_gravity_path(current_path);
    if source.is_empty() {
        return Ok(None);
    }
    let open_set: HashSet<String> = open_opposite_paths
        .iter()
        .map(|p| normalize_gravity_path(p))
        .collect();

    let mut best: Option<GravitySuggestion> = None;

    // 1. Learned SQLite affinity candidates (ordered highest-first after recency decay).
    for (partner, score) in db.gravity_candidates(&source, now_ms, 8)? {
        if partner == source || open_set.contains(&partner) {
            continue;
        }
        if !Path::new(&partner).is_dir() {
            continue;
        }
        best = Some(GravitySuggestion {
            source_path: source.clone(),
            path: partner,
            reason: GravityReason::Learned,
            score,
            suggested_mode: PairMode::Linked,
            hint: None,
        });
        break;
    }

    // 2. Structural Git worktree match (matching relative subfolder in sibling worktrees).
    let cur_p = Path::new(&source);
    if let Some(wt) = discover_worktree_siblings(cur_p) {
        let rel = cur_p.strip_prefix(&wt.current_root).ok();
        for (sib_root, hint) in wt.siblings {
            let candidate_path = match rel {
                Some(r) if !r.as_os_str().is_empty() => {
                    let sub = sib_root.join(r);
                    if sub.is_dir() {
                        sub
                    } else if sib_root.is_dir() {
                        sib_root
                    } else {
                        continue;
                    }
                }
                _ => sib_root,
            };
            let cand_str = normalize_gravity_path(&candidate_path.to_string_lossy());
            if cand_str.is_empty() || cand_str == source || open_set.contains(&cand_str) {
                continue;
            }
            if !Path::new(&cand_str).is_dir() {
                continue;
            }
            if db.is_gravity_suppressed(&source, &cand_str)? {
                continue;
            }
            let wt_suggestion = GravitySuggestion {
                source_path: source.clone(),
                path: cand_str,
                reason: GravityReason::Worktree,
                score: WORKTREE_STRUCTURAL_SCORE,
                suggested_mode: PairMode::Tandem,
                hint,
            };
            if best.as_ref().is_none_or(|b| wt_suggestion.score > b.score) {
                best = Some(wt_suggestion);
            }
            break;
        }
    }

    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gravity_persists_decays_and_respects_suppression() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("src");
        let b = dir.path().join("dest");
        let c = dir.path().join("other");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(&c).unwrap();
        let a_s = a.to_string_lossy().into_owned();
        let b_s = b.to_string_lossy().into_owned();
        let c_s = c.to_string_lossy().into_owned();

        let db = Db::open_in_memory().unwrap();
        let t0 = 1_700_000_000_000_i64;

        // Single weak signal (1.0) stays below threshold (1.5) to avoid noisy ghost tabs.
        db.record_gravity_at(&a_s, &b_s, 1.0, false, t0).unwrap();
        assert!(suggest_partner(&db, &a_s, &[], t0).unwrap().is_none());

        // Second cross-panel signal pushes score to 2.0 >= 1.5 -> surfaces ghost tab!
        db.record_gravity_at(&a_s, &b_s, 1.0, false, t0 + 1000)
            .unwrap();
        let sug = suggest_partner(&db, &a_s, &[], t0 + 1000).unwrap().unwrap();
        assert_eq!(sug.path, b_s);
        assert_eq!(sug.reason, GravityReason::Learned);

        // Already open on opposite side -> never suggested.
        assert!(
            suggest_partner(&db, &a_s, std::slice::from_ref(&b_s), t0 + 1000)
                .unwrap()
                .is_none()
        );

        // After 28 days (2 half-lives), score 2.0 decays to 0.5 (< 1.5) -> no longer suggested.
        let t_old = t0 + 1000 + (28 * 86_400_000);
        assert!(suggest_partner(&db, &a_s, &[], t_old).unwrap().is_none());

        // Missing folder on disk -> never suggested.
        db.record_gravity_at(&a_s, &c_s, 3.0, false, t0).unwrap();
        std::fs::remove_dir(&c).unwrap();
        let sug2 = suggest_partner(&db, &a_s, &[], t0 + 1000).unwrap().unwrap();
        assert_eq!(sug2.path, b_s);

        // Dismissing suppresses it persistently, and manual pair (unsuppress=true) re-enables it.
        db.suppress_gravity(&a_s, &b_s).unwrap();
        assert!(suggest_partner(&db, &a_s, &[], t0 + 2000)
            .unwrap()
            .is_none());
        db.record_gravity_at(&a_s, &b_s, 2.5, true, t0 + 3000)
            .unwrap();
        assert_eq!(
            suggest_partner(&db, &a_s, &[], t0 + 3000)
                .unwrap()
                .unwrap()
                .path,
            b_s
        );
    }

    #[test]
    fn structural_git_worktree_suggests_matching_relative_subfolder_zero_config() {
        let dir = tempfile::tempdir().unwrap();
        let main_wt = dir.path().join("repo-main");
        let feat_wt = dir.path().join("repo-feat");
        let main_sub = main_wt.join("src").join("ui");
        let feat_sub = feat_wt.join("src").join("ui");
        std::fs::create_dir_all(&main_sub).unwrap();
        std::fs::create_dir_all(&feat_sub).unwrap();

        // Simulate a linked worktree `.git` structure cleanly (no git binary required).
        let common_wt = main_wt.join(".git").join("worktrees").join("repo-feat");
        std::fs::create_dir_all(&common_wt).unwrap();
        std::fs::write(
            common_wt.join("gitdir"),
            format!("{}\n", feat_wt.join(".git").display()),
        )
        .unwrap();
        std::fs::write(
            feat_wt.join(".git"),
            format!("gitdir: {}\n", common_wt.display()),
        )
        .unwrap();

        let db = Db::open_in_memory().unwrap();
        let now = 1_700_000_000_000_i64;

        // From linked worktree subfolder -> suggests matching main worktree subfolder in Tandem!
        let sug_from_feat = suggest_partner(&db, &feat_sub.to_string_lossy(), &[], now)
            .unwrap()
            .expect("main worktree match");
        assert_eq!(sug_from_feat.path, main_sub.to_string_lossy());
        assert_eq!(sug_from_feat.reason, GravityReason::Worktree);
        assert_eq!(sug_from_feat.suggested_mode, PairMode::Tandem);

        // From main worktree subfolder -> suggests linked worktree subfolder!
        let sug_from_main = suggest_partner(&db, &main_sub.to_string_lossy(), &[], now)
            .unwrap()
            .expect("linked worktree match");
        assert_eq!(sug_from_main.path, feat_sub.to_string_lossy());
        assert_eq!(sug_from_main.reason, GravityReason::Worktree);

        // Dismissing a structural worktree suggestion suppresses it cleanly.
        db.suppress_gravity(&feat_sub.to_string_lossy(), &main_sub.to_string_lossy())
            .unwrap();
        assert!(suggest_partner(&db, &feat_sub.to_string_lossy(), &[], now)
            .unwrap()
            .is_none());
    }
}
