//! Text and layout-block extraction of documents for comparison (`.docx` and `.pdf`).
//!
//! Each document yields both plain text lines (so the text diff fallback works unchanged) and
//! structured [`DocBlock`] items carrying page/section number, block index, table cell info,
//! heading level, and optional PDF page geometry.
//!
//! Safety bounds:
//! * File size, archive entry count, unpacked XML part size, XML node count, block count, and
//!   total output text are bounded.
//! * PDFs are parsed with `lopdf` (MIT, already in the workspace) using a single O(N) page map
//!   lookup, per-page decompression limits, cumulative font stream budgets across the whole
//!   document, panic-safe decoding, refusal of encrypted/malformed files, and cancellation checks
//!   between pages. The Tauri backend additionally runs PDF extraction in an isolated child
//!   process with OS memory and wall-clock limits that is killed immediately on cancel.
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read};
use std::path::Path;

/// The largest Word document that is opened, and the limits of its parts.
pub const MAX_FILE_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 10_000;
/// The largest uncompressed size of `word/document.xml` (and of the relationships part).
pub const MAX_PART_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_XML_NODES: u32 = 2_000_000;
pub const MAX_BLOCKS: usize = 200_000;
pub const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
const CANCEL_EVERY: usize = 512;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

/// Page-space bounding box (in PDF points, origin at bottom-left).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub page_width: f32,
    pub page_height: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BlockKind {
    Heading,
    Paragraph,
    ListItem,
    TableRow,
    SectionBreak,
}

/// One positioned block in a document, aligned 1:1 with a 1-based line in [`Extracted::text`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocBlock {
    /// 1-based line number in `Extracted.text`.
    pub line: usize,
    /// 1-based page (PDF) or section (DOCX).
    pub page: usize,
    /// 1-based block index within `page` (`0` for page/section separator markers).
    pub block: usize,
    pub kind: BlockKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_level: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_row: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cells: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<BlockGeometry>,
    /// Normalized plain text without markdown syntax prefixes (`#`, `-`, `|`).
    pub text: String,
}

/// What a document gives to a comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extracted {
    /// Document kind: `"docx"` or `"pdf"`.
    pub format: String,
    /// The blocks as lines, joined by `\n`.
    pub text: String,
    pub blocks: usize,
    pub sections: usize,
    /// Positioned blocks matching the lines of `text` in reading order.
    #[serde(default)]
    pub items: Vec<DocBlock>,
    /// What was not read or not shown, in words for the user.
    pub warnings: Vec<String>,
    /// The text was cut at a limit.
    pub truncated: bool,
}

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

fn cancelled(check: &dyn Fn() -> bool) -> Result<()> {
    if check() {
        Err(CoreError::Cancelled)
    } else {
        Ok(())
    }
}

/// The bytes of one part of the archive, at most [`MAX_PART_BYTES`] of them. `None` when the part is not there.
fn read_part(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Option<Vec<u8>>> {
    let file = match archive.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(zip::result::ZipError::UnsupportedArchive(why))
            if why.to_lowercase().contains("password") =>
        {
            return Err(fail("The document is encrypted"));
        }
        Err(zip::result::ZipError::InvalidPassword) => {
            return Err(fail("The document is encrypted"))
        }
        Err(e) => return Err(fail(format!("{name} could not be read: {e}"))),
    };
    if file.size() > MAX_PART_BYTES {
        return Err(fail(format!(
            "{name} is larger than {} MiB when unpacked",
            MAX_PART_BYTES / (1024 * 1024)
        )));
    }
    // The declared size is not trusted: reading stops one byte past the limit.
    let mut bytes = Vec::new();
    file.take(MAX_PART_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| fail(format!("{name} could not be read: {e}")))?;
    if bytes.len() as u64 > MAX_PART_BYTES {
        return Err(fail(format!(
            "{name} is larger than {} MiB when unpacked",
            MAX_PART_BYTES / (1024 * 1024)
        )));
    }
    Ok(Some(bytes))
}

fn parse(xml: &[u8], name: &str) -> Result<String> {
    // The text is kept as an owned string: roxmltree borrows it.
    let text = std::str::from_utf8(xml).map_err(|_| fail(format!("{name} is not UTF-8")))?;
    Ok(text.to_owned())
}

fn options() -> roxmltree::ParsingOptions {
    // No DTD (so no entity can be declared or loaded), and a limit on the nodes.
    roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_XML_NODES,
    }
}

fn is_w(node: &roxmltree::Node, local: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == local
        && node.tag_name().namespace() == Some(W_NS)
}

fn w_attr<'a>(node: &roxmltree::Node<'a, 'a>, local: &str) -> Option<&'a str> {
    node.attribute((W_NS, local))
}

/// The level of a heading from the style or the outline level; `None` for body text.
fn heading_level(p: &roxmltree::Node) -> Option<usize> {
    let props = p.children().find(|c| is_w(c, "pPr"))?;
    if let Some(style) = props
        .children()
        .find(|c| is_w(c, "pStyle"))
        .and_then(|s| w_attr(&s, "val"))
    {
        let lower = style.to_lowercase();
        if lower == "title" {
            return Some(1);
        }
        if let Some(rest) = lower.strip_prefix("heading") {
            let digits: String = rest.trim().chars().filter(char::is_ascii_digit).collect();
            if let Ok(n) = digits.parse::<usize>() {
                return Some(n.clamp(1, 9));
            }
        }
    }
    props
        .children()
        .find(|c| is_w(c, "outlineLvl"))
        .and_then(|l| w_attr(&l, "val"))
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|level| *level < 9)
        .map(|level| level + 1)
}

fn is_list_item(p: &roxmltree::Node) -> bool {
    p.children()
        .find(|c| is_w(c, "pPr"))
        .is_some_and(|props| props.children().any(|c| is_w(&c, "numPr")))
}

fn ends_section(p: &roxmltree::Node) -> bool {
    p.children()
        .find(|c| is_w(c, "pPr"))
        .is_some_and(|props| props.children().any(|c| is_w(&c, "sectPr")))
}

#[derive(Default)]
struct Counters {
    text_boxes: usize,
    deleted_runs: usize,
}

/// The text of a paragraph or another container, in reading order. Deleted text of tracked changes, field codes and
/// text boxes are not part of it (text boxes and deletions are counted for the warnings).
fn collect(node: &roxmltree::Node, out: &mut String, seen: &mut Counters) {
    for child in node.children().filter(roxmltree::Node::is_element) {
        if child.tag_name().namespace() != Some(W_NS) {
            // Pictures, drawing and other namespaces: nothing to read here, but a text box inside is counted.
            collect(&child, out, seen);
            continue;
        }
        match child.tag_name().name() {
            "t" => out.push_str(child.text().unwrap_or("")),
            "tab" => out.push('\t'),
            "br" | "cr" => out.push(' '),
            "noBreakHyphen" => out.push('-'),
            "del" | "moveFrom" => seen.deleted_runs += 1,
            "txbxContent" => seen.text_boxes += 1,
            "pPr" | "rPr" | "instrText" | "delText" | "sectPr" | "tblPr" | "trPr" | "tcPr" => {}
            _ => collect(&child, out, seen),
        }
    }
}

fn clean(text: &str) -> String {
    // One line per block: runs of spaces and line breaks become one space.
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct Output {
    lines: Vec<String>,
    items: Vec<DocBlock>,
    bytes: usize,
    sections: usize,
    block_in_section: usize,
    tables: usize,
    /// A section ended and more content may follow: the separator is written with the next block.
    pending_break: bool,
    truncated: bool,
}

impl Output {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        line: String,
        plain: String,
        kind: BlockKind,
        heading_level: Option<usize>,
        table: Option<usize>,
        table_row: Option<usize>,
        cells: Option<Vec<String>>,
    ) -> bool {
        if self.lines.len() >= MAX_BLOCKS || self.bytes + line.len() + 1 > MAX_TEXT_BYTES {
            self.truncated = true;
            return false;
        }
        if self.pending_break {
            self.pending_break = false;
            self.sections += 1;
            self.block_in_section = 0;
            let marker = format!("--- section {} ---", self.sections);
            self.bytes += marker.len() + 1;
            self.lines.push(marker.clone());
            self.items.push(DocBlock {
                line: self.lines.len(),
                page: self.sections,
                block: 0,
                kind: BlockKind::SectionBreak,
                heading_level: None,
                table: None,
                table_row: None,
                cells: None,
                geometry: None,
                text: marker,
            });
        }
        self.block_in_section += 1;
        self.bytes += line.len() + 1;
        self.lines.push(line);
        self.items.push(DocBlock {
            line: self.lines.len(),
            page: self.sections,
            block: self.block_in_section,
            kind,
            heading_level,
            table,
            table_row,
            cells,
            geometry: None,
            text: plain,
        });
        true
    }
}

/// The text of a table cell: its paragraphs (and those of a nested table) in order, one space apart.
fn cell_text(cell: &roxmltree::Node, seen: &mut Counters) -> String {
    let mut parts: Vec<String> = Vec::new();
    for child in cell.children().filter(roxmltree::Node::is_element) {
        if is_w(&child, "p") {
            let mut text = String::new();
            collect(&child, &mut text, seen);
            parts.push(clean(&text));
        } else if is_w(&child, "tbl") {
            for row in child.children().filter(|r| is_w(r, "tr")) {
                for nested in row.children().filter(|c| is_w(c, "tc")) {
                    parts.push(cell_text(&nested, seen));
                }
            }
        }
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" / ")
}

fn walk_block(
    node: &roxmltree::Node,
    out: &mut Output,
    seen: &mut Counters,
    cancel: &dyn Fn() -> bool,
    count: &mut usize,
) -> Result<bool> {
    *count += 1;
    if (*count).is_multiple_of(CANCEL_EVERY) {
        cancelled(cancel)?;
    }
    if is_w(node, "p") {
        let mut text = String::new();
        collect(node, &mut text, seen);
        let text = clean(&text);
        if !text.is_empty() {
            let level = heading_level(node);
            let (line, kind) = match level {
                Some(lvl) => (format!("{} {text}", "#".repeat(lvl)), BlockKind::Heading),
                None if is_list_item(node) => (format!("- {text}"), BlockKind::ListItem),
                None => (text.clone(), BlockKind::Paragraph),
            };
            if !out.push(line, text, kind, level, None, None, None) {
                return Ok(false);
            }
        }
        if ends_section(node) {
            out.pending_break = true;
        }
    } else if is_w(node, "tbl") {
        let mut table_id: Option<usize> = None;
        let mut row_num = 0usize;
        for row in node.children().filter(|r| is_w(r, "tr")) {
            let cells: Vec<String> = row
                .children()
                .filter(|c| is_w(c, "tc"))
                .map(|c| cell_text(&c, seen))
                .collect();
            if cells.iter().all(String::is_empty) {
                continue;
            }
            let tid = *table_id.get_or_insert_with(|| {
                out.tables += 1;
                out.tables
            });
            row_num += 1;
            let joined = cells.join(" | ");
            let formatted = format!("| {joined} |");
            if !out.push(
                formatted,
                joined,
                BlockKind::TableRow,
                None,
                Some(tid),
                Some(row_num),
                Some(cells),
            ) {
                return Ok(false);
            }
            *count += 1;
            if (*count).is_multiple_of(CANCEL_EVERY) {
                cancelled(cancel)?;
            }
        }
    } else if is_w(node, "sdt") {
        // A content control: its content is part of the flow.
        if let Some(content) = node.children().find(|c| is_w(c, "sdtContent")) {
            for child in content.children().filter(roxmltree::Node::is_element) {
                if !walk_block(&child, out, seen, cancel, count)? {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

/// The text and positioned blocks of a `.docx` for comparison. Fails (with the reason) for what is no Word document:
/// not an archive, an archive without `word/document.xml`, an encrypted file, a part that is too large or not valid XML.
pub fn extract_docx(bytes: &[u8], cancel: &dyn Fn() -> bool) -> Result<Extracted> {
    cancelled(cancel)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(fail(format!(
            "The document is larger than {} MiB",
            MAX_FILE_BYTES / (1024 * 1024)
        )));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| fail("Not a Word document (.docx is a zip archive); an encrypted or old .doc file cannot be read"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(fail(format!(
            "The archive has more than {MAX_ENTRIES} entries"
        )));
    }
    let Some(document) = read_part(&mut archive, "word/document.xml")? else {
        return Err(fail("Not a Word document: word/document.xml is missing"));
    };
    cancelled(cancel)?;
    let mut warnings: Vec<String> = Vec::new();
    // Relationships are counted for the warning and never followed: linked pictures and templates stay unloaded.
    if let Ok(Some(rels)) = read_part(&mut archive, "word/_rels/document.xml.rels") {
        if let Ok(text) = parse(&rels, "word/_rels/document.xml.rels") {
            if let Ok(doc) = roxmltree::Document::parse_with_options(&text, options()) {
                let external = doc
                    .descendants()
                    .filter(|n| {
                        n.is_element()
                            && n.tag_name().name() == "Relationship"
                            && n.tag_name().namespace() == Some(REL_NS)
                    })
                    .filter(|n| {
                        n.attribute("TargetMode")
                            .is_some_and(|m| m.eq_ignore_ascii_case("External"))
                    })
                    .count();
                if external > 0 {
                    warnings.push(format!(
                        "{external} external link(s) or linked resource(s) were not loaded"
                    ));
                }
            }
        }
    }
    let xml = parse(&document, "word/document.xml")?;
    let doc = roxmltree::Document::parse_with_options(&xml, options())
        .map_err(|e| fail(format!("word/document.xml is not valid XML: {e}")))?;
    cancelled(cancel)?;
    let Some(body) = doc.root_element().children().find(|c| is_w(c, "body")) else {
        return Err(fail("Not a Word document: it has no body"));
    };
    let mut out = Output {
        lines: Vec::new(),
        items: Vec::new(),
        bytes: 0,
        sections: 1,
        block_in_section: 0,
        tables: 0,
        pending_break: false,
        truncated: false,
    };
    let mut seen = Counters::default();
    let mut count = 0usize;
    for node in body.children().filter(roxmltree::Node::is_element) {
        if !walk_block(&node, &mut out, &mut seen, cancel, &mut count)? {
            break;
        }
    }
    if out.truncated {
        warnings
            .push("The text is cut at the limit for a document; the rest is not compared".into());
    }
    if seen.text_boxes > 0 {
        warnings.push(format!("{} text box(es) are not compared", seen.text_boxes));
    }
    if seen.deleted_runs > 0 {
        warnings.push(format!(
            "{} tracked deletion(s) are not part of the text",
            seen.deleted_runs
        ));
    }
    if out.lines.is_empty() {
        warnings.push("The document has no text (a picture-only or scanned document)".into());
    }
    Ok(Extracted {
        format: "docx".into(),
        blocks: out.lines.len(),
        sections: out.sections,
        text: out.lines.join("\n"),
        items: out.items,
        warnings,
        truncated: out.truncated,
    })
}

// ───────────────────────── PDF extraction with positions ─────────────────────────

/// Largest PDF read. Smaller than a Word document's limit because `lopdf` parses the cross-reference
/// structure into memory before page iteration (and the backend runs PDF parsing in an isolated child
/// process with memory and wall-clock limits).
pub const MAX_PDF_BYTES: usize = 32 * 1024 * 1024;
/// Most pages of a PDF read; the rest is not compared (and a warning says so).
pub const MAX_PDF_PAGES: usize = 2_000;
/// Most bytes a single page's content stream may decompress to (bounds memory on decompression bombs
/// while accommodating text pages with moderate vector graphics).
pub const MAX_PAGE_DECOMPRESSED: usize = 2 * 1024 * 1024;
/// Most bytes one object stream or cross-reference stream may unpack to while the file is loaded.
pub const MAX_PDF_STREAM: usize = 1024 * 1024;
/// Cumulative byte budget for all font ToUnicode CMap streams decoded across a PDF.
pub const MAX_TOTAL_FONT_BYTES: usize = 16 * 1024 * 1024;
/// Most object streams in a file (they are all unpacked when it is loaded).
pub const MAX_OBJECT_STREAMS: usize = 2_000;
/// Most content-stream operations processed on one page.
const MAX_PAGE_OPERATIONS: usize = 250_000;
/// Internal CLI flag for running PDF extraction in a memory- and time-bounded child process.
pub const PDF_WORKER_ARG: &str = "--internal-extract-pdf";

/// Whether a failure of the PDF reader means that the file is encrypted.
fn is_encryption_message(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("encrypt") || m.contains("password")
}

fn mat_mul(a: [f32; 6], b: [f32; 6]) -> [f32; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn apply_mat(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn as_f32(obj: &lopdf::Object) -> Option<f32> {
    match obj {
        lopdf::Object::Integer(i) => Some(*i as f32),
        lopdf::Object::Real(f) => Some(*f),
        _ => None,
    }
}

fn page_media_box(doc: &lopdf::Document, page_id: lopdf::ObjectId) -> (f32, f32) {
    let mut current = Some(page_id);
    let mut hops = 0;
    while let Some(id) = current {
        hops += 1;
        if hops > 32 {
            break;
        }
        let Ok(dict) = doc.get_dictionary(id) else {
            break;
        };
        if let Ok(arr) = dict
            .get(b"MediaBox")
            .and_then(|o| doc.dereference(o).map(|(_, v)| v))
            .and_then(|o| o.as_array())
        {
            if arr.len() >= 4 {
                if let (Some(x0), Some(y0), Some(x1), Some(y1)) = (
                    as_f32(&arr[0]),
                    as_f32(&arr[1]),
                    as_f32(&arr[2]),
                    as_f32(&arr[3]),
                ) {
                    let w = (x1 - x0).abs();
                    let h = (y1 - y0).abs();
                    if w > 10.0 && h > 10.0 {
                        return (w, h);
                    }
                }
            }
        }
        current = dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
    }
    (612.0, 792.0)
}

/// Checks whether a PDF font dictionary lacks a usable Unicode mapping (e.g. Type0 / Identity-H
/// or Type3 without a `/ToUnicode` stream).
fn font_lacks_unicode_map(doc: &lopdf::Document, font: &lopdf::Dictionary) -> bool {
    if font.get(b"ToUnicode").is_ok() {
        return false;
    }
    let subtype = font
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    if subtype == "Type0" || subtype == "Type3" {
        return true;
    }
    if let Ok(enc_obj) = font.get(b"Encoding") {
        if let Ok((_, lopdf::Object::Name(name))) = doc.dereference(enc_obj) {
            if name == b"Identity-H" || name == b"Identity-V" {
                return true;
            }
        }
    }
    false
}

/// Heuristic check for decoded text that contains replacement glyphs or excessive control / PUA
/// code points indicative of a missing or broken ToUnicode map.
fn looks_garbled(text: &str) -> bool {
    let mut total = 0usize;
    let mut bad = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        total += 1;
        let cp = ch as u32;
        if ch == '\u{FFFD}'
            || (0xE000..=0xF8FF).contains(&cp)
            || (cp < 0x20 && !matches!(ch, '\t' | '\n' | '\r'))
        {
            bad += 1;
        }
    }
    total > 0 && (bad * 5 >= total || bad >= 3)
}

#[derive(Debug, Clone)]
struct TextRun {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    font_size: f32,
    text: String,
}

#[derive(Debug, Clone)]
struct VisualSpan {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    font_size: f32,
    text: String,
}

#[derive(Debug, Clone)]
struct VisualBand {
    y: f32,
    height: f32,
    spans: Vec<VisualSpan>,
}

#[derive(Debug, Clone)]
struct PdfBlockDraft {
    kind: BlockKind,
    heading_level: Option<usize>,
    table: Option<usize>,
    table_row: Option<usize>,
    cells: Option<Vec<String>>,
    geometry: BlockGeometry,
    line: String,
    plain: String,
}

struct PageExtract {
    runs: Vec<TextRun>,
    has_table_rules: bool,
    missing_unicode_font: bool,
    garbled_text: bool,
    page_width: f32,
    page_height: f32,
}

struct FontCache<'a> {
    encodings: HashMap<usize, lopdf::Encoding<'a>>,
    lacks_unicode: HashMap<usize, bool>,
    remaining_budget: usize,
    budget_exceeded: bool,
}

impl<'a> FontCache<'a> {
    fn new() -> Self {
        Self {
            encodings: HashMap::new(),
            lacks_unicode: HashMap::new(),
            remaining_budget: MAX_TOTAL_FONT_BYTES,
            budget_exceeded: false,
        }
    }

    fn resolve(
        &mut self,
        doc: &'a lopdf::Document,
        font: &'a lopdf::Dictionary,
    ) -> (Option<&lopdf::Encoding<'a>>, bool) {
        let key = font as *const lopdf::Dictionary as usize;
        if let std::collections::hash_map::Entry::Vacant(entry) = self.lacks_unicode.entry(key) {
            let lacks = font_lacks_unicode_map(doc, font);
            entry.insert(lacks);
            let stream_hint = font
                .get(b"ToUnicode")
                .ok()
                .and_then(|o| doc.dereference(o).ok())
                .and_then(|(_, o)| o.as_stream().ok())
                .map(|s| s.content.len().saturating_mul(4).max(4096))
                .unwrap_or(0);
            if self.remaining_budget == 0 {
                self.budget_exceeded = true;
            } else {
                let per_font_limit = self.remaining_budget.min(MAX_PAGE_DECOMPRESSED);
                match font.get_font_encoding_with_limit(doc, per_font_limit) {
                    Ok(enc) => {
                        self.remaining_budget = self.remaining_budget.saturating_sub(stream_hint);
                        self.encodings.insert(key, enc);
                    }
                    Err(_) => {
                        self.budget_exceeded = true;
                    }
                }
            }
        }
        let lacks = *self.lacks_unicode.get(&key).unwrap_or(&false);
        (self.encodings.get(&key), lacks)
    }
}

fn extract_page_runs<'a>(
    doc: &'a lopdf::Document,
    page_id: lopdf::ObjectId,
    font_cache: &mut FontCache<'a>,
) -> std::result::Result<PageExtract, String> {
    let (page_width, page_height) = page_media_box(doc, page_id);
    let content_data = doc
        .get_page_content_with_limit(page_id, MAX_PAGE_DECOMPRESSED)
        .map_err(|e| e.to_string())?;
    let content = lopdf::content::Content::decode(&content_data).map_err(|e| e.to_string())?;
    if content.operations.len() > MAX_PAGE_OPERATIONS {
        return Err("page has too many operations".into());
    }

    let fonts = doc.get_page_fonts(page_id).unwrap_or_default();
    let mut font_keys: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for (name, dict) in &fonts {
        font_cache.resolve(doc, dict);
        font_keys.insert(name.clone(), *dict as *const lopdf::Dictionary as usize);
    }

    let mut runs: Vec<TextRun> = Vec::new();
    let mut ctm: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut ctm_stack: Vec<[f32; 6]> = Vec::new();
    let mut tm: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut tlm: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut leading: f32 = 0.0;
    let mut font_size: f32 = 12.0;
    let mut current_font_key: Option<usize> = None;
    let mut missing_unicode_font = false;
    let mut garbled_text = false;
    let mut rule_strokes = 0usize;

    let emit_bytes = |bytes: &[u8],
                      tm: &mut [f32; 6],
                      ctm: [f32; 6],
                      font_size: f32,
                      current_font_key: Option<usize>,
                      runs: &mut Vec<TextRun>,
                      missing_unicode_font: &mut bool,
                      garbled_text: &mut bool| {
        if bytes.is_empty() {
            return;
        }
        let decoded = if let Some(fk) = current_font_key {
            if *font_cache.lacks_unicode.get(&fk).unwrap_or(&false) {
                *missing_unicode_font = true;
            }
            if let Some(enc) = font_cache.encodings.get(&fk) {
                lopdf::Document::decode_text(enc, bytes)
                    .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned())
            } else {
                *missing_unicode_font = true;
                String::from_utf8_lossy(bytes).into_owned()
            }
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        };
        if looks_garbled(&decoded) {
            *garbled_text = true;
        }
        let scale_y = (tm[3].hypot(tm[1]) * ctm[3].hypot(ctm[1])).max(0.01);
        let eff_size = (font_size * scale_y).clamp(1.0, 200.0);
        let char_count = decoded.chars().count().max(1) as f32;
        let advance_text = char_count * font_size * 0.52;
        let (x, y) = apply_mat(ctm, tm[4], tm[5]);
        let (x_end, _) = apply_mat(ctm, tm[4] + advance_text * tm[0].max(0.1), tm[5]);
        let width = (x_end - x).abs().max(eff_size * 0.4);
        tm[4] += advance_text * tm[0];
        tm[5] += advance_text * tm[1];
        if !decoded.trim().is_empty() {
            runs.push(TextRun {
                x,
                y,
                width,
                height: eff_size,
                font_size: eff_size,
                text: decoded,
            });
        }
    };

    for op in &content.operations {
        match op.operator.as_str() {
            "q" => {
                if ctm_stack.len() < 64 {
                    ctm_stack.push(ctm);
                }
            }
            "Q" => {
                if let Some(prev) = ctm_stack.pop() {
                    ctm = prev;
                }
            }
            "cm" if op.operands.len() >= 6 => {
                if let (Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)) = (
                    as_f32(&op.operands[0]),
                    as_f32(&op.operands[1]),
                    as_f32(&op.operands[2]),
                    as_f32(&op.operands[3]),
                    as_f32(&op.operands[4]),
                    as_f32(&op.operands[5]),
                ) {
                    ctm = mat_mul([a, b, c, d, e, f], ctm);
                }
            }
            "re" | "l" => {
                rule_strokes += 1;
            }
            "BT" => {
                tm = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                tlm = tm;
            }
            "ET" => {}
            "Tf" if op.operands.len() >= 2 => {
                if let Ok(name) = op.operands[0].as_name() {
                    current_font_key = font_keys.get(name).copied();
                }
                if let Some(sz) = as_f32(&op.operands[1]) {
                    if sz.abs() > 0.1 {
                        font_size = sz.abs();
                    }
                }
            }
            "TL" if !op.operands.is_empty() => {
                if let Some(l) = as_f32(&op.operands[0]) {
                    leading = l;
                }
            }
            "Td" if op.operands.len() >= 2 => {
                if let (Some(tx), Some(ty)) = (as_f32(&op.operands[0]), as_f32(&op.operands[1])) {
                    tlm = mat_mul([1.0, 0.0, 0.0, 1.0, tx, ty], tlm);
                    tm = tlm;
                }
            }
            "TD" if op.operands.len() >= 2 => {
                if let (Some(tx), Some(ty)) = (as_f32(&op.operands[0]), as_f32(&op.operands[1])) {
                    leading = -ty;
                    tlm = mat_mul([1.0, 0.0, 0.0, 1.0, tx, ty], tlm);
                    tm = tlm;
                }
            }
            "Tm" if op.operands.len() >= 6 => {
                if let (Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)) = (
                    as_f32(&op.operands[0]),
                    as_f32(&op.operands[1]),
                    as_f32(&op.operands[2]),
                    as_f32(&op.operands[3]),
                    as_f32(&op.operands[4]),
                    as_f32(&op.operands[5]),
                ) {
                    tm = [a, b, c, d, e, f];
                    tlm = tm;
                }
            }
            "T*" => {
                tlm = mat_mul([1.0, 0.0, 0.0, 1.0, 0.0, -leading], tlm);
                tm = tlm;
            }
            "Tj" => {
                if let Some(lopdf::Object::String(bytes, _)) = op.operands.first() {
                    emit_bytes(
                        bytes,
                        &mut tm,
                        ctm,
                        font_size,
                        current_font_key,
                        &mut runs,
                        &mut missing_unicode_font,
                        &mut garbled_text,
                    );
                }
            }
            "'" => {
                tlm = mat_mul([1.0, 0.0, 0.0, 1.0, 0.0, -leading], tlm);
                tm = tlm;
                if let Some(lopdf::Object::String(bytes, _)) = op.operands.first() {
                    emit_bytes(
                        bytes,
                        &mut tm,
                        ctm,
                        font_size,
                        current_font_key,
                        &mut runs,
                        &mut missing_unicode_font,
                        &mut garbled_text,
                    );
                }
            }
            "\"" => {
                tlm = mat_mul([1.0, 0.0, 0.0, 1.0, 0.0, -leading], tlm);
                tm = tlm;
                if let Some(lopdf::Object::String(bytes, _)) = op.operands.get(2) {
                    emit_bytes(
                        bytes,
                        &mut tm,
                        ctm,
                        font_size,
                        current_font_key,
                        &mut runs,
                        &mut missing_unicode_font,
                        &mut garbled_text,
                    );
                }
            }
            "TJ" => {
                if let Some(lopdf::Object::Array(items)) = op.operands.first() {
                    let mut buf: Vec<u8> = Vec::new();
                    let flush = |buf: &mut Vec<u8>,
                                 tm: &mut [f32; 6],
                                 runs: &mut Vec<TextRun>,
                                 missing_unicode_font: &mut bool,
                                 garbled_text: &mut bool| {
                        if !buf.is_empty() {
                            emit_bytes(
                                buf,
                                tm,
                                ctm,
                                font_size,
                                current_font_key,
                                runs,
                                missing_unicode_font,
                                garbled_text,
                            );
                            buf.clear();
                        }
                    };
                    for item in items {
                        match item {
                            lopdf::Object::String(bytes, _) => buf.extend_from_slice(bytes),
                            other => {
                                if let Some(kern) = as_f32(other) {
                                    // Large horizontal jump in a TJ array (e.g. tabular spacing) splits into a new run.
                                    if kern <= -1500.0 {
                                        flush(
                                            &mut buf,
                                            &mut tm,
                                            &mut runs,
                                            &mut missing_unicode_font,
                                            &mut garbled_text,
                                        );
                                        let shift = (-kern / 1000.0) * font_size;
                                        tm[4] += shift * tm[0];
                                        tm[5] += shift * tm[1];
                                    } else if kern <= -180.0 && !buf.ends_with(b" ") {
                                        buf.push(b' ');
                                    }
                                }
                            }
                        }
                    }
                    flush(
                        &mut buf,
                        &mut tm,
                        &mut runs,
                        &mut missing_unicode_font,
                        &mut garbled_text,
                    );
                }
            }
            _ => {}
        }
    }

    Ok(PageExtract {
        runs,
        has_table_rules: rule_strokes >= 4,
        missing_unicode_font,
        garbled_text,
        page_width,
        page_height,
    })
}

/// Groups raw positioned PDF text runs into baseline bands and horizontal spans.
fn build_visual_bands(mut runs: Vec<TextRun>) -> Vec<VisualBand> {
    if runs.is_empty() {
        return Vec::new();
    }
    runs.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut bands: Vec<VisualBand> = Vec::new();
    for run in runs {
        let matched = bands
            .iter_mut()
            .find(|b| (b.y - run.y).abs() <= b.height.max(run.height) * 0.45);
        if let Some(band) = matched {
            band.height = band.height.max(run.height);
            band.spans.push(VisualSpan {
                x: run.x,
                y: run.y,
                width: run.width,
                height: run.height,
                font_size: run.font_size,
                text: run.text,
            });
        } else {
            bands.push(VisualBand {
                y: run.y,
                height: run.height,
                spans: vec![VisualSpan {
                    x: run.x,
                    y: run.y,
                    width: run.width,
                    height: run.height,
                    font_size: run.font_size,
                    text: run.text,
                }],
            });
        }
    }
    // Sort bands top-to-bottom and merge adjacent runs on the same baseline into spans.
    bands.sort_by(|a, b| b.y.partial_cmp(&a.y).unwrap_or(std::cmp::Ordering::Equal));
    for band in &mut bands {
        band.spans
            .sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        let mut merged: Vec<VisualSpan> = Vec::new();
        for span in std::mem::take(&mut band.spans) {
            if let Some(prev) = merged.last_mut() {
                let gap = span.x - (prev.x + prev.width);
                let merge_limit = (prev.font_size.max(span.font_size) * 1.4).max(12.0);
                if gap <= merge_limit {
                    if gap > prev.font_size * 0.18
                        && !prev.text.ends_with(' ')
                        && !span.text.starts_with(' ')
                    {
                        prev.text.push(' ');
                    }
                    prev.text.push_str(&span.text);
                    let right = (prev.x + prev.width).max(span.x + span.width);
                    prev.width = right - prev.x;
                    prev.height = prev.height.max(span.height);
                    prev.font_size = prev.font_size.max(span.font_size);
                    continue;
                }
            }
            merged.push(span);
        }
        for s in &mut merged {
            s.text = clean(&s.text);
        }
        merged.retain(|s| !s.text.is_empty());
        band.spans = merged;
    }
    bands.retain(|b| !b.spans.is_empty());
    bands
}

/// Finds a vertical column gutter across a slice of bands (if at least 2 left spans and 2 right
/// spans sit cleanly on opposite sides of a vertical cut near the middle of the page).
fn find_column_gutter(bands: &[VisualBand], page_width: f32) -> Option<f32> {
    if bands.len() < 2 || page_width < 100.0 {
        return None;
    }
    let min_x = page_width * 0.25;
    let max_x = page_width * 0.75;
    let mut best: Option<(f32, usize, f32)> = None;
    // Candidate cut positions at 12-pt intervals across the middle half of the page.
    let mut cut = min_x;
    while cut <= max_x {
        let mut left_count = 0usize;
        let mut right_count = 0usize;
        let mut crossing = 0usize;
        let mut min_clearance = f32::INFINITY;
        for band in bands {
            for span in &band.spans {
                let right = span.x + span.width;
                if right <= cut + 2.0 {
                    left_count += 1;
                    min_clearance = min_clearance.min((cut - right).max(0.0));
                } else if span.x >= cut - 2.0 {
                    right_count += 1;
                    min_clearance = min_clearance.min((span.x - cut).max(0.0));
                } else {
                    crossing += 1;
                }
            }
        }
        if crossing == 0 && left_count >= 2 && right_count >= 2 {
            let balance = left_count.min(right_count);
            let better = match best {
                None => true,
                Some((_, b_bal, b_clr)) => {
                    balance > b_bal || (balance == b_bal && min_clearance > b_clr)
                }
            };
            if better {
                best = Some((cut, balance, min_clearance));
            }
        }
        cut += 12.0;
    }
    best.map(|(x, _, _)| x)
}

/// Returns true if a contiguous run of bands forms a table:
/// either >= 3 columns across >= 2 rows, or >= 2 columns with explicit drawn table rules.
fn is_table_region(bands: &[VisualBand], has_table_rules: bool) -> bool {
    if bands.len() < 2 {
        return false;
    }
    let multi_3 = bands.iter().filter(|b| b.spans.len() >= 3).count();
    if multi_3 >= 2 {
        return true;
    }
    if has_table_rules && bands.iter().filter(|b| b.spans.len() >= 2).count() >= 2 {
        return true;
    }
    false
}

fn median_font_size(bands: &[VisualBand]) -> f32 {
    let mut sizes: Vec<f32> = bands
        .iter()
        .flat_map(|b| b.spans.iter().map(|s| s.font_size))
        .collect();
    if sizes.is_empty() {
        return 12.0;
    }
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sizes[sizes.len() / 2]
}

fn span_to_block(
    span: &VisualSpan,
    body_font: f32,
    page_width: f32,
    page_height: f32,
) -> PdfBlockDraft {
    let ratio = span.font_size / body_font.max(1.0);
    let heading_level = if span.text.len() <= 140 && ratio >= 1.25 {
        Some(if ratio >= 1.6 {
            1
        } else if ratio >= 1.35 {
            2
        } else {
            3
        })
    } else {
        None
    };
    let (kind, line) = match heading_level {
        Some(lvl) => (
            BlockKind::Heading,
            format!("{} {}", "#".repeat(lvl), span.text),
        ),
        None => (BlockKind::Paragraph, span.text.clone()),
    };
    PdfBlockDraft {
        kind,
        heading_level,
        table: None,
        table_row: None,
        cells: None,
        geometry: BlockGeometry {
            x: (span.x * 10.0).round() / 10.0,
            y: (span.y * 10.0).round() / 10.0,
            width: (span.width * 10.0).round() / 10.0,
            height: (span.height * 10.0).round() / 10.0,
            page_width,
            page_height,
        },
        line,
        plain: span.text.clone(),
    }
}

/// Orders a PDF page's visual bands into reading order:
/// * Full-width headings/footers stay in top-to-bottom position around multi-column regions.
/// * Tables emit `BlockKind::TableRow` with aligned cells.
/// * Multi-column regions read left column top-to-bottom, then right column top-to-bottom.
fn organize_pdf_page(
    extract: PageExtract,
    body_font: f32,
    table_counter: &mut usize,
) -> Vec<PdfBlockDraft> {
    let PageExtract {
        runs,
        has_table_rules,
        page_width,
        page_height,
        ..
    } = extract;
    let bands = build_visual_bands(runs);
    if bands.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();

    // Split page vertically whenever a single full-width span crosses the center so top titles
    // and bottom footers do not block two-column detection on the body between them.
    let mid = page_width * 0.5;
    let is_full_width = |band: &VisualBand| {
        band.spans.len() == 1 && band.spans[0].x < mid - 12.0 && {
            let s = &band.spans[0];
            s.x + s.width > mid + 12.0 || s.width > page_width * 0.55
        }
    };

    let mut idx = 0;
    while idx < bands.len() {
        if is_full_width(&bands[idx]) {
            out.push(span_to_block(
                &bands[idx].spans[0],
                body_font,
                page_width,
                page_height,
            ));
            idx += 1;
            continue;
        }
        let start = idx;
        while idx < bands.len() && !is_full_width(&bands[idx]) {
            idx += 1;
        }
        let region = &bands[start..idx];

        if is_table_region(region, has_table_rules) {
            *table_counter += 1;
            let tid = *table_counter;
            let mut row_num = 0usize;
            for band in region {
                if band.spans.len() >= 2 {
                    row_num += 1;
                    let cells: Vec<String> = band.spans.iter().map(|s| s.text.clone()).collect();
                    let joined = cells.join(" | ");
                    let x0 = band.spans.iter().map(|s| s.x).fold(f32::INFINITY, f32::min);
                    let x1 = band
                        .spans
                        .iter()
                        .map(|s| s.x + s.width)
                        .fold(0.0f32, f32::max);
                    out.push(PdfBlockDraft {
                        kind: BlockKind::TableRow,
                        heading_level: None,
                        table: Some(tid),
                        table_row: Some(row_num),
                        cells: Some(cells),
                        geometry: BlockGeometry {
                            x: (x0 * 10.0).round() / 10.0,
                            y: (band.y * 10.0).round() / 10.0,
                            width: ((x1 - x0).max(1.0) * 10.0).round() / 10.0,
                            height: (band.height * 10.0).round() / 10.0,
                            page_width,
                            page_height,
                        },
                        line: format!("| {joined} |"),
                        plain: joined,
                    });
                } else {
                    out.push(span_to_block(
                        &band.spans[0],
                        body_font,
                        page_width,
                        page_height,
                    ));
                }
            }
        } else if let Some(split_x) = find_column_gutter(region, page_width) {
            // Column reading order: all left-column spans top-to-bottom, then right-column top-to-bottom.
            let mut left_spans: Vec<&VisualSpan> = Vec::new();
            let mut right_spans: Vec<&VisualSpan> = Vec::new();
            for band in region {
                for span in &band.spans {
                    if span.x + span.width <= split_x + 2.0 {
                        left_spans.push(span);
                    } else {
                        right_spans.push(span);
                    }
                }
            }
            for span in left_spans.into_iter().chain(right_spans) {
                out.push(span_to_block(span, body_font, page_width, page_height));
            }
        } else {
            for band in region {
                if band.spans.len() == 1 {
                    out.push(span_to_block(
                        &band.spans[0],
                        body_font,
                        page_width,
                        page_height,
                    ));
                } else {
                    let combined = band
                        .spans
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    let x0 = band.spans.iter().map(|s| s.x).fold(f32::INFINITY, f32::min);
                    let x1 = band
                        .spans
                        .iter()
                        .map(|s| s.x + s.width)
                        .fold(0.0f32, f32::max);
                    let max_font = band
                        .spans
                        .iter()
                        .map(|s| s.font_size)
                        .fold(0.0f32, f32::max);
                    let merged = VisualSpan {
                        x: x0,
                        y: band.y,
                        width: (x1 - x0).max(1.0),
                        height: band.height,
                        font_size: max_font,
                        text: combined,
                    };
                    out.push(span_to_block(&merged, body_font, page_width, page_height));
                }
            }
        }
    }
    out
}

/// Text and positioned layout blocks of a PDF, page by page in reading order.
///
/// Uses `lopdf` with O(N) page lookup, bounded decompression per page, cumulative font stream
/// budget across the document, panic-safe decoding, refusal of encrypted/malformed files,
/// multi-column & table reading order, missing-Unicode-map warnings, and cancellation checks
/// between pages.
pub fn extract_pdf(bytes: &[u8], cancel: &dyn Fn() -> bool) -> Result<Extracted> {
    cancelled(cancel)?;
    if bytes.len() > MAX_PDF_BYTES {
        return Err(fail(format!(
            "The PDF is larger than {} MiB, the most that is read for a comparison",
            MAX_PDF_BYTES / (1024 * 1024)
        )));
    }
    if !bytes[..bytes.len().min(1024)]
        .windows(5)
        .any(|w| w == b"%PDF-")
    {
        return Err(fail("Not a PDF document (it does not start with %PDF-)"));
    }
    // Every object stream is unpacked when the file is loaded: a file with very many is refused before that.
    if bytes.windows(7).filter(|w| *w == b"/ObjStm").count() > MAX_OBJECT_STREAMS {
        return Err(fail(
            "The PDF has too many object streams to be read safely",
        ));
    }
    let options = lopdf::LoadOptions {
        max_decompressed_size: Some(MAX_PDF_STREAM),
        ..Default::default()
    };
    let parsed =
        std::panic::catch_unwind(|| lopdf::Document::load_mem_with_options(bytes, options));
    let document = match parsed {
        Ok(Ok(document)) => document,
        Ok(Err(e)) => {
            let message = e.to_string();
            return Err(fail(if is_encryption_message(&message) {
                "The document is encrypted".to_string()
            } else {
                format!("The PDF could not be read: {message}")
            }));
        }
        Err(_) => return Err(fail("The PDF could not be read: it is malformed")),
    };
    // `is_encrypted` only follows a reference; a dictionary written straight into the trailer is encryption as well.
    if document.is_encrypted() || document.trailer.get(b"Encrypt").is_ok() {
        return Err(fail("The document is encrypted"));
    }
    cancelled(cancel)?;

    let has_acroform = document
        .catalog()
        .ok()
        .is_some_and(|cat| cat.get(b"AcroForm").is_ok());

    // Build page map ONCE (O(N), fixing the previous quadratic per-page page-tree walk).
    let pages = document.get_pages();
    let mut warnings: Vec<String> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut items: Vec<DocBlock> = Vec::new();
    let mut bytes_out = 0usize;
    let mut font_cache = FontCache::new();
    let mut table_counter = 0usize;
    let mut any_missing_unicode = false;
    let mut any_garbled = false;
    let (mut truncated, mut unreadable, mut with_text, mut blocks, mut read) =
        (false, Vec::new(), 0usize, 0usize, 0usize);

    for (shown, (_page_num, page_id)) in pages.iter().enumerate() {
        if shown >= MAX_PDF_PAGES {
            warnings.push(format!(
                "Only the first {MAX_PDF_PAGES} pages are compared; the rest is not"
            ));
            truncated = true;
            break;
        }
        cancelled(cancel)?;
        let page_index = shown + 1;
        read += 1;
        let marker = format!("--- page {page_index} ---");
        bytes_out += marker.len() + 1;
        lines.push(marker.clone());
        items.push(DocBlock {
            line: lines.len(),
            page: page_index,
            block: 0,
            kind: BlockKind::SectionBreak,
            heading_level: None,
            table: None,
            table_row: None,
            cells: None,
            geometry: None,
            text: marker,
        });

        let extracted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            extract_page_runs(&document, *page_id, &mut font_cache)
        }));
        match extracted {
            Ok(Ok(page_runs)) => {
                any_missing_unicode |= page_runs.missing_unicode_font;
                any_garbled |= page_runs.garbled_text;
                let preview_bands = build_visual_bands(page_runs.runs.clone());
                let body_font = median_font_size(&preview_bands);
                let drafts = organize_pdf_page(page_runs, body_font, &mut table_counter);
                let mut any = false;
                let mut block_in_page = 0usize;
                for draft in drafts {
                    if draft.line.is_empty() {
                        continue;
                    }
                    any = true;
                    bytes_out += draft.line.len() + 1;
                    if bytes_out > MAX_TEXT_BYTES || blocks >= MAX_BLOCKS {
                        truncated = true;
                        break;
                    }
                    blocks += 1;
                    block_in_page += 1;
                    lines.push(draft.line);
                    items.push(DocBlock {
                        line: lines.len(),
                        page: page_index,
                        block: block_in_page,
                        kind: draft.kind,
                        heading_level: draft.heading_level,
                        table: draft.table,
                        table_row: draft.table_row,
                        cells: draft.cells,
                        geometry: Some(draft.geometry),
                        text: draft.plain,
                    });
                }
                with_text += usize::from(any);
            }
            // A page that cannot be read (a damaged stream, a bomb over the limit) is told, not hidden.
            _ => unreadable.push(page_index),
        }
        if truncated {
            warnings.push(
                "The text is cut at the limit for a document; the rest is not compared".into(),
            );
            break;
        }
    }

    if !unreadable.is_empty() {
        let list: Vec<String> = unreadable.iter().take(10).map(|p| p.to_string()).collect();
        warnings.push(format!(
            "{} page(s) could not be read ({}{}) and are empty here",
            unreadable.len(),
            list.join(", "),
            if unreadable.len() > 10 { ", …" } else { "" }
        ));
    }
    if font_cache.budget_exceeded {
        warnings.push(
            "Font encoding tables exceeded the memory budget; some text may be missing".into(),
        );
    }
    if any_missing_unicode || any_garbled {
        warnings.push(
            "Some fonts have no Unicode map; extracted text may be incomplete or garbled".into(),
        );
    }
    if has_acroform {
        warnings.push(
            "The PDF contains form fields (AcroForm); interactive form values may not appear in page text".into(),
        );
    }
    // Pages that could not be read are no evidence of a scan: that warning is for pages that were read and are empty.
    if with_text == 0 && unreadable.len() < read {
        warnings.push("The document has no text (a picture-only or scanned document)".into());
    }
    Ok(Extracted {
        format: "pdf".into(),
        blocks,
        sections: pages.len().clamp(1, MAX_PDF_PAGES),
        text: lines.join("\n"),
        items,
        warnings,
        truncated,
    })
}

/// Applies OS-level memory and CPU bounds for the isolated PDF extraction child process.
#[cfg(target_os = "linux")]
fn apply_pdf_worker_limits() {
    // Allow at most +512 MiB above current mapped address space (shared libraries mapped at exec)
    // and cap CPU time to 35 seconds so a crafted PDF cannot exhaust host memory or spin forever.
    const EXTRA_AS_BYTES: u64 = 512 * 1024 * 1024;
    let current_vsz = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<u64>().ok())
        .map(|pages| pages.saturating_mul(unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 }))
        .unwrap_or(512 * 1024 * 1024);
    let as_limit = current_vsz.saturating_add(EXTRA_AS_BYTES);
    let rlim_as = libc::rlimit {
        rlim_cur: as_limit,
        rlim_max: as_limit,
    };
    unsafe {
        let _ = libc::setrlimit(libc::RLIMIT_AS, &rlim_as);
    }
    let rlim_cpu = libc::rlimit {
        rlim_cur: 35,
        rlim_max: 35,
    };
    unsafe {
        let _ = libc::setrlimit(libc::RLIMIT_CPU, &rlim_cpu);
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_pdf_worker_limits() {}

/// Entrypoint for `--internal-extract-pdf <path>`: runs PDF extraction in a bounded child process
/// and writes a JSON-encoded `Result<Extracted, String>` to stdout.
pub fn handle_pdf_worker_arg(raw_args: &[std::ffi::OsString]) -> Option<i32> {
    if raw_args.len() != 3 || raw_args[1].to_str() != Some(PDF_WORKER_ARG) {
        return None;
    }
    apply_pdf_worker_limits();
    let path = Path::new(&raw_args[2]);
    let result: std::result::Result<Extracted, String> = (|| {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take((MAX_PDF_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        extract_pdf(&bytes, &|| false).map_err(|e| e.to_string())
    })();
    match serde_json::to_writer(std::io::stdout().lock(), &result) {
        Ok(()) => Some(0),
        Err(e) => {
            eprintln!("pdf worker encode failed: {e}");
            Some(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const NEVER: &dyn Fn() -> bool = &|| false;

    fn docx(body: &str, rels: Option<&str>) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="{W_NS}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{body}</w:body></w:document>"#).as_bytes(),
        )
        .unwrap();
        if let Some(rels) = rels {
            zip.start_file("word/_rels/document.xml.rels", options)
                .unwrap();
            zip.write_all(
                format!(
                    r#"<?xml version="1.0"?><Relationships xmlns="{REL_NS}">{rels}</Relationships>"#
                )
                .as_bytes(),
            )
            .unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn p(text: &str) -> String {
        format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
    }
    fn styled(style: &str, text: &str) -> String {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
        )
    }
    fn text_of(body: &str) -> Extracted {
        extract_docx(&docx(body, None), NEVER).unwrap()
    }

    #[test]
    fn headings_paragraphs_lists_and_tables_become_readable_lines_in_order() {
        let body = format!(
            "{}{}{}{}<w:tbl><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>{}",
            styled("Heading1", "Intro"),
            p("First paragraph."),
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>an item</w:t></w:r></w:p>"#,
            styled("Heading2", "Detail"),
            p("a1"), p("b1"), p("a2"), p("b2"),
            p("Last."),
        );
        let e = text_of(&body);
        assert_eq!(
            e.text,
            "# Intro\nFirst paragraph.\n- an item\n## Detail\n| a1 | b1 |\n| a2 | b2 |\nLast."
        );
        assert_eq!((e.blocks, e.sections), (7, 1));
        assert_eq!(e.items.len(), 7);
        assert_eq!(e.items[0].kind, BlockKind::Heading);
        assert_eq!(e.items[0].heading_level, Some(1));
        assert_eq!(
            (e.items[0].page, e.items[0].block, e.items[0].line),
            (1, 1, 1)
        );
        assert_eq!(e.items[4].kind, BlockKind::TableRow);
        assert_eq!((e.items[4].table, e.items[4].table_row), (Some(1), Some(1)));
        assert_eq!(
            e.items[4].cells.as_deref(),
            Some(&["a1".to_string(), "b1".to_string()][..])
        );
        assert!(e.warnings.is_empty() && !e.truncated);
    }

    #[test]
    fn runs_tabs_breaks_and_whitespace_are_joined_into_one_line_per_paragraph() {
        let body = r#"<w:p><w:r><w:t>Hel</w:t></w:r><w:r><w:t xml:space="preserve">lo </w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>wor</w:t></w:r><w:hyperlink><w:r><w:t>ld</w:t></w:r></w:hyperlink><w:r><w:br/></w:r><w:r><w:t>  end</w:t></w:r></w:p>"#;
        assert_eq!(text_of(body).text, "Hello world end");
        // Headings by outline level and by a Title style; the level is clamped.
        let outline =
            r#"<w:p><w:pPr><w:outlineLvl w:val="2"/></w:pPr><w:r><w:t>Deep</w:t></w:r></w:p>"#;
        assert_eq!(text_of(outline).text, "### Deep");
        assert_eq!(text_of(&styled("Title", "Name")).text, "# Name");
        assert_eq!(
            text_of(&styled("heading 12", "Huge")).text,
            "######### Huge"
        );
        assert_eq!(text_of(&styled("Normal", "Body")).text, "Body");
    }

    #[test]
    fn sections_are_numbered_and_a_trailing_section_mark_adds_nothing() {
        let end = r#"<w:p><w:pPr><w:sectPr/></w:pPr><w:r><w:t>end of one</w:t></w:r></w:p>"#;
        let e = text_of(&format!(
            "{}{}{}<w:sectPr/>",
            p("start"),
            end,
            p("second section")
        ));
        assert_eq!(
            e.text,
            "start\nend of one\n--- section 2 ---\nsecond section"
        );
        assert_eq!(e.sections, 2);
        assert_eq!(e.items.last().unwrap().page, 2);
        assert_eq!(e.items.last().unwrap().block, 1);
        // A section that ends the document has no separator after it.
        let e = text_of(&format!("{}{}", p("only"), end));
        assert_eq!(e.text, "only\nend of one");
        assert_eq!(e.sections, 1);
    }

    #[test]
    fn deleted_text_field_codes_and_text_boxes_are_not_text_and_are_reported() {
        let body = r#"<w:p><w:r><w:t>kept</w:t></w:r><w:del><w:r><w:delText>gone</w:delText></w:r></w:del><w:r><w:instrText>PAGE</w:instrText></w:r><w:ins><w:r><w:t> added</w:t></w:r></w:ins></w:p><w:p><w:r><w:drawing><w:txbxContent><w:p><w:r><w:t>boxed</w:t></w:r></w:p></w:txbxContent></w:drawing></w:r><w:r><w:t>after</w:t></w:r></w:p>"#;
        let e = text_of(body);
        assert_eq!(e.text, "kept added\nafter");
        assert!(
            e.warnings.iter().any(|w| w.contains("text box")),
            "{:?}",
            e.warnings
        );
        assert!(
            e.warnings.iter().any(|w| w.contains("tracked deletion")),
            "{:?}",
            e.warnings
        );
        assert!(!e.text.contains("gone") && !e.text.contains("PAGE") && !e.text.contains("boxed"));
    }

    #[test]
    fn a_document_without_text_says_so_and_empty_blocks_are_left_out() {
        let e = text_of(&format!(
            "<w:p/>{}<w:p><w:r><w:t>   </w:t></w:r></w:p>",
            "<w:p><w:r><w:drawing/></w:r></w:p>"
        ));
        assert_eq!(e.text, "");
        assert_eq!(e.blocks, 0);
        assert!(
            e.warnings.iter().any(|w| w.contains("no text")),
            "{:?}",
            e.warnings
        );
    }

    #[test]
    fn nested_tables_and_content_controls_are_in_the_flow() {
        let nested = format!("<w:tbl><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>", p("inner"));
        let body = format!(
            "<w:sdt><w:sdtContent>{}</w:sdtContent></w:sdt><w:tbl><w:tr><w:tc>{}{nested}</w:tc><w:tc></w:tc></w:tr></w:tbl>",
            p("controlled"),
            p("outer")
        );
        assert_eq!(text_of(&body).text, "controlled\n| outer / inner |  |");
    }

    #[test]
    fn external_relationships_are_counted_and_never_followed() {
        let rels = r#"<Relationship Id="r1" Type="x" Target="https://example.org/a.png" TargetMode="External"/><Relationship Id="r2" Type="x" Target="media/a.png"/><Relationship Id="r3" Type="x" Target="file:///etc/passwd" TargetMode="External"/>"#;
        let e = extract_docx(&docx(&p("text"), Some(rels)), NEVER).unwrap();
        assert_eq!(e.text, "text");
        assert!(
            e.warnings.iter().any(|w| w.starts_with("2 external")),
            "{:?}",
            e.warnings
        );
    }

    #[test]
    fn what_is_not_a_word_document_fails_with_a_reason() {
        let reason = |bytes: &[u8]| extract_docx(bytes, NEVER).unwrap_err().to_string();
        assert!(reason(b"not a zip at all").contains("Not a Word document"));
        assert!(reason(b"").contains("Not a Word document"));
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("xl/workbook.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"<x/>").unwrap();
        assert!(reason(&zip.finish().unwrap().into_inner()).contains("document.xml is missing"));
        assert!(
            reason(&docx("<w:p>", None)).contains("not valid XML")
                || reason(&docx("<w:p>", None)).contains("XML")
        );
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(format!(r#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY e "x">]><w:document xmlns:w="{W_NS}"><w:body><w:p><w:r><w:t>&e;</w:t></w:r></w:p></w:body></w:document>"#).as_bytes()).unwrap();
        assert!(reason(&zip.finish().unwrap().into_inner()).contains("not valid XML"));
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(format!(r#"<w:document xmlns:w="{W_NS}"/>"#).as_bytes())
            .unwrap();
        assert!(reason(&zip.finish().unwrap().into_inner()).contains("no body"));
    }

    #[test]
    fn a_part_that_is_too_large_when_unpacked_is_refused_without_reading_it_all() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        let chunk = vec![b' '; 1024 * 1024];
        for _ in 0..(MAX_PART_BYTES / (1024 * 1024) + 1) {
            zip.write_all(&chunk).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        assert!(bytes.len() < 1024 * 1024);
        let error = extract_docx(&bytes, NEVER).unwrap_err().to_string();
        assert!(error.contains("larger than") && error.contains("unpacked"));
    }

    #[test]
    fn the_text_is_cut_at_the_block_limit_and_says_so() {
        let body: String = (0..MAX_BLOCKS + 10)
            .map(|i| p(&format!("line {i}")))
            .collect();
        let e = text_of(&body);
        assert!(e.truncated);
        assert_eq!(e.blocks, MAX_BLOCKS);
        assert!(e.warnings.iter().any(|w| w.contains("cut at the limit")));
        assert!(e.text.ends_with(&format!("line {}", MAX_BLOCKS - 1)));
    }

    #[test]
    fn cancelling_stops_the_work() {
        let bytes = docx(
            &(0..5000)
                .map(|i| p(&format!("line {i}")))
                .collect::<String>(),
            None,
        );
        assert!(matches!(
            extract_docx(&bytes, &|| true),
            Err(CoreError::Cancelled)
        ));
        let calls = std::cell::Cell::new(0usize);
        let later: &dyn Fn() -> bool = &|| {
            calls.set(calls.get() + 1);
            calls.get() > 4
        };
        assert!(matches!(
            extract_docx(&bytes, later),
            Err(CoreError::Cancelled)
        ));
        assert!(extract_docx(&bytes, NEVER).is_ok());
    }

    #[test]
    fn unicode_text_is_kept_and_the_same_document_gives_the_same_text() {
        let body = format!(
            "{}{}",
            styled("Heading1", "Čas in prostor"),
            p("日本語 – ž")
        );
        let one = text_of(&body);
        assert_eq!(one.text, "# Čas in prostor\n日本語 – ž");
        assert_eq!(one, text_of(&body));
    }

    // ── PDF ──

    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};

    /// A PDF whose pages each hold the given lines of text (one text object per line).
    fn pdf(pages: &[&[&str]]) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier" },
        );
        let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
        let mut kids = Vec::new();
        for lines in pages {
            let mut ops = Vec::new();
            for (k, line) in lines.iter().enumerate() {
                ops.push(Operation::new("BT", vec![]));
                ops.push(Operation::new("Tf", vec!["F1".into(), 12.into()]));
                ops.push(Operation::new(
                    "Td",
                    vec![50.into(), (700 - 20 * k as i64).into()],
                ));
                ops.push(Operation::new("Tj", vec![Object::string_literal(*line)]));
                ops.push(Operation::new("ET", vec![]));
            }
            let content = doc.add_object(Stream::new(
                dictionary! {},
                Content { operations: ops }.encode().unwrap(),
            ));
            let page = doc.add_object(
                dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => content },
            );
            kids.push(page.into());
        }
        let count = kids.len() as i64;
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count, "Resources" => resources, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()] }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    /// Helper to build a custom positioned PDF where each page is a list of `(x, y, font_size, text)`.
    fn positioned_pdf(pages: &[&[(i64, i64, i64, &str)]], with_acroform: bool) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        );
        let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
        let mut kids = Vec::new();
        for items in pages {
            let mut ops = Vec::new();
            for &(x, y, sz, text) in *items {
                ops.push(Operation::new("BT", vec![]));
                ops.push(Operation::new("Tf", vec!["F1".into(), sz.into()]));
                ops.push(Operation::new("Td", vec![x.into(), y.into()]));
                ops.push(Operation::new("Tj", vec![Object::string_literal(text)]));
                ops.push(Operation::new("ET", vec![]));
            }
            let content = doc.add_object(Stream::new(
                dictionary! {},
                Content { operations: ops }.encode().unwrap(),
            ));
            let page = doc.add_object(
                dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => content },
            );
            kids.push(page.into());
        }
        let count = kids.len() as i64;
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count, "Resources" => resources, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()] }));
        let mut cat_dict = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
        if with_acroform {
            cat_dict.set("AcroForm", dictionary! { "Fields" => vec![] });
        }
        let catalog = doc.add_object(cat_dict);
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn a_pdf_gives_its_pages_in_order_with_page_markers_and_lines() {
        let bytes = pdf(&[&["Title", "first paragraph"], &["second page text"]]);
        let e = extract_pdf(&bytes, NEVER).unwrap();
        assert_eq!(
            e.text,
            "--- page 1 ---\nTitle\nfirst paragraph\n--- page 2 ---\nsecond page text"
        );
        assert_eq!((e.blocks, e.sections), (3, 2));
        assert!(e.warnings.is_empty(), "{:?}", e.warnings);
        assert!(!e.truncated);
        // Positioned blocks keep page, block index and PDF geometry.
        let b1 = e.items.iter().find(|b| b.text == "Title").unwrap();
        assert_eq!((b1.page, b1.block, b1.line), (1, 1, 2));
        assert!(b1
            .geometry
            .as_ref()
            .is_some_and(|g| (g.x - 50.0).abs() < 1.0 && (g.y - 700.0).abs() < 1.0));
        let b3 = e
            .items
            .iter()
            .find(|b| b.text == "second page text")
            .unwrap();
        assert_eq!((b3.page, b3.block, b3.line), (2, 1, 5));
    }

    #[test]
    fn two_column_pdf_reads_column_by_column_even_when_drawn_interleaved() {
        // Drawn in interleaved y-order (left 1, right 1, left 2, right 2) beneath a full-width title.
        let bytes = positioned_pdf(
            &[&[
                (60, 740, 20, "Two-Column Paper Title Spanning Both Columns"),
                (50, 680, 11, "Left column first line"),
                (330, 680, 11, "Right column first line"),
                (50, 660, 11, "Left column second line"),
                (330, 660, 11, "Right column second line"),
                (50, 640, 11, "Left column third line"),
                (330, 640, 11, "Right column third line"),
            ]],
            false,
        );
        let e = extract_pdf(&bytes, NEVER).unwrap();
        let texts: Vec<&str> = e
            .items
            .iter()
            .filter(|b| b.kind != BlockKind::SectionBreak)
            .map(|b| b.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec![
                "Two-Column Paper Title Spanning Both Columns",
                "Left column first line",
                "Left column second line",
                "Left column third line",
                "Right column first line",
                "Right column second line",
                "Right column third line",
            ]
        );
        // Heading detected by larger font size (20 pt vs 11 pt body).
        let title = e
            .items
            .iter()
            .find(|b| b.text.starts_with("Two-Column"))
            .unwrap();
        assert_eq!(title.kind, BlockKind::Heading);
        assert_eq!(title.heading_level, Some(1));
    }

    #[test]
    fn pdf_table_reads_in_row_order_with_cells_and_table_coordinates() {
        // 3-column table drawn with cells out of left-to-right order in the PDF stream.
        let bytes = positioned_pdf(
            &[&[
                (50, 740, 18, "Quarterly Metrics Report Heading"),
                (350, 680, 11, "Revenue"),
                (50, 680, 11, "Region"),
                (200, 680, 11, "Units"),
                (200, 660, 11, "120"),
                (350, 660, 11, "4800"),
                (50, 660, 11, "North"),
                (50, 640, 11, "South"),
                (200, 640, 11, "95"),
                (350, 640, 11, "3900"),
            ]],
            false,
        );
        let e = extract_pdf(&bytes, NEVER).unwrap();
        let table_rows: Vec<&DocBlock> = e
            .items
            .iter()
            .filter(|b| b.kind == BlockKind::TableRow)
            .collect();
        assert_eq!(table_rows.len(), 3, "items: {:?}", e.items);
        assert_eq!(
            table_rows[0].cells.as_deref(),
            Some(&["Region".into(), "Units".into(), "Revenue".into()][..])
        );
        assert_eq!(
            table_rows[1].cells.as_deref(),
            Some(&["North".into(), "120".into(), "4800".into()][..])
        );
        assert_eq!(
            table_rows[2].cells.as_deref(),
            Some(&["South".into(), "95".into(), "3900".into()][..])
        );
        assert_eq!(
            (table_rows[2].table, table_rows[2].table_row),
            (Some(1), Some(3))
        );
    }

    #[test]
    fn pdf_with_missing_unicode_map_warns_user() {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        // Type0 Identity-H font with no /ToUnicode stream.
        let font = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type0",
            "BaseFont" => "CustomCID",
            "Encoding" => "Identity-H"
        });
        let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
        let ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Td", vec![50.into(), 700.into()]),
            Operation::new("Tj", vec![Object::string_literal("\x00\x24\x00\x25")]),
            Operation::new("ET", vec![]),
        ];
        let content = doc.add_object(Stream::new(
            dictionary! {},
            Content { operations: ops }.encode().unwrap(),
        ));
        let page = doc.add_object(
            dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => content },
        );
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1, "Resources" => resources, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()] }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let e = extract_pdf(&bytes, NEVER).unwrap();
        assert!(
            e.warnings.iter().any(|w| w.contains("no Unicode map")),
            "expected missing Unicode map warning, got: {:?}",
            e.warnings
        );
    }

    #[test]
    fn pdf_with_acroform_reports_form_warning() {
        let bytes = positioned_pdf(&[&[(50, 700, 12, "Application Form")]], true);
        let e = extract_pdf(&bytes, NEVER).unwrap();
        assert!(
            e.warnings.iter().any(|w| w.contains("AcroForm")),
            "{:?}",
            e.warnings
        );
    }

    #[test]
    fn five_hundred_page_pdf_extracts_quickly_in_linear_time() {
        let pages: Vec<&[&str]> = (0..500)
            .map(|_| &["Chapter line one", "Body paragraph line two"][..])
            .collect();
        let bytes = pdf(&pages);
        let started = std::time::Instant::now();
        let e = extract_pdf(&bytes, NEVER).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(e.sections, 500);
        assert_eq!(e.blocks, 1000);
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "500-page PDF took too long: {elapsed:?}"
        );
    }

    #[test]
    fn an_inserted_page_shows_in_the_text_comparison_as_added_lines() {
        let a = extract_pdf(&pdf(&[&["one"], &["three"]]), NEVER)
            .unwrap()
            .text;
        let b = extract_pdf(&pdf(&[&["one"], &["two"], &["three"]]), NEVER)
            .unwrap()
            .text;
        let d = crate::fs::diff::diff_text(&a, &b, crate::fs::diff::DiffOptions::default());
        assert!(
            d.added >= 2,
            "the new page marker and its text are added: {}",
            d.added
        );
        assert!(d.right_lines.iter().any(|l| l == "two"));
    }

    #[test]
    fn a_pdf_without_text_is_told_and_empty() {
        let e = extract_pdf(&pdf(&[&[], &[]]), NEVER).unwrap();
        assert!(
            e.warnings.iter().any(|w| w.contains("no text")),
            "{:?}",
            e.warnings
        );
        assert_eq!(e.blocks, 0);
    }

    #[test]
    fn a_file_that_is_no_pdf_or_is_damaged_is_refused_with_a_reason_and_never_a_panic() {
        assert!(extract_pdf(b"hello", NEVER)
            .unwrap_err()
            .to_string()
            .contains("Not a PDF"));
        let mut broken = pdf(&[&["text"]]);
        broken.truncate(broken.len() / 2);
        let err = extract_pdf(&broken, NEVER);
        assert!(err.is_err() || err.unwrap().blocks <= 1);
        assert!(extract_pdf(
            b"%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\ngarbage\x00\xff",
            NEVER
        )
        .is_err());
    }

    #[test]
    fn an_encrypted_pdf_is_refused_as_encrypted_whether_the_dictionary_is_referenced_or_direct() {
        for direct in [false, true] {
            let mut doc = Document::load_mem(&pdf(&[&["secret"]])).unwrap();
            let dict = dictionary! { "Filter" => "Standard", "V" => 1, "R" => 2, "O" => Object::string_literal("x"), "U" => Object::string_literal("x"), "P" => -4 };
            if direct {
                doc.trailer.set("Encrypt", Object::Dictionary(dict));
            } else {
                let encrypt = doc.add_object(dict);
                doc.trailer.set("Encrypt", encrypt);
            }
            doc.trailer.set(
                "ID",
                vec![Object::string_literal("a"), Object::string_literal("a")],
            );
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            let err = extract_pdf(&bytes, NEVER).unwrap_err().to_string();
            assert!(err.contains("encrypted"), "direct={direct}: {err}");
        }
    }

    /// A one-page PDF whose content stream is `body` repeated until it is `size` bytes, compressed.
    fn pdf_with_content(body: &[u8], size: usize, second_page: Option<&str>) -> Vec<u8> {
        let lines: &[&[&str]] = &[&["first"], &[second_page.unwrap_or("second")]];
        let mut doc = Document::load_mem(&pdf(lines)).unwrap();
        let first = doc.get_pages()[&1];
        let mut stream = Stream::new(
            dictionary! {},
            body.iter().copied().cycle().take(size).collect(),
        );
        stream.compress().unwrap();
        let id = doc.add_object(stream);
        doc.get_object_mut(first)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Contents", id);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_page_that_unpacks_past_the_limit_is_told_by_page_and_the_others_are_read() {
        for (what, body) in [("bytes", &b" "[..]), ("operations", &b"q\n"[..])] {
            let bytes = pdf_with_content(body, MAX_PAGE_DECOMPRESSED + (8 << 20), None);
            assert!(
                bytes.len() < 1 << 20,
                "{what}: small on disk: {}",
                bytes.len()
            );
            let started = std::time::Instant::now();
            let e = extract_pdf(&bytes, NEVER).unwrap();
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "{what}: {:?}",
                started.elapsed()
            );
            assert!(e.text.contains("second"), "{what}: {}", e.text);
            assert!(
                e.warnings
                    .iter()
                    .any(|w| w.starts_with("1 page(s) could not be read (1)")),
                "{what}: {:?}",
                e.warnings
            );
            assert!(
                !e.warnings.iter().any(|w| w.contains("no text")),
                "{what}: {:?}",
                e.warnings
            );
        }
    }

    #[test]
    fn when_every_page_is_unreadable_it_is_not_called_a_scan() {
        let mut doc = Document::load_mem(&pdf(&[&["x"]])).unwrap();
        let first = doc.get_pages()[&1];
        let mut stream = Stream::new(dictionary! {}, vec![b' '; MAX_PAGE_DECOMPRESSED + 1024]);
        stream.compress().unwrap();
        let id = doc.add_object(stream);
        doc.get_object_mut(first)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Contents", id);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let e = extract_pdf(&bytes, NEVER).unwrap();
        assert!(
            e.warnings.iter().any(|w| w.contains("could not be read")),
            "{:?}",
            e.warnings
        );
        assert!(
            !e.warnings.iter().any(|w| w.contains("no text")),
            "{:?}",
            e.warnings
        );
    }

    #[test]
    fn too_many_pages_are_cut_with_a_warning_and_a_large_file_is_refused() {
        let many: Vec<&[&str]> = (0..MAX_PDF_PAGES + 3).map(|_| &["p"][..]).collect();
        let e = extract_pdf(&pdf(&many), NEVER).unwrap();
        assert!(e.truncated);
        assert!(
            e.warnings.iter().any(|w| w.contains("Only the first")),
            "{:?}",
            e.warnings
        );
        assert_eq!(e.blocks, MAX_PDF_PAGES);
        // Hostile 64 MiB PDF is rejected immediately before parsing.
        let started = std::time::Instant::now();
        let big = vec![b'x'; 64 * 1024 * 1024];
        assert!(extract_pdf(&big, NEVER)
            .unwrap_err()
            .to_string()
            .contains("larger"));
        assert!(started.elapsed() < std::time::Duration::from_millis(200));
    }

    #[test]
    fn cancelling_stops_the_extraction_before_and_between_pages() {
        let bytes = pdf(&[&["a"], &["b"], &["c"], &["d"]]);
        assert!(matches!(
            extract_pdf(&bytes, &|| true),
            Err(CoreError::Cancelled)
        ));
        let asked = std::cell::Cell::new(0);
        let result = extract_pdf(&bytes, &|| {
            asked.set(asked.get() + 1);
            asked.get() > 4
        });
        assert!(
            matches!(result, Err(CoreError::Cancelled)),
            "{} questions",
            asked.get()
        );
    }
}
