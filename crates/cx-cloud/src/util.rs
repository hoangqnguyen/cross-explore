//! Small helpers shared by the three services: timestamps (all three speak
//! RFC 3339), URL encoding, randomness for PKCE and the name juggling that
//! folder creation and duplicate names need. Dates are written out here
//! rather than pulling in a date crate for two conversions (as in cx-s3).

use cx_core::{Entry, EntryKind};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

/// RFC 3986 unreserved characters stay as they are; everything else is escaped.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// Encode a query value or a single path segment.
pub(crate) fn encode(s: &str) -> String {
    utf8_percent_encode(s, UNRESERVED).to_string()
}

/// Encode a `/`-separated path segment by segment, keeping the slashes.
pub(crate) fn encode_path(path: &str) -> String {
    path.split('/').map(encode).collect::<Vec<_>>().join("/")
}

/// `k=v&k=v` with both sides encoded.
pub(crate) fn query(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&")
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `n` random bytes from the OS.
pub(crate) fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("the OS random source is available");
    buf
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

/// Parse RFC 3339 (`2024-05-01T10:20:30Z`, `…30.123Z`, Graph's seven
/// fractional digits, or a `+02:00` offset) into milliseconds since the epoch.
pub(crate) fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|v| v.parse::<i64>().ok());
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &s[19..];
    let mut ms = 0;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        rest = &frac[digits.len()..];
        ms = format!("{digits:0<3}")[..3].parse::<i64>().ok()?;
    }
    let offset_min = match rest.as_bytes().first() {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh: i64 = rest.get(1..3)?.parse().ok()?;
            let om: i64 = rest.get(4..6)?.parse().ok()?;
            let v = oh * 60 + om;
            if *sign == b'-' { -v } else { v }
        }
        _ => return None,
    };
    Some((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset_min * 60) * 1000 + ms)
}

/// Format milliseconds since the epoch as `2024-05-01T10:20:30.123Z`.
pub(crate) fn format_rfc3339(ms: i64) -> String {
    let (secs, milli) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

pub(crate) fn dir_entry(name: String, modified: Option<i64>, readonly: bool) -> Entry {
    Entry { hidden: name.starts_with('.'), kind: EntryKind::Dir, is_dir: true, size: 0, modified, created: None, readonly, executable: false, name }
}

pub(crate) fn file_entry(name: String, size: u64, modified: Option<i64>, created: Option<i64>) -> Entry {
    Entry { hidden: name.starts_with('.'), kind: EntryKind::File, is_dir: false, size, modified, created, readonly: false, executable: false, name }
}

/// "New folder", then "New folder (2)", "New folder (3)"…
pub(crate) fn new_folder_names() -> impl Iterator<Item = String> {
    (1..10_000).map(|n| if n == 1 { "New folder".to_string() } else { format!("New folder ({n})") })
}

/// Split `report.pdf` into (`report`, `.pdf`). Dotfiles and names without a
/// dot have no extension.
pub(crate) fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// `name` if unused, else `stem (2).ext`, `stem (3).ext`… The chosen name is
/// added to `used`.
pub(crate) fn unique_name(used: &mut HashSet<String>, name: &str) -> String {
    if used.insert(name.to_string()) {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    for n in 2.. {
        let candidate = format!("{stem} ({n}){ext}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("some suffix is free")
}

/// reqwest's top-level message ("error sending request") hides the cause.
pub(crate) fn describe(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        s.push_str(": ");
        s.push_str(&c.to_string());
        src = c.source();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        assert_eq!(parse_rfc3339("2013-05-24T00:00:00Z"), Some(1_369_353_600_000));
        assert_eq!(parse_rfc3339("2013-05-24T00:00:01.5Z"), Some(1_369_353_601_500));
        assert_eq!(parse_rfc3339("2013-05-24T00:00:01.1234567Z"), Some(1_369_353_601_123));
        assert_eq!(parse_rfc3339("2013-05-24T02:00:00+02:00"), Some(1_369_353_600_000));
        assert_eq!(format_rfc3339(1_369_353_601_123), "2013-05-24T00:00:01.123Z");
        assert_eq!(parse_rfc3339("nope"), None);
    }

    #[test]
    fn unique_names_suffix_before_the_extension() {
        let mut used = HashSet::new();
        assert_eq!(unique_name(&mut used, "a.txt"), "a.txt");
        assert_eq!(unique_name(&mut used, "a.txt"), "a (2).txt");
        assert_eq!(unique_name(&mut used, "a.txt"), "a (3).txt");
        assert_eq!(unique_name(&mut used, ".env"), ".env");
        assert_eq!(unique_name(&mut used, ".env"), ".env (2)");
    }

    #[test]
    fn encoding() {
        assert_eq!(encode_path("/a b/c#d"), "/a%20b/c%23d");
        assert_eq!(query(&[("q", "'x' in parents")]), "q=%27x%27%20in%20parents");
    }
}
