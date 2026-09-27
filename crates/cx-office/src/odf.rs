//! OpenDocument text (.odt/.ott) and presentations (.odp/.otp) to HTML.
//! Spreadsheets (.ods) go through calamine in [`crate::sheet`].
//!
//! ODF keeps formatting in named styles (automatic ones in content.xml,
//! common ones in styles.xml) with parent chains; the few properties a
//! preview needs (weight, slant, underline, color, size, alignment, page
//! breaks, list numbering) are resolved through those chains. Presentations
//! reuse the text renderer inside absolutely positioned frames, using the
//! same slide-card layout as PowerPoint.

use crate::html::{self, Html, ImageBudget, Kind, Rgb, Style};
use crate::package::Package;
use crate::xml::{self, El, Node};
use crate::Rendered;
use cx_core::{CxError, Result};
use std::collections::HashMap;

const MAX_PAGES: usize = 500;
/// Cap on `number-rows-repeated` / `number-columns-repeated` expansion.
const MAX_REPEAT: usize = 100;

#[derive(Clone, Default)]
struct TextProps {
    b: Option<bool>,
    i: Option<bool>,
    u: Option<bool>,
    s: Option<bool>,
    color: Option<Rgb>,
    bg: Option<Rgb>,
    size: Option<f64>,
    /// +1 superscript, -1 subscript.
    pos: Option<i8>,
}

impl TextProps {
    fn overlay(&mut self, o: &TextProps) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f; } )* };
        }
        take!(b, i, u, s, color, bg, size, pos);
    }

    fn parse(tp: &El) -> TextProps {
        let mut t = TextProps::default();
        if let Some(w) = tp.attr("fo:font-weight") {
            t.b = Some(w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
        }
        if let Some(s) = tp.attr("fo:font-style") {
            t.i = Some(s == "italic" || s == "oblique");
        }
        if let Some(u) = tp.attr("style:text-underline-style") {
            t.u = Some(u != "none");
        }
        if let Some(s) = tp.attr("style:text-line-through-style") {
            t.s = Some(s != "none");
        }
        t.color = tp.attr("fo:color").and_then(Rgb::parse);
        t.bg = tp.attr("fo:background-color").and_then(Rgb::parse);
        t.size = tp.attr("fo:font-size").and_then(length_pt);
        if let Some(p) = tp.attr("style:text-position") {
            let first = p.split_whitespace().next().unwrap_or("");
            t.pos = Some(if first == "super" || first.trim_end_matches('%').parse::<f64>().is_ok_and(|v| v > 0.0) {
                1
            } else if first == "sub" || first.trim_end_matches('%').parse::<f64>().is_ok_and(|v| v < 0.0) {
                -1
            } else {
                0
            });
        }
        t
    }
}

/// An ODF length (`2.5cm`, `10mm`, `1in`, `12pt`, `2pc`, `96px`) in points.
fn length_pt(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic() || c == '%')?;
    let (num, unit) = s.split_at(split);
    let v: f64 = num.trim().parse().ok()?;
    let pt = match unit {
        "pt" => v,
        "cm" => v * 72.0 / 2.54,
        "mm" => v * 72.0 / 25.4,
        "in" | "inch" => v * 72.0,
        "pc" => v * 12.0,
        "px" => v * 0.75,
        _ => return None,
    };
    pt.is_finite().then_some(pt)
}

#[derive(Default)]
struct StyleInfo {
    parent: Option<String>,
    name: String,
    text: TextProps,
    align: Option<&'static str>,
    break_before: bool,
    fill: Option<Rgb>,
}

#[derive(Default)]
struct Styles {
    map: HashMap<String, StyleInfo>,
    /// List style name → `<ol type>` per level (None = bullets).
    lists: HashMap<String, Vec<Option<&'static str>>>,
    default_size: f64,
    /// First page layout's size in points.
    page: Option<(f64, f64)>,
}

impl Styles {
    fn load(&mut self, container: Option<&El>) {
        let Some(c) = container else { return };
        for st in c.elements() {
            match st.name.as_str() {
                "style:style" => {
                    let Some(name) = st.attr("style:name") else { continue };
                    let mut info = StyleInfo {
                        parent: st.attr("style:parent-style-name").map(str::to_string),
                        name: st.attr("style:display-name").unwrap_or(name).replace("_20_", " ").to_ascii_lowercase(),
                        ..Default::default()
                    };
                    if let Some(tp) = st.child("style:text-properties") {
                        info.text = TextProps::parse(tp);
                    }
                    if let Some(pp) = st.child("style:paragraph-properties") {
                        info.align = match pp.attr("fo:text-align") {
                            Some("center") => Some("center"),
                            Some("end" | "right") => Some("right"),
                            Some("justify") => Some("justify"),
                            _ => None,
                        };
                        info.break_before = pp.attr("fo:break-before") == Some("page");
                    }
                    for gp in [st.child("style:graphic-properties"), st.child("style:drawing-page-properties")].into_iter().flatten() {
                        if gp.attr("draw:fill") == Some("solid") {
                            info.fill = gp.attr("draw:fill-color").and_then(Rgb::parse);
                        }
                    }
                    self.map.insert(name.to_string(), info);
                }
                "text:list-style" => {
                    let Some(name) = st.attr("style:name") else { continue };
                    let mut levels = vec![None; 10];
                    for l in st.elements() {
                        let lvl = l.attr("text:level").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).clamp(1, 10) - 1;
                        levels[lvl] = if l.is("text:list-level-style-number") {
                            Some(match l.attr("style:num-format") {
                                Some("a") => "a",
                                Some("A") => "A",
                                Some("i") => "i",
                                Some("I") => "I",
                                _ => "1",
                            })
                        } else {
                            None
                        };
                    }
                    self.lists.insert(name.to_string(), levels);
                }
                "style:default-style" if st.attr("style:family") == Some("paragraph") => {
                    if let Some(sz) = st.child("style:text-properties").and_then(|t| t.attr("fo:font-size")).and_then(length_pt) {
                        self.default_size = sz;
                    }
                }
                "style:page-layout" if self.page.is_none() => {
                    let p = st.child("style:page-layout-properties");
                    let w = p.and_then(|p| p.attr("fo:page-width")).and_then(length_pt);
                    let h = p.and_then(|p| p.attr("fo:page-height")).and_then(length_pt);
                    if let (Some(w), Some(h)) = (w, h) {
                        if w > 0.0 && h > 0.0 {
                            self.page = Some((w, h));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn chain(&self, name: &str) -> Vec<&StyleInfo> {
        let mut out = Vec::new();
        let mut cur = Some(name.to_string());
        while let Some(n) = cur {
            if out.len() > 16 {
                break;
            }
            match self.map.get(&n) {
                Some(s) => {
                    cur = s.parent.clone();
                    out.push(s);
                }
                None => break,
            }
        }
        out
    }

    fn text(&self, name: Option<&str>) -> TextProps {
        let mut t = TextProps::default();
        if let Some(n) = name {
            for s in self.chain(n).into_iter().rev() {
                t.overlay(&s.text);
            }
        }
        t
    }

    fn align(&self, name: &str) -> Option<&'static str> {
        self.chain(name).into_iter().find_map(|s| s.align)
    }

    fn break_before(&self, name: &str) -> bool {
        self.chain(name).first().is_some_and(|s| s.break_before)
    }

    fn fill(&self, name: &str) -> Option<Rgb> {
        self.chain(name).into_iter().find_map(|s| s.fill)
    }

    /// Heading level for paragraph styles named like headings.
    fn heading(&self, name: &str) -> Option<u8> {
        for s in self.chain(name) {
            if s.name == "title" {
                return Some(1);
            }
            if s.name == "subtitle" {
                return Some(2);
            }
            if let Some(n) = s.name.strip_prefix("heading ").and_then(|n| n.trim().parse::<u8>().ok()) {
                return Some(n.clamp(1, 6));
            }
        }
        None
    }
}

/// How font sizes are written.
#[derive(Clone, Copy)]
enum Sizes {
    /// Relative to the document's default size (documents).
    Relative(f64),
    /// Container-query units of a slide `width_pt` wide.
    Slide(f64),
}

struct Ctx<'a> {
    pkg: &'a mut Package,
    styles: Styles,
    images: ImageBudget,
    sizes: Sizes,
    textboxes: Vec<El>,
    truncated: bool,
}

/// An opened document: package, parsed content.xml, styles, title, page count.
type Opened = (Package, El, Styles, Option<String>, Option<u32>);

fn open_package(bytes: Vec<u8>, file_name: &str) -> Result<Opened> {
    let mut pkg = Package::open(bytes)?;
    let content = pkg.xml("content.xml")?.ok_or_else(|| CxError::Unsupported(format!("{file_name} has no content.xml")))?;
    let mut styles = Styles { default_size: 12.0, ..Default::default() };
    if let Some(st) = pkg.xml("styles.xml").ok().flatten() {
        let root = xml::root(&st);
        styles.load(root.and_then(|r| r.child("office:styles")));
        styles.load(root.and_then(|r| r.child("office:automatic-styles")));
    }
    styles.load(xml::root(&content).and_then(|r| r.child("office:automatic-styles")));
    let meta = pkg.xml("meta.xml").ok().flatten();
    let title = meta.as_ref().and_then(|m| m.find("dc:title")).map(|t| t.text().trim().to_string()).filter(|t| !t.is_empty());
    let pages = meta
        .as_ref()
        .and_then(|m| m.find("meta:document-statistic"))
        .and_then(|s| s.attr("meta:page-count"))
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&n| n > 0);
    Ok((pkg, content, styles, title, pages))
}

pub(crate) fn render_text(bytes: Vec<u8>, file_name: &str) -> Result<Rendered> {
    let (mut pkg, content, styles, title, pages) = open_package(bytes, file_name)?;
    let body = xml::root(&content)
        .and_then(|r| r.child("office:body"))
        .and_then(|b| b.child("office:text"))
        .ok_or_else(|| CxError::Unsupported(format!("{file_name} is not a text document")))?;
    let base = styles.default_size;
    let mut cx = Ctx { pkg: &mut pkg, styles, images: ImageBudget::new(), sizes: Sizes::Relative(base), textboxes: Vec::new(), truncated: false };
    let mut out = Html::new();
    out.raw("<article class=\"paper\">");
    cx.blocks(&mut out, body, 0);
    out.raw("</article>");
    if cx.truncated {
        out.raw("<p class=\"note\">This document is long; the preview stops here.</p>");
    }
    if cx.images.skipped > 0 {
        out.raw("<p class=\"note\">Some images were left out to keep the preview small.</p>");
    }
    let html = html::page(Kind::Document, title.as_deref().unwrap_or(file_name), out, "");
    Ok(Rendered { html, title, pages })
}

pub(crate) fn render_presentation(bytes: Vec<u8>, file_name: &str) -> Result<Rendered> {
    let (mut pkg, content, styles, title, _) = open_package(bytes, file_name)?;
    let body = xml::root(&content)
        .and_then(|r| r.child("office:body"))
        .and_then(|b| b.child("office:presentation").or_else(|| b.child("office:drawing")))
        .ok_or_else(|| CxError::Unsupported(format!("{file_name} is not a presentation")))?;
    // 28 × 15.75 cm is Impress's default 16:9 page.
    let page = styles.page.unwrap_or((28.0 * 72.0 / 2.54, 15.75 * 72.0 / 2.54));
    let mut cx = Ctx { pkg: &mut pkg, styles, images: ImageBudget::new(), sizes: Sizes::Slide(page.0), textboxes: Vec::new(), truncated: false };
    let mut out = Html::new();
    out.raw("<main class=\"deck\">");
    let pages: Vec<&El> = body.children_named("draw:page").collect();
    for (i, pg) in pages.iter().enumerate() {
        if i >= MAX_PAGES || out.full() {
            cx.truncated = true;
            break;
        }
        cx.slide(&mut out, pg, i + 1, page);
    }
    out.raw("</main>");
    if cx.truncated {
        out.raw("<p class=\"note\">This presentation is long; the preview stops here.</p>");
    }
    let html = html::page(Kind::Slides, title.as_deref().unwrap_or(file_name), out, "");
    Ok(Rendered { html, title, pages: Some(pages.len() as u32) })
}

/// ODF collapses runs of whitespace in text content (`text:s` encodes
/// intentional extra spaces).
fn collapse_ws(s: &str, out: &mut Html) {
    let mut buf = String::with_capacity(s.len());
    let mut last_ws = false;
    for c in s.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r') {
            if !last_ws {
                buf.push(' ');
            }
            last_ws = true;
        } else {
            buf.push(c);
            last_ws = false;
        }
    }
    out.text(&buf);
}

impl Ctx<'_> {
    fn size_style(&self, st: &mut Style, pt: f64) {
        match self.sizes {
            Sizes::Relative(base) => {
                if base > 0.0 && (pt - base).abs() > 0.4 {
                    st.num("font-size", (pt / base).clamp(0.5, 4.0), "em");
                }
            }
            Sizes::Slide(width) => {
                if width > 0.0 {
                    st.num("font-size", pt / width * 100.0, "cqw");
                }
            }
        }
    }

    /// Open `tag` (a static `<p`, `<span`…) with classes and style for `t`.
    fn open(&self, out: &mut Html, tag: &'static str, t: &TextProps, align: Option<&'static str>) {
        out.raw(tag);
        let flags = [(t.b, "b "), (t.i, "i "), (t.u, "u "), (t.s, "s ")];
        let sup = t.pos == Some(1);
        let sub = t.pos == Some(-1);
        if flags.iter().any(|(f, _)| *f == Some(true)) || sup || sub {
            out.raw(" class=\"");
            for (f, c) in flags {
                if f == Some(true) {
                    out.raw(c);
                }
            }
            if sup {
                out.raw("sup ");
            }
            if sub {
                out.raw("sub ");
            }
            out.raw("\"");
        }
        let mut st = Style::new();
        if let Some(c) = t.color {
            st.color("color", c);
        }
        if let Some(c) = t.bg {
            st.color("background-color", c);
        }
        if let Some(sz) = t.size {
            self.size_style(&mut st, sz);
        }
        match align {
            Some("center") => {
                st.kw("text-align", "center");
            }
            Some("right") => {
                st.kw("text-align", "right");
            }
            Some("justify") => {
                st.kw("text-align", "justify");
            }
            _ => {}
        }
        out.style(&st).raw(">");
    }

    fn blocks(&mut self, out: &mut Html, parent: &El, depth: usize) {
        if depth > 32 {
            return;
        }
        for el in parent.elements() {
            if out.full() {
                self.truncated = true;
                return;
            }
            match el.name.as_str() {
                "text:p" | "text:h" => self.paragraph(out, el, depth),
                "text:list" => self.list(out, el, None, 0, depth),
                "table:table" => self.table(out, el, depth),
                "text:section" | "text:index-body" | "text:table-of-content" | "text:alphabetical-index" | "text:illustration-index" | "text:bibliography"
                | "text:user-index" | "text:object-index" | "text:table-index" => self.blocks(out, el, depth + 1),
                "draw:frame" | "draw:a" => {
                    out.raw("<p>");
                    self.frame(out, el);
                    out.raw("</p>");
                    self.flush_textboxes(out, depth);
                }
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, out: &mut Html, p: &El, depth: usize) {
        let style = p.attr("text:style-name");
        if style.is_some_and(|s| self.styles.break_before(s)) {
            out.raw("<hr class=\"page-break\">");
        }
        let level = if p.is("text:h") {
            Some(p.attr("text:outline-level").and_then(|v| v.parse::<u8>().ok()).unwrap_or(1).clamp(1, 6))
        } else {
            style.and_then(|s| self.styles.heading(s))
        };
        let (open, close) = match level {
            Some(1) => ("<h1", "</h1>"),
            Some(2) => ("<h2", "</h2>"),
            Some(3) => ("<h3", "</h3>"),
            Some(4) => ("<h4", "</h4>"),
            Some(5) => ("<h5", "</h5>"),
            Some(_) => ("<h6", "</h6>"),
            None => ("<p", "</p>"),
        };
        let mut props = self.styles.text(style);
        if level.is_some() {
            // Heading sizes and weight come from the stylesheet.
            props.size = None;
            props.b = None;
        }
        let align = style.and_then(|s| self.styles.align(s));
        self.open(out, open, &props, align);
        self.inline(out, p);
        out.raw(close);
        self.flush_textboxes(out, depth);
    }

    fn flush_textboxes(&mut self, out: &mut Html, depth: usize) {
        for tb in std::mem::take(&mut self.textboxes) {
            out.raw("<div class=\"textbox\">");
            self.blocks(out, &tb, depth + 1);
            out.raw("</div>");
        }
    }

    fn inline(&mut self, out: &mut Html, el: &El) {
        for n in &el.children {
            let c = match n {
                Node::Text(t) => {
                    collapse_ws(t, out);
                    continue;
                }
                Node::El(c) => c,
            };
            match c.name.as_str() {
                "text:span" => {
                    let props = self.styles.text(c.attr("text:style-name"));
                    self.open(out, "<span", &props, None);
                    self.inline(out, c);
                    out.raw("</span>");
                }
                "text:a" => {
                    out.raw("<span class=\"link\">");
                    self.inline(out, c);
                    out.raw("</span>");
                }
                "text:s" => {
                    let n = c.attr("text:c").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).min(100);
                    for _ in 0..n {
                        out.raw(" ");
                    }
                }
                "text:tab" => {
                    out.raw("\t");
                }
                "text:line-break" => {
                    out.raw("<br>");
                }
                "draw:frame" | "draw:a" => self.frame(out, c),
                "text:note" => {
                    if let Some(cit) = c.child("text:note-citation") {
                        out.raw("<sup>").text(&cit.text()).raw("</sup>");
                    }
                }
                "office:annotation" | "office:annotation-end" | "text:bookmark" | "text:bookmark-start" | "text:bookmark-end" | "text:soft-page-break"
                | "text:reference-mark" | "text:reference-mark-start" | "text:reference-mark-end" | "text:tracked-changes" => {}
                // Fields (page number, date, title…) carry their displayed value as content.
                n if n.starts_with("text:") => self.inline(out, c),
                _ => {}
            }
        }
    }

    /// Images inline; text boxes are queued until the paragraph closes.
    fn frame(&mut self, out: &mut Html, frame: &El) {
        if frame.is("draw:a") {
            for f in frame.children_named("draw:frame") {
                self.frame(out, f);
            }
            return;
        }
        if let Some(tb) = frame.child("draw:text-box") {
            self.textboxes.push(tb.clone());
            return;
        }
        let width = frame.attr("svg:width").and_then(length_pt);
        if let Some(img) = frame.child("draw:image") {
            let href = img.attr("xlink:href").unwrap_or("");
            // Only images inside the package; linked files are never fetched.
            let bytes = if href.contains(':') || href.is_empty() { None } else { self.pkg.read(href.trim_start_matches("./")).ok().flatten() };
            match bytes.as_deref().and_then(|b| self.images.take(b)) {
                Some(data) => {
                    let mut st = Style::new();
                    if let Some(w) = width.filter(|w| *w > 0.0) {
                        st.num("width", (w / 0.75).min(2000.0), "px");
                    }
                    let alt = frame.child("svg:title").map(|t| t.text()).unwrap_or_default();
                    out.img(&data, "img", &st, &alt);
                }
                None => {
                    out.raw("<span class=\"missing\">image</span>");
                }
            }
        } else if frame.child("draw:object").is_some() || frame.child("draw:object-ole").is_some() {
            out.raw("<span class=\"missing\">object</span>");
        }
    }

    fn list(&mut self, out: &mut Html, list: &El, inherited: Option<&str>, level: usize, depth: usize) {
        if depth > 32 {
            return;
        }
        let style = list.attr("text:style-name").or(inherited).map(str::to_string);
        let kind = style.as_deref().and_then(|s| self.styles.lists.get(s)).and_then(|l| l.get(level.min(9)).copied()).flatten();
        match kind {
            None => {
                out.raw("<ul>");
            }
            Some(t) => {
                out.raw("<ol type=\"").raw(t).raw("\">");
            }
        }
        for item in list.elements().filter(|e| e.is("text:list-item") || e.is("text:list-header")) {
            out.raw("<li>");
            for c in item.elements() {
                match c.name.as_str() {
                    "text:p" | "text:h" => self.paragraph(out, c, depth + 1),
                    "text:list" => self.list(out, c, style.as_deref(), level + 1, depth + 1),
                    _ => {}
                }
            }
            out.raw("</li>");
        }
        out.raw(if kind.is_some() { "</ol>" } else { "</ul>" });
    }

    fn table(&mut self, out: &mut Html, table: &El, depth: usize) {
        if depth > 24 {
            return;
        }
        fn rows<'e>(el: &'e El, out: &mut Vec<&'e El>) {
            for c in el.elements() {
                match c.name.as_str() {
                    "table:table-row" => out.push(c),
                    "table:table-header-rows" | "table:table-rows" | "table:table-row-group" => rows(c, out),
                    _ => {}
                }
            }
        }
        let mut trs = Vec::new();
        rows(table, &mut trs);
        out.raw("<div class=\"tablewrap\"><table><tbody>");
        let mut emitted = 0usize;
        for tr in trs {
            let repeat = tr.attr("table:number-rows-repeated").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).clamp(1, MAX_REPEAT);
            for _ in 0..repeat {
                emitted += 1;
                if emitted > 2000 || out.full() {
                    self.truncated = true;
                    out.raw("</tbody></table></div>");
                    return;
                }
                out.raw("<tr>");
                for tc in tr.children_named("table:table-cell") {
                    let crep = tc.attr("table:number-columns-repeated").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).clamp(1, MAX_REPEAT);
                    for _ in 0..crep {
                        out.raw("<td");
                        for (attr, html_attr) in [("table:number-columns-spanned", " colspan=\""), ("table:number-rows-spanned", " rowspan=\"")] {
                            if let Some(n) = tc.attr(attr).and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 1) {
                                out.raw(html_attr).int(n.min(1000)).raw("\"");
                            }
                        }
                        let mut st = Style::new();
                        if let Some(c) = tc.attr("table:style-name").and_then(|s| self.styles.map.get(s)).and_then(|s| s.text.bg.or(s.fill)) {
                            st.color("background-color", c);
                        }
                        out.style(&st).raw(">");
                        self.blocks(out, tc, depth + 1);
                        out.raw("</td>");
                    }
                }
                out.raw("</tr>");
            }
        }
        out.raw("</tbody></table></div>");
    }

    // ------------------------------------------------------ presentations

    fn slide(&mut self, out: &mut Html, pg: &El, number: usize, size: (f64, f64)) {
        out.raw("<section class=\"slide-wrap\"><div class=\"slide-num\">").int(number as u64).raw("</div><div class=\"slide\"");
        let mut st = Style::new();
        st.ratio("aspect-ratio", size.0, size.1);
        if let Some(c) = pg.attr("draw:style-name").and_then(|s| self.styles.fill(s)) {
            st.color("background", c);
        }
        out.style(&st).raw(">");
        self.draw_shapes(out, pg, size, 0);
        out.raw("</div>");
        if let Some(notes) = pg.child("presentation:notes") {
            let mut paras = Vec::new();
            for f in notes.children_named("draw:frame") {
                if f.attr("presentation:class") == Some("notes") {
                    let mut ps = Vec::new();
                    f.find_all("text:p", &mut ps);
                    paras.extend(ps.into_iter().map(|p| p.text()));
                }
            }
            if paras.iter().any(|p| !p.trim().is_empty()) {
                out.raw("<details class=\"notes\"><summary>Notes</summary>");
                for p in paras {
                    out.raw("<p>").text(&p).raw("</p>");
                }
                out.raw("</details>");
            }
        }
        out.raw("</section>");
    }

    fn draw_shapes(&mut self, out: &mut Html, parent: &El, size: (f64, f64), depth: usize) {
        if depth > 16 {
            return;
        }
        for el in parent.elements() {
            match el.name.as_str() {
                "draw:g" => self.draw_shapes(out, el, size, depth + 1),
                "draw:frame" | "draw:custom-shape" | "draw:rect" | "draw:ellipse" => self.draw_shape(out, el, size),
                _ => {}
            }
        }
    }

    fn draw_shape(&mut self, out: &mut Html, el: &El, size: (f64, f64)) {
        let n = |a: &str| el.attr(a).and_then(length_pt);
        let (Some(x), Some(y), Some(w), Some(h)) = (n("svg:x"), n("svg:y"), n("svg:width"), n("svg:height")) else { return };
        let class = el.attr("presentation:class");
        let style = el.attr("presentation:style-name").or(el.attr("draw:style-name"));
        let mut st = Style::new();
        st.num("left", x / size.0 * 100.0, "%").num("top", y / size.1 * 100.0, "%").num("width", w / size.0 * 100.0, "%").num("height", h / size.1 * 100.0, "%");
        if let Some(c) = style.and_then(|s| self.styles.fill(s)) {
            st.color("background", c);
        }
        if el.is("draw:ellipse") {
            st.kw("border-radius", "50%");
        }
        let is_title = matches!(class, Some("title"));
        st.kw("justify-content", if is_title || el.is("draw:custom-shape") { "center" } else { "flex-start" });
        st.num("padding", 0.8, "cqw");
        let mut props = self.styles.text(style);
        if props.size.is_none() {
            props.size = Some(match class {
                Some("title") => 40.0,
                Some("subtitle") => 28.0,
                Some("outline") => 26.0,
                _ => 18.0,
            });
        }
        // The frame's style sets the base size and color for its text.
        if let Some(sz) = props.size {
            self.size_style(&mut st, sz);
        }
        if let Some(c) = props.color {
            st.color("color", c);
        }
        let t = TextProps { b: props.b, i: props.i, ..Default::default() };
        out.raw("<div class=\"shape\"").style(&st).raw(">");
        if el.is("draw:frame") {
            if let Some(tb) = el.child("draw:text-box") {
                self.open(out, "<div", &t, None);
                self.blocks(out, tb, 1);
                out.raw("</div>");
            } else if let Some(img) = el.child("draw:image") {
                let href = img.attr("xlink:href").unwrap_or("");
                let bytes = if href.contains(':') || href.is_empty() { None } else { self.pkg.read(href.trim_start_matches("./")).ok().flatten() };
                match bytes.as_deref().and_then(|b| self.images.take(b)) {
                    Some(data) => {
                        let mut ist = Style::new();
                        ist.num("width", 100.0, "%").num("height", 100.0, "%").kw("object-fit", "contain").kw("position", "static");
                        out.img(&data, "pic", &ist, "");
                    }
                    None => {
                        out.raw("<span class=\"missing\">image</span>");
                    }
                }
            } else if el.child("draw:object").is_some() || el.child("draw:object-ole").is_some() {
                out.raw("<span class=\"missing\">object</span>");
            }
        } else {
            // Shapes hold paragraphs directly.
            self.open(out, "<div", &t, Some("center"));
            self.blocks(out, el, 1);
            out.raw("</div>");
        }
        out.raw("</div>");
    }
}

#[cfg(test)]
mod tests {
    use super::length_pt;

    #[test]
    fn lengths() {
        assert_eq!(length_pt("72pt"), Some(72.0));
        assert_eq!(length_pt("1in"), Some(72.0));
        assert!((length_pt("2.54cm").unwrap() - 72.0).abs() < 1e-9);
        assert_eq!(length_pt("50%"), None);
        assert_eq!(length_pt("x"), None);
    }
}
