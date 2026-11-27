//! Three-way merge of two independent edits against a base, as a model the merge views can share.
//!
//! The base is cut into ordered chunks by the changes of the two sides:
//! * a stretch nobody changed is `Stable`;
//! * a change of one side only is taken as it is (`Left`, `Right`);
//! * the same change on both sides is taken once (`Same`);
//! * edits that overlap or touch, and differ, are a `Conflict` that stays open until a choice (left, right, both
//!   with the left first, base) is made.
//!
//! Every chunk has a stable ID (the same inputs give the same IDs, so a choice made for an ID stays valid when the
//! merge is computed again), and spans in line numbers of each source. Lines keep their terminators (`\r\n`, a missing
//! final newline), so a result is made of the original bytes and never of a re-encoded copy.
//!
//! Nothing here writes anything. A result can be saved only through [`resolve`], which refuses while a conflict has no
//! choice; [`preview`] shows an unfinished merge with Git-style markers for editing by hand and is not a result.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag};
use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

/// Largest stage that is merged, in bytes, and the most lines of one.
pub const MAX_STAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_STAGE_LINES: usize = 1_000_000;
/// The time a line diff may take before it settles for a result that is valid but not minimal.
#[cfg(not(target_arch = "wasm32"))]
const DIFF_BUDGET: Duration = Duration::from_secs(10);
/// Work between two looks at the cancellation flag.
const CANCEL_EVERY: usize = 2048;

/// One of the three inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Base,
    Left,
    Right,
}

/// The three inputs as bytes. `None`: the stage does not exist (no common ancestor for a file added on both sides,
/// a file deleted on one side).
#[derive(Debug, Clone, Copy, Default)]
pub struct Inputs<'a> {
    pub base: Option<&'a [u8]>,
    pub left: Option<&'a [u8]>,
    pub right: Option<&'a [u8]>,
}

/// Why a merge cannot be made from text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unavailable {
    /// The left or right side does not exist (a deletion, or a stage that could not be read).
    Missing,
    /// Larger than [`MAX_STAGE_BYTES`] or more than [`MAX_STAGE_LINES`] lines.
    TooLarge,
}

/// A range of lines of one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    pub start: usize,
    pub len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Nobody changed it.
    Stable,
    /// Only the left side changed it.
    Left,
    /// Only the right side changed it.
    Right,
    /// Both sides made the same change.
    Same,
    /// Both sides changed it, differently: open until a choice is made.
    Conflict,
}

/// What to take for a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Choice {
    Left,
    Right,
    /// The left lines, then the right lines, always in this order.
    Both,
    /// Merge compatible edits against the base, when unambiguous.
    Combine,
    Base,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    /// Stable for the same inputs; the key of a choice.
    pub id: String,
    pub kind: Kind,
    pub base: Span,
    pub left: Span,
    pub right: Span,
    /// The lines of each source for a chunk that was changed; empty for a stable one (read them through the spans).
    pub base_lines: Vec<String>,
    pub left_lines: Vec<String>,
    pub right_lines: Vec<String>,
    /// A base-aware combination, or None if edits are ambiguous.
    pub combined_lines: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Merge {
    pub chunks: Vec<Chunk>,
    /// The IDs of the conflicts, in order.
    pub conflicts: Vec<String>,
}

/// The answer to a merge: the chunks, or why there is nothing to merge as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Outcome {
    Text {
        merge: Merge,
    },
    /// A stage is binary (a NUL byte, or not UTF-8); the first one found.
    Binary {
        stage: Stage,
    },
    Unavailable {
        stage: Stage,
        reason: Unavailable,
    },
}

/// Where a part of a result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    Base,
    Left,
    Right,
    /// The change both sides made.
    Same,
    ChosenLeft,
    ChosenRight,
    ChosenBoth,
    ChosenCombine,
    ChosenBase,
}

/// A run of lines of a result and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub chunk: String,
    pub origin: Origin,
    /// First line and number of lines in the result.
    pub start: usize,
    pub len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Resolution {
    Resolved {
        text: String,
        provenance: Vec<Segment>,
    },
    /// Conflicts that have no choice yet; nothing can be saved.
    Unresolved {
        conflicts: Vec<String>,
    },
    Binary {
        stage: Stage,
    },
    Unavailable {
        stage: Stage,
        reason: Unavailable,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub text: String,
    /// Conflicts shown with markers because they have no choice.
    pub unresolved: Vec<String>,
}

/// The inputs as text, or the reason there is none.
struct Texts<'a> {
    base: &'a str,
    left: &'a str,
    right: &'a str,
}

enum Blocked {
    Binary(Stage),
    Unavailable(Stage, Unavailable),
}

fn text_of(bytes: &[u8]) -> Option<&str> {
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

fn texts<'a>(inputs: &Inputs<'a>) -> std::result::Result<Texts<'a>, Blocked> {
    let (left, right) = match (inputs.left, inputs.right) {
        (Some(left), Some(right)) => (left, right),
        (None, _) => return Err(Blocked::Unavailable(Stage::Left, Unavailable::Missing)),
        (_, None) => return Err(Blocked::Unavailable(Stage::Right, Unavailable::Missing)),
    };
    // A file added on both sides has no base: it merges against an empty one.
    let base = inputs.base.unwrap_or(&[]);
    let mut found = Vec::with_capacity(3);
    for (stage, bytes) in [
        (Stage::Base, base),
        (Stage::Left, left),
        (Stage::Right, right),
    ] {
        if bytes.len() > MAX_STAGE_BYTES {
            return Err(Blocked::Unavailable(stage, Unavailable::TooLarge));
        }
        match text_of(bytes) {
            Some(text) => found.push(text),
            None => return Err(Blocked::Binary(stage)),
        }
    }
    Ok(Texts {
        base: found[0],
        left: found[1],
        right: found[2],
    })
}

/// Lines with their terminators, so that joining them gives back the text exactly.
fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

#[derive(Debug, Clone, Copy)]
struct Hunk {
    bs: usize,
    be: usize,
    se: usize,
}

/// The changed ranges of a side against the base. Neighbouring changes are one range.
fn hunks(base: &[&str], side: &[&str]) -> Vec<Hunk> {
    // `Instant::now` panics on wasm32-unknown-unknown: there the worker is ended from outside
    // instead (the page drops a result it no longer wants and terminates the worker).
    #[cfg(not(target_arch = "wasm32"))]
    let deadline = Some(Instant::now() + DIFF_BUDGET);
    #[cfg(target_arch = "wasm32")]
    let deadline = None;
    let ops = similar::capture_diff_slices_deadline(Algorithm::Myers, base, side, deadline);
    let mut out: Vec<Hunk> = Vec::new();
    for op in ops {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.be == old.start && last.se == new.start => {
                last.be = old.end;
                last.se = new.end;
            }
            _ => out.push(Hunk {
                bs: old.start,
                be: old.end,
                se: new.end,
            }),
        }
    }
    out
}

// Restrict automatic combination to small, unambiguous sequences. In particular,
// duplicate lines can admit multiple equally valid alignments and are left for
// the user to resolve instead of guessing which occurrence was edited.
fn combine(base: &[&str], left: &[&str], right: &[&str]) -> Option<Vec<String>> {
    use std::collections::HashSet;

    const MAX_LINES: usize = 512;
    const MAX_BYTES: usize = 1024 * 1024;
    if [base, left, right].iter().any(|part| {
        part.len() > MAX_LINES || part.iter().map(|line| line.len()).sum::<usize>() > MAX_BYTES
    }) {
        return None;
    }
    fn key(line: &str) -> &str {
        match line.strip_suffix('\n') {
            Some(line) => line.strip_suffix('\r').unwrap_or(line),
            None => line,
        }
    }
    let b: Vec<&str> = base.iter().map(|s| key(s)).collect();
    let l: Vec<&str> = left.iter().map(|s| key(s)).collect();
    let r: Vec<&str> = right.iter().map(|s| key(s)).collect();
    for part in [&b, &l, &r] {
        let mut seen = HashSet::new();
        if part.iter().any(|line| !seen.insert(*line)) {
            return None;
        }
    }

    #[derive(Clone, Copy)]
    struct Edit {
        start: usize,
        end: usize,
        side_start: usize,
        side_end: usize,
    }
    fn edits(base: &[&str], side: &[&str]) -> (Vec<Edit>, Vec<Option<usize>>) {
        let mut result: Vec<Edit> = Vec::new();
        let mut map = vec![None; base.len()];
        for op in similar::capture_diff_slices(Algorithm::Myers, base, side) {
            let (tag, old, new) = op.as_tag_tuple();
            if tag == DiffTag::Equal {
                for (b, s) in old.zip(new) {
                    map[b] = Some(s);
                }
            } else if let Some(last) = result
                .last_mut()
                .filter(|last| last.end == old.start && last.side_end == new.start)
            {
                last.end = old.end;
                last.side_end = new.end;
            } else {
                result.push(Edit {
                    start: old.start,
                    end: old.end,
                    side_start: new.start,
                    side_end: new.end,
                });
            }
        }
        (result, map)
    }
    let (le, lm) = edits(&b, &l);
    let (re, rm) = edits(&b, &r);
    let (mut li, mut ri, mut cursor) = (0, 0, 0);
    let mut result: Vec<String> = Vec::new();
    while cursor < base.len() || li < le.len() || ri < re.len() {
        let next = le
            .get(li)
            .map(|e| e.start)
            .into_iter()
            .chain(re.get(ri).map(|e| e.start))
            .min()
            .unwrap_or(base.len());
        while cursor < next {
            let (b, l, r) = (base[cursor], left[lm[cursor]?], right[rm[cursor]?]);
            let selected = if l == r {
                l
            } else if l == b {
                r
            } else if r == b {
                l
            } else if l.ends_with('\n') && r.ends_with('\n') && key(l) == key(r) {
                // Both added a terminator or changed its style; prefer the left bytes.
                l
            } else {
                return None;
            };
            result.push(selected.to_owned());
            cursor += 1;
        }
        let (a, z) = (le.get(li).copied(), re.get(ri).copied());
        if let (Some(a), Some(z)) = (a, z) {
            if a.start == z.start {
                if a.end != z.end {
                    return None;
                }
                let left_part = &left[a.side_start..a.side_end];
                let right_part = &right[z.side_start..z.side_end];
                if left_part != right_part && a.start != a.end {
                    return None;
                }
                result.extend(left_part.iter().map(|s| (*s).to_owned()));
                if left_part != right_part {
                    // A shared inserted line might be the same edit. Refuse rather than
                    // duplicate it or guess how to align partially overlapping inserts.
                    let inserted: HashSet<&str> = left_part.iter().map(|line| key(line)).collect();
                    if right_part.iter().any(|line| inserted.contains(key(line))) {
                        return None;
                    }
                    // Explicit Combine orders independent insertions left before right.
                    if let Some(last) = result.last_mut() {
                        if !right_part.is_empty() && !last.ends_with('\n') {
                            last.push_str(if left.iter().any(|s| s.ends_with("\r\n")) {
                                "\r\n"
                            } else {
                                "\n"
                            });
                        }
                    }
                    result.extend(right_part.iter().map(|s| (*s).to_owned()));
                }
                cursor = a.end;
                li += 1;
                ri += 1;
                continue;
            }
        }
        let use_left = match (a, z) {
            (Some(a), Some(z)) => a.start < z.start,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        let (edit, other, source) = if use_left {
            (a?, z, left)
        } else {
            (z?, a, right)
        };
        // Insertions at the boundary of another edit have no certain ordering.
        if other.is_some_and(|o| o.start < edit.end || (o.start == edit.end && o.start == o.end)) {
            return None;
        }
        result.extend(
            source[edit.side_start..edit.side_end]
                .iter()
                .map(|s| (*s).to_owned()),
        );
        cursor = edit.end;
        if use_left {
            li += 1;
        } else {
            ri += 1;
        }
    }
    // Never invent a terminator when moving a formerly final line into the middle.
    if result
        .iter()
        .take(result.len().saturating_sub(1))
        .any(|line| !line.ends_with('\n'))
    {
        return None;
    }
    Some(result)
}

fn owned(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

/// FNV-1a over the pieces, so an ID depends on nothing but the inputs.
fn fingerprint(parts: &[&[&str]], start: usize) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(&start.to_le_bytes());
    for part in parts {
        feed(&(part.len() as u64).to_le_bytes());
        for line in *part {
            feed(&(line.len() as u64).to_le_bytes());
            feed(line.as_bytes());
        }
    }
    hash
}

fn cancelled(check: &dyn Fn() -> bool) -> Result<()> {
    if check() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn build(texts: &Texts, cancel: &dyn Fn() -> bool) -> Result<Merge> {
    let (base, left, right) = (lines(texts.base), lines(texts.left), lines(texts.right));
    if base.len() > MAX_STAGE_LINES || left.len() > MAX_STAGE_LINES || right.len() > MAX_STAGE_LINES
    {
        // Reported by the caller as too large; this is only reached through `merge_texts`.
        return Err(Error::Other("A file has too many lines to merge".into()));
    }
    cancelled(cancel)?;
    let left_hunks = hunks(&base, &left);
    cancelled(cancel)?;
    let right_hunks = hunks(&base, &right);
    cancelled(cancel)?;
    // Left first on equal positions, so the order of the result is the same every time.
    let mut tagged: Vec<(Hunk, bool)> = left_hunks
        .into_iter()
        .map(|h| (h, true))
        .chain(right_hunks.into_iter().map(|h| (h, false)))
        .collect();
    tagged.sort_by_key(|(h, _)| (h.bs, h.be));

    let mut chunks: Vec<Chunk> = Vec::new();
    let (mut cursor, mut lpos, mut rpos) = (0usize, 0usize, 0usize);
    let stable =
        |chunks: &mut Vec<Chunk>, from: usize, to: usize, lpos: &mut usize, rpos: &mut usize| {
            if to > from {
                let n = to - from;
                chunks.push(Chunk {
                    id: format!("s{from:x}"),
                    kind: Kind::Stable,
                    base: Span {
                        start: from,
                        len: n,
                    },
                    left: Span {
                        start: *lpos,
                        len: n,
                    },
                    right: Span {
                        start: *rpos,
                        len: n,
                    },
                    base_lines: Vec::new(),
                    left_lines: Vec::new(),
                    right_lines: Vec::new(),
                    combined_lines: None,
                });
                *lpos += n;
                *rpos += n;
            }
        };
    let mut index = 0;
    let mut groups = 0usize;
    while index < tagged.len() {
        groups += 1;
        if groups.is_multiple_of(CANCEL_EVERY) {
            cancelled(cancel)?;
        }
        // Changes that overlap or touch belong to one chunk.
        let first = index;
        let start = tagged[first].0.bs;
        let mut end = tagged[first].0.be;
        index += 1;
        while index < tagged.len() && tagged[index].0.bs <= end {
            end = end.max(tagged[index].0.be);
            index += 1;
        }
        let group = &tagged[first..index];
        stable(&mut chunks, cursor, start, &mut lpos, &mut rpos);
        let last_of = |from_left: bool| {
            group
                .iter()
                .rev()
                .find(|(_, l)| *l == from_left)
                .map(|(h, _)| *h)
        };
        let (lh, rh) = (last_of(true), last_of(false));
        let left_end = lh.map_or(lpos + (end - start), |h| h.se + (end - h.be));
        let right_end = rh.map_or(rpos + (end - start), |h| h.se + (end - h.be));
        let (b, l, r) = (
            &base[start..end],
            &left[lpos..left_end],
            &right[rpos..right_end],
        );
        let kind = match (lh.is_some(), rh.is_some()) {
            (true, true) if l == r => Kind::Same,
            (true, true) => Kind::Conflict,
            (true, false) => Kind::Left,
            _ => Kind::Right,
        };
        let id = format!(
            "c{:012x}",
            fingerprint(&[b, l, r], start) & 0xffff_ffff_ffff
        );
        chunks.push(Chunk {
            id,
            kind,
            base: Span {
                start,
                len: end - start,
            },
            left: Span {
                start: lpos,
                len: left_end - lpos,
            },
            right: Span {
                start: rpos,
                len: right_end - rpos,
            },
            base_lines: owned(b),
            left_lines: owned(l),
            right_lines: owned(r),
            combined_lines: if kind == Kind::Conflict {
                combine(b, l, r)
            } else {
                None
            },
        });
        lpos = left_end;
        rpos = right_end;
        cursor = end;
    }
    stable(&mut chunks, cursor, base.len(), &mut lpos, &mut rpos);
    let conflicts = chunks
        .iter()
        .filter(|chunk| chunk.kind == Kind::Conflict)
        .map(|chunk| chunk.id.clone())
        .collect();
    Ok(Merge { chunks, conflicts })
}

fn too_many_lines(texts: &Texts) -> Option<Stage> {
    [
        (Stage::Base, texts.base),
        (Stage::Left, texts.left),
        (Stage::Right, texts.right),
    ]
    .into_iter()
    .find(|(_, text)| text.bytes().filter(|b| *b == b'\n').count() >= MAX_STAGE_LINES)
    .map(|(stage, _)| stage)
}

/// Merges the three inputs. `cancel` is asked now and then and stops the work with `Error::Cancelled`; the
/// inputs are only read.
pub fn merge(inputs: &Inputs, cancel: &dyn Fn() -> bool) -> Result<Outcome> {
    cancelled(cancel)?;
    let found = match texts(inputs) {
        Ok(found) => found,
        Err(Blocked::Binary(stage)) => return Ok(Outcome::Binary { stage }),
        Err(Blocked::Unavailable(stage, reason)) => {
            return Ok(Outcome::Unavailable { stage, reason })
        }
    };
    if let Some(stage) = too_many_lines(&found) {
        return Ok(Outcome::Unavailable {
            stage,
            reason: Unavailable::TooLarge,
        });
    }
    Ok(Outcome::Text {
        merge: build(&found, cancel)?,
    })
}

/// The terminator style of a text: `\r\n` when its first terminated line has one.
fn eol_of(chunks: &[Chunk]) -> &'static str {
    let crlf = chunks
        .iter()
        .flat_map(|c| {
            c.left_lines
                .iter()
                .chain(&c.right_lines)
                .chain(&c.base_lines)
        })
        .any(|l| l.ends_with("\r\n"));
    if crlf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Appends the lines of `part`, ending the previous one with `eol` when it has no terminator.
fn push_lines(out: &mut Vec<String>, part: &[String], eol: &str) {
    if let Some(last) = out.last_mut() {
        if !part.is_empty() && !last.ends_with('\n') {
            last.push_str(eol);
        }
    }
    out.extend(part.iter().cloned());
}

/// The lines of a chunk for a choice, and where they came from. `None`: a conflict without a choice.
fn chosen(
    chunk: &Chunk,
    choice: Option<Choice>,
    texts: &Texts,
    eol: &str,
) -> Option<(Vec<String>, Origin)> {
    let from_base = |span: Span, text: &str| owned(&lines(text)[span.start..span.start + span.len]);
    Some(match chunk.kind {
        Kind::Stable => (from_base(chunk.base, texts.base), Origin::Base),
        Kind::Left => (chunk.left_lines.clone(), Origin::Left),
        Kind::Right => (chunk.right_lines.clone(), Origin::Right),
        Kind::Same => (chunk.left_lines.clone(), Origin::Same),
        Kind::Conflict => match choice? {
            Choice::Left => (chunk.left_lines.clone(), Origin::ChosenLeft),
            Choice::Right => (chunk.right_lines.clone(), Origin::ChosenRight),
            Choice::Base => (chunk.base_lines.clone(), Origin::ChosenBase),
            Choice::Combine => (chunk.combined_lines.clone()?, Origin::ChosenCombine),
            Choice::Both => {
                let mut both = chunk.left_lines.clone();
                push_lines(&mut both, &chunk.right_lines, eol);
                (both, Origin::ChosenBoth)
            }
        },
    })
}

/// The merge and the texts it was made from, or the reason there is none.
fn prepared<'a>(
    inputs: &Inputs<'a>,
    cancel: &dyn Fn() -> bool,
) -> Result<std::result::Result<(Merge, Texts<'a>), Outcome>> {
    cancelled(cancel)?;
    let found = match texts(inputs) {
        Ok(found) => found,
        Err(Blocked::Binary(stage)) => return Ok(Err(Outcome::Binary { stage })),
        Err(Blocked::Unavailable(stage, reason)) => {
            return Ok(Err(Outcome::Unavailable { stage, reason }))
        }
    };
    if let Some(stage) = too_many_lines(&found) {
        return Ok(Err(Outcome::Unavailable {
            stage,
            reason: Unavailable::TooLarge,
        }));
    }
    let merge = build(&found, cancel)?;
    Ok(Ok((merge, found)))
}

fn blocked(outcome: Outcome) -> Resolution {
    match outcome {
        Outcome::Binary { stage } => Resolution::Binary { stage },
        Outcome::Unavailable { stage, reason } => Resolution::Unavailable { stage, reason },
        Outcome::Text { .. } => unreachable!("a text outcome is not blocked"),
    }
}

/// The result of the merge with `choices` (by conflict ID), ready to be saved: only when every conflict has a
/// choice. A choice for an ID that is not a conflict of these inputs is an error, not ignored (it would mean the
/// choices belong to other inputs).
pub fn resolve(
    inputs: &Inputs,
    choices: &BTreeMap<String, Choice>,
    cancel: &dyn Fn() -> bool,
) -> Result<Resolution> {
    let (merge, found) = match prepared(inputs, cancel)? {
        Ok(ready) => ready,
        Err(outcome) => return Ok(blocked(outcome)),
    };
    let conflict_ids: std::collections::HashSet<&str> =
        merge.conflicts.iter().map(String::as_str).collect();
    if let Some(unknown) = choices
        .keys()
        .find(|id| !conflict_ids.contains(id.as_str()))
    {
        return Err(Error::Other(format!(
            "{unknown} is not a conflict of these files; the merge changed"
        )));
    }
    let mut open = Vec::new();
    for (n, chunk) in merge.chunks.iter().enumerate() {
        if n.is_multiple_of(CANCEL_EVERY) {
            cancelled(cancel)?;
        }
        if chunk.kind != Kind::Conflict {
            continue;
        }
        let choice = choices.get(&chunk.id).copied();
        if choice.is_none() || (choice == Some(Choice::Combine) && chunk.combined_lines.is_none()) {
            open.push(chunk.id.clone());
        }
    }
    if !open.is_empty() {
        return Ok(Resolution::Unresolved { conflicts: open });
    }
    let eol = eol_of(&merge.chunks);
    let mut text = String::new();
    let mut provenance = Vec::with_capacity(merge.chunks.len());
    let mut line = 0usize;
    let mut previous_open = false;
    for (n, chunk) in merge.chunks.iter().enumerate() {
        if n.is_multiple_of(CANCEL_EVERY) {
            cancelled(cancel)?;
        }
        let (taken, origin) = chosen(chunk, choices.get(&chunk.id).copied(), &found, eol)
            .expect("every conflict has a choice");
        // A part that has no terminator at its end and is followed by lines is ended with the terminator of the file.
        if previous_open && !taken.is_empty() && !text.ends_with('\n') {
            text.push_str(eol);
        }
        for part in &taken {
            text.push_str(part);
        }
        previous_open = taken.last().is_some_and(|l| !l.ends_with('\n'));
        provenance.push(Segment {
            chunk: chunk.id.clone(),
            origin,
            start: line,
            len: taken.len(),
        });
        line += taken.len();
    }
    Ok(Resolution::Resolved { text, provenance })
}

/// An unfinished merge for editing by hand: the chunks with a choice as chosen, the conflicts without one between
/// Git-style markers. It is not a result and cannot be saved through [`resolve`].
pub fn preview(
    inputs: &Inputs,
    choices: &BTreeMap<String, Choice>,
    cancel: &dyn Fn() -> bool,
) -> Result<Option<Preview>> {
    let (merge, found) = match prepared(inputs, cancel)? {
        Ok(ready) => ready,
        Err(_) => return Ok(None),
    };
    let eol = eol_of(&merge.chunks);
    let mut out: Vec<String> = Vec::new();
    let mut unresolved = Vec::new();
    for (n, chunk) in merge.chunks.iter().enumerate() {
        if n.is_multiple_of(CANCEL_EVERY) {
            cancelled(cancel)?;
        }
        match chosen(chunk, choices.get(&chunk.id).copied(), &found, eol) {
            Some((taken, _)) => push_lines(&mut out, &taken, eol),
            None => {
                unresolved.push(chunk.id.clone());
                push_lines(&mut out, &[format!("<<<<<<< ours{eol}")], eol);
                push_lines(&mut out, &chunk.left_lines, eol);
                push_lines(&mut out, &[format!("======={eol}")], eol);
                push_lines(&mut out, &chunk.right_lines, eol);
                push_lines(&mut out, &[format!(">>>>>>> theirs{eol}")], eol);
            }
        }
    }
    Ok(Some(Preview {
        text: out.concat(),
        unresolved,
    }))
}

/// The request of [`merge_json`]: `choices` (conflict ID to choice) asks for the resolved result instead of the
/// chunks; `missing` names the stages that do not exist (their bytes are ignored).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct JsonRequest {
    choices: Option<BTreeMap<String, Choice>>,
    missing: Vec<Stage>,
}

/// The byte-in, JSON-out form of the merge that the browser worker calls: an [`Outcome`] without `choices`, a
/// [`Resolution`] with them, and `{"kind":"cancelled"}` or `{"kind":"error","message":…}` otherwise.
pub fn merge_json(
    base: &[u8],
    left: &[u8],
    right: &[u8],
    request: &str,
    cancel: &dyn Fn() -> bool,
) -> String {
    let request = match serde_json::from_str::<JsonRequest>(if request.trim().is_empty() {
        "{}"
    } else {
        request
    }) {
        Ok(request) => request,
        Err(e) => return error_json(&format!("invalid request: {e}")),
    };
    let stage = |which: Stage, bytes| (!request.missing.contains(&which)).then_some(bytes);
    let inputs = Inputs {
        base: stage(Stage::Base, base),
        left: stage(Stage::Left, left),
        right: stage(Stage::Right, right),
    };
    let json = match &request.choices {
        None => merge(&inputs, cancel).map(|o| serde_json::to_string(&o)),
        Some(choices) => resolve(&inputs, choices, cancel).map(|r| serde_json::to_string(&r)),
    };
    match json {
        Ok(Ok(json)) => json,
        Ok(Err(e)) => error_json(&format!("could not write the answer: {e}")),
        Err(Error::Cancelled) => r#"{"kind":"cancelled"}"#.to_string(),
        Err(Error::Other(reason)) => error_json(&reason),
    }
}

fn error_json(message: &str) -> String {
    serde_json::json!({ "kind": "error", "message": message }).to_string()
}

#[cfg(feature = "wasm")]
mod wasm {
    use wasm_bindgen::prelude::*;

    /// Merges three byte buffers; `request` and the answer are JSON (see `merge_json`).
    #[wasm_bindgen(js_name = merge)]
    pub fn merge(base: &[u8], left: &[u8], right: &[u8], request: &str) -> String {
        super::merge_json(base, left, right, request, &|| false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    const NEVER: &dyn Fn() -> bool = &|| false;

    fn inputs<'a>(base: Option<&'a str>, left: &'a str, right: &'a str) -> Inputs<'a> {
        Inputs {
            base: base.map(str::as_bytes),
            left: Some(left.as_bytes()),
            right: Some(right.as_bytes()),
        }
    }

    fn merged(base: &str, left: &str, right: &str) -> Merge {
        match merge(&inputs(Some(base), left, right), NEVER).unwrap() {
            Outcome::Text { merge } => merge,
            other => panic!("not text: {other:?}"),
        }
    }

    fn choose(merge: &Merge, picks: &[Choice]) -> BTreeMap<String, Choice> {
        merge
            .conflicts
            .iter()
            .cloned()
            .zip(picks.iter().copied())
            .collect()
    }

    fn text(base: &str, left: &str, right: &str, picks: &[Choice]) -> String {
        let m = merged(base, left, right);
        match resolve(&inputs(Some(base), left, right), &choose(&m, picks), NEVER).unwrap() {
            Resolution::Resolved { text, .. } => text,
            other => panic!("not resolved: {other:?}"),
        }
    }

    #[test]
    fn combine_merges_disjoint_changes_and_keeps_deletions() {
        let base = lines("a\nb\nc\nd\n");
        let left = lines("a\nc\nd\n");
        let right = lines("a\nb\nd\n");
        assert_eq!(combine(&base, &left, &right).unwrap().concat(), "a\nd\n");
    }

    #[test]
    fn combine_refuses_overlaps_and_ambiguous_repeated_lines() {
        assert!(combine(&lines("a\nb\n"), &lines("a\nleft\n"), &lines("a\nright\n")).is_none());
        assert!(combine(&lines("a\nx\na\n"), &lines("x\na\n"), &lines("a\nx\ny\n")).is_none());
        assert_eq!(
            combine(&lines("a\n"), &lines("x\na\n"), &lines("y\na\n"))
                .unwrap()
                .concat(),
            "x\ny\na\n"
        );
    }

    #[test]
    fn combine_preserves_line_endings_and_missing_final_newline() {
        let result = combine(&lines("a\nb\nc"), &lines("a\r\nb\r\nc"), &lines("a\nB\nc"));
        assert_eq!(result.unwrap().concat(), "a\r\nB\nc");
        assert_eq!(
            combine(&lines("a"), &lines("a\nx\n"), &lines("a\ny\n"))
                .unwrap()
                .concat(),
            "a\nx\ny\n"
        );
    }

    #[test]
    fn combine_moved_line_and_concurrent_appends_with_mixed_endings() {
        let base = "header\nold-location\nalpha\nbeta\ngamma\nlast";
        let left = format!("{}\r\nleft-one\r\nleft-two\r\n", base.replace('\n', "\r\n"));
        let right = "header\nalpha\nnew-one\nnew-two\nbeta\nnew-three\nold-location\ngamma\nlast\nright-one\nright-two";
        let result = combine(&lines(base), &lines(&left), &lines(right))
            .unwrap()
            .concat();
        assert_eq!(result.matches("old-location").count(), 1);
        assert!(result.contains("new-one\nnew-two\n"));
        assert!(result.contains("new-three\nold-location\n"));
        assert!(result.ends_with("last\r\nleft-one\r\nleft-two\r\nright-one\nright-two"));
        assert_eq!(text(base, &left, right, &[Choice::Combine]), result);
    }

    #[test]
    fn unavailable_combine_cannot_be_resolved() {
        let ins = inputs(Some("a\n"), "left\n", "right\n");
        let m = merged("a\n", "left\n", "right\n");
        assert!(m
            .chunks
            .iter()
            .find(|c| c.kind == Kind::Conflict)
            .unwrap()
            .combined_lines
            .is_none());
        assert!(matches!(
            resolve(&ins, &choose(&m, &[Choice::Combine]), NEVER).unwrap(),
            Resolution::Unresolved { .. }
        ));
    }

    #[test]
    fn one_sided_and_disjoint_changes_merge_without_a_conflict() {
        let m = merged("a\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\n", "a\nb\nc\nd\nE\n");
        assert!(m.conflicts.is_empty());
        assert_eq!(
            text("a\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\n", "a\nb\nc\nd\nE\n", &[]),
            "a\nB\nc\nd\nE\n"
        );
        let kinds: Vec<Kind> = m.chunks.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, [Kind::Stable, Kind::Left, Kind::Stable, Kind::Right]);
    }

    #[test]
    fn overlapping_edits_that_differ_are_a_conflict_that_stays_open() {
        let m = merged("a\nb\nc\n", "a\nours\nc\n", "a\ntheirs\nc\n");
        assert_eq!(m.conflicts.len(), 1);
        let inputs = inputs(Some("a\nb\nc\n"), "a\nours\nc\n", "a\ntheirs\nc\n");
        match resolve(&inputs, &BTreeMap::new(), NEVER).unwrap() {
            Resolution::Unresolved { conflicts } => assert_eq!(conflicts, m.conflicts),
            other => panic!("{other:?}"),
        }
        let preview = preview(&inputs, &BTreeMap::new(), NEVER).unwrap().unwrap();
        assert_eq!(
            preview.text,
            "a\n<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> theirs\nc\n"
        );
        assert_eq!(preview.unresolved, m.conflicts);
    }

    #[test]
    fn identical_changes_on_both_sides_are_taken_once() {
        let m = merged("a\n", "x\n", "x\n");
        assert_eq!(m.chunks.len(), 1);
        assert_eq!(m.chunks[0].kind, Kind::Same);
        assert_eq!(text("a\n", "x\n", "x\n", &[]), "x\n");
    }

    #[test]
    fn choices_build_the_result_with_left_before_right_and_say_where_each_part_came_from() {
        let (base, left, right) = ("a\nb\nc\nd\ne\n", "a\nO1\nc\nO2\ne\n", "a\nT1\nc\nT2\ne\n");
        let m = merged(base, left, right);
        assert_eq!(m.conflicts.len(), 2);
        assert_eq!(
            text(base, left, right, &[Choice::Left, Choice::Right]),
            "a\nO1\nc\nT2\ne\n"
        );
        assert_eq!(
            text(base, left, right, &[Choice::Both, Choice::Base]),
            "a\nO1\nT1\nc\nd\ne\n"
        );
        let resolved = resolve(
            &inputs(Some(base), left, right),
            &choose(&m, &[Choice::Both, Choice::Right]),
            NEVER,
        )
        .unwrap();
        let Resolution::Resolved { provenance, text } = resolved else {
            panic!()
        };
        assert_eq!(text, "a\nO1\nT1\nc\nT2\ne\n");
        let origins: Vec<Origin> = provenance.iter().map(|s| s.origin).collect();
        assert_eq!(
            origins,
            [
                Origin::Base,
                Origin::ChosenBoth,
                Origin::Base,
                Origin::ChosenRight,
                Origin::Base
            ]
        );
        // The segments cover the result line by line.
        assert_eq!(
            provenance.iter().map(|s| s.len).sum::<usize>(),
            text.lines().count()
        );
        assert_eq!(
            provenance[1],
            Segment {
                chunk: m.conflicts[0].clone(),
                origin: Origin::ChosenBoth,
                start: 1,
                len: 2
            }
        );
    }

    #[test]
    fn a_file_added_on_both_sides_merges_against_an_empty_base_and_a_root_insertion_is_a_chunk_of_its_own(
    ) {
        // No base: different additions conflict, identical ones collapse.
        let added =
            |left: &str, right: &str| match merge(&inputs(None, left, right), NEVER).unwrap() {
                Outcome::Text { merge } => merge,
                other => panic!("{other:?}"),
            };
        let m = added("one\n", "two\n");
        assert_eq!(m.conflicts.len(), 1);
        let ins = inputs(None, "one\n", "two\n");
        let both: BTreeMap<_, _> = m
            .conflicts
            .iter()
            .cloned()
            .map(|id| (id, Choice::Both))
            .collect();
        assert!(
            matches!(resolve(&ins, &both, NEVER).unwrap(), Resolution::Resolved { text, .. } if text == "one\ntwo\n")
        );
        assert_eq!(added("same\n", "same\n").chunks[0].kind, Kind::Same);
        // An insertion at the top of one side and an edit further down are separate chunks.
        let m = merged("a\nb\n", "new\na\nb\n", "a\nB\n");
        assert!(m.conflicts.is_empty());
        assert_eq!(text("a\nb\n", "new\na\nb\n", "a\nB\n", &[]), "new\na\nB\n");
        assert_eq!(m.chunks[0].base, Span { start: 0, len: 0 });
    }

    #[test]
    fn touching_changes_of_the_two_sides_are_one_conflict_and_equal_deletions_collapse() {
        // Left deletes b, right deletes c: adjacent, so one chunk that differs.
        let m = merged("a\nb\nc\nd\n", "a\nc\nd\n", "a\nb\nd\n");
        assert_eq!(m.conflicts.len(), 1);
        assert_eq!(
            text("a\nb\nc\nd\n", "a\nc\nd\n", "a\nb\nd\n", &[Choice::Both]),
            "a\nc\nb\nd\n"
        );
        // The same deletion on both sides is taken once.
        let m = merged("a\nb\nc\n", "a\nc\n", "a\nc\n");
        assert_eq!(
            m.chunks.iter().map(|c| c.kind).collect::<Vec<_>>(),
            [Kind::Stable, Kind::Same, Kind::Stable]
        );
        assert_eq!(text("a\nb\nc\n", "a\nc\n", "a\nc\n", &[]), "a\nc\n");
        // A change on one side and a deletion of the same line on the other is a conflict.
        let m = merged("a\nb\nc\n", "a\nB\nc\n", "a\nc\n");
        assert_eq!(m.conflicts.len(), 1);
    }

    #[test]
    fn modify_delete_and_binary_stages_are_typed_results_not_text() {
        let some = |s: &'static str| Some(s.as_bytes());
        // Deleted on one side: the stage is missing, said by name.
        let gone = Inputs {
            base: some("a\n"),
            left: None,
            right: some("b\n"),
        };
        assert_eq!(
            merge(&gone, NEVER).unwrap(),
            Outcome::Unavailable {
                stage: Stage::Left,
                reason: Unavailable::Missing
            }
        );
        let gone = Inputs {
            base: some("a\n"),
            left: some("b\n"),
            right: None,
        };
        assert_eq!(
            merge(&gone, NEVER).unwrap(),
            Outcome::Unavailable {
                stage: Stage::Right,
                reason: Unavailable::Missing
            }
        );
        // A NUL byte, or bytes that are not UTF-8, make a stage binary: the first one found is named.
        let nul = Inputs {
            base: some("a\n"),
            left: Some(b"a\0b".as_slice()),
            right: some("a\n"),
        };
        assert_eq!(
            merge(&nul, NEVER).unwrap(),
            Outcome::Binary { stage: Stage::Left }
        );
        let latin = Inputs {
            base: some("a\n"),
            left: some("a\n"),
            right: Some([0x61, 0xFF, 0x0A].as_slice()),
        };
        assert_eq!(
            merge(&latin, NEVER).unwrap(),
            Outcome::Binary {
                stage: Stage::Right
            }
        );
        // Nothing can be saved for them either.
        assert!(matches!(
            resolve(&nul, &BTreeMap::new(), NEVER).unwrap(),
            Resolution::Binary { stage: Stage::Left }
        ));
        assert!(matches!(
            resolve(&gone, &BTreeMap::new(), NEVER).unwrap(),
            Resolution::Unavailable { .. }
        ));
        assert!(preview(&nul, &BTreeMap::new(), NEVER).unwrap().is_none());
    }

    #[test]
    fn a_stage_that_is_too_large_is_unavailable() {
        let big = vec![b'a'; MAX_STAGE_BYTES + 1];
        let input = Inputs {
            base: Some(b"a\n".as_slice()),
            left: Some(big.as_slice()),
            right: Some(b"a\n".as_slice()),
        };
        assert_eq!(
            merge(&input, NEVER).unwrap(),
            Outcome::Unavailable {
                stage: Stage::Left,
                reason: Unavailable::TooLarge
            }
        );
    }

    #[test]
    fn crlf_and_a_missing_final_newline_survive_byte_for_byte() {
        // CRLF everywhere: lines and the result keep their \r\n.
        // (The two edits are apart: edits on neighbouring lines touch and make a conflict.)
        let (base, left, right) = (
            "a\r\nb\r\nc\r\nd\r\n",
            "a\r\nB\r\nc\r\nd\r\n",
            "a\r\nb\r\nc\r\nD\r\n",
        );
        assert_eq!(text(base, left, right, &[]), "a\r\nB\r\nc\r\nD\r\n");
        // A conflict joined with both in a CRLF file ends the first part with CRLF, not LF.
        let m = merged("a\r\nb\r\n", "a\r\nL", "a\r\nR\r\n");
        assert_eq!(m.conflicts.len(), 1);
        assert_eq!(
            text("a\r\nb\r\n", "a\r\nL", "a\r\nR\r\n", &[Choice::Both]),
            "a\r\nL\r\nR\r\n"
        );
        // No final newline on any side, ours and theirs both change the last line.
        assert_eq!(
            text("a\nb", "a\nours", "a\ntheirs", &[Choice::Both]),
            "a\nours\ntheirs"
        );
        assert_eq!(
            text("a\nb", "a\nours", "a\ntheirs", &[Choice::Left]),
            "a\nours"
        );
        // A file that gains a final newline on one side only: that is its change.
        assert_eq!(text("a\nb", "a\nb\n", "a\nb", &[]), "a\nb\n");
        // Empty inputs.
        assert_eq!(text("", "", "", &[]), "");
        let preview = preview(
            &inputs(Some("a\r\nb\r\n"), "a\r\nL\r\n", "a\r\nR\r\n"),
            &BTreeMap::new(),
            NEVER,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            preview.text,
            "a\r\n<<<<<<< ours\r\nL\r\n=======\r\nR\r\n>>>>>>> theirs\r\n"
        );
    }

    #[test]
    fn spans_follow_each_source_and_ids_are_stable_for_the_same_inputs() {
        let (base, left, right) = ("a\nb\nc\nd\n", "x\ny\na\nB\nc\nd\n", "a\nb\nc\nD\n");
        let m = merged(base, left, right);
        // Left added two lines at the top, so its spans are shifted by two against the base.
        let changed: Vec<&Chunk> = m.chunks.iter().filter(|c| c.kind != Kind::Stable).collect();
        assert_eq!(changed.len(), 3);
        assert_eq!(changed[0].base, Span { start: 0, len: 0 });
        assert_eq!(changed[0].left, Span { start: 0, len: 2 });
        assert_eq!(changed[1].base, Span { start: 1, len: 1 });
        assert_eq!(changed[1].left, Span { start: 3, len: 1 });
        assert_eq!(changed[2].right, Span { start: 3, len: 1 });
        // The spans of all chunks tile every source from its first line to its last.
        for (len, pick) in [(4usize, 0), (6, 1), (4, 2)] {
            let total: usize = m
                .chunks
                .iter()
                .map(|c| [c.base, c.left, c.right][pick].len)
                .sum();
            assert_eq!(total, len);
        }
        // The same inputs give the same IDs, and they differ between chunks.
        let again = merged(base, left, right);
        assert_eq!(m, again);
        let ids: std::collections::BTreeSet<&String> = m.chunks.iter().map(|c| &c.id).collect();
        assert_eq!(ids.len(), m.chunks.len());
        // A conflict keeps its ID when something elsewhere in a side changes.
        let a = merged("a\nb\nc\nd\n", "a\nL\nc\nd\n", "a\nR\nc\nd\n");
        let b = merged("a\nb\nc\nd\n", "a\nL\nc\nd\nmore\n", "a\nR\nc\nd\n");
        assert_eq!(a.conflicts, b.conflicts);
    }

    #[test]
    fn choices_that_belong_to_other_inputs_are_refused_and_nothing_is_saved_without_all_of_them() {
        let (base, left, right) = ("a\nb\nc\nd\ne\n", "a\nO1\nc\nO2\ne\n", "a\nT1\nc\nT2\ne\n");
        let m = merged(base, left, right);
        let ins = inputs(Some(base), left, right);
        // One of two conflicts decided: still not a result.
        let one: BTreeMap<_, _> = m
            .conflicts
            .iter()
            .take(1)
            .cloned()
            .map(|id| (id, Choice::Left))
            .collect();
        match resolve(&ins, &one, NEVER).unwrap() {
            Resolution::Unresolved { conflicts } => {
                assert_eq!(conflicts, vec![m.conflicts[1].clone()])
            }
            other => panic!("{other:?}"),
        }
        // An ID that is no conflict of these inputs.
        let mut stale = choose(&m, &[Choice::Left, Choice::Right]);
        stale.insert("c000000000000".into(), Choice::Base);
        assert!(resolve(&ins, &stale, NEVER).is_err());
        // The unfinished preview marks only what is open.
        let p = preview(&ins, &one, NEVER).unwrap().unwrap();
        assert_eq!(p.unresolved, vec![m.conflicts[1].clone()]);
        assert!(p
            .text
            .starts_with("a\nO1\nc\n<<<<<<< ours\nO2\n=======\nT2\n>>>>>>> theirs\ne\n"));
    }

    #[test]
    fn cancelling_stops_the_work_and_leaves_the_inputs_as_they_were() {
        let base = "a\nb\nc\n".repeat(3000);
        let left = base.replace("b\n", "B\n");
        let right = base.replace("c\n", "C\n");
        let copy = (base.clone(), left.clone(), right.clone());
        let flag = AtomicBool::new(false);
        let cancel: &dyn Fn() -> bool = &|| flag.load(Ordering::Relaxed);
        assert!(merge(&inputs(Some(&base), &left, &right), cancel).is_ok());
        flag.store(true, Ordering::Relaxed);
        assert!(matches!(
            merge(&inputs(Some(&base), &left, &right), cancel),
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            resolve(
                &inputs(Some(&base), &left, &right),
                &BTreeMap::new(),
                cancel
            ),
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            preview(
                &inputs(Some(&base), &left, &right),
                &BTreeMap::new(),
                cancel
            ),
            Err(Error::Cancelled)
        ));
        // The inputs are only borrowed: they are what they were.
        assert_eq!(copy, (base, left, right));
        // Cancelling in the middle: the flag is raised while the chunks are built.
        let calls = std::cell::Cell::new(0usize);
        let later: &dyn Fn() -> bool = &|| {
            calls.set(calls.get() + 1);
            calls.get() > 3
        };
        assert!(matches!(
            merge(&inputs(Some(&copy.0), &copy.1, &copy.2), later),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn a_large_file_has_few_chunks_and_the_result_is_every_line() {
        let numbered: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        let left = numbered.replace("line 2500\n", "ours\n");
        let right = numbered.replace("line 2500\n", "theirs\n");
        let m = merged(&numbered, &left, &right);
        assert_eq!(m.conflicts.len(), 1);
        assert_eq!(m.chunks.len(), 3);
        assert_eq!(text(&numbered, &left, &right, &[Choice::Left]), left);
        assert_eq!(text(&numbered, &left, &right, &[Choice::Right]), right);
        assert_eq!(text(&numbered, &left, &right, &[Choice::Base]), numbered);
    }

    #[test]
    fn json_entry_merges_resolves_and_reports_bad_requests() {
        let (b, l, r) = ("a\nb\nc\n", "a\nours\nc\n", "a\ntheirs\nc\n");
        let never = || false;
        let open = merge_json(b.as_bytes(), l.as_bytes(), r.as_bytes(), "", &never);
        let value: serde_json::Value = serde_json::from_str(&open).unwrap();
        assert_eq!(value["kind"], "text");
        let id = value["merge"]["conflicts"][0].as_str().unwrap().to_string();
        let request = format!(r#"{{"choices":{{"{id}":"left"}}}}"#);
        let done = merge_json(b.as_bytes(), l.as_bytes(), r.as_bytes(), &request, &never);
        assert!(
            done.contains(r#""kind":"resolved""#) && done.contains("ours"),
            "{done}"
        );
        let missing = merge_json(
            b.as_bytes(),
            l.as_bytes(),
            b"",
            r#"{"missing":["right"]}"#,
            &never,
        );
        assert!(missing.contains(r#""kind":"unavailable""#), "{missing}");
        assert!(merge_json(b"", b"", b"", "{", &never).contains(r#""kind":"error""#));
        assert_eq!(
            merge_json(b.as_bytes(), l.as_bytes(), r.as_bytes(), "", &|| true),
            r#"{"kind":"cancelled"}"#
        );
    }
}
