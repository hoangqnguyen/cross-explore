//! A tiny XML tree, built with quick-xml.
//!
//! Office formats are easiest to render by walking a tree (a table cell holds
//! paragraphs that hold runs that hold drawings…), and the parts we read are
//! at most a few tens of megabytes, so a DOM is simpler than threading state
//! through a streaming parser in every renderer.
//!
//! Element and attribute names are *canonicalized*: whatever prefix a file
//! binds to a namespace, we name it by the conventional one (`w:` for
//! WordprocessingML, `a:` for DrawingML, `text:` for ODF text…). Producers
//! are free to pick their own prefixes, and the renderers then never have to
//! care. Strict OOXML namespaces map to the same prefixes as transitional.
//!
//! Safety properties that matter for untrusted files: no DTD entities are
//! ever expanded (only the five predefined ones and character references),
//! and nesting deeper than [`MAX_DEPTH`] is dropped so recursive renderers
//! cannot overflow the stack.

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::Event;
use quick_xml::name::{Namespace, QName, ResolveResult};
use quick_xml::NsReader;

/// Deeper elements (and their content) are ignored. Real documents nest a
/// few dozen levels at most.
pub(crate) const MAX_DEPTH: usize = 200;

#[derive(Debug, Clone)]
pub(crate) enum Node {
    El(El),
    Text(String),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct El {
    /// Canonical qualified name, e.g. `w:p`.
    pub name: String,
    /// Canonical qualified attribute names (unprefixed ones stay bare).
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl El {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn is(&self, name: &str) -> bool {
        self.name == name
    }

    /// Child elements, in order.
    pub fn elements(&self) -> impl Iterator<Item = &El> {
        self.children.iter().filter_map(|n| match n {
            Node::El(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    pub fn child(&self, name: &str) -> Option<&El> {
        self.elements().find(|e| e.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.elements().filter(move |e| e.name == name)
    }

    /// Follow a path of child names: `el.path(&["w:pPr", "w:pStyle"])`.
    pub fn path(&self, names: &[&str]) -> Option<&El> {
        names.iter().try_fold(self, |el, n| el.child(n))
    }

    /// First descendant (depth-first, self excluded) with this name.
    pub fn find(&self, name: &str) -> Option<&El> {
        for e in self.elements() {
            if e.name == name {
                return Some(e);
            }
            if let Some(f) = e.find(name) {
                return Some(f);
            }
        }
        None
    }

    /// All descendants with this name, depth-first, not descending into
    /// matches.
    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a El>) {
        for e in self.elements() {
            if e.name == name {
                out.push(e);
            } else {
                e.find_all(name, out);
            }
        }
    }

    /// Concatenated text of this element and its descendants.
    pub fn text(&self) -> String {
        let mut s = String::new();
        self.collect_text(&mut s);
        s
    }

    fn collect_text(&self, out: &mut String) {
        for n in &self.children {
            match n {
                Node::Text(t) => out.push_str(t),
                Node::El(e) => e.collect_text(out),
            }
        }
    }
}

/// Map a namespace URI to its conventional prefix.
fn canonical_prefix(ns: &[u8]) -> Option<&'static str> {
    const OOXML: &[(&str, &str)] = &[
        ("wordprocessingml/2006/main", "w"),
        ("wordprocessingml/main", "w"),
        ("drawingml/2006/main", "a"),
        ("drawingml/main", "a"),
        ("presentationml/2006/main", "p"),
        ("presentationml/main", "p"),
        ("officeDocument/2006/relationships", "r"),
        ("officeDocument/relationships", "r"),
        ("package/2006/relationships", "pr"),
        ("drawingml/2006/wordprocessingDrawing", "wp"),
        ("drawingml/wordprocessingDrawing", "wp"),
        ("drawingml/2006/picture", "pic"),
        ("drawingml/picture", "pic"),
        ("markup-compatibility/2006", "mc"),
        ("officeDocument/2006/extended-properties", "ep"),
        ("officeDocument/extendedProperties", "ep"),
        ("package/2006/metadata/core-properties", "cp"),
    ];
    let s = std::str::from_utf8(ns).ok()?;
    for (suffix, p) in OOXML {
        if (s.starts_with("http://schemas.openxmlformats.org/") || s.starts_with("http://purl.oclc.org/ooxml/")) && s.ends_with(suffix) {
            return Some(p);
        }
    }
    Some(match s {
        "http://schemas.microsoft.com/office/word/2010/wordprocessingShape" => "wps",
        "urn:schemas-microsoft-com:vml" => "v",
        "http://purl.org/dc/elements/1.1/" => "dc",
        "http://www.w3.org/1999/xlink" => "xlink",
        "http://www.w3.org/XML/1998/namespace" => "xml",
        "urn:oasis:names:tc:opendocument:xmlns:office:1.0" => "office",
        "urn:oasis:names:tc:opendocument:xmlns:text:1.0" => "text",
        "urn:oasis:names:tc:opendocument:xmlns:table:1.0" => "table",
        "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" => "draw",
        "urn:oasis:names:tc:opendocument:xmlns:style:1.0" => "style",
        "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" => "fo",
        "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" => "svg",
        "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" => "presentation",
        "urn:oasis:names:tc:opendocument:xmlns:meta:1.0" => "meta",
        _ => return None,
    })
}

fn qualify(res: ResolveResult<'_>, name: QName<'_>) -> String {
    let local = String::from_utf8_lossy(name.local_name().as_ref()).into_owned();
    let prefix = match res {
        ResolveResult::Bound(Namespace(ns)) => match canonical_prefix(ns) {
            Some(p) => Some(p.to_string()),
            None => name.prefix().map(|p| String::from_utf8_lossy(p.as_ref()).into_owned()),
        },
        ResolveResult::Unbound => None,
        ResolveResult::Unknown(p) => Some(String::from_utf8_lossy(&p).into_owned()),
    };
    match prefix {
        Some(p) => format!("{p}:{local}"),
        None => local,
    }
}

/// Parse `xml` into a tree whose root is a synthetic element holding the
/// document element. Errors are reported as text for the caller to wrap.
pub(crate) fn parse(xml: &[u8]) -> Result<El, String> {
    let xml = xml.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(xml);
    let mut reader = NsReader::from_reader(xml);
    let mut stack: Vec<El> = vec![El::default()];
    // >0 while inside an element nested too deeply to keep.
    let mut skipping = 0usize;
    loop {
        let pos = reader.buffer_position();
        let (res, ev) = reader.read_resolved_event().map_err(|e| format!("XML error after byte {pos}: {e}"))?;
        match ev {
            Event::Start(ref s) | Event::Empty(ref s) => {
                let empty = matches!(ev, Event::Empty(_));
                if skipping > 0 || stack.len() > MAX_DEPTH {
                    if !empty {
                        skipping += 1;
                    }
                    continue;
                }
                let name = qualify(res, s.name());
                let mut attrs = Vec::new();
                for a in s.attributes().with_checks(false).flatten() {
                    let key = a.key;
                    if key.as_ref().starts_with(b"xmlns") {
                        continue;
                    }
                    let (ares, _) = reader.resolver().resolve_attribute(key);
                    let k = qualify(ares, key);
                    let v = a
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
                    attrs.push((k, v));
                }
                let el = El { name, attrs, children: Vec::new() };
                if empty {
                    push_child(stack.last_mut(), el);
                } else {
                    stack.push(el);
                }
            }
            Event::End(_) => {
                if skipping > 0 {
                    skipping -= 1;
                    continue;
                }
                if stack.len() > 1 {
                    let el = stack.pop().unwrap_or_default();
                    push_child(stack.last_mut(), el);
                }
            }
            Event::Text(t) => {
                if skipping == 0 {
                    let s = t.decode().map(|c| c.into_owned()).unwrap_or_default();
                    push_text(stack.last_mut(), &s);
                }
            }
            Event::CData(t) => {
                if skipping == 0 {
                    let s = t.decode().map(|c| c.into_owned()).unwrap_or_default();
                    push_text(stack.last_mut(), &s);
                }
            }
            Event::GeneralRef(r) => {
                if skipping > 0 {
                    continue;
                }
                if r.is_char_ref() {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        push_text(stack.last_mut(), c.encode_utf8(&mut [0; 4]));
                    }
                } else if let Ok(name) = r.decode() {
                    // Only the predefined entities: DTD-declared ones are
                    // never expanded (no "billion laughs").
                    if let Some(s) = resolve_predefined_entity(&name) {
                        push_text(stack.last_mut(), s);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // Unclosed elements (truncated file): keep what we have.
    while stack.len() > 1 {
        let el = stack.pop().unwrap_or_default();
        push_child(stack.last_mut(), el);
    }
    Ok(stack.pop().unwrap_or_default())
}

fn push_text(parent: Option<&mut El>, s: &str) {
    let Some(parent) = parent else { return };
    if let Some(Node::Text(t)) = parent.children.last_mut() {
        t.push_str(s);
    } else {
        parent.children.push(Node::Text(s.to_string()));
    }
}

/// Append `el` to `parent`. Markup-compatibility wrappers are resolved here,
/// once, for every format: `mc:AlternateContent` is replaced by the content
/// of its first `mc:Choice` (the modern representation; its `mc:Fallback`
/// is the same thing for old readers and would render twice).
fn push_child(parent: Option<&mut El>, el: El) {
    let Some(parent) = parent else { return };
    if el.name == "mc:AlternateContent" {
        let pick = el.elements().find(|e| e.is("mc:Choice")).or_else(|| el.elements().find(|e| e.is("mc:Fallback"))).cloned();
        if let Some(p) = pick {
            parent.children.extend(p.children);
        }
        return;
    }
    parent.children.push(Node::El(el));
}

/// The document element (first element under the synthetic root).
pub(crate) fn root(tree: &El) -> Option<&El> {
    tree.elements().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_names_and_entities() {
        let x = br#"<?xml version="1.0"?><!DOCTYPE x [<!ENTITY boom "BOOM">]><q:document xmlns:q="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><q:t q:val="a&amp;b">x &lt; y &#65;&boom;</q:t></q:document>"#;
        let tree = parse(x).unwrap();
        let doc = root(&tree).unwrap();
        assert_eq!(doc.name, "w:document");
        let t = doc.child("w:t").unwrap();
        assert_eq!(t.attr("w:val"), Some("a&b"));
        assert_eq!(t.text(), "x < y A");
    }

    #[test]
    fn deep_nesting_is_cut() {
        let mut x = String::new();
        for _ in 0..5000 {
            x.push_str("<a>");
        }
        x.push_str("deep");
        for _ in 0..5000 {
            x.push_str("</a>");
        }
        let tree = parse(x.as_bytes()).unwrap();
        assert!(!tree.text().contains("deep"));
    }

    #[test]
    fn alternate_content_keeps_choice() {
        let x = br#"<r xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:AlternateContent><mc:Choice Requires="x"><new/></mc:Choice><mc:Fallback><old/></mc:Fallback></mc:AlternateContent></r>"#;
        let tree = parse(x).unwrap();
        let r = root(&tree).unwrap();
        assert!(r.child("new").is_some());
        assert!(r.child("old").is_none());
    }
}
