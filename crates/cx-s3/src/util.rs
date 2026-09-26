//! Small helpers: hashing, S3's flavor of URI encoding and the two timestamp
//! formats S3 speaks (ISO 8601 in XML, RFC 1123 in headers). Written out here
//! rather than pulling in a date crate for four conversions.

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// SigV4 escapes everything but RFC 3986 unreserved characters.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// Encode a query value or a single path segment.
pub(crate) fn encode(s: &str) -> String {
    utf8_percent_encode(s, UNRESERVED).to_string()
}

/// Encode an object key for a URL path: each segment escaped, slashes kept.
pub(crate) fn encode_key(key: &str) -> String {
    key.split('/').map(encode).collect::<Vec<_>>().join("/")
}

/// Undo `encoding-type=url` in listings. S3 escapes like an HTML form (a
/// space may come back as `+`; a literal `+` always comes back as `%2B`).
pub(crate) fn decode_url(s: &str) -> String {
    percent_encoding::percent_decode_str(&s.replace('+', " ")).decode_utf8_lossy().into_owned()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `YYYYMMDDTHHMMSSZ` for `x-amz-date`.
pub(crate) fn amz_date(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// Parse `2009-10-12T17:50:30.000Z` (ListObjects' `LastModified`) into
/// milliseconds since the epoch.
pub(crate) fn parse_iso8601(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|v| v.parse::<i64>().ok());
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut ms = 0;
    if s.as_bytes().get(19) == Some(&b'.') {
        let frac: String = s[20..].chars().take_while(char::is_ascii_digit).collect();
        let frac = format!("{frac:0<3}");
        ms = frac[..3].parse::<i64>().ok()?;
    }
    Some(((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se) * 1000) + ms)
}

/// Parse an HTTP date (`Last-Modified` on HEAD/GET) into milliseconds.
pub(crate) fn parse_http_date(s: &str) -> Option<i64> {
    let t = httpdate::parse_http_date(s.trim()).ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn dates() {
        let t = UNIX_EPOCH + Duration::from_secs(1_369_353_600);
        assert_eq!(amz_date(t), "20130524T000000Z");
        assert_eq!(parse_iso8601("2013-05-24T00:00:00.000Z"), Some(1_369_353_600_000));
        assert_eq!(parse_iso8601("2013-05-24T00:00:01.5Z"), Some(1_369_353_601_500));
        assert_eq!(parse_iso8601("2013-05-24T00:00:02Z"), Some(1_369_353_602_000));
        assert_eq!(parse_http_date("Fri, 24 May 2013 00:00:00 GMT"), Some(1_369_353_600_000));
        for days in [-1000, 0, 11_000, 20_000, 60_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    #[test]
    fn encoding() {
        assert_eq!(encode_key("a b/c+d/é~.txt"), "a%20b/c%2Bd/%C3%A9~.txt");
        assert_eq!(decode_url("a+b%2Bc%2F"), "a b+c/");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
