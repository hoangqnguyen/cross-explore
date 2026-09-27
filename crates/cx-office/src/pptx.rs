//! PowerPoint (PresentationML: .pptx/.pptm/.potx/.ppsx) to HTML.
//!
//! Each slide becomes a card with the deck's aspect ratio. Shapes are
//! positioned absolutely from their EMU offsets as percentages of the slide,
//! and font sizes use container-query units (`cqw`), so a card scales as a
//! whole at any width, like a picture of the slide, while text stays
//! selectable. Placeholders without their own geometry inherit it from the
//! slide layout, then the master, as PowerPoint does; theme colors are
//! resolved through the master's color map. Charts and SmartArt are shown
//! as labeled boxes.

use crate::html::{self, Html, ImageBudget, Kind, Rgb, Style};
use crate::package::{rel_by_id, Package, Rel};
use crate::xml::{self, El};
use crate::Rendered;
use cx_core::{CxError, Result};
use std::collections::HashMap;
use std::rc::Rc;

/// Decks longer than this are cut (with a note).
const MAX_SLIDES: usize = 500;

/// 13.333 × 7.5 in, PowerPoint's default 16:9.
const DEFAULT_SIZE: (f64, f64) = (12_192_000.0, 6_858_000.0);
const EMU_PER_PT: f64 = 12_700.0;

pub(crate) fn render(bytes: Vec<u8>, file_name: &str) -> Result<Rendered> {
    let mut pkg = Package::open(bytes)?;
    let main = pkg.main_part("officeDocument").unwrap_or_else(|| "ppt/presentation.xml".into());
    let tree = pkg.xml(&main)?.ok_or_else(|| CxError::Unsupported(format!("{file_name} has no presentation part")))?;
    let pres = xml::root(&tree).filter(|r| r.is("p:presentation")).ok_or_else(|| CxError::Unsupported(format!("{file_name} is not a presentation")))?;
    let size = pres
        .child("p:sldSz")
        .and_then(|s| Some((s.attr("cx")?.parse::<f64>().ok()?, s.attr("cy")?.parse::<f64>().ok()?)))
        .filter(|(w, h)| *w > 0.0 && *h > 0.0)
        .unwrap_or(DEFAULT_SIZE);
    let rels = pkg.rels(&main);
    let slides: Vec<String> = pres
        .child("p:sldIdLst")
        .map(|l| l.children_named("p:sldId").filter_map(|s| rel_by_id(&rels, s.attr("r:id")?)).filter(|r| !r.external).map(|r| r.target.clone()).collect())
        .unwrap_or_default();
    let title = pkg.core_title();

    let mut deck = Deck { pkg: &mut pkg, size, parts: HashMap::new(), images: ImageBudget::new() };
    let mut out = Html::new();
    out.raw("<main class=\"deck\">");
    let mut truncated = false;
    for (i, part) in slides.iter().enumerate() {
        if i >= MAX_SLIDES || out.full() {
            truncated = true;
            break;
        }
        deck.slide(&mut out, part, i + 1);
    }
    if slides.is_empty() {
        out.raw("<p class=\"note\">This presentation has no slides.</p>");
    }
    out.raw("</main>");
    if truncated {
        out.raw("<p class=\"note\">This presentation is long; the preview stops here.</p>");
    }
    if deck.images.skipped > 0 {
        out.raw("<p class=\"note\">Some images were left out to keep the preview small.</p>");
    }
    let html = html::page(Kind::Slides, title.as_deref().unwrap_or(file_name), out, "");
    Ok(Rendered { html, title, pages: Some(slides.len() as u32) })
}

/// A parsed slide, layout or master with its relationships.
struct Part {
    tree: El,
    rels: Vec<Rel>,
}

impl Part {
    fn root(&self) -> Option<&El> {
        xml::root(&self.tree)
    }

    fn rel(&self, kind: &str) -> Option<&Rel> {
        self.rels.iter().find(|r| r.kind == kind && !r.external)
    }
}

struct Deck<'a> {
    pkg: &'a mut Package,
    size: (f64, f64),
    /// Layouts, masters and themes are shared by many slides.
    parts: HashMap<String, Option<Rc<Part>>>,
    images: ImageBudget,
}

/// Maps child coordinates of a group to slide coordinates.
#[derive(Clone, Copy)]
struct Xf {
    ox: f64,
    oy: f64,
    sx: f64,
    sy: f64,
}

const IDENTITY: Xf = Xf { ox: 0.0, oy: 0.0, sx: 1.0, sy: 1.0 };

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    rot: f64,
}

fn xfrm(el: Option<&El>) -> Option<Rect> {
    let el = el?;
    let off = el.child("a:off")?;
    let ext = el.child("a:ext")?;
    let n = |e: &El, a: &str| e.attr(a).and_then(|v| v.parse::<f64>().ok());
    Some(Rect {
        x: n(off, "x")?,
        y: n(off, "y")?,
        w: n(ext, "cx")?,
        h: n(ext, "cy")?,
        rot: el.attr("rot").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / 60000.0,
    })
}

/// Placeholder kind, normalized so slides match their layout and master.
fn ph_kind(ph: &El) -> &'static str {
    match ph.attr("type").unwrap_or("body") {
        "title" | "ctrTitle" => "title",
        "subTitle" | "body" | "obj" => "body",
        "dt" => "dt",
        "ftr" => "ftr",
        "sldNum" => "sldNum",
        "pic" => "pic",
        "tbl" | "chart" | "dgm" | "media" | "clipArt" => "obj",
        _ => "other",
    }
}

fn placeholder(sp: &El) -> Option<&El> {
    sp.elements().find(|e| e.name.starts_with("p:nv")).and_then(|nv| nv.path(&["p:nvPr", "p:ph"]))
}

/// Theme colors plus the master's color map (bg1 → lt1…).
#[derive(Default, Clone)]
struct Colors {
    scheme: HashMap<String, Rgb>,
    map: HashMap<String, String>,
}

impl Colors {
    fn scheme(&self, name: &str) -> Option<Rgb> {
        let mapped = self.map.get(name).map(String::as_str).unwrap_or(match name {
            "bg1" => "lt1",
            "tx1" => "dk1",
            "bg2" => "lt2",
            "tx2" => "dk2",
            n => n,
        });
        self.scheme.get(mapped).copied()
    }

    /// The color of a color element (`a:srgbClr`, `a:schemeClr`…) with
    /// luminance modifiers applied.
    fn color(&self, c: &El) -> Option<Rgb> {
        let base = match c.name.as_str() {
            "a:srgbClr" => Rgb::parse(c.attr("val")?),
            "a:sysClr" => c.attr("lastClr").and_then(Rgb::parse).or(match c.attr("val") {
                Some("window") => Some(Rgb(255, 255, 255)),
                Some("windowText") => Some(Rgb(0, 0, 0)),
                _ => None,
            }),
            "a:schemeClr" => self.scheme(c.attr("val")?),
            "a:prstClr" => match c.attr("val")? {
                "black" => Some(Rgb(0, 0, 0)),
                "white" => Some(Rgb(255, 255, 255)),
                "red" => Some(Rgb(255, 0, 0)),
                "green" => Some(Rgb(0, 128, 0)),
                "blue" => Some(Rgb(0, 0, 255)),
                "yellow" => Some(Rgb(255, 255, 0)),
                "gray" | "grey" => Some(Rgb(128, 128, 128)),
                _ => None,
            },
            _ => None,
        }?;
        let pct = |n: &str| c.child(n).and_then(|e| e.attr("val")).and_then(|v| v.parse::<f64>().ok()).map(|v| v / 100_000.0);
        let (lm, lo) = (pct("a:lumMod"), pct("a:lumOff"));
        if lm.is_none() && lo.is_none() {
            return Some(base);
        }
        let (h, s, l) = to_hsl(base);
        Some(from_hsl(h, s, (l * lm.unwrap_or(1.0) + lo.unwrap_or(0.0)).clamp(0.0, 1.0)))
    }

    /// The color of the first fill child (`a:solidFill`, or the first stop
    /// of a gradient) of `el`.
    fn fill(&self, el: Option<&El>) -> Option<Rgb> {
        let el = el?;
        if let Some(f) = el.child("a:solidFill") {
            return f.elements().next().and_then(|c| self.color(c));
        }
        if let Some(g) = el.child("a:gradFill") {
            return g.find("a:gs").and_then(|gs| gs.elements().next()).and_then(|c| self.color(c));
        }
        None
    }
}

fn to_hsl(c: Rgb) -> (f64, f64, f64) {
    let (r, g, b) = (c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-9 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn from_hsl(h: f64, s: f64, l: f64) -> Rgb {
    let to = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    if s == 0.0 {
        return Rgb(to(l), to(l), to(l));
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    Rgb(to(hue(h + 1.0 / 3.0)), to(hue(h)), to(hue(h - 1.0 / 3.0)))
}

/// Text defaults for one shape: size and bullets per level.
#[derive(Clone)]
struct TextDefaults {
    /// Font size in points per level 0..9.
    sizes: [f64; 9],
    /// Bullet per level when the paragraph doesn't say (`None` = none).
    bullets: [Option<char>; 9],
    color: Option<Rgb>,
}

impl TextDefaults {
    fn plain(size: f64) -> TextDefaults {
        TextDefaults { sizes: [size; 9], bullets: [None; 9], color: None }
    }

    /// Read `a:lvlNpPr` entries of a list style (master text styles or a
    /// shape's `a:lstStyle`) over these defaults.
    fn apply_list_style(&mut self, style: Option<&El>, colors: &Colors) {
        let Some(style) = style else { return };
        for (lvl, name) in LVL_NAMES.iter().enumerate() {
            let Some(p) = style.child(name) else { continue };
            if let Some(sz) = p.child("a:defRPr").and_then(|r| r.attr("sz")).and_then(|v| v.parse::<f64>().ok()) {
                self.sizes[lvl] = sz / 100.0;
            }
            if p.child("a:buNone").is_some() {
                self.bullets[lvl] = None;
            } else if let Some(c) = p.child("a:buChar").and_then(|b| b.attr("char")).and_then(|c| c.chars().next()) {
                self.bullets[lvl] = Some(c);
            }
            if lvl == 0 {
                if let Some(c) = colors.fill(p.child("a:defRPr")) {
                    self.color = Some(c);
                }
            }
        }
    }
}

const LVL_NAMES: [&str; 9] = ["a:lvl1pPr", "a:lvl2pPr", "a:lvl3pPr", "a:lvl4pPr", "a:lvl5pPr", "a:lvl6pPr", "a:lvl7pPr", "a:lvl8pPr", "a:lvl9pPr"];

/// Everything a slide inherits from its layout and master.
struct Inherited {
    layout: Option<Rc<Part>>,
    master: Option<Rc<Part>>,
    colors: Colors,
    title: TextDefaults,
    body: TextDefaults,
    other: TextDefaults,
}

impl Inherited {
    /// Geometry and anchor of a placeholder, from the layout then master.
    fn placeholder(&self, ph: &El) -> (Option<Rect>, Option<String>) {
        let kind = ph_kind(ph);
        let idx = ph.attr("idx");
        let mut rect = None;
        let mut anchor = None;
        for (part, by_idx) in [(&self.layout, true), (&self.master, false)] {
            let Some(tree) = part.as_ref().and_then(|p| p.root()).and_then(|r| r.path(&["p:cSld", "p:spTree"])) else { continue };
            let found = tree.children_named("p:sp").find(|sp| {
                let Some(p) = placeholder(sp) else { return false };
                if by_idx && idx.is_some() && p.attr("idx") == idx {
                    return true;
                }
                ph_kind(p) == kind
            });
            if let Some(sp) = found {
                if rect.is_none() {
                    rect = xfrm(sp.path(&["p:spPr", "a:xfrm"]));
                }
                if anchor.is_none() {
                    anchor = sp.path(&["p:txBody", "a:bodyPr"]).and_then(|b| b.attr("anchor")).map(str::to_string);
                }
            }
        }
        (rect, anchor)
    }
}

impl Deck<'_> {
    fn part(&mut self, name: &str) -> Option<Rc<Part>> {
        if let Some(p) = self.parts.get(name) {
            return p.clone();
        }
        let part = self.pkg.xml(name).ok().flatten().map(|tree| {
            let rels = self.pkg.rels(name);
            Rc::new(Part { tree, rels })
        });
        self.parts.insert(name.to_string(), part.clone());
        part
    }

    fn inherited(&mut self, slide: &Part) -> Inherited {
        let layout = slide.rel("slideLayout").map(|r| r.target.clone()).and_then(|n| self.part(&n));
        let master = layout.as_ref().and_then(|l| l.rel("slideMaster")).map(|r| r.target.clone()).and_then(|n| self.part(&n));
        let theme = master.as_ref().and_then(|m| m.rel("theme")).map(|r| r.target.clone()).and_then(|n| self.part(&n));

        let mut colors = Colors::default();
        if let Some(scheme) = theme.as_ref().and_then(|t| t.root()).and_then(|r| r.find("a:clrScheme")) {
            for c in scheme.elements() {
                let name = c.name.trim_start_matches("a:").to_string();
                if let Some(rgb) = c.elements().next().and_then(|e| colors.color(e)) {
                    colors.scheme.insert(name, rgb);
                }
            }
        }
        let mroot = master.as_ref().and_then(|m| m.root());
        if let Some(map) = mroot.and_then(|r| r.child("p:clrMap")) {
            colors.map = map.attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        }
        // A layout may override the master's map.
        if let Some(ovr) = layout.as_ref().and_then(|l| l.root()).and_then(|r| r.path(&["p:clrMapOvr", "a:overrideClrMapping"])) {
            colors.map = ovr.attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        }

        let mut title = TextDefaults::plain(44.0);
        let mut body = TextDefaults { sizes: [28.0, 24.0, 20.0, 20.0, 20.0, 20.0, 20.0, 20.0, 20.0], bullets: [Some('•'); 9], color: None };
        let mut other = TextDefaults::plain(18.0);
        let styles = mroot.and_then(|r| r.child("p:txStyles"));
        title.apply_list_style(styles.and_then(|s| s.child("p:titleStyle")), &colors);
        body.apply_list_style(styles.and_then(|s| s.child("p:bodyStyle")), &colors);
        other.apply_list_style(styles.and_then(|s| s.child("p:otherStyle")), &colors);
        let tx1 = colors.scheme("tx1");
        for d in [&mut title, &mut body, &mut other] {
            d.color = d.color.or(tx1);
        }
        Inherited { layout, master, colors, title, body, other }
    }

    fn pct_x(&self, v: f64) -> f64 {
        v / self.size.0 * 100.0
    }

    fn pct_y(&self, v: f64) -> f64 {
        v / self.size.1 * 100.0
    }

    /// Points to container-query width units of the slide card.
    fn cqw(&self, pt: f64) -> f64 {
        pt * EMU_PER_PT / self.size.0 * 100.0
    }

    fn slide(&mut self, out: &mut Html, name: &str, number: usize) {
        let Some(slide) = self.part(name) else { return };
        // Slides are not shared; don't keep them alive in the cache.
        self.parts.remove(name);
        let Some(root) = slide.root() else { return };
        let inh = self.inherited(&slide);

        out.raw("<section class=\"slide-wrap\"><div class=\"slide-num\">").int(number as u64);
        if root.attr("show").is_some_and(|v| v == "0" || v == "false") {
            out.raw("<span class=\"hidden\">Hidden</span>");
        }
        out.raw("</div><div class=\"slide\"");
        let mut st = Style::new();
        st.ratio("aspect-ratio", self.size.0, self.size.1);
        // Background: slide, then layout, then master.
        let mut bg_image = None;
        for part in [Some(&slide), inh.layout.as_ref(), inh.master.as_ref()].into_iter().flatten() {
            let Some(bg) = part.root().and_then(|r| r.path(&["p:cSld", "p:bg"])) else { continue };
            if let Some(pr) = bg.child("p:bgPr") {
                if let Some(c) = inh.colors.fill(Some(pr)) {
                    st.color("background", c);
                } else if let Some(id) = pr.path(&["a:blipFill", "a:blip"]).and_then(|b| b.attr("r:embed")) {
                    bg_image = Some((part.clone(), id.to_string()));
                }
            } else if let Some(c) = bg.child("p:bgRef").and_then(|r| r.elements().next()).and_then(|c| inh.colors.color(c)) {
                st.color("background", c);
            }
            break;
        }
        if let Some(c) = inh.other.color {
            st.color("color", c);
        }
        out.style(&st).raw(">");
        if let Some((part, id)) = bg_image {
            self.picture_by_id(out, &part.rels, &id, None, "bg");
        }

        let show_master = !matches!(root.attr("showMasterSp"), Some("0" | "false"));
        if show_master {
            for part in [inh.master.clone(), inh.layout.clone()].into_iter().flatten() {
                if let Some(tree) = part.root().and_then(|r| r.path(&["p:cSld", "p:spTree"])) {
                    self.shapes(out, tree, IDENTITY, &part, &inh, true);
                }
            }
        }
        if let Some(tree) = root.path(&["p:cSld", "p:spTree"]) {
            self.shapes(out, tree, IDENTITY, &slide, &inh, false);
        }
        out.raw("</div>");
        self.notes(out, &slide);
        out.raw("</section>");
    }

    /// Render a shape tree. `decor_only` renders just the non-placeholder
    /// shapes (logos, lines of a master/layout that appear on every slide).
    fn shapes(&mut self, out: &mut Html, tree: &El, xf: Xf, part: &Part, inh: &Inherited, decor_only: bool) {
        for el in tree.elements() {
            if out.full() {
                return;
            }
            match el.name.as_str() {
                "p:sp" => {
                    let ph = placeholder(el);
                    if decor_only && ph.is_some() {
                        continue;
                    }
                    self.shape(out, el, ph, xf, inh);
                }
                "p:pic" if !(decor_only && placeholder(el).is_some()) => {
                    let rect = xfrm(el.path(&["p:spPr", "a:xfrm"])).or_else(|| placeholder(el).and_then(|p| inh.placeholder(p).0));
                    if let Some(id) = el.path(&["p:blipFill", "a:blip"]).and_then(|b| b.attr("r:embed")) {
                        let r = rect.map(|r| self.map(r, xf));
                        self.picture_by_id(out, &part.rels, id, r, "pic");
                    }
                }
                "p:graphicFrame" if !decor_only => self.frame(out, el, xf, inh),
                "p:grpSp" => {
                    let Some(g) = el.path(&["p:grpSpPr", "a:xfrm"]) else {
                        self.shapes(out, el, xf, part, inh, decor_only);
                        continue;
                    };
                    let n = |e: Option<&El>, a: &str| e.and_then(|e| e.attr(a)).and_then(|v| v.parse::<f64>().ok());
                    let (off, ext, choff, chext) = (g.child("a:off"), g.child("a:ext"), g.child("a:chOff"), g.child("a:chExt"));
                    let sx = match (n(ext, "cx"), n(chext, "cx")) {
                        (Some(a), Some(b)) if b > 0.0 => a / b,
                        _ => 1.0,
                    };
                    let sy = match (n(ext, "cy"), n(chext, "cy")) {
                        (Some(a), Some(b)) if b > 0.0 => a / b,
                        _ => 1.0,
                    };
                    let (ox, oy) = (n(off, "x").unwrap_or(0.0) - n(choff, "x").unwrap_or(0.0) * sx, n(off, "y").unwrap_or(0.0) - n(choff, "y").unwrap_or(0.0) * sy);
                    let inner = Xf { ox: xf.ox + xf.sx * ox, oy: xf.oy + xf.sy * oy, sx: xf.sx * sx, sy: xf.sy * sy };
                    self.shapes(out, el, inner, part, inh, decor_only);
                }
                _ => {}
            }
        }
    }

    fn map(&self, r: Rect, xf: Xf) -> Rect {
        Rect { x: xf.ox + r.x * xf.sx, y: xf.oy + r.y * xf.sy, w: r.w * xf.sx, h: r.h * xf.sy, rot: r.rot }
    }

    fn position(&self, st: &mut Style, r: Rect) {
        st.num("left", self.pct_x(r.x), "%").num("top", self.pct_y(r.y), "%").num("width", self.pct_x(r.w), "%").num("height", self.pct_y(r.h), "%");
        st.rotate(r.rot);
    }

    fn shape(&mut self, out: &mut Html, sp: &El, ph: Option<&El>, xf: Xf, inh: &Inherited) {
        let sppr = sp.child("p:spPr");
        let body = sp.child("p:txBody");
        let has_text = body.is_some_and(|b| !b.text().trim().is_empty());
        let fill = inh.colors.fill(sppr);
        if !has_text && fill.is_none() {
            return;
        }
        let (inherited_rect, inherited_anchor) = ph.map(|p| inh.placeholder(p)).unwrap_or((None, None));
        let kind = ph.map(ph_kind);
        let rect = xfrm(sppr.and_then(|p| p.child("a:xfrm"))).map(|r| self.map(r, xf)).or(inherited_rect).unwrap_or(match kind {
            Some("title") => Rect { x: 0.05 * self.size.0, y: 0.04 * self.size.1, w: 0.9 * self.size.0, h: 0.18 * self.size.1, rot: 0.0 },
            _ => Rect { x: 0.05 * self.size.0, y: 0.25 * self.size.1, w: 0.9 * self.size.0, h: 0.68 * self.size.1, rot: 0.0 },
        });

        let mut st = Style::new();
        self.position(&mut st, rect);
        if let Some(c) = fill {
            st.color("background", c);
        }
        if let Some(ln) = sppr.and_then(|p| p.child("a:ln")) {
            if let Some(c) = inh.colors.fill(Some(ln)) {
                st.kw("border-style", "solid").num("border-width", 1.0, "px").color("border-color", c);
            }
        }
        match sppr.and_then(|p| p.child("a:prstGeom")).and_then(|g| g.attr("prst")) {
            Some("ellipse") => {
                st.kw("border-radius", "50%");
            }
            Some("roundRect") => {
                st.num("border-radius", 1.5, "cqw");
            }
            _ => {}
        }
        let bodypr = body.and_then(|b| b.child("a:bodyPr"));
        let anchor = bodypr.and_then(|b| b.attr("anchor")).map(str::to_string).or(inherited_anchor).unwrap_or_else(|| if kind == Some("title") { "ctr".into() } else { "t".into() });
        st.kw(
            "justify-content",
            match anchor.as_str() {
                "ctr" => "center",
                "b" => "flex-end",
                _ => "flex-start",
            },
        );
        let inset = |a: &str, d: f64| bodypr.and_then(|b| b.attr(a)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
        st.num("padding-left", self.cqw(inset("lIns", 91440.0) / EMU_PER_PT), "cqw")
            .num("padding-right", self.cqw(inset("rIns", 91440.0) / EMU_PER_PT), "cqw")
            .num("padding-top", self.cqw(inset("tIns", 45720.0) / EMU_PER_PT), "cqw")
            .num("padding-bottom", self.cqw(inset("bIns", 45720.0) / EMU_PER_PT), "cqw");
        out.raw("<div class=\"shape\"").style(&st).raw(">");
        if let Some(body) = body {
            let mut defaults = match kind {
                Some("title") => inh.title.clone(),
                Some("body") => inh.body.clone(),
                Some(_) => TextDefaults { bullets: [None; 9], ..inh.other.clone() },
                None => inh.other.clone(),
            };
            if kind == Some("body") && ph.and_then(|p| p.attr("type")) == Some("subTitle") {
                defaults.bullets = [None; 9];
            }
            defaults.apply_list_style(body.child("a:lstStyle"), &inh.colors);
            let scale = bodypr
                .and_then(|b| b.child("a:normAutofit"))
                .and_then(|a| a.attr("fontScale"))
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| v / 100_000.0)
                .unwrap_or(1.0)
                .clamp(0.1, 1.0);
            self.text_body(out, body, &defaults, scale, &inh.colors);
        }
        out.raw("</div>");
    }

    fn text_body(&self, out: &mut Html, body: &El, d: &TextDefaults, scale: f64, colors: &Colors) {
        let mut counters = [0u32; 9];
        for p in body.children_named("a:p") {
            let ppr = p.child("a:pPr");
            let lvl = ppr.and_then(|p| p.attr("lvl")).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(8);
            let has_text = !p.text().trim().is_empty();
            let first_sz = p.children_named("a:r").find_map(|r| r.child("a:rPr").and_then(|r| r.attr("sz"))).or_else(|| p.child("a:endParaRPr").and_then(|r| r.attr("sz")));
            let size = first_sz.and_then(|v| v.parse::<f64>().ok()).map(|v| v / 100.0).unwrap_or(d.sizes[lvl]) * scale;

            let mut st = Style::new();
            st.num("font-size", self.cqw(size), "cqw");
            if lvl > 0 {
                st.num("margin-left", lvl as f64 * 1.4, "em");
            }
            match ppr.and_then(|p| p.attr("algn")) {
                Some("ctr") => {
                    st.kw("text-align", "center");
                }
                Some("r") => {
                    st.kw("text-align", "right");
                }
                Some("just" | "dist") => {
                    st.kw("text-align", "justify");
                }
                _ => {}
            }
            if let Some(c) = d.color {
                st.color("color", c);
            }
            out.raw("<p").style(&st).raw(">");

            // Bullet: explicit on the paragraph, else the level default.
            let autonum = ppr.and_then(|p| p.child("a:buAutoNum"));
            if has_text {
                if let Some(an) = autonum {
                    let start = an.attr("startAt").and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
                    if counters[lvl] == 0 {
                        counters[lvl] = start;
                    } else {
                        counters[lvl] += 1;
                    }
                    out.raw("<span class=\"bu\">").text(&auto_number(an.attr("type").unwrap_or("arabicPeriod"), counters[lvl])).raw("</span>");
                } else {
                    counters[lvl] = 0;
                    let bullet = if ppr.and_then(|p| p.child("a:buNone")).is_some() {
                        None
                    } else if let Some(c) = ppr.and_then(|p| p.child("a:buChar")).and_then(|b| b.attr("char")).and_then(|c| c.chars().next()) {
                        Some(c)
                    } else {
                        d.bullets[lvl]
                    };
                    if let Some(b) = bullet {
                        // Symbol-font bullets live in the private-use area.
                        let b = if ('\u{E000}'..='\u{F8FF}').contains(&b) { '•' } else { b };
                        out.raw("<span class=\"bu\">").text(b.encode_utf8(&mut [0; 4])).raw("</span>");
                    }
                }
            }
            for r in p.elements() {
                match r.name.as_str() {
                    "a:r" | "a:fld" => {
                        let open = run_open(out, r.child("a:rPr"), colors, |pt| self.cqw(pt * scale));
                        out.text(&r.child("a:t").map(|t| t.text()).unwrap_or_default());
                        if open {
                            out.raw("</span>");
                        }
                    }
                    "a:br" => {
                        out.raw("<br>");
                    }
                    _ => {}
                }
            }
            out.raw("</p>");
        }
    }

    fn picture_by_id(&mut self, out: &mut Html, rels: &[Rel], id: &str, rect: Option<Rect>, class: &'static str) {
        let Some(rel) = rel_by_id(rels, id).filter(|r| !r.external).cloned() else { return };
        let bytes = self.pkg.read(&rel.target).ok().flatten();
        let mut st = Style::new();
        if let Some(r) = rect {
            self.position(&mut st, r);
        }
        match bytes.as_deref().and_then(|b| self.images.take(b)) {
            Some(img) => {
                out.img(&img, class, &st, "");
            }
            None if class == "pic" => {
                out.raw("<div class=\"shape\"").style(&st).raw("><span class=\"missing\">image</span></div>");
            }
            None => {}
        }
    }

    /// Tables, charts, diagrams.
    fn frame(&mut self, out: &mut Html, gf: &El, xf: Xf, inh: &Inherited) {
        let Some(rect) = xfrm(gf.child("p:xfrm")).map(|r| self.map(r, xf)) else { return };
        let mut st = Style::new();
        self.position(&mut st, rect);
        out.raw("<div class=\"shape\"").style(&st).raw(">");
        let data = gf.path(&["a:graphic", "a:graphicData"]);
        if let Some(tbl) = data.and_then(|d| d.child("a:tbl")) {
            out.raw("<table>");
            for tr in tbl.children_named("a:tr") {
                out.raw("<tr>");
                for tc in tr.children_named("a:tc") {
                    if tc.attr("hMerge").is_some() || tc.attr("vMerge").is_some() {
                        continue;
                    }
                    out.raw("<td");
                    for (attr, html_attr) in [("gridSpan", " colspan=\""), ("rowSpan", " rowspan=\"")] {
                        if let Some(n) = tc.attr(attr).and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 1) {
                            out.raw(html_attr).int(n.min(1000)).raw("\"");
                        }
                    }
                    let mut cst = Style::new();
                    if let Some(c) = inh.colors.fill(tc.child("a:tcPr")) {
                        cst.color("background", c);
                    }
                    out.style(&cst).raw(">");
                    if let Some(body) = tc.child("a:txBody") {
                        self.text_body(out, body, &TextDefaults { sizes: [18.0; 9], bullets: [None; 9], color: inh.other.color }, 1.0, &inh.colors);
                    }
                    out.raw("</td>");
                }
                out.raw("</tr>");
            }
            out.raw("</table>");
        } else {
            let uri = data.and_then(|d| d.attr("uri")).unwrap_or("");
            let label = if uri.contains("chart") {
                "chart"
            } else if uri.contains("diagram") {
                "diagram"
            } else {
                "object"
            };
            out.raw("<span class=\"missing\">").raw(label).raw("</span>");
        }
        out.raw("</div>");
    }

    fn notes(&mut self, out: &mut Html, slide: &Part) {
        let Some(name) = slide.rel("notesSlide").map(|r| r.target.clone()) else { return };
        let Some(notes) = self.pkg.xml(&name).ok().flatten() else { return };
        let Some(tree) = xml::root(&notes).and_then(|r| r.path(&["p:cSld", "p:spTree"])) else { return };
        let mut paras = Vec::new();
        for sp in tree.children_named("p:sp") {
            if placeholder(sp).is_some_and(|p| p.attr("type") == Some("body")) {
                if let Some(body) = sp.child("p:txBody") {
                    paras.extend(body.children_named("a:p").map(|p| p.text()));
                }
            }
        }
        if paras.iter().all(|p| p.trim().is_empty()) {
            return;
        }
        out.raw("<details class=\"notes\"><summary>Notes</summary>");
        for p in paras {
            out.raw("<p>").text(&p).raw("</p>");
        }
        out.raw("</details>");
    }
}

/// Open a `<span>` for DrawingML run properties; false if none needed.
fn run_open(out: &mut Html, rpr: Option<&El>, colors: &Colors, cqw: impl Fn(f64) -> f64) -> bool {
    let Some(rpr) = rpr else { return false };
    let flag = |a: &str| rpr.attr(a).is_some_and(|v| v == "1" || v == "true");
    let b = flag("b");
    let i = flag("i");
    let u = rpr.attr("u").is_some_and(|v| v != "none");
    let s = rpr.attr("strike").is_some_and(|v| v != "noStrike");
    let baseline = rpr.attr("baseline").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let mut st = Style::new();
    if let Some(sz) = rpr.attr("sz").and_then(|v| v.parse::<f64>().ok()) {
        st.num("font-size", cqw(sz / 100.0), "cqw");
    }
    if let Some(c) = colors.fill(Some(rpr)) {
        st.color("color", c);
    }
    if let Some(c) = rpr.child("a:highlight").and_then(|h| h.elements().next()).and_then(|c| colors.color(c)) {
        st.color("background-color", c);
    }
    if rpr.attr("cap") == Some("all") {
        st.kw("text-transform", "uppercase");
    }
    let classes = b || i || u || s || baseline != 0;
    if !classes && st.is_empty() {
        return false;
    }
    out.raw("<span");
    if classes {
        out.raw(" class=\"");
        for (on, c) in [(b, "b "), (i, "i "), (u, "u "), (s, "s "), (baseline > 0, "sup "), (baseline < 0, "sub ")] {
            if on {
                out.raw(c);
            }
        }
        out.raw("\"");
    }
    out.style(&st).raw(">");
    true
}

/// The label of item `n` of an auto-numbered list (`arabicPeriod` → "3.").
fn auto_number(kind: &str, n: u32) -> String {
    let alpha = |n: u32, upper: bool| {
        let mut s = String::new();
        let mut n = n.max(1);
        while n > 0 {
            let r = ((n - 1) % 26) as u8;
            s.insert(0, (if upper { b'A' } else { b'a' } + r) as char);
            n = (n - 1) / 26;
        }
        s
    };
    let roman = |n: u32, upper: bool| {
        let table = [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
        let mut n = n.min(3999);
        let mut s = String::new();
        for (v, r) in table {
            while n >= v {
                s.push_str(r);
                n -= v;
            }
        }
        if upper {
            s.to_uppercase()
        } else {
            s
        }
    };
    let body = if kind.starts_with("alphaLc") {
        alpha(n, false)
    } else if kind.starts_with("alphaUc") {
        alpha(n, true)
    } else if kind.starts_with("romanLc") {
        roman(n, false)
    } else if kind.starts_with("romanUc") {
        roman(n, true)
    } else {
        n.to_string()
    };
    if kind.ends_with("ParenBoth") {
        format!("({body})")
    } else if kind.ends_with("ParenR") {
        format!("{body})")
    } else if kind.ends_with("Plain") {
        body
    } else {
        format!("{body}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbering_labels() {
        assert_eq!(auto_number("arabicPeriod", 3), "3.");
        assert_eq!(auto_number("alphaLcParenR", 2), "b)");
        assert_eq!(auto_number("romanUcPeriod", 4), "IV.");
        assert_eq!(auto_number("alphaUcParenBoth", 27), "(AA)");
    }

    #[test]
    fn luminance_modifiers() {
        let c = from_hsl(to_hsl(Rgb(68, 114, 196)).0, to_hsl(Rgb(68, 114, 196)).1, to_hsl(Rgb(68, 114, 196)).2);
        assert_eq!(c, Rgb(68, 114, 196));
    }
}
