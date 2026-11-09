//! File comparison: line diff with intra-line (word) highlights for text, a
//! byte-level verdict for binary files, and unified patches. Lines are compared
//! with `imara-diff` (histogram + indent heuristic, as git does); changed lines
//! are paired by similarity and words compared with `similar`. Sources can be
//! files, the clipboard text or a file's version in Git HEAD.

use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag, TextDiff};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::IoContext;
use crate::{CoreError, Result};

/// Largest side compared as text; bigger files get the binary verdict only.
pub const MAX_TEXT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DiffSource {
    File {
        path: PathBuf,
    },
    /// The file as committed in Git HEAD.
    GitHead {
        path: PathBuf,
    },
    /// The file at a specific commit, identified by its full object ID.
    GitCommit {
        path: PathBuf,
        commit: String,
    },
    /// Text supplied by the caller (e.g. the clipboard, read by the app).
    Text {
        label: String,
        text: String,
    },
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiffOptions {
    pub ignore_whitespace: bool,
    pub ignore_case: bool,
    /// Treat CRLF and LF as equal (on by default in the UI).
    pub ignore_eol: bool,
    /// Minimal diff (Myers): the fewest changed lines. Off: histogram diff,
    /// which reads better.
    pub minimal: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    pub label: String,
    pub path: Option<PathBuf>,
    pub size: u64,
    pub crlf: bool,
    pub final_newline: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Op {
    /// "equal" | "delete" | "insert" | "replace"
    pub tag: &'static str,
    pub a: usize,
    pub a_len: usize,
    pub b: usize,
    pub b_len: usize,
}

/// Half-open UTF-16 ranges.
pub type Spans = Vec<(usize, usize)>;

/// Changed character ranges (UTF-16 offsets, as JavaScript indexes strings)
/// for one paired line of a "replace" block.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinePair {
    pub a: usize,
    pub b: usize,
    pub a_spans: Spans,
    pub b_spans: Spans,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDiffResult {
    pub left_lines: Vec<String>,
    pub right_lines: Vec<String>,
    pub ops: Vec<Op>,
    pub pairs: Vec<LinePair>,
    pub added: usize,
    pub removed: usize,
    /// 0..1 similarity of the two sides.
    pub ratio: f32,
    /// Removed lines that reappear unchanged (indentation aside) among the added lines.
    pub moves: Vec<Move>,
}

/// `len` lines removed at left `a` and added at right `b`: moved, not changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Move {
    pub a: usize,
    pub b: usize,
    pub len: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinaryDiffResult {
    pub identical: bool,
    /// Offset of the first differing byte (None when identical or one is a prefix up to its end).
    pub first_difference: Option<u64>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    pub left: Side,
    pub right: Side,
    pub text: Option<TextDiffResult>,
    pub binary: Option<BinaryDiffResult>,
}

struct Loaded {
    side: Side,
    bytes: Vec<u8>,
    /// Too large to load fully: compared by streaming (files only).
    stream: Option<PathBuf>,
}

fn label_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

fn load(src: &DiffSource) -> Result<Loaded> {
    match src {
        DiffSource::File { path } => {
            let meta = std::fs::metadata(path).ctx("read", path)?;
            if meta.is_dir() {
                return Err(CoreError::Other(format!("{} is a folder", path.display())));
            }
            let size = meta.len();
            let side = Side {
                label: label_of(path),
                path: Some(path.clone()),
                size,
                crlf: false,
                final_newline: false,
            };
            if size > MAX_TEXT_BYTES {
                return Ok(Loaded {
                    side,
                    bytes: Vec::new(),
                    stream: Some(path.clone()),
                });
            }
            Ok(Loaded {
                side,
                bytes: super::read_regular(path).ctx("read", path)?,
                stream: None,
            })
        }
        DiffSource::GitHead { path } | DiffSource::GitCommit { path, .. } => {
            let revision = match src {
                DiffSource::GitCommit { commit, .. } => commit.as_str(),
                _ => "HEAD",
            };
            if revision != "HEAD"
                && !((revision.len() == 40 || revision.len() == 64)
                    && revision.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(CoreError::Other("invalid commit ID".into()));
            }
            let bytes = git_revision_bytes(path, revision)?;
            Ok(Loaded {
                side: Side {
                    label: format!(
                        "{} ({})",
                        label_of(path),
                        if revision == "HEAD" {
                            revision
                        } else {
                            &revision[..12]
                        }
                    ),
                    path: Some(path.clone()),
                    size: bytes.len() as u64,
                    crlf: false,
                    final_newline: false,
                },
                bytes,
                stream: None,
            })
        }
        DiffSource::Text { label, text } => Ok(Loaded {
            side: Side {
                label: label.clone(),
                path: None,
                size: text.len() as u64,
                crlf: false,
                final_newline: false,
            },
            bytes: text.clone().into_bytes(),
            stream: None,
        }),
    }
}

/// Text = valid UTF-8 without NUL bytes in the first 8 KiB.
fn as_text(bytes: &[u8]) -> Option<&str> {
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

pub fn compare(a: &DiffSource, b: &DiffSource, opts: DiffOptions) -> Result<DiffResult> {
    let mut la = load(a)?;
    let mut lb = load(b)?;
    if la.stream.is_some() || lb.stream.is_some() {
        let bin = compare_streams(&la, &lb, "file too large for a text comparison")?;
        return Ok(DiffResult {
            left: la.side,
            right: lb.side,
            text: None,
            binary: Some(bin),
        });
    }
    match (as_text(&la.bytes), as_text(&lb.bytes)) {
        (Some(ta), Some(tb)) => {
            la.side.crlf = ta.contains("\r\n");
            lb.side.crlf = tb.contains("\r\n");
            la.side.final_newline = ta.is_empty() || ta.ends_with('\n');
            lb.side.final_newline = tb.is_empty() || tb.ends_with('\n');
            let text = diff_text(ta, tb, opts);
            Ok(DiffResult {
                left: la.side,
                right: lb.side,
                text: Some(text),
                binary: None,
            })
        }
        _ => {
            let bin = compare_streams(&la, &lb, "binary content")?;
            Ok(DiffResult {
                left: la.side,
                right: lb.side,
                text: None,
                binary: Some(bin),
            })
        }
    }
}

/// Byte comparison; streams from disk for files that were not loaded.
fn compare_streams(a: &Loaded, b: &Loaded, reason: &str) -> Result<BinaryDiffResult> {
    fn reader(l: &Loaded) -> Result<Box<dyn Read>> {
        Ok(match &l.stream {
            Some(p) => Box::new(std::io::BufReader::new(
                super::open_regular(p).ctx("open", p)?,
            )),
            None => Box::new(std::io::Cursor::new(l.bytes.clone())),
        })
    }
    let (mut ra, mut rb) = (reader(a)?, reader(b)?);
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    let mut off: u64 = 0;
    loop {
        let na = read_full(&mut ra, &mut ba)?;
        let nb = read_full(&mut rb, &mut bb)?;
        let n = na.min(nb);
        if let Some(i) = (0..n).find(|&i| ba[i] != bb[i]) {
            return Ok(BinaryDiffResult {
                identical: false,
                first_difference: Some(off + i as u64),
                reason: reason.into(),
            });
        }
        if na != nb {
            return Ok(BinaryDiffResult {
                identical: false,
                first_difference: Some(off + n as u64),
                reason: reason.into(),
            });
        }
        if na == 0 {
            return Ok(BinaryDiffResult {
                identical: true,
                first_difference: None,
                reason: reason.into(),
            });
        }
        off += n as u64;
    }
}

fn read_full(r: &mut dyn Read, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = r.read(&mut buf[n..]).ctx("read", "")?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

fn split_lines(t: &str) -> Vec<String> {
    let mut v: Vec<String> = t
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop(); // trailing newline is not an extra empty line
    }
    v
}

fn normalize(line: &str, raw: &str, opts: DiffOptions) -> String {
    let mut s = if opts.ignore_eol {
        line.to_string()
    } else {
        raw.to_string()
    };
    if opts.ignore_whitespace {
        s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    if opts.ignore_case {
        s = s.to_lowercase();
    }
    s
}

pub fn diff_text(a: &str, b: &str, opts: DiffOptions) -> TextDiffResult {
    let left = split_lines(a);
    let right = split_lines(b);
    // Raw lines keep "\r" so CRLF vs LF counts as a change unless ignore_eol.
    let raw = |t: &str| -> Vec<String> {
        let mut v: Vec<String> = t.split('\n').map(str::to_string).collect();
        if v.last().is_some_and(|l| l.is_empty()) {
            v.pop();
        }
        v
    };
    let (ra, rb) = (raw(a), raw(b));
    let na: Vec<String> = left
        .iter()
        .zip(&ra)
        .map(|(l, r)| normalize(l, r, opts))
        .collect();
    let nb: Vec<String> = right
        .iter()
        .zip(&rb)
        .map(|(l, r)| normalize(l, r, opts))
        .collect();

    let hunks = line_hunks(&na, &nb, opts.minimal);
    let moves = find_moves(&na, &nb, &hunks);
    let (mut moved_a, mut moved_b) = (vec![false; na.len()], vec![false; nb.len()]);
    for m in &moves {
        moved_a[m.a..m.a + m.len].fill(true);
        moved_b[m.b..m.b + m.len].fill(true);
    }

    let mut ops = Vec::new();
    let mut pairs = Vec::new();
    let (mut added, mut removed) = (0, 0);
    let (mut pa, mut pb) = (0usize, 0usize);
    let equal = |ops: &mut Vec<Op>, a: usize, b: usize, len: usize| {
        if len > 0 {
            ops.push(Op {
                tag: "equal",
                a,
                a_len: len,
                b,
                b_len: len,
            });
        }
    };
    for (a0, a1, b0, b1) in hunks {
        equal(&mut ops, pa, pb, a0 - pa);
        let (al, bl) = (a1 - a0, b1 - b0);
        removed += al;
        added += bl;
        let tag = match (al, bl) {
            (_, 0) => "delete",
            (0, _) => "insert",
            _ => "replace",
        };
        if tag == "replace" {
            // Moved lines are not edits of their neighbours: leave them out of pairing.
            let ia: Vec<usize> = (a0..a1).filter(|&i| !moved_a[i]).collect();
            let ib: Vec<usize> = (b0..b1).filter(|&j| !moved_b[j]).collect();
            let la: Vec<String> = ia.iter().map(|&i| left[i].clone()).collect();
            let lb: Vec<String> = ib.iter().map(|&j| right[j].clone()).collect();
            for (i, j) in pair_lines(&la, &lb) {
                let (a, b) = (ia[i], ib[j]);
                let (sa, sb) = word_spans(&left[a], &right[b]);
                pairs.push(LinePair {
                    a,
                    b,
                    a_spans: sa,
                    b_spans: sb,
                });
            }
        }
        ops.push(Op {
            tag,
            a: a0,
            a_len: al,
            b: b0,
            b_len: bl,
        });
        pa = a1;
        pb = b1;
    }
    equal(&mut ops, pa, pb, na.len() - pa);
    let total = na.len() + nb.len();
    let same = total - added - removed; // equal lines, counted on both sides
    let ratio = if total == 0 {
        1.0
    } else {
        same as f32 / total as f32
    };
    TextDiffResult {
        left_lines: left,
        right_lines: right,
        ops,
        pairs,
        added,
        removed,
        ratio,
        moves,
    }
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
fn find_moves(na: &[String], nb: &[String], hunks: &[(usize, usize, usize, usize)]) -> Vec<Move> {
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
            moves.push(Move { a: i, b: j, len: n });
            i += n;
        } else {
            i += 1;
        }
    }
    moves
}

/// Changed line ranges `(a0, a1, b0, b1)`. Histogram diff (as git uses) with the
/// indent heuristic, so ambiguous blocks start and end where a reader expects
/// (a whole function, not "}\n\nfn x() {"); `minimal` gives the fewest changed lines.
fn line_hunks(na: &[String], nb: &[String], minimal: bool) -> Vec<(usize, usize, usize, usize)> {
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

/// Lines that count as "the same line, changed" rather than one removed and another added.
const PAIR_THRESHOLD: f32 = 0.4;
/// Largest block (left × right lines) aligned optimally; bigger ones pair by position.
const PAIR_MAX_CELLS: usize = 40_000;

/// Similarity 0..1 of two lines: Dice coefficient of their character bigrams,
/// ignoring leading and trailing whitespace.
fn line_similarity(a: &str, b: &str) -> f32 {
    let (a, b) = (a.trim(), b.trim());
    if a == b {
        return 1.0;
    }
    let bigrams = |s: &str| -> Vec<(char, char)> {
        let c: Vec<char> = s.chars().collect();
        c.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let (ba, bb) = (bigrams(a), bigrams(b));
    if ba.is_empty() || bb.is_empty() {
        return 0.0;
    }
    let mut counts: std::collections::HashMap<(char, char), i32> = std::collections::HashMap::new();
    for g in &ba {
        *counts.entry(*g).or_default() += 1;
    }
    let mut common = 0;
    for g in &bb {
        if let Some(c) = counts.get_mut(g) {
            if *c > 0 {
                *c -= 1;
                common += 1;
            }
        }
    }
    2.0 * common as f32 / (ba.len() + bb.len()) as f32
}

/// Which lines of a replaced block are edits of each other: an order-keeping
/// alignment that maximises total similarity, so an inserted line does not push
/// every following line out of step. Lines too different stay unpaired (shown
/// as removed / added). Returns (left index, right index) within the block.
fn pair_lines(l: &[String], r: &[String]) -> Vec<(usize, usize)> {
    let (m, n) = (l.len(), r.len());
    if m * n > PAIR_MAX_CELLS {
        return (0..m.min(n))
            .filter(|&i| line_similarity(&l[i], &r[i]) >= PAIR_THRESHOLD)
            .map(|i| (i, i))
            .collect();
    }
    let sim: Vec<Vec<f32>> = l
        .iter()
        .map(|x| r.iter().map(|y| line_similarity(x, y)).collect())
        .collect();
    // score[i][j]: best total for l[..i] and r[..j]
    let mut score = vec![vec![0f32; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            let mut best = score[i - 1][j].max(score[i][j - 1]);
            let s = sim[i - 1][j - 1];
            if s >= PAIR_THRESHOLD {
                best = best.max(score[i - 1][j - 1] + s);
            }
            score[i][j] = best;
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (m, n);
    while i > 0 && j > 0 {
        let s = sim[i - 1][j - 1];
        if s >= PAIR_THRESHOLD && (score[i][j] - (score[i - 1][j - 1] + s)).abs() < 1e-6 {
            out.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if score[i - 1][j] >= score[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    out.reverse();
    out
}

/// Splits a line into words (letters, digits, `_`), whitespace runs and single
/// other characters.
fn tokens(s: &str) -> Vec<&str> {
    #[derive(PartialEq, Clone, Copy)]
    enum K {
        Word,
        Space,
        Other,
    }
    let kind = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            K::Word
        } else if c.is_whitespace() {
            K::Space
        } else {
            K::Other
        }
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<K> = None;
    for (i, c) in s.char_indices() {
        let k = kind(c);
        if let Some(p) = prev {
            if p != k || k == K::Other {
                out.push(&s[start..i]);
                start = i;
            }
        }
        prev = Some(k);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn utf16(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Word-level changed ranges of two lines, in UTF-16 offsets (as JavaScript
/// indexes strings).
///
/// - a word changed into a similar word marks only the differing characters
///   (`count1` → `count2` marks the digit);
/// - tiny unchanged gaps between two changes (a space, one character) join the
///   changes, so a rewritten phrase reads as one change, not confetti.
fn word_spans(a: &str, b: &str) -> (Spans, Spans) {
    let (ta, tb) = (tokens(a), tokens(b));
    let (mut sa, mut sb): (Spans, Spans) = (Vec::new(), Vec::new());
    let width = |t: &[&str]| t.iter().map(|x| utf16(x)).sum::<usize>();
    let (mut pa, mut pb) = (0usize, 0usize);
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &ta, &tb) {
        let (_, ra, rb) = op.as_tag_tuple();
        let (wa, wb) = (width(&ta[ra.clone()]), width(&tb[rb.clone()]));
        match op.tag() {
            DiffTag::Equal => {}
            DiffTag::Replace if ra.len() == 1 && rb.len() == 1 => {
                let (x, y) = (ta[ra.start], tb[rb.start]);
                let (pre, suf) = common_affixes(x, y);
                let keep = pre + suf;
                if keep * 2 >= utf16(x).min(utf16(y)) && keep > 0 {
                    push_span(&mut sa, pa + pre, pa + wa - suf);
                    push_span(&mut sb, pb + pre, pb + wb - suf);
                } else {
                    push_span(&mut sa, pa, pa + wa);
                    push_span(&mut sb, pb, pb + wb);
                }
            }
            _ => {
                push_span(&mut sa, pa, pa + wa);
                push_span(&mut sb, pb, pb + wb);
            }
        }
        pa += wa;
        pb += wb;
    }
    (join_gaps(sa, a), join_gaps(sb, b))
}

/// Common prefix and suffix of two words, in UTF-16 units (never overlapping).
fn common_affixes(x: &str, y: &str) -> (usize, usize) {
    let (cx, cy): (Vec<char>, Vec<char>) = (x.chars().collect(), y.chars().collect());
    let mut p = 0;
    while p < cx.len() && p < cy.len() && cx[p] == cy[p] {
        p += 1;
    }
    let mut s = 0;
    while s < cx.len() - p && s < cy.len() - p && cx[cx.len() - 1 - s] == cy[cy.len() - 1 - s] {
        s += 1;
    }
    let w = |c: &[char]| c.iter().map(|ch| ch.len_utf16()).sum::<usize>();
    (w(&cx[..p]), w(&cx[cx.len() - s..]))
}

fn push_span(v: &mut Spans, s: usize, e: usize) {
    if s >= e {
        return;
    }
    match v.last_mut() {
        Some(last) if last.1 >= s => last.1 = last.1.max(e),
        _ => v.push((s, e)),
    }
}

/// Joins spans separated by whitespace only or by a single character.
fn join_gaps(spans: Spans, line: &str) -> Spans {
    let units: Vec<u16> = line.encode_utf16().collect();
    let mut out: Spans = Vec::new();
    for (s, e) in spans {
        if let Some(last) = out.last_mut() {
            let gap = &units[last.1..s];
            let blank = String::from_utf16_lossy(gap).trim().is_empty();
            if gap.len() <= 1 || blank {
                last.1 = e;
                continue;
            }
        }
        out.push((s, e));
    }
    out
}

pub fn unified_patch(a: &DiffSource, b: &DiffSource) -> Result<String> {
    let (la, lb) = (load(a)?, load(b)?);
    let (Some(ta), Some(tb)) = (as_text(&la.bytes), as_text(&lb.bytes)) else {
        return Err(CoreError::Other("a patch needs two text files".into()));
    };
    let name = |l: &Loaded| {
        l.side
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| l.side.label.clone())
    };
    Ok(TextDiff::from_lines(ta, tb)
        .unified_diff()
        .context_radius(3)
        .header(
            &format!("a/{}", name(&la).trim_start_matches('/')),
            &format!("b/{}", name(&lb).trim_start_matches('/')),
        )
        .to_string())
}

// ───────────────────────────── git ─────────────────────────────

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    // core.fsmonitor=false: never start a background fsmonitor daemon in the user's
    // repositories (users with core.fsmonitor=true would otherwise collect one per repo).
    let mut cmd = std::process::Command::new("git");
    cmd.args(["-c", "core.fsmonitor=false", "-C"])
        .arg(dir)
        .args(args);
    // A GUI process spawning a console program gets a fresh console window on Windows,
    // which flashes on screen (FolderPane calls git_root on every navigation).
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd
        .output()
        .map_err(|e| CoreError::Other(format!("git not available: {e}")))?;
    if !out.status.success() {
        return Err(CoreError::Other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(out.stdout)
}

/// Root of the Git work tree containing `path`, if any.
pub fn git_root(path: &Path) -> Option<PathBuf> {
    let dir = if path.is_dir() { path } else { path.parent()? };
    // Outside a repository no git is started (this runs on every navigation).
    super::gitstatus::work_tree(dir)?;
    let out = git(dir, &["rev-parse", "--show-toplevel"]).ok()?;
    let s = String::from_utf8_lossy(&out).trim().to_string();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

fn git_revision_bytes(path: &Path, revision: &str) -> Result<Vec<u8>> {
    let root =
        git_root(path).ok_or_else(|| CoreError::Other("not inside a Git repository".into()))?;
    let canon = path.canonicalize().ctx("resolve", path)?;
    let root = root.canonicalize().ctx("resolve", &root)?;
    let rel = canon
        .strip_prefix(&root)
        .map_err(|_| CoreError::Other("file is outside the repository".into()))?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    git(&root, &["show", &format!("{revision}:{rel}")])
        .map_err(|e| CoreError::Other(format!("file is not in the selected revision: {e}")))
}

#[derive(Serialize)]
pub struct GitCommitInfo {
    pub id: String,
    pub subject: String,
}

/// Recent commits reachable from HEAD; does not alter the work tree.
pub fn git_commits(path: &Path) -> Result<Vec<GitCommitInfo>> {
    let root =
        git_root(path).ok_or_else(|| CoreError::Other("not inside a Git repository".into()))?;
    let out = git(
        &root,
        &[
            "log",
            "--no-show-signature",
            "-100",
            "--format=%H%x00%s%x00",
        ],
    )?;
    let fields: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    Ok(fields
        .chunks(2)
        .filter_map(|pair| {
            if pair.len() != 2 {
                return None;
            }
            let id = String::from_utf8_lossy(pair[0]).trim().to_string();
            if id.len() != 40 && id.len() != 64 {
                return None;
            }
            Some(GitCommitInfo {
                id,
                subject: String::from_utf8_lossy(pair[1]).into_owned(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt(s: &str) -> DiffSource {
        DiffSource::Text {
            label: "t".into(),
            text: s.into(),
        }
    }

    #[test]
    fn line_ops_and_stats() {
        let r = diff_text("a\nb\nc\n", "a\nB\nc\nd\n", DiffOptions::default());
        let tags: Vec<_> = r.ops.iter().map(|o| o.tag).collect();
        assert_eq!(tags, ["equal", "replace", "equal", "insert"]);
        assert_eq!((r.added, r.removed), (2, 1));
        assert!(r.ratio > 0.5 && r.ratio < 1.0);
        assert_eq!(r.left_lines, ["a", "b", "c"]);
    }

    #[test]
    fn intra_line_spans_are_utf16() {
        let r = diff_text("let čas = 1;\n", "let čas = 2;\n", DiffOptions::default());
        let p = &r.pairs[0];
        // "let čas = " is 10 UTF-16 units; the changed word starts there.
        assert_eq!(p.a_spans, vec![(10, 11)]);
        assert_eq!(p.b_spans, vec![(10, 11)]);
    }

    #[test]
    fn inserted_line_does_not_shift_pairs() {
        let a = "echo start\nrun alpha --fast\nrun beta --slow\n";
        let b = "echo started\ntotally 12345 unrelated\nrun alpha --faster\nrun beta --slower\n";
        let r = diff_text(a, b, DiffOptions::default());
        let p: Vec<_> = r.pairs.iter().map(|p| (p.a, p.b)).collect();
        assert_eq!(p, [(0, 0), (1, 2), (2, 3)]);
    }

    #[test]
    fn unrelated_lines_are_not_paired() {
        let r = diff_text(
            "alpha beta gamma\n",
            "1234 + 5678\n",
            DiffOptions::default(),
        );
        assert_eq!(r.ops[0].tag, "replace");
        assert!(r.pairs.is_empty());
    }

    #[test]
    fn similar_word_marks_only_changed_chars() {
        let r = diff_text(
            "let count1 = x;\n",
            "let count2 = x;\n",
            DiffOptions::default(),
        );
        assert_eq!(r.pairs[0].a_spans, vec![(9, 10)]);
        assert_eq!(r.pairs[0].b_spans, vec![(9, 10)]);
    }

    #[test]
    fn small_gaps_between_changes_join() {
        let r = diff_text(
            "say foo bar now\n",
            "say xx yy now\n",
            DiffOptions::default(),
        );
        assert_eq!(r.pairs[0].a_spans, vec![(4, 11)]);
        assert_eq!(r.pairs[0].b_spans, vec![(4, 9)]);
    }

    #[test]
    fn indent_heuristic_keeps_functions_whole() {
        let a = "fn a() {\n    x\n}\n\nfn c() {\n    z\n}\n";
        let b = "fn a() {\n    x\n}\n\nfn b() {\n    y\n}\n\nfn c() {\n    z\n}\n";
        let r = diff_text(a, b, DiffOptions::default());
        let ins = r.ops.iter().find(|o| o.tag == "insert").unwrap();
        assert_eq!(r.right_lines[ins.b], "fn b() {");
        assert_eq!(ins.b_len, 4);
    }

    #[test]
    fn moved_function_is_detected() {
        let f = "fn helper(x: u32) -> u32 {\n    let y = x * 2;\n    y + 1\n}\n";
        let a = format!("fn main() {{\n    run();\n}}\n\n{f}\nfn other() {{}}\n");
        let b = format!(
            "{}\nfn main() {{\n    run();\n}}\n\nfn other() {{}}\n",
            f.replace("    ", "\t")
        );
        let r = diff_text(&a, &b, DiffOptions::default());
        assert_eq!(
            r.moves.len(),
            1,
            "{:?}",
            r.ops.iter().map(|o| o.tag).collect::<Vec<_>>()
        );
        let m = r.moves[0];
        assert_eq!(r.left_lines[m.a], "fn helper(x: u32) -> u32 {");
        assert_eq!(r.right_lines[m.b], "fn helper(x: u32) -> u32 {");
        assert!(m.len >= 3);
        // moved lines are never paired as edits
        assert!(r.pairs.iter().all(|p| p.a < m.a || p.a >= m.a + m.len));
    }

    #[test]
    fn short_or_trivial_blocks_are_not_moves() {
        let r = diff_text(
            "a1\nx\n}\n}\n}\nb\n",
            "b\n}\n}\n}\nc\n",
            DiffOptions::default(),
        );
        assert!(r.moves.is_empty());
    }

    #[test]
    fn ignore_options() {
        let o = DiffOptions {
            ignore_whitespace: true,
            ignore_case: true,
            ignore_eol: true,
            minimal: false,
        };
        let r = diff_text("Hello   World\r\nx\n", "hello world\nx\n", o);
        assert!(r.ops.iter().all(|op| op.tag == "equal"));
        let strict = diff_text("a\r\n", "a\n", DiffOptions::default());
        assert_eq!(
            strict.ops[0].tag, "replace",
            "CRLF vs LF differs unless ignored"
        );
    }

    #[test]
    fn binary_verdict() {
        let t = tempfile::tempdir().unwrap();
        let (p1, p2) = (t.path().join("a.bin"), t.path().join("b.bin"));
        std::fs::write(&p1, [0u8, 1, 2, 3]).unwrap();
        std::fs::write(&p2, [0u8, 1, 9, 3]).unwrap();
        let r = compare(
            &DiffSource::File { path: p1.clone() },
            &DiffSource::File { path: p2 },
            DiffOptions::default(),
        )
        .unwrap();
        let b = r.binary.unwrap();
        assert!(!b.identical);
        assert_eq!(b.first_difference, Some(2));
        let same = compare(
            &DiffSource::File { path: p1.clone() },
            &DiffSource::File { path: p1 },
            DiffOptions::default(),
        )
        .unwrap();
        assert!(same.binary.unwrap().identical);
    }

    #[test]
    fn text_vs_clipboard_and_patch() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("f.txt");
        std::fs::write(&p, "one\ntwo\n").unwrap();
        let r = compare(
            &DiffSource::File { path: p.clone() },
            &txt("one\n2\n"),
            DiffOptions::default(),
        )
        .unwrap();
        assert_eq!(r.text.unwrap().added, 1);
        let patch = unified_patch(&DiffSource::File { path: p }, &txt("one\n2\n")).unwrap();
        assert!(patch.contains("-two") && patch.contains("+2") && patch.contains("@@"));
    }

    #[test]
    fn git_head_version() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let ok = |args: &[&str]| {
            std::process::Command::new("git")
                .args(["-c", "core.fsmonitor=false", "-C"])
                .arg(d)
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !ok(&["init", "-q"]) {
            return; // git not installed
        }
        ok(&["config", "user.email", "t@t"]);
        ok(&["config", "user.name", "t"]);
        std::fs::write(d.join("f.txt"), "old\n").unwrap();
        ok(&["add", "f.txt"]);
        assert!(ok(&["commit", "-qm", "x"]));
        std::fs::write(d.join("f.txt"), "new\n").unwrap();
        let p = d.join("f.txt");
        assert!(git_root(&p).is_some());
        let r = compare(
            &DiffSource::GitHead { path: p.clone() },
            &DiffSource::File { path: p },
            DiffOptions::default(),
        )
        .unwrap();
        let tx = r.text.unwrap();
        assert_eq!(tx.left_lines, ["old"]);
        assert_eq!(tx.right_lines, ["new"]);
        assert!(ok(&["add", "f.txt"]));
        assert!(ok(&["commit", "-qm", "second"]));
        std::fs::write(d.join("f.txt"), "working\n").unwrap();
        let p = d.join("f.txt");
        let commits = git_commits(&p).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[1].subject, "x");
        let result = compare(
            &DiffSource::GitCommit {
                path: p.clone(),
                commit: commits[1].id.clone(),
            },
            &DiffSource::File { path: p.clone() },
            DiffOptions::default(),
        )
        .unwrap();
        assert_eq!(result.text.unwrap().left_lines, ["old"]);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "working\n");
        assert!(compare(
            &DiffSource::GitCommit {
                path: p.clone(),
                commit: "-".repeat(40)
            },
            &DiffSource::File { path: p },
            DiffOptions::default()
        )
        .is_err());
    }
}

// ───────────────────────────── images ─────────────────────────────

/// Images larger than this (pixels) are not diffed pixel by pixel.
pub const MAX_IMAGE_PIXELS: u64 = 60_000_000;
/// Per-channel difference below this counts as unchanged (compression noise).
const PIXEL_THRESHOLD: u8 = 10;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDiff {
    pub left_size: (u32, u32),
    pub right_size: (u32, u32),
    /// Pixels compared (the overlapping area when sizes differ).
    pub compared: u64,
    pub changed: u64,
    /// Largest per-channel difference found (0–255).
    pub max_delta: u8,
    /// PNG highlighting changed pixels in red over a faded copy of the left image.
    pub diff_png: PathBuf,
}

pub fn image_diff(a: &Path, b: &Path, out: &Path) -> Result<ImageDiff> {
    let open = |p: &Path| -> Result<image::RgbaImage> {
        drop(
            super::open_regular(p)
                .map_err(|e| CoreError::Other(format!("{}: {e}", p.display())))?,
        );
        let (w, h) = image::image_dimensions(p)
            .map_err(|e| CoreError::Other(format!("{}: {e}", p.display())))?;
        if w as u64 * h as u64 > MAX_IMAGE_PIXELS {
            return Err(CoreError::Other(format!(
                "{} is too large to compare pixel by pixel",
                p.display()
            )));
        }
        Ok(image::open(p)
            .map_err(|e| CoreError::Other(format!("{}: {e}", p.display())))?
            .to_rgba8())
    };
    let (ia, ib) = (open(a)?, open(b)?);
    let (w, h) = (ia.width().min(ib.width()), ia.height().min(ib.height()));
    let mut out_img = image::RgbaImage::new(w, h);
    let (mut changed, mut max_delta) = (0u64, 0u8);
    for y in 0..h {
        for x in 0..w {
            let (pa, pb) = (ia.get_pixel(x, y).0, ib.get_pixel(x, y).0);
            let d = (0..4).map(|c| pa[c].abs_diff(pb[c])).max().unwrap_or(0);
            max_delta = max_delta.max(d);
            let px = if d > PIXEL_THRESHOLD {
                changed += 1;
                [235, 40, 70, 255]
            } else {
                // faded luminance of the left image as context
                let l = (0.299 * pa[0] as f32 + 0.587 * pa[1] as f32 + 0.114 * pa[2] as f32) as u8;
                let f = 170 + l / 3;
                [f, f, f, 255]
            };
            out_img.put_pixel(x, y, image::Rgba(px));
        }
    }
    out_img
        .save(out)
        .map_err(|e| CoreError::Other(format!("cannot write diff image: {e}")))?;
    Ok(ImageDiff {
        left_size: ia.dimensions(),
        right_size: ib.dimensions(),
        compared: w as u64 * h as u64,
        changed,
        max_delta,
        diff_png: out.to_path_buf(),
    })
}

#[cfg(test)]
mod image_tests {
    use super::*;

    #[test]
    fn counts_changed_pixels() {
        let t = tempfile::tempdir().unwrap();
        let (pa, pb, po) = (
            t.path().join("a.png"),
            t.path().join("b.png"),
            t.path().join("d.png"),
        );
        let mut a = image::RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 255]));
        a.save(&pa).unwrap();
        for x in 0..5 {
            a.put_pixel(x, 0, image::Rgba([255, 255, 255, 255]));
        }
        a.put_pixel(9, 9, image::Rgba([3, 3, 3, 255])); // below threshold
        a.save(&pb).unwrap();
        let d = image_diff(&pa, &pb, &po).unwrap();
        assert_eq!((d.compared, d.changed, d.max_delta), (100, 5, 255));
        assert!(po.exists());
    }
}

// ───────────────────────────── many files ─────────────────────────────

/// Most files in one similarity matrix.
pub const MAX_MATRIX_FILES: usize = 30;
/// Larger files are compared byte-for-byte only (identical or not).
const MATRIX_TEXT_LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarityMatrix {
    pub paths: Vec<PathBuf>,
    pub names: Vec<String>,
    pub sizes: Vec<u64>,
    /// "text" | "binary" | an error message
    pub kinds: Vec<String>,
    /// ratios[i][j] in 0..1 (1 on the diagonal).
    pub ratios: Vec<Vec<f32>>,
    /// Indices ordered so similar files sit next to each other.
    pub order: Vec<usize>,
}

enum Content {
    Lines(Vec<String>),
    Bytes(Vec<u8>),
    Large(PathBuf),
    Error(String),
}

pub fn similarity_matrix(paths: &[PathBuf]) -> Result<SimilarityMatrix> {
    if paths.len() < 2 {
        return Err(CoreError::Other("select at least two files".into()));
    }
    if paths.len() > MAX_MATRIX_FILES {
        return Err(CoreError::Other(format!(
            "at most {MAX_MATRIX_FILES} files can be compared at once"
        )));
    }
    let contents: Vec<(u64, Content)> = paths
        .iter()
        .map(|p| match std::fs::metadata(p) {
            Err(e) => (0, Content::Error(e.to_string())),
            // Spreadsheets: compare cell rows, not the zipped bytes.
            Ok(m) if crate::fs::table::is_spreadsheet(p) => {
                match crate::fs::table::table_as_lines(p) {
                    Ok(lines) => (m.len(), Content::Lines(lines)),
                    Err(e) => (m.len(), Content::Error(e.to_string())),
                }
            }
            Ok(m) if m.is_dir() => (0, Content::Error("folder".into())),
            Ok(m) if m.len() > MATRIX_TEXT_LIMIT => (m.len(), Content::Large(p.clone())),
            Ok(m) => match super::read_regular(p) {
                Err(e) => (m.len(), Content::Error(e.to_string())),
                Ok(b) => match as_text(&b) {
                    Some(t) => (m.len(), Content::Lines(split_lines(t))),
                    None => (m.len(), Content::Bytes(b)),
                },
            },
        })
        .collect();
    let n = paths.len();
    let mut ratios = vec![vec![1.0f32; n]; n];
    let pairs: Vec<(usize, usize)> = (0..n)
        .flat_map(|i| (i + 1..n).map(move |j| (i, j)))
        .collect();
    let threads = std::thread::available_parallelism()
        .map(|t| t.get())
        .unwrap_or(4)
        .min(8);
    let results: Vec<(usize, usize, f32)> = std::thread::scope(|s| {
        let chunks: Vec<_> = pairs.chunks(pairs.len().div_ceil(threads).max(1)).collect();
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let contents = &contents;
                s.spawn(move || {
                    chunk
                        .iter()
                        .map(|&(i, j)| (i, j, pair_ratio(&contents[i].1, &contents[j].1)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    for (i, j, r) in results {
        ratios[i][j] = r;
        ratios[j][i] = r;
    }
    let kinds = contents
        .iter()
        .map(|(_, c)| match c {
            Content::Lines(_) => "text".to_string(),
            Content::Bytes(_) | Content::Large(_) => "binary".to_string(),
            Content::Error(e) => e.clone(),
        })
        .collect();
    Ok(SimilarityMatrix {
        names: paths.iter().map(|p| label_of(p)).collect(),
        sizes: contents.iter().map(|(s, _)| *s).collect(),
        order: seriate(&ratios),
        paths: paths.to_vec(),
        kinds,
        ratios,
    })
}

fn pair_ratio(a: &Content, b: &Content) -> f32 {
    match (a, b) {
        (Content::Lines(x), Content::Lines(y)) => {
            let ops = similar::capture_diff_slices(Algorithm::Myers, x, y);
            similar::get_diff_ratio(&ops, x.len(), y.len())
        }
        (Content::Bytes(x), Content::Bytes(y)) => (x == y) as u8 as f32,
        (Content::Large(x), Content::Large(y)) => {
            let la = Loaded {
                side: Side {
                    label: String::new(),
                    path: None,
                    size: 0,
                    crlf: false,
                    final_newline: false,
                },
                bytes: vec![],
                stream: Some(x.clone()),
            };
            let lb = Loaded {
                side: Side {
                    label: String::new(),
                    path: None,
                    size: 0,
                    crlf: false,
                    final_newline: false,
                },
                bytes: vec![],
                stream: Some(y.clone()),
            };
            compare_streams(&la, &lb, "")
                .map(|r| r.identical as u8 as f32)
                .unwrap_or(0.0)
        }
        _ => 0.0,
    }
}

/// Nearest-neighbour chain from the file most similar to all others.
fn seriate(r: &[Vec<f32>]) -> Vec<usize> {
    let n = r.len();
    let avg = |i: usize| r[i].iter().sum::<f32>();
    let mut start = 0;
    for i in 1..n {
        if avg(i) > avg(start) {
            start = i;
        }
    }
    let mut order = vec![start];
    let mut used = vec![false; n];
    used[start] = true;
    while order.len() < n {
        let last = *order.last().unwrap();
        let next = (0..n)
            .filter(|&j| !used[j])
            .max_by(|&a, &b| r[last][a].total_cmp(&r[last][b]))
            .unwrap();
        used[next] = true;
        order.push(next);
    }
    order
}

#[cfg(test)]
mod matrix_tests {
    use super::*;

    #[test]
    fn groups_similar_files() {
        let t = tempfile::tempdir().unwrap();
        let base: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let near = base.replace("line 7\n", "line seven\n");
        let other: String = (0..50).map(|i| format!("other {i}\n")).collect();
        let ps: Vec<PathBuf> = ["a.txt", "b.txt", "c.txt", "d.txt"]
            .iter()
            .map(|n| t.path().join(n))
            .collect();
        std::fs::write(&ps[0], &base).unwrap();
        std::fs::write(&ps[1], &other).unwrap();
        std::fs::write(&ps[2], &near).unwrap();
        std::fs::write(&ps[3], &base).unwrap();
        let m = similarity_matrix(&ps).unwrap();
        assert_eq!(m.ratios[0][3], 1.0);
        assert!(m.ratios[0][2] > 0.9 && m.ratios[0][1] < 0.1);
        // the odd one (b) ends up at an edge, the near-identical files together
        let pos = |i: usize| m.order.iter().position(|&x| x == i).unwrap();
        assert!(pos(1) == 0 || pos(1) == 3);
        assert!(pos(0).abs_diff(pos(3)) == 1 || pos(0).abs_diff(pos(2)) == 1);
    }
}
