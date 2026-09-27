//! Spreadsheets (.xlsx/.xlsm/.xlsb/.xls/.ods via calamine) and delimited
//! text (.csv/.tsv) to HTML grids.
//!
//! Every sheet becomes a table with column letters and row numbers, like the
//! app it came from. Sheets switch with a CSS-only tab strip (hidden radio
//! inputs plus `:checked ~` rules), since previews run without scripts.
//! Values are shown as calamine decodes them: numbers with up to ten
//! significant decimals, dates as ISO dates. Cell styles (fonts, fills,
//! custom number formats) are not rendered.

use crate::html::{self, Html, Kind};
use crate::Rendered;
use calamine::{open_workbook_auto_from_rs, Data, Reader, SheetType, SheetVisible, Sheets};
use cx_core::{CxError, Result};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::io::Cursor;

pub(crate) const MAX_ROWS: usize = 1000;
pub(crate) const MAX_COLS: usize = 100;
const MAX_SHEETS: usize = 64;
const MAX_MERGES: usize = 10_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CellKind {
    Text,
    Number,
    Bool,
    Error,
}

struct Cell {
    text: String,
    kind: CellKind,
}

/// One sheet, already cut to the preview window.
struct Grid {
    name: String,
    /// Absolute (row, col) of the first shown cell.
    origin: (u32, u32),
    cells: Vec<Vec<Cell>>,
    cols: usize,
    truncated_rows: bool,
    truncated_cols: bool,
    /// (row, col) relative to origin → (rowspan, colspan).
    spans: HashMap<(usize, usize), (usize, usize)>,
    covered: HashSet<(usize, usize)>,
    error: Option<String>,
}

impl Grid {
    fn empty(name: String) -> Grid {
        Grid {
            name,
            origin: (0, 0),
            cells: Vec::new(),
            cols: 0,
            truncated_rows: false,
            truncated_cols: false,
            spans: HashMap::new(),
            covered: HashSet::new(),
            error: None,
        }
    }

    /// Register merged areas (absolute, inclusive), clipped to the window.
    fn merges(&mut self, merges: impl Iterator<Item = ((u32, u32), (u32, u32))>) {
        let (rows, cols) = (self.cells.len(), self.cols);
        for (start, end) in merges.take(MAX_MERGES) {
            if start.0 < self.origin.0 || start.1 < self.origin.1 {
                continue;
            }
            let (r0, c0) = ((start.0 - self.origin.0) as usize, (start.1 - self.origin.1) as usize);
            if r0 >= rows || c0 >= cols || end.0 < start.0 || end.1 < start.1 {
                continue;
            }
            let r1 = ((end.0 - self.origin.0) as usize).min(rows - 1);
            let c1 = ((end.1 - self.origin.1) as usize).min(cols - 1);
            if (r1, c1) == (r0, c0) || self.covered.contains(&(r0, c0)) {
                continue;
            }
            self.spans.insert((r0, c0), (r1 - r0 + 1, c1 - c0 + 1));
            for r in r0..=r1 {
                for c in c0..=c1 {
                    if (r, c) != (r0, c0) {
                        self.covered.insert((r, c));
                    }
                }
            }
        }
    }
}

pub(crate) fn render_workbook(bytes: Vec<u8>, file_name: &str) -> Result<Rendered> {
    let mut wb = open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|e| CxError::Unsupported(format!("{file_name}: {e}")))?;
    let metas: Vec<_> = wb.sheets_metadata().to_vec();
    let visible: Vec<String> =
        metas.iter().filter(|s| s.visible == SheetVisible::Visible && matches!(s.typ, SheetType::WorkSheet | SheetType::MacroSheet | SheetType::DialogSheet)).map(|s| s.name.clone()).collect();
    let names = if visible.is_empty() { wb.sheet_names() } else { visible };
    let total = names.len();
    let mut grids = Vec::new();
    for name in names.into_iter().take(MAX_SHEETS) {
        let mut grid = match wb.worksheet_range(&name) {
            Ok(range) => grid_from_range(name.clone(), &range),
            Err(e) => {
                let mut g = Grid::empty(name.clone());
                g.error = Some(e.to_string());
                g
            }
        };
        let merges = match &mut wb {
            Sheets::Xlsx(x) => x.merge_cells_by_sheet_name(&name).unwrap_or_default(),
            Sheets::Xls(x) => x.merge_cells_by_sheet_name(&name).unwrap_or_default(),
            _ => Vec::new(),
        };
        grid.merges(merges.iter().map(|d| (d.start, d.end)));
        grids.push(grid);
    }
    let mut out_note = None;
    if total > MAX_SHEETS {
        out_note = Some(total - MAX_SHEETS);
    }
    let html = render_grids(&grids, file_name, out_note);
    Ok(Rendered { html, title: None, pages: Some(total as u32) })
}

fn grid_from_range(name: String, range: &calamine::Range<Data>) -> Grid {
    let mut g = Grid::empty(name);
    let (Some(start), Some(end)) = (range.start(), range.end()) else { return g };
    // Show from A1 unless the data starts far away from it.
    let origin = (if start.0 < 50 { 0 } else { start.0 }, if start.1 < 26 { 0 } else { start.1 });
    g.origin = origin;
    let rows = (end.0 - origin.0) as usize + 1;
    let cols = (end.1 - origin.1) as usize + 1;
    g.truncated_rows = rows > MAX_ROWS;
    g.truncated_cols = cols > MAX_COLS;
    g.cols = cols.min(MAX_COLS);
    for r in 0..rows.min(MAX_ROWS) {
        let row = (0..g.cols)
            .map(|c| match range.get_value((origin.0 + r as u32, origin.1 + c as u32)) {
                Some(d) => cell(d),
                None => Cell { text: String::new(), kind: CellKind::Text },
            })
            .collect();
        g.cells.push(row);
    }
    g
}

fn cell(d: &Data) -> Cell {
    let (text, kind) = match d {
        Data::Empty => (String::new(), CellKind::Text),
        Data::String(s) => (s.clone(), CellKind::Text),
        Data::Int(i) => (i.to_string(), CellKind::Number),
        Data::Float(f) => (fmt_float(*f), CellKind::Number),
        Data::Bool(b) => ((if *b { "TRUE" } else { "FALSE" }).to_string(), CellKind::Bool),
        Data::DateTime(dt) => {
            if dt.is_duration() {
                let secs = (dt.as_f64() * 86_400.0).round() as i64;
                let sign = if secs < 0 { "-" } else { "" };
                let s = secs.abs();
                (format!("{sign}{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60), CellKind::Number)
            } else {
                let (y, mo, da, h, mi, s, _) = dt.to_ymd_hms_milli();
                let v = dt.as_f64();
                let text = if (0.0..1.0).contains(&v) {
                    format!("{h:02}:{mi:02}:{s:02}")
                } else if v.fract().abs() < 1e-9 {
                    format!("{y:04}-{mo:02}-{da:02}")
                } else if s == 0 {
                    format!("{y:04}-{mo:02}-{da:02} {h:02}:{mi:02}")
                } else {
                    format!("{y:04}-{mo:02}-{da:02} {h:02}:{mi:02}:{s:02}")
                };
                (text, CellKind::Number)
            }
        }
        Data::DateTimeIso(s) | Data::DurationIso(s) => (s.clone(), CellKind::Number),
        Data::Error(e) => (e.to_string(), CellKind::Error),
    };
    Cell { text, kind }
}

/// Up to ten decimals, trailing zeros trimmed; scientific notation for
/// very large or very small magnitudes (as a spreadsheet's General format).
pub(crate) fn fmt_float(v: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-5..1e15).contains(&a) {
        let s = format!("{v:.6e}");
        // 1.500000e3 → 1.5e3
        if let Some((m, e)) = s.split_once('e') {
            let m = if m.contains('.') { m.trim_end_matches('0').trim_end_matches('.') } else { m };
            return format!("{m}E{e}");
        }
        return s;
    }
    let s = format!("{v:.10}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// Spreadsheet column name: 0 → A, 25 → Z, 26 → AA.
pub(crate) fn col_name(mut c: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

fn render_grids(grids: &[Grid], file_name: &str, more_sheets: Option<usize>) -> String {
    let mut out = Html::new();
    let tabs = grids.len() > 1;
    if tabs {
        for i in 0..grids.len() {
            out.raw("<input type=\"radio\" name=\"sheet\" class=\"tabin\" id=\"t").int(i as u64).raw("\"");
            if i == 0 {
                out.raw(" checked");
            }
            out.raw(">");
        }
        out.raw("<nav class=\"tabs\">");
        for (i, g) in grids.iter().enumerate() {
            out.raw("<label for=\"t").int(i as u64).raw("\" id=\"l").int(i as u64).raw("\">").text(&g.name).raw("</label>");
        }
        out.raw("</nav>");
    }
    out.raw("<div class=\"panels\">");
    for (i, g) in grids.iter().enumerate() {
        out.raw(if tabs { "<section class=\"panel\" id=\"p" } else { "<section class=\"panel only\" id=\"p" }).int(i as u64).raw("\">");
        grid_table(&mut out, g);
        out.raw("</section>");
    }
    out.raw("</div>");
    if let Some(n) = more_sheets {
        out.raw("<p class=\"note\">").int(n as u64).raw(" more sheets are not shown.</p>");
    }
    if grids.is_empty() {
        out.raw("<p class=\"empty-sheet\">This workbook has no sheets.</p>");
    }

    // Per-sheet rules for the CSS-only tabs; generated from indices only.
    let mut css = String::new();
    if tabs {
        for i in 0..grids.len() {
            let _ = write!(
                css,
                "#t{i}:checked~.panels #p{i}{{display:block}}#t{i}:checked~.tabs #l{i}{{background:var(--bg);color:var(--fg);border-color:var(--line)}}#t{i}:focus-visible~.tabs #l{i}{{outline:2px solid var(--accent)}}"
            );
        }
    }
    html::page(Kind::Sheet, file_name, out, &css)
}

fn grid_table(out: &mut Html, g: &Grid) {
    if let Some(e) = &g.error {
        out.raw("<p class=\"empty-sheet\">This sheet can't be shown: ").text(e).raw("</p>");
        return;
    }
    if g.cells.is_empty() {
        out.raw("<p class=\"empty-sheet\">This sheet is empty.</p>");
        return;
    }
    out.raw("<div class=\"grid-wrap\"><table class=\"grid\"><thead><tr><th class=\"corner\"></th>");
    for c in 0..g.cols {
        out.raw("<th>").text(&col_name(g.origin.1 + c as u32)).raw("</th>");
    }
    out.raw("</tr></thead><tbody>");
    for (r, row) in g.cells.iter().enumerate() {
        out.raw("<tr><th>").int(g.origin.0 as u64 + r as u64 + 1).raw("</th>");
        for (c, cell) in row.iter().enumerate() {
            if g.covered.contains(&(r, c)) {
                continue;
            }
            out.raw(match cell.kind {
                CellKind::Text => "<td",
                CellKind::Number => "<td class=\"n\"",
                CellKind::Bool => "<td class=\"bool\"",
                CellKind::Error => "<td class=\"e\"",
            });
            if let Some(&(rs, cs)) = g.spans.get(&(r, c)) {
                if rs > 1 {
                    out.raw(" rowspan=\"").int(rs as u64).raw("\"");
                }
                if cs > 1 {
                    out.raw(" colspan=\"").int(cs as u64).raw("\"");
                }
            }
            out.raw(">").text(&cell.text).raw("</td>");
        }
        out.raw("</tr>");
    }
    out.raw("</tbody></table></div>");
    if g.truncated_rows || g.truncated_cols {
        out.raw("<p class=\"note\">Only the first ");
        out.int(MAX_ROWS as u64).raw(" rows and ").int(MAX_COLS as u64).raw(" columns are shown (truncated).</p>");
    }
}

// ----------------------------------------------------------------- CSV

/// Decode text files: UTF-8 (with or without BOM), UTF-16 with BOM, else
/// Windows-1252 (what Excel on Windows writes).
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    if let Some((enc, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        return enc.decode_without_bom_handling(&bytes[bom..]).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

/// Guess the delimiter from the first line: the most frequent of `, ; \t |`
/// outside quotes.
fn sniff_delimiter(text: &str) -> char {
    let mut counts = [(',', 0usize), (';', 0), ('\t', 0), ('|', 0)];
    let mut quoted = false;
    for c in text.chars().take(8192) {
        match c {
            '"' => quoted = !quoted,
            '\n' if !quoted => break,
            c if !quoted => {
                for e in counts.iter_mut() {
                    if e.0 == c {
                        e.1 += 1;
                    }
                }
            }
            _ => {}
        }
    }
    counts.iter().filter(|e| e.1 > 0).max_by_key(|e| e.1).map(|e| e.0).unwrap_or(',')
}

/// RFC 4180 parsing (quotes, doubled quotes, newlines inside quotes).
/// Returns the rows and whether more rows/columns existed than kept.
fn parse_delimited(text: &str, delim: char, max_rows: usize, max_cols: usize) -> (Vec<Vec<String>>, bool, bool) {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut more_cols = false;
    let mut chars = text.chars().peekable();
    let end_field = |row: &mut Vec<String>, field: &mut String, more_cols: &mut bool| {
        if row.len() < max_cols {
            row.push(std::mem::take(field));
        } else {
            *more_cols = true;
            field.clear();
        }
    };
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            c if c == delim => end_field(&mut row, &mut field, &mut more_cols),
            '\r' => {}
            '\n' => {
                end_field(&mut row, &mut field, &mut more_cols);
                rows.push(std::mem::take(&mut row));
                if rows.len() >= max_rows {
                    let more = chars.any(|c| !c.is_whitespace());
                    return (rows, more, more_cols);
                }
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        end_field(&mut row, &mut field, &mut more_cols);
        rows.push(row);
    }
    (rows, false, more_cols)
}

pub(crate) fn render_csv(bytes: &[u8], file_name: &str, tsv: bool) -> Result<Rendered> {
    let text = decode_text(bytes);
    let delim = if tsv { '\t' } else { sniff_delimiter(&text) };
    let (rows, more_rows, more_cols) = parse_delimited(&text, delim, MAX_ROWS, MAX_COLS);
    let mut g = Grid::empty(file_name.to_string());
    g.cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    g.truncated_rows = more_rows;
    g.truncated_cols = more_cols;
    g.cells = rows
        .into_iter()
        .map(|r| {
            let mut cells: Vec<Cell> = r
                .into_iter()
                .map(|t| {
                    let kind = if !t.is_empty() && t.trim().parse::<f64>().is_ok() { CellKind::Number } else { CellKind::Text };
                    Cell { text: t, kind }
                })
                .collect();
            cells.resize_with(g.cols, || Cell { text: String::new(), kind: CellKind::Text });
            cells
        })
        .collect();
    let html = render_grids(&[g], file_name, None);
    Ok(Rendered { html, title: None, pages: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_numbers() {
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(col_name(701), "ZZ");
        assert_eq!(col_name(702), "AAA");
        assert_eq!(fmt_float(0.1 + 0.2), "0.3");
        assert_eq!(fmt_float(1234.5), "1234.5");
        assert_eq!(fmt_float(1.5e20), "1.5E20");
        assert_eq!(fmt_float(-2.0), "-2");
    }

    #[test]
    fn csv_quotes_and_delimiters() {
        let t = "a;b;\"c;d\"\n1;\"say \"\"hi\"\"\nthere\";3\n";
        assert_eq!(sniff_delimiter(t), ';');
        let (rows, more, _) = parse_delimited(t, ';', 10, 10);
        assert_eq!(rows, vec![vec!["a", "b", "c;d"], vec!["1", "say \"hi\"\nthere", "3"]]);
        assert!(!more);
        let (rows, more, more_cols) = parse_delimited("1,2,3\n4,5,6\n7,8,9\n", ',', 2, 2);
        assert_eq!(rows.len(), 2);
        assert!(more && more_cols);
    }
}
