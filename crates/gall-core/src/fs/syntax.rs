//! Syntax colouring for the compare view. Grammars are Sublime Text's (via
//! syntect, with bat's extra languages from two-face). Instead of a colour
//! theme each run gets a small category (comment, string, keyword, …) that the
//! UI colours from the app's light or dark theme.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use syntect::parsing::{
    ParseState, Scope, ScopeStack, SyntaxDefinition, SyntaxReference, SyntaxSet,
};

/// Larger texts are not coloured (the diff itself stays fast).
const MAX_BYTES: usize = 4 * 1024 * 1024;
/// Lines longer than this (minified code) are not parsed; colouring continues after them.
const MAX_LINE: usize = 4000;
/// Time budget (it runs in the background, after the diff is shown); lines
/// after it stay plain. About 5000 dense lines per second in release builds.
const BUDGET: Duration = Duration::from_secs(5);

/// Run categories, as sent to the UI (0 = plain, never sent).
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cat {
    Plain = 0,
    Comment = 1,
    Str = 2,
    Number = 3,
    Constant = 4,
    Keyword = 5,
    Operator = 6,
    Function = 7,
    Type = 8,
    Variable = 9,
    Tag = 10,
    Attr = 11,
    Heading = 12,
}

/// Coloured runs of one line: `(start, end, category)`, UTF-16 offsets.
pub type LineRuns = Vec<(u32, u32, u8)>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Highlight {
    /// Language name, e.g. "Bourne Again Shell (bash)".
    pub language: String,
    pub lines: Vec<LineRuns>,
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        let base = two_face::syntax::extra_newlines();
        // Grammars two-face leaves out of the fancy-regex set (they need Oniguruma).
        let mut b = base.into_builder();
        match SyntaxDefinition::load_from_str(include_str!("powershell.sublime-syntax"), true, None)
        {
            Ok(def) => b.add(def),
            Err(e) => log::warn!("powershell grammar: {e}"),
        }
        b.build()
    })
}

/// The grammar for a file name (extension, or the whole name such as
/// "Makefile"), else from the first line (shebang, `<?xml`, modelines).
fn find<'a>(ss: &'a SyntaxSet, name: &str, first_line: &str) -> Option<&'a SyntaxReference> {
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e);
    ext.and_then(|e| ss.find_syntax_by_extension(e))
        .or_else(|| ss.find_syntax_by_extension(&lower))
        .or_else(|| ss.find_syntax_by_extension(name))
        .or_else(|| ss.find_syntax_by_first_line(first_line))
        .filter(|s| s.name != "Plain Text")
}

/// A grammar exists for this file name (extension or whole name): the preview shows
/// such files as source text even when they are not in its own list.
pub fn knows(name: &str) -> bool {
    find(syntaxes(), name, "").is_some()
}

fn classify_name(s: &str) -> Cat {
    let has = |p: &str| s == p || s.starts_with(&format!("{p}."));
    if has("comment") || has("punctuation.definition.comment") {
        Cat::Comment
    } else if has("string")
        || has("constant.character.escape")
        || has("punctuation.definition.string")
    {
        Cat::Str
    } else if has("constant.numeric") {
        Cat::Number
    } else if has("constant.language")
        || has("support.constant")
        || has("constant.other")
        || has("constant.character")
    {
        Cat::Constant
    } else if has("keyword.operator") {
        Cat::Operator
    } else if has("keyword") || has("storage") {
        Cat::Keyword
    } else if has("entity.name.function") || has("support.function") || has("variable.function") {
        Cat::Function
    } else if has("entity.name.type")
        || has("entity.name.class")
        || has("support.type")
        || has("support.class")
        || has("entity.other.inherited-class")
    {
        Cat::Type
    } else if has("variable.parameter")
        || has("variable.other")
        || has("punctuation.definition.variable")
        || has("variable.language")
    {
        Cat::Variable
    } else if has("entity.name.tag") {
        Cat::Tag
    } else if has("entity.other.attribute-name")
        || has("support.type.property-name")
        || has("meta.mapping.key")
    {
        Cat::Attr
    } else if has("markup.heading") || has("entity.name.section") {
        Cat::Heading
    } else {
        Cat::Plain
    }
}

/// Category of the innermost scope that has one.
fn classify(stack: &ScopeStack, cache: &mut HashMap<Scope, Cat>) -> Cat {
    for sc in stack.as_slice().iter().rev() {
        let c = *cache
            .entry(*sc)
            .or_insert_with(|| classify_name(&sc.build_string()));
        if c != Cat::Plain {
            return c;
        }
    }
    Cat::Plain
}

/// Colours `lines` (without line endings) as the language of file `name`.
/// None when the language is unknown or the text too large.
pub fn highlight(name: &str, lines: &[String]) -> Option<Highlight> {
    let bytes: usize = lines.iter().map(|l| l.len() + 1).sum();
    if bytes > MAX_BYTES {
        return None;
    }
    let ss = syntaxes();
    let syntax = find(ss, name, lines.first().map_or("", |l| l.as_str()))?;
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut cache = HashMap::new();
    let start = Instant::now();
    let mut out = Vec::with_capacity(lines.len());
    let mut buf = String::new();
    for line in lines {
        if line.len() > MAX_LINE || start.elapsed() > BUDGET {
            out.push(Vec::new());
            continue;
        }
        buf.clear();
        buf.push_str(line);
        buf.push('\n');
        let Ok(ops) = state.parse_line(&buf, ss) else {
            out.push(Vec::new());
            continue;
        };
        let mut runs: LineRuns = Vec::new();
        let (mut pos, mut u16pos) = (0usize, 0u32);
        let mut emit = |from: usize, to: usize, u16pos: &mut u32, cat: Cat| {
            let to = to.min(line.len());
            if to <= from {
                return;
            }
            let w = line[from..to].encode_utf16().count() as u32;
            if cat != Cat::Plain {
                match runs.last_mut() {
                    Some(r) if r.1 == *u16pos && r.2 == cat as u8 => r.1 += w,
                    _ => runs.push((*u16pos, *u16pos + w, cat as u8)),
                }
            }
            *u16pos += w;
        };
        for (at, op) in ops {
            if at > pos {
                emit(pos, at, &mut u16pos, classify(&stack, &mut cache));
                pos = at.min(line.len()).max(pos);
            }
            let _ = stack.apply(&op);
        }
        emit(pos, line.len(), &mut u16pos, classify(&stack, &mut cache));
        out.push(runs);
    }
    Some(Highlight {
        language: syntax.name.clone(),
        lines: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    fn cats(h: &Highlight, line: usize, text: &str) -> Vec<(String, u8)> {
        let t: Vec<u16> = text.encode_utf16().collect();
        h.lines[line]
            .iter()
            .map(|&(s, e, c)| (String::from_utf16_lossy(&t[s as usize..e as usize]), c))
            .collect()
    }

    #[test]
    fn shell_script_by_extension_and_shebang() {
        let src = "#!/bin/bash\n# say hi\necho \"hi $USER\"\n";
        let h = highlight("run.sh", &lines(src)).unwrap();
        assert!(
            h.language.to_lowercase().contains("bash") || h.language.contains("Shell"),
            "{}",
            h.language
        );
        let c1 = cats(&h, 1, "# say hi");
        assert!(c1.iter().all(|(_, c)| *c == Cat::Comment as u8), "{c1:?}");
        let c2 = cats(&h, 2, "echo \"hi $USER\"");
        assert!(c2.iter().any(|(_, c)| *c == Cat::Str as u8), "{c2:?}");
        // no extension: found from the shebang
        assert!(highlight("run", &lines(src)).is_some());
    }

    #[test]
    fn rust_keywords_and_utf16_offsets() {
        let src = "fn čas() -> u32 { 42 }";
        let h = highlight("a.rs", &lines(src)).unwrap();
        let c = cats(&h, 0, src);
        assert!(c.contains(&("fn".into(), Cat::Keyword as u8)), "{c:?}");
        assert!(c.contains(&("42".into(), Cat::Number as u8)), "{c:?}");
        assert!(c.iter().any(|(t, _)| t == "čas"), "{c:?}");
    }

    #[test]
    fn scripts_are_coloured() {
        for (name, src) in [
            ("a.ps1", "# hi\n$x = \"a\"\nWrite-Host $x"),
            ("a.psm1", "function Get-X { return 1 }"),
            ("a.sh", "# hi\necho \"a\""),
            ("a.bat", "@echo off\nREM hi\nset X=1"),
            ("a.cmd", "@echo off\nREM hi"),
            ("a.zsh", "echo hi"),
            ("a.fish", "echo hi"),
            ("a.py", "# hi\nprint('a')"),
        ] {
            let h = highlight(name, &lines(src));
            assert!(h.is_some(), "{name}: no grammar");
            assert!(
                h.unwrap().lines.iter().any(|l| !l.is_empty()),
                "{name}: nothing coloured"
            );
        }
        let src = "# note\n$name = \"hi $env:USER\"\nGet-Item -Path $name";
        let h = highlight("a.ps1", &lines(src)).unwrap();
        assert_eq!(h.language, "PowerShell");
        assert!(cats(&h, 0, "# note")
            .iter()
            .all(|(_, c)| *c == Cat::Comment as u8));
        let l1 = cats(&h, 1, "$name = \"hi $env:USER\"");
        assert!(
            l1.contains(&("$name".into(), Cat::Variable as u8)),
            "{l1:?}"
        );
        assert!(l1.iter().any(|(_, c)| *c == Cat::Str as u8), "{l1:?}");
        let l2 = cats(&h, 2, "Get-Item -Path $name");
        assert!(
            l2.contains(&("Get-Item".into(), Cat::Function as u8)),
            "{l2:?}"
        );
        assert!(knows("x.php") && knows("x.swift") && knows("Makefile") && knows("x.psm1"));
        assert!(!knows("x.unknownext"));
    }

    #[test]
    fn typescript_and_unknown() {
        assert!(highlight("x.ts", &lines("const a: number = 1;")).is_some());
        assert!(highlight("notes.unknownext", &lines("just text")).is_none());
    }
}
