//! File comparison: line diff with intra-line (word) highlights for text, a
//! byte-level verdict for binary files, and unified patches. Lines are compared
//! with `imara-diff` (histogram + indent heuristic, as git does); changed lines
//! are paired by similarity and words compared with `similar`. Sources can be
//! files, the clipboard text or a file's version in Git HEAD.

use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag, TextDiff};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::ignorerules::Rules;
use super::structural::{self, Mark, Syntax};
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
    /// A repository-relative path in a fixed commit, independent of its worktree spelling.
    GitBlob {
        root: PathBuf,
        file: String,
        commit: String,
    },
    /// Text supplied by the caller (e.g. the clipboard, read by the app).
    Text {
        label: String,
        text: String,
    },
}

/// Which whitespace differences between two lines do not count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Whitespace {
    /// Every difference counts.
    #[default]
    None,
    /// Indentation does not count.
    Leading,
    /// Whitespace at the end of a line does not count (a line ending is not whitespace here).
    Trailing,
    /// Runs of whitespace are one space and the ends of a line do not count.
    All,
}

/// How finely a changed line is marked inside.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    /// The line is changed as a whole; nothing is marked inside it.
    Line,
    /// Changed words (and the differing characters of a similar word).
    #[default]
    Word,
    /// Changed characters, as Unicode grapheme clusters: a letter with its combining marks is one.
    Char,
}

/// Lines (both together, in bytes) up to which the character view runs; longer ones are marked by
/// word, which is much cheaper.
const MAX_CHAR_VIEW: usize = 20_000;

impl DiffOptions {
    /// The mode in force, including the old boolean setting.
    pub fn whitespace_mode(&self) -> Whitespace {
        if self.ignore_whitespace {
            Whitespace::All
        } else {
            self.whitespace
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiffOptions {
    /// Settings saved before `whitespace` existed: `true` means `Whitespace::All`.
    pub ignore_whitespace: bool,
    /// Which whitespace differences are ignored; `ignore_whitespace` overrides it with `All`.
    pub whitespace: Whitespace,
    pub ignore_case: bool,
    /// Treat CRLF and LF as equal (on by default in the UI).
    pub ignore_eol: bool,
    /// Minimal diff (Myers): the fewest changed lines. Off: histogram diff,
    /// which reads better.
    pub minimal: bool,
    /// How finely changed lines are marked inside.
    pub granularity: Granularity,
    /// Ignore comments (as the language of the files defines them; plain text has none, and where
    /// the lexer is not sure the text is read literally, so nothing is ignored by mistake).
    pub ignore_comments: bool,
    /// Regular expressions: a line that one matches is ignored (compares as an empty line).
    pub ignore_lines: Vec<String>,
    /// Regular expressions: the text one matches is ignored inside a line.
    pub ignore_text: Vec<String>,
    /// Compare changed lines by their tokens: a change of layout alone (line breaks,
    /// indentation, alignment) is told apart from a change of content.
    pub structural: bool,
    /// Also find moved blocks that were edited on the way (opt-in): anchored by lines that
    /// are unique in both files, never by repeated boilerplate. Ambiguous candidates stay
    /// ordinary removed and added lines.
    pub moves_edited: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    pub label: String,
    pub path: Option<PathBuf>,
    pub size: u64,
    pub crlf: bool,
    pub final_newline: bool,
    /// Size and modification time of a file when it was read (empty for anything else): a save
    /// that carries it is refused when the file changed on disk meanwhile.
    pub revision: String,
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
    /// The two sides hold the same tokens: only the layout differs (structural diff).
    pub cosmetic: bool,
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
    /// Structural diff: lines of changed blocks by what changed on them. An empty list:
    /// nothing, only the layout differs; else the changed ranges. A line not listed
    /// changed as a whole (or was compared by words, see `pairs`).
    pub left_marks: BTreeMap<usize, Spans>,
    pub right_marks: BTreeMap<usize, Spans>,
}

pub use gall_compare::lines::Move;

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
    load_comparison_input(src, false)
}

fn load_comparison_input(src: &DiffSource, bounded: bool) -> Result<Loaded> {
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
                revision: super::ops::file_revision(path).unwrap_or_default(),
            };
            if size > MAX_TEXT_BYTES {
                return Ok(Loaded {
                    side,
                    bytes: Vec::new(),
                    stream: Some(path.clone()),
                });
            }
            let mut side = side;
            let mut bytes = Vec::new();
            if bounded {
                super::open_regular(path)
                    .ctx("open", path)?
                    .take(MAX_TEXT_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .ctx("read", path)?;
                side.size = bytes.len() as u64;
            } else {
                bytes = super::read_regular(path).ctx("read", path)?;
            }
            Ok(Loaded {
                side,
                bytes,
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
            let (size, bytes) = if bounded {
                bounded_git_revision_bytes(path, revision)?
            } else {
                let bytes = git_revision_bytes(path, revision)?;
                (bytes.len() as u64, bytes)
            };
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
                    size,
                    crlf: false,
                    final_newline: false,
                    revision: String::new(),
                },
                bytes,
                stream: None,
            })
        }
        DiffSource::GitBlob { root, file, commit } => {
            super::gitinspect::relative_file(file)?;
            let commit = super::gitinspect::commit_id(root, commit)?;
            let object = format!("{commit}:{file}");
            let (size, bytes) = if bounded {
                bounded_git_object_bytes(root, &object)?
            } else {
                let bytes = git(root, &["cat-file", "blob", &object])?;
                (bytes.len() as u64, bytes)
            };
            Ok(Loaded {
                side: Side {
                    label: format!("{file} ({})", &commit[..12]),
                    path: None,
                    size,
                    crlf: false,
                    final_newline: false,
                    revision: String::new(),
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
                revision: String::new(),
            },
            bytes: if bounded && text.len() as u64 > MAX_TEXT_BYTES {
                Vec::new()
            } else {
                text.as_bytes().to_vec()
            },
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
    compare_with_cancel(a, b, opts, &AtomicBool::new(false))
}

/// `compare` that ends with `CoreError::Cancelled` soon after `cancel` is set, also in the middle of a large
/// comparison (a report export is cancelled while its current pair is still running).
pub fn compare_with_cancel(
    a: &DiffSource,
    b: &DiffSource,
    opts: DiffOptions,
    cancel: &AtomicBool,
) -> Result<DiffResult> {
    let stopped = || {
        if cancel.load(Ordering::Relaxed) {
            Err(CoreError::Cancelled)
        } else {
            Ok(())
        }
    };
    stopped()?;
    // A rule that does not compile is refused with its reason, not left out of the comparison.
    Rules::compile(&opts.ignore_lines, &opts.ignore_text).map_err(CoreError::Other)?;
    let mut la = load(a)?;
    stopped()?;
    let mut lb = load(b)?;
    stopped()?;
    if la.stream.is_some() || lb.stream.is_some() {
        let bin =
            compare_streams_with_cancel(&la, &lb, "file too large for a text comparison", cancel)?;
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
            // The language of the newer side, else of the older one.
            let syntax = Syntax::for_names(&[&lb.side.label, &la.side.label]);
            // This form cannot report an invalid rule (refused above), and ends early when cancelled.
            let text = diff_text_with_cancel(ta, tb, opts, syntax, true, cancel)?;
            Ok(DiffResult {
                left: la.side,
                right: lb.side,
                text: Some(text),
                binary: None,
            })
        }
        _ => {
            let bin = compare_streams_with_cancel(&la, &lb, "binary content", cancel)?;
            Ok(DiffResult {
                left: la.side,
                right: lb.side,
                text: None,
                binary: Some(bin),
            })
        }
    }
}

/// Metadata and immutable text data for the paged comparison API.
pub struct PagedDiff {
    pub left: Side,
    pub right: Side,
    pub text: Option<crate::compare::Comparison>,
    pub binary: Option<BinaryDiffResult>,
    pub limit: Option<crate::compare::InputError>,
}

pub fn compare_paged(
    a: &DiffSource,
    b: &DiffSource,
    opts: DiffOptions,
    cancel: &AtomicBool,
) -> Result<PagedDiff> {
    use crate::compare::{Comparison, InputError};
    let check = || {
        if cancel.load(Ordering::Relaxed) {
            Err(CoreError::Cancelled)
        } else {
            Ok(())
        }
    };
    check()?;
    // An invalid rule is an error with its reason here, not a comparison that looks cancelled.
    Rules::compile(&opts.ignore_lines, &opts.ignore_text).map_err(CoreError::Other)?;
    let mut left = load_comparison_input(a, true)?;
    check()?;
    let mut right = load_comparison_input(b, true)?;
    check()?;
    let (text, binary, limit) =
        if left.side.size > MAX_TEXT_BYTES || right.side.size > MAX_TEXT_BYTES {
            (None, None, Some(InputError::TooManyBytes))
        } else {
            match Comparison::new(
                &left.bytes,
                &right.bytes,
                &[&right.side.label, &left.side.label],
                opts,
                cancel,
            ) {
                Ok(text) => {
                    left.side.crlf = text.left.crlf;
                    left.side.final_newline = text.left.final_newline;
                    right.side.crlf = text.right.crlf;
                    right.side.final_newline = text.right.final_newline;
                    (Some(text), None, None)
                }
                Err(InputError::Cancelled) => return Err(CoreError::Cancelled),
                Err(InputError::Binary) => (
                    None,
                    Some(compare_streams_with_cancel(
                        &left,
                        &right,
                        "binary content",
                        cancel,
                    )?),
                    None,
                ),
                Err(limit) => (None, None, Some(limit)),
            }
        };
    check()?;
    Ok(PagedDiff {
        left: left.side,
        right: right.side,
        text,
        binary,
        limit,
    })
}

/// Byte comparison; streams from disk for files that were not loaded.
fn compare_streams(a: &Loaded, b: &Loaded, reason: &str) -> Result<BinaryDiffResult> {
    compare_streams_with_cancel(a, b, reason, &AtomicBool::new(false))
}

fn compare_streams_with_cancel(
    a: &Loaded,
    b: &Loaded,
    reason: &str,
    cancel: &AtomicBool,
) -> Result<BinaryDiffResult> {
    fn reader(l: &Loaded) -> Result<Box<dyn Read + '_>> {
        Ok(match &l.stream {
            Some(p) => Box::new(std::io::BufReader::new(
                super::open_regular(p).ctx("open", p)?,
            )),
            None => Box::new(std::io::Cursor::new(l.bytes.as_slice())),
        })
    }
    let (mut ra, mut rb) = (reader(a)?, reader(b)?);
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    let mut off: u64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
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

/// The lines of a text with their `\r` kept, and without the empty piece after a final `\n`.
fn raw_lines(t: &str) -> Vec<&str> {
    let mut v: Vec<&str> = t.split('\n').collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop(); // trailing newline is not an extra empty line
    }
    v
}

/// The display lines. The final piece is dropped before its `\r` is stripped, so a last line that
/// is only a `\r` stays one (an input of one `\r` is one empty line, not none).
fn split_lines(t: &str) -> Vec<String> {
    raw_lines(t)
        .into_iter()
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect()
}

/// `line` without the byte ranges (sorted, not overlapping, on character boundaries). A `\r` that
/// ends a raw line lies outside them and stays.
fn without_ranges(line: &str, ranges: &[(usize, usize)]) -> String {
    let mut kept = String::with_capacity(line.len());
    let mut at = 0;
    for &(start, end) in ranges {
        if start >= at && end <= line.len() {
            kept.push_str(&line[at..start]);
            at = end;
        }
    }
    kept.push_str(&line[at..]);
    kept
}

fn normalize(line: &str, raw: &str, opts: &DiffOptions, rules: &Rules) -> String {
    if rules.ignores_line(line) {
        return String::new();
    }
    let mut s = if opts.ignore_eol {
        line.to_string()
    } else {
        raw.to_string()
    };
    if !rules.is_empty() {
        s = rules.strip(&s);
    }
    s = match opts.whitespace_mode() {
        Whitespace::None => s,
        Whitespace::All => s.split_whitespace().collect::<Vec<_>>().join(" "),
        Whitespace::Leading => s.trim_start().to_string(),
        Whitespace::Trailing => {
            // A line ending that is compared (`ignore_eol` off) is not whitespace of the line.
            match s.strip_suffix('\r') {
                Some(rest) => format!("{}\r", rest.trim_end()),
                None => s.trim_end().to_string(),
            }
        }
    };
    if opts.ignore_case {
        s = s.to_lowercase();
    }
    s
}

pub fn diff_text(a: &str, b: &str, opts: DiffOptions) -> TextDiffResult {
    diff_text_as(a, b, opts, Syntax::for_names(&[]))
}

/// `diff_text` with the token rules of the files' language, for `opts.structural`.
pub fn diff_text_as(a: &str, b: &str, mut opts: DiffOptions, syntax: Syntax) -> TextDiffResult {
    // This form cannot report an error: rules that do not compile are left out here, while the
    // callers that return a `Result` refuse them with the reason.
    if Rules::compile(&opts.ignore_lines, &opts.ignore_text).is_err() {
        opts.ignore_lines.clear();
        opts.ignore_text.clear();
    }
    diff_text_with_cancel(a, b, opts, syntax, true, &AtomicBool::new(false))
        .expect("the legacy comparison has no cancellation source")
}

pub(crate) fn diff_text_with_cancel(
    a: &str,
    b: &str,
    opts: DiffOptions,
    syntax: Syntax,
    detailed: bool,
    cancel: &AtomicBool,
) -> Result<TextDiffResult> {
    let check = || {
        if cancel.load(Ordering::Relaxed) {
            Err(CoreError::Cancelled)
        } else {
            Ok(())
        }
    };
    check()?;
    let rules = Rules::compile(&opts.ignore_lines, &opts.ignore_text).map_err(CoreError::Other)?;
    let left = split_lines(a);
    let right = split_lines(b);
    // Raw lines keep "\r" so CRLF vs LF counts as a change unless ignore_eol.
    let raw = |t: &str| -> Vec<String> { raw_lines(t).into_iter().map(str::to_string).collect() };
    let (ra, rb) = (raw(a), raw(b));
    // With comments ignored the lines that are compared lack them; the lines shown stay whole.
    let (ra_cmp, rb_cmp, left_cmp, right_cmp);
    let (compared_ra, compared_rb, compared_left, compared_right) = if opts.ignore_comments {
        let cut = |lines: &[String], comments: &[Vec<(usize, usize)>]| -> Vec<String> {
            lines
                .iter()
                .zip(comments)
                .map(|(line, ranges)| without_ranges(line, ranges))
                .collect()
        };
        let (ca, cb) = (
            structural::comment_ranges(&left, syntax),
            structural::comment_ranges(&right, syntax),
        );
        left_cmp = cut(&left, &ca);
        right_cmp = cut(&right, &cb);
        ra_cmp = cut(&ra, &ca);
        rb_cmp = cut(&rb, &cb);
        (&ra_cmp, &rb_cmp, &left_cmp, &right_cmp)
    } else {
        (&ra, &rb, &left, &right)
    };
    let na: Vec<String> = compared_left
        .iter()
        .zip(compared_ra)
        .map(|(l, r)| normalize(l, r, &opts, &rules))
        .collect();
    let nb: Vec<String> = compared_right
        .iter()
        .zip(compared_rb)
        .map(|(l, r)| normalize(l, r, &opts, &rules))
        .collect();

    check()?;
    let hunks = gall_compare::lines::line_hunks(&na, &nb, opts.minimal);
    check()?;
    let moves = if detailed {
        let mut moves = gall_compare::lines::find_moves(&na, &nb, &hunks);
        if opts.moves_edited {
            // An edited block swallows the exact moves inside it (their lines anchor it).
            let edited = gall_compare::lines::find_edited_moves(&na, &nb, &hunks);
            moves.retain(|m| {
                edited.iter().all(|e| {
                    (m.a + m.len <= e.a || e.a + e.len <= m.a)
                        && (m.b + m.b_len <= e.b || e.b + e.b_len <= m.b)
                })
            });
            moves.extend(edited);
        }
        moves.sort_by_key(|m| (m.a, m.b));
        for (k, m) in moves.iter_mut().enumerate() {
            m.id = k + 1;
        }
        moves
    } else {
        Vec::new()
    };
    let (mut moved_a, mut moved_b) = (vec![false; na.len()], vec![false; nb.len()]);
    for m in &moves {
        moved_a[m.a..m.a + m.len].fill(true);
        moved_b[m.b..m.b + m.b_len].fill(true);
    }

    check()?;
    let mut blocks = if detailed && opts.structural {
        structural::compare_hunks(
            &left,
            &right,
            &hunks,
            syntax,
            opts.whitespace_mode() == Whitespace::All,
            opts.ignore_case,
        )
    } else {
        Vec::new()
    };
    if !opts.ignore_eol {
        for (block, &(a0, a1, b0, b1)) in blocks.iter_mut().zip(&hunks) {
            // Tokenization uses display lines without CR. Keep a literal block when
            // the user explicitly requests comparing its CRLF line endings.
            if ra[a0..a1]
                .iter()
                .chain(&rb[b0..b1])
                .any(|line| line.ends_with('\r'))
            {
                *block = None;
            }
        }
    }
    let (mut left_marks, mut right_marks) = (BTreeMap::new(), BTreeMap::new());
    let note = |marks: &mut BTreeMap<usize, Spans>, first: usize, of: Vec<Mark>| {
        for (k, mark) in of.into_iter().enumerate() {
            match mark {
                Mark::Whole => {}
                Mark::Same => drop(marks.insert(first + k, Vec::new())),
                Mark::Part(spans) => drop(marks.insert(first + k, spans)),
            }
        }
    };

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
                cosmetic: false,
            });
        }
    };
    for (n, (a0, a1, b0, b1)) in hunks.into_iter().enumerate() {
        check()?;
        equal(&mut ops, pa, pb, a0 - pa);
        let (al, bl) = (a1 - a0, b1 - b0);
        // Compared by tokens: the marks say what changed on each line.
        let block = blocks.get_mut(n).and_then(Option::take);
        let by_tokens = block.is_some();
        let cosmetic = block.as_ref().is_some_and(|b| !b.changed);
        if let Some(block) = block {
            note(&mut left_marks, a0, block.a);
            note(&mut right_marks, b0, block.b);
        }
        if !cosmetic {
            removed += al;
            added += bl;
        }
        let tag = match (al, bl) {
            (_, 0) => "delete",
            (0, _) => "insert",
            _ => "replace",
        };
        if detailed && tag == "replace" {
            // Moved lines are not edits of their neighbours: leave them out of pairing.
            let ia: Vec<usize> = (a0..a1).filter(|&i| !moved_a[i]).collect();
            let ib: Vec<usize> = (b0..b1).filter(|&j| !moved_b[j]).collect();
            let la: Vec<String> = ia.iter().map(|&i| left[i].clone()).collect();
            let lb: Vec<String> = ib.iter().map(|&j| right[j].clone()).collect();
            for (i, j) in pair_lines(&la, &lb) {
                check()?;
                let (a, b) = (ia[i], ib[j]);
                let (sa, sb) = if by_tokens {
                    let of = |marks: &BTreeMap<usize, Spans>, line| {
                        marks.get(&line).cloned().unwrap_or_default()
                    };
                    (of(&left_marks, a), of(&right_marks, b))
                } else {
                    inline_spans(opts.granularity, &left[a], &right[b])
                };
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
            cosmetic,
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
    check()?;
    Ok(TextDiffResult {
        left_lines: left,
        right_lines: right,
        ops,
        pairs,
        added,
        removed,
        ratio,
        moves,
        left_marks,
        right_marks,
    })
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

/// The changed ranges of two lines at the chosen granularity, in UTF-16 offsets.
fn inline_spans(granularity: Granularity, a: &str, b: &str) -> (Spans, Spans) {
    match granularity {
        Granularity::Line => (Vec::new(), Vec::new()),
        Granularity::Word => word_spans(a, b),
        Granularity::Char if a.len() + b.len() <= MAX_CHAR_VIEW => grapheme_spans(a, b),
        Granularity::Char => word_spans(a, b),
    }
}

/// Changed grapheme clusters of two lines, in UTF-16 offsets. Unlike the word view nothing is
/// joined: every cluster that differs is marked, and only those.
fn grapheme_spans(a: &str, b: &str) -> (Spans, Spans) {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_graphemes(a, b);
    let (mut sa, mut sb): (Spans, Spans) = (Vec::new(), Vec::new());
    let (mut pa, mut pb) = (0usize, 0usize);
    for change in diff.iter_all_changes() {
        let width = utf16(change.value());
        match change.tag() {
            similar::ChangeTag::Equal => {
                pa += width;
                pb += width;
            }
            similar::ChangeTag::Delete => {
                push_span(&mut sa, pa, pa + width);
                pa += width;
            }
            similar::ChangeTag::Insert => {
                push_span(&mut sb, pb, pb + width);
                pb += width;
            }
        }
    }
    (sa, sb)
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

pub(super) fn push_span(v: &mut Spans, s: usize, e: usize) {
    if s >= e {
        return;
    }
    match v.last_mut() {
        Some(last) if last.1 >= s => last.1 = last.1.max(e),
        _ => v.push((s, e)),
    }
}

/// Joins spans separated by whitespace only or by a single character.
pub(super) fn join_gaps(spans: Spans, line: &str) -> Spans {
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

/// The unified patch of `a` → `b` ([`export_patch`] in the default format).
pub fn unified_patch(a: &DiffSource, b: &DiffSource) -> Result<String> {
    export_patch(a, b, super::patch::Format::Unified)
}

/// The patch of `a` → `b` in a format. The sources are the ones the compare view shows now, a draft with unsaved edits
/// included. Header names are portable: the path inside the repository for a Git blob or a file of its working tree,
/// otherwise the name of the file (never an absolute path or a folder of this computer), prefixed `a/` and `b/`.
pub fn export_patch(
    a: &DiffSource,
    b: &DiffSource,
    format: super::patch::Format,
) -> Result<String> {
    let (la, lb) = (load(a)?, load(b)?);
    let (Some(ta), Some(tb)) = (as_text(&la.bytes), as_text(&lb.bytes)) else {
        return Err(CoreError::Other("a patch needs two text files".into()));
    };
    let repository = [a, b].into_iter().find_map(|source| match source {
        DiffSource::GitBlob { root, .. } => Some(root),
        _ => None,
    });
    let name = |source: &DiffSource, l: &Loaded| {
        if let DiffSource::GitBlob { file, .. } = source {
            return file.to_owned();
        }
        if let (Some(root), DiffSource::File { path }) = (repository, source) {
            if let Ok(relative) = path.strip_prefix(root) {
                return relative
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
            }
        }
        l.side
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| l.side.label.clone())
    };
    let header = |prefix: &str, source: &DiffSource, l: &Loaded| {
        format!("{prefix}/{}", name(source, l).trim_start_matches('/'))
    };
    let patch =
        super::patch::make_patch(&header("a", a, &la), &header("b", b, &lb), ta, tb, format);
    Ok(super::patch::write(format, &[patch]))
}

// ───────────────────────────── git ─────────────────────────────

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    // core.fsmonitor=false: never start a background fsmonitor daemon in the user's
    // repositories (users with core.fsmonitor=true would otherwise collect one per repo).
    // `command` also keeps a console window from flashing on Windows (FolderPane calls
    // git_root on every navigation) and runs Git inside WSL for repositories there.
    let mut cmd = super::gitexec::command(dir, super::gitstatus::program());
    cmd.args(["-c", "core.fsmonitor=false", "-C"])
        .arg(super::gitexec::dir_arg(dir))
        .args(args);
    super::gitexec::forward_env(&mut cmd);
    let out = match super::gitexec::run(&cmd, std::time::Duration::from_secs(120)) {
        Some(out) => out,
        None => cmd.output(),
    }
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
    // Git is asked once per folder (see `gitfacts::root`).
    super::gitfacts::root(dir, || {
        let out = git(dir, &["rev-parse", "--show-toplevel"]).ok()?;
        let s = String::from_utf8_lossy(&out).trim().to_string();
        (!s.is_empty()).then(|| super::gitexec::host_path(dir, &s))
    })
}

fn git_revision_object(path: &Path, revision: &str) -> Result<(PathBuf, String)> {
    let root =
        git_root(path).ok_or_else(|| CoreError::Other("not inside a Git repository".into()))?;
    let canon = path.canonicalize().ctx("resolve", path)?;
    let root = root.canonicalize().ctx("resolve", &root)?;
    let rel = canon
        .strip_prefix(&root)
        .map_err(|_| CoreError::Other("file is outside the repository".into()))?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    Ok((root, format!("{revision}:{rel}")))
}

fn git_revision_bytes(path: &Path, revision: &str) -> Result<Vec<u8>> {
    let (root, object) = git_revision_object(path, revision)?;
    git(&root, &["show", &object])
        .map_err(|e| CoreError::Other(format!("file is not in the selected revision: {e}")))
}

/// Whether a source read through Git still exists: the file in HEAD or in the commit, the blob of a commit. Checks
/// the object only (no content is read); other sources are always present here. Used to show a restored tab as
/// missing instead of ready.
pub fn git_source_present(src: &DiffSource) -> bool {
    match src {
        DiffSource::GitHead { path } => git_revision_object(path, "HEAD")
            .and_then(|(root, object)| git(&root, &["rev-parse", "--verify", &object]))
            .is_ok(),
        DiffSource::GitCommit { path, commit } => git_revision_object(path, commit)
            .and_then(|(root, object)| git(&root, &["rev-parse", "--verify", &object]))
            .is_ok(),
        DiffSource::GitBlob { root, file, commit } => {
            super::gitinspect::relative_file(file).is_ok()
                && super::gitinspect::commit_id(root, commit)
                    .and_then(|id| git(root, &["rev-parse", "--verify", &format!("{id}:{file}")]))
                    .is_ok()
        }
        _ => true,
    }
}

fn bounded_git_revision_bytes(path: &Path, revision: &str) -> Result<(u64, Vec<u8>)> {
    let (root, object) = git_revision_object(path, revision)?;
    bounded_git_object_bytes(&root, &object)
}

fn bounded_git_object_bytes(root: &Path, object: &str) -> Result<(u64, Vec<u8>)> {
    let oid = git(root, &["rev-parse", "--verify", object])
        .map_err(|e| CoreError::Other(format!("file is not in the selected revision: {e}")))?;
    let oid = String::from_utf8_lossy(&oid);
    let oid = oid.trim();
    let size = git(root, &["cat-file", "-s", oid])?;
    let size: u64 = String::from_utf8_lossy(&size)
        .trim()
        .parse()
        .map_err(|_| CoreError::Other("invalid Git object size".into()))?;
    if size > MAX_TEXT_BYTES {
        return Ok((size, Vec::new()));
    }
    let bytes = git(root, &["cat-file", "blob", oid])?;
    Ok((size, bytes))
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
    fn a_final_line_that_is_only_a_carriage_return_is_kept() {
        // One `\r` is one empty line, not none, and does not compare equal to nothing.
        assert_eq!(split_lines("\r"), [""]);
        assert_eq!(split_lines("x\n\r"), ["x", ""]);
        assert_eq!(split_lines("x\n"), ["x"]);
        assert_eq!(split_lines(""), Vec::<String>::new());
        assert_eq!(raw_lines("x\n\r").len(), split_lines("x\n\r").len());
        let strict = DiffOptions::default();
        assert!(diff_text("\r", "", strict)
            .ops
            .iter()
            .any(|op| op.tag != "equal"));
        let eol = DiffOptions {
            ignore_eol: true,
            ..DiffOptions::default()
        };
        // With line endings ignored a lone `\r` is an empty line, which still differs from no line.
        assert!(diff_text("\r", "", eol.clone())
            .ops
            .iter()
            .any(|op| op.tag != "equal"));
        assert!(diff_text("x\n\r", "x\n\n", eol)
            .ops
            .iter()
            .all(|op| op.tag == "equal"));
    }

    #[test]
    fn a_comparison_with_the_cancel_flag_set_ends_as_cancelled_and_without_it_equals_compare() {
        let text = |t: &str| DiffSource::Text {
            label: "t".into(),
            text: t.into(),
        };
        let (a, b) = (text("one\ntwo\n"), text("one\nthree\n"));
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            compare_with_cancel(&a, &b, DiffOptions::default(), &cancelled),
            Err(CoreError::Cancelled)
        ));
        let free = AtomicBool::new(false);
        let with = compare_with_cancel(&a, &b, DiffOptions::default(), &free).unwrap();
        let plain = compare(&a, &b, DiffOptions::default()).unwrap();
        assert_eq!(with.text.unwrap().ops.len(), plain.text.unwrap().ops.len());
    }

    #[test]
    fn ignore_rules_hide_matched_lines_and_text_and_invalid_ones_are_refused() {
        let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let options = DiffOptions {
            ignore_lines: strings(&[r"^\s*//"]),
            ignore_text: strings(&[r"\d{4}-\d\d-\d\d"]),
            ..DiffOptions::default()
        };
        let a = "// header 1\nbuilt 2026-11-24\nkeep\n";
        let b = "// another header\nbuilt 2027-01-02\nkeep\n";
        assert!(diff_text(a, b, options.clone())
            .ops
            .iter()
            .all(|op| op.tag == "equal"));
        // What no rule matches still differs, and without rules everything differs.
        assert!(
            !diff_text(a, "built 2026-11-24\nchanged\n", options.clone())
                .ops
                .iter()
                .all(|op| op.tag == "equal")
        );
        assert!(!diff_text(a, b, DiffOptions::default())
            .ops
            .iter()
            .all(|op| op.tag == "equal"));
        // The result with an invalid rule: the call that returns a `Result` says why.
        let bad = DiffOptions {
            ignore_text: strings(&["("]),
            ..DiffOptions::default()
        };
        let error = diff_text_with_cancel(
            a,
            b,
            bad.clone(),
            Syntax::for_names(&[]),
            true,
            &AtomicBool::new(false),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("Invalid ignore rule"), "{error}");
        // The paged comparison of the window refuses it with the reason too (not as "cancelled").
        let text = |t: &str| DiffSource::Text {
            label: "t".into(),
            text: t.into(),
        };
        let error = compare_paged(&text(a), &text(b), bad.clone(), &AtomicBool::new(false))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("Invalid ignore rule"), "{error}");
        // The comparison of two sources (window, report) refuses it too.
        let error = compare(&text(a), &text(b), bad.clone())
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("Invalid ignore rule"), "{error}");
        // The form that cannot fail leaves the rule out instead of panicking.
        assert!(!diff_text(a, b, bad).ops.iter().all(|op| op.tag == "equal"));
    }

    #[test]
    fn ignored_comments_do_not_count_but_comment_marks_inside_strings_do() {
        let options = DiffOptions {
            ignore_comments: true,
            ..DiffOptions::default()
        };
        let rust = || Syntax::for_names(&["a.rs"]);
        let same = |a: &str, b: &str, syntax: Syntax| {
            diff_text_as(a, b, options.clone(), syntax)
                .ops
                .iter()
                .all(|op| op.tag == "equal")
        };
        // A changed trailing comment and a changed block comment do not count.
        assert!(same(
            "let x = 1; // one\n/* a\n b */\nfn f() {}\n",
            "let x = 1; // two\n/* c\n d */\nfn f() {}\n",
            rust()
        ));
        // `//` inside a string literal is code: a change there counts.
        assert!(!same(
            "let s = \"a // one\";\n",
            "let s = \"a // two\";\n",
            rust()
        ));
        // Plain text has no comments, so nothing is ignored.
        assert!(!same(
            "x // one\n",
            "x // two\n",
            Syntax::for_names(&["a.txt"])
        ));
        // Where the lexer is not sure (a shell heredoc) the text is read literally.
        assert!(!same(
            "cat <<EOF\n# one\nEOF\n",
            "cat <<EOF\n# two\nEOF\n",
            Syntax::for_names(&["a.sh"])
        ));
        // The comparison with comments ignored still shows the lines whole.
        let r = diff_text_as(
            "a; // one\nb;\n",
            "a; // two\nc;\n",
            options.clone(),
            rust(),
        );
        assert!(r.ops.iter().any(|op| op.tag != "equal"));
        assert_eq!(r.left_lines.first().map(String::as_str), Some("a; // one"));
    }

    #[test]
    fn the_character_view_marks_whole_grapheme_clusters_and_the_line_view_marks_nothing() {
        // `é` as `e` and a combining accent is one cluster: it is marked whole, never the accent alone.
        let (sa, sb) = inline_spans(Granularity::Char, "cafe\u{301}!", "cafe!");
        assert_eq!((sa, sb), (vec![(3, 5)], vec![(3, 4)]));
        // A flag (two regional indicators, four UTF-16 units) and a family emoji with joiners.
        let (sa, sb) = inline_spans(Granularity::Char, "a\u{1F1F8}\u{1F1EE}b", "ab");
        assert_eq!((sa, sb), (vec![(1, 5)], Vec::new()));
        // Unchanged text is not marked, and changed characters are not joined across a gap.
        let (sa, sb) = inline_spans(Granularity::Char, "a1b2c", "aXbYc");
        assert_eq!((sa, sb), (vec![(1, 2), (3, 4)], vec![(1, 2), (3, 4)]));
        let (sa, sb) = inline_spans(Granularity::Line, "a1", "a2");
        assert!(sa.is_empty() && sb.is_empty());
        // The word view is what it was.
        let (sa, _) = inline_spans(Granularity::Word, "count1 x", "count2 x");
        assert_eq!(sa, vec![(5, 6)]);
        // A very long line is marked by word, not by character, so it cannot take long.
        let long_a = format!("{} x", "word ".repeat(MAX_CHAR_VIEW));
        let long_b = format!("{} y", "word ".repeat(MAX_CHAR_VIEW));
        let (sa, _) = inline_spans(Granularity::Char, &long_a, &long_b);
        assert!(!sa.is_empty());
    }

    #[test]
    fn whitespace_modes_ignore_only_what_they_name() {
        let same = |a: &str, b: &str, mode: Whitespace, eol: bool| {
            let options = DiffOptions {
                whitespace: mode,
                ignore_eol: eol,
                ..DiffOptions::default()
            };
            diff_text(a, b, options)
                .ops
                .iter()
                .all(|op| op.tag == "equal")
        };
        // Leading: indentation only.
        assert!(same("    x = 1\n", "x = 1\n", Whitespace::Leading, true));
        assert!(!same("x = 1  \n", "x = 1\n", Whitespace::Leading, true));
        assert!(!same("x  =  1\n", "x = 1\n", Whitespace::Leading, true));
        // Trailing: the end of the line only.
        assert!(same("x = 1  \n", "x = 1\n", Whitespace::Trailing, true));
        assert!(!same("  x = 1\n", "x = 1\n", Whitespace::Trailing, true));
        // A compared line ending is not trailing whitespace.
        assert!(same("x  \r\n", "x\r\n", Whitespace::Trailing, false));
        assert!(!same("x\r\n", "x\n", Whitespace::Trailing, false));
        // All, and the old boolean setting that means the same.
        assert!(same("  x   =  1  \n", "x = 1\n", Whitespace::All, true));
        let old: DiffOptions = serde_json::from_str(r#"{"ignoreWhitespace":true}"#).unwrap();
        assert_eq!(old.whitespace_mode(), Whitespace::All);
        let new: DiffOptions = serde_json::from_str(r#"{"whitespace":"trailing"}"#).unwrap();
        assert_eq!(new.whitespace_mode(), Whitespace::Trailing);
        assert_eq!(DiffOptions::default().whitespace_mode(), Whitespace::None);
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

    fn edited_opts() -> DiffOptions {
        DiffOptions {
            moves_edited: true,
            ..DiffOptions::default()
        }
    }

    const EDIT_BLOCK: &str = "fn compute_total(items: &[Item]) -> u64 {\n    let mut sum = 0;\n    for item in items {\n        sum += item.price_in_cents();\n    }\n    sum\n}\n";

    /// Twelve distinct lines that stay in place: the block moves across them, so the
    /// line diff has to show it as removed in one place and added in another.
    fn stay(tag: &str) -> String {
        (0..12)
            .map(|k| format!("stay_{tag}_statement_number_{k}();\n"))
            .collect()
    }

    fn edited_pair() -> (String, String) {
        let s = stay("a");
        let a = format!("{EDIT_BLOCK}{s}end_of_file_marker();\n");
        let moved = EDIT_BLOCK.replace(
            "sum += item.price_in_cents();",
            "sum += item.price_with_tax();",
        );
        let b = format!("{s}{moved}end_of_file_marker();\n");
        (a, b)
    }

    #[test]
    fn edited_move_is_found_only_when_asked() {
        let (a, b) = edited_pair();
        let off = diff_text(&a, &b, DiffOptions::default());
        assert!(off.moves.iter().all(|m| !m.edited));
        let on = diff_text(&a, &b, edited_opts());
        let m = on.moves.iter().find(|m| m.edited).expect("an edited move");
        assert_eq!(
            on.left_lines[m.a],
            "fn compute_total(items: &[Item]) -> u64 {"
        );
        assert_eq!(
            on.right_lines[m.b],
            "fn compute_total(items: &[Item]) -> u64 {"
        );
        assert_eq!((m.a, m.len, m.b_len), (0, 7, 7));
        // The edited line (left line 3) lies inside the block, and the exact move of the
        // unchanged head is part of the block, not a second move.
        assert_eq!(on.moves.len(), 1, "{:?}", on.moves);
    }

    #[test]
    fn edited_move_may_change_its_length() {
        let (a, b) = edited_pair();
        let b = b.replace(
            "    let mut sum = 0;\n",
            "    let mut sum = 0;\n    let _debug_note = true;\n",
        );
        let r = diff_text(&a, &b, edited_opts());
        let m = r.moves.iter().find(|m| m.edited).expect("an edited move");
        assert_eq!((m.len, m.b_len), (7, 8), "{m:?}");
    }

    #[test]
    fn edited_blocks_in_place_are_no_moves() {
        let a = format!("head_line_one\n{EDIT_BLOCK}tail_line_one\n");
        let b = format!(
            "head_line_one\n{}tail_line_one\n",
            EDIT_BLOCK.replace(
                "sum += item.price_in_cents();",
                "sum += item.price_with_tax();"
            )
        );
        let r = diff_text(&a, &b, edited_opts());
        assert!(r.moves.is_empty(), "{:?}", r.moves);
    }

    #[test]
    fn repeated_lines_never_anchor_a_move() {
        // Every substantial line occurs twice on the right: ambiguous, so plain del/add.
        let block = "let first_value = compute_first();\nlet second_value = compute_second();\nlet third_value = compute_third();\n";
        let a = format!("start_of_file_marker\n{block}middle_of_file_marker\nend_of_file_marker\n");
        let b = format!(
            "middle_of_file_marker\n{block}{block}start_of_file_marker\nend_of_file_marker\n"
        );
        let r = diff_text(&a, &b, edited_opts());
        assert!(r.moves.iter().all(|m| !m.edited), "{:?}", r.moves);
    }

    #[test]
    fn one_shared_line_is_not_enough_evidence() {
        let a = "alpha_unique_statement();\nx1\nx2\nx3\nkeep_one\nkeep_two\n";
        let b = "keep_one\nkeep_two\ny1\ny2\nalpha_unique_statement();\ny3\n";
        let r = diff_text(a, b, edited_opts());
        assert!(r.moves.iter().all(|m| !m.edited), "{:?}", r.moves);
    }

    #[test]
    fn moves_get_stable_ids_shared_by_both_ends() {
        let f = "fn helper(x: u32) -> u32 {\n    let y = x * 2;\n    y + 1\n}\n";
        let g = "fn another(x: u32) -> u32 {\n    let z = x * 3;\n    z + 2\n}\n";
        let (s1, s2) = (stay("one"), stay("two"));
        let a = format!("{f}{s1}{g}{s2}");
        let b = format!("{s1}{s2}{g}{f}");
        let one = diff_text(&a, &b, edited_opts());
        let two = diff_text(&a, &b, edited_opts());
        let ids = |r: &TextDiffResult| r.moves.iter().map(|m| (m.id, m.a, m.b)).collect::<Vec<_>>();
        assert_eq!(ids(&one), ids(&two));
        assert_eq!(one.moves.len(), 2, "{:?}", one.moves);
        // Ids count in the order of the left start and belong to the block, not to a side.
        assert_eq!((one.moves[0].id, one.moves[0].a), (1, 0));
        assert_eq!(one.moves[1].id, 2);
        assert!(one.moves.iter().all(|m| !m.edited && m.len == m.b_len));
    }

    #[test]
    fn whitespace_only_move_stays_exact() {
        let f = "fn helper(x: u32) -> u32 {\n    let y = x * 2;\n    y + 1\n}\n";
        let s = stay("w");
        let a = format!("{f}{s}");
        let b = format!("{s}{}", f.replace("    ", "\t\t"));
        let r = diff_text(&a, &b, edited_opts());
        assert_eq!(r.moves.len(), 1, "{:?}", r.moves);
        let m = r.moves[0];
        assert!(!m.edited && m.len == 4 && m.b_len == 4, "{m:?}");
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
    fn structural_diff_tells_layout_from_content() {
        let o = DiffOptions {
            structural: true,
            ..DiffOptions::default()
        };
        let a = "fn a() {\n    call(one, two);\n}\n\nfn b() { old(); }\n";
        let b = "fn a() {\n    call(\n        one,\n        two,\n    );\n}\n\nfn b() { new(); }\n";
        let r = diff_text_as(a, b, o.clone(), Syntax::for_names(&["a.rs"]));
        let changed: Vec<&Op> = r.ops.iter().filter(|op| op.tag != "equal").collect();
        assert_eq!(changed.len(), 2);
        // The reflowed call differs only by a trailing comma: one token marked.
        assert!(!changed[0].cosmetic);
        assert_eq!(r.left_marks.get(&1), Some(&vec![]));
        assert_eq!(r.right_marks.get(&3), Some(&vec![(11, 12)]));
        assert_eq!(r.left_marks.get(&4), Some(&vec![(9, 12)]));
        assert_eq!(r.right_marks.get(&7), Some(&vec![(9, 12)]));
        // Layout alone is a block of its own kind and is not counted.
        let r = diff_text_as(
            "a(b);\nc();\n",
            "a( b );\nc();\n",
            o.clone(),
            Syntax::for_names(&["a.rs"]),
        );
        assert!(r.ops[0].cosmetic && r.ops[0].tag == "replace");
        assert_eq!((r.added, r.removed, r.ratio), (0, 0, 1.0));
        // Without the option, and for a file of no known language, spaces are content.
        assert!(!diff_text("a(b);\n", "a( b );\n", DiffOptions::default()).ops[0].cosmetic);
        assert!(!diff_text("a(b);\n", "a( b );\n", o).ops[0].cosmetic);
    }

    #[test]
    fn structural_diff_respects_explicit_line_ending_comparison() {
        let options = DiffOptions {
            structural: true,
            ignore_eol: false,
            ..DiffOptions::default()
        };
        let result = diff_text_as("a();\r\n", "a();\n", options, Syntax::for_names(&["a.rs"]));
        assert!(result
            .ops
            .iter()
            .any(|op| op.tag != "equal" && !op.cosmetic));
        assert_eq!((result.added, result.removed), (1, 1));
    }

    #[test]
    fn ignore_options() {
        let o = DiffOptions {
            ignore_whitespace: true,
            ignore_case: true,
            ignore_eol: true,
            ..DiffOptions::default()
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
    fn exported_patches_have_portable_names_in_every_format_and_use_the_sources_as_given() {
        use crate::fs::patch::{parse, Format};
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("some folder").join("deep");
        std::fs::create_dir_all(&dir).unwrap();
        let (p, q) = (dir.join("old name.txt"), dir.join("new name.txt"));
        std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
        std::fs::write(&q, "one\n2\nthree\n").unwrap();
        let (a, b) = (
            DiffSource::File { path: p.clone() },
            DiffSource::File { path: q },
        );
        let unified = export_patch(&a, &b, Format::Unified).unwrap();
        // File names, not the folders of this computer; quoted, as GNU `patch` reads a name with a space.
        assert!(
            unified.starts_with("--- \"a/old name.txt\"\n+++ \"b/new name.txt\"\n"),
            "{unified}"
        );
        assert!(!unified.contains(t.path().to_str().unwrap()));
        for format in [Format::Unified, Format::Context, Format::Normal] {
            let text = export_patch(&a, &b, format).unwrap();
            let patches =
                parse(format, &text).unwrap_or_else(|e| panic!("{format:?}: {e}\n{text}"));
            let hunks: Vec<_> = patches.into_iter().flat_map(|p| p.hunks).collect();
            let (applied, _) = crate::fs::patch::apply_hunks("one\ntwo\nthree\n", &hunks).unwrap();
            assert_eq!(applied, "one\n2\nthree\n", "{format:?}");
        }
        // A draft: the right side is text that is not saved anywhere; the patch is of that text (the draft), not of the file.
        let draft = DiffSource::Text {
            label: "draft.txt".into(),
            text: "one\ntwo\nTHREE\n".into(),
        };
        let patch = export_patch(&a, &draft, Format::Unified).unwrap();
        assert!(
            patch.contains("+THREE") && patch.contains("+++ b/draft.txt"),
            "{patch}"
        );
        assert!(unified_patch(&a, &draft).unwrap() == patch);
        // Binary input is no patch.
        let bin = t.path().join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2]).unwrap();
        assert!(export_patch(&a, &DiffSource::File { path: bin }, Format::Unified).is_err());
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

    #[test]
    fn compare_paged_missing_git_revision_reports_selected_revision_error() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .current_dir(root)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join("first.txt"), "one\n").unwrap();
        run(&["add", "first.txt"]);
        run(&["commit", "-qm", "first"]);
        let first_rev = String::from_utf8(
            std::process::Command::new("git")
                .current_dir(root)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let first_rev = first_rev.trim().to_string();
        let later = root.join("later.txt");
        std::fs::write(&later, "two\n").unwrap();
        let err = compare_paged(
            &DiffSource::GitCommit {
                path: later.clone(),
                commit: first_rev,
            },
            &DiffSource::File { path: later },
            DiffOptions::default(),
            &AtomicBool::new(false),
        )
        .err()
        .expect("missing revision should fail")
        .to_string();
        assert!(
            err.contains("file is not in the selected revision"),
            "unexpected error: {err}"
        );
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
            similar::diff_ratio(&ops, x.len(), y.len())
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
                    revision: String::new(),
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
                    revision: String::new(),
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
