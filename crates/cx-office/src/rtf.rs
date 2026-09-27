//! Rich Text Format to HTML.
//!
//! A small RTF reader covering what documents in the wild use for their
//! visible content: paragraphs, alignment, bold/italic/underline/strike,
//! super/subscript, colors from the color table, Unicode (`\u`) and code
//! page (`\'hh`) text, tables (`\intbl`/`\cell`/`\row`), page breaks,
//! hyperlink fields (text only) and PNG/JPEG pictures. Fonts, style sheets,
//! headers/footers, footnotes and other destinations are skipped.

use crate::html::{self, Html, ImageBudget, Kind, Rgb, Style};
use crate::Rendered;
use cx_core::{CxError, Result};
use encoding_rs::Encoding;

/// Group nesting deeper than this is ignored (malformed or hostile input).
const MAX_DEPTH: usize = 256;

#[derive(Clone, Default)]
struct State {
    /// Inside a destination whose text is not shown.
    skip: bool,
    b: bool,
    i: bool,
    u: bool,
    s: bool,
    sup: bool,
    sub: bool,
    color: usize,
    highlight: usize,
    /// Characters to skip after `\uN`.
    uc: usize,
    intbl: bool,
    align: Option<&'static str>,
    heading: Option<u8>,
    link: bool,
    /// Collecting a field instruction (`\fldinst`).
    instr: bool,
    /// Collecting a picture: `Some(is_supported_format)`.
    pict: Option<bool>,
    colortbl: bool,
}

#[derive(Clone, PartialEq)]
struct RunKey {
    b: bool,
    i: bool,
    u: bool,
    s: bool,
    sup: bool,
    sub: bool,
    color: Option<Rgb>,
    highlight: Option<Rgb>,
    link: bool,
}

struct Reader<'a> {
    src: &'a [u8],
    pos: usize,
    stack: Vec<State>,
    st: State,
    enc: &'static Encoding,
    colors: Vec<Option<Rgb>>,
    cur_color: (u8, u8, u8, bool),
    /// Code-page bytes awaiting decoding (multi-byte code pages need them together).
    bytes: Vec<u8>,
    /// Text of the current run.
    text: String,
    run: Option<RunKey>,
    para: Html,
    body: Html,
    cell: Html,
    row: Vec<Html>,
    table: Vec<Vec<Html>>,
    instr: String,
    last_instr: String,
    pict: Vec<u8>,
    pict_width: Option<f64>,
    uc_skip: usize,
    images: ImageBudget,
    truncated: bool,
}

pub(crate) fn render(bytes: &[u8], file_name: &str) -> Result<Rendered> {
    if !bytes.starts_with(b"{\\rtf") {
        return Err(CxError::Unsupported(format!("{file_name} is not an RTF document")));
    }
    let mut r = Reader {
        src: bytes,
        pos: 0,
        stack: Vec::new(),
        st: State { uc: 1, ..Default::default() },
        enc: encoding_rs::WINDOWS_1252,
        colors: Vec::new(),
        cur_color: (0, 0, 0, false),
        bytes: Vec::new(),
        text: String::new(),
        run: None,
        para: Html::new(),
        body: Html::new(),
        cell: Html::new(),
        row: Vec::new(),
        table: Vec::new(),
        instr: String::new(),
        last_instr: String::new(),
        pict: Vec::new(),
        pict_width: None,
        uc_skip: 0,
        images: ImageBudget::new(),
        truncated: false,
    };
    r.run_parser();
    r.flush_bytes();
    if !r.para.is_empty() || !r.text.is_empty() {
        r.end_paragraph(false);
    }
    r.flush_table();
    let mut out = Html::new();
    out.raw("<article class=\"paper\">");
    out.append(std::mem::replace(&mut r.body, Html::new()));
    out.raw("</article>");
    if r.truncated {
        out.raw("<p class=\"note\">This document is long; the preview stops here.</p>");
    }
    let html = html::page(Kind::Document, file_name, out, "");
    Ok(Rendered { html, title: None, pages: None })
}

impl Reader<'_> {
    fn run_parser(&mut self) {
        while self.pos < self.src.len() {
            if self.body.full() {
                self.truncated = true;
                return;
            }
            let c = self.src[self.pos];
            self.pos += 1;
            match c {
                b'{' => {
                    self.flush_bytes();
                    if self.stack.len() >= MAX_DEPTH {
                        // Skip the whole group.
                        self.skip_group();
                        continue;
                    }
                    self.stack.push(self.st.clone());
                }
                b'}' => {
                    self.flush_bytes();
                    self.end_group();
                }
                b'\\' => self.control(),
                b'\r' | b'\n' => {}
                _ => {
                    if self.st.pict.is_some() {
                        self.pict.push(c);
                    } else if self.uc_skip > 0 {
                        self.uc_skip -= 1;
                    } else {
                        self.bytes.push(c);
                    }
                }
            }
        }
    }

    fn skip_group(&mut self) {
        let mut depth = 1usize;
        while self.pos < self.src.len() && depth > 0 {
            match self.src[self.pos] {
                b'\\' => self.pos += 1,
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            self.pos += 1;
        }
    }

    fn end_group(&mut self) {
        let closing = std::mem::take(&mut self.st);
        self.st = self.stack.pop().unwrap_or(State { uc: 1, ..Default::default() });
        if closing.instr && !self.st.instr {
            self.last_instr = std::mem::take(&mut self.instr);
        }
        if let Some(ok) = closing.pict {
            if self.st.pict.is_none() {
                let hex = std::mem::take(&mut self.pict);
                if ok {
                    self.picture(&hex);
                }
            }
        }
    }

    fn picture(&mut self, hex: &[u8]) {
        let digits: Vec<u8> = hex.iter().filter_map(|c| (*c as char).to_digit(16).map(|d| d as u8)).collect();
        let bytes: Vec<u8> = digits.chunks_exact(2).map(|p| (p[0] << 4) | p[1]).collect();
        if let Some(img) = self.images.take(&bytes) {
            self.flush_run();
            let mut st = Style::new();
            if let Some(w) = self.pict_width.take() {
                st.num("width", w.min(2000.0), "px");
            }
            self.para.img(&img, "img", &st, "");
        }
    }

    fn read_word(&mut self) -> (String, Option<i64>) {
        let start = self.pos;
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_alphabetic() && self.pos - start < 32 {
            self.pos += 1;
        }
        let word = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        let pstart = self.pos;
        if self.pos < self.src.len() && self.src[self.pos] == b'-' {
            self.pos += 1;
        }
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() && self.pos - pstart < 12 {
            self.pos += 1;
        }
        let param = std::str::from_utf8(&self.src[pstart..self.pos]).ok().and_then(|s| s.parse::<i64>().ok());
        if self.pos < self.src.len() && self.src[self.pos] == b' ' {
            self.pos += 1;
        }
        (word, param)
    }

    fn control(&mut self) {
        let Some(&c) = self.src.get(self.pos) else { return };
        if !c.is_ascii_alphabetic() {
            self.pos += 1;
            match c {
                b'\'' => {
                    let hex = self.src.get(self.pos..self.pos + 2).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                    self.pos += 2;
                    if let Some(b) = hex {
                        if self.st.pict.is_some() {
                            return;
                        }
                        if self.uc_skip > 0 {
                            self.uc_skip -= 1;
                        } else {
                            self.bytes.push(b);
                        }
                    }
                }
                b'\\' | b'{' | b'}' => {
                    if self.uc_skip > 0 {
                        self.uc_skip -= 1;
                    } else {
                        self.bytes.push(c);
                    }
                }
                b'~' => self.push_str("\u{A0}"),
                b'_' => self.push_str("\u{2011}"),
                b'-' => self.push_str("\u{AD}"),
                b'*' => {
                    // Unknown destinations marked optional are skipped, except
                    // the ones whose content we want.
                    let save = self.pos;
                    let (word, _) = self.read_word_after_backslash();
                    match word.as_str() {
                        "shppict" => {}
                        "fldinst" => {
                            self.st.instr = true;
                            self.instr.clear();
                        }
                        _ => {
                            self.pos = save;
                            self.st.skip = true;
                        }
                    }
                }
                b'\r' | b'\n' => self.end_paragraph(false),
                _ => {}
            }
            return;
        }
        // Text so far belongs to the state before this control word.
        self.flush_bytes();
        let (word, param) = self.read_word();
        self.word(&word, param);
    }

    /// After `\*`: read the following `\word`, if any.
    fn read_word_after_backslash(&mut self) -> (String, Option<i64>) {
        while self.pos < self.src.len() && matches!(self.src[self.pos], b' ' | b'\r' | b'\n') {
            self.pos += 1;
        }
        if self.src.get(self.pos) == Some(&b'\\') {
            self.pos += 1;
            return self.read_word();
        }
        (String::new(), None)
    }

    fn word(&mut self, word: &str, param: Option<i64>) {
        let on = param != Some(0);
        match word {
            // Destinations we don't show.
            "fonttbl" | "stylesheet" | "info" | "header" | "headerl" | "headerr" | "headerf" | "footer" | "footerl" | "footerr" | "footerf" | "footnote"
            | "annotation" | "listtable" | "listoverridetable" | "rsidtbl" | "generator" | "xmlnstbl" | "themedata" | "colorschememapping" | "datastore"
            | "latentstyles" | "filetbl" | "revtbl" | "nonshppict" | "object" | "bkmkstart" | "bkmkend" | "private" | "userprops" | "template" | "pgdsctbl" => {
                self.flush_bytes();
                self.st.skip = true
            }
            "colortbl" => {
                self.st.colortbl = true;
                self.st.skip = true;
                self.colors.clear();
                self.cur_color = (0, 0, 0, false);
            }
            "red" if self.st.colortbl => {
                self.cur_color.0 = param.unwrap_or(0).clamp(0, 255) as u8;
                self.cur_color.3 = true;
            }
            "green" if self.st.colortbl => {
                self.cur_color.1 = param.unwrap_or(0).clamp(0, 255) as u8;
                self.cur_color.3 = true;
            }
            "blue" if self.st.colortbl => {
                self.cur_color.2 = param.unwrap_or(0).clamp(0, 255) as u8;
                self.cur_color.3 = true;
            }
            "pict" => {
                self.flush_bytes();
                self.st.pict = Some(false);
                self.pict.clear();
                self.pict_width = None;
            }
            "pngblip" | "jpegblip" if self.st.pict.is_some() => self.st.pict = Some(true),
            "picwgoal" if self.st.pict.is_some() => self.pict_width = param.map(|t| t as f64 / 15.0),
            "bin" => {
                // Raw binary data (only legal in pictures); skip it.
                let n = param.unwrap_or(0).max(0) as usize;
                self.pos = (self.pos + n).min(self.src.len());
            }
            "fldrslt" => {
                self.st.link = self.last_instr.trim_start().to_ascii_uppercase().starts_with("HYPERLINK");
            }
            "ansicpg" => {
                if let Some(e) = param.and_then(|n| Encoding::for_label(format!("windows-{n}").as_bytes()).or_else(|| Encoding::for_label(format!("cp{n}").as_bytes()))) {
                    self.enc = e;
                }
            }
            "mac" => self.enc = encoding_rs::MACINTOSH,
            "uc" => self.st.uc = param.unwrap_or(1).clamp(0, 10) as usize,
            "u" => {
                if let Some(n) = param {
                    let code = if n < 0 { n + 65536 } else { n } as u32;
                    self.flush_bytes();
                    if let Some(ch) = char::from_u32(code) {
                        self.push_str(ch.encode_utf8(&mut [0; 4]));
                    }
                    self.uc_skip = self.st.uc;
                }
            }
            "par" | "sect" => self.end_paragraph(false),
            "page" => self.end_paragraph(true),
            "line" => {
                self.flush_run();
                self.para.raw("<br>");
            }
            "tab" => self.push_str("\t"),
            "emdash" => self.push_str("\u{2014}"),
            "endash" => self.push_str("\u{2013}"),
            "bullet" => self.push_str("\u{2022}"),
            "lquote" => self.push_str("\u{2018}"),
            "rquote" => self.push_str("\u{2019}"),
            "ldblquote" => self.push_str("\u{201C}"),
            "rdblquote" => self.push_str("\u{201D}"),
            "plain" => {
                self.flush_bytes();
                let s = &mut self.st;
                (s.b, s.i, s.u, s.s, s.sup, s.sub, s.color, s.highlight) = (false, false, false, false, false, false, 0, 0);
            }
            "pard" => {
                self.flush_bytes();
                self.st.intbl = false;
                self.st.align = None;
                self.st.heading = None;
            }
            "b" => self.set(|s| s.b = on),
            "i" => self.set(|s| s.i = on),
            "ul" | "uld" | "uldb" | "ulw" | "ulwave" | "uldash" => self.set(|s| s.u = on),
            "ulnone" => self.set(|s| s.u = false),
            "strike" | "striked" => self.set(|s| s.s = on),
            "super" => self.set(|s| (s.sup, s.sub) = (true, false)),
            "sub" => self.set(|s| (s.sup, s.sub) = (false, true)),
            "nosupersub" => self.set(|s| (s.sup, s.sub) = (false, false)),
            "cf" => self.set(|s| s.color = param.unwrap_or(0).max(0) as usize),
            "highlight" | "cb" | "chcbpat" => self.set(|s| s.highlight = param.unwrap_or(0).max(0) as usize),
            "qc" => self.st.align = Some("center"),
            "qr" => self.st.align = Some("right"),
            "qj" => self.st.align = Some("justify"),
            "ql" => self.st.align = None,
            "outlinelevel" => self.st.heading = param.map(|p| (p.clamp(0, 5) + 1) as u8),
            "intbl" => self.st.intbl = true,
            "cell" | "nestcell" => {
                self.end_paragraph(false);
                let cell = std::mem::replace(&mut self.cell, Html::new());
                self.row.push(cell);
            }
            "row" | "nestrow" => {
                let row = std::mem::take(&mut self.row);
                if !row.is_empty() {
                    self.table.push(row);
                }
            }
            _ => {}
        }
    }

    fn set(&mut self, f: impl FnOnce(&mut State)) {
        self.flush_bytes();
        f(&mut self.st);
    }

    fn flush_bytes(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.bytes);
        let (s, _, _) = self.enc.decode(&bytes);
        let s = s.into_owned();
        if self.st.colortbl {
            // Entries end with ';'; an empty first entry is "auto".
            for c in s.chars() {
                if c == ';' {
                    let (r, g, b, set) = self.cur_color;
                    self.colors.push(set.then_some(Rgb(r, g, b)));
                    self.cur_color = (0, 0, 0, false);
                }
            }
            return;
        }
        self.push_str(&s);
    }

    fn push_str(&mut self, s: &str) {
        if self.st.instr {
            self.instr.push_str(s);
            return;
        }
        if self.st.skip || self.st.pict.is_some() {
            return;
        }
        let key = RunKey {
            b: self.st.b,
            i: self.st.i,
            u: self.st.u,
            s: self.st.s,
            sup: self.st.sup,
            sub: self.st.sub,
            color: self.colors.get(self.st.color).copied().flatten().filter(|_| self.st.color > 0),
            highlight: self.colors.get(self.st.highlight).copied().flatten().filter(|_| self.st.highlight > 0),
            link: self.st.link,
        };
        if self.run.as_ref() != Some(&key) {
            self.flush_run();
            self.run = Some(key);
        }
        self.text.push_str(s);
    }

    fn flush_run(&mut self) {
        let Some(k) = self.run.take() else { return };
        if self.text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.text);
        let classes = k.b || k.i || k.u || k.s || k.sup || k.sub || k.link;
        let mut st = Style::new();
        if let Some(c) = k.color {
            st.color("color", c);
        }
        if let Some(c) = k.highlight {
            st.color("background-color", c);
        }
        if !classes && st.is_empty() {
            self.para.text(&text);
            return;
        }
        self.para.raw("<span");
        if classes {
            self.para.raw(" class=\"");
            for (on, c) in [(k.b, "b "), (k.i, "i "), (k.u, "u "), (k.s, "s "), (k.sup, "sup "), (k.sub, "sub "), (k.link, "link ")] {
                if on {
                    self.para.raw(c);
                }
            }
            self.para.raw("\"");
        }
        self.para.style(&st).raw(">").text(&text).raw("</span>");
    }

    fn end_paragraph(&mut self, page_break: bool) {
        self.flush_bytes();
        self.flush_run();
        let content = std::mem::replace(&mut self.para, Html::new());
        let (open, close) = match self.st.heading {
            Some(1) => ("<h1", "</h1>"),
            Some(2) => ("<h2", "</h2>"),
            Some(3) => ("<h3", "</h3>"),
            Some(4) => ("<h4", "</h4>"),
            Some(5) => ("<h5", "</h5>"),
            Some(_) => ("<h6", "</h6>"),
            None => ("<p", "</p>"),
        };
        let mut st = Style::new();
        if let Some(a) = self.st.align {
            st.kw("text-align", a);
        }
        let target = if self.st.intbl {
            &mut self.cell
        } else {
            self.flush_table();
            &mut self.body
        };
        target.raw(open).style(&st).raw(">").append(content).raw(close);
        if page_break {
            self.body.raw("<hr class=\"page-break\">");
        }
    }

    fn flush_table(&mut self) {
        let row = std::mem::take(&mut self.row);
        if !row.is_empty() {
            self.table.push(row);
        }
        if self.table.is_empty() {
            return;
        }
        self.body.raw("<div class=\"tablewrap\"><table><tbody>");
        for row in std::mem::take(&mut self.table) {
            self.body.raw("<tr>");
            for cell in row {
                self.body.raw("<td>").append(cell).raw("</td>");
            }
            self.body.raw("</tr>");
        }
        self.body.raw("</tbody></table></div>");
    }
}
