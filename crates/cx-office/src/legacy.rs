//! Text-only previews of legacy binary Word (.doc) and PowerPoint (.ppt)
//! files, used when LibreOffice isn't available to render them properly.
//!
//! Both are OLE compound files. For Word, the main text is reassembled from
//! the piece table ([MS-DOC] §2.8.35 `Clx`); for PowerPoint, text atoms are
//! collected per slide from the `SlideListWithText` records ([MS-PPT]
//! §2.4.14.3), falling back to text inside the slide drawings. Formatting,
//! tables and images are not reconstructed; the preview says so.

use crate::html::{self, Html, Kind};
use crate::Rendered;
use cx_core::{CxError, Result};
use std::io::{Cursor, Read};

const MAX_STREAM: u64 = 96 << 20;

fn stream(cf: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, name: &str) -> Option<Vec<u8>> {
    let s = cf.open_stream(name).ok()?;
    let mut buf = Vec::new();
    s.take(MAX_STREAM).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn open(bytes: Vec<u8>, file_name: &str) -> Result<cfb::CompoundFile<Cursor<Vec<u8>>>> {
    cfb::CompoundFile::open(Cursor::new(bytes)).map_err(|e| CxError::Unsupported(format!("{file_name} is not a legacy Office file: {e}")))
}

const TEXT_ONLY_NOTE: &str = "<p class=\"note\">Text-only preview of a legacy Office file. Install LibreOffice for a full-fidelity preview.</p>";

// ------------------------------------------------------------------ .doc

/// The main document text of a Word 97–2003 file.
fn doc_text(bytes: Vec<u8>, file_name: &str) -> Result<String> {
    let bad = || CxError::Unsupported(format!("{file_name}: unrecognized Word document structure"));
    let mut cf = open(bytes, file_name)?;
    let word = stream(&mut cf, "/WordDocument").ok_or_else(bad)?;
    if u16_at(&word, 0) != Some(0xA5EC) {
        return Err(bad());
    }
    let flags = u16_at(&word, 0x0A).ok_or_else(bad)?;
    if flags & 0x0100 != 0 {
        return Err(CxError::Unsupported(format!("{file_name} is password-protected")));
    }
    let table_name = if flags & 0x0200 != 0 { "/1Table" } else { "/0Table" };
    let table = stream(&mut cf, table_name).ok_or_else(bad)?;
    let ccp_text = u32_at(&word, 0x4C).ok_or_else(bad)? as usize;
    let fc_clx = u32_at(&word, 0x01A2).ok_or_else(bad)? as usize;
    let lcb_clx = u32_at(&word, 0x01A6).ok_or_else(bad)? as usize;
    let clx = table.get(fc_clx..fc_clx.checked_add(lcb_clx).ok_or_else(bad)?).ok_or_else(bad)?;

    // Skip Prc entries (formatting), find the PlcPcd.
    let mut i = 0usize;
    while clx.get(i) == Some(&0x01) {
        i += 3 + u16_at(clx, i + 1).ok_or_else(bad)? as usize;
    }
    if clx.get(i) != Some(&0x02) {
        return Err(bad());
    }
    let lcb = u32_at(clx, i + 1).ok_or_else(bad)? as usize;
    let plc = clx.get(i + 5..i + 5 + lcb).ok_or_else(bad)?;
    let n = lcb.saturating_sub(4) / 12;
    let mut text = String::new();
    let mut total = 0usize;
    for k in 0..n {
        let (Some(cp0), Some(cp1)) = (u32_at(plc, 4 * k), u32_at(plc, 4 * (k + 1))) else { break };
        let Some(fc) = u32_at(plc, 4 * (n + 1) + 8 * k + 2) else { break };
        let mut len = cp1.saturating_sub(cp0) as usize;
        len = len.min(ccp_text.saturating_sub(total));
        if len == 0 {
            if total >= ccp_text {
                break;
            }
            continue;
        }
        total += len;
        if fc & 0x4000_0000 != 0 {
            let off = ((fc & 0x3FFF_FFFF) / 2) as usize;
            let Some(b) = word.get(off..off + len) else { break };
            text.push_str(&encoding_rs::WINDOWS_1252.decode_without_bom_handling(b).0);
        } else {
            let off = (fc & 0x3FFF_FFFF) as usize;
            let Some(b) = word.get(off..off + 2 * len) else { break };
            let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            text.push_str(&String::from_utf16_lossy(&units));
        }
    }
    Ok(text)
}

pub(crate) fn render_doc(bytes: Vec<u8>, file_name: &str) -> Result<Rendered> {
    let text = doc_text(bytes, file_name)?;
    let mut out = Html::new();
    out.raw(TEXT_ONLY_NOTE);
    out.raw("<article class=\"paper\"><p>");
    // Field codes sit between 0x13 and 0x14 (instruction) and 0x15 (end);
    // only the result is shown.
    let mut fields: Vec<bool> = Vec::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Html| {
        out.text(run);
        run.clear();
    };
    for c in text.chars() {
        match c {
            '\u{13}' => {
                flush(&mut run, &mut out);
                fields.push(false);
            }
            '\u{14}' => {
                flush(&mut run, &mut out);
                if let Some(f) = fields.last_mut() {
                    *f = true;
                }
            }
            '\u{15}' => {
                flush(&mut run, &mut out);
                fields.pop();
            }
            _ if !fields.iter().all(|&showing| showing) => {}
            '\r' | '\u{7}' => {
                flush(&mut run, &mut out);
                out.raw("</p><p>");
            }
            '\u{b}' => {
                flush(&mut run, &mut out);
                out.raw("<br>");
            }
            '\u{c}' => {
                flush(&mut run, &mut out);
                out.raw("</p><hr class=\"page-break\"><p>");
            }
            '\u{1e}' => run.push('\u{2011}'),
            '\u{1f}' => run.push('\u{AD}'),
            c if c.is_control() && c != '\t' => {}
            c => run.push(c),
        }
        if out.full() {
            break;
        }
    }
    flush(&mut run, &mut out);
    out.raw("</p></article>");
    let html = html::page(Kind::Document, file_name, out, "");
    Ok(Rendered { html, title: None, pages: None })
}

// ------------------------------------------------------------------ .ppt

const RT_SLIDE: u16 = 0x03EE;
const RT_SLIDE_LIST_WITH_TEXT: u16 = 0x0FF0;
const RT_SLIDE_PERSIST_ATOM: u16 = 0x03F3;
const RT_TEXT_HEADER_ATOM: u16 = 0x0F9F;
const RT_TEXT_CHARS_ATOM: u16 = 0x0FA0;
const RT_TEXT_BYTES_ATOM: u16 = 0x0FA8;

#[derive(Default)]
struct PptSlide {
    title: Vec<String>,
    body: Vec<String>,
}

struct PptWalk {
    /// Slides from SlideListWithText (instance 0 = slides, not notes/masters).
    listed: Vec<PptSlide>,
    /// Slides from the drawing records, in stream order.
    drawn: Vec<PptSlide>,
    text_type: u32,
}

fn atom_text(rec_type: u16, body: &[u8]) -> Option<String> {
    match rec_type {
        RT_TEXT_CHARS_ATOM => {
            let units: Vec<u16> = body.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            Some(String::from_utf16_lossy(&units))
        }
        // "Compressed" UTF-16: the low bytes only, i.e. Latin-1.
        RT_TEXT_BYTES_ATOM => Some(body.iter().map(|&b| b as char).collect()),
        _ => None,
    }
}

impl PptWalk {
    /// Walk records in `data`. `ctx`: 1 inside slide-list instance 0,
    /// 2 inside a Slide container, 0 elsewhere.
    fn walk(&mut self, data: &[u8], ctx: u8, depth: usize) {
        if depth > 32 {
            return;
        }
        let mut i = 0usize;
        while i + 8 <= data.len() {
            let (Some(vi), Some(rt), Some(len)) = (u16_at(data, i), u16_at(data, i + 2), u32_at(data, i + 4)) else { return };
            let start = i + 8;
            let end = start.saturating_add(len as usize).min(data.len());
            let body = &data[start..end];
            let (ver, instance) = (vi & 0xF, vi >> 4);
            if ver == 0xF {
                let inner = match rt {
                    RT_SLIDE_LIST_WITH_TEXT if instance == 0 => 1,
                    RT_SLIDE_LIST_WITH_TEXT => 3, // notes / masters: ignore
                    RT_SLIDE => {
                        self.drawn.push(PptSlide::default());
                        2
                    }
                    _ => ctx,
                };
                if inner != 3 {
                    self.walk(body, inner, depth + 1);
                }
            } else if ctx == 1 && rt == RT_SLIDE_PERSIST_ATOM {
                self.listed.push(PptSlide::default());
            } else if rt == RT_TEXT_HEADER_ATOM {
                self.text_type = u32_at(body, 0).unwrap_or(1);
            } else if let Some(text) = atom_text(rt, body) {
                let slide = match ctx {
                    1 => self.listed.last_mut(),
                    2 => self.drawn.last_mut(),
                    _ => None,
                };
                if let Some(s) = slide {
                    // 0 = title, 6 = centered title.
                    if matches!(self.text_type, 0 | 6) {
                        s.title.push(text);
                    } else {
                        s.body.push(text);
                    }
                }
            }
            i = end;
        }
    }
}

pub(crate) fn render_ppt(bytes: Vec<u8>, file_name: &str) -> Result<Rendered> {
    let mut cf = open(bytes, file_name)?;
    let data = stream(&mut cf, "/PowerPoint Document").ok_or_else(|| CxError::Unsupported(format!("{file_name}: no PowerPoint document stream")))?;
    let mut w = PptWalk { listed: Vec::new(), drawn: Vec::new(), text_type: 1 };
    w.walk(&data, 0, 0);
    let has_text = |v: &[PptSlide]| v.iter().any(|s| !s.title.is_empty() || !s.body.is_empty());
    let slides = if has_text(&w.listed) || !has_text(&w.drawn) { w.listed } else { w.drawn };

    let mut out = Html::new();
    out.raw(TEXT_ONLY_NOTE);
    out.raw("<main class=\"deck\">");
    for (n, s) in slides.iter().enumerate() {
        if out.full() {
            break;
        }
        out.raw("<section class=\"slide-wrap\"><div class=\"slide-num\">").int(n as u64 + 1).raw("</div><div class=\"slide\"><div class=\"shape flow\">");
        for t in &s.title {
            out.raw("<h2 style=\"font-size:3.6cqw\">").text(t.trim()).raw("</h2>");
        }
        for t in &s.body {
            for para in t.split('\r') {
                out.raw("<p style=\"font-size:2.2cqw\">");
                for (k, line) in para.split('\u{b}').enumerate() {
                    if k > 0 {
                        out.raw("<br>");
                    }
                    out.text(line);
                }
                out.raw("</p>");
            }
        }
        out.raw("</div></div></section>");
    }
    out.raw("</main>");
    let pages = Some(slides.len() as u32);
    let html = html::page(Kind::Slides, file_name, out, "");
    Ok(Rendered { html, title: None, pages })
}
