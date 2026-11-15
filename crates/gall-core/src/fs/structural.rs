//! Structural comparison: the changed lines of a line diff are compared again by their
//! tokens (words, operators, brackets, strings, comments), so that a change of layout
//! alone (line breaks, indentation, alignment, blank lines) is told apart from a
//! change of content, and only the tokens that really changed are marked.
//!
//! The tokens come from a small lexer of its own, not from a parser: it knows, per
//! language family, the comments, the string literals and whether a line break or the
//! indentation means something. Where it cannot be sure it counts whitespace as
//! content, so a real change is not passed off as layout. It reads a file from its
//! start (a block comment or a string may begin anywhere above a change) but keeps the
//! tokens of the changed lines only.

use std::borrow::Cow;
use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput, Interner};

use super::diff::{join_gaps, push_span, Spans};

/// What a line break means to the language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Breaks {
    /// Nothing: statements end with `;` or a bracket (C, Rust, JSON, CSS).
    Free,
    /// It may end a statement, unless the line clearly goes on (JavaScript, Go, Kotlin).
    Soft,
    /// It ends the line's statement (Python, shell, YAML, text).
    Hard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Quote {
    open: &'static str,
    close: &'static str,
    /// A backslash takes the next character with it.
    escape: bool,
    /// The literal may run over several lines.
    multiline: bool,
}

const fn quote(open: &'static str, escape: bool, multiline: bool) -> Quote {
    Quote {
        open,
        close: open,
        escape,
        multiline,
    }
}

const DQ: Quote = quote("\"", true, false);
const SQ: Quote = quote("'", true, false);
const DQ_LINES: Quote = quote("\"", true, true);
const SQ_LINES: Quote = quote("'", true, true);
const SQ_RAW: Quote = quote("'", false, true);
const TICK: Quote = quote("`", true, true);
const TICK_RAW: Quote = quote("`", false, true);
const DQ3: Quote = quote("\"\"\"", true, true);
const SQ3: Quote = quote("'''", true, true);

/// How one language family is split into tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Syntax {
    line_comments: &'static [&'static str],
    block_comments: &'static [(&'static str, &'static str)],
    strings: &'static [Quote],
    breaks: Breaks,
    /// Leading whitespace is structure (Python, YAML, Makefile).
    indent: bool,
    /// Whether two tokens touch matters: `a-b` is not `a - b` (shell, CSS, Lisp).
    glue: bool,
    /// Every run of whitespace is content (plain text, unknown files).
    spaces: bool,
    /// A line starting with `#` ends at its line break (C preprocessor).
    preprocessor: bool,
    /// Line breaks inside `{}` mean nothing, as inside `()` and `[]` (Python).
    data_braces: bool,
    /// Rust: `'a` is a lifetime, `r#"…"#` a raw string.
    rust: bool,
    /// A slash may open a regex. Preserve its line literally when the grammar is ambiguous.
    regex: bool,
}

const PLAIN: Syntax = Syntax {
    line_comments: &[],
    block_comments: &[],
    strings: &[],
    breaks: Breaks::Hard,
    indent: false,
    glue: false,
    spaces: true,
    preprocessor: false,
    data_braces: false,
    rust: false,
    regex: false,
};
const C_LIKE: Syntax = Syntax {
    line_comments: &["//"],
    block_comments: &[("/*", "*/")],
    strings: &[DQ, SQ],
    breaks: Breaks::Free,
    spaces: false,
    ..PLAIN
};
const C: Syntax = Syntax {
    preprocessor: true,
    ..C_LIKE
};
const RUST: Syntax = Syntax {
    strings: &[DQ_LINES],
    rust: true,
    ..C_LIKE
};
const PHP: Syntax = Syntax {
    line_comments: &["//", "#"],
    strings: &[DQ_LINES, SQ_LINES],
    ..C_LIKE
};
const CSS: Syntax = Syntax {
    line_comments: &[],
    glue: true,
    ..C_LIKE
};
const SQL: Syntax = Syntax {
    line_comments: &["--"],
    strings: &[SQ_RAW, DQ],
    ..C_LIKE
};
const LISP: Syntax = Syntax {
    line_comments: &[";"],
    block_comments: &[],
    strings: &[DQ_LINES],
    glue: true,
    ..C_LIKE
};
const SCRIPT: Syntax = Syntax {
    strings: &[DQ, SQ, TICK],
    breaks: Breaks::Soft,
    regex: true,
    ..C_LIKE
};
const GO: Syntax = Syntax {
    strings: &[DQ, SQ, TICK_RAW],
    regex: false,
    ..SCRIPT
};
const KOTLIN: Syntax = Syntax {
    strings: &[DQ3, DQ, SQ],
    regex: false,
    ..SCRIPT
};
const HASH: Syntax = Syntax {
    line_comments: &["#"],
    strings: &[DQ, SQ],
    glue: true,
    spaces: false,
    ..PLAIN
};
const PYTHON: Syntax = Syntax {
    strings: &[DQ3, SQ3, DQ, SQ],
    indent: true,
    glue: false,
    data_braces: true,
    ..HASH
};
const SHELL: Syntax = Syntax {
    strings: &[DQ_LINES, SQ_RAW],
    ..HASH
};
// Scalar bodies can contain quotes and comment markers as literal data.
const YAML: Syntax = PLAIN;
const TOML: Syntax = Syntax {
    strings: &[DQ3, SQ3, DQ, SQ],
    ..HASH
};
// Unquoted values may contain spaces, quotes and comment markers as literal data.
const INI: Syntax = PLAIN;
const LUA: Syntax = Syntax {
    line_comments: &["--"],
    block_comments: &[("--[[", "]]")],
    glue: false,
    ..HASH
};
const HASKELL: Syntax = Syntax {
    line_comments: &["--"],
    block_comments: &[("{-", "-}")],
    strings: &[DQ],
    indent: true,
    ..HASH
};
const MARKUP: Syntax = Syntax {
    line_comments: &[],
    block_comments: &[("<!--", "-->")],
    spaces: true,
    ..HASH
};
const TEX: Syntax = Syntax {
    line_comments: &["%"],
    strings: &[],
    ..HASH
};
const PROSE: Syntax = Syntax {
    line_comments: &[],
    strings: &[],
    indent: true,
    spaces: true,
    ..HASH
};

impl Syntax {
    /// The token rules for a file name; `None` when the name says nothing.
    pub fn for_name(name: &str) -> Option<Syntax> {
        let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let lower = file.to_ascii_lowercase();
        match lower.as_str() {
            "makefile" | "gnumakefile" => return Some(YAML),
            "dockerfile" | "containerfile" | "cmakelists.txt" | "gemfile" | "rakefile" => {
                return Some(HASH)
            }
            ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" | ".zprofile" | ".zshenv" => {
                return Some(SHELL)
            }
            _ => {}
        }
        let (_, ext) = lower.rsplit_once('.')?;
        Some(match ext {
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "m" | "mm" | "cs" => C,
            "java" | "dart" | "glsl" | "hlsl" | "wgsl" | "proto" | "json" | "jsonc" | "json5" => {
                C_LIKE
            }
            "rs" => RUST,
            "php" => PHP,
            "css" | "scss" | "less" => CSS,
            "sql" => SQL,
            "clj" | "cljs" | "edn" | "el" | "lisp" | "scm" => LISP,
            "js" | "mjs" | "cjs" | "ts" | "mts" | "cts" => SCRIPT,
            // JSX text and embedded languages need a parser to distinguish content from layout.
            "jsx" | "tsx" => PLAIN,
            "go" => GO,
            "kt" | "kts" | "swift" | "scala" | "sc" | "groovy" | "gradle" => KOTLIN,
            "py" | "pyi" | "pyw" => PYTHON,
            "sh" | "bash" | "zsh" | "fish" | "ksh" => SHELL,
            "rb" | "pl" | "pm" | "r" | "tcl" | "cmake" | "ps1" | "nix" => HASH,
            "yaml" | "yml" | "mk" | "mak" | "coffee" | "nim" => YAML,
            "toml" => TOML,
            "ini" | "cfg" | "conf" | "properties" | "desktop" | "editorconfig" => INI,
            "lua" => LUA,
            "hs" => HASKELL,
            "html" | "htm" | "xhtml" | "xml" | "svg" | "vue" | "svelte" | "xsl" | "plist" => MARKUP,
            "tex" | "sty" | "cls" => TEX,
            "md" | "markdown" | "rst" | "adoc" | "org" => PROSE,
            _ => return None,
        })
    }

    /// The rules of the first name that has some, else those of plain text.
    pub fn for_names(names: &[&str]) -> Syntax {
        names
            .iter()
            .find_map(|name| Syntax::for_name(name))
            .unwrap_or(PLAIN)
    }
}

const CODE: u8 = 0;
const STRING: u8 = 1;
const COMMENT: u8 = 2;
/// A line break that means something; no width.
const BREAK: u8 = 3;
/// The indentation of a line whose indentation means something.
const INDENT: u8 = 4;
const CLASS: u8 = 0x0f;
/// Whitespace (or the line start) comes before the token; set where touching matters.
const SPACED: u8 = 0x10;

#[derive(Debug, Clone, Copy)]
struct Token {
    line: u32,
    /// Byte range within the line.
    start: u32,
    end: u32,
    class: u8,
}

impl Token {
    fn text<'a>(&self, lines: &'a [String]) -> &'a str {
        &lines[self.line as usize][self.start as usize..self.end as usize]
    }
    /// Has text of its own on the line (a break has none, an indentation may).
    fn visible(&self) -> bool {
        self.end > self.start
    }
    /// A word, operator, bracket or piece of a string or comment.
    fn real(&self) -> bool {
        self.visible() && self.class & CLASS != INDENT
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Code,
    /// Inside the block comment of this index.
    Block(usize),
    /// Inside the string literal of this index.
    Str(usize),
    /// Inside a Rust raw string closed by a quote and this many `#`.
    Raw(usize),
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_operator(c: char) -> bool {
    "+-*/%=<>!&|^~?:.".contains(c)
}

struct Lexer<'a> {
    syntax: Syntax,
    ignore_space: bool,
    lines: &'a [String],
    mode: Mode,
    /// Open `(` and `[` (and `{` where they hold data).
    depth: u32,
    /// A soft break of an earlier line, decided by the next token: (line, column, kept).
    pending: Option<(u32, u32, bool)>,
    /// State of the line being read.
    line: u32,
    keep: bool,
    spaced: bool,
    /// The line had a token; `indent_at` is where its indentation ends, if that counts.
    any: bool,
    indent_at: Option<u32>,
    last: Option<Token>,
    out: Vec<Token>,
}

impl<'a> Lexer<'a> {
    fn new(lines: &'a [String], syntax: Syntax, ignore_space: bool) -> Self {
        Lexer {
            syntax,
            ignore_space,
            lines,
            mode: Mode::Code,
            depth: 0,
            pending: None,
            line: 0,
            keep: false,
            spaced: true,
            any: false,
            indent_at: None,
            last: None,
            out: Vec::new(),
        }
    }

    fn record(&mut self, token: Token, keep: bool) {
        if keep {
            self.out.push(token);
        }
    }

    /// Adds a token of the current line.
    fn push(&mut self, start: usize, end: usize, class: u8) {
        let mut token = Token {
            line: self.line,
            start: start as u32,
            end: end as u32,
            class,
        };
        if let Some((line, column, keep)) = self.pending.take() {
            // The earlier line ended where a statement may end: the break counts unless
            // this token can only continue that statement.
            let text = token.text(self.lines);
            let goes_on = class == CODE && text.starts_with(|c| ".,)]}?:=|^%>;".contains(c));
            if !goes_on {
                let token = Token {
                    line,
                    start: column,
                    end: column,
                    class: BREAK,
                };
                self.record(token, keep);
            }
        }
        if !self.any {
            self.any = true;
            if let Some(end) = self.indent_at {
                let indent = Token {
                    line: self.line,
                    start: 0,
                    end,
                    class: INDENT,
                };
                self.record(indent, self.keep);
            }
        }
        if self.syntax.glue && !self.ignore_space && self.spaced {
            token.class |= SPACED;
        }
        self.spaced = false;
        self.last = Some(token);
        self.record(token, self.keep);
    }

    fn bracket(&mut self, c: char) {
        let counted = match c {
            '(' | '[' | ')' | ']' => true,
            '{' | '}' => self.syntax.data_braces,
            _ => false,
        };
        if counted {
            if matches!(c, '(' | '[' | '{') {
                self.depth += 1;
            } else {
                self.depth = self.depth.saturating_sub(1);
            }
        }
    }

    /// Words and single characters of `text[from..to]`, inside a comment or a string.
    /// In a string every run of whitespace is a token too.
    fn pieces(&mut self, text: &str, from: usize, to: usize, class: u8, escape: bool) {
        let mut i = from;
        while i < to {
            let c = text[i..].chars().next().unwrap_or(' ');
            let mut j = i + c.len_utf8();
            if c.is_whitespace() {
                while j < to && text[j..].starts_with(char::is_whitespace) {
                    j += text[j..].chars().next().map_or(1, char::len_utf8);
                }
                if class == STRING && !self.ignore_space {
                    self.push(i, j, class);
                }
                self.spaced = true;
            } else if is_word(c) {
                while j < to && text[j..].starts_with(is_word) {
                    j += text[j..].chars().next().map_or(1, char::len_utf8);
                }
                self.push(i, j, class);
            } else {
                if escape && c == '\\' && j < to {
                    j += text[j..].chars().next().map_or(1, char::len_utf8);
                }
                self.push(i, j, class);
            }
            i = j;
        }
    }

    /// Where the literal opened before `from` closes on this line.
    fn closing(text: &str, from: usize, quote: &Quote) -> Option<usize> {
        let bytes = text.as_bytes();
        let mut i = from;
        while i < bytes.len() {
            if quote.escape && bytes[i] == b'\\' {
                i += 2;
            } else if text.is_char_boundary(i) && text[i..].starts_with(quote.close) {
                return Some(i);
            } else {
                i += 1;
            }
        }
        None
    }

    fn opens_comment(&self, rest: &str) -> bool {
        self.syntax
            .line_comments
            .iter()
            .any(|open| rest.starts_with(open))
            || self
                .syntax
                .block_comments
                .iter()
                .any(|(open, _)| rest.starts_with(open))
    }

    fn read_line(&mut self, n: usize, keep: bool) {
        let lines = self.lines;
        let text = lines[n].as_str();
        let syntax = self.syntax;
        self.line = n as u32;
        self.keep = keep;
        self.spaced = true;
        self.any = false;
        self.last = None;
        let in_code = self.mode == Mode::Code;
        let indent = text.len() - text.trim_start().len();
        self.indent_at = (syntax.indent && !self.ignore_space && in_code && self.depth == 0)
            .then_some(indent as u32);
        let directive = syntax.preprocessor && in_code && text[indent..].starts_with('#');
        let mut i = 0;
        'line: while i < text.len() {
            match self.mode {
                Mode::Block(k) => {
                    let close = syntax.block_comments[k].1;
                    match text[i..].find(close) {
                        Some(at) => {
                            self.pieces(text, i, i + at, COMMENT, false);
                            self.push(i + at, i + at + close.len(), COMMENT);
                            self.mode = Mode::Code;
                            i += at + close.len();
                        }
                        None => {
                            self.pieces(text, i, text.len(), COMMENT, false);
                            i = text.len();
                        }
                    }
                }
                Mode::Str(k) => {
                    let quote = syntax.strings[k];
                    match Self::closing(text, i, &quote) {
                        Some(at) => {
                            self.pieces(text, i, at, STRING, quote.escape);
                            self.push(at, at + quote.close.len(), STRING);
                            self.mode = Mode::Code;
                            i = at + quote.close.len();
                        }
                        None => {
                            self.pieces(text, i, text.len(), STRING, quote.escape);
                            i = text.len();
                        }
                    }
                }
                Mode::Raw(hashes) => {
                    let close = format!("\"{}", "#".repeat(hashes));
                    match text[i..].find(&close) {
                        Some(at) => {
                            self.pieces(text, i, i + at, STRING, false);
                            self.push(i + at, i + at + close.len(), STRING);
                            self.mode = Mode::Code;
                            i += at + close.len();
                        }
                        None => {
                            self.pieces(text, i, text.len(), STRING, false);
                            i = text.len();
                        }
                    }
                }
                Mode::Code => {
                    let rest = &text[i..];
                    let c = rest.chars().next().unwrap_or(' ');
                    if c.is_whitespace() {
                        let len = rest.len() - rest.trim_start().len();
                        if (syntax.spaces || directive) && !self.ignore_space {
                            self.push(i, i + len, CODE);
                        }
                        self.spaced = true;
                        i += len;
                        continue;
                    }
                    for (k, (open, _)) in syntax.block_comments.iter().enumerate() {
                        if rest.starts_with(open) {
                            self.push(i, i + open.len(), COMMENT);
                            self.mode = Mode::Block(k);
                            i += open.len();
                            continue 'line;
                        }
                    }
                    for open in syntax.line_comments {
                        if rest.starts_with(open) {
                            self.push(i, i + open.len(), COMMENT);
                            self.pieces(text, i + open.len(), text.len(), COMMENT, false);
                            i = text.len();
                            continue 'line;
                        }
                    }
                    if syntax.regex && c == '/' {
                        // Regex-vs-division requires grammar context. Keep the remainder
                        // literal rather than discarding potentially meaningful whitespace.
                        self.pieces(text, i, text.len(), STRING, false);
                        i = text.len();
                        continue;
                    }
                    for (k, quote) in syntax.strings.iter().enumerate() {
                        if !rest.starts_with(quote.open) {
                            continue;
                        }
                        let inside = i + quote.open.len();
                        // A quote that does not close on its line is no literal (an
                        // apostrophe in text) unless the literal may span lines.
                        if quote.multiline || Self::closing(text, inside, quote).is_some() {
                            self.push(i, inside, STRING);
                            self.mode = Mode::Str(k);
                            i = inside;
                            continue 'line;
                        }
                    }
                    if syntax.rust && c == '\'' {
                        // A character literal is one token; a lifetime's quote is just a mark.
                        let mut after = rest[1..].chars();
                        let end = match (after.next(), after.next()) {
                            (Some('\\'), _) => rest[2..].find('\'').map(|at| at + 3),
                            (Some(ch), Some('\'')) => Some(1 + ch.len_utf8() + 1),
                            _ => None,
                        };
                        match end {
                            Some(end) => self.push(i, i + end, STRING),
                            None => self.push(i, i + 1, CODE),
                        }
                        i += end.unwrap_or(1);
                        continue;
                    }
                    let mut j = i + c.len_utf8();
                    if is_word(c) {
                        while text[j..].starts_with(is_word) {
                            j += text[j..].chars().next().map_or(1, char::len_utf8);
                        }
                        self.push(i, j, CODE);
                        if syntax.rust && matches!(&text[i..j], "r" | "br" | "cr") {
                            let hashes = text[j..].len() - text[j..].trim_start_matches('#').len();
                            if text[j + hashes..].starts_with('"') {
                                self.push(j, j + hashes + 1, STRING);
                                self.mode = Mode::Raw(hashes);
                                j += hashes + 1;
                            }
                        }
                    } else if is_operator(c) {
                        // Operators stay whole (`a - -b` is not `a --b`), up to a comment.
                        while text[j..].starts_with(is_operator)
                            && !self.opens_comment(&text[j..])
                            && !(syntax.regex && text[j..].starts_with('/'))
                        {
                            j += 1;
                        }
                        self.push(i, j, CODE);
                    } else {
                        self.push(i, j, CODE);
                        self.bracket(c);
                    }
                    i = j;
                }
            }
        }
        match self.mode {
            // A line break inside a literal is part of it.
            Mode::Str(k) if syntax.strings[k].multiline => self.line_end(text.len(), STRING),
            Mode::Raw(_) => self.line_end(text.len(), STRING),
            Mode::Str(_) => self.mode = Mode::Code,
            Mode::Block(_) => {}
            Mode::Code => {
                let Some(last) = self.last else {
                    // A blank line is layout, except where all whitespace is content.
                    if syntax.spaces && !self.ignore_space {
                        self.line_end(text.len(), BREAK);
                    }
                    return;
                };
                let last_text = last.text(lines);
                let code = last.class & CLASS == CODE;
                let continued = code && last_text == "\\";
                let counts = match syntax.breaks {
                    _ if directive => !continued,
                    Breaks::Free => false,
                    Breaks::Hard => self.depth == 0 && !continued,
                    Breaks::Soft => {
                        if self.depth == 0
                            && (!code
                                || last_text.starts_with(is_word)
                                || matches!(last_text, ")" | "]" | "}"))
                        {
                            self.pending = Some((self.line, text.len() as u32, keep));
                        }
                        false
                    }
                };
                if counts {
                    self.line_end(text.len(), BREAK);
                }
            }
        }
    }

    /// A break that counts, at the end of the current line.
    fn line_end(&mut self, at: usize, class: u8) {
        let token = Token {
            line: self.line,
            start: at as u32,
            end: at as u32,
            class,
        };
        // Not through `push`: a break is no token of the line (no indentation before it).
        self.pending = None;
        self.record(token, self.keep);
    }
}

/// The tokens of the `wanted` line ranges (sorted, not overlapping). The lines above
/// them are read too, for the state they leave behind, but not kept.
fn lex(
    lines: &[String],
    syntax: Syntax,
    ignore_space: bool,
    wanted: &[Range<usize>],
) -> Vec<Token> {
    let mut lexer = Lexer::new(lines, syntax, ignore_space);
    let mut ranges = wanted.iter().filter(|r| !r.is_empty()).peekable();
    for n in 0..lines.len() {
        while ranges.peek().is_some_and(|r| r.end <= n) {
            ranges.next();
        }
        let Some(range) = ranges.peek() else {
            // Past the last wanted line: only a break still waiting for its verdict.
            if lexer.pending.is_none() {
                break;
            }
            lexer.read_line(n, false);
            continue;
        };
        lexer.read_line(n, range.contains(&n));
    }
    if let Some((line, column, keep)) = lexer.pending.take() {
        let token = Token {
            line,
            start: column,
            end: column,
            class: BREAK,
        };
        lexer.record(token, keep);
    }
    // A waiting break is written when the next token arrives, which can be after
    // tokens of a later line were already kept: never, since it is written first.
    lexer.out
}

/// How one line of a changed block differs from the other side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mark {
    /// Everything on it changed (or it exists on this side only).
    Whole,
    /// Nothing on it changed: only its layout differs.
    Same,
    /// These ranges changed (UTF-16 offsets).
    Part(Spans),
}

/// Most tokens (both sides together) of one changed block that are still compared.
const MAX_TOKENS: usize = 400_000;
/// Work allowed for moving changed runs to where they read best, per block side.
const SETTLE_BUDGET: usize = 200_000;

fn tokens_of<'a>(tokens: &'a [Token], lines: &Range<usize>) -> &'a [Token] {
    let from = tokens.partition_point(|t| (t.line as usize) < lines.start);
    let to = tokens.partition_point(|t| (t.line as usize) < lines.end);
    &tokens[from..to]
}

fn key<'a>(token: &Token, lines: &'a [String], ignore_case: bool) -> (u8, Cow<'a, str>) {
    let text = token.text(lines);
    let text = if ignore_case {
        Cow::Owned(text.to_lowercase())
    } else {
        Cow::Borrowed(text)
    };
    (token.class, text)
}

/// A run of changed tokens between equal neighbours can often sit in several places
/// (a new function: `fn b() { … }` or `} fn b() { …`). Moves each run to where its
/// brackets pair up, then to whole lines, then as far down as it goes.
fn settle(keys: &[imara_diff::Token], changed: &mut [bool], tokens: &[Token], lines: &[String]) {
    let n = keys.len();
    let balanced = |range: Range<usize>| {
        let mut depth = 0i32;
        for token in &tokens[range] {
            if token.class & CLASS != CODE {
                continue;
            }
            match token.text(lines) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                _ => {}
            }
            if depth < 0 {
                return false;
            }
        }
        depth == 0
    };
    let starts_line = |i: usize| i == 0 || tokens[i - 1].line != tokens[i].line;
    let ends_line = |i: usize| i + 1 >= n || tokens[i + 1].line != tokens[i].line;
    let mut budget = SETTLE_BUDGET;
    let (mut start, mut floor) = (0, 0);
    while start < n {
        if !changed[start] {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < n && changed[end] {
            end += 1;
        }
        let mut up = 0;
        while start - up > floor && keys[start - up - 1] == keys[end - up - 1] {
            up += 1;
        }
        let mut down = 0;
        while end + down < n && !changed[end + down] && keys[start + down] == keys[end + down] {
            down += 1;
        }
        let cost = (end - start) * (up + down + 1);
        let mut shift = 0isize;
        if up + down > 0 && cost <= budget {
            budget -= cost;
            shift = (-(up as isize)..=down as isize)
                .max_by_key(|&d| {
                    let (s, e) = (start.wrapping_add_signed(d), end.wrapping_add_signed(d));
                    (balanced(s..e), starts_line(s), ends_line(e - 1), d)
                })
                .unwrap_or(0);
        }
        if shift != 0 {
            changed[start..end].fill(false);
            let (s, e) = (
                start.wrapping_add_signed(shift),
                end.wrapping_add_signed(shift),
            );
            changed[s..e].fill(true);
            end = e;
        }
        floor = end;
        start = end;
    }
}

/// One mark per line of `range`, from the changed flags of its tokens.
fn marks(
    lines: &[String],
    tokens: &[Token],
    changed: &[bool],
    range: &Range<usize>,
    any: bool,
) -> Vec<Mark> {
    // A line without tokens (blank) goes with its block.
    let mut out = vec![if any { Mark::Whole } else { Mark::Same }; range.len()];
    let mut i = 0;
    while i < tokens.len() {
        let line = tokens[i].line;
        let mut j = i;
        while j < tokens.len() && tokens[j].line == line {
            j += 1;
        }
        let on_line = &tokens[i..j];
        let flags = &changed[i..j];
        let real = on_line.iter().filter(|t| t.real()).count();
        let real_changed = on_line
            .iter()
            .zip(flags)
            .filter(|(t, &c)| c && t.real())
            .count();
        let mark = if !flags.contains(&true) {
            Mark::Same
        } else if real_changed == real {
            Mark::Whole
        } else {
            let mut bytes: Vec<(u32, u32)> = Vec::new();
            for (token, _) in on_line.iter().zip(flags).filter(|(_, &c)| c) {
                if token.visible() {
                    bytes.push((token.start, token.end));
                } else {
                    // A break or an empty indentation has nothing to mark: the token
                    // next to it stands in (the last of the line, or the first).
                    let mut reals = on_line.iter().filter(|t| t.real());
                    let stand_in = if token.class & CLASS == INDENT {
                        reals.next()
                    } else {
                        reals.next_back()
                    };
                    if let Some(t) = stand_in {
                        bytes.push((t.start, t.end));
                    }
                }
            }
            bytes.sort_unstable();
            let text = lines[line as usize].as_str();
            let mut spans = Spans::new();
            let (mut at, mut units) = (0usize, 0usize);
            for (start, end) in bytes {
                let (start, end) = (start as usize, end as usize);
                if start < at {
                    continue; // the same stand-in twice
                }
                units += text[at..start].encode_utf16().count();
                let width = text[start..end].encode_utf16().count();
                push_span(&mut spans, units, units + width);
                units += width;
                at = end;
            }
            Mark::Part(join_gaps(spans, text))
        };
        out[line as usize - range.start] = mark;
        i = j;
    }
    out
}

/// One side of a comparison: its lines and the tokens kept for its changed lines.
struct Side<'a> {
    lines: &'a [String],
    tokens: &'a [Token],
}

/// Compares the tokens of one changed block: a mark for each of its lines on both
/// sides, and whether any token changed at all. `None`: too large to compare.
fn compare_block(
    a: &Side,
    a_lines: Range<usize>,
    b: &Side,
    b_lines: Range<usize>,
    ignore_case: bool,
) -> Option<Block> {
    let (ta, tb) = (tokens_of(a.tokens, &a_lines), tokens_of(b.tokens, &b_lines));
    if ta.len() + tb.len() > MAX_TOKENS {
        return None;
    }
    let mut interner = Interner::new(ta.len() + tb.len());
    let before: Vec<_> = ta
        .iter()
        .map(|t| interner.intern(key(t, a.lines, ignore_case)))
        .collect();
    let after: Vec<_> = tb
        .iter()
        .map(|t| interner.intern(key(t, b.lines, ignore_case)))
        .collect();
    let input = InternedInput {
        before,
        after,
        interner,
    };
    let diff = Diff::compute(Algorithm::Histogram, &input);
    let mut removed: Vec<bool> = (0..ta.len()).map(|i| diff.is_removed(i as u32)).collect();
    let mut added: Vec<bool> = (0..tb.len()).map(|i| diff.is_added(i as u32)).collect();
    settle(&input.before, &mut removed, ta, a.lines);
    settle(&input.after, &mut added, tb, b.lines);
    let changed = removed.contains(&true) || added.contains(&true);
    Some(Block {
        a: marks(a.lines, ta, &removed, &a_lines, changed),
        b: marks(b.lines, tb, &added, &b_lines, changed),
        changed,
    })
}

/// The marks of one changed block of lines.
#[derive(Debug)]
pub(crate) struct Block {
    pub a: Vec<Mark>,
    pub b: Vec<Mark>,
    /// False: the two sides have the same tokens, only the layout differs.
    pub changed: bool,
}

/// Token comparison of the changed blocks `(a0, a1, b0, b1)` of a line diff, one
/// result per block (`None`: too large, left to the caller).
pub(crate) fn compare_hunks(
    left: &[String],
    right: &[String],
    hunks: &[(usize, usize, usize, usize)],
    syntax: Syntax,
    ignore_space: bool,
    ignore_case: bool,
) -> Vec<Option<Block>> {
    // Shell escapes/heredocs, Lua long brackets, C-family raw strings/text blocks
    // and continued macros require more grammar context. Preserve the whole file
    // literally when these markers occur, including unchanged lines above a hunk.
    let literal = left.iter().chain(right).any(|line| {
        (syntax == SHELL && (line.contains("<<") || line.contains('\\')))
            || (syntax == LUA
                && line
                    .match_indices('[')
                    .any(|(at, _)| line[at + 1..].trim_start_matches('=').starts_with('[')))
            || ((syntax == C || syntax == C_LIKE)
                && (line.contains("R\"") || line.contains("@\"") || line.contains("\"\"\"")))
            || (syntax.preprocessor
                && line.trim_start().starts_with('#')
                && line.trim_end().ends_with('\\'))
    });
    let syntax = if literal { PLAIN } else { syntax };
    let a_ranges: Vec<Range<usize>> = hunks.iter().map(|h| h.0..h.1).collect();
    let b_ranges: Vec<Range<usize>> = hunks.iter().map(|h| h.2..h.3).collect();
    let (ta, tb) = (
        lex(left, syntax, ignore_space, &a_ranges),
        lex(right, syntax, ignore_space, &b_ranges),
    );
    let (a, b) = (
        Side {
            lines: left,
            tokens: &ta,
        },
        Side {
            lines: right,
            tokens: &tb,
        },
    );
    a_ranges
        .into_iter()
        .zip(b_ranges)
        .map(|(ar, br)| compare_block(&a, ar, &b, br, ignore_case))
        .collect()
}

/// Largest patch that is compared by tokens.
const MAX_PATCH_BYTES: usize = 4 * 1024 * 1024;

/// Token marks for the lines of a unified patch (as `git diff` prints it), one entry
/// per line of the patch: `None` for a line that is no change or changed as a whole,
/// an empty list for a removed or added line whose content did not change (layout
/// only), else the changed ranges in UTF-16 offsets of the patch line, its `+` or `-`
/// included. `name` picks the language when the patch does not name its file.
///
/// A patch shows a few lines around each change, not the file: a hunk that starts
/// inside a block comment or a string spanning lines is read as code.
pub fn patch_marks(name: &str, patch: &str) -> Vec<Option<Spans>> {
    let lines: Vec<&str> = patch.split_terminator('\n').collect();
    let mut out: Vec<Option<Spans>> = vec![None; lines.len()];
    if patch.len() > MAX_PATCH_BYTES {
        return out;
    }
    let mut syntax = Syntax::for_names(&[name]);
    let count = |field: Option<&str>| -> usize {
        field
            .and_then(|f| f.get(1..))
            .map(|f| f.split_once(',').map_or("1", |(_, n)| n))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if let Some(path) = line.strip_prefix("+++ ") {
            // `+++ b/dir/file.rs`, possibly followed by a tab and a time.
            let path = path.split('\t').next().unwrap_or(path).trim_end();
            if path != "/dev/null" {
                syntax = Syntax::for_names(&[path, name]);
            }
            continue;
        }
        if !line.starts_with("@@ ") {
            continue;
        }
        let mut fields = line.split_whitespace().skip(1);
        let (mut old_left, mut new_left) = (count(fields.next()), count(fields.next()));
        // The hunk as two files: the old lines and the new lines, with the patch line
        // each came from, and the runs of changed lines between unchanged ones.
        let (mut old, mut new): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        let (mut old_at, mut new_at): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
        let mut runs: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut run: Option<(usize, usize)> = None;
        while i < lines.len() && (old_left > 0 || new_left > 0) {
            let line = lines[i];
            let text = || line[1..].trim_end_matches('\r').to_string();
            match line.as_bytes().first() {
                Some(b'-') if old_left > 0 => {
                    run.get_or_insert((old.len(), new.len()));
                    old.push(text());
                    old_at.push(i);
                    old_left -= 1;
                }
                Some(b'+') if new_left > 0 => {
                    run.get_or_insert((old.len(), new.len()));
                    new.push(text());
                    new_at.push(i);
                    new_left -= 1;
                }
                Some(b' ') if old_left > 0 && new_left > 0 => {
                    if let Some((a, b)) = run.take() {
                        runs.push((a, old.len(), b, new.len()));
                    }
                    old.push(text());
                    new.push(text());
                    old_at.push(i);
                    new_at.push(i);
                    old_left -= 1;
                    new_left -= 1;
                }
                Some(b'\\') => {}
                _ => break,
            }
            i += 1;
        }
        if let Some((a, b)) = run.take() {
            runs.push((a, old.len(), b, new.len()));
        }
        let blocks = compare_hunks(&old, &new, &runs, syntax, false, false);
        for (&(a0, _, b0, _), block) in runs.iter().zip(blocks) {
            let Some(block) = block else { continue };
            let sides = [(block.a, &old_at, a0), (block.b, &new_at, b0)];
            for (marks, at, first) in sides {
                for (k, mark) in marks.into_iter().enumerate() {
                    out[at[first + k]] = match mark {
                        Mark::Whole => None,
                        Mark::Same => Some(Vec::new()),
                        // Past the `+` or `-` in front of the line.
                        Mark::Part(spans) => {
                            Some(spans.into_iter().map(|(s, e)| (s + 1, e + 1)).collect())
                        }
                    };
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    /// Compares two texts as one changed block; the marked text of each line.
    fn block(name: &str, a: &str, b: &str) -> (bool, Vec<String>, Vec<String>) {
        let (la, lb) = (lines(a), lines(b));
        let hunks = [(0, la.len(), 0, lb.len())];
        let syntax = Syntax::for_names(&[name]);
        let block = compare_hunks(&la, &lb, &hunks, syntax, false, false)
            .remove(0)
            .unwrap();
        let show = |marks: &[Mark], lines: &[String]| -> Vec<String> {
            marks
                .iter()
                .zip(lines)
                .map(|(mark, line)| match mark {
                    Mark::Whole => "*".to_string(),
                    Mark::Same => String::new(),
                    Mark::Part(spans) => {
                        let units: Vec<u16> = line.encode_utf16().collect();
                        spans
                            .iter()
                            .map(|&(s, e)| String::from_utf16_lossy(&units[s..e]))
                            .collect::<Vec<_>>()
                            .join("|")
                    }
                })
                .collect()
        };
        (block.changed, show(&block.a, &la), show(&block.b, &lb))
    }

    fn same(name: &str, a: &str, b: &str) -> bool {
        !block(name, a, b).0
    }

    #[test]
    fn reflowed_code_is_layout_only() {
        assert!(same(
            "a.rs",
            "let x = call(first, second);\n",
            "let x = call(\n    first,\n    second\n);\n"
        ));
        assert!(same("a.c", "if (a) { b(); }\n", "if (a)\n{\n\tb();\n}\n"));
        assert!(same(
            "a.json",
            "{\"a\": [1, 2]}",
            "{\n  \"a\": [\n    1,\n    2\n  ]\n}"
        ));
        // A blank line and trailing spaces are layout too.
        assert!(same("a.rs", "a();\nb();\n", "a();  \n\nb();\n"));
    }

    #[test]
    fn wrapping_code_marks_only_the_wrapper() {
        let (changed, a, b) = block(
            "a.rs",
            "foo();\nbar();\n",
            "if ready {\n    foo();\n    bar();\n}\n",
        );
        assert!(changed);
        assert_eq!(a, ["", ""]);
        assert_eq!(b, ["*", "", "", "*"]);
    }

    #[test]
    fn a_changed_token_is_marked_alone() {
        let (changed, a, b) = block(
            "a.rs",
            "let total = add(left, right);\n",
            "let total = add(\n    left,\n    other,\n);\n",
        );
        assert!(changed);
        assert_eq!(a, ["right"]);
        // The comma after the last argument is new as well.
        assert_eq!(b, ["", "", "*", ""]);
    }

    #[test]
    fn whitespace_inside_a_string_is_content() {
        assert!(!same("a.rs", "let s = \"a b\";", "let s = \"a  b\";"));
        assert!(!same("a.py", "s = 'a b'", "s = 'a  b'"));
        // The same spaces outside the string are layout.
        assert!(same("a.rs", "let s = \"a b\";", "let  s  =  \"a b\" ;"));
        // A string over several lines keeps its line breaks and indentation.
        assert!(!same("a.rs", "let s = \"a\n  b\";", "let s = \"a\n b\";"));
        assert!(!same(
            "a.rs",
            "let s = r#\"a \" b\"#;",
            "let s = r#\"a \"  b\"#;"
        ));
    }

    #[test]
    fn significant_whitespace_is_visible_in_comparison_and_git_patches() {
        use super::super::diff::{diff_text_as, DiffOptions};

        let cases = [
            ("a.sh", r"printf '<%s>\n' a\ b", r"printf '<%s>\n' a\  b"),
            ("a.sh", "printf x\\\n y", "printf x\\\n  y"),
            ("a.lua", "return [[a b]]", "return [[a  b]]"),
            ("a.lua", "return [=[a b]=]", "return [=[a  b]=]"),
            ("a.lua", "return [===[a b]===]", "return [===[a  b]===]"),
            ("a.lua", "return [=[\na b\n]=]", "return [=[\na  b\n]=]"),
            (
                "a.properties",
                "message=hello world",
                "message=hello  world",
            ),
            ("a.ini", "message=hello world", "message=hello  world"),
            ("a.desktop", "Name=hello world", "Name=hello  world"),
            ("a.properties", "message=hello ", "message=hello  "),
            (
                "a.properties",
                "message=hello # world",
                "message=hello #  world",
            ),
            (
                "a.properties",
                "message=hello 'world",
                "message=hello  'world",
            ),
        ];
        for (name, before, after) in cases {
            let syntax = Syntax::for_names(&[name]);
            let result = diff_text_as(
                before,
                after,
                DiffOptions {
                    structural: true,
                    ..DiffOptions::default()
                },
                syntax,
            );
            assert!(
                result
                    .ops
                    .iter()
                    .any(|op| op.tag != "equal" && !op.cosmetic),
                "{name}: {before:?}"
            );
            assert!(!same(name, before, after), "{name}: {before:?}");
            let a = lines(before);
            let b = lines(after);
            let patch = format!(
                "@@ -1,{} +1,{} @@\n{}{}",
                a.len(),
                b.len(),
                a.iter().map(|s| format!("-{s}\n")).collect::<String>(),
                b.iter().map(|s| format!("+{s}\n")).collect::<String>(),
            );
            assert!(
                patch_marks(name, &patch)[1..]
                    .iter()
                    .any(|mark| mark.as_ref().is_none_or(|spans| !spans.is_empty())),
                "{name}: {patch}"
            );
        }
        // Ordinary Lua code still supports formatting-only comparison.
        assert!(same("a.lua", "return f(a, b)", "return f( a,  b )"));
    }

    #[test]
    fn code_turned_into_a_comment_is_a_change() {
        assert!(!same("a.rs", "// note\ncall();\n", "// note call();\n"));
        assert!(!same("a.c", "a(); /* b(); */", "a(); b();"));
        // Reflowing a block comment is layout.
        assert!(same(
            "a.c",
            "/* one two\n   three */",
            "/* one\n   two three */"
        ));
        // A comment marker inside a string is no comment.
        assert!(!same("a.js", "x = \"// a  b\";", "x = \"// a b\";"));
    }

    #[test]
    fn line_breaks_count_where_the_language_reads_them() {
        // Python: a break ends a statement, indentation nests it.
        assert!(!same("a.py", "a = 1\nb = 2\n", "a = 1; b = 2\n"));
        assert!(!same(
            "a.py",
            "if a:\n    b()\nc()\n",
            "if a:\n    b()\n    c()\n"
        ));
        assert!(same("a.py", "f(a, b)\n", "f(\n    a,\n    b\n)\n"));
        assert!(same("a.py", "x = {'a': 1}\n", "x = {\n    'a': 1\n}\n"));
        assert!(same("a.py", "a  =  1  # note\n", "a = 1 # note\n"));
        // JavaScript: a break between two statements counts, one inside a list does not.
        assert!(!same("a.js", "a()\nb()\n", "a() b()\n"));
        assert!(same(
            "a.ts",
            "import { a, b } from \"x\";\n",
            "import {\n  a,\n  b\n} from \"x\";\n"
        ));
        assert!(same("a.js", "f(a).then(b);\n", "f(a)\n  .then(b);\n"));
        assert!(same("a.go", "if a {\n\tb()\n}\n", "if a {\n\n\tb()\n\n}\n"));
        // C: only a preprocessor line ends at its break.
        assert!(!same("a.c", "#define A 1\nb();\n", "#define A 1 b();\n"));
        // YAML: indentation is structure.
        assert!(!same("a.yml", "a:\n  b: 1\n", "a:\nb: 1\n"));
        assert!(!same("a.yml", "a:   1\n", "a: 1\n"));
    }

    #[test]
    fn touching_tokens_count_where_words_are_split_by_spaces() {
        assert!(!same(
            "a.sh",
            "printf '%s' \"a\" \"b\"\n",
            "printf '%s' \"a\"\"b\"\n"
        ));
        assert!(!same("a.sh", "echo word\"tail\"\n", "echo word \"tail\"\n"));
        assert!(!same("a.sh", "run a -b\n", "run a-b\n"));
        assert!(same("a.sh", "run   a   -b\n", "run a -b\n"));
        assert!(!same(
            "a.css",
            "a { margin: 0 -1px; }",
            "a { margin: 0 - 1px; }"
        ));
        // Elsewhere spaces around an operator are layout, but operators stay whole.
        assert!(same("a.rs", "a=b+c;", "a = b + c;"));
        assert!(!same("a.c", "x = a - -b;", "x = a --b;"));
    }

    #[test]
    fn plain_text_counts_every_space() {
        assert!(!same("notes.txt", "a b\n", "a  b\n"));
        assert!(!same("data", "a\nb\n", "a\n\nb\n"));
        assert!(!same("notes.txt", "a\n", "a \n"));
        // Prose and scalar formats retain whitespace conservatively.
        assert!(!same("a.md", "| a   | b |\n", "| a | b |\n"));
        assert!(!same("a.md", "one\ntwo\n", "one two\n"));
    }

    #[test]
    fn ambiguous_whitespace_remains_content() {
        assert!(!same(
            "a.yml",
            "message: |\n  # hello world\n",
            "message: |\n  # hello  world\n"
        ));
        assert!(!same(
            "a.cpp",
            "auto s = R\"(a \" b c)\";",
            "auto s = R\"(a \" b  c)\";"
        ));
        assert!(!same(
            "a.java",
            "String s = \"\"\"\na b\n\"\"\";",
            "String s = \"\"\"\na  b\n\"\"\";"
        ));
        assert!(!same(
            "a.sh",
            "cat <<EOF\nhello world\nEOF\n",
            "cat <<EOF\nhello  world\nEOF\n"
        ));
        assert!(!same("a.js", "const r = /a b/;\n", "const r = /a  b/;\n"));
        assert!(!same("a.ts", "const r=/a b/;\n", "const r=/a  b/;\n"));
        assert!(!same(
            "a.yml",
            "message: hello world\n",
            "message: hello  world\n"
        ));
        assert!(!same("a.md", "one\ntwo\n", "one  \ntwo\n"));
        assert!(!same("a.html", "<p>hello world</p>", "<p>hello  world</p>"));
        assert!(!same(
            "a.tsx",
            "const v = <p>a b</p>;",
            "const v = <p>a  b</p>;"
        ));
        assert!(!same("a.c", "#define F(x) x\n", "#define F (x) x\n"));
    }

    #[test]
    fn state_carries_from_the_lines_above_a_change() {
        // Line 2 is inside a comment on the left and inside a string on the right.
        let comment = lines("/* start\n a  b\n*/");
        let comment2 = lines("/* start\n a b\n*/");
        let hunks = [(1, 2, 1, 2)];
        let inside = |a: &[String], b: &[String], name: &str| {
            compare_hunks(a, b, &hunks, Syntax::for_names(&[name]), false, false)
                .remove(0)
                .unwrap()
                .changed
        };
        assert!(!inside(&comment, &comment2, "a.c"));
        let string = lines("let s = \"start\n a  b\n\";");
        let string2 = lines("let s = \"start\n a b\n\";");
        assert!(inside(&string, &string2, "a.rs"));
    }

    #[test]
    fn a_new_block_is_marked_with_its_own_brackets() {
        let (changed, a, b) = block(
            "a.rs",
            "fn a() {\n    one();\n}\n",
            "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n",
        );
        assert!(changed);
        assert_eq!(a, ["", "", ""]);
        // Not "}\n\nfn b() {\n two();" with the old closing bracket taken as new.
        assert_eq!(b, ["", "", "", "*", "*", "*", "*"]);
    }

    #[test]
    fn spans_are_utf16_offsets_and_options_apply() {
        let (_, a, b) = block("a.rs", "let š = \"😀\" + old;", "let š = \"😀\" + new;");
        assert_eq!((a, b), (vec!["old".to_string()], vec!["new".to_string()]));
        let (la, lb) = (lines("Select A"), lines("select  a"));
        let hunks = [(0, 1, 0, 1)];
        let run = |space, case| {
            compare_hunks(&la, &lb, &hunks, PLAIN, space, case)
                .remove(0)
                .unwrap()
                .changed
        };
        assert!(run(false, false));
        assert!(run(true, false));
        assert!(run(false, true));
        assert!(!run(true, true));
    }

    #[test]
    fn a_patch_gets_marks_for_its_changed_lines() {
        let patch = "diff --git a/src/a.rs b/src/a.rs\n\
index 1..2 100644\n\
--- a/src/a.rs\n\
+++ b/src/a.rs\n\
@@ -1,4 +1,6 @@\n fn main() {\n\
-    let x = call(one, two);\n\
+    let x = call(\n\
+        one,\n\
+        three);\n\
\x20\n\
-    done();\n\
+    done( );\n\
@@ -20,2 +22,3 @@ fn other() {\n context\n\
+added();\n\
 more\n";
        let marks = patch_marks("", patch);
        let at = |text: &str| {
            let n = patch.lines().position(|l| l == text).unwrap();
            marks[n].clone()
        };
        assert_eq!(marks.len(), patch.lines().count());
        // Headers and unchanged lines carry nothing.
        assert_eq!(at("--- a/src/a.rs"), None);
        assert_eq!(at(" fn main() {"), None);
        // `two` became `three`: offsets count the leading `-` or `+`.
        assert_eq!(at("-    let x = call(one, two);"), Some(vec![(23, 26)]));
        assert_eq!(at("+    let x = call("), Some(vec![]));
        assert_eq!(at("+        one,"), Some(vec![]));
        assert_eq!(at("+        three);"), Some(vec![(9, 14)]));
        // Layout only.
        assert_eq!(at("-    done();"), Some(vec![]));
        assert_eq!(at("+    done( );"), Some(vec![]));
        // A line that exists on one side only is changed as a whole.
        assert_eq!(at("+added();"), None);
    }

    #[test]
    fn a_patch_of_another_shape_is_left_alone() {
        assert!(patch_marks("a.rs", "Binary file (preview unavailable)")
            .iter()
            .all(Option::is_none));
        // A hunk cut short (a truncated preview) does not read past its lines.
        let cut = "@@ -1,3 +1,3 @@\n a\n-b\n+b \n\n[Diff preview truncated at 1 MiB]\n";
        let marks = patch_marks("a.rs", cut);
        assert_eq!(marks[2], Some(vec![]));
        assert_eq!(marks[3], Some(vec![]));
        assert_eq!(marks[5], None);
        // CRLF files keep their `\r` in the patch.
        let crlf = "@@ -1 +1 @@\n-a(b);\r\n+a( b );\r\n";
        assert_eq!(
            patch_marks("a.rs", crlf),
            [None, Some(vec![]), Some(vec![])]
        );
    }
}
