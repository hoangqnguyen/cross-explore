//! Small helpers: `cx:` pseudo-URIs for search, compare and tag views,
//! display paths, and OSC 52 clipboard writes.

use cx_core::Location;

pub fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 1 && i + 3 <= b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `cx:search?root=…&q=…` and friends.
pub fn cx_uri(kind: &str, params: &[(&str, &str)]) -> String {
    let q: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect();
    format!("cx:{kind}?{}", q.join("&"))
}

/// A parameter of a `cx:` URI.
pub fn param(uri: &str, key: &str) -> Option<String> {
    let (_, q) = uri.split_once('?')?;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| decode(v))
    })
}

/// What to show for a URI in the header or a list: local paths as paths,
/// the home folder as `~`, remote ones as URIs.
pub fn display(uri: &str) -> String {
    match Location::parse(uri) {
        Ok(loc) => {
            let d = loc.info().display;
            if let (Some(p), Some(home)) = (loc.local_path(), cx_core::location::home_dir()) {
                if let Ok(rest) = p.strip_prefix(&home) {
                    let r = rest.to_string_lossy();
                    return if r.is_empty() {
                        "~".into()
                    } else {
                        format!("~{}{r}", std::path::MAIN_SEPARATOR)
                    };
                }
            }
            d
        }
        Err(_) => uri.to_string(),
    }
}

/// Last path component of a URI, for labels.
pub fn name_of(uri: &str) -> String {
    Location::parse(uri)
        .map(|l| l.name())
        .ok()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| display(uri))
}

/// The terminal escape that puts `text` on the system clipboard (OSC 52).
/// Works in most modern terminals, over SSH too.
pub fn osc52(text: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b = text.as_bytes();
    let mut out = String::with_capacity(b.len() * 4 / 3 + 8);
    for chunk in b.chunks(3) {
        let n = match chunk.len() {
            3 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8 | chunk[2] as u32,
            2 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8,
            _ => (chunk[0] as u32) << 16,
        };
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(T[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    format!("\x1b]52;c;{out}\x07")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cx_uris_round_trip() {
        let u = cx_uri("search", &[("root", "file:///a b"), ("q", "*.rs&x")]);
        assert_eq!(param(&u, "root").unwrap(), "file:///a b");
        assert_eq!(param(&u, "q").unwrap(), "*.rs&x");
        assert!(param(&u, "nope").is_none());
    }

    #[test]
    fn base64_for_osc52() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
        assert_eq!(osc52("abc"), "\x1b]52;c;YWJj\x07");
        assert_eq!(osc52("a"), "\x1b]52;c;YQ==\x07");
    }
}
