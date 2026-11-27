//! The report of a comparison as a standalone PDF, from the same `Report` model as the HTML page.
//!
//! A small bounded writer, no dependency: A4 pages in the built-in Courier fonts (nothing is embedded or
//! fetched), long lines wrapped, a contents list with page numbers instead of links, a page number at the foot.
//! The built-in fonts hold Latin-1 only: any other character is written as `?` (the HTML report keeps the
//! text as it is), a tab is four spaces, and control characters are replaced. It takes the same `check`
//! callback as the HTML export, so a large export can be cancelled; pages and lines are bounded.

use super::compare_report::{build, write_atomic, Report, RowKind};
use super::diff::{DiffOptions, DiffSource};
use crate::Result;
use std::sync::atomic::{AtomicBool, Ordering};

/// Most pages in one report; a bigger one is refused with the reason (split the set of comparisons).
pub const MAX_PAGES: usize = 5_000;
const PAGE_W: f32 = 595.0;
const PAGE_H: f32 = 842.0;
const MARGIN: f32 = 40.0;
const FONT: f32 = 8.0;
const LEADING: f32 = 10.0;
/// Characters of Courier 8 pt that fit between the margins (every glyph is 0.6 em wide).
const COLUMNS: usize = ((PAGE_W - 2.0 * MARGIN) / (FONT * 0.6)) as usize;
const LINES_PER_PAGE: usize = ((PAGE_H - 2.0 * MARGIN - 20.0) / LEADING) as usize;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Plain,
    Added,
    Removed,
    Heading,
    Muted,
}

struct Line {
    style: Style,
    text: String,
}

/// The text of a line as the Latin-1 bytes of the built-in fonts: other characters become `?`.
fn latin1(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => out.extend_from_slice(b"    "),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push(b'?'),
            c if (c as u32) < 0x100 => out.push(c as u32 as u8),
            _ => out.push(b'?'),
        }
    }
    out
}

/// A string for a PDF content stream: bytes escaped where `(`, `)` or `\` stand.
fn pdf_string(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![b'('];
    for &b in bytes {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
    out
}

/// Pieces of at most `COLUMNS` characters (by characters, not bytes).
fn wrap(text: &str, first: usize, rest: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= first {
        return vec![text.to_string()];
    }
    let mut out = vec![chars[..first].iter().collect::<String>()];
    let mut at = first;
    while at < chars.len() {
        let end = (at + rest).min(chars.len());
        out.push(chars[at..end].iter().collect());
        at = end;
    }
    out
}

fn push_wrapped(lines: &mut Vec<Line>, style: Style, prefix: &str, text: &str) {
    let width = COLUMNS.saturating_sub(prefix.chars().count()).max(10);
    let blank = " ".repeat(prefix.chars().count());
    for (i, piece) in wrap(&text.replace('\t', "    "), width, width)
        .into_iter()
        .enumerate()
    {
        let lead = if i == 0 { prefix } else { &blank };
        lines.push(Line {
            style,
            text: format!("{lead}{piece}"),
        });
    }
}

/// Lays the report out: the body as lines (with the line at which each file starts), then the contents.
fn layout(report: &Report, check: &dyn Fn() -> Result<()>) -> Result<Vec<Vec<Line>>> {
    let mut body: Vec<Line> = Vec::new();
    let mut starts = Vec::new();
    for file in &report.files {
        check()?;
        // Each file starts on a page of its own.
        while !body.len().is_multiple_of(LINES_PER_PAGE) {
            body.push(Line {
                style: Style::Plain,
                text: String::new(),
            });
        }
        starts.push(body.len() / LINES_PER_PAGE);
        push_wrapped(
            &mut body,
            Style::Heading,
            "",
            &format!(
                "{} -> {}  (+{} -{})",
                file.left, file.right, file.added, file.removed
            ),
        );
        if let Some(note) = &file.note {
            push_wrapped(&mut body, Style::Muted, "", note);
        }
        for (n, hunk) in file.hunks.iter().enumerate() {
            check()?;
            body.push(Line {
                style: Style::Muted,
                text: format!("@@ change {} of {} @@", n + 1, file.hunks.len()),
            });
            for row in &hunk.rows {
                let number =
                    |v: Option<usize>| v.map_or("     ".to_string(), |n| format!("{n:>5}"));
                let (style, sign) = match row.kind {
                    RowKind::Context => (Style::Plain, ' '),
                    RowKind::Removed => (Style::Removed, '-'),
                    RowKind::Added => (Style::Added, '+'),
                };
                let prefix = format!("{} {} {sign} ", number(row.left), number(row.right));
                push_wrapped(&mut body, style, &prefix, &row.text);
            }
        }
        if body.len() / LINES_PER_PAGE >= MAX_PAGES {
            return Err(crate::CoreError::Other(format!(
                "the report is longer than {MAX_PAGES} pages; export fewer comparisons"
            )));
        }
    }
    // The contents come first: its pages shift every page number of the body.
    let entries = report.files.len() + 2;
    let contents_pages = entries.div_ceil(LINES_PER_PAGE).max(1);
    let mut contents: Vec<Line> = vec![
        Line {
            style: Style::Heading,
            text: report.title.clone(),
        },
        Line {
            style: Style::Muted,
            text: format!("{} comparisons", report.files.len()),
        },
    ];
    for (i, file) in report.files.iter().enumerate() {
        let label = format!("{}. {} -> {}", i + 1, file.left, file.right);
        let page = format!(" page {}", starts[i] + contents_pages + 1);
        let room = COLUMNS.saturating_sub(page.chars().count());
        let shown: String = if label.chars().count() > room {
            label
                .chars()
                .take(room.saturating_sub(3))
                .collect::<String>()
                + "..."
        } else {
            label
        };
        let fill = room.saturating_sub(shown.chars().count());
        contents.push(Line {
            style: Style::Plain,
            text: format!("{shown}{}{page}", ".".repeat(fill)),
        });
    }
    let mut lines = contents;
    while !lines.len().is_multiple_of(LINES_PER_PAGE) {
        lines.push(Line {
            style: Style::Plain,
            text: String::new(),
        });
    }
    lines.extend(body);
    Ok(lines
        .into_iter()
        .fold(Vec::new(), |mut pages: Vec<Vec<Line>>, line| {
            if pages.last().is_none_or(|p| p.len() >= LINES_PER_PAGE) {
                pages.push(Vec::new());
            }
            pages.last_mut().unwrap().push(line);
            pages
        }))
}

/// The PDF of a report.
pub fn render_pdf(report: &Report, check: &dyn Fn() -> Result<()>) -> Result<Vec<u8>> {
    let pages = layout(report, check)?;
    if pages.len() > MAX_PAGES {
        return Err(crate::CoreError::Other(format!(
            "the report is longer than {MAX_PAGES} pages; export fewer comparisons"
        )));
    }
    // Objects: 1 catalog, 2 pages, 3 Courier, 4 Courier-Bold, then a page and its content for each page.
    let mut objects: Vec<Vec<u8>> = vec![
        Vec::new(),
        Vec::new(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier-Bold /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    ];
    let mut kids = String::new();
    let total = pages.len();
    for (n, page) in pages.iter().enumerate() {
        check()?;
        let mut content: Vec<u8> = Vec::new();
        content.extend_from_slice(
            format!(
                "BT /F1 {FONT} Tf {LEADING} TL {MARGIN} {} Td\n",
                PAGE_H - MARGIN
            )
            .as_bytes(),
        );
        let mut current = Style::Plain;
        content.extend_from_slice(b"0 g /F1 8 Tf\n");
        for line in page {
            if line.style != current {
                let (colour, font) = match line.style {
                    Style::Plain => ("0 g", "/F1"),
                    Style::Added => ("0 0.45 0 rg", "/F1"),
                    Style::Removed => ("0.7 0 0 rg", "/F1"),
                    Style::Heading => ("0 g", "/F2"),
                    Style::Muted => ("0.35 g", "/F1"),
                };
                content.extend_from_slice(format!("{colour} {font} {FONT} Tf\n").as_bytes());
                current = line.style;
            }
            content.extend(pdf_string(&latin1(&line.text)));
            content.extend_from_slice(b" Tj T*\n");
        }
        content.extend_from_slice(b"ET\n");
        // The page number at the foot.
        content.extend_from_slice(
            format!("BT /F1 {FONT} Tf 0.35 g {} 25 Td ", PAGE_W / 2.0 - 20.0).as_bytes(),
        );
        content.extend(pdf_string(&latin1(&format!("Page {} of {total}", n + 1))));
        content.extend_from_slice(b" Tj ET\n");
        let content_id = objects.len() + 2;
        let page_id = objects.len() + 1;
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        );
        let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stream.extend_from_slice(&content);
        stream.extend_from_slice(b"endstream");
        objects.push(stream);
        kids.push_str(&format!("{page_id} 0 R "));
    }
    objects[0] = b"<< /Type /Catalog /Pages 2 0 R >>".to_vec();
    objects[1] = format!("<< /Type /Pages /Kids [{kids}] /Count {total} >>").into_bytes();

    let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    Ok(out)
}

/// Builds the report and writes the PDF to `dest`, atomically like the HTML export (a cancelled or failed export
/// leaves no file).
pub fn export_pdf(
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
    let pdf = render_pdf(&report, &check)?;
    check()?;
    write_atomic(dest, &pdf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::compare_report::{ReportFile, ReportHunk, ReportRow};

    fn ok() -> Result<()> {
        Ok(())
    }

    fn report(rows: Vec<ReportRow>) -> Report {
        Report {
            title: "T (a) \\ b".into(),
            files: vec![ReportFile {
                left: "a.txt".into(),
                right: "b.txt".into(),
                added: 1,
                removed: 1,
                note: None,
                hunks: vec![ReportHunk { rows }],
            }],
        }
    }

    fn row(kind: RowKind, text: &str) -> ReportRow {
        ReportRow {
            kind,
            left: Some(1),
            right: Some(1),
            text: text.into(),
        }
    }

    #[test]
    fn a_pdf_has_the_structure_and_escapes_text() {
        let pdf = render_pdf(
            &report(vec![
                row(RowKind::Removed, "old (x) \\ y"),
                row(RowKind::Added, "new ä é"),
            ]),
            &ok,
        )
        .unwrap();
        let text = String::from_utf8_lossy(&pdf).into_owned();
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.trim_end().ends_with("%%EOF"));
        assert!(text.contains("/Type /Catalog") && text.contains("/Type /Pages"));
        // Parentheses and the backslash are escaped, in the title too.
        assert!(text.contains("old \\(x\\) \\\\ y"));
        assert!(text.contains("T \\(a\\) \\\\ b"));
        // The cross-reference offsets point at their objects.
        let startxref: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(pdf[startxref..].starts_with(b"xref"));
        // Latin-1 letters stay as one byte; this letter is out of the font and is written as ?.
        let with_other = render_pdf(&report(vec![row(RowKind::Added, "ž ✓")]), &ok).unwrap();
        assert!(
            with_other.windows(3).any(|w| w == b"(?)" || w == b"? ?")
                || String::from_utf8_lossy(&with_other).contains("? ?")
        );
    }

    #[test]
    fn long_lines_are_wrapped_and_long_reports_paginate_with_page_numbers_in_the_contents() {
        let long = "x".repeat(COLUMNS * 3);
        let many: Vec<ReportRow> = (0..LINES_PER_PAGE * 3)
            .map(|_| row(RowKind::Context, "line"))
            .collect();
        let pdf = render_pdf(&report(many), &ok).unwrap();
        let text = String::from_utf8_lossy(&pdf).into_owned();
        let pages = text.matches("/Type /Page ").count();
        assert!(pages >= 4, "{pages}");
        assert!(
            text.contains("page 2"),
            "the contents name the page of the file"
        );
        let wrapped = render_pdf(&report(vec![row(RowKind::Added, &long)]), &ok).unwrap();
        assert!(
            String::from_utf8_lossy(&wrapped)
                .matches(&"x".repeat(10))
                .count()
                >= 3
        );
        // No line is wider than the page holds.
        for piece in wrap(&long, COLUMNS, COLUMNS) {
            assert!(piece.chars().count() <= COLUMNS);
        }
    }

    #[test]
    fn a_cancel_ends_the_export_and_leaves_no_file() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("r.pdf");
        let text = |t: &str| DiffSource::Text {
            label: "t".into(),
            text: t.into(),
        };
        let cancelled = AtomicBool::new(true);
        let err = export_pdf(
            &dest,
            "t",
            &[(text("a\n"), text("b\n"))],
            &DiffOptions::default(),
            &cancelled,
        );
        assert!(matches!(err, Err(crate::CoreError::Cancelled)));
        assert!(!dest.exists());
        assert_eq!(
            std::fs::read_dir(d.path()).unwrap().count(),
            0,
            "no temporary file stays"
        );
        export_pdf(
            &dest,
            "t",
            &[(text("a\n"), text("b\n"))],
            &DiffOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(std::fs::read(&dest).unwrap().starts_with(b"%PDF-1.4"));
    }
}
