//! Regular-expression rules that make a text comparison ignore parts of a line: a line rule
//! drops every line it matches, a text rule drops the matched text inside a line. The rules are
//! compiled once per comparison with fixed limits (count, length, compiled size), and the engine
//! behind them runs in time linear in the line, so a rule cannot stall a comparison.
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};

/// More rules than anyone writes by hand; the rest is refused, not cut off.
pub const MAX_RULES: usize = 16;
/// Characters of one pattern.
pub const MAX_PATTERN: usize = 512;
/// Bytes of the compiled program and of the lazy automaton cache of one rule.
const SIZE_LIMIT: usize = 1 << 20;

/// The compiled rules of one comparison.
#[derive(Default)]
pub struct Rules {
    lines: Vec<RegexMatcher>,
    text: Vec<RegexMatcher>,
}

fn compile_all(kind: &str, patterns: &[String]) -> Result<Vec<RegexMatcher>, String> {
    if patterns.len() > MAX_RULES {
        return Err(format!(
            "At most {MAX_RULES} {kind} ignore rules are allowed"
        ));
    }
    patterns
        .iter()
        .filter(|pattern| !pattern.is_empty())
        .map(|pattern| {
            if pattern.chars().count() > MAX_PATTERN {
                return Err(format!(
                    "An ignore rule is longer than {MAX_PATTERN} characters"
                ));
            }
            RegexMatcherBuilder::new()
                .size_limit(SIZE_LIMIT)
                .dfa_size_limit(SIZE_LIMIT)
                .build(pattern)
                .map_err(|e| format!("Invalid ignore rule {pattern:?}: {e}"))
        })
        .collect()
}

impl Rules {
    /// Fails, naming the rule, for a pattern that is too long, too large or not valid.
    pub fn compile(lines: &[String], text: &[String]) -> Result<Rules, String> {
        Ok(Rules {
            lines: compile_all("line", lines)?,
            text: compile_all("text", text)?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.text.is_empty()
    }

    /// Whether a line rule matches somewhere in the line.
    pub fn ignores_line(&self, line: &str) -> bool {
        self.lines
            .iter()
            .any(|rule| rule.is_match(line.as_bytes()).unwrap_or(false))
    }

    /// The line without the text that the text rules match. An empty match removes nothing.
    pub fn strip(&self, line: &str) -> String {
        let mut text = line.to_string();
        for rule in &self.text {
            let mut ranges = Vec::new();
            let found = rule.find_iter(text.as_bytes(), |m| {
                if m.start() < m.end() {
                    ranges.push((m.start(), m.end()));
                }
                true
            });
            if found.is_err() || ranges.is_empty() {
                continue;
            }
            let mut kept = String::with_capacity(text.len());
            let mut at = 0;
            for (start, end) in ranges {
                // Matches of one rule never overlap and lie on character boundaries.
                if start >= at {
                    kept.push_str(&text[at..start]);
                    at = end;
                }
            }
            kept.push_str(&text[at..]);
            text = kept;
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(lines: &[&str], text: &[&str]) -> Result<Rules, String> {
        let own = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        Rules::compile(&own(lines), &own(text))
    }

    #[test]
    fn text_rules_remove_matches_and_keep_the_rest() {
        let r = rules(&[], &[r"\d{4}-\d\d-\d\d", "#.*$"]).unwrap();
        assert_eq!(r.strip("built 2026-11-24 ok # note"), "built  ok ");
        assert_eq!(r.strip("no match"), "no match");
        // Non-ASCII text stays whole around a match.
        assert_eq!(r.strip("čas 2026-11-24 žar"), "čas  žar");
    }

    #[test]
    fn an_empty_match_removes_nothing() {
        let r = rules(&[], &["x*"]).unwrap();
        assert_eq!(r.strip("abc"), "abc");
        assert_eq!(r.strip("axxb"), "ab");
    }

    #[test]
    fn line_rules_match_anywhere_in_the_line() {
        let r = rules(&[r"^\s*//"], &[]).unwrap();
        assert!(r.ignores_line("   // comment"));
        assert!(!r.ignores_line("code(); // trailing"));
        assert!(!Rules::default().ignores_line("anything"));
        assert!(Rules::default().is_empty());
    }

    #[test]
    fn invalid_long_large_and_too_many_rules_are_refused_with_a_reason() {
        let error = |r: Result<Rules, String>| r.err().unwrap();
        assert!(error(rules(&["("], &[])).contains("Invalid ignore rule"));
        assert!(error(rules(&[], &[&"a".repeat(MAX_PATTERN + 1)])).contains("longer than"));
        assert!(error(rules(&[], &["a{1000}{1000}{1000}"])).contains("Invalid ignore rule"));
        let many = vec!["a"; MAX_RULES + 1];
        assert!(error(rules(&many, &[])).contains("At most"));
        // Empty patterns are skipped.
        assert!(rules(&[""], &[""]).unwrap().is_empty());
    }
}
