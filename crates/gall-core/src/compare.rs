//! Source-independent inputs for text comparison. File and Git adapters supply bytes.

use crate::fs::diff::{diff_text_with_cancel, DiffOptions, Op, TextDiffResult};
use crate::fs::structural::Syntax;
use serde::Serialize;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

/// The maximum size of either text input.
pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
/// Bound line-index memory independently of input bytes.
pub const MAX_INPUT_LINES: usize = 200_000;
/// A single line must fit into a range response, including JSON escaping.
pub const MAX_LINE_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InputError {
    Cancelled,
    Binary,
    TooManyBytes,
    TooManyLines,
    LineTooLong,
    MinimalWorkLimit,
}

/// Byte offsets into one UTF-8 input. Line ranges exclude LF but retain CR,
/// so callers can choose whether to compare line endings literally.
#[derive(Debug)]
pub struct LineIndex<'a> {
    text: &'a str,
    lines: Vec<Range<usize>>,
    crlf: bool,
}

impl<'a> LineIndex<'a> {
    pub fn new(bytes: &'a [u8], cancel: &AtomicBool) -> Result<Self, InputError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(InputError::Cancelled);
        }
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(InputError::TooManyBytes);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| InputError::Binary)?;
        if bytes[..bytes.len().min(8192)].contains(&0) {
            return Err(InputError::Binary);
        }

        let mut lines = Vec::new();
        let mut crlf = false;
        let mut start = 0;
        for (offset, &byte) in bytes.iter().enumerate() {
            if offset % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                return Err(InputError::Cancelled);
            }
            if byte == b'\n' {
                Self::push_line(&mut lines, start..offset)?;
                crlf |= offset > start && bytes[offset - 1] == b'\r';
                start = offset + 1;
            } else if offset - start >= MAX_LINE_BYTES {
                return Err(InputError::LineTooLong);
            }
        }
        if start < bytes.len() {
            Self::push_line(&mut lines, start..bytes.len())?;
        }
        Ok(Self { text, lines, crlf })
    }

    fn push_line(lines: &mut Vec<Range<usize>>, range: Range<usize>) -> Result<(), InputError> {
        if lines.len() >= MAX_INPUT_LINES {
            return Err(InputError::TooManyLines);
        }
        lines.push(range);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn raw(&self, line: usize) -> Option<&'a str> {
        self.lines.get(line).map(|range| &self.text[range.clone()])
    }

    pub fn display(&self, line: usize) -> Option<&'a str> {
        self.raw(line).map(|s| s.strip_suffix('\r').unwrap_or(s))
    }

    pub fn crlf(&self) -> bool {
        self.crlf
    }

    pub fn final_newline(&self) -> bool {
        self.text.is_empty() || self.text.ends_with('\n')
    }
}

/// Responses reserve space for the envelope and transport metadata.
pub const MAX_PAGE_BYTES: usize = 1024 * 1024;
const PAGE_ENVELOPE_BYTES: usize = 512;
const MAX_PAGE_ITEMS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextShape {
    pub lines: usize,
    pub max_line_columns: usize,
    pub crlf: bool,
    pub final_newline: bool,
}

impl From<&LineIndex<'_>> for TextShape {
    fn from(index: &LineIndex<'_>) -> Self {
        Self {
            lines: index.len(),
            max_line_columns: index
                .lines
                .iter()
                .map(|range| {
                    index.text[range.clone()].chars().fold(0, |width, ch| {
                        if ch == '\t' {
                            width + 8 - width % 8
                        } else {
                            width + 1
                        }
                    })
                })
                .max()
                .unwrap_or(0),
            crlf: index.crlf(),
            final_newline: index.final_newline(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CompareSide {
    Left,
    Right,
}

/// One immutable text comparison. The IPC layer can request individual ranges
/// without returning the full line arrays alongside every comparison.
#[derive(Debug)]
pub struct Comparison {
    pub left: TextShape,
    pub right: TextShape,
    pub only_crlf_differs: bool,
    diff: TextDiffResult,
    detailed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub start: usize,
    pub next: Option<usize>,
    pub total: usize,
    pub items: Vec<T>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk<'a> {
    pub id: String,
    #[serde(flatten)]
    pub op: &'a Op,
}

fn page<T: Serialize>(
    start: usize,
    count: usize,
    total: usize,
    mut item: impl FnMut(usize) -> T,
) -> serde_json::Result<Page<T>> {
    let start = start.min(total);
    let mut items = Vec::new();
    let mut encoded = PAGE_ENVELOPE_BYTES;
    for index in start
        ..start
            .saturating_add(count.clamp(1, MAX_PAGE_ITEMS))
            .min(total)
    {
        let value = item(index);
        let size = serde_json::to_vec(&value)?.len() + 1;
        if encoded + size > MAX_PAGE_BYTES {
            break;
        }
        encoded += size;
        items.push(value);
    }
    let next = start + items.len();
    Ok(Page {
        start,
        next: (next < total).then_some(next),
        total,
        items,
    })
}

impl Comparison {
    pub fn new(
        left: &[u8],
        right: &[u8],
        names: &[&str],
        options: DiffOptions,
        cancel: &AtomicBool,
    ) -> Result<Self, InputError> {
        let left = LineIndex::new(left, cancel)?;
        let right = LineIndex::new(right, cancel)?;
        // Exact Myers disables imara-diff's early exits. Cap the two-sided
        // search space so one request cannot monopolize the window's worker.
        const MAX_MINIMAL_LINE_PAIRS: usize = 25_000_000;
        if options.minimal && left.len().saturating_mul(right.len()) > MAX_MINIMAL_LINE_PAIRS {
            return Err(InputError::MinimalWorkLimit);
        }
        // A paged line-only view cannot display pairing, move links, or token spans.
        // Bound that work separately, including the cost of a single pair of lines.
        let detailed = left.len() + right.len() <= 8_000
            && left.text.len() + right.text.len() <= 256 * 1024
            && left
                .lines
                .iter()
                .chain(&right.lines)
                .all(|range| range.len() <= 4_096);
        let diff = diff_text_with_cancel(
            left.text,
            right.text,
            options,
            Syntax::for_names(names),
            detailed,
            cancel,
        )
        .map_err(|_| InputError::Cancelled)?;
        let mut left_shape = TextShape::from(&left);
        let mut right_shape = TextShape::from(&right);
        left_shape.lines = diff.left_lines.len();
        right_shape.lines = diff.right_lines.len();
        Ok(Self {
            left: left_shape,
            right: right_shape,
            only_crlf_differs: (left.crlf() || right.crlf())
                && differs_only_in_crlf(left.text, right.text),
            diff,
            detailed,
        })
    }

    pub fn line_page(
        &self,
        side: CompareSide,
        start: usize,
        count: usize,
    ) -> serde_json::Result<Page<&str>> {
        let lines = match side {
            CompareSide::Left => &self.diff.left_lines,
            CompareSide::Right => &self.diff.right_lines,
        };
        page(start, count, lines.len(), |i| lines[i].as_str())
    }

    pub fn hunk_page(&self, start: usize, count: usize) -> serde_json::Result<Page<Hunk<'_>>> {
        page(start, count, self.diff.ops.len(), |i| {
            let op = &self.diff.ops[i];
            Hunk {
                id: format!("{}:{}:{}:{}:{}", op.tag, op.a, op.a_len, op.b, op.b_len),
                op,
            }
        })
    }

    pub fn is_detailed(&self) -> bool {
        self.detailed
    }

    pub fn diff(&self) -> &TextDiffResult {
        &self.diff
    }
}

fn differs_only_in_crlf(left: &str, right: &str) -> bool {
    fn normalized(line: &str) -> (&str, bool) {
        if let Some(body) = line
            .strip_suffix("\r\n")
            .or_else(|| line.strip_suffix('\n'))
        {
            (body, true)
        } else {
            (line, false)
        }
    }
    left != right
        && left
            .split_inclusive('\n')
            .map(normalized)
            .eq(right.split_inclusive('\n').map(normalized))
}

#[cfg(test)]
mod tests {
    use super::differs_only_in_crlf;

    #[test]
    fn detects_crlf_only_changes_including_mixed_line_endings() {
        assert!(differs_only_in_crlf("a\r\nb\n", "a\nb\r\n"));
        assert!(!differs_only_in_crlf("a\r\nb\n", "a\nc\r\n"));
        assert!(!differs_only_in_crlf("a\r\n", "a"));
        assert!(!differs_only_in_crlf("a\r", "a"));
        assert!(!differs_only_in_crlf("a\r\n", "a\r\n"));
    }
}
