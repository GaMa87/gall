//! The line-level core of the text comparison: changed line ranges (histogram or Myers diff with the
//! indent heuristic, as Git does) and moved blocks. Pure functions on lines, so the desktop and a
//! browser worker give the same answer.

use serde::Serialize;

/// `len` lines removed at left `a` and added at right `b`: moved, not changed.
/// An `edited` move was changed on the way: it spans `len` lines at the left and
/// `b_len` at the right, anchored by lines that are unique in both files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Move {
    /// Shared by both ends: 1, 2, … in the order of the left start. The same inputs and
    /// options always give the same ids.
    pub id: usize,
    pub a: usize,
    pub b: usize,
    pub len: usize,
    pub b_len: usize,
    pub edited: bool,
}

/// Fewest lines in a moved block.
const MOVE_MIN_LINES: usize = 3;
/// Fewest non-blank characters in a moved block (three lines of "}" are no move).
const MOVE_MIN_CHARS: usize = 20;
/// Candidate positions tried per line (repeated lines would make this quadratic).
const MOVE_MAX_CANDIDATES: usize = 64;

/// Blocks of removed lines that reappear, in the same order, among the added
/// lines (compared without leading/trailing whitespace, so re-indented code
/// still counts). Greedy, longest match first at each removed line.
pub fn find_moves(
    na: &[String],
    nb: &[String],
    hunks: &[(usize, usize, usize, usize)],
) -> Vec<Move> {
    let (mut del, mut ins) = (vec![false; na.len()], vec![false; nb.len()]);
    for &(a0, a1, b0, b1) in hunks {
        del[a0..a1].fill(true);
        ins[b0..b1].fill(true);
    }
    let key = |s: &str| s.trim().to_string();
    let mut at: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (j, l) in nb.iter().enumerate() {
        if ins[j] {
            let v = at.entry(key(l)).or_default();
            if v.len() < MOVE_MAX_CANDIDATES {
                v.push(j);
            }
        }
    }
    let mut used = vec![false; nb.len()];
    let mut moves = Vec::new();
    let mut i = 0;
    while i < na.len() {
        let k = key(&na[i]);
        if !del[i] || k.chars().filter(|c| !c.is_whitespace()).count() < 2 {
            i += 1;
            continue;
        }
        let mut best = (0usize, 0usize); // (len, b)
        for &j in at.get(&k).map(|v| v.as_slice()).unwrap_or(&[]) {
            let mut n = 0;
            while i + n < na.len()
                && j + n < nb.len()
                && del[i + n]
                && ins[j + n]
                && !used[j + n]
                && na[i + n].trim() == nb[j + n].trim()
            {
                n += 1;
            }
            if n > best.0 {
                best = (n, j);
            }
        }
        let (n, j) = best;
        let chars: usize = na[i..i + n]
            .iter()
            .map(|l| l.chars().filter(|c| !c.is_whitespace()).count())
            .sum();
        if n >= MOVE_MIN_LINES && chars >= MOVE_MIN_CHARS {
            used[j..j + n].fill(true);
            moves.push(Move {
                id: 0,
                a: i,
                b: j,
                len: n,
                b_len: n,
                edited: false,
            });
            i += n;
        } else {
            i += 1;
        }
    }
    moves
}

/// Fewest non-blank characters of an anchor line of an edited move.
const ANCHOR_MIN_CHARS: usize = 12;
/// Most lines between two anchors of one edited move, per side (the edits in between).
const ANCHOR_MAX_GAP: usize = 4;
/// Fewest anchors of an edited move: one shared line proves nothing.
const ANCHOR_MIN_COUNT: usize = 2;

/// How often each trimmed line occurs in the whole file.
fn line_counts(lines: &[String]) -> std::collections::HashMap<&str, usize> {
    let mut c: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for l in lines {
        *c.entry(l.trim()).or_default() += 1;
    }
    c
}

/// Moved blocks that were edited on the way. Anchors are lines that occur exactly once in
/// each whole file (so repeated boilerplate never anchors), are removed on the left and
/// added on the right, and are not part of an exact move. Consecutive anchors in the same
/// order, with only a few removed/added lines between them, form one block; it is a move
/// only when its two ends lie in different hunks (in one hunk it is an edit in place) and
/// it has enough evidence. Anything else stays removed and added.
pub fn find_edited_moves(
    na: &[String],
    nb: &[String],
    hunks: &[(usize, usize, usize, usize)],
) -> Vec<Move> {
    let (mut free_a, mut free_b) = (vec![false; na.len()], vec![false; nb.len()]);
    let (mut hunk_a, mut hunk_b) = (vec![usize::MAX; na.len()], vec![usize::MAX; nb.len()]);
    for (h, &(a0, a1, b0, b1)) in hunks.iter().enumerate() {
        free_a[a0..a1].fill(true);
        free_b[b0..b1].fill(true);
        hunk_a[a0..a1].fill(h);
        hunk_b[b0..b1].fill(h);
    }
    let (ca, cb) = (line_counts(na), line_counts(nb));
    let mut right: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (j, l) in nb.iter().enumerate() {
        if free_b[j] && cb.get(l.trim()) == Some(&1) {
            right.insert(l.trim(), j);
        }
    }
    let chars = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
    // Anchors in left order: (left line, right line).
    let anchors: Vec<(usize, usize)> = (0..na.len())
        .filter(|&i| {
            free_a[i] && ca.get(na[i].trim()) == Some(&1) && chars(&na[i]) >= ANCHOR_MIN_CHARS
        })
        .filter_map(|i| right.get(na[i].trim()).map(|&j| (i, j)))
        .collect();
    let mut out = Vec::new();
    let mut taken_a: Vec<(usize, usize)> = Vec::new();
    let mut taken_b: Vec<(usize, usize)> = Vec::new();
    let mut flush = |run: &[(usize, usize)]| {
        let (Some(&(mut a0, mut b0)), Some(&(a1, b1))) = (run.first(), run.last()) else {
            return;
        };
        let (mut a1, mut b1) = (a1, b1);
        let evidence: usize = run.iter().map(|&(i, _)| chars(&na[i])).sum();
        // The edits sit next to the anchors: take up to a few removed and added lines
        // beyond the first and last anchor, equally on both sides.
        for _ in 0..ANCHOR_MAX_GAP {
            if a0 > 0 && b0 > 0 && free_a[a0 - 1] && free_b[b0 - 1] {
                a0 -= 1;
                b0 -= 1;
            } else {
                break;
            }
        }
        for _ in 0..ANCHOR_MAX_GAP {
            if a1 + 1 < na.len() && b1 + 1 < nb.len() && free_a[a1 + 1] && free_b[b1 + 1] {
                a1 += 1;
                b1 += 1;
            } else {
                break;
            }
        }
        let (len, b_len) = (a1 - a0 + 1, b1 - b0 + 1);
        let edited = len != b_len || (0..len).any(|k| na[a0 + k].trim() != nb[b0 + k].trim());
        if run.len() >= ANCHOR_MIN_COUNT
            && edited
            && len >= MOVE_MIN_LINES
            && b_len >= MOVE_MIN_LINES
            && evidence >= MOVE_MIN_CHARS
            && hunk_a[a0] != hunk_b[b0]
            && taken_a.iter().all(|&(s, e)| a1 < s || e < a0)
            && taken_b.iter().all(|&(s, e)| b1 < s || e < b0)
        {
            taken_a.push((a0, a1));
            taken_b.push((b0, b1));
            out.push(Move {
                id: 0,
                a: a0,
                b: b0,
                len,
                b_len,
                edited: true,
            });
        }
    };
    let mut run: Vec<(usize, usize)> = Vec::new();
    for &(i, j) in &anchors {
        let joins = run.last().is_some_and(|&(pi, pj)| {
            i > pi
                && j > pj
                && i - pi - 1 <= ANCHOR_MAX_GAP
                && j - pj - 1 <= ANCHOR_MAX_GAP
                && free_a[pi..=i].iter().all(|&f| f)
                && free_b[pj..=j].iter().all(|&f| f)
        });
        if !joins {
            flush(&run);
            run.clear();
        }
        run.push((i, j));
    }
    flush(&run);
    out
}

/// Changed line ranges `(a0, a1, b0, b1)`. Histogram diff (as git uses) with the
/// indent heuristic, so ambiguous blocks start and end where a reader expects
/// (a whole function, not "}\n\nfn x() {"); `minimal` gives the fewest changed lines.
pub fn line_hunks(
    na: &[String],
    nb: &[String],
    minimal: bool,
) -> Vec<(usize, usize, usize, usize)> {
    use imara_diff::{Algorithm, Diff, InternedInput, Interner};
    let mut interner = Interner::new(na.len() + nb.len());
    let before = na.iter().map(|s| interner.intern(s.as_str())).collect();
    let after = nb.iter().map(|s| interner.intern(s.as_str())).collect();
    let input = InternedInput {
        before,
        after,
        interner,
    };
    let alg = if minimal {
        Algorithm::MyersMinimal
    } else {
        Algorithm::Histogram
    };
    let mut diff = Diff::compute(alg, &input);
    diff.postprocess_lines(&input);
    diff.hunks()
        .map(|h| {
            (
                h.before.start as usize,
                h.before.end as usize,
                h.after.start as usize,
                h.after.end as usize,
            )
        })
        .collect()
}
