//! PROPFIND request bodies and a streaming parser for the `multistatus`
//! answer.
//!
//! The parser works on the response body as it arrives, handing out one
//! `<response>` at a time, so a folder with thousands of entries starts
//! showing rows before the server has finished sending. It is namespace-aware
//! (servers use `D:`, `d:`, `lp1:` or a default namespace for `DAV:`) and
//! only trusts properties from a `propstat` whose status is 200.

use quick_xml::events::Event;
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::NsReader;
use std::time::UNIX_EPOCH;
use tokio::io::AsyncBufRead;

pub(crate) const PROPS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/><d:getlastmodified/><d:creationdate/></d:prop></d:propfind>"#;

pub(crate) const QUOTA: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:quota-available-bytes/><d:quota-used-bytes/></d:prop></d:propfind>"#;

const DAV: Namespace<'static> = Namespace("DAV:");

/// One `<response>` of a multistatus.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Resource {
    pub href: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub modified: Option<i64>,
    pub created: Option<i64>,
    pub quota_available: Option<u64>,
    pub quota_used: Option<u64>,
}

#[derive(Default)]
struct PropStat {
    res: Resource,
    ok: bool,
    has_status: bool,
}

/// Pulls `<response>`s out of a multistatus body as it streams in.
pub(crate) struct Multistatus<R> {
    xml: NsReader<R>,
    buf: Vec<u8>,
    /// Local names of the open elements ("" for non-DAV ones).
    stack: Vec<String>,
    text: String,
    current: Option<Resource>,
    ps: Option<PropStat>,
}

impl<R: AsyncBufRead + Unpin> Multistatus<R> {
    pub(crate) fn new(reader: R) -> Self {
        Multistatus { xml: NsReader::from_reader(reader), buf: Vec::new(), stack: Vec::new(), text: String::new(), current: None, ps: None }
    }

    /// The next response, or `None` at the end of the document.
    pub(crate) async fn next(&mut self) -> Result<Option<Resource>, String> {
        let Multistatus { xml, buf, stack, text, current, ps } = self;
        loop {
            buf.clear();
            let (ns, ev) = xml.read_resolved_event_into_async(buf).await.map_err(|e| format!("bad PROPFIND answer: {e}"))?;
            let is_dav = matches!(ns, ResolveResult::Bound(n) if n == DAV);
            match ev {
                Event::Start(e) => {
                    let name = if is_dav { e.local_name().as_ref().to_string() } else { String::new() };
                    text.clear();
                    match name.as_str() {
                        "response" => *current = Some(Resource::default()),
                        "propstat" => *ps = Some(PropStat::default()),
                        "collection" if stack.last().is_some_and(|s| s == "resourcetype") => {
                            if let Some(p) = ps.as_mut() {
                                p.res.is_dir = true;
                            }
                        }
                        _ => {}
                    }
                    stack.push(name);
                }
                Event::Empty(e) => {
                    if is_dav && e.local_name().as_ref() == "collection" && stack.last().is_some_and(|s| s == "resourcetype") {
                        if let Some(p) = ps.as_mut() {
                            p.res.is_dir = true;
                        }
                    }
                }
                Event::Text(t) => text.push_str(&t),
                Event::CData(t) => text.push_str(&t),
                Event::GeneralRef(r) => match r.resolve_char_ref() {
                    Ok(Some(c)) => text.push(c),
                    _ => text.push_str(quick_xml::escape::resolve_predefined_entity(&r).unwrap_or("")),
                },
                Event::End(_) => {
                    let name = stack.pop().unwrap_or_default();
                    let value = text.trim();
                    let in_prop = stack.last().is_some_and(|s| s == "prop");
                    match name.as_str() {
                        "href" if stack.last().is_some_and(|s| s == "response") => {
                            if let Some(r) = current.as_mut() {
                                r.href = value.to_string();
                            }
                        }
                        "status" if stack.last().is_some_and(|s| s == "propstat") => {
                            if let Some(p) = ps.as_mut() {
                                p.has_status = true;
                                p.ok = value.split_whitespace().nth(1).is_some_and(|c| c.starts_with('2'));
                            }
                        }
                        "getcontentlength" if in_prop => set(ps, |r| r.size = value.parse().ok()),
                        "getlastmodified" if in_prop => set(ps, |r| r.modified = parse_http_date(value)),
                        "creationdate" if in_prop => set(ps, |r| r.created = parse_rfc3339(value).or_else(|| parse_http_date(value))),
                        "quota-available-bytes" if in_prop => set(ps, |r| r.quota_available = value.parse().ok()),
                        "quota-used-bytes" if in_prop => set(ps, |r| r.quota_used = value.parse().ok()),
                        "propstat" => {
                            if let (Some(p), Some(r)) = (ps.take(), current.as_mut()) {
                                if p.ok || !p.has_status {
                                    merge(r, p.res);
                                }
                            }
                        }
                        "response" => {
                            if let Some(r) = current.take() {
                                if !r.href.is_empty() {
                                    return Ok(Some(r));
                                }
                            }
                        }
                        _ => {}
                    }
                    text.clear();
                }
                Event::Eof => return Ok(None),
                _ => {}
            }
        }
    }
}

fn set(ps: &mut Option<PropStat>, f: impl FnOnce(&mut Resource)) {
    if let Some(p) = ps.as_mut() {
        f(&mut p.res);
    }
}

fn merge(into: &mut Resource, from: Resource) {
    into.is_dir |= from.is_dir;
    into.size = into.size.or(from.size);
    into.modified = into.modified.or(from.modified);
    into.created = into.created.or(from.created);
    into.quota_available = into.quota_available.or(from.quota_available);
    into.quota_used = into.quota_used.or(from.quota_used);
}

fn parse_http_date(s: &str) -> Option<i64> {
    let t = httpdate::parse_http_date(s).ok()?;
    Some(match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    })
}

/// `2024-01-02T03:04:05Z`, with optional fraction and `±hh:mm` offset.
fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b't' || b[10] == b' ') {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    let mut i = 19;
    let mut ms = 0i64;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let frac = &s[start..i];
        ms = format!("{frac:0<3}")[..3].parse().ok()?;
    }
    let offset_min = match b.get(i) {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(&c) if c == b'+' || c == b'-' => {
            let o = num(i + 1..i + 3)? * 60 + num(i + 4..i + 6)?;
            if c == b'+' {
                o
            } else {
                -o
            }
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60;
    Some(secs * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn all(xml: &str) -> Vec<Resource> {
        let mut out = Vec::new();
        let mut ms = Multistatus::new(xml.as_bytes());
        while let Some(r) = ms.next().await.unwrap() {
            out.push(r);
        }
        out
    }

    #[tokio::test]
    async fn parses_apache_style_multistatus() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:ns0="DAV:">
<D:response xmlns:lp1="DAV:">
<D:href>/dav/</D:href>
<D:propstat><D:prop><lp1:resourcetype><D:collection/></lp1:resourcetype>
<lp1:getlastmodified>Tue, 02 Jan 2024 03:04:05 GMT</lp1:getlastmodified></D:prop>
<D:status>HTTP/1.1 200 OK</D:status></D:propstat>
</D:response>
<D:response>
<D:href>/dav/a%20b&amp;c.txt</D:href>
<D:propstat><D:prop><D:resourcetype/><D:getcontentlength>42</D:getcontentlength>
<D:creationdate>2024-01-02T03:04:05.5+01:00</D:creationdate></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
<D:propstat><D:prop><D:getlastmodified/></D:prop><D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>
</D:response>
</D:multistatus>"#;
        let r = all(xml).await;
        assert_eq!(r.len(), 2);
        assert!(r[0].is_dir);
        assert_eq!(r[0].modified, Some(1_704_164_645_000));
        assert_eq!(r[1].href, "/dav/a%20b&c.txt");
        assert!(!r[1].is_dir);
        assert_eq!(r[1].size, Some(42));
        assert_eq!(r[1].modified, None);
        assert_eq!(r[1].created, Some(1_704_164_645_000 - 3_600_000 + 500));
    }

    #[tokio::test]
    async fn default_namespace_and_absolute_hrefs() {
        let xml = r#"<multistatus xmlns="DAV:"><response><href>http://h/x/</href><propstat><prop>
<resourcetype><collection></collection></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
        let r = all(xml).await;
        assert_eq!(r.len(), 1);
        assert!(r[0].is_dir);
        assert_eq!(r[0].href, "http://h/x/");
    }

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800_000));
        assert_eq!(parse_rfc3339("nonsense"), None);
    }
}
