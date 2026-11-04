//! Table comparison for CSV/TSV and spreadsheets (xlsx, xls, xlsb, ods).
//! Columns are matched by header name, rows by a key column (detected or chosen)
//! or by position, and cells are compared with optional numeric/whitespace/case
//! tolerance. Spreadsheets are read with `calamine`, CSV with `csv`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::{CoreError, Result};

/// Rows kept per sheet and side; larger sheets are truncated (and flagged).
pub const MAX_ROWS: usize = 200_000;
const SPREADSHEET_EXT: &[&str] = &["xlsx", "xlsm", "xls", "xlsb", "ods"];
const CSV_EXT: &[&str] = &["csv", "tsv", "tab"];

pub fn is_table_path(p: &Path) -> bool {
    let e = p
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    SPREADSHEET_EXT.contains(&e.as_str()) || CSV_EXT.contains(&e.as_str())
}

pub fn is_spreadsheet(p: &Path) -> bool {
    let e = p
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    SPREADSHEET_EXT.contains(&e.as_str())
}

#[derive(Debug, Clone)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
}

// ───────────────────────────── reading ─────────────────────────────

/// Decodes text: UTF-8 (BOM stripped), else Windows-1252-ish Latin-1 fallback.
fn decode(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

/// Picks the delimiter that splits the first lines into the most consistent columns.
pub fn sniff_delimiter(text: &str, ext: &str) -> u8 {
    if ext == "tsv" || ext == "tab" {
        return b'\t';
    }
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .collect();
    let mut best = (b',', 0usize);
    for &d in b",;\t|" {
        let counts: Vec<usize> = lines
            .iter()
            .map(|l| count_outside_quotes(l, d as char))
            .collect();
        let Some(&first) = counts.first() else {
            continue;
        };
        if first == 0 {
            continue;
        }
        let consistent = counts.iter().filter(|&&c| c == first).count();
        let score = consistent * 100 + first;
        if score > best.1 {
            best = (d, score);
        }
    }
    best.0
}

fn count_outside_quotes(line: &str, d: char) -> usize {
    let mut q = false;
    line.chars()
        .filter(|&c| {
            if c == '"' {
                q = !q;
            }
            c == d && !q
        })
        .count()
}

pub fn read_csv(path: &Path) -> Result<Sheet> {
    let bytes = super::read_regular(path)
        .map_err(|e| CoreError::Other(format!("{}: {e}", path.display())))?;
    let text = decode(&bytes);
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let delim = sniff_delimiter(&text, &ext);
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    let mut truncated = false;
    for rec in rdr.records() {
        let rec = rec.map_err(|e| CoreError::Other(format!("{}: {e}", path.display())))?;
        if rows.len() >= MAX_ROWS {
            truncated = true;
            break;
        }
        rows.push(rec.iter().map(str::to_string).collect());
    }
    Ok(Sheet {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        rows,
        truncated,
    })
}

pub fn read_spreadsheet(path: &Path, formulas: bool) -> Result<Vec<Sheet>> {
    use calamine::{open_workbook_auto, Reader};
    super::open_regular(path).map_err(|e| CoreError::Other(format!("{}: {e}", path.display())))?;
    let mut wb = open_workbook_auto(path)
        .map_err(|e| CoreError::Other(format!("{}: {e}", path.display())))?;
    let mut out = Vec::new();
    for name in wb.sheet_names() {
        let mut truncated = false;
        let rows: Vec<Vec<String>> = if formulas {
            // Formula text where there is one, else the value.
            let values = wb
                .worksheet_range(&name)
                .map_err(|e| CoreError::Other(format!("{name}: {e}")))?;
            let f = wb.worksheet_formula(&name).ok();
            let (r0, c0) = values.start().unwrap_or((0, 0));
            values
                .rows()
                .enumerate()
                .take_while(|(i, _)| {
                    let keep = *i < MAX_ROWS;
                    truncated |= !keep;
                    keep
                })
                .map(|(i, row)| {
                    row.iter()
                        .enumerate()
                        .map(|(j, v)| {
                            let ff = f
                                .as_ref()
                                .and_then(|f| f.get_value((r0 + i as u32, c0 + j as u32)))
                                .filter(|s| !s.is_empty());
                            ff.map(|s| format!("={s}")).unwrap_or_else(|| cell_text(v))
                        })
                        .collect()
                })
                .collect()
        } else {
            let values = wb
                .worksheet_range(&name)
                .map_err(|e| CoreError::Other(format!("{name}: {e}")))?;
            let mut v = Vec::new();
            for row in values.rows() {
                if v.len() >= MAX_ROWS {
                    truncated = true;
                    break;
                }
                v.push(row.iter().map(cell_text).collect());
            }
            v
        };
        out.push(Sheet {
            name,
            rows: trim_trailing_empty(rows),
            truncated,
        });
    }
    Ok(out)
}

/// Cell value as shown in the compare view. Date/time cells in xlsx/xls (Excel and
/// LibreOffice alike) are serial numbers with a date format; `Display` would print
/// the bare serial (46037), so they are written as ISO dates instead. Durations
/// become `h:mm:ss`. Everything else uses calamine's own formatting.
fn cell_text(v: &calamine::Data) -> String {
    match v {
        calamine::Data::DateTime(dt) if dt.is_duration() => {
            let secs = (dt.as_f64() * 86_400.0).round() as i64;
            let sign = if secs < 0 { "-" } else { "" };
            let s = secs.abs();
            format!("{sign}{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
        }
        calamine::Data::DateTime(dt) => {
            let (y, mo, d, h, mi, s, ms) = dt.to_ymd_hms_milli();
            let has_time = (h, mi, s, ms) != (0, 0, 0, 0);
            let time = if s == 0 && ms == 0 {
                format!("{h:02}:{mi:02}")
            } else {
                format!("{h:02}:{mi:02}:{s:02}")
            };
            if dt.as_f64().abs() < 1.0 {
                time // time of day only (serial 0.x)
            } else if has_time {
                format!("{y:04}-{mo:02}-{d:02} {time}")
            } else {
                format!("{y:04}-{mo:02}-{d:02}")
            }
        }
        other => other.to_string(),
    }
}

fn trim_trailing_empty(mut rows: Vec<Vec<String>>) -> Vec<Vec<String>> {
    while rows
        .last()
        .is_some_and(|r| r.iter().all(|c| c.trim().is_empty()))
    {
        rows.pop();
    }
    rows
}

pub fn read_tables(path: &Path, formulas: bool) -> Result<Vec<Sheet>> {
    if is_spreadsheet(path) {
        read_spreadsheet(path, formulas)
    } else {
        Ok(vec![read_csv(path)?])
    }
}

/// Text form of a table (one line per row) for similarity scoring.
pub fn table_as_lines(path: &Path) -> Result<Vec<String>> {
    Ok(read_tables(path, false)?
        .into_iter()
        .flat_map(|s| {
            s.rows
                .into_iter()
                .map(move |r| format!("{}\u{1f}{}", s.name, r.join("\u{1f}")))
        })
        .collect())
}

// ───────────────────────────── diff ─────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TableDiffOptions {
    /// Key column name; None = auto-detect, Some("") = match rows by position.
    pub key: Option<String>,
    /// First row holds column names.
    pub header: bool,
    /// Compare numbers by value (1.0 = 1).
    pub numeric: bool,
    /// Numbers closer than this count as equal.
    pub tolerance: f64,
    pub ignore_whitespace: bool,
    pub ignore_case: bool,
    /// Spreadsheets: compare formulas instead of values.
    pub formulas: bool,
}

impl Default for TableDiffOptions {
    fn default() -> Self {
        TableDiffOptions {
            key: None,
            header: true,
            numeric: true,
            tolerance: 1e-9,
            ignore_whitespace: false,
            ignore_case: false,
            formulas: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    pub left: Option<usize>,
    pub right: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RowStatus {
    Same,
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowDiff {
    pub status: RowStatus,
    /// Data row index on each side (header excluded).
    pub left: Option<usize>,
    pub right: Option<usize>,
    /// Unified column indices whose values differ (changed rows).
    pub cells: Vec<usize>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetCounts {
    pub same: usize,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub cells: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetDiff {
    pub name: String,
    /// "both" | "leftOnly" | "rightOnly"
    pub presence: &'static str,
    pub columns: Vec<Column>,
    /// Key column used (None = rows matched by position).
    pub key: Option<String>,
    pub key_candidates: Vec<String>,
    pub left_rows: Vec<Vec<String>>,
    pub right_rows: Vec<Vec<String>>,
    pub rows: Vec<RowDiff>,
    pub counts: SheetCounts,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableDiffResult {
    pub sheets: Vec<SheetDiff>,
}

fn col_letter(mut i: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (i % 26) as u8) as char);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s
}

fn norm(v: &str, o: &TableDiffOptions) -> String {
    let mut s = if o.ignore_whitespace {
        v.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        v.trim_end_matches(['\r']).to_string()
    };
    if o.ignore_case {
        s = s.to_lowercase();
    }
    s
}

fn num(v: &str) -> Option<f64> {
    let t = v.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().or_else(|| {
        // "1 234,5" / "1234,5" (European decimal comma) when there is no dot
        if t.contains('.') {
            return None;
        }
        t.replace([' ', '\u{a0}'], "")
            .replace(',', ".")
            .parse()
            .ok()
    })
}

fn cells_equal(a: &str, b: &str, o: &TableDiffOptions) -> bool {
    if o.numeric {
        if let (Some(x), Some(y)) = (num(a), num(b)) {
            return (x - y).abs() <= o.tolerance.max(0.0) * x.abs().max(y.abs()).max(1.0);
        }
    }
    norm(a, o) == norm(b, o)
}

/// Key candidates: columns on both sides whose values are present and unique on both.
fn key_candidates(cols: &[Column], l: &[Vec<String>], r: &[Vec<String>]) -> Vec<String> {
    let unique = |rows: &[Vec<String>], c: usize| {
        let mut seen = std::collections::HashSet::new();
        let vals: Vec<&str> = rows
            .iter()
            .map(|row| row.get(c).map(|s| s.trim()).unwrap_or(""))
            .collect();
        !vals.is_empty() && vals.iter().all(|v| !v.is_empty() && seen.insert(*v))
    };
    let mut c: Vec<(i32, String)> = cols
        .iter()
        .filter_map(|col| match (col.left, col.right) {
            (Some(a), Some(b)) if unique(l, a) && unique(r, b) => {
                let n = col.name.to_lowercase();
                let named = [
                    "id", "key", "code", "sku", "email", "uuid", "šifra", "sifra", "number", "no",
                ]
                .iter()
                .any(|k| {
                    n == *k
                        || n.ends_with(&format!(" {k}"))
                        || n.ends_with(&format!("_{k}"))
                        || n.starts_with(&format!("{k}_"))
                });
                Some((if named { 0 } else { 1 }, col.name.clone()))
            }
            _ => None,
        })
        .collect();
    c.sort_by_key(|(rank, _)| *rank); // stable: named keys first, then column order
    c.into_iter().map(|(_, n)| n).collect()
}

fn diff_sheet(name: &str, l: Option<&Sheet>, r: Option<&Sheet>, o: &TableDiffOptions) -> SheetDiff {
    let empty = Sheet {
        name: name.into(),
        rows: vec![],
        truncated: false,
    };
    let (ls, rs) = (l.unwrap_or(&empty), r.unwrap_or(&empty));
    let width = |s: &Sheet| s.rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let (lh, rh): (Vec<String>, Vec<String>) = if o.header {
        (
            ls.rows.first().cloned().unwrap_or_default(),
            rs.rows.first().cloned().unwrap_or_default(),
        )
    } else {
        (
            (0..width(ls)).map(col_letter).collect(),
            (0..width(rs)).map(col_letter).collect(),
        )
    };
    let skip = usize::from(o.header);
    let ld: Vec<Vec<String>> = ls.rows.iter().skip(skip).cloned().collect();
    let rd: Vec<Vec<String>> = rs.rows.iter().skip(skip).cloned().collect();

    // Columns: left order, then right-only columns; matched by trimmed, case-insensitive name.
    let key_of = |s: &str| s.trim().to_lowercase();
    let mut columns: Vec<Column> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (i, n) in lh.iter().enumerate() {
        let k = key_of(n);
        let label = if n.trim().is_empty() {
            col_letter(i)
        } else {
            n.clone()
        };
        if !k.is_empty() && !by_name.contains_key(&k) {
            by_name.insert(k, columns.len());
        }
        columns.push(Column {
            name: label,
            left: Some(i),
            right: None,
        });
    }
    for (j, n) in rh.iter().enumerate() {
        let k = key_of(n);
        match by_name
            .get(&k)
            .copied()
            .filter(|_| !k.is_empty())
            .or_else(|| {
                if k.is_empty() {
                    columns
                        .iter()
                        .position(|c| c.left == Some(j) && c.right.is_none())
                } else {
                    None
                }
            }) {
            Some(ci) if columns[ci].right.is_none() => columns[ci].right = Some(j),
            _ => columns.push(Column {
                name: if n.trim().is_empty() {
                    col_letter(j)
                } else {
                    n.clone()
                },
                left: None,
                right: Some(j),
            }),
        }
    }

    let candidates = key_candidates(&columns, &ld, &rd);
    let key: Option<String> = match &o.key {
        Some(k) if k.is_empty() => None,
        Some(k) => columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(k) && c.left.is_some() && c.right.is_some())
            .map(|c| c.name.clone()),
        None => candidates.first().cloned(),
    };
    let cell = |rows: &[Vec<String>], i: usize, c: Option<usize>| -> String {
        c.and_then(|c| rows[i].get(c).cloned()).unwrap_or_default()
    };
    let changed_cells = |li: usize, ri: usize| -> Vec<usize> {
        columns
            .iter()
            .enumerate()
            // Columns on one side only are reported as added/removed columns, not as
            // a change in every row.
            .filter(|(_, c)| c.left.is_some() && c.right.is_some())
            .filter(|(_, c)| !cells_equal(&cell(&ld, li, c.left), &cell(&rd, ri, c.right), o))
            .map(|(k, _)| k)
            .collect()
    };

    let mut rows: Vec<RowDiff> = Vec::new();
    let pair = |li: usize, ri: usize, rows: &mut Vec<RowDiff>| {
        let cells = changed_cells(li, ri);
        rows.push(RowDiff {
            status: if cells.is_empty() {
                RowStatus::Same
            } else {
                RowStatus::Changed
            },
            left: Some(li),
            right: Some(ri),
            cells,
        });
    };
    if let Some(kname) = &key {
        let kc = columns.iter().find(|c| &c.name == kname).unwrap();
        let (kl, kr) = (kc.left.unwrap(), kc.right.unwrap());
        let kv = |rows: &[Vec<String>], i: usize, c: usize| {
            norm(rows[i].get(c).map(|s| s.trim()).unwrap_or(""), o)
        };
        let rmap: HashMap<String, usize> = (0..rd.len()).map(|i| (kv(&rd, i, kr), i)).collect();
        let lmatch: Vec<Option<usize>> = (0..ld.len())
            .map(|li| rmap.get(&kv(&ld, li, kl)).copied())
            .collect();
        let mut right_matched = vec![false; rd.len()];
        for ri in lmatch.iter().flatten() {
            right_matched[*ri] = true;
        }
        // Left order; a right-only row is placed before the next matched right row that follows it.
        let mut emitted = vec![false; rd.len()];
        let mut next_r = 0usize;
        for (li, m) in lmatch.iter().enumerate() {
            match *m {
                Some(ri) => {
                    while next_r < ri {
                        if !right_matched[next_r] && !emitted[next_r] {
                            emitted[next_r] = true;
                            rows.push(RowDiff {
                                status: RowStatus::Added,
                                left: None,
                                right: Some(next_r),
                                cells: vec![],
                            });
                        }
                        next_r += 1;
                    }
                    next_r = next_r.max(ri + 1);
                    pair(li, ri, &mut rows);
                }
                None => rows.push(RowDiff {
                    status: RowStatus::Removed,
                    left: Some(li),
                    right: None,
                    cells: vec![],
                }),
            }
        }
        for ri in 0..rd.len() {
            if !right_matched[ri] && !emitted[ri] {
                rows.push(RowDiff {
                    status: RowStatus::Added,
                    left: None,
                    right: Some(ri),
                    cells: vec![],
                });
            }
        }
    } else {
        // Positional: line diff over whole-row signatures, replaced runs paired cell by cell.
        let sig = |rows: &[Vec<String>], i: usize, side_left: bool| -> String {
            columns
                .iter()
                .map(|c| {
                    let v = cell(rows, i, if side_left { c.left } else { c.right });
                    match num(&v).filter(|_| o.numeric) {
                        Some(n) => format!("{n}"),
                        None => norm(&v, o),
                    }
                })
                .collect::<Vec<_>>()
                .join("\u{1f}")
        };
        let sl: Vec<String> = (0..ld.len()).map(|i| sig(&ld, i, true)).collect();
        let sr: Vec<String> = (0..rd.len()).map(|i| sig(&rd, i, false)).collect();
        for op in similar::capture_diff_slices(similar::Algorithm::Myers, &sl, &sr) {
            match op {
                similar::DiffOp::Equal {
                    old_index,
                    new_index,
                    len,
                } => (0..len).for_each(|k| pair(old_index + k, new_index + k, &mut rows)),
                similar::DiffOp::Delete {
                    old_index, old_len, ..
                } => (0..old_len).for_each(|k| {
                    rows.push(RowDiff {
                        status: RowStatus::Removed,
                        left: Some(old_index + k),
                        right: None,
                        cells: vec![],
                    })
                }),
                similar::DiffOp::Insert {
                    new_index, new_len, ..
                } => (0..new_len).for_each(|k| {
                    rows.push(RowDiff {
                        status: RowStatus::Added,
                        left: None,
                        right: Some(new_index + k),
                        cells: vec![],
                    })
                }),
                similar::DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => {
                    let n = old_len.min(new_len);
                    (0..n).for_each(|k| pair(old_index + k, new_index + k, &mut rows));
                    (n..old_len).for_each(|k| {
                        rows.push(RowDiff {
                            status: RowStatus::Removed,
                            left: Some(old_index + k),
                            right: None,
                            cells: vec![],
                        })
                    });
                    (n..new_len).for_each(|k| {
                        rows.push(RowDiff {
                            status: RowStatus::Added,
                            left: None,
                            right: Some(new_index + k),
                            cells: vec![],
                        })
                    });
                }
            }
        }
    }

    let mut counts = SheetCounts::default();
    for r in &rows {
        match r.status {
            RowStatus::Same => counts.same += 1,
            RowStatus::Added => counts.added += 1,
            RowStatus::Removed => counts.removed += 1,
            RowStatus::Changed => {
                counts.changed += 1;
                counts.cells += r.cells.len();
            }
        }
    }
    SheetDiff {
        name: name.into(),
        presence: match (l, r) {
            (Some(_), None) => "leftOnly",
            (None, Some(_)) => "rightOnly",
            _ => "both",
        },
        columns,
        key,
        key_candidates: candidates,
        left_rows: ld,
        right_rows: rd,
        rows,
        counts,
        truncated: ls.truncated || rs.truncated,
    }
}

pub fn compare_tables(a: &Path, b: &Path, o: &TableDiffOptions) -> Result<TableDiffResult> {
    let (la, lb) = (read_tables(a, o.formulas)?, read_tables(b, o.formulas)?);
    let single = la.len() == 1 && lb.len() == 1;
    let mut sheets = Vec::new();
    if single {
        // CSV vs CSV (or one-sheet workbooks): compare the only sheets whatever their names.
        let name = if la[0].name == lb[0].name {
            la[0].name.clone()
        } else {
            format!("{} ↔ {}", la[0].name, lb[0].name)
        };
        sheets.push(diff_sheet(&name, Some(&la[0]), Some(&lb[0]), o));
    } else {
        for s in &la {
            sheets.push(diff_sheet(
                &s.name,
                Some(s),
                lb.iter().find(|x| x.name == s.name),
                o,
            ));
        }
        for s in lb.iter().filter(|s| !la.iter().any(|x| x.name == s.name)) {
            sheets.push(diff_sheet(&s.name, None, Some(s), o));
        }
    }
    Ok(TableDiffResult { sheets })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, s: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, s).unwrap();
        p
    }

    #[test]
    fn sniffs_delimiters() {
        assert_eq!(sniff_delimiter("a;b;c\n1;2;3\n", "csv"), b';');
        assert_eq!(sniff_delimiter("a,b\n\"x,y\",2\n", "csv"), b',');
        assert_eq!(sniff_delimiter("a\tb\n1\t2\n", "csv"), b'\t');
    }

    #[test]
    fn key_rows_columns_and_numbers() {
        let t = tempfile::tempdir().unwrap();
        let a = write(
            t.path(),
            "a.csv",
            "id;name;price;old\n1;Apple;1.0;x\n2;Pear;2,50;y\n3;Plum;3;z\n",
        );
        // reordered columns, a removed column (old), an added one (stock), row 2 removed,
        // row 4 added, row 3 price changed, 1.0 vs 1 equal, 2,50 vs 2.5 equal
        let b = write(
            t.path(),
            "b.csv",
            "name;id;price;stock\nPlum;3;3.5;9\nApple;1;1;5\nKiwi;4;4;1\n",
        );
        let r = compare_tables(&a, &b, &TableDiffOptions::default()).unwrap();
        let s = &r.sheets[0];
        assert_eq!(s.key.as_deref(), Some("id"));
        let names: Vec<_> = s
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.left.is_some(), c.right.is_some()))
            .collect();
        assert_eq!(
            names,
            [
                ("id", true, true),
                ("name", true, true),
                ("price", true, true),
                ("old", true, false),
                ("stock", false, true)
            ]
        );
        assert_eq!(
            (s.counts.removed, s.counts.added, s.counts.changed),
            (1, 1, 1)
        );
        // apple: price 1.0 = 1, and one-sided columns (old/stock) are not row changes
        let apple = s.rows.iter().find(|r| r.left == Some(0)).unwrap();
        assert_eq!(apple.status, RowStatus::Same);
        let plum = s.rows.iter().find(|r| r.left == Some(2)).unwrap();
        assert!(plum.cells.iter().any(|&k| s.columns[k].name == "price"));
    }

    #[test]
    fn positional_when_no_key() {
        let t = tempfile::tempdir().unwrap();
        let a = write(t.path(), "a.csv", "x,y\n1,1\n1,1\n2,2\n");
        let b = write(t.path(), "b.csv", "x,y\n1,1\n1,1\n2,3\n5,5\n");
        let r = compare_tables(&a, &b, &TableDiffOptions::default()).unwrap();
        let s = &r.sheets[0];
        assert!(s.key.is_none(), "duplicate values: no key");
        assert_eq!((s.counts.same, s.counts.changed, s.counts.added), (2, 1, 1));
    }

    #[test]
    fn ignore_options_and_no_header() {
        let t = tempfile::tempdir().unwrap();
        let a = write(t.path(), "a.csv", "Hello  World,1\n");
        let b = write(t.path(), "b.csv", "hello world,1.000\n");
        let o = TableDiffOptions {
            header: false,
            ignore_whitespace: true,
            ignore_case: true,
            key: Some(String::new()),
            ..Default::default()
        };
        let r = compare_tables(&a, &b, &o).unwrap();
        assert_eq!(r.sheets[0].counts.same, 1);
        assert_eq!(r.sheets[0].columns[0].name, "A");
    }

    #[test]
    fn excel_dates_are_iso() {
        use calamine::{Data, ExcelDateTime, ExcelDateTimeType::*};
        let dt = |v: f64, t| cell_text(&Data::DateTime(ExcelDateTime::new(v, t, false)));
        assert_eq!(dt(46037.0, DateTime), "2026-01-15"); // what Excel writes for 15.1.2026
        assert_eq!(dt(46037.5, DateTime), "2026-01-15 12:00");
        assert_eq!(dt(0.75, DateTime), "18:00");
        assert_eq!(dt(1.5, TimeDelta), "36:00:00");
        assert_eq!(cell_text(&Data::Float(2.49)), "2.49");
        assert_eq!(cell_text(&Data::Bool(true)), "true");
    }

    #[test]
    fn column_letters() {
        assert_eq!(col_letter(0), "A");
        assert_eq!(col_letter(25), "Z");
        assert_eq!(col_letter(26), "AA");
        assert_eq!(col_letter(701), "ZZ");
    }
}
