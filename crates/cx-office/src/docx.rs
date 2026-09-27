//! Word (WordprocessingML: .docx/.docm/.dotx/.dotm) to HTML.
//!
//! Maps the document's *structure* rather than its layout: headings (from
//! style names or outline levels), paragraphs, character formatting, lists,
//! tables with merged cells, images and page breaks. Page geometry, floating
//! positions, fonts and section columns are left to CSS; that is what makes
//! the result readable at any width.

use crate::html::{self, Html, ImageBudget, Kind, Rgb, Style};
use crate::package::{rel_by_id, Package, Rel};
use crate::xml::{self, El};
use crate::Rendered;
use cx_core::{CxError, Result};
use std::collections::HashMap;

pub(crate) fn render(bytes: Vec<u8>, file_name: &str) -> Result<Rendered> {
    let mut pkg = Package::open(bytes)?;
    let main = pkg.main_part("officeDocument").unwrap_or_else(|| "word/document.xml".into());
    let tree = pkg.xml(&main)?.ok_or_else(|| CxError::Unsupported(format!("{file_name} has no document body")))?;
    let body = xml::root(&tree).and_then(|d| d.child("w:body")).ok_or_else(|| CxError::Unsupported(format!("{file_name} is not a Word document")))?;

    let rels = pkg.rels(&main);
    let styles = rels.iter().find(|r| r.kind == "styles").and_then(|r| pkg.xml(&r.target).ok().flatten()).map(|t| Styles::parse(&t)).unwrap_or_default();
    let numbering =
        rels.iter().find(|r| r.kind == "numbering").and_then(|r| pkg.xml(&r.target).ok().flatten()).map(|t| Numbering::parse(&t)).unwrap_or_default();
    let title = pkg.core_title();
    let pages = pkg
        .xml("docProps/app.xml")
        .ok()
        .flatten()
        .and_then(|t| t.find("ep:Pages").map(|p| p.text()))
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&n| n > 0);

    let mut cx = Ctx { pkg: &mut pkg, rels, styles, numbering, images: ImageBudget::new(), fields: Vec::new(), textboxes: Vec::new(), page_break: false, truncated: false };
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

/// `w:val`-style on/off: present means on unless val is 0/false/off.
fn on(el: &El) -> bool {
    !matches!(el.attr("w:val"), Some("0" | "false" | "off" | "none"))
}

fn val<'a>(el: &'a El, child: &str) -> Option<&'a str> {
    el.child(child).and_then(|c| c.attr("w:val"))
}

// ---------------------------------------------------------------- styles

#[derive(Default)]
struct StyleDef {
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    rpr: Option<El>,
    num: Option<(String, u8)>,
}

#[derive(Default)]
struct Styles {
    map: HashMap<String, StyleDef>,
    default_para: Option<String>,
    /// Document default font size in points.
    default_size: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Heading {
    Title,
    Subtitle,
    Level(u8),
}

impl Styles {
    fn parse(tree: &El) -> Styles {
        let mut s = Styles { default_size: 11.0, ..Default::default() };
        let Some(root) = xml::root(tree) else { return s };
        if let Some(sz) = root.path(&["w:docDefaults", "w:rPrDefault", "w:rPr", "w:sz"]).and_then(|e| e.attr("w:val")).and_then(|v| v.parse::<f64>().ok()) {
            s.default_size = sz / 2.0;
        }
        for st in root.children_named("w:style") {
            let Some(id) = st.attr("w:styleId") else { continue };
            if st.attr("w:type") == Some("paragraph") && st.attr("w:default").is_some_and(|d| d == "1" || d == "true") {
                s.default_para = Some(id.to_string());
            }
            let ppr = st.child("w:pPr");
            let def = StyleDef {
                name: val(st, "w:name").unwrap_or(id).to_ascii_lowercase(),
                based_on: val(st, "w:basedOn").map(str::to_string),
                outline: ppr.and_then(|p| val(p, "w:outlineLvl")).and_then(|v| v.parse().ok()),
                rpr: st.child("w:rPr").cloned(),
                num: ppr.and_then(|p| p.child("w:numPr")).and_then(num_pr),
            };
            s.map.insert(id.to_string(), def);
        }
        s
    }

    /// The style and its ancestors, most specific first (cycle-safe).
    fn chain(&self, id: &str) -> Vec<&StyleDef> {
        let mut out = Vec::new();
        let mut cur = Some(id.to_string());
        while let Some(i) = cur {
            if out.len() > 16 {
                break;
            }
            match self.map.get(&i) {
                Some(d) => {
                    cur = d.based_on.clone();
                    out.push(d);
                }
                None => break,
            }
        }
        out
    }

    fn heading(&self, id: &str) -> Option<Heading> {
        for d in self.chain(id) {
            if d.name == "title" {
                return Some(Heading::Title);
            }
            if d.name == "subtitle" {
                return Some(Heading::Subtitle);
            }
            if let Some(n) = d.name.strip_prefix("heading ").and_then(|n| n.trim().parse::<u8>().ok()) {
                return Some(Heading::Level(n.clamp(1, 6)));
            }
            if let Some(o) = d.outline {
                if o < 9 {
                    return Some(Heading::Level((o + 1).min(6)));
                }
            }
        }
        // No styles part: Word's built-in ids are still recognizable.
        let lower = id.to_ascii_lowercase();
        if self.map.is_empty() {
            if lower == "title" {
                return Some(Heading::Title);
            }
            if let Some(n) = lower.strip_prefix("heading").and_then(|n| n.parse::<u8>().ok()) {
                return Some(Heading::Level(n.clamp(1, 6)));
            }
        }
        None
    }

    fn num(&self, id: &str) -> Option<(String, u8)> {
        self.chain(id).into_iter().find_map(|d| d.num.clone())
    }

    /// Apply a style's run properties, root ancestor first.
    fn apply_rpr(&self, id: &str, props: &mut RunProps) {
        for d in self.chain(id).into_iter().rev() {
            if let Some(r) = &d.rpr {
                props.apply(r);
            }
        }
    }
}

fn num_pr(el: &El) -> Option<(String, u8)> {
    let id = val(el, "w:numId")?.to_string();
    let lvl = val(el, "w:ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
    Some((id, lvl))
}

// ------------------------------------------------------------- numbering

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Level {
    /// `None` for bullets, else the `<ol type>`.
    ordered: Option<&'static str>,
    start: u32,
}

impl Default for Level {
    fn default() -> Self {
        Level { ordered: None, start: 1 }
    }
}

#[derive(Default)]
struct Numbering {
    abstracts: HashMap<String, HashMap<u8, Level>>,
    nums: HashMap<String, (String, HashMap<u8, Level>)>,
}

fn parse_level(l: &El) -> Level {
    let ordered = match val(l, "w:numFmt").unwrap_or("bullet") {
        "bullet" | "none" => None,
        "lowerLetter" => Some("a"),
        "upperLetter" => Some("A"),
        "lowerRoman" => Some("i"),
        "upperRoman" => Some("I"),
        _ => Some("1"),
    };
    Level { ordered, start: val(l, "w:start").and_then(|v| v.parse().ok()).unwrap_or(1) }
}

impl Numbering {
    fn parse(tree: &El) -> Numbering {
        let mut n = Numbering::default();
        let Some(root) = xml::root(tree) else { return n };
        for a in root.children_named("w:abstractNum") {
            let Some(id) = a.attr("w:abstractNumId") else { continue };
            let levels = a.children_named("w:lvl").filter_map(|l| Some((l.attr("w:ilvl")?.parse().ok()?, parse_level(l)))).collect();
            n.abstracts.insert(id.to_string(), levels);
        }
        for num in root.children_named("w:num") {
            let (Some(id), Some(abs)) = (num.attr("w:numId"), val(num, "w:abstractNumId")) else { continue };
            let overrides = num
                .children_named("w:lvlOverride")
                .filter_map(|o| {
                    let ilvl = o.attr("w:ilvl")?.parse().ok()?;
                    Some((ilvl, parse_level(o.child("w:lvl")?)))
                })
                .collect();
            n.nums.insert(id.to_string(), (abs.to_string(), overrides));
        }
        n
    }

    fn level(&self, num_id: &str, ilvl: u8) -> Level {
        let Some((abs, overrides)) = self.nums.get(num_id) else { return Level::default() };
        overrides.get(&ilvl).or_else(|| self.abstracts.get(abs).and_then(|l| l.get(&ilvl))).copied().unwrap_or_default()
    }
}

// ------------------------------------------------------------ run props

#[derive(Clone, Default, PartialEq)]
struct RunProps {
    b: bool,
    i: bool,
    u: bool,
    s: bool,
    sup: bool,
    sub: bool,
    caps: bool,
    hidden: bool,
    color: Option<Rgb>,
    highlight: Option<Rgb>,
    size: Option<f64>,
}

fn highlight_color(name: &str) -> Option<Rgb> {
    Some(match name {
        "yellow" => Rgb(255, 255, 0),
        "green" => Rgb(0, 255, 0),
        "cyan" => Rgb(0, 255, 255),
        "magenta" => Rgb(255, 0, 255),
        "blue" => Rgb(0, 0, 255),
        "red" => Rgb(255, 0, 0),
        "darkBlue" => Rgb(0, 0, 139),
        "darkCyan" => Rgb(0, 139, 139),
        "darkGreen" => Rgb(0, 100, 0),
        "darkMagenta" => Rgb(139, 0, 139),
        "darkRed" => Rgb(139, 0, 0),
        "darkYellow" => Rgb(128, 128, 0),
        "darkGray" => Rgb(169, 169, 169),
        "lightGray" => Rgb(211, 211, 211),
        "black" => Rgb(0, 0, 0),
        "white" => Rgb(255, 255, 255),
        _ => return None,
    })
}

impl RunProps {
    fn apply(&mut self, rpr: &El) {
        for e in rpr.elements() {
            match e.name.as_str() {
                "w:b" => self.b = on(e),
                "w:i" => self.i = on(e),
                "w:u" => self.u = on(e),
                "w:strike" | "w:dstrike" => self.s = on(e),
                "w:caps" => self.caps = on(e),
                "w:vanish" => self.hidden = on(e),
                "w:vertAlign" => {
                    let v = e.attr("w:val").unwrap_or("");
                    self.sup = v == "superscript";
                    self.sub = v == "subscript";
                }
                "w:color" => self.color = e.attr("w:val").and_then(Rgb::parse),
                "w:highlight" => self.highlight = e.attr("w:val").and_then(highlight_color),
                "w:shd" => {
                    if let Some(c) = e.attr("w:fill").and_then(Rgb::parse) {
                        self.highlight = Some(c);
                    }
                }
                "w:sz" => self.size = e.attr("w:val").and_then(|v| v.parse::<f64>().ok()).map(|v| v / 2.0),
                _ => {}
            }
        }
    }

    /// Open a `<span>` for these props; false when none is needed.
    fn open(&self, out: &mut Html, default_size: f64, sizes: bool) -> bool {
        let mut st = Style::new();
        if let Some(c) = self.color {
            st.color("color", c);
        }
        if let Some(c) = self.highlight {
            st.color("background-color", c);
        }
        if self.caps {
            st.kw("text-transform", "uppercase");
        }
        if sizes {
            if let Some(sz) = self.size {
                if (sz - default_size).abs() > 0.4 && default_size > 0.0 {
                    st.num("font-size", (sz / default_size).clamp(0.5, 4.0), "em");
                }
            }
        }
        let classes = self.b || self.i || self.u || self.s || self.sup || self.sub;
        if !classes && st.is_empty() {
            return false;
        }
        out.raw("<span");
        if classes {
            out.raw(" class=\"");
            for (on, c) in [(self.b, "b "), (self.i, "i "), (self.u, "u "), (self.s, "s "), (self.sup, "sup "), (self.sub, "sub ")] {
                if on {
                    out.raw(c);
                }
            }
            out.raw("\"");
        }
        out.style(&st).raw(">");
        true
    }
}

// --------------------------------------------------------------- render

struct Ctx<'a> {
    pkg: &'a mut Package,
    rels: Vec<Rel>,
    styles: Styles,
    numbering: Numbering,
    images: ImageBudget,
    /// Open complex fields: `true` once past `separate` (showing the result).
    fields: Vec<bool>,
    /// Text boxes met inside the current paragraph, rendered after it
    /// (a block can't live inside `<p>`).
    textboxes: Vec<El>,
    page_break: bool,
    truncated: bool,
}

/// Open lists: each entry is an open `<ul>`/`<ol>` holding an open `<li>`.
#[derive(Default)]
struct Lists(Vec<Option<&'static str>>);

impl Lists {
    fn item(&mut self, out: &mut Html, depth: usize, level: Level) {
        let depth = depth.min(8);
        while self.0.len() > depth + 1 {
            self.pop(out);
        }
        if self.0.len() == depth + 1 {
            if self.0.last() == Some(&level.ordered) {
                out.raw("</li><li>");
                return;
            }
            self.pop(out);
        }
        while self.0.len() < depth + 1 {
            match level.ordered {
                None => {
                    out.raw("<ul>");
                }
                Some(t) => {
                    out.raw("<ol type=\"").raw(t).raw("\"");
                    if level.start != 1 {
                        out.raw(" start=\"").int(level.start as u64).raw("\"");
                    }
                    out.raw(">");
                }
            }
            out.raw("<li>");
            self.0.push(level.ordered);
        }
    }

    fn pop(&mut self, out: &mut Html) {
        match self.0.pop() {
            Some(None) => {
                out.raw("</li></ul>");
            }
            Some(Some(_)) => {
                out.raw("</li></ol>");
            }
            None => {}
        }
    }

    fn close(&mut self, out: &mut Html) {
        while !self.0.is_empty() {
            self.pop(out);
        }
    }
}

const PAGE_BREAK: &str = "<hr class=\"page-break\">";

impl Ctx<'_> {
    /// Block-level content: the body, table cells, text boxes, content controls.
    fn blocks(&mut self, out: &mut Html, parent: &El, depth: usize) {
        let mut lists = Lists::default();
        for el in parent.elements() {
            if out.full() {
                self.truncated = true;
                break;
            }
            match el.name.as_str() {
                "w:p" => {
                    let list = self.list_of(el);
                    match list {
                        Some((ilvl, level)) => lists.item(out, ilvl as usize, level),
                        None => lists.close(out),
                    }
                    self.paragraph(out, el, depth);
                }
                "w:tbl" => {
                    lists.close(out);
                    self.table(out, el, depth);
                }
                "w:sdt" => {
                    if let Some(c) = el.child("w:sdtContent") {
                        lists.close(out);
                        self.blocks(out, c, depth + 1);
                    }
                }
                "w:customXml" | "w:ins" | "w:moveTo" | "w:sdtContent" => {
                    lists.close(out);
                    self.blocks(out, el, depth + 1);
                }
                _ => {}
            }
        }
        lists.close(out);
    }

    fn para_style(&self, p: &El) -> Option<String> {
        p.child("w:pPr").and_then(|ppr| val(ppr, "w:pStyle")).map(str::to_string).or_else(|| self.styles.default_para.clone())
    }

    fn heading_of(&self, p: &El) -> Option<Heading> {
        let ppr = p.child("w:pPr");
        if let Some(o) = ppr.and_then(|p| val(p, "w:outlineLvl")).and_then(|v| v.parse::<u8>().ok()) {
            if o < 9 {
                return Some(Heading::Level((o + 1).min(6)));
            }
        }
        self.para_style(p).and_then(|s| self.styles.heading(&s))
    }

    /// `(level, format)` when this paragraph is a list item.
    fn list_of(&self, p: &El) -> Option<(u8, Level)> {
        if self.heading_of(p).is_some() {
            return None;
        }
        let (id, ilvl) = p.child("w:pPr").and_then(|ppr| ppr.child("w:numPr")).and_then(num_pr).or_else(|| self.para_style(p).and_then(|s| self.styles.num(&s)))?;
        if id == "0" {
            return None;
        }
        Some((ilvl, self.numbering.level(&id, ilvl)))
    }

    fn paragraph(&mut self, out: &mut Html, p: &El, depth: usize) {
        let ppr = p.child("w:pPr");
        if ppr.and_then(|p| p.child("w:pageBreakBefore")).is_some_and(on) {
            out.raw(PAGE_BREAK);
        }
        let heading = self.heading_of(p);
        let (open, close) = match heading {
            Some(Heading::Title) => ("<h1 class=\"title\"", "</h1>"),
            Some(Heading::Subtitle) => ("<h2 class=\"subtitle\"", "</h2>"),
            Some(Heading::Level(1)) => ("<h1", "</h1>"),
            Some(Heading::Level(2)) => ("<h2", "</h2>"),
            Some(Heading::Level(3)) => ("<h3", "</h3>"),
            Some(Heading::Level(4)) => ("<h4", "</h4>"),
            Some(Heading::Level(5)) => ("<h5", "</h5>"),
            Some(Heading::Level(_)) => ("<h6", "</h6>"),
            None => ("<p", "</p>"),
        };
        let mut st = Style::new();
        match ppr.and_then(|p| val(p, "w:jc")) {
            Some("center") => {
                st.kw("text-align", "center");
            }
            Some("right" | "end") => {
                st.kw("text-align", "right");
            }
            Some("both" | "distribute") => {
                st.kw("text-align", "justify");
            }
            _ => {}
        }
        if let Some(fill) = ppr.and_then(|p| p.child("w:shd")).and_then(|s| s.attr("w:fill")).and_then(Rgb::parse) {
            st.color("background-color", fill);
        }
        out.raw(open).style(&st).raw(">");

        let mut base = RunProps::default();
        if let Some(s) = self.para_style(p) {
            self.styles.apply_rpr(&s, &mut base);
        }
        if heading.is_some() {
            // Heading sizes come from the stylesheet; keep the color.
            base.size = None;
            base.b = false;
        }
        self.inline(out, p, &base, heading.is_none());
        out.raw(close);

        let section_break = ppr.and_then(|p| p.child("w:sectPr")).is_some_and(|s| val(s, "w:type") != Some("continuous"));
        if std::mem::take(&mut self.page_break) || section_break {
            out.raw(PAGE_BREAK);
        }
        for tb in std::mem::take(&mut self.textboxes) {
            if depth < 8 {
                out.raw("<div class=\"textbox\">");
                self.blocks(out, &tb, depth + 1);
                out.raw("</div>");
            }
        }
    }

    fn visible(&self) -> bool {
        self.fields.iter().all(|&showing| showing)
    }

    fn inline(&mut self, out: &mut Html, el: &El, base: &RunProps, sizes: bool) {
        for c in el.elements() {
            match c.name.as_str() {
                "w:r" => self.run(out, c, base, sizes),
                "w:hyperlink" => {
                    // Text only: no navigation out of a preview.
                    out.raw("<span class=\"link\">");
                    self.inline(out, c, base, sizes);
                    out.raw("</span>");
                }
                "w:fldSimple" | "w:ins" | "w:moveTo" | "w:smartTag" | "w:customXml" | "w:sdtContent" | "w:dir" | "w:bdo" => {
                    self.inline(out, c, base, sizes)
                }
                "w:sdt" => {
                    if let Some(content) = c.child("w:sdtContent") {
                        self.inline(out, content, base, sizes);
                    }
                }
                "m:oMath" | "m:oMathPara" => {
                    out.raw("<span class=\"i\">").text(&c.text()).raw("</span>");
                }
                _ => {}
            }
        }
    }

    fn run(&mut self, out: &mut Html, r: &El, base: &RunProps, sizes: bool) {
        let mut props = base.clone();
        if let Some(rpr) = r.child("w:rPr") {
            if let Some(rs) = val(rpr, "w:rStyle") {
                self.styles.apply_rpr(rs, &mut props);
            }
            props.apply(rpr);
        }
        let default_size = self.styles.default_size;
        let mut opened = false;
        let mut ensure_open = |out: &mut Html| {
            if !opened {
                opened = true;
                props.open(out, default_size, sizes)
            } else {
                false
            }
        };
        let mut span = false;
        for c in r.elements() {
            match c.name.as_str() {
                "w:fldChar" => match c.attr("w:fldCharType") {
                    Some("begin") => self.fields.push(false),
                    Some("separate") => {
                        if let Some(last) = self.fields.last_mut() {
                            *last = true;
                        }
                    }
                    Some("end") => {
                        self.fields.pop();
                    }
                    _ => {}
                },
                "w:t" if self.visible() && !props.hidden => {
                    span |= ensure_open(out);
                    out.text(&c.text());
                }
                "w:tab" | "w:ptab" if self.visible() => {
                    out.raw("\t");
                }
                "w:br" if self.visible() => match c.attr("w:type") {
                    Some("page") => self.page_break = true,
                    Some("column") => {}
                    _ => {
                        out.raw("<br>");
                    }
                },
                "w:cr" if self.visible() => {
                    out.raw("<br>");
                }
                "w:noBreakHyphen" => {
                    out.raw("\u{2011}");
                }
                "w:softHyphen" => {
                    out.raw("\u{AD}");
                }
                "w:sym" => {
                    // Private-use code points are symbol-font glyphs; skip.
                    if let Some(ch) = c.attr("w:char").and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32) {
                        if !('\u{E000}'..='\u{F8FF}').contains(&ch) {
                            span |= ensure_open(out);
                            out.text(ch.encode_utf8(&mut [0; 4]));
                        }
                    }
                }
                "w:footnoteReference" | "w:endnoteReference" => {
                    if let Some(id) = c.attr("w:id").and_then(|v| v.parse::<u64>().ok()) {
                        out.raw("<sup>").int(id).raw("</sup>");
                    }
                }
                "w:drawing" if self.visible() => self.drawing(out, c),
                "w:pict" | "w:object" if self.visible() => self.vml(out, c),
                _ => {}
            }
        }
        if span {
            out.raw("</span>");
        }
    }

    fn drawing(&mut self, out: &mut Html, d: &El) {
        for anchor in d.elements() {
            let width_emu = anchor.child("wp:extent").and_then(|e| e.attr("cx")).and_then(|v| v.parse::<f64>().ok());
            let alt = anchor.child("wp:docPr").and_then(|p| p.attr("descr").or(p.attr("title"))).unwrap_or("").to_string();
            if let Some(tb) = anchor.find("w:txbxContent") {
                self.textboxes.push(tb.clone());
            }
            let mut blips = Vec::new();
            anchor.find_all("a:blip", &mut blips);
            for blip in blips {
                if let Some(id) = blip.attr("r:embed") {
                    self.image(out, id, width_emu.map(|w| w / 9525.0), &alt);
                }
            }
        }
    }

    /// Legacy VML pictures and embedded objects' preview images.
    fn vml(&mut self, out: &mut Html, pict: &El) {
        if let Some(tb) = pict.find("w:txbxContent") {
            self.textboxes.push(tb.clone());
        }
        let mut imgs = Vec::new();
        pict.find_all("v:imagedata", &mut imgs);
        for i in imgs {
            if let Some(id) = i.attr("r:id") {
                self.image(out, id, None, "");
            }
        }
    }

    fn image(&mut self, out: &mut Html, rid: &str, width_px: Option<f64>, alt: &str) {
        let Some(rel) = rel_by_id(&self.rels, rid).filter(|r| !r.external).cloned() else { return };
        let bytes = self.pkg.read(&rel.target).ok().flatten();
        match bytes.as_deref().and_then(|b| self.images.take(b)) {
            Some(img) => {
                let mut st = Style::new();
                if let Some(w) = width_px.filter(|w| *w >= 1.0) {
                    st.num("width", w.min(2000.0), "px");
                }
                out.img(&img, "img", &st, alt);
            }
            None => {
                out.raw("<span class=\"missing\">image</span>");
            }
        }
    }

    fn table(&mut self, out: &mut Html, tbl: &El, depth: usize) {
        if depth > 24 {
            return;
        }
        #[derive(PartialEq)]
        enum VMerge {
            None,
            Restart,
            Continue,
        }
        struct Cell<'e> {
            el: &'e El,
            col: usize,
            span: usize,
            vmerge: VMerge,
        }
        let rows: Vec<Vec<Cell>> = tbl
            .children_named("w:tr")
            .map(|tr| {
                let mut col = tr.child("w:trPr").and_then(|p| val(p, "w:gridBefore")).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                let mut cells = Vec::new();
                let mut tcs = Vec::new();
                for c in tr.elements() {
                    match c.name.as_str() {
                        "w:tc" => tcs.push(c),
                        // Content controls around cells.
                        "w:sdt" => tcs.extend(c.child("w:sdtContent").into_iter().flat_map(|s| s.children_named("w:tc"))),
                        _ => {}
                    }
                }
                for tc in tcs {
                    let pr = tc.child("w:tcPr");
                    let span = pr.and_then(|p| val(p, "w:gridSpan")).and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).clamp(1, 1000);
                    let vmerge = match pr.and_then(|p| p.child("w:vMerge")) {
                        None => VMerge::None,
                        Some(v) if v.attr("w:val") == Some("restart") => VMerge::Restart,
                        Some(_) => VMerge::Continue,
                    };
                    cells.push(Cell { el: tc, col, span, vmerge });
                    col += span;
                }
                cells
            })
            .collect();

        out.raw("<div class=\"tablewrap\"><table><tbody>");
        for (ri, row) in rows.iter().enumerate() {
            if out.full() {
                self.truncated = true;
                break;
            }
            out.raw("<tr>");
            for cell in row {
                if cell.vmerge == VMerge::Continue {
                    continue;
                }
                out.raw("<td");
                if cell.span > 1 {
                    out.raw(" colspan=\"").int(cell.span as u64).raw("\"");
                }
                if cell.vmerge == VMerge::Restart {
                    let extra = rows[ri + 1..].iter().take_while(|r| r.iter().any(|c| c.col == cell.col && c.vmerge == VMerge::Continue)).count();
                    if extra > 0 {
                        out.raw(" rowspan=\"").int(extra as u64 + 1).raw("\"");
                    }
                }
                let mut st = Style::new();
                if let Some(fill) = cell.el.path(&["w:tcPr", "w:shd"]).and_then(|s| s.attr("w:fill")).and_then(Rgb::parse) {
                    st.color("background-color", fill);
                }
                out.style(&st).raw(">");
                self.blocks(out, cell.el, depth + 1);
                out.raw("</td>");
            }
            out.raw("</tr>");
        }
        out.raw("</tbody></table></div>");
    }
}
