//! HTTP authentication for WebDAV: Basic and Digest (RFC 7616, MD5 and
//! MD5-sess with `qop=auth`), negotiated from the server's 401 challenge.
//!
//! The first request goes out without credentials; a 401 tells us which
//! scheme the server wants and we remember it, so every later request is
//! authorized up front. That matters for uploads: a streamed PUT body can't be
//! replayed after a 401, so the scheme must be known before one is sent.

use md5::{Digest, Md5};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    pub qop_auth: bool,
    pub sess: bool,
    /// Nonce count, incremented per request that reuses the nonce.
    pub nc: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Scheme {
    /// Nothing negotiated yet (or the server never asked).
    Unknown,
    Basic,
    Digest(DigestChallenge),
}

/// What a set of `WWW-Authenticate` headers offers.
#[derive(Debug, Default)]
pub(crate) struct Offer {
    pub basic: bool,
    pub digest: Option<DigestChallenge>,
    /// A Digest challenge with `stale=true`: the nonce expired, the
    /// credentials were fine.
    pub stale: bool,
}

impl Offer {
    /// Prefer Digest (the password never crosses the wire) when we can speak
    /// the offered variant.
    pub fn pick(self) -> Option<Scheme> {
        match (self.digest, self.basic) {
            (Some(d), _) => Some(Scheme::Digest(d)),
            (None, true) => Some(Scheme::Basic),
            (None, false) => None,
        }
    }
}

pub(crate) fn parse_offer<'a>(values: impl IntoIterator<Item = &'a str>) -> Offer {
    let mut offer = Offer::default();
    for value in values {
        for (scheme, params) in split_challenges(value) {
            if scheme.eq_ignore_ascii_case("basic") {
                offer.basic = true;
            } else if scheme.eq_ignore_ascii_case("digest") && offer.digest.is_none() {
                let get = |k: &str| params.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
                let algorithm = get("algorithm").unwrap_or_else(|| "MD5".into());
                let sess = algorithm.eq_ignore_ascii_case("MD5-sess");
                if !(algorithm.eq_ignore_ascii_case("MD5") || sess) {
                    continue; // SHA-256 etc.: fall back to another challenge
                }
                let qop = get("qop");
                let qop_auth = qop.as_deref().is_some_and(|q| q.split(',').any(|t| t.trim().eq_ignore_ascii_case("auth")));
                if qop.is_some() && !qop_auth {
                    continue; // auth-int only
                }
                let Some(nonce) = get("nonce") else { continue };
                offer.stale |= get("stale").is_some_and(|s| s.eq_ignore_ascii_case("true"));
                offer.digest = Some(DigestChallenge { realm: get("realm").unwrap_or_default(), nonce, opaque: get("opaque"), qop_auth, sess, nc: 0 });
            }
        }
    }
    offer
}

/// Split one header value into `(scheme, params)` challenges. A header may
/// carry several (`Basic realm="a", Digest realm="b", nonce="c"`), and
/// quoted strings may contain commas.
fn split_challenges(value: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut chars = value.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        let mut token = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == ',' || c == '=' {
                break;
            }
            token.push(c);
            chars.next();
        }
        if token.is_empty() {
            break;
        }
        while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
            chars.next();
        }
        if chars.peek() == Some(&'=') {
            chars.next();
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            let mut val = String::new();
            if chars.peek() == Some(&'"') {
                chars.next();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(n) = chars.next() {
                                val.push(n);
                            }
                        }
                        '"' => break,
                        c => val.push(c),
                    }
                }
            } else {
                while let Some(&c) = chars.peek() {
                    if c == ',' || c.is_whitespace() {
                        break;
                    }
                    val.push(c);
                    chars.next();
                }
            }
            // A param before any scheme is malformed: ignore it.
            if let Some((_, params)) = out.last_mut() {
                params.push((token, val));
            }
        } else {
            out.push((token, Vec::new()));
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn md5_hex(s: &str) -> String {
    hex(&Md5::digest(s.as_bytes()))
}

fn cnonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    md5_hex(&format!("{t}:{}:{:p}", COUNTER.fetch_add(1, Ordering::Relaxed), &COUNTER))[..16].to_string()
}

fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The `Authorization` header value for one request. `uri` is the request
/// target (path and query) exactly as sent.
pub(crate) fn authorization(scheme: &mut Scheme, user: &str, password: &str, method: &str, uri: &str) -> Option<String> {
    match scheme {
        Scheme::Unknown => None,
        Scheme::Basic => Some(format!("Basic {}", base64(format!("{user}:{password}").as_bytes()))),
        Scheme::Digest(ch) => {
            ch.nc += 1;
            let nc = format!("{:08x}", ch.nc);
            let cnonce = cnonce();
            let mut ha1 = md5_hex(&format!("{user}:{}:{password}", ch.realm));
            if ch.sess {
                ha1 = md5_hex(&format!("{ha1}:{}:{cnonce}", ch.nonce));
            }
            let ha2 = md5_hex(&format!("{method}:{uri}"));
            let response = if ch.qop_auth {
                md5_hex(&format!("{ha1}:{}:{nc}:{cnonce}:auth:{ha2}", ch.nonce))
            } else {
                md5_hex(&format!("{ha1}:{}:{ha2}", ch.nonce))
            };
            let mut h = format!(
                "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{response}\", algorithm={}",
                quote(user),
                quote(&ch.realm),
                quote(&ch.nonce),
                quote(uri),
                if ch.sess { "MD5-sess" } else { "MD5" },
            );
            if ch.qop_auth {
                h.push_str(&format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\""));
            }
            if let Some(o) = &ch.opaque {
                h.push_str(&format!(", opaque=\"{}\"", quote(o)));
            }
            Some(h)
        }
    }
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_challenges() {
        let o = parse_offer([r#"Basic realm="files", Digest realm="a, b", nonce="n1", qop="auth,auth-int", opaque="op", algorithm=MD5"#]);
        assert!(o.basic);
        let d = o.digest.unwrap();
        assert_eq!(d.realm, "a, b");
        assert_eq!(d.nonce, "n1");
        assert_eq!(d.opaque.as_deref(), Some("op"));
        assert!(d.qop_auth && !d.sess);
    }

    #[test]
    fn skips_unsupported_digest_algorithms() {
        let o = parse_offer([r#"Digest realm="r", nonce="n", algorithm=SHA-256"#, r#"Basic realm="r""#]);
        assert!(o.digest.is_none());
        assert_eq!(o.pick(), Some(Scheme::Basic));
    }

    #[test]
    fn rfc2617_digest_example() {
        // The worked example from RFC 2617 § 3.5.
        let mut ch = DigestChallenge {
            realm: "testrealm@host.com".into(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".into(),
            opaque: Some("5ccc069c403ebaf9f0171e9517f40e41".into()),
            qop_auth: true,
            sess: false,
            nc: 0,
        };
        let ha1 = md5_hex("Mufasa:testrealm@host.com:Circle Of Life");
        let ha2 = md5_hex("GET:/dir/index.html");
        let expected = md5_hex(&format!("{ha1}:{}:00000001:0a4f113b:auth:{ha2}", ch.nonce));
        assert_eq!(expected, "6629fae49393a05397450978507c4ef1");
        let h = authorization(&mut Scheme::Digest(ch.clone()), "Mufasa", "Circle Of Life", "GET", "/dir/index.html").unwrap();
        assert!(h.contains("nc=00000001") && h.contains(r#"uri="/dir/index.html""#));
        ch.nc = 5;
        let mut s = Scheme::Digest(ch);
        authorization(&mut s, "u", "p", "GET", "/").unwrap();
        assert!(matches!(s, Scheme::Digest(DigestChallenge { nc: 6, .. })));
    }

    #[test]
    fn basic_header() {
        assert_eq!(authorization(&mut Scheme::Basic, "Aladdin", "open sesame", "GET", "/"), Some("Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==".into()));
    }
}
