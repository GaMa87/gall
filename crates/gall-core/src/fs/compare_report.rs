//! Standalone reports of comparisons: one report model (files, hunks, rows) rendered as a static
//! HTML page. The page has no script and no remote asset (a content security policy forbids
//! them), every name and line of text is escaped, and ids are made from counters, never from
//! file names. Links lead from the contents to each change and between changes.
//!
//! Building and rendering take a `check` callback that can end the work (`Err`), so a large
//! export can be cancelled; callers run both off the UI thread.

use super::diff::{compare_with_cancel, DiffOptions, DiffResult, DiffSource, TextDiffResult};
use crate::Result;
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// Most comparisons in one report; a bigger set is split by the caller.
pub const MAX_PAIRS: usize = 500;
/// Unchanged lines shown around each change.
pub const CONTEXT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Context,
    Removed,
    Added,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRow {
    pub kind: RowKind,
    /// 1-based line numbers on each side.
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportHunk {
    pub rows: Vec<ReportRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReportFile {
    pub left: String,
    pub right: String,
    pub added: usize,
    pub removed: usize,
    /// Why there are no hunks: identical, binary, …
    pub note: Option<String>,
    pub hunks: Vec<ReportHunk>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub title: String,
    pub files: Vec<ReportFile>,
}

/// The hunks of a text comparison, with `context` unchanged lines around each change.
pub fn hunks_of(
    text: &TextDiffResult,
    context: usize,
    check: &dyn Fn() -> Result<()>,
) -> Result<Vec<ReportHunk>> {
    let ops = &text.ops;
    let changed = |i: usize| ops[i].tag != "equal" && !ops[i].cosmetic;
    let mut hunks = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        check()?;
        if !changed(i) {
            i += 1;
            continue;
        }
        // The group: changes separated by short unchanged stretches (they would share context).
        let first = i;
        let mut last = i;
        let mut j = i + 1;
        while j < ops.len() {
            if changed(j) {
                last = j;
                j += 1;
            } else if ops[j].tag == "equal"
                && ops[j].a_len <= 2 * context
                && j + 1 < ops.len()
                && changed(j + 1)
            {
                j += 1;
            } else if ops[j].tag != "equal" {
                // A change that only differs in layout: part of the stretch.
                j += 1;
            } else {
                break;
            }
        }
        let mut rows = Vec::new();
        let line = |side: &Vec<String>, n: usize| side.get(n).cloned().unwrap_or_default();
        let quiet = |i: usize| ops[i].tag == "equal" || ops[i].cosmetic;
        if first > 0 && quiet(first - 1) {
            let p = &ops[first - 1];
            let take = p.a_len.min(context);
            for k in p.a_len - take..p.a_len {
                rows.push(ReportRow {
                    kind: RowKind::Context,
                    left: Some(p.a + k + 1),
                    right: Some(p.b + k + 1),
                    text: line(&text.left_lines, p.a + k),
                });
            }
        }
        for op in &ops[first..=last] {
            if op.tag == "equal" || op.cosmetic {
                // A change of layout only is no change: its lines are shown as they are on the left.
                for k in 0..op.a_len {
                    rows.push(ReportRow {
                        kind: RowKind::Context,
                        left: Some(op.a + k + 1),
                        right: (k < op.b_len).then_some(op.b + k + 1),
                        text: line(&text.left_lines, op.a + k),
                    });
                }
                continue;
            }
            for k in 0..op.a_len {
                rows.push(ReportRow {
                    kind: RowKind::Removed,
                    left: Some(op.a + k + 1),
                    right: None,
                    text: line(&text.left_lines, op.a + k),
                });
            }
            for k in 0..op.b_len {
                rows.push(ReportRow {
                    kind: RowKind::Added,
                    left: None,
                    right: Some(op.b + k + 1),
                    text: line(&text.right_lines, op.b + k),
                });
            }
        }
        if let Some(n) = ops.get(last + 1).filter(|n| n.tag == "equal" || n.cosmetic) {
            for k in 0..n.a_len.min(context) {
                rows.push(ReportRow {
                    kind: RowKind::Context,
                    left: Some(n.a + k + 1),
                    right: Some(n.b + k + 1),
                    text: line(&text.left_lines, n.a + k),
                });
            }
        }
        hunks.push(ReportHunk { rows });
        i = last + 1;
    }
    Ok(hunks)
}

/// The report entry of one comparison result.
pub fn file_of(result: &DiffResult, check: &dyn Fn() -> Result<()>) -> Result<ReportFile> {
    let (left, right) = (result.left.label.clone(), result.right.label.clone());
    if let Some(text) = &result.text {
        let hunks = hunks_of(text, CONTEXT, check)?;
        let note = hunks
            .is_empty()
            .then(|| "The files are identical.".to_string());
        return Ok(ReportFile {
            left,
            right,
            added: text.added,
            removed: text.removed,
            note,
            hunks,
        });
    }
    let note = match &result.binary {
        Some(b) if b.identical => "The files are identical (binary).".to_string(),
        Some(b) => format!("Binary files differ: {}.", b.reason),
        None => "Not compared.".to_string(),
    };
    Ok(ReportFile {
        left,
        right,
        added: 0,
        removed: 0,
        note: Some(note),
        hunks: Vec::new(),
    })
}

/// Compares each pair and builds the report. A pair that cannot be read is listed with the
/// reason instead of ending the report.
pub fn build(
    title: &str,
    pairs: &[(DiffSource, DiffSource)],
    opts: &DiffOptions,
    cancel: &AtomicBool,
) -> Result<Report> {
    let check = &|| {
        if cancel.load(Ordering::Relaxed) {
            Err(crate::CoreError::Cancelled)
        } else {
            Ok(())
        }
    };
    if pairs.len() > MAX_PAIRS {
        return Err(crate::CoreError::Other(format!(
            "a report holds at most {MAX_PAIRS} comparisons ({} requested)",
            pairs.len()
        )));
    }
    // An invalid ignore rule refuses the whole report with the rule and the reason, instead of
    // being listed as "not compared" for every pair.
    super::ignorerules::Rules::compile(&opts.ignore_lines, &opts.ignore_text)
        .map_err(crate::CoreError::Other)?;
    let mut files = Vec::new();
    for (a, b) in pairs {
        check()?;
        match compare_with_cancel(a, b, opts.clone(), cancel) {
            Ok(result) => files.push(file_of(&result, check)?),
            // A cancelled comparison ends the report; any other error is listed for its pair.
            Err(crate::CoreError::Cancelled) => return Err(crate::CoreError::Cancelled),
            Err(e) => files.push(ReportFile {
                left: source_name(a),
                right: source_name(b),
                added: 0,
                removed: 0,
                note: Some(format!("Not compared: {e}")),
                hunks: Vec::new(),
            }),
        }
    }
    Ok(Report {
        title: title.to_string(),
        files,
    })
}

fn source_name(s: &DiffSource) -> String {
    match s {
        DiffSource::File { path }
        | DiffSource::GitHead { path }
        | DiffSource::GitCommit { path, .. } => path.display().to_string(),
        DiffSource::GitBlob { file, .. } => file.clone(),
        DiffSource::Text { label, .. } => label.clone(),
    }
}

/// Escapes text for HTML content and attribute values.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // Control characters other than tab have no place in a page.
            c if (c as u32) < 0x20 && c != '\t' => out.push('\u{FFFD}'),
            // Bidirectional controls would reorder the text around them (a name or a line that reads
            // as another one): shown as a visible mark instead of acting.
            c if matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}') => {
                let _ = write!(out, "&lt;U+{:04X}&gt;", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

const STYLE: &str = "body{font:14px/1.45 system-ui,sans-serif;margin:2rem;color:#1f2328;background:#fff}\
h1{font-size:1.4rem}h2{font-size:1.1rem;margin-top:2rem;word-break:break-all}\
nav ul{list-style:none;padding-left:1rem}nav a,.hn a{color:#0969da;text-decoration:none}\
.hn{font-size:12px;margin:1rem 0 .25rem;color:#57606a}\
table.h{border-collapse:collapse;width:100%;font:12px/1.4 ui-monospace,monospace}\
table.h td{padding:0 .5rem;vertical-align:top}td.n{color:#8c959f;text-align:right;user-select:none;width:3rem}\
td.t{white-space:pre-wrap;word-break:break-all}tr.r td{background:#ffebe9}tr.a td{background:#dafbe1}\
.note{color:#57606a;font-style:italic}\
@media print{body{margin:0}h2{break-after:avoid}tr{break-inside:avoid}}";

/// The page. Works offline; contains no script.
pub fn render_html(report: &Report, check: &dyn Fn() -> Result<()>) -> Result<String> {
    let mut out = String::new();
    let total: usize = report.files.iter().map(|f| f.hunks.len()).sum();
    out.push_str("<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">");
    out.push_str("<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\">");
    let _ = write!(
        out,
        "<title>{}</title><style>{STYLE}</style></head><body>",
        escape(&report.title)
    );
    let _ = write!(out, "<h1 id=\"top\">{}</h1>", escape(&report.title));
    out.push_str("<nav aria-label=\"Contents\"><ul>");
    let mut n = 0;
    for (fi, f) in report.files.iter().enumerate() {
        let _ = write!(
            out,
            "<li><a href=\"#f{fi}\">{} ↔ {}</a> (+{} −{})<ul>",
            escape(&f.left),
            escape(&f.right),
            f.added,
            f.removed
        );
        for (hi, h) in f.hunks.iter().enumerate() {
            n += 1;
            let first = h.rows.iter().find_map(|r| r.left.or(r.right)).unwrap_or(1);
            let _ = write!(
                out,
                "<li><a href=\"#f{fi}h{hi}\">Change {n} of {total}, line {first}</a></li>"
            );
        }
        out.push_str("</ul></li>");
    }
    out.push_str("</ul></nav>");
    let mut order: Vec<(usize, usize)> = Vec::new();
    for (fi, f) in report.files.iter().enumerate() {
        order.extend((0..f.hunks.len()).map(|hi| (fi, hi)));
    }
    let mut at = 0;
    for (fi, f) in report.files.iter().enumerate() {
        check()?;
        let _ = write!(
            out,
            "<section><h2 id=\"f{fi}\">{} ↔ {}</h2>",
            escape(&f.left),
            escape(&f.right)
        );
        if let Some(note) = &f.note {
            let _ = write!(out, "<p class=\"note\">{}</p>", escape(note));
        }
        for (hi, h) in f.hunks.iter().enumerate() {
            let _ = write!(
                out,
                "<p class=\"hn\" id=\"f{fi}h{hi}\">Change {} of {total} · ",
                at + 1
            );
            if at > 0 {
                let (pf, ph) = order[at - 1];
                let _ = write!(out, "<a href=\"#f{pf}h{ph}\">previous</a> · ");
            }
            if let Some(&(nf, nh)) = order.get(at + 1) {
                let _ = write!(out, "<a href=\"#f{nf}h{nh}\">next</a> · ");
            }
            out.push_str("<a href=\"#top\">contents</a></p><table class=\"h\">");
            at += 1;
            for r in &h.rows {
                let (class, sign) = match r.kind {
                    RowKind::Context => ("c", " "),
                    RowKind::Removed => ("r", "−"),
                    RowKind::Added => ("a", "+"),
                };
                let num = |n: Option<usize>| n.map(|n| n.to_string()).unwrap_or_default();
                let _ = write!(
                    out,
                    "<tr class=\"{class}\"><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"t\">{sign} {}</td></tr>",
                    num(r.left),
                    num(r.right),
                    escape(&r.text)
                );
            }
            out.push_str("</table>");
        }
        out.push_str("</section>");
    }
    out.push_str("</body></html>\n");
    Ok(out)
}

/// Builds the report and writes the page to `dest` (a temporary file in the same folder is
/// renamed over it, so a cancelled or failed export leaves no half page).
pub fn export_html(
    dest: &std::path::Path,
    title: &str,
    pairs: &[(DiffSource, DiffSource)],
    opts: &DiffOptions,
    cancel: &AtomicBool,
) -> Result<()> {
    let check = || {
        if cancel.load(Ordering::Relaxed) {
            Err(crate::CoreError::Cancelled)
        } else {
            Ok(())
        }
    };
    let report = build(title, pairs, opts, cancel)?;
    let page = render_html(&report, &check)?;
    check()?;
    write_atomic(dest, page.as_bytes())
}

/// Writes `bytes` to `dest` through a temporary file of its own name in the same folder that is renamed over it, so a
/// failed export leaves no half file (the temporary file is removed again when anything fails).
pub(super) fn write_atomic(dest: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use crate::error::IoContext;
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dest.with_file_name(format!(".{name}.{}.part", std::process::id()));
    let written = std::fs::write(&tmp, bytes)
        .ctx("write", &tmp)
        .and_then(|()| std::fs::rename(&tmp, dest).ctx("replace", dest));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::diff::diff_text;
    use crate::CoreError;

    fn ok() -> Result<()> {
        Ok(())
    }

    fn report_of(a: &str, b: &str) -> Report {
        let text = diff_text(a, b, DiffOptions::default());
        let hunks = hunks_of(&text, CONTEXT, &ok).unwrap();
        Report {
            title: "T".into(),
            files: vec![ReportFile {
                left: "a".into(),
                right: "b".into(),
                added: text.added,
                removed: text.removed,
                note: None,
                hunks,
            }],
        }
    }

    #[test]
    fn distant_changes_are_separate_hunks_with_context() {
        let a: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let b = a
            .replace("line 5\n", "five\n")
            .replace("line 30\n", "thirty\n");
        let r = report_of(&a, &b);
        let h = &r.files[0].hunks;
        assert_eq!(h.len(), 2);
        // 3 before, 1 removed, 1 added, 3 after.
        assert_eq!(h[0].rows.len(), 8);
        assert_eq!(h[0].rows[0].text, "line 2");
        assert_eq!(h[0].rows[3].kind, RowKind::Removed);
        assert_eq!(h[0].rows[4].kind, RowKind::Added);
    }

    #[test]
    fn bidirectional_controls_are_shown_not_applied() {
        let e = escape("evil\u{202E}gpj.exe \u{2066}x\u{2069}");
        assert_eq!(e, "evil&lt;U+202E&gt;gpj.exe &lt;U+2066&gt;x&lt;U+2069&gt;");
        assert!(!e
            .chars()
            .any(|c| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')));
    }

    #[test]
    fn a_report_with_an_invalid_ignore_rule_is_refused_with_the_reason() {
        let text = |t: &str| DiffSource::Text {
            label: "t".into(),
            text: t.into(),
        };
        let opts = DiffOptions {
            ignore_lines: vec!["(".into()],
            ..DiffOptions::default()
        };
        let error = build(
            "T",
            &[(text("a\n"), text("b\n"))],
            &opts,
            &AtomicBool::new(false),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Invalid ignore rule"), "{error}");
    }

    #[test]
    fn close_changes_share_one_hunk() {
        let a: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let b = a
            .replace("line 5\n", "five\n")
            .replace("line 8\n", "eight\n");
        assert_eq!(report_of(&a, &b).files[0].hunks.len(), 1);
    }

    #[test]
    fn page_is_escaped_offline_and_linked() {
        let a = "<script>alert(1)</script>\nsame\n";
        let b = "<img src=x onerror=alert(2)>&\nsame\n";
        let mut r = report_of(a, b);
        r.files[0].left = "<b>left</b>.txt".into();
        r.files[0].right = "\"quote\" & 'tick'.txt".into();
        let page = render_html(&r, &ok).unwrap();
        for raw in ["<script>", "<img", "<b>left", "onerror=alert(2)>"] {
            assert!(!page.contains(raw), "{raw} appears unescaped");
        }
        assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(page.contains("&lt;b&gt;left&lt;/b&gt;.txt"));
        assert!(page.contains("&quot;quote&quot; &amp; &#39;tick&#39;.txt"));
        // No remote asset or script: only the page itself, and a policy that forbids more.
        assert!(!page.contains("http://") && !page.contains("https://"));
        assert!(page.contains("default-src 'none'"));
        assert!(page.contains("href=\"#f0h0\"") && page.contains("id=\"f0h0\""));
    }

    #[test]
    fn changes_link_to_each_other() {
        let a: String = (0..60).map(|i| format!("line {i}\n")).collect();
        let b = a.replace("line 5\n", "x\n").replace("line 50\n", "y\n");
        let page = render_html(&report_of(&a, &b), &ok).unwrap();
        assert!(page.contains("Change 1 of 2") && page.contains("Change 2 of 2"));
        assert!(page.contains(">next</a>") && page.contains(">previous</a>"));
    }

    #[test]
    fn unicode_and_wide_lines_are_kept() {
        let wide = "ž".repeat(500);
        let r = report_of(&format!("{wide}\n"), "日本語 🎉\n");
        let page = render_html(&r, &ok).unwrap();
        assert!(page.contains(&wide) && page.contains("日本語 🎉"));
        assert!(page.contains("pre-wrap") && page.contains("break-all"));
    }

    #[test]
    fn a_cancelled_export_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a.txt"), d.path().join("b.txt"));
        std::fs::write(&a, "x\n").unwrap();
        std::fs::write(&b, "y\n").unwrap();
        let dest = d.path().join("r.html");
        let pairs = [(DiffSource::File { path: a }, DiffSource::File { path: b })];
        let cancelled = AtomicBool::new(true);
        let err = export_html(&dest, "t", &pairs, &DiffOptions::default(), &cancelled);
        assert!(matches!(err, Err(CoreError::Cancelled)));
        assert!(!dest.exists());
        assert_eq!(
            std::fs::read_dir(d.path()).unwrap().count(),
            2,
            "only the two inputs"
        );
        export_html(
            &dest,
            "t",
            &pairs,
            &DiffOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(std::fs::read_to_string(&dest).unwrap().contains("<table"));
    }

    #[test]
    fn a_cancelled_export_keeps_the_old_report() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a.txt"), d.path().join("b.txt"));
        std::fs::write(&a, "x\n").unwrap();
        std::fs::write(&b, "y\n").unwrap();
        let dest = d.path().join("r.html");
        std::fs::write(&dest, "old report").unwrap();
        let pairs = [(DiffSource::File { path: a }, DiffSource::File { path: b })];
        let cancelled = AtomicBool::new(true);
        assert!(export_html(&dest, "t", &pairs, &DiffOptions::default(), &cancelled).is_err());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "old report");
    }

    #[test]
    fn layout_only_changes_are_context_not_changes() {
        let a: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let b = a.replace("line 5\n", "five\n");
        let mut text = diff_text(&a, &b, DiffOptions::default());
        // As if line 8 differed in layout only.
        let eight = text
            .ops
            .iter()
            .position(|o| o.a <= 8 && 8 < o.a + o.a_len)
            .unwrap();
        let op = text.ops[eight].clone();
        let end = op.a + op.a_len;
        text.ops[eight] = crate::fs::diff::Op {
            tag: "replace",
            a: 8,
            a_len: 1,
            b: 8,
            b_len: 1,
            cosmetic: true,
        };
        text.ops.insert(
            eight + 1,
            crate::fs::diff::Op {
                tag: "equal",
                a: 9,
                a_len: end - 9,
                b: 9,
                b_len: end - 9,
                cosmetic: false,
            },
        );
        let hunks = hunks_of(&text, CONTEXT, &ok).unwrap();
        assert!(
            hunks
                .iter()
                .flat_map(|h| &h.rows)
                .filter(|r| r.kind != RowKind::Context)
                .count()
                == 2
        );
    }

    #[test]
    fn too_many_pairs_are_refused() {
        let p = (
            DiffSource::Text {
                label: "a".into(),
                text: String::new(),
            },
            DiffSource::Text {
                label: "b".into(),
                text: String::new(),
            },
        );
        let pairs = vec![p; MAX_PAIRS + 1];
        assert!(build(
            "t",
            &pairs,
            &DiffOptions::default(),
            &AtomicBool::new(false)
        )
        .is_err());
    }

    #[test]
    fn unreadable_or_binary_pairs_are_listed_with_the_reason() {
        let d = tempfile::tempdir().unwrap();
        let missing = DiffSource::File {
            path: d.path().join("none.txt"),
        };
        let r = build(
            "t",
            &[(missing.clone(), missing)],
            &DiffOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(r.files[0]
            .note
            .as_deref()
            .unwrap()
            .starts_with("Not compared"));
    }
}
