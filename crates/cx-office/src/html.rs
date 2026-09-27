//! The only way markup is produced.
//!
//! Previews are shown in the app's web view, and their content comes from
//! arbitrary files, so sanitization is structural rather than a filter run
//! afterwards: [`Html`] accepts markup only as `&'static str` (literals
//! written in this crate), and everything that comes from a document goes
//! through a typed method that escapes it ([`Html::text`]) or validates it
//! ([`Rgb`], numbers, [`DataImage`]). There is therefore no path by which a
//! file can contribute a tag, an attribute name, an event handler, a URL or
//! a `<script>`: hyperlinks are rendered as styled text, images only as
//! `data:` URIs of sniffed raster formats. A Content-Security-Policy that
//! forbids scripts and remote loads is added on top as defense in depth.

use base64::Engine;
use std::fmt::Write;

/// Stop adding content past this much HTML (images excluded): a web view
/// chokes on much more, and nobody reads that far in a preview.
pub(crate) const MAX_HTML_BYTES: usize = 24 << 20;

/// Embedded images (raw bytes, before base64) stop here per document.
pub(crate) const MAX_IMAGE_BYTES: usize = 10 << 20;

/// A validated sRGB color. Written as `#rrggbb`, so it can never break out
/// of a style attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Parse `RRGGBB` or `#RRGGBB` (anything else, including `auto`, is None).
    pub fn parse(s: &str) -> Option<Rgb> {
        let h = s.trim().trim_start_matches('#');
        if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }
}

/// Inline style built only from validated values.
#[derive(Default, Clone)]
pub(crate) struct Style(String);

impl Style {
    pub fn new() -> Style {
        Style::default()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn kw(&mut self, prop: &'static str, value: &'static str) -> &mut Self {
        let _ = write!(self.0, "{prop}:{value};");
        self
    }

    pub fn color(&mut self, prop: &'static str, c: Rgb) -> &mut Self {
        let _ = write!(self.0, "{prop}:#{:02x}{:02x}{:02x};", c.0, c.1, c.2);
        self
    }

    /// A finite number with a fixed unit (`pt`, `%`, `cqw`, `em`…).
    pub fn num(&mut self, prop: &'static str, v: f64, unit: &'static str) -> &mut Self {
        if v.is_finite() {
            let _ = write!(self.0, "{prop}:{}{unit};", fmt_num(v));
        }
        self
    }

    /// `transform: rotate(<deg>deg)`.
    pub fn rotate(&mut self, deg: f64) -> &mut Self {
        if deg.is_finite() && deg.abs() > 0.01 {
            let _ = write!(self.0, "transform:rotate({}deg);", fmt_num(deg));
        }
        self
    }

    /// `prop: a / b` (aspect ratios).
    pub fn ratio(&mut self, prop: &'static str, a: f64, b: f64) -> &mut Self {
        if a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 {
            let _ = write!(self.0, "{prop}:{}/{};", fmt_num(a), fmt_num(b));
        }
        self
    }
}

fn fmt_num(v: f64) -> String {
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.to_string()
    }
}

/// A raster image that will be inlined as a `data:` URI.
pub(crate) struct DataImage {
    mime: &'static str,
    b64: String,
}

/// Sniff the format from magic bytes; formats a web view can't show or that
/// could carry active content (EMF/WMF/TIFF/SVG) are rejected.
pub(crate) fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"BM") && bytes.len() > 26 {
        Some("image/bmp")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Shared cap on inlined image bytes for one document.
pub(crate) struct ImageBudget {
    left: usize,
    /// Images dropped because the budget ran out (reported to the reader).
    pub skipped: usize,
}

impl ImageBudget {
    pub fn new() -> ImageBudget {
        ImageBudget { left: MAX_IMAGE_BYTES, skipped: 0 }
    }

    /// `Some` if `bytes` is a supported raster image and fits the budget.
    pub fn take(&mut self, bytes: &[u8]) -> Option<DataImage> {
        let mime = image_mime(bytes)?;
        if bytes.len() > self.left {
            self.skipped += 1;
            return None;
        }
        self.left -= bytes.len();
        Some(DataImage { mime, b64: base64::engine::general_purpose::STANDARD.encode(bytes) })
    }
}

/// HTML under construction. See the module docs for why markup is only ever
/// `&'static str`.
pub(crate) struct Html {
    buf: String,
}

impl Html {
    pub fn new() -> Html {
        Html { buf: String::with_capacity(16 << 10) }
    }

    /// Past [`MAX_HTML_BYTES`]; renderers check this between blocks.
    pub fn full(&self) -> bool {
        self.buf.len() > MAX_HTML_BYTES
    }

    pub fn raw(&mut self, markup: &'static str) -> &mut Self {
        self.buf.push_str(markup);
        self
    }

    /// Escaped text content (also safe inside a quoted attribute value).
    /// Control characters other than tab and newline are dropped.
    pub fn text(&mut self, s: &str) -> &mut Self {
        for c in s.chars() {
            match c {
                '&' => self.buf.push_str("&amp;"),
                '<' => self.buf.push_str("&lt;"),
                '>' => self.buf.push_str("&gt;"),
                '"' => self.buf.push_str("&quot;"),
                '\'' => self.buf.push_str("&#39;"),
                '\t' | '\n' => self.buf.push(c),
                c if c.is_control() => {}
                '\u{FFFE}' | '\u{FFFF}' => {}
                c => self.buf.push(c),
            }
        }
        self
    }

    pub fn int(&mut self, v: u64) -> &mut Self {
        let _ = write!(self.buf, "{v}");
        self
    }

    /// ` style="…"` when the style has anything in it.
    pub fn style(&mut self, s: &Style) -> &mut Self {
        if !s.is_empty() {
            self.buf.push_str(" style=\"");
            self.buf.push_str(&s.0);
            self.buf.push('"');
        }
        self
    }

    /// `<img>` with a data URI. `alt` is escaped text.
    pub fn img(&mut self, img: &DataImage, class: &'static str, style: &Style, alt: &str) -> &mut Self {
        self.buf.push_str("<img class=\"");
        self.buf.push_str(class);
        self.buf.push_str("\" src=\"data:");
        self.buf.push_str(img.mime);
        self.buf.push_str(";base64,");
        self.buf.push_str(&img.b64);
        self.buf.push('"');
        self.style(style);
        self.buf.push_str(" alt=\"");
        self.text(alt);
        self.buf.push_str("\">");
        self
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Append another fragment (itself built only through this API).
    pub fn append(&mut self, other: Html) -> &mut Self {
        self.buf.push_str(&other.buf);
        self
    }

    pub fn into_string(self) -> String {
        self.buf
    }
}

/// Which stylesheet a page uses.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Document,
    Sheet,
    Slides,
}

/// Wrap a rendered body into a complete, self-contained page.
/// `extra_css` must be generated by this crate (e.g. per-sheet tab rules).
pub(crate) fn page(kind: Kind, title: &str, body: Html, extra_css: &str) -> String {
    let body = body.into_string();
    let mut out = Html { buf: String::with_capacity(body.len() + 8192) };
    out.raw("<!DOCTYPE html><html><head><meta charset=\"utf-8\">");
    out.raw("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    out.raw("<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'\">");
    out.raw("<meta name=\"color-scheme\" content=\"light dark\"><title>");
    out.text(title);
    out.raw("</title><style>");
    out.raw(BASE_CSS);
    out.raw(match kind {
        Kind::Document => DOC_CSS,
        Kind::Sheet => SHEET_CSS,
        Kind::Slides => SLIDES_CSS,
    });
    let mut s = out.into_string();
    s.push_str(extra_css);
    s.push_str("</style></head><body class=\"");
    s.push_str(match kind {
        Kind::Document => "cx-doc",
        Kind::Sheet => "cx-sheet",
        Kind::Slides => "cx-slides",
    });
    s.push_str("\">");
    s.push_str(&body);
    s.push_str("</body></html>");
    s
}

/// Chrome colors adapt to the system theme; document "paper" and slides stay
/// light (like Quick Look), since their own colors were chosen for that.
const BASE_CSS: &str = r#"
:root{--bg:#eef0f3;--fg:#1d1d1f;--muted:#6e6e73;--line:#d2d2d7;--chrome:#f7f7f8;--accent:#0a66d8;--paper:#fff;--ink:#1d1d1f;--link:#0a58ca}
@media (prefers-color-scheme: dark){:root{--bg:#1c1c1e;--fg:#ececf0;--muted:#98989f;--line:#3a3a3c;--chrome:#2c2c2e;--accent:#4c9bff}}
*{box-sizing:border-box}
html,body{margin:0;padding:0}
body{background:var(--bg);color:var(--fg);font:15px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;-webkit-text-size-adjust:100%}
.note{margin:12px auto;max-width:860px;padding:8px 12px;border-radius:8px;background:var(--chrome);color:var(--muted);font-size:13px;border:1px solid var(--line)}
img{max-width:100%;height:auto}
.link{color:var(--link);text-decoration:underline}
.b{font-weight:700}.i{font-style:italic}.u{text-decoration:underline}.s{text-decoration:line-through}.u.s{text-decoration:underline line-through}
.sup{vertical-align:super;font-size:.75em}.sub{vertical-align:sub;font-size:.75em}
.missing{display:inline-block;padding:2px 8px;border:1px dashed #aaa;border-radius:4px;color:#888;font-size:12px}
"#;

const DOC_CSS: &str = r#"
.paper{background:var(--paper);color:var(--ink);max-width:860px;margin:16px auto;padding:56px 64px;border-radius:4px;box-shadow:0 1px 3px rgba(0,0,0,.12),0 4px 16px rgba(0,0,0,.06);overflow-wrap:anywhere;font-family:Georgia,"Times New Roman",serif;font-size:16px;line-height:1.55}
@media (max-width:700px){.paper{margin:0;padding:24px 18px;border-radius:0}}
.paper p,.paper h1,.paper h2,.paper h3,.paper h4,.paper h5,.paper h6{white-space:pre-wrap;tab-size:4;margin:0 0 .6em}
.paper p:empty::before{content:"\200b"}
.paper h1,.paper h2,.paper h3,.paper h4,.paper h5,.paper h6{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Arial,sans-serif;line-height:1.25;margin-top:1em}
.paper h1{font-size:1.9em}.paper h2{font-size:1.5em}.paper h3{font-size:1.25em}.paper h4{font-size:1.1em}.paper h5,.paper h6{font-size:1em}
.paper .title{font-size:2.3em;margin-top:0}.paper .subtitle{font-size:1.3em;color:#555;font-weight:400}
.paper ul,.paper ol{margin:0 0 .6em;padding-left:1.8em}.paper li>p{margin:0 0 .2em}
.paper table{border-collapse:collapse;margin:0 0 1em;max-width:100%;font-size:.95em}
.paper td,.paper th{border:1px solid #c8c8c8;padding:4px 8px;vertical-align:top}
.paper td>p:last-child{margin-bottom:0}
.paper .tablewrap{overflow-x:auto}
.paper .page-break{border:0;border-top:1px dashed #c8c8c8;margin:32px -64px;height:0}
@media (max-width:700px){.paper .page-break{margin:24px -18px}}
.paper .textbox{border-left:3px solid #d0d7e2;padding-left:10px;margin:0 0 .6em}
.paper .hdr,.paper .ftr{color:#777;font-size:.85em}
.paper blockquote{margin:0 0 .6em 1.5em}
"#;

const SHEET_CSS: &str = r#"
body.cx-sheet{font-size:13px}
.tabin{position:absolute;opacity:0;pointer-events:none}
.tabs{position:sticky;top:0;z-index:5;display:flex;gap:2px;overflow-x:auto;padding:6px 8px 0;background:var(--chrome);border-bottom:1px solid var(--line)}
.tabs label{padding:5px 14px;border:1px solid transparent;border-bottom:0;border-radius:6px 6px 0 0;color:var(--muted);cursor:pointer;white-space:nowrap;user-select:none}
.tabs label:hover{color:var(--fg)}
.panel{display:none}
.panel.only{display:block}
.grid-wrap{overflow:auto;max-width:100%;max-height:calc(100vh - 38px)}
.panel.only .grid-wrap{max-height:100vh}
table.grid{border-collapse:separate;border-spacing:0;font-variant-numeric:tabular-nums}
.grid th,.grid td{border-right:1px solid var(--line);border-bottom:1px solid var(--line);padding:2px 6px;white-space:pre;max-width:420px;overflow:hidden;text-overflow:ellipsis;height:20px}
.grid td{background:var(--bg)}
.grid thead th{position:sticky;top:0;z-index:2;background:var(--chrome);color:var(--muted);font-weight:500;text-align:center}
.grid tbody th{position:sticky;left:0;z-index:1;background:var(--chrome);color:var(--muted);font-weight:500;text-align:right;min-width:40px}
.grid thead th.corner{left:0;z-index:3}
.grid td.n{text-align:right}.grid td.e{color:#d33}.grid td.bool{text-align:center}
.empty-sheet{padding:24px;color:var(--muted)}
"#;

const SLIDES_CSS: &str = r#"
.deck{max-width:1000px;margin:0 auto;padding:16px}
.slide-wrap{margin:0 0 28px}
.slide-num{color:var(--muted);font-size:12px;margin:0 0 4px 2px}
.slide-num .hidden{margin-left:8px}
.slide{position:relative;width:100%;aspect-ratio:16/9;background:#fff;color:#000;overflow:hidden;border-radius:6px;box-shadow:0 1px 3px rgba(0,0,0,.18),0 6px 20px rgba(0,0,0,.08);container-type:inline-size;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;line-height:1.15}
.shape{position:absolute;display:flex;flex-direction:column;overflow:visible;overflow-wrap:anywhere}
.shape p{margin:0 0 .25em;white-space:pre-wrap}
.shape p:empty::before{content:"\200b"}
.shape .missing{margin:auto}
.shape ul,.shape ol{margin:0 0 .25em;padding-left:1.3em}
.shape h1,.shape h2,.shape h3,.shape h4,.shape h5,.shape h6{margin:0 0 .25em;font-size:inherit;white-space:pre-wrap}
.slide>img.bg{left:0;top:0;width:100%;height:100%;object-fit:cover}
.shape.flow{position:static;padding:2cqw 4cqw}
.shape .bu{display:inline-block;min-width:1.2em}
.shape img,.slide>img{position:absolute;object-fit:contain}
.shape table{border-collapse:collapse;width:100%;height:100%}
.shape td{border:1px solid rgba(0,0,0,.25);padding:.4cqw .8cqw;vertical-align:top}
.slide img.pic{max-width:none}
.notes{margin:8px 2px 0;color:var(--fg);font-size:13px}
.notes summary{cursor:pointer;color:var(--muted)}
.notes p{margin:4px 0;white-space:pre-wrap}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_everything_dynamic() {
        let mut h = Html::new();
        h.text("<script>alert('x')</script> & \"q\"\u{0}");
        assert_eq!(h.into_string(), "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp; &quot;q&quot;");
    }

    #[test]
    fn colors_are_validated() {
        assert_eq!(Rgb::parse("FF0000"), Some(Rgb(255, 0, 0)));
        assert_eq!(Rgb::parse("#00ff00"), Some(Rgb(0, 255, 0)));
        assert_eq!(Rgb::parse("auto"), None);
        assert_eq!(Rgb::parse("red;x:y"), None);
        let mut s = Style::new();
        s.color("color", Rgb(1, 2, 3)).num("font-size", 12.5, "pt").num("left", f64::NAN, "%");
        assert_eq!(s.0, "color:#010203;font-size:12.5pt;");
    }

    #[test]
    fn only_raster_images() {
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(image_mime(b"<svg onload=alert(1)>"), None);
        let mut b = ImageBudget { left: 10, skipped: 0 };
        assert!(b.take(b"\x89PNG\r\n\x1a\n").is_some());
        assert!(b.take(b"\x89PNG\r\n\x1a\n").is_none());
        assert_eq!(b.skipped, 1);
    }
}
