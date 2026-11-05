//! Fuzzy matching with an fzf-style query language, used by Quick Open.
//!
//! A query is a list of space-separated terms that must all match:
//!
//! | Term          | Meaning                                              |
//! |---------------|------------------------------------------------------|
//! | `abc`         | fuzzy: the letters appear in order                   |
//! | `'abc`        | exact substring                                      |
//! | `^abc`        | name starts with                                     |
//! | `abc$`        | name ends with                                       |
//! | `^abc$`       | name equals                                          |
//! | `!abc`        | name/path must *not* contain                         |
//! | `*.rs`, `a?c` | wildcard against the name                            |
//! | `ext:rs,toml` | extension is one of                                  |
//! | `src/ main`   | any term containing `/` matches the whole path       |
//! | `proj/`       | a query ending in `/` lists only directories         |
//!
//! `\` and `/` are interchangeable, so path terms also work on Windows paths.
//! Matching is smart-case: a term with an upper-case letter is case-sensitive.
//! Plain terms try the file name first (scored higher, like an editor's quick
//! open) and fall back to the full path.

use super::entry::extension_of;
use super::search::Wildcard;

const SCORE_MATCH: i32 = 16;
const BONUS_BOUNDARY: i32 = 10;
const BONUS_SLASH: i32 = 12;
const BONUS_CAMEL: i32 = 8;
const BONUS_CONSECUTIVE: i32 = 6;
const PENALTY_GAP_START: i32 = 3;
const PENALTY_GAP_EXT: i32 = 1;
const BONUS_NAME: i32 = 30;
const PENALTY_PATH_ONLY: i32 = 10;

#[derive(Debug, Clone)]
enum Kind {
    Fuzzy,
    Exact,
    Prefix,
    Suffix,
    Equal,
    Glob(Wildcard),
    Ext(Vec<String>),
}

#[derive(Debug, Clone)]
struct Term {
    kind: Kind,
    /// Pattern characters; lower-cased unless `case_sensitive`.
    pat: Vec<char>,
    case_sensitive: bool,
    inverse: bool,
    /// Matches against the whole path instead of the name.
    path: bool,
    /// The pattern has no accented letters: they match their base letter ("porocilo"
    /// finds "Poročilo"), as smart case does for capitals. Typed accents match exactly.
    fold: bool,
}

/// A parsed query. Cheap to clone and safe to share between threads.
#[derive(Debug, Clone, Default)]
pub struct Query {
    terms: Vec<Term>,
    /// The query ended in a separator: only directories match.
    dirs_only: bool,
}

/// Score and highlighted character positions (indices into the path's chars).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    pub positions: Vec<usize>,
}

/// Reusable buffers so scoring a million paths does not allocate per path.
#[derive(Default)]
pub struct Scratch {
    chars: Vec<char>,
    lower: Vec<char>,
    /// `chars` / `lower` with accents removed (see [`fold_char`]), for paths with any.
    fold_chars: Vec<char>,
    fold_lower: Vec<char>,
}

/// Latin letters with accents (Latin-1 and Latin Extended-A: Slovenian, Croatian, Czech,
/// Polish, German, French, …) and their base letter; every letter stays one letter, so
/// match positions still point at the right characters.
const FOLD: &[(&str, char)] = &[
    ("àáâãäåāăą", 'a'),
    ("ÀÁÂÃÄÅĀĂĄ", 'A'),
    ("çćĉċč", 'c'),
    ("ÇĆĈĊČ", 'C'),
    ("ďđ", 'd'),
    ("ĎĐ", 'D'),
    ("èéêëēĕėęě", 'e'),
    ("ÈÉÊËĒĔĖĘĚ", 'E'),
    ("ĝğġģ", 'g'),
    ("ĜĞĠĢ", 'G'),
    ("ĥħ", 'h'),
    ("ĤĦ", 'H'),
    ("ìíîïĩīĭįı", 'i'),
    ("ÌÍÎÏĨĪĬĮİ", 'I'),
    ("ĵ", 'j'),
    ("Ĵ", 'J'),
    ("ķ", 'k'),
    ("Ķ", 'K'),
    ("ĺļľŀł", 'l'),
    ("ĹĻĽĿŁ", 'L'),
    ("ñńņňŉ", 'n'),
    ("ÑŃŅŇ", 'N'),
    ("òóôõöøōŏő", 'o'),
    ("ÒÓÔÕÖØŌŎŐ", 'O'),
    ("ŕŗř", 'r'),
    ("ŔŖŘ", 'R'),
    ("śŝşš", 's'),
    ("ŚŜŞŠ", 'S'),
    ("ţťŧ", 't'),
    ("ŢŤŦ", 'T'),
    ("ùúûüũūŭůűų", 'u'),
    ("ÙÚÛÜŨŪŬŮŰŲ", 'U'),
    ("ŵ", 'w'),
    ("Ŵ", 'W'),
    ("ýÿŷ", 'y'),
    ("ÝŸŶ", 'Y'),
    ("źżž", 'z'),
    ("ŹŻŽ", 'Z'),
];

/// The base letter of an accented Latin letter (č → c); anything else unchanged.
pub fn fold_char(c: char) -> char {
    if c.is_ascii() {
        return c;
    }
    FOLD.iter()
        .find(|(set, _)| set.contains(c))
        .map_or(c, |(_, base)| *base)
}

impl Query {
    pub fn parse(q: &str) -> Self {
        let mut raw: Vec<&str> = q.split_whitespace().collect();
        // A trailing separator ("proj/", "src/comp/") asks for directories: drop it
        // from the last term (directory paths are indexed without one).
        let dirs_only = raw.last().is_some_and(|t| t.ends_with(is_sep));
        if let Some(last) = raw.last_mut() {
            *last = last.trim_end_matches(is_sep);
        }
        let terms = raw.into_iter().filter_map(Term::parse).collect();
        Query { terms, dirs_only }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Only directories can match (the query ended in `/` or `\`).
    pub fn dirs_only(&self) -> bool {
        self.dirs_only
    }

    /// Fast byte-level rejection: every positive literal term must have its
    /// ASCII letters present in order somewhere in the path.
    fn prefilter(&self, path: &str) -> bool {
        let bytes = path.as_bytes();
        let ascii = path.is_ascii();
        self.terms.iter().all(|t| {
            // An accented letter in the path may stand for a plain one in the pattern.
            if t.fold && !ascii {
                return true;
            }
            if t.inverse
                || !matches!(
                    t.kind,
                    Kind::Fuzzy | Kind::Exact | Kind::Prefix | Kind::Suffix | Kind::Equal
                )
            {
                return true;
            }
            let mut i = 0;
            for &c in &t.pat {
                if !c.is_ascii() {
                    return true; // cannot prefilter non-ASCII cheaply; let the scorer decide
                }
                let c = c as u8;
                loop {
                    let Some(&b) = bytes.get(i) else { return false };
                    i += 1;
                    let b = if b == b'\\' { b'/' } else { b };
                    let b = if t.case_sensitive {
                        b
                    } else {
                        b.to_ascii_lowercase()
                    };
                    if b == c {
                        break;
                    }
                }
            }
            true
        })
    }

    /// Scores `path`, whose file name starts at byte `name_start`.
    /// Returns `None` when any term fails.
    pub fn score(&self, path: &str, name_start: usize, s: &mut Scratch) -> Option<Match> {
        if !self.prefilter(path) {
            return None;
        }
        s.chars.clear();
        s.chars.extend(path.chars().map(norm_sep));
        s.lower.clear();
        s.lower.extend(s.chars.iter().map(|c| lower(*c)));
        let folded = !path.is_ascii() && self.terms.iter().any(|t| t.fold);
        if folded {
            s.fold_chars.clear();
            s.fold_chars.extend(s.chars.iter().map(|c| fold_char(*c)));
            s.fold_lower.clear();
            s.fold_lower.extend(s.lower.iter().map(|c| fold_char(*c)));
        }
        let (fc, fl) = if folded {
            (&s.fold_chars[..], &s.fold_lower[..])
        } else {
            (&s.chars[..], &s.lower[..])
        };
        let name_off = path[..name_start.min(path.len())].chars().count();
        let name = &path[name_start.min(path.len())..];

        let mut total = 0;
        let mut positions = Vec::new();
        for t in &self.terms {
            let r = if t.fold {
                t.eval(
                    &s.chars,
                    if t.case_sensitive { fc } else { fl },
                    name_off,
                    name,
                )
            } else {
                t.eval(
                    &s.chars,
                    if t.case_sensitive { &s.chars } else { &s.lower },
                    name_off,
                    name,
                )
            };
            match (r, t.inverse) {
                (Some(_), true) => return None,
                (None, true) => {}
                (None, false) => return None,
                (Some(m), false) => {
                    total += m.score;
                    positions.extend(m.positions);
                }
            }
        }
        positions.sort_unstable();
        positions.dedup();
        Some(Match {
            score: total,
            positions,
        })
    }
}

fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Path separators compare equal: `\` is matched as `/`.
fn norm_sep(c: char) -> char {
    if c == '\\' {
        '/'
    } else {
        c
    }
}

fn lower(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

impl Term {
    fn parse(raw: &str) -> Option<Term> {
        let mut t = raw;
        let inverse = t.len() > 1 && t.starts_with('!');
        if inverse {
            t = &t[1..];
        }
        if let Some(exts) = t.strip_prefix("ext:") {
            let list: Vec<String> = exts
                .split(',')
                .map(|e| e.trim_start_matches('.').to_lowercase())
                .filter(|e| !e.is_empty())
                .collect();
            if list.is_empty() {
                return None;
            }
            return Some(Term {
                kind: Kind::Ext(list),
                pat: vec![],
                case_sensitive: false,
                inverse,
                path: false,
                fold: false,
            });
        }
        let path = t.contains(is_sep);
        if t.contains('*') || t.contains('?') {
            // Path terms are matched against the path with `\` turned into `/`.
            let mut pattern = if path {
                t.replace('\\', "/")
            } else {
                t.to_string()
            };
            // Indexed paths are absolute: a relative pattern ("src/*.rs") matches from the
            // start of any folder name in the path, not only from the root.
            let b = pattern.as_bytes();
            let absolute = b.first().is_some_and(|&c| c == b'/' || c == b'*')
                || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':');
            if path && !absolute {
                pattern.insert_str(0, "*/");
            }
            return Some(Term {
                kind: Kind::Glob(Wildcard::new(&pattern)),
                pat: vec![],
                case_sensitive: false,
                inverse,
                path,
                fold: false,
            });
        }
        let (kind, body) = if inverse {
            // Inverse terms are exact, as in fzf.
            (Kind::Exact, t)
        } else if let Some(b) = t.strip_prefix('\'') {
            (Kind::Exact, b)
        } else if t.len() > 2 && t.starts_with('^') && t.ends_with('$') {
            (Kind::Equal, &t[1..t.len() - 1])
        } else if t.len() > 1 && t.starts_with('^') {
            (Kind::Prefix, &t[1..])
        } else if t.len() > 1 && t.ends_with('$') {
            (Kind::Suffix, &t[..t.len() - 1])
        } else {
            (Kind::Fuzzy, t)
        };
        if body.is_empty() {
            return None;
        }
        let case_sensitive = body.chars().any(|c| c.is_uppercase());
        let fold = body.chars().all(|c| fold_char(c) == c);
        let pat = body
            .chars()
            .map(norm_sep)
            .map(|c| if case_sensitive { c } else { lower(c) })
            .collect();
        Some(Term {
            kind,
            pat,
            case_sensitive,
            inverse,
            path,
            fold,
        })
    }

    /// `hay`: the path as the pattern is matched against it (lower-cased and accent-folded
    /// as the term asks); `chars`: the path as written, for word-boundary bonuses.
    fn eval(&self, chars: &[char], hay: &[char], name_off: usize, name: &str) -> Option<Match> {
        match &self.kind {
            Kind::Ext(list) => list.contains(&extension_of(name)).then(|| Match {
                score: 0,
                positions: vec![],
            }),
            Kind::Glob(w) => {
                let target: String = if self.path {
                    chars.iter().collect()
                } else {
                    name.to_string()
                };
                w.matches(&target).then(|| Match {
                    score: 20,
                    positions: vec![],
                })
            }
            Kind::Fuzzy => {
                if !self.path {
                    if let Some(mut m) = fuzzy(chars, hay, name_off, &self.pat) {
                        m.score += BONUS_NAME;
                        return Some(m);
                    }
                }
                let mut m = fuzzy(chars, hay, 0, &self.pat)?;
                if !self.path {
                    m.score -= PENALTY_PATH_ONLY;
                }
                Some(m)
            }
            Kind::Exact => {
                if !self.path {
                    if let Some(i) = find(&hay[name_off..], &self.pat) {
                        return Some(self.literal(chars, name_off + i, BONUS_NAME));
                    }
                }
                find(hay, &self.pat).map(|i| self.literal(chars, i, 0))
            }
            Kind::Prefix => {
                let from = if self.path { 0 } else { name_off };
                hay[from..]
                    .starts_with(&self.pat)
                    .then(|| self.literal(chars, from, BONUS_NAME))
            }
            Kind::Suffix => {
                let n = self.pat.len();
                (hay.len() >= n
                    && hay[hay.len() - n..] == self.pat[..]
                    && (self.path || hay.len() - n >= name_off))
                    .then(|| self.literal(chars, hay.len() - n, BONUS_NAME / 2))
            }
            Kind::Equal => {
                let from = if self.path { 0 } else { name_off };
                (hay[from..] == self.pat[..]).then(|| self.literal(chars, from, BONUS_NAME * 2))
            }
        }
    }

    fn literal(&self, chars: &[char], at: usize, extra: i32) -> Match {
        let n = self.pat.len();
        let score = SCORE_MATCH * n as i32
            + BONUS_CONSECUTIVE * (n as i32 - 1)
            + bonus_at(chars, at) * 2
            + extra;
        Match {
            score,
            positions: (at..at + n).collect(),
        }
    }
}

fn find(hay: &[char], pat: &[char]) -> Option<usize> {
    if pat.len() > hay.len() {
        return None;
    }
    hay.windows(pat.len()).position(|w| w == pat)
}

/// Bonus for a match at position `i`, based on the character before it.
fn bonus_at(chars: &[char], i: usize) -> i32 {
    if i == 0 {
        return BONUS_BOUNDARY;
    }
    let (p, c) = (chars[i - 1], chars[i]);
    match p {
        '/' | '\\' => BONUS_SLASH,
        '_' | '-' | '.' | ' ' | '(' | '[' => BONUS_BOUNDARY,
        _ if p.is_lowercase() && c.is_uppercase() => BONUS_CAMEL,
        _ if !p.is_ascii_digit() && c.is_ascii_digit() => BONUS_CAMEL / 2,
        _ => 0,
    }
}

/// fzf "v1" algorithm: find the first full subsequence match, shrink it from
/// the right to the shortest window, then score greedily inside that window.
/// `from` restricts the search to `hay[from..]` (used for the file name).
fn fuzzy(chars: &[char], hay: &[char], from: usize, pat: &[char]) -> Option<Match> {
    if pat.is_empty() || hay.len() - from < pat.len() {
        return None;
    }
    // Forward pass.
    let mut pi = 0;
    let mut end = None;
    for (i, &c) in hay.iter().enumerate().skip(from) {
        if c == pat[pi] {
            pi += 1;
            if pi == pat.len() {
                end = Some(i);
                break;
            }
        }
    }
    let end = end?;
    // Backward pass.
    let mut pi = pat.len() - 1;
    let mut start = end;
    for i in (from..=end).rev() {
        if hay[i] == pat[pi] {
            if pi == 0 {
                start = i;
                break;
            }
            pi -= 1;
        }
    }
    // Scoring pass.
    let mut score = 0;
    let mut positions = Vec::with_capacity(pat.len());
    let mut pi = 0;
    let mut prev_match = false;
    let mut in_gap = false;
    let mut run_bonus = 0;
    for (i, &c) in hay.iter().enumerate().take(end + 1).skip(start) {
        if pi < pat.len() && c == pat[pi] {
            let mut s = SCORE_MATCH;
            let mut b = bonus_at(chars, i);
            if prev_match {
                // A consecutive run keeps the bonus of the boundary it started on.
                b = b.max(run_bonus);
                s += BONUS_CONSECUTIVE;
            } else {
                run_bonus = b;
            }
            s += if pi == 0 { b * 2 } else { b };
            score += s;
            positions.push(i);
            pi += 1;
            prev_match = true;
            in_gap = false;
        } else {
            score -= if in_gap {
                PENALTY_GAP_EXT
            } else {
                PENALTY_GAP_START
            };
            in_gap = true;
            prev_match = false;
        }
    }
    // Prefer matches that start early in the segment.
    score -= ((start - from) as i32).min(8);
    Some(Match { score, positions })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(q: &str, path: &str) -> Option<i32> {
        let name_start = path.rfind(is_sep).map(|i| i + 1).unwrap_or(0);
        Query::parse(q)
            .score(path, name_start, &mut Scratch::default())
            .map(|m| m.score)
    }

    /// Typed without accents, a query finds names with them (Slovenian users often type
    /// "porocilo" for "Poročilo"); typed with accents it matches them exactly.
    #[test]
    fn accents_fold_unless_typed() {
        let p = "C:/u/Dokumenti/Poročilo čšž.txt";
        assert!(sc("porocilo", p).is_some());
        assert!(sc("porocilo csz", p).is_some());
        assert!(sc("'porocilo", p).is_some(), "exact terms fold too");
        assert!(
            sc("Porocilo", p).is_some(),
            "smart case and folding together"
        );
        assert!(sc("poročilo", p).is_some());
        assert!(
            sc("poročilo", "C:/u/porocilo.txt").is_none(),
            "a typed č wants č"
        );
        // The folded match counts as the name's own: it beats a scattered match.
        assert!(
            sc("porocilo", p).unwrap()
                > sc("porocilo", "C:/x/PowerToys.GcodePreviewHandler.dll").unwrap_or(i32::MIN)
        );
        // Positions point at the written letters (č is one character).
        assert_eq!(pos("porocilo", p), (15..23).collect::<Vec<_>>());
        assert_eq!(fold_char('Ž'), 'Z');
        assert_eq!(fold_char('ł'), 'l');
        assert_eq!(fold_char('日'), '日');
    }

    fn pos(q: &str, path: &str) -> Vec<usize> {
        let name_start = path.rfind('/').map(|i| i + 1).unwrap_or(0);
        Query::parse(q)
            .score(path, name_start, &mut Scratch::default())
            .unwrap()
            .positions
    }

    #[test]
    fn fuzzy_basics() {
        assert!(sc("fpane", "/src/components/FolderPane.tsx").is_some());
        assert!(sc("xyz", "/src/components/FolderPane.tsx").is_none());
        assert_eq!(pos("fp", "/a/FolderPane.tsx"), vec![3, 9]);
    }

    #[test]
    fn name_beats_path_and_boundaries_win() {
        // Match inside the file name scores above a match spread over the path.
        assert!(sc("main", "/x/main.rs") > sc("main", "/m/a/i/n/x.rs"));
        // Word-boundary matches beat mid-word matches.
        assert!(sc("fp", "/a/FolderPane.tsx") > sc("fp", "/a/offprint.tsx"));
        // Consecutive beats scattered.
        assert!(sc("app", "/x/app.css") > sc("app", "/x/a_p_p.css"));
    }

    #[test]
    fn smart_case() {
        assert!(sc("readme", "/x/README.md").is_some());
        assert!(sc("README", "/x/readme.md").is_none());
        assert!(sc("README", "/x/README.md").is_some());
    }

    #[test]
    fn extended_syntax() {
        assert!(sc("'pane", "/a/FolderPane.tsx").is_some());
        assert!(sc("'fpane", "/a/FolderPane.tsx").is_none());
        assert!(sc("^fold", "/a/FolderPane.tsx").is_some());
        assert!(sc("^pane", "/a/FolderPane.tsx").is_none());
        assert!(sc(".tsx$", "/a/FolderPane.tsx").is_some());
        assert!(sc(".ts$", "/a/FolderPane.tsx").is_none());
        assert!(sc("^app.css$", "/ui/app.css").is_some());
        assert!(sc("^app$", "/ui/app.css").is_none());
        assert!(sc("pane !test", "/a/FolderPane.tsx").is_some());
        assert!(sc("pane !folder", "/a/FolderPane.tsx").is_none());
        assert!(sc("*.tsx", "/a/FolderPane.tsx").is_some());
        assert!(sc("ext:tsx,ts", "/a/FolderPane.tsx").is_some());
        assert!(sc("ext:rs", "/a/FolderPane.tsx").is_none());
    }

    #[test]
    fn path_terms_and_multiple_terms() {
        assert!(sc("comp/pane", "/src/components/FolderPane.tsx").is_some());
        assert!(sc("ui/ pane", "/ui/src/FolderPane.tsx").is_some());
        assert!(sc("ui/ pane", "/core/src/FolderPane.tsx").is_none());
        // Directory name only matches through the path fallback.
        assert!(sc("components", "/src/components/x.ts").is_some());
    }

    #[test]
    fn trailing_separator_means_directories() {
        let q = Query::parse("proj/");
        assert!(q.dirs_only());
        assert!(Query::parse("proj\\").dirs_only());
        assert!(!Query::parse("src/ pane  ").dirs_only());
        assert!(!Query::parse("proj").dirs_only());
        // The separator is dropped: "proj/" matches the directory itself by name.
        assert!(sc("proj/", "/home/u/Projects").is_some());
        assert!(sc("src/comp/", "/x/src/components").is_some());
        // A lone "/" is not a query.
        assert!(Query::parse("/").is_empty());
    }

    #[test]
    fn windows_separators() {
        assert!(sc("comp/pane", r"C:\src\components\FolderPane.tsx").is_some());
        assert!(sc(r"comp\pane", "/src/components/FolderPane.tsx").is_some());
        assert!(sc("ui/ pane", r"C:\ui\src\FolderPane.tsx").is_some());
        assert!(sc("ui/ pane", r"C:\core\src\FolderPane.tsx").is_none());
        // Wildcard path terms too, either separator on either side.
        assert!(sc(r"*src\*.rs", r"C:\p\src\main.rs").is_some());
        assert!(sc(r"*src\*.rs", "/p/src/main.rs").is_some());
        assert!(sc(r"*src\*.rs", "/p/lib/main.rs").is_none());
    }

    /// Paths in the index are absolute; a relative wildcard path term names the end of one.
    #[test]
    fn wildcard_path_terms_match_inside_the_path() {
        assert!(sc("src/*.rs", "/home/u/p/src/main.rs").is_some());
        assert!(sc(r"src\*.rs", r"C:\p\src\main.rs").is_some());
        assert!(sc("src/*.rs", "/home/u/p/src/main.ts").is_none());
        assert!(
            sc("src/*.rs", "/home/u/p/mysrc/main.rs").is_none(),
            "whole folder name"
        );
        assert!(sc("/home/*.rs", "/home/u/p/src/main.rs").is_some());
        assert!(
            sc("/opt/*.rs", "/home/u/p/src/main.rs").is_none(),
            "absolute stays anchored"
        );
        assert!(sc(r"C:\p\*.rs", r"C:\p\src\main.rs").is_some());
        assert!(sc(r"D:\p\*.rs", r"C:\p\src\main.rs").is_none());
    }

    #[test]
    fn unicode_names() {
        assert!(sc("šum", "/x/Šumnik čšž.txt").is_some());
        assert_eq!(pos("čš", "/x/čšž"), vec![3, 4]);
    }
}
