//! The byte-in, JSON-out entry that the browser worker calls (and the desktop tests call too, so a
//! golden test can hold both to the same answer). It takes the two inputs as bytes, classifies them
//! (text or binary), compares the lines and returns the changed ranges and moved blocks.
//!
//! No file system, no clock, no thread: the caller supplies bytes, and cancellation is a callback
//! that is asked between the steps (in a browser the worker is ended from outside instead).

use crate::lines::{find_edited_moves, find_moves, line_hunks, Move};
use serde::{Deserialize, Serialize};

/// Largest input of the adapter, as on the desktop (`compare::MAX_INPUT_BYTES`).
pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
/// Longest line, as on the desktop (`compare::MAX_LINE_BYTES`).
pub const MAX_LINE_BYTES: usize = 128 * 1024;
/// Most lines of one input, as on the desktop (`compare::MAX_INPUT_LINES`).
pub const MAX_INPUT_LINES: usize = 200_000;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    /// Fewest changed lines (Myers) instead of the more readable histogram diff.
    pub minimal: bool,
    /// Which whitespace differences are ignored (as the desktop's `whitespace`).
    pub whitespace: Whitespace,
    /// Settings saved before `whitespace` existed: `true` means `Whitespace::All`.
    pub ignore_whitespace: bool,
    pub ignore_case: bool,
    /// Treat CRLF and LF as equal. Off (as on the desktop by default): a line ending is part of the line.
    pub ignore_eol: bool,
    /// Also find moved blocks that were edited on the way.
    pub moves_edited: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Whitespace {
    #[default]
    None,
    Leading,
    Trailing,
    All,
}

impl Options {
    fn whitespace_mode(&self) -> Whitespace {
        if self.ignore_whitespace {
            Whitespace::All
        } else {
            self.whitespace
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Range {
    pub tag: &'static str,
    pub a: usize,
    pub a_len: usize,
    pub b: usize,
    pub b_len: usize,
}

/// The answer, serialized as JSON with a `kind` of `text`, `binary`, `tooLarge`, `cancelled` or
/// `error`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Answer {
    #[serde(rename_all = "camelCase")]
    Text {
        /// The changed ranges and, between them, the equal ones.
        ops: Vec<Range>,
        moves: Vec<Move>,
        added: usize,
        removed: usize,
        /// Both inputs hold the same lines under these options.
        identical: bool,
    },
    Binary,
    TooLarge {
        reason: &'static str,
    },
    Cancelled,
    Error {
        message: String,
    },
}

/// Text is valid UTF-8 without a NUL byte in its first 8 KiB (the desktop rule).
fn text_of(bytes: &[u8]) -> Option<&str> {
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// The lines of a text without their LF; a CR before it stays (the desktop's raw lines).
fn lines_of(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// A line as it is compared: the same rules as the desktop's `normalize` (without ignore rules).
fn normalized(raw: &str, opts: &Options) -> String {
    let line = raw.strip_suffix('\r').unwrap_or(raw);
    let s = if opts.ignore_eol { line } else { raw };
    let mut s = match opts.whitespace_mode() {
        Whitespace::None => s.to_string(),
        Whitespace::All => s.split_whitespace().collect::<Vec<_>>().join(" "),
        Whitespace::Leading => s.trim_start().to_string(),
        // A line ending that is compared is not whitespace of the line.
        Whitespace::Trailing => match s.strip_suffix('\r') {
            Some(rest) => format!("{}\r", rest.trim_end()),
            None => s.trim_end().to_string(),
        },
    };
    if opts.ignore_case {
        s = s.to_lowercase();
    }
    s
}

/// Compares two inputs. `cancel` is asked between the steps.
pub fn compare(a: &[u8], b: &[u8], opts: &Options, cancel: &dyn Fn() -> bool) -> Answer {
    if cancel() {
        return Answer::Cancelled;
    }
    if a.len() > MAX_INPUT_BYTES || b.len() > MAX_INPUT_BYTES {
        return Answer::TooLarge {
            reason: "too many bytes",
        };
    }
    let (Some(ta), Some(tb)) = (text_of(a), text_of(b)) else {
        return Answer::Binary;
    };
    let (la, lb) = (lines_of(ta), lines_of(tb));
    if la.len() > MAX_INPUT_LINES || lb.len() > MAX_INPUT_LINES {
        return Answer::TooLarge {
            reason: "too many lines",
        };
    }
    if la.iter().chain(&lb).any(|l| l.len() > MAX_LINE_BYTES) {
        return Answer::TooLarge {
            reason: "a line is too long",
        };
    }
    let na: Vec<String> = la.iter().map(|l| normalized(l, opts)).collect();
    let nb: Vec<String> = lb.iter().map(|l| normalized(l, opts)).collect();
    if cancel() {
        return Answer::Cancelled;
    }
    let hunks = line_hunks(&na, &nb, opts.minimal);
    if cancel() {
        return Answer::Cancelled;
    }
    let mut moves = find_moves(&na, &nb, &hunks);
    if opts.moves_edited {
        let edited = find_edited_moves(&na, &nb, &hunks);
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
    let (mut ops, mut pa, mut pb) = (Vec::new(), 0, 0);
    let (mut added, mut removed) = (0, 0);
    let equal = |ops: &mut Vec<Range>, a: usize, b: usize, len: usize| {
        if len > 0 {
            ops.push(Range {
                tag: "equal",
                a,
                a_len: len,
                b,
                b_len: len,
            });
        }
    };
    for &(a0, a1, b0, b1) in &hunks {
        equal(&mut ops, pa, pb, a0 - pa);
        let tag = match (a1 - a0, b1 - b0) {
            (_, 0) => "delete",
            (0, _) => "insert",
            _ => "replace",
        };
        ops.push(Range {
            tag,
            a: a0,
            a_len: a1 - a0,
            b: b0,
            b_len: b1 - b0,
        });
        removed += a1 - a0;
        added += b1 - b0;
        pa = a1;
        pb = b1;
    }
    equal(&mut ops, pa, pb, na.len() - pa);
    Answer::Text {
        ops,
        moves,
        added,
        removed,
        identical: hunks.is_empty(),
    }
}

/// `compare` with the options as JSON and the answer as JSON: the form the worker uses.
pub fn compare_json(a: &[u8], b: &[u8], options: &str, cancel: &dyn Fn() -> bool) -> String {
    let answer = match serde_json::from_str::<Options>(if options.trim().is_empty() {
        "{}"
    } else {
        options
    }) {
        Ok(opts) => compare(a, b, &opts, cancel),
        Err(e) => Answer::Error {
            message: format!("invalid options: {e}"),
        },
    };
    serde_json::to_string(&answer).unwrap_or_else(|e| {
        format!(r#"{{"kind":"error","message":"could not write the answer: {e}"}}"#)
    })
}

#[cfg(feature = "wasm")]
mod wasm {
    use wasm_bindgen::prelude::*;

    /// Compares two byte buffers; `options` and the answer are JSON (see `compare_json`).
    #[wasm_bindgen(js_name = compare)]
    pub fn compare(a: &[u8], b: &[u8], options: &str) -> String {
        super::compare_json(a, b, options, &|| false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never() -> bool {
        false
    }

    fn text(a: &str, b: &str, opts: &Options) -> Answer {
        compare(a.as_bytes(), b.as_bytes(), opts, &never)
    }

    #[test]
    fn identical_and_changed_inputs() {
        let same = text("a\nb\n", "a\nb\n", &Options::default());
        assert!(matches!(
            same,
            Answer::Text {
                identical: true,
                added: 0,
                removed: 0,
                ..
            }
        ));
        let Answer::Text {
            ops,
            added,
            removed,
            ..
        } = text("a\nb\nc\n", "a\nx\nc\n", &Options::default())
        else {
            panic!("text")
        };
        assert_eq!((added, removed), (1, 1));
        assert_eq!(
            ops.iter().map(|o| o.tag).collect::<Vec<_>>(),
            ["equal", "replace", "equal"]
        );
    }

    #[test]
    fn binary_and_oversized_inputs_are_classified() {
        assert_eq!(
            compare(b"a\0b", b"a", &Options::default(), &never),
            Answer::Binary
        );
        assert_eq!(
            compare(&[0xff, 0xfe], b"a", &Options::default(), &never),
            Answer::Binary
        );
        let many = "x\n".repeat(MAX_INPUT_LINES + 1);
        assert!(matches!(
            text(&many, "", &Options::default()),
            Answer::TooLarge {
                reason: "too many lines"
            }
        ));
    }

    #[test]
    fn crlf_matters_unless_ignored_and_a_missing_final_newline_does_not() {
        let strict = text("a\r\nb", "a\nb\n", &Options::default());
        assert!(matches!(
            strict,
            Answer::Text {
                identical: false,
                ..
            }
        ));
        let loose = Options {
            ignore_eol: true,
            ..Options::default()
        };
        assert!(matches!(
            text("a\r\nb", "a\nb\n", &loose),
            Answer::Text {
                identical: true,
                ..
            }
        ));
    }

    #[test]
    fn whitespace_modes_follow_the_desktop() {
        let all = Options {
            whitespace: Whitespace::All,
            ..Options::default()
        };
        assert!(matches!(
            text("a  b\n", "a b\n", &all),
            Answer::Text {
                identical: true,
                ..
            }
        ));
        let lead = Options {
            whitespace: Whitespace::Leading,
            ..Options::default()
        };
        assert!(matches!(
            text("  a\n", "a\n", &lead),
            Answer::Text {
                identical: true,
                ..
            }
        ));
        assert!(matches!(
            text("a  \n", "a\n", &lead),
            Answer::Text {
                identical: false,
                ..
            }
        ));
        let trail = Options {
            whitespace: Whitespace::Trailing,
            ..Options::default()
        };
        assert!(matches!(
            text("a  \n", "a\n", &trail),
            Answer::Text {
                identical: true,
                ..
            }
        ));
        let legacy = Options {
            ignore_whitespace: true,
            ..Options::default()
        };
        assert!(matches!(
            text("a   b\n", "a b\n", &legacy),
            Answer::Text {
                identical: true,
                ..
            }
        ));
    }

    #[test]
    fn a_very_long_line_is_refused() {
        let long = "x".repeat(MAX_LINE_BYTES + 1);
        assert!(matches!(
            text(&long, "", &Options::default()),
            Answer::TooLarge {
                reason: "a line is too long"
            }
        ));
    }

    #[test]
    fn options_change_what_differs() {
        let (a, b) = ("Hello  \nworld\n", "hello\nworld\n");
        assert!(matches!(
            text(a, b, &Options::default()),
            Answer::Text {
                identical: false,
                ..
            }
        ));
        let both = Options {
            whitespace: Whitespace::All,
            ignore_case: true,
            ..Options::default()
        };
        assert!(matches!(
            text(a, b, &both),
            Answer::Text {
                identical: true,
                ..
            }
        ));
    }

    #[test]
    fn a_cancelled_comparison_gives_no_result() {
        assert_eq!(
            compare(b"a", b"b", &Options::default(), &|| true),
            Answer::Cancelled
        );
        let json = compare_json(b"a", b"b", "", &|| true);
        assert_eq!(json, r#"{"kind":"cancelled"}"#);
    }

    #[test]
    fn json_in_json_out_and_bad_options() {
        let json = compare_json(b"a\n", b"b\n", r#"{"minimal":true}"#, &never);
        assert!(json.starts_with(r#"{"kind":"text""#), "{json}");
        let bad = compare_json(b"a", b"b", "{not json", &never);
        assert!(bad.contains(r#""kind":"error""#));
    }

    #[test]
    fn moved_blocks_get_ids() {
        let f = "fn helper(x: u32) -> u32 {\n    let y = x * 2;\n    y + 1\n}\n";
        let stay: String = (0..12)
            .map(|k| format!("stay_statement_number_{k}();\n"))
            .collect();
        let Answer::Text { moves, .. } = text(
            &format!("{f}{stay}"),
            &format!("{stay}{f}"),
            &Options::default(),
        ) else {
            panic!("text")
        };
        assert_eq!(moves.len(), 1);
        assert_eq!(
            (moves[0].id, moves[0].len, moves[0].b_len, moves[0].edited),
            (1, 4, 4, false)
        );
    }
}
