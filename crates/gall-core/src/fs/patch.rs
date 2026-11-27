//! Patches in the three standard formats (unified, context and normal) through one hunk model.
//!
//! * **Writing** builds hunks from two texts and prints them in the chosen format, with `\ No newline at end of file`
//!   where a file has no final newline. Header names are given by the caller (portable, relative).
//! * **Reading** parses each format back into the same model. Binary patches and metadata changes (modes, renames,
//!   created or deleted files) are refused with their reason: they cannot be applied as text.
//! * **Applying** is exact: a hunk must match its context at the stated line, or at one other place that is the only
//!   match; two possible places, a context that does not match, or hunks that overlap or are out of order are errors.
//!   Line terminators (CRLF, a missing final newline) are part of the match, so applying never changes them.
//! * **Targets** are resolved below a chosen root: absolute paths, `..`, drives and links are refused. A [`plan`] checks
//!   every file first and writes nothing; [`apply_plan`] re-checks the revision of each file, writes atomically and
//!   restores the files it had already written if one write fails.
use serde::{Deserialize, Serialize};
use similar::{Algorithm, ChangeTag, TextDiff};
use std::path::{Path, PathBuf};

/// The largest file a patch is applied to.
pub const MAX_TARGET_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Unified,
    Context,
    Normal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Context,
    Delete,
    Insert,
}

/// One line of a hunk: its text without the terminator (a `\r` stays), and whether the file ends here without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
    pub no_newline: bool,
}

impl Line {
    /// The line as it is in a file: with its terminator unless it is the last one of a file without a final newline.
    fn in_file(&self) -> String {
        if self.no_newline {
            self.text.clone()
        } else {
            format!("{}\n", self.text)
        }
    }
}

/// A hunk. `old_start` and `new_start` are 1-based line numbers of the first line of the old and the new side, or, for
/// a side without lines, the number of the line before it (0 at the top), as in a unified header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<Line>,
}

impl Hunk {
    pub fn old_len(&self) -> usize {
        self.lines.iter().filter(|l| l.kind != Kind::Insert).count()
    }
    pub fn new_len(&self) -> usize {
        self.lines.iter().filter(|l| l.kind != Kind::Delete).count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePatch {
    pub old_name: Option<String>,
    pub new_name: Option<String>,
    pub hunks: Vec<Hunk>,
}

/// Why a patch could not be read, applied or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    Malformed(String),
    Unsupported(String),
    /// The context of hunk `n` (1-based) is not where the file has it.
    Mismatch {
        hunk: usize,
    },
    /// The context of hunk `n` is in more than one place and not at the stated line.
    Ambiguous {
        hunk: usize,
    },
    /// Hunk `n` overlaps the one before it or comes before it.
    Overlap {
        hunk: usize,
    },
    Target(String),
    Stale(String),
    Io(String),
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::Malformed(m) => write!(f, "The patch is malformed: {m}"),
            PatchError::Unsupported(m) => write!(f, "Not supported: {m}"),
            PatchError::Mismatch { hunk } => write!(f, "Hunk {hunk} does not match the file (the file is not the one the patch was made from)"),
            PatchError::Ambiguous { hunk } => write!(f, "Hunk {hunk} matches in more than one place, so it is not applied"),
            PatchError::Overlap { hunk } => write!(f, "Hunk {hunk} overlaps the hunk before it or comes out of order"),
            PatchError::Target(m) => write!(f, "{m}"),
            PatchError::Stale(m) => write!(f, "{m}"),
            PatchError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for PatchError {}

type Result<T> = std::result::Result<T, PatchError>;

// ───────────────────────────── building ─────────────────────────────

fn split_terminator(value: &str) -> (&str, bool) {
    match value.strip_suffix('\n') {
        Some(text) => (text, false),
        None => (value, true),
    }
}

/// The hunks that turn `old` into `new`, each with `context` lines around its changes.
pub fn make_hunks(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_lines(old, new);
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(context) {
        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let (text, no_newline) = split_terminator(change.value());
                let kind = match change.tag() {
                    ChangeTag::Equal => Kind::Context,
                    ChangeTag::Delete => Kind::Delete,
                    ChangeTag::Insert => Kind::Insert,
                };
                lines.push(Line {
                    kind,
                    text: text.to_owned(),
                    no_newline,
                });
            }
        }
        let (Some(first), Some(_)) = (group.first(), group.last()) else {
            continue;
        };
        let mut hunk = Hunk {
            old_start: 0,
            new_start: 0,
            lines,
        };
        let (old_before, new_before) = (first.old_range().start, first.new_range().start);
        hunk.old_start = if hunk.old_len() == 0 {
            old_before
        } else {
            old_before + 1
        };
        hunk.new_start = if hunk.new_len() == 0 {
            new_before
        } else {
            new_before + 1
        };
        hunks.push(hunk);
    }
    hunks
}

/// The patch of one file pair. `old_name` and `new_name` are the header names, written as they are.
pub fn make_patch(
    old_name: &str,
    new_name: &str,
    old: &str,
    new: &str,
    format: Format,
) -> FilePatch {
    let context = if format == Format::Normal { 0 } else { 3 };
    FilePatch {
        old_name: Some(old_name.to_owned()),
        new_name: Some(new_name.to_owned()),
        hunks: make_hunks(old, new, context),
    }
}

// ───────────────────────────── writing ─────────────────────────────

const NO_NEWLINE: &str = "\\ No newline at end of file\n";

fn range(start: usize, len: usize) -> String {
    match len {
        1 => start.to_string(),
        _ => format!("{start},{len}"),
    }
}

/// A line of a hunk as printed, with the marker after a line that has no final newline.
fn print_line(out: &mut String, prefix: &str, line: &Line) {
    out.push_str(prefix);
    out.push_str(&line.text);
    out.push('\n');
    if line.no_newline {
        out.push_str(NO_NEWLINE);
    }
}

/// A name as a header line holds it. GNU `patch` ends a bare name at a space or a tab, so a name with one of them, a
/// quote, a backslash or a control character is written between double quotes with C escapes, as GNU `diff` and Git
/// write it (`unquote` reads it back). Other letters stay as they are, so a UTF-8 name remains readable.
fn header_text(name: Option<&str>) -> String {
    let Some(name) = name else {
        return "/dev/null".to_owned();
    };
    if !name
        .chars()
        .any(|c| c == ' ' || c == '"' || c == '\\' || c.is_ascii_control())
    {
        return name.to_owned();
    }
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for c in name.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_ascii_control() => out.push_str(&format!("\\{:03o}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn write_unified(patch: &FilePatch, out: &mut String) {
    if patch.hunks.is_empty() {
        return;
    }
    out.push_str(&format!(
        "--- {}\n+++ {}\n",
        header_text(patch.old_name.as_deref()),
        header_text(patch.new_name.as_deref())
    ));
    for hunk in &patch.hunks {
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            range(hunk.old_start, hunk.old_len()),
            range(hunk.new_start, hunk.new_len())
        ));
        for line in &hunk.lines {
            let prefix = match line.kind {
                Kind::Context => " ",
                Kind::Delete => "-",
                Kind::Insert => "+",
            };
            print_line(out, prefix, line);
        }
    }
}

/// Runs of changed lines in a hunk: a run that both deletes and inserts is a change (`!` in the context format).
fn changed_runs(hunk: &Hunk) -> Vec<bool> {
    let mut changed = vec![false; hunk.lines.len()];
    let mut i = 0;
    while i < hunk.lines.len() {
        if hunk.lines[i].kind == Kind::Context {
            i += 1;
            continue;
        }
        let start = i;
        while i < hunk.lines.len() && hunk.lines[i].kind != Kind::Context {
            i += 1;
        }
        let run = &hunk.lines[start..i];
        let both = run.iter().any(|l| l.kind == Kind::Delete)
            && run.iter().any(|l| l.kind == Kind::Insert);
        changed[start..i].fill(both);
    }
    changed
}

fn context_range(start: usize, len: usize) -> String {
    match len {
        0 => start.to_string(),
        1 => start.to_string(),
        _ => format!("{start},{}", start + len - 1),
    }
}

fn write_context(patch: &FilePatch, out: &mut String) {
    if patch.hunks.is_empty() {
        return;
    }
    out.push_str(&format!(
        "*** {}\n--- {}\n",
        header_text(patch.old_name.as_deref()),
        header_text(patch.new_name.as_deref())
    ));
    for hunk in &patch.hunks {
        let changed = changed_runs(hunk);
        out.push_str("***************\n");
        out.push_str(&format!(
            "*** {} ****\n",
            context_range(hunk.old_start, hunk.old_len())
        ));
        // The old side is printed only when something is deleted or changed, the new side only when something is inserted or changed.
        if hunk.lines.iter().any(|l| l.kind == Kind::Delete) {
            for (line, ch) in hunk.lines.iter().zip(&changed) {
                match line.kind {
                    Kind::Context => print_line(out, "  ", line),
                    Kind::Delete => print_line(out, if *ch { "! " } else { "- " }, line),
                    Kind::Insert => {}
                }
            }
        }
        out.push_str(&format!(
            "--- {} ----\n",
            context_range(hunk.new_start, hunk.new_len())
        ));
        if hunk.lines.iter().any(|l| l.kind == Kind::Insert) {
            for (line, ch) in hunk.lines.iter().zip(&changed) {
                match line.kind {
                    Kind::Context => print_line(out, "  ", line),
                    Kind::Insert => print_line(out, if *ch { "! " } else { "+ " }, line),
                    Kind::Delete => {}
                }
            }
        }
    }
}

fn normal_range(start: usize, len: usize) -> String {
    match len {
        0 | 1 => start.to_string(),
        _ => format!("{start},{}", start + len - 1),
    }
}

fn write_normal(patch: &FilePatch, out: &mut String) {
    for hunk in &patch.hunks {
        let deletes: Vec<&Line> = hunk
            .lines
            .iter()
            .filter(|l| l.kind == Kind::Delete)
            .collect();
        let inserts: Vec<&Line> = hunk
            .lines
            .iter()
            .filter(|l| l.kind == Kind::Insert)
            .collect();
        let command = match (deletes.is_empty(), inserts.is_empty()) {
            (false, true) => 'd',
            (true, false) => 'a',
            _ => 'c',
        };
        out.push_str(&format!(
            "{}{command}{}\n",
            normal_range(hunk.old_start, deletes.len()),
            normal_range(hunk.new_start, inserts.len())
        ));
        for line in &deletes {
            print_line(out, "< ", line);
        }
        if command == 'c' {
            out.push_str("---\n");
        }
        for line in &inserts {
            print_line(out, "> ", line);
        }
    }
}

/// Prints patches in a format. Files without changes print nothing.
pub fn write(format: Format, patches: &[FilePatch]) -> String {
    let mut out = String::new();
    for patch in patches {
        match format {
            Format::Unified => write_unified(patch, &mut out),
            Format::Context => write_context(patch, &mut out),
            Format::Normal => write_normal(patch, &mut out),
        }
    }
    out
}

// ───────────────────────────── reading ─────────────────────────────

/// What cannot be applied as text, from the lines before a file's headers: say why instead of ignoring it.
fn unsupported_line(line: &str) -> Option<String> {
    let kinds = [
        ("Binary files ", "a binary change"),
        ("GIT binary patch", "a binary change"),
        ("new file mode", "a created file"),
        ("deleted file mode", "a deleted file"),
        ("old mode", "a change of file mode"),
        ("new mode", "a change of file mode"),
        ("rename from", "a rename"),
        ("rename to", "a rename"),
        ("copy from", "a copy"),
        ("copy to", "a copy"),
        ("similarity index", "a rename or copy"),
    ];
    kinds
        .iter()
        .find(|(prefix, _)| line.starts_with(prefix))
        .map(|(_, what)| format!("{what} cannot be applied as text ({})", line.trim_end()))
}

/// A file name as `diff` and Git write it when it holds a space or an unusual character: between double quotes, with
/// C escapes (`\\`, `\"`, `\n`, `\t`, and `\303\244` for the bytes of a non-ASCII letter). Anything else is
/// returned as it is.
fn unquote(name: &str) -> String {
    let Some(inner) = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) else {
        return name.to_owned();
    };
    // A closing quote that is itself escaped (`"end\"`) closes nothing: the name is not quoted.
    if inner.bytes().rev().take_while(|b| *b == b'\\').count() % 2 == 1 {
        return name.to_owned();
    }
    let mut bytes: Vec<u8> = Vec::with_capacity(inner.len());
    let mut it = inner.bytes().peekable();
    while let Some(b) = it.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match it.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'"') => bytes.push(b'"'),
            Some(b'\\') => bytes.push(b'\\'),
            Some(d @ b'0'..=b'7') => {
                let mut value = u32::from(d - b'0');
                for _ in 0..2 {
                    match it.peek() {
                        Some(c @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(*c - b'0');
                            it.next();
                        }
                        _ => break,
                    }
                }
                // `\457` is not a byte: a name that holds it is not a name this reads (it would become another file).
                let Ok(byte) = u8::try_from(value) else {
                    return name.to_owned();
                };
                bytes.push(byte);
            }
            Some(other) => {
                bytes.push(b'\\');
                bytes.push(other);
            }
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| name.to_owned())
}

fn header_name(rest: &str) -> Option<String> {
    // The timestamp after a tab (or two spaces) is not part of the name.
    let name = rest
        .split('\t')
        .next()
        .unwrap_or(rest)
        .trim_end_matches('\r')
        .trim();
    (!name.is_empty()).then(|| unquote(name))
}

fn lines_of(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn number(text: &str, what: &str) -> Result<usize> {
    text.parse::<usize>()
        .map_err(|_| PatchError::Malformed(format!("{what} is not a number: {text:?}")))
}

fn mark_no_newline(lines: &mut [Line]) -> Result<()> {
    match lines.last_mut() {
        Some(line) => {
            line.no_newline = true;
            Ok(())
        }
        None => Err(PatchError::Malformed(
            "a \"No newline\" marker with no line before it".into(),
        )),
    }
}

/// `-a[,b]` or `+c[,d]` from a unified hunk header: (start, length).
fn unified_range(text: &str) -> Result<(usize, usize)> {
    let body = &text[1..];
    match body.split_once(',') {
        Some((a, b)) => Ok((number(a, "a line number")?, number(b, "a length")?)),
        None => Ok((number(body, "a line number")?, 1)),
    }
}

pub fn parse_unified(text: &str) -> Result<Vec<FilePatch>> {
    let lines = lines_of(text);
    let mut patches: Vec<FilePatch> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(why) = unsupported_line(line) {
            return Err(PatchError::Unsupported(why));
        }
        if let Some(old) = line.strip_prefix("--- ") {
            let Some(new) = lines.get(i + 1).and_then(|l| l.strip_prefix("+++ ")) else {
                i += 1;
                continue;
            };
            let name = |n: Option<String>| n.filter(|n| n != "/dev/null");
            let (old_name, new_name) = (header_name(old), header_name(new));
            if old_name.as_deref() == Some("/dev/null") || new_name.as_deref() == Some("/dev/null")
            {
                return Err(PatchError::Unsupported(
                    "a created or deleted file cannot be applied as text (/dev/null)".into(),
                ));
            }
            patches.push(FilePatch {
                old_name: name(old_name),
                new_name: name(new_name),
                hunks: Vec::new(),
            });
            i += 2;
            continue;
        }
        if let Some(header) = line.strip_prefix("@@ ") {
            let current = patches
                .last_mut()
                .ok_or_else(|| PatchError::Malformed("a hunk before any file header".into()))?;
            let inner = header.split(" @@").next().unwrap_or(header);
            let mut parts = inner.split(' ');
            let (old, new) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            if !old.starts_with('-') || !new.starts_with('+') {
                return Err(PatchError::Malformed(format!("a hunk header: {line:?}")));
            }
            let ((os, ol), (ns, nl)) = (unified_range(old)?, unified_range(new)?);
            let mut hunk_lines: Vec<Line> = Vec::new();
            let (mut have_old, mut have_new) = (0, 0);
            i += 1;
            while i < lines.len() && (have_old < ol || have_new < nl || lines[i].starts_with('\\'))
            {
                let l = lines[i];
                let (kind, text) = match l.chars().next() {
                    Some(' ') => (Kind::Context, &l[1..]),
                    Some('-') => (Kind::Delete, &l[1..]),
                    Some('+') => (Kind::Insert, &l[1..]),
                    // An empty context line whose space was stripped.
                    None => (Kind::Context, ""),
                    Some('\\') => {
                        mark_no_newline(&mut hunk_lines)?;
                        i += 1;
                        continue;
                    }
                    Some(_) => {
                        return Err(PatchError::Malformed(format!(
                            "a hunk line that starts with neither space, - nor +: {l:?}"
                        )))
                    }
                };
                match kind {
                    Kind::Context => {
                        have_old += 1;
                        have_new += 1;
                    }
                    Kind::Delete => have_old += 1,
                    Kind::Insert => have_new += 1,
                }
                hunk_lines.push(Line {
                    kind,
                    text: text.to_owned(),
                    no_newline: false,
                });
                i += 1;
            }
            if have_old != ol || have_new != nl {
                return Err(PatchError::Malformed(format!("a hunk has {have_old} old and {have_new} new lines, its header says {ol} and {nl}")));
            }
            current.hunks.push(Hunk {
                old_start: os,
                new_start: ns,
                lines: hunk_lines,
            });
            continue;
        }
        i += 1;
    }
    Ok(patches)
}

/// `a,b` or `a` of a context or normal range: (first, last).
fn span(text: &str) -> Result<(usize, usize)> {
    match text.split_once(',') {
        Some((a, b)) => Ok((
            number(a.trim(), "a line number")?,
            number(b.trim(), "a line number")?,
        )),
        None => {
            let n = number(text.trim(), "a line number")?;
            Ok((n, n))
        }
    }
}

pub fn parse_context(text: &str) -> Result<Vec<FilePatch>> {
    let lines = lines_of(text);
    let mut patches: Vec<FilePatch> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(why) = unsupported_line(line) {
            return Err(PatchError::Unsupported(why));
        }
        if let Some(old) = line.strip_prefix("*** ") {
            if let Some(new) = lines.get(i + 1).and_then(|l| l.strip_prefix("--- ")) {
                let (old_name, new_name) = (header_name(old), header_name(new));
                if old_name.as_deref() == Some("/dev/null")
                    || new_name.as_deref() == Some("/dev/null")
                {
                    return Err(PatchError::Unsupported(
                        "a created or deleted file cannot be applied as text (/dev/null)".into(),
                    ));
                }
                patches.push(FilePatch {
                    old_name,
                    new_name,
                    hunks: Vec::new(),
                });
                i += 2;
                continue;
            }
        }
        if line == "***************" {
            let current = patches
                .last_mut()
                .ok_or_else(|| PatchError::Malformed("a hunk before any file header".into()))?;
            let old_header = lines
                .get(i + 1)
                .and_then(|l| l.strip_prefix("*** "))
                .and_then(|l| l.strip_suffix(" ****"))
                .ok_or_else(|| {
                    PatchError::Malformed("a context hunk without its \"*** a,b ****\" line".into())
                })?;
            let (os, oe) = span(old_header)?;
            i += 2;
            // Old side: lines until the "--- c,d ----" line.
            let mut old_items: Vec<(char, Line)> = Vec::new();
            while i < lines.len() && !(lines[i].starts_with("--- ") && lines[i].ends_with(" ----"))
            {
                let l = lines[i];
                if l.starts_with('\\') {
                    let last = old_items.last_mut().ok_or_else(|| {
                        PatchError::Malformed(
                            "a \"No newline\" marker with no line before it".into(),
                        )
                    })?;
                    last.1.no_newline = true;
                    i += 1;
                    continue;
                }
                let (mark, rest) = split_marker(l)?;
                if !matches!(mark, ' ' | '-' | '!') {
                    return Err(PatchError::Malformed(format!(
                        "an old-side line with the mark {mark:?}"
                    )));
                }
                old_items.push((
                    mark,
                    Line {
                        kind: Kind::Context,
                        text: rest.to_owned(),
                        no_newline: false,
                    },
                ));
                i += 1;
            }
            let new_header = lines
                .get(i)
                .and_then(|l| l.strip_prefix("--- "))
                .and_then(|l| l.strip_suffix(" ----"))
                .ok_or_else(|| {
                    PatchError::Malformed("a context hunk without its \"--- c,d ----\" line".into())
                })?;
            let (ns, ne) = span(new_header)?;
            i += 1;
            let mut new_items: Vec<(char, Line)> = Vec::new();
            while i < lines.len()
                && lines[i] != "***************"
                && !(lines[i].starts_with("*** ")
                    && lines[i + 1..]
                        .first()
                        .is_some_and(|n| n.starts_with("--- "))
                    && !lines[i].ends_with(" ****"))
            {
                let l = lines[i];
                if l.starts_with('\\') {
                    let last = new_items.last_mut().ok_or_else(|| {
                        PatchError::Malformed(
                            "a \"No newline\" marker with no line before it".into(),
                        )
                    })?;
                    last.1.no_newline = true;
                    i += 1;
                    continue;
                }
                let (mark, rest) = split_marker(l)?;
                if !matches!(mark, ' ' | '+' | '!') {
                    return Err(PatchError::Malformed(format!(
                        "a new-side line with the mark {mark:?}"
                    )));
                }
                new_items.push((
                    mark,
                    Line {
                        kind: Kind::Context,
                        text: rest.to_owned(),
                        no_newline: false,
                    },
                ));
                i += 1;
            }
            current.hunks.push(merge_context_sides(
                os, oe, ns, ne, old_items, new_items, old_header, new_header,
            )?);
            continue;
        }
        i += 1;
    }
    Ok(patches)
}

fn split_marker(line: &str) -> Result<(char, &str)> {
    let bytes = line.as_bytes();
    match bytes.first() {
        // An empty context line whose spaces were stripped.
        None => Ok((' ', "")),
        // The mark is one ASCII character and a space, so the text starts at a character boundary.
        Some(&mark) if mark.is_ascii() && bytes.get(1) == Some(&b' ') => {
            Ok((mark as char, &line[2..]))
        }
        Some(&b' ') if bytes.len() == 1 => Ok((' ', "")),
        _ => Err(PatchError::Malformed(format!(
            "a context-format line without its two-character mark: {line:?}"
        ))),
    }
}

/// The two sides of a context hunk as one list: shared lines once, deletions before insertions.
#[allow(clippy::too_many_arguments)]
fn merge_context_sides(
    os: usize,
    oe: usize,
    ns: usize,
    ne: usize,
    old: Vec<(char, Line)>,
    new: Vec<(char, Line)>,
    old_header: &str,
    new_header: &str,
) -> Result<Hunk> {
    // A side that is not printed (nothing deleted, or nothing inserted) is the shared lines of the other one.
    let shared = |items: &[(char, Line)]| -> Vec<(char, Line)> {
        items.iter().filter(|(m, _)| *m == ' ').cloned().collect()
    };
    let old = if old.is_empty() && !new.is_empty() {
        shared(&new)
    } else {
        old
    };
    let new = if new.is_empty() && !old.is_empty() {
        shared(&old)
    } else {
        new
    };
    let mut lines: Vec<Line> = Vec::new();
    let (mut a, mut b) = (0usize, 0usize);
    while a < old.len() || b < new.len() {
        match (old.get(a), new.get(b)) {
            (Some((' ', ol)), Some((' ', nl))) => {
                if ol.text != nl.text {
                    return Err(PatchError::Malformed(format!(
                        "a shared line differs between the sides: {:?} and {:?}",
                        ol.text, nl.text
                    )));
                }
                lines.push(Line {
                    kind: Kind::Context,
                    text: ol.text.clone(),
                    no_newline: ol.no_newline || nl.no_newline,
                });
                a += 1;
                b += 1;
            }
            _ => {
                let mut moved = false;
                while let Some((m, l)) = old.get(a).filter(|(m, _)| *m != ' ') {
                    let _ = m;
                    lines.push(Line {
                        kind: Kind::Delete,
                        ..l.clone()
                    });
                    a += 1;
                    moved = true;
                }
                while let Some((m, l)) = new.get(b).filter(|(m, _)| *m != ' ') {
                    let _ = m;
                    lines.push(Line {
                        kind: Kind::Insert,
                        ..l.clone()
                    });
                    b += 1;
                    moved = true;
                }
                if !moved {
                    return Err(PatchError::Malformed(
                        "the two sides of a context hunk do not line up".into(),
                    ));
                }
            }
        }
    }
    let hunk = Hunk {
        old_start: 0,
        new_start: 0,
        lines,
    };
    let (ol, nl) = (hunk.old_len(), hunk.new_len());
    // The header ranges must agree with the lines.
    let expected = |start: usize, end: usize, len: usize| {
        if len == 0 {
            true
        } else {
            end + 1 >= start && end + 1 - start == len
        }
    };
    if !expected(os, oe, ol) || !expected(ns, ne, nl) {
        return Err(PatchError::Malformed(format!(
            "the ranges {old_header:?} and {new_header:?} do not match the lines of the hunk"
        )));
    }
    Ok(Hunk {
        old_start: os,
        new_start: ns,
        ..hunk
    })
}

pub fn parse_normal(text: &str) -> Result<Vec<FilePatch>> {
    let lines = lines_of(text);
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(why) = unsupported_line(line) {
            return Err(PatchError::Unsupported(why));
        }
        let Some(at) = line.find(['a', 'c', 'd']) else {
            return Err(PatchError::Malformed(format!(
                "not a normal-format command: {line:?}"
            )));
        };
        let (left, right) = (&line[..at], &line[at + 1..]);
        if left.is_empty()
            || !left.bytes().all(|b| b.is_ascii_digit() || b == b',')
            || right.is_empty()
            || !right.bytes().all(|b| b.is_ascii_digit() || b == b',')
        {
            return Err(PatchError::Malformed(format!(
                "not a normal-format command: {line:?}"
            )));
        }
        let command = line.as_bytes()[at] as char;
        let ((os, oe), (ns, ne)) = (span(left)?, span(right)?);
        i += 1;
        let mut hunk_lines: Vec<Line> = Vec::new();
        let width = |first: usize, last: usize| {
            last.checked_add(1)
                .and_then(|end| end.checked_sub(first))
                .ok_or_else(|| {
                    PatchError::Malformed(format!(
                        "a line range that ends before it starts: {line:?}"
                    ))
                })
        };
        let want_old = if command == 'a' { 0 } else { width(os, oe)? };
        let want_new = if command == 'd' { 0 } else { width(ns, ne)? };
        let read = |prefix: &str,
                    want: usize,
                    kind: Kind,
                    i: &mut usize,
                    lines_out: &mut Vec<Line>|
         -> Result<()> {
            let mut got = 0;
            while *i < lines.len() && (got < want || lines[*i].starts_with('\\')) {
                let l = lines[*i];
                if l.starts_with('\\') {
                    mark_no_newline(lines_out)?;
                } else if let Some(text) = l.strip_prefix(prefix) {
                    lines_out.push(Line {
                        kind,
                        text: text.to_owned(),
                        no_newline: false,
                    });
                    got += 1;
                } else {
                    break;
                }
                *i += 1;
            }
            if got != want {
                return Err(PatchError::Malformed(format!(
                    "a normal-format hunk has {got} lines where {want} were expected"
                )));
            }
            Ok(())
        };
        read("< ", want_old, Kind::Delete, &mut i, &mut hunk_lines)?;
        if command == 'c' {
            if lines.get(i) != Some(&"---") {
                return Err(PatchError::Malformed(
                    "a change without its \"---\" separator".into(),
                ));
            }
            i += 1;
        }
        read("> ", want_new, Kind::Insert, &mut i, &mut hunk_lines)?;
        // In the model a side without lines is numbered by the line before it, which is what the command says.
        hunks.push(Hunk {
            old_start: os,
            new_start: ns,
            lines: hunk_lines,
        });
    }
    Ok(vec![FilePatch {
        old_name: None,
        new_name: None,
        hunks,
    }])
}

pub fn parse(format: Format, text: &str) -> Result<Vec<FilePatch>> {
    match format {
        Format::Unified => parse_unified(text),
        Format::Context => parse_context(text),
        Format::Normal => parse_normal(text),
    }
}

/// The format a text looks like, to preselect the selector; `None` when it is none of them. Reading still goes
/// through the format the user confirms.
pub fn sniff(text: &str) -> Option<Format> {
    let first = text.lines().find(|l| {
        !l.trim().is_empty()
            && !l.starts_with("diff ")
            && !l.starts_with("index ")
            && !l.starts_with("Index:")
            && !l.starts_with("=====")
    })?;
    if first.starts_with("*** ") {
        Some(Format::Context)
    } else if first.starts_with("--- ") || first.starts_with("@@ ") {
        Some(Format::Unified)
    } else if first.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && first.contains(['a', 'c', 'd'])
    {
        Some(Format::Normal)
    } else {
        None
    }
}

// ───────────────────────────── applying ─────────────────────────────

/// The text with the hunks applied, exactly: each hunk at its stated line or at the only other place that matches.
/// Also the offsets (found line minus stated line) of each hunk, for the preview.
pub fn apply_hunks(source: &str, hunks: &[Hunk]) -> Result<(String, Vec<isize>)> {
    let src: Vec<&str> = source.split_inclusive('\n').collect();
    let mut out = String::with_capacity(source.len());
    let mut offsets = Vec::with_capacity(hunks.len());
    let mut cursor = 0usize;
    for (n, hunk) in hunks.iter().enumerate() {
        let number = n + 1;
        let expected: Vec<String> = hunk
            .lines
            .iter()
            .filter(|l| l.kind != Kind::Insert)
            .map(Line::in_file)
            .collect();
        let stated = if hunk.old_len() == 0 {
            hunk.old_start
        } else {
            hunk.old_start.saturating_sub(1)
        };
        let at = if expected.is_empty() {
            // A pure insertion without context goes after the stated line.
            if stated > src.len() {
                return Err(PatchError::Mismatch { hunk: number });
            }
            stated
        } else {
            let fits = |pos: usize| {
                pos + expected.len() <= src.len()
                    && src[pos..pos + expected.len()]
                        .iter()
                        .zip(&expected)
                        .all(|(a, b)| *a == b.as_str())
            };
            if fits(stated) {
                stated
            } else {
                let found: Vec<usize> = (0..src.len()).filter(|pos| fits(*pos)).collect();
                match found.as_slice() {
                    [] => return Err(PatchError::Mismatch { hunk: number }),
                    [only] => *only,
                    _ => return Err(PatchError::Ambiguous { hunk: number }),
                }
            }
        };
        if at < cursor {
            return Err(PatchError::Overlap { hunk: number });
        }
        offsets.push(at as isize - stated as isize);
        for line in &src[cursor..at] {
            out.push_str(line);
        }
        for line in hunk.lines.iter().filter(|l| l.kind != Kind::Delete) {
            out.push_str(&line.in_file());
        }
        cursor = at + expected.len();
    }
    for line in &src[cursor..] {
        out.push_str(line);
    }
    Ok((out, offsets))
}

// ───────────────────────────── targets and plans ─────────────────────────────

/// The file a patch names, below `root`. `strip` leading components are dropped (`None`: one when the name starts with
/// `a/` or `b/`, as a patch made by Git or by Gall does). Refused: an absolute name, `..`, a drive, a NUL, a name
/// with nothing left, a folder above the target that is a link leading out of `root`, and a target that is itself a link.
pub fn resolve_target(root: &Path, name: &str, strip: Option<usize>) -> Result<PathBuf> {
    let bad = |why: &str| PatchError::Target(format!("{name:?}: {why}"));
    if name.contains('\0') {
        return Err(bad("the name has a NUL"));
    }
    // A backslash is a folder separator in a Windows name only; in another name it is a character of the file name.
    let normalized = if cfg!(windows) {
        name.replace('\\', "/")
    } else {
        name.to_owned()
    };
    if normalized.starts_with('/') || normalized.as_bytes().get(1) == Some(&b':') {
        return Err(bad("an absolute path is not allowed"));
    }
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let strip = strip.unwrap_or_else(|| {
        usize::from(matches!(parts.first(), Some(&"a") | Some(&"b")) && parts.len() > 1)
    });
    let parts = parts
        .get(strip..)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad("nothing is left of the name after removing the leading folders"))?;
    if parts.iter().any(|p| *p == ".." || p.contains(':')) {
        return Err(bad("a name with .. or a drive leaves the folder"));
    }
    let target = parts
        .iter()
        .fold(root.to_path_buf(), |path, part| path.join(part));
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|e| PatchError::Target(format!("{}: {e}", root.display())))?;
    let parent = target.parent().ok_or_else(|| bad("no folder"))?;
    let canonical_parent =
        std::fs::canonicalize(parent).map_err(|_| bad("the folder does not exist"))?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(bad("the folder leads out of the chosen root (a link)"));
    }
    match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(bad("the file is a link, which a patch does not follow"))
        }
        Ok(meta) if !meta.is_file() => Err(bad("it is not a regular file")),
        Ok(_) => Ok(target),
        Err(_) => Err(bad("the file does not exist")),
    }
}

/// One file of a plan: what it will become, and what it is now (to restore it and to notice a change).
#[derive(Debug, Clone)]
pub struct Planned {
    pub path: PathBuf,
    pub name: String,
    pub hunks: usize,
    pub offsets: Vec<isize>,
    pub new_text: String,
    original: Vec<u8>,
    revision: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub files: Vec<Planned>,
}

/// Checks every file of the patches against `root` and builds the result in memory; nothing is written. All problems
/// are returned together (`name: reason`), so one preview shows everything that is wrong. Two patches for one file are
/// a problem, not merged.
pub fn plan(
    root: &Path,
    patches: &[FilePatch],
    strip: Option<usize>,
) -> std::result::Result<Plan, Vec<String>> {
    let mut files: Vec<Planned> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for patch in patches {
        let name = patch
            .old_name
            .clone()
            .or_else(|| patch.new_name.clone())
            .unwrap_or_default();
        if name.is_empty() {
            problems.push("a patch without a file name needs its target chosen".into());
            continue;
        }
        let one = (|| -> Result<Planned> {
            let path = resolve_target(root, &name, strip)?;
            // The same file under two spellings (`Readme.md` and `README.md` on a case-blind volume, a hard link) is one file:
            // on Unix the device and inode say so, elsewhere the canonical path.
            let identity = |p: &Path| -> (Option<(u64, u64)>, PathBuf) {
                #[cfg(unix)]
                let id = {
                    use std::os::unix::fs::MetadataExt;
                    std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
                };
                #[cfg(not(unix))]
                let id = None;
                (
                    id,
                    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()),
                )
            };
            let this = identity(&path);
            let same = |f: &Planned| {
                let other = identity(&f.path);
                f.path == path || other.1 == this.1 || (this.0.is_some() && other.0 == this.0)
            };
            if files.iter().any(same) {
                return Err(PatchError::Target(format!(
                    "{name:?}: two patches name the same file"
                )));
            }
            let meta =
                std::fs::metadata(&path).map_err(|e| PatchError::Io(format!("{name:?}: {e}")))?;
            if meta.len() > MAX_TARGET_BYTES {
                return Err(PatchError::Target(format!(
                    "{name:?}: the file is larger than {} MiB",
                    MAX_TARGET_BYTES / (1024 * 1024)
                )));
            }
            let original =
                std::fs::read(&path).map_err(|e| PatchError::Io(format!("{name:?}: {e}")))?;
            let text = std::str::from_utf8(&original)
                .ok()
                .filter(|t| !t.contains('\0'))
                .ok_or_else(|| {
                    PatchError::Unsupported(format!(
                        "{name:?}: a binary file cannot be patched as text"
                    ))
                })?;
            let (new_text, offsets) = apply_hunks(text, &patch.hunks)
                .map_err(|e| PatchError::Target(format!("{name:?}: {e}")))?;
            Ok(Planned {
                revision: super::ops::file_revision(&path),
                path,
                name: name.clone(),
                hunks: patch.hunks.len(),
                offsets,
                new_text,
                original,
            })
        })();
        match one {
            Ok(planned) => files.push(planned),
            Err(e) => problems.push(e.to_string()),
        }
    }
    if problems.is_empty() {
        Ok(Plan { files })
    } else {
        Err(problems)
    }
}

/// What a failed apply did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyFailure {
    /// The file that could not be written (or that changed since the preview), and why.
    pub failed: PathBuf,
    pub error: String,
    /// Files that had been written and were put back as they were.
    pub restored: Vec<PathBuf>,
    /// Files that had been written and could not be put back: they hold the patched text now.
    pub not_restored: Vec<(PathBuf, String)>,
}

/// Writes a plan. Before anything is written every file is checked to be as it was at the preview; each write is
/// atomic and keeps the permissions. If a write fails the files already written are put back, and the failure says
/// which could and which could not be.
pub fn apply_plan(plan: &Plan) -> std::result::Result<Vec<PathBuf>, ApplyFailure> {
    for file in &plan.files {
        if super::ops::file_revision(&file.path) != file.revision {
            return Err(ApplyFailure {
                failed: file.path.clone(),
                error: format!(
                    "{:?} changed since the preview; nothing was written",
                    file.name
                ),
                restored: Vec::new(),
                not_restored: Vec::new(),
            });
        }
    }
    let mut written: Vec<&Planned> = Vec::new();
    for file in &plan.files {
        if let Err(e) = crate::handoff::save_output(&file.path, file.new_text.as_bytes()) {
            let (mut restored, mut not_restored) = (Vec::new(), Vec::new());
            for done in written.iter().rev() {
                match crate::handoff::save_output(&done.path, &done.original) {
                    Ok(()) => restored.push(done.path.clone()),
                    Err(back) => not_restored.push((done.path.clone(), back.to_string())),
                }
            }
            return Err(ApplyFailure {
                failed: file.path.clone(),
                error: e.to_string(),
                restored,
                not_restored,
            });
        }
        written.push(file);
    }
    Ok(plan.files.iter().map(|f| f.path.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(format: Format, old: &str, new: &str) -> String {
        write(
            format,
            &[make_patch("a/f.txt", "b/f.txt", old, new, format)],
        )
    }

    /// Writes, reads back and applies in every format.
    fn round_trip(old: &str, new: &str) {
        for format in [Format::Unified, Format::Context, Format::Normal] {
            let text = one(format, old, new);
            let parsed = parse(format, &text).unwrap_or_else(|e| panic!("{format:?}: {e}\n{text}"));
            let hunks: Vec<Hunk> = parsed.into_iter().flat_map(|p| p.hunks).collect();
            let (result, _) =
                apply_hunks(old, &hunks).unwrap_or_else(|e| panic!("{format:?}: {e}\n{text}"));
            assert_eq!(result, new, "{format:?}\n{text}");
        }
    }

    #[test]
    fn all_three_formats_round_trip_text_edits() {
        round_trip("a\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\nf\n");
        round_trip("one\n", "one\ntwo\nthree\n");
        round_trip("x\ny\nz\n", "");
        round_trip("", "new file\ncontent\n");
        round_trip(
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n",
            "1\n2\nTHREE\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n",
        );
        round_trip("same\n", "same\n");
        // Far apart changes make two hunks.
        let old: String = (1..=40).map(|i| format!("l{i}\n")).collect();
        let new = old.replace("l3\n", "L3\n").replace("l38\n", "L38\n");
        assert_eq!(make_hunks(&old, &new, 3).len(), 2);
        round_trip(&old, &new);
    }

    #[test]
    fn newline_markers_crlf_and_unicode_survive_every_format() {
        // No final newline on either side, on one side, and a change of only the final newline.
        round_trip("a\nb", "a\nB");
        round_trip("a\nb\n", "a\nb");
        round_trip("a\nb", "a\nb\n");
        round_trip("only", "other");
        // CRLF: the \\r is part of each line and stays.
        round_trip("a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\n");
        round_trip("a\r\nb\r\n", "a\r\nb\r\nc");
        // Unicode text and lines that look like patch syntax.
        round_trip("čas\n日本語\n", "čas\n日本語 – ž\n");
        round_trip(
            "--- not a header\n+++ nor this\n@@ -1 +1 @@\n",
            "--- changed\n+++ nor this\n@@ -1 +1 @@\n",
        );
        let text = one(Format::Unified, "a\nb", "a\nB");
        assert!(text.contains("\\ No newline at end of file\n"), "{text}");
    }

    #[test]
    fn the_formats_print_what_standard_patch_tools_expect() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\ne\nf\n";
        assert_eq!(
            one(Format::Unified, old, new),
            "--- a/f.txt\n+++ b/f.txt\n@@ -1,5 +1,6 @@\n a\n-b\n+B\n c\n d\n e\n+f\n"
        );
        assert_eq!(
            one(Format::Context, old, new),
            "*** a/f.txt\n--- b/f.txt\n***************\n*** 1,5 ****\n  a\n! b\n  c\n  d\n  e\n--- 1,6 ----\n  a\n! B\n  c\n  d\n  e\n+ f\n"
        );
        assert_eq!(
            one(Format::Normal, old, new),
            "2c2\n< b\n---\n> B\n5a6\n> f\n"
        );
        // A deletion and an insertion at the top and in an empty file.
        assert_eq!(one(Format::Normal, "a\nb\nc\n", "a\nc\n"), "2d1\n< b\n");
        assert_eq!(one(Format::Normal, "", "x\ny\n"), "0a1,2\n> x\n> y\n");
        assert_eq!(
            one(Format::Unified, "", "x\n"),
            "--- a/f.txt\n+++ b/f.txt\n@@ -0,0 +1 @@\n+x\n"
        );
        // Nothing changed: nothing is printed.
        for format in [Format::Unified, Format::Context, Format::Normal] {
            assert_eq!(one(format, "same\n", "same\n"), "");
        }
    }

    #[test]
    fn a_context_hunk_without_deletions_or_insertions_omits_the_unused_side() {
        let text = one(Format::Context, "a\nb\n", "a\nb\nc\n");
        assert_eq!(text, "*** a/f.txt\n--- b/f.txt\n***************\n*** 1,2 ****\n--- 1,3 ----\n  a\n  b\n+ c\n");
        let text = one(Format::Context, "a\nb\nc\n", "a\nb\n");
        assert_eq!(text, "*** a/f.txt\n--- b/f.txt\n***************\n*** 1,3 ****\n  a\n  b\n- c\n--- 1,2 ----\n");
        for (old, new) in [("a\nb\n", "a\nb\nc\n"), ("a\nb\nc\n", "a\nb\n")] {
            round_trip(old, new);
        }
    }

    #[test]
    fn a_stale_source_is_a_mismatch_and_an_offset_is_taken_only_when_it_is_the_only_match() {
        let old = "a\nb\nc\nd\n";
        let new = "a\nB\nc\nd\n";
        let hunks = make_hunks(old, new, 1);
        // The file changed under the patch.
        assert_eq!(
            apply_hunks("a\nX\nc\nd\n", &hunks),
            Err(PatchError::Mismatch { hunk: 1 })
        );
        // Lines were added above: the hunk is found 2 lines lower, the only place it fits.
        let (text, offsets) = apply_hunks("p\nq\na\nb\nc\nd\n", &hunks).unwrap();
        assert_eq!(text, "p\nq\na\nB\nc\nd\n");
        assert_eq!(offsets, vec![2]);
        // The same context twice and not at the stated line: ambiguous, so nothing is applied.
        let ambiguous = "z\nz\nz\na\nb\nc\nz\nz\na\nb\nc\nd\n";
        let hunk = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                Line {
                    kind: Kind::Context,
                    text: "a".into(),
                    no_newline: false,
                },
                Line {
                    kind: Kind::Delete,
                    text: "b".into(),
                    no_newline: false,
                },
                Line {
                    kind: Kind::Insert,
                    text: "B".into(),
                    no_newline: false,
                },
                Line {
                    kind: Kind::Context,
                    text: "c".into(),
                    no_newline: false,
                },
            ],
        };
        assert_eq!(
            apply_hunks(ambiguous, std::slice::from_ref(&hunk)),
            Err(PatchError::Ambiguous { hunk: 1 })
        );
        // At the stated line it is not ambiguous, even if the text repeats elsewhere.
        let (text, _) = apply_hunks("a\nb\nc\nz\na\nb\nc\n", std::slice::from_ref(&hunk)).unwrap();
        assert_eq!(text, "a\nB\nc\nz\na\nb\nc\n");
        // A missing final newline is part of the match: the patch was made for a file with one.
        assert_eq!(
            apply_hunks("a\nb\nc\nd", &make_hunks(old, new, 3)),
            Err(PatchError::Mismatch { hunk: 1 })
        );
        // With less context the hunk does not reach the last line, so it still applies.
        assert!(apply_hunks("a\nb\nc\nd", &make_hunks(old, new, 1)).is_ok());
    }

    #[test]
    fn overlapping_and_out_of_order_hunks_are_refused() {
        let line = |kind, text: &str| Line {
            kind,
            text: text.into(),
            no_newline: false,
        };
        let first = Hunk {
            old_start: 2,
            new_start: 2,
            lines: vec![
                line(Kind::Context, "b"),
                line(Kind::Delete, "c"),
                line(Kind::Insert, "C"),
            ],
        };
        let overlapping = Hunk {
            old_start: 3,
            new_start: 3,
            lines: vec![line(Kind::Delete, "c"), line(Kind::Context, "d")],
        };
        assert_eq!(
            apply_hunks("a\nb\nc\nd\ne\n", &[first.clone(), overlapping]),
            Err(PatchError::Overlap { hunk: 2 })
        );
        let before = Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(Kind::Delete, "a"), line(Kind::Insert, "A")],
        };
        assert_eq!(
            apply_hunks("a\nb\nc\nd\ne\n", &[first, before]),
            Err(PatchError::Overlap { hunk: 2 })
        );
    }

    #[test]
    fn malformed_patches_are_refused_with_a_reason() {
        let bad_count = "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n";
        assert!(
            matches!(parse_unified(bad_count), Err(PatchError::Malformed(m)) if m.contains("header says"))
        );
        assert!(matches!(
            parse_unified("@@ -1 +1 @@\n a\n"),
            Err(PatchError::Malformed(_))
        ));
        assert!(matches!(
            parse_unified("--- a/f\n+++ b/f\n@@ nonsense @@\n"),
            Err(PatchError::Malformed(_))
        ));
        assert!(matches!(
            parse_unified("--- a/f\n+++ b/f\n@@ -1 +1 @@\n?bad\n"),
            Err(PatchError::Malformed(_))
        ));
        assert!(matches!(
            parse_context("***************\n*** 1 ****\n"),
            Err(PatchError::Malformed(_))
        ));
        assert!(matches!(
            parse_normal("this is not a patch\n"),
            Err(PatchError::Malformed(_))
        ));
        assert!(
            matches!(parse_normal("2c2\n< b\n> B\n"), Err(PatchError::Malformed(m)) if m.contains("---"))
        );
        assert!(matches!(
            parse_normal("1,3d0\n< a\n"),
            Err(PatchError::Malformed(_))
        ));
        // A range that ends before it starts, and a multi-byte character where a mark belongs, are errors, not panics.
        assert!(
            matches!(parse_normal("5,2d0\n< a\n"), Err(PatchError::Malformed(m)) if m.contains("ends before"))
        );
        assert!(matches!(
            parse_context("*** a\n--- b\n***************\n*** 1 ****\n\u{e9}x\n--- 1 ----\n"),
            Err(PatchError::Malformed(_))
        ));
        assert_eq!(parse_unified("").unwrap(), vec![]);
        assert_eq!(parse_unified("no patch here\n").unwrap(), vec![]);
    }

    #[test]
    fn binary_and_metadata_changes_are_refused_as_unsupported() {
        for text in [
            "diff --git a/x b/x\nBinary files a/x and b/x differ\n",
            "GIT binary patch\nliteral 0\n",
            "diff --git a/x b/x\nnew file mode 100644\n--- /dev/null\n+++ b/x\n@@ -0,0 +1 @@\n+x\n",
            "diff --git a/x b/x\ndeleted file mode 100644\n",
            "diff --git a/x b/y\nrename from x\nrename to y\n",
            "diff --git a/x b/x\nold mode 100644\nnew mode 100755\n",
        ] {
            assert!(
                matches!(parse_unified(text), Err(PatchError::Unsupported(_))),
                "{text}"
            );
        }
        // A created file by /dev/null, in unified and in context format.
        assert!(matches!(
            parse_unified("--- /dev/null\n+++ b/x\n@@ -0,0 +1 @@\n+x\n"),
            Err(PatchError::Unsupported(_))
        ));
        assert!(matches!(
            parse_context("*** /dev/null\n--- b/x\n"),
            Err(PatchError::Unsupported(_))
        ));
        // Git's `diff --git` and `index` lines on a text change are fine.
        let ok = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n";
        assert_eq!(parse_unified(ok).unwrap()[0].hunks.len(), 1);
    }

    #[test]
    fn the_names_in_headers_are_read_without_timestamps_and_the_format_is_sniffed() {
        let text = "--- old.txt\t2026-01-01 00:00:00\n+++ new.txt\t2026-01-02 00:00:00\n@@ -1 +1 @@\n-a\n+b\n";
        let patch = &parse_unified(text).unwrap()[0];
        assert_eq!(
            (patch.old_name.as_deref(), patch.new_name.as_deref()),
            (Some("old.txt"), Some("new.txt"))
        );
        assert_eq!(sniff("*** a\n--- b\n"), Some(Format::Context));
        assert_eq!(
            sniff("diff --git a b\n--- a\n+++ b\n"),
            Some(Format::Unified)
        );
        assert_eq!(sniff("@@ -1 +1 @@\n"), Some(Format::Unified));
        assert_eq!(sniff("2c2\n< a\n---\n> b\n"), Some(Format::Normal));
        assert_eq!(sniff("hello\n"), None);
        assert_eq!(sniff(""), None);
        // Each format of the same change is read as the same hunks.
        let (old, new) = ("a\nb\nc\n", "a\nX\nc\n");
        let content = |format| {
            parse(format, &one(format, old, new)).unwrap()[0]
                .hunks
                .iter()
                .flat_map(|h| {
                    h.lines
                        .iter()
                        .filter(|l| l.kind != Kind::Context)
                        .map(|l| (l.kind, l.text.clone()))
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(content(Format::Unified), content(Format::Context));
        assert_eq!(content(Format::Unified), content(Format::Normal));
    }

    fn tree() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        std::fs::create_dir_all(root.join("sub dir")).unwrap();
        (d, root)
    }

    #[test]
    fn targets_stay_below_the_root_and_are_never_links() {
        let (d, root) = tree();
        std::fs::write(root.join("a.txt"), "x\n").unwrap();
        std::fs::write(root.join("sub dir/čas.txt"), "x\n").unwrap();
        std::fs::write(d.path().join("outside.txt"), "secret\n").unwrap();
        // Git's a/ and b/ are stripped on their own; spaces and Unicode are fine.
        assert_eq!(
            resolve_target(&root, "a/a.txt", None).unwrap(),
            root.join("a.txt")
        );
        assert_eq!(
            resolve_target(&root, "b/sub dir/čas.txt", None).unwrap(),
            root.join("sub dir/čas.txt")
        );
        assert_eq!(
            resolve_target(&root, "a.txt", None).unwrap(),
            root.join("a.txt")
        );
        assert_eq!(
            resolve_target(&root, "x/y/a.txt", Some(2)).unwrap(),
            root.join("a.txt")
        );
        // Everything that leaves the root, or is no plain name, is refused.
        for name in [
            "/etc/passwd",
            "../outside.txt",
            "a/../../outside.txt",
            "sub dir/../../outside.txt",
            "C:/x",
            "C:\\x",
            "a/nul\0.txt",
            "",
            "a/",
            "missing.txt",
            "sub dir",
        ] {
            assert!(resolve_target(&root, name, None).is_err(), "{name:?}");
        }
        assert!(resolve_target(&root, "a/a.txt", Some(5)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_as_target_or_as_folder_is_refused() {
        let (d, root) = tree();
        std::fs::write(d.path().join("outside.txt"), "secret\n").unwrap();
        std::os::unix::fs::symlink(d.path().join("outside.txt"), root.join("link.txt")).unwrap();
        std::fs::create_dir(d.path().join("elsewhere")).unwrap();
        std::fs::write(d.path().join("elsewhere/f.txt"), "x\n").unwrap();
        std::os::unix::fs::symlink(d.path().join("elsewhere"), root.join("out")).unwrap();
        let file = resolve_target(&root, "link.txt", None)
            .unwrap_err()
            .to_string();
        assert!(file.contains("link"), "{file}");
        let folder = resolve_target(&root, "out/f.txt", None)
            .unwrap_err()
            .to_string();
        assert!(folder.contains("leads out"), "{folder}");
        // A link inside the root that stays inside is fine as a folder.
        std::os::unix::fs::symlink(root.join("sub dir"), root.join("inner")).unwrap();
        std::fs::write(root.join("sub dir/ok.txt"), "x\n").unwrap();
        assert!(resolve_target(&root, "inner/ok.txt", None).is_ok());
    }

    fn patch_for(name: &str, old: &str, new: &str) -> FilePatch {
        make_patch(
            &format!("a/{name}"),
            &format!("b/{name}"),
            old,
            new,
            Format::Unified,
        )
    }

    #[test]
    fn a_plan_shows_everything_that_is_wrong_and_writes_nothing() {
        let (_d, root) = tree();
        std::fs::write(root.join("good.txt"), "a\nb\nc\n").unwrap();
        std::fs::write(root.join("stale.txt"), "changed\nb\nc\n").unwrap();
        let patches = vec![
            patch_for("good.txt", "a\nb\nc\n", "a\nB\nc\n"),
            patch_for("stale.txt", "a\nb\nc\n", "a\nB\nc\n"),
            patch_for("missing.txt", "a\n", "b\n"),
            FilePatch {
                old_name: Some("../escape.txt".into()),
                new_name: None,
                hunks: make_hunks("a\n", "b\n", 3),
            },
        ];
        let problems = plan(&root, &patches, None).unwrap_err();
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems
            .iter()
            .any(|p| p.contains("stale.txt") && p.contains("does not match")));
        assert!(problems.iter().any(|p| p.contains("missing.txt")));
        assert!(problems.iter().any(|p| p.contains("escape.txt")));
        // The good file was not touched by the failed preflight.
        assert_eq!(
            std::fs::read_to_string(root.join("good.txt")).unwrap(),
            "a\nb\nc\n"
        );
        // Two patches for one file are refused, not merged.
        let twice = plan(
            &root,
            &[
                patch_for("good.txt", "a\nb\nc\n", "a\nB\nc\n"),
                patch_for("good.txt", "a\nb\nc\n", "x\n"),
            ],
            None,
        )
        .unwrap_err();
        assert!(twice.iter().any(|p| p.contains("same file")), "{twice:?}");
        // A binary file is not patched as text.
        std::fs::write(root.join("bin.dat"), [0u8, 1, 2]).unwrap();
        let binary = plan(&root, &[patch_for("bin.dat", "a\n", "b\n")], None).unwrap_err();
        assert!(binary[0].contains("binary"), "{binary:?}");
    }

    #[test]
    fn applying_a_plan_writes_every_file_keeps_crlf_and_refuses_a_file_that_changed_since_the_preview(
    ) {
        let (_d, root) = tree();
        std::fs::write(root.join("one.txt"), "a\r\nb\r\nc\r\n").unwrap();
        std::fs::write(root.join("sub dir/two.txt"), "x\ny").unwrap();
        let patches = vec![
            patch_for("one.txt", "a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\n"),
            patch_for("sub dir/two.txt", "x\ny", "x\nY"),
        ];
        let plan = plan(&root, &patches, None).unwrap();
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.files[0].hunks, 1);
        // The source changes after the preview: nothing is written.
        std::fs::write(root.join("one.txt"), "a\r\nb\r\nc\r\nmore\r\n").unwrap();
        let failure = apply_plan(&plan).unwrap_err();
        assert!(
            failure.error.contains("changed since the preview"),
            "{}",
            failure.error
        );
        assert_eq!(
            std::fs::read_to_string(root.join("sub dir/two.txt")).unwrap(),
            "x\ny"
        );
        // A fresh preview applies.
        std::fs::write(root.join("one.txt"), "a\r\nb\r\nc\r\n").unwrap();
        let plan = super::plan(&root, &patches, None).unwrap();
        let written = apply_plan(&plan).unwrap();
        assert_eq!(written.len(), 2);
        assert_eq!(
            std::fs::read(root.join("one.txt")).unwrap(),
            b"a\r\nB\r\nc\r\n"
        );
        assert_eq!(
            std::fs::read(root.join("sub dir/two.txt")).unwrap(),
            b"x\nY"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_write_puts_back_the_files_already_written() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, root) = tree();
        std::fs::write(root.join("first.txt"), "a\nb\n").unwrap();
        std::fs::write(root.join("sub dir/second.txt"), "a\nb\n").unwrap();
        let patches = vec![
            patch_for("first.txt", "a\nb\n", "a\nB\n"),
            patch_for("sub dir/second.txt", "a\nb\n", "a\nB\n"),
        ];
        let plan = plan(&root, &patches, None).unwrap();
        // The second file's folder cannot be written to: its temporary file cannot be created.
        let folder = root.join("sub dir");
        let saved = std::fs::metadata(&folder).unwrap().permissions();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        let can_write = std::fs::File::create(folder.join(".probe")).is_ok();
        let result = apply_plan(&plan);
        std::fs::set_permissions(&folder, saved).unwrap();
        if can_write {
            let _ = std::fs::remove_file(folder.join(".probe"));
            eprintln!("this fixture needs a user who cannot bypass folder permissions");
            return;
        }
        let failure = result.unwrap_err();
        assert_eq!(failure.failed, root.join("sub dir/second.txt"));
        assert_eq!(failure.restored, vec![root.join("first.txt")]);
        assert!(failure.not_restored.is_empty());
        // Both files are as they were.
        assert_eq!(
            std::fs::read_to_string(root.join("first.txt")).unwrap(),
            "a\nb\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("sub dir/second.txt")).unwrap(),
            "a\nb\n"
        );
    }

    #[test]
    fn quoted_header_names_are_read_as_diff_and_git_write_them() {
        assert_eq!(unquote("plain"), "plain");
        assert_eq!(unquote("\"a/my file.txt\""), "a/my file.txt");
        assert_eq!(unquote("\"a/f\\303\\244ile.txt\""), "a/fäile.txt");
        assert_eq!(unquote("\"say \\\"hi\\\".txt\""), "say \"hi\".txt");
        assert_eq!(unquote("\"back\\\\slash\""), "back\\slash");
        assert_eq!(unquote("\"tab\\there\\nnew\""), "tab\there\nnew");
        // Not valid UTF-8 after unescaping, an unfinished quote and a lone backslash stay as written.
        assert_eq!(unquote("\"\\377\\376\""), "\"\\377\\376\"");
        assert_eq!(unquote("\"open"), "\"open");
        assert_eq!(unquote("\"end\\\""), "\"end\\\"");
        // The timestamp after the tab is not part of a quoted name either.
        assert_eq!(
            header_name("\"a/my file.txt\"\t2026-11-24 20:48:29.068858332 +0100\n").as_deref(),
            Some("a/my file.txt")
        );
        let patches = parse_unified("--- \"a/my file.txt\"\t2026-01-01 00:00:00\n+++ \"b/my file.txt\"\t2026-01-01 00:00:00\n@@ -1 +1 @@\n-x\n+y\n").unwrap();
        assert_eq!(patches[0].old_name.as_deref(), Some("a/my file.txt"));
        assert_eq!(patches[0].new_name.as_deref(), Some("b/my file.txt"));
    }

    #[test]
    fn names_that_a_bare_header_would_cut_are_written_quoted_and_read_back() {
        assert_eq!(header_text(Some("a/plain.txt")), "a/plain.txt");
        assert_eq!(header_text(Some("a/fäile.txt")), "a/fäile.txt");
        assert_eq!(header_text(None), "/dev/null");
        assert_eq!(header_text(Some("a/my file.txt")), "\"a/my file.txt\"");
        assert_eq!(
            header_text(Some("a/say \"hi\"\tx\\y\u{1}.txt")),
            "\"a/say \\\"hi\\\"\\tx\\\\y\\001.txt\""
        );
        for name in [
            "a/my file.txt",
            "a/tab\there.txt",
            "a/say \"hi\".txt",
            "a/back\\slash ü.txt",
            "a/new\nline",
        ] {
            for format in [Format::Unified, Format::Context] {
                let text = write(format, &[make_patch(name, name, "x\n", "y\n", format)]);
                let patches = parse(format, &text).unwrap();
                assert_eq!(
                    patches[0].old_name.as_deref(),
                    Some(name),
                    "{format:?}\n{text}"
                );
                assert_eq!(
                    patches[0].new_name.as_deref(),
                    Some(name),
                    "{format:?}\n{text}"
                );
            }
        }
    }

    #[test]
    fn a_backslash_separates_folders_only_in_a_windows_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("f.txt"), "a\n").unwrap();
        if cfg!(windows) {
            // `a\sub\f.txt` is three folders deep: the automatic strip drops `a`, and a strip counts the same parts.
            let wanted = root.join("sub").join("f.txt");
            assert_eq!(resolve_target(root, "a\\sub\\f.txt", None).unwrap(), wanted);
            assert_eq!(
                resolve_target(root, "x\\y\\sub\\f.txt", Some(2)).unwrap(),
                wanted
            );
            assert_eq!(
                resolve_target(root, "a/sub\\f.txt", Some(1)).unwrap(),
                wanted
            );
            assert!(resolve_target(root, "a\\..\\..\\f.txt", Some(1)).is_err());
            assert!(resolve_target(root, "\\sub\\f.txt", Some(0)).is_err());
        } else {
            // Elsewhere it is a character of the name: one file, `a\f.txt`, not `f.txt` in a folder `a`.
            std::fs::write(root.join("a\\f.txt"), "a\n").unwrap();
            assert_eq!(
                resolve_target(root, "a\\f.txt", None).unwrap(),
                root.join("a\\f.txt")
            );
            assert_eq!(
                resolve_target(root, "b/a\\f.txt", None).unwrap(),
                root.join("a\\f.txt")
            );
            assert!(resolve_target(root, "sub\\f.txt", Some(0)).is_err());
        }
    }

    #[test]
    fn an_octal_escape_past_a_byte_and_a_second_spelling_of_one_file_are_refused() {
        assert_eq!(unquote("\"a/\\457x\""), "\"a/\\457x\"");
        assert_eq!(
            unquote("\"a/\\377\""),
            "\"a/\\377\"",
            "a lone 0xff is not UTF-8"
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "a\n").unwrap();
        // Two patches for one file, the second named through a hard link: one file, refused.
        #[cfg(unix)]
        {
            std::fs::hard_link(dir.path().join("f.txt"), dir.path().join("g.txt")).unwrap();
            let make = |name: &str| make_patch(name, name, "a\n", "b\n", Format::Unified);
            let problems = plan(dir.path(), &[make("f.txt"), make("g.txt")], None).unwrap_err();
            assert!(
                problems
                    .iter()
                    .any(|p| p.contains("same file") || p.contains("link")),
                "{problems:?}"
            );
        }
    }
}
