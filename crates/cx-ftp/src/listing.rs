//! Turning FTP listing lines into entries.
//!
//! `MLSD`/`MLST` (RFC 3659) give exact sizes and UTC times in a fixed
//! format, so they are preferred. Old servers only have `LIST`, whose output
//! is whatever `ls -l` (or `DIR` on Windows) printed; suppaftp's parsers
//! handle the Unix and DOS styles.

use cx_core::{Entry, EntryKind};
use std::time::UNIX_EPOCH;
use suppaftp::list::{File as ListFile, ListParser, PosixPexQuery};

/// A parsed `MLSD`/`MLST` line.
#[derive(Debug, Clone, PartialEq)]
pub struct Mlsx {
    pub entry: Entry,
    /// `cdir`/`pdir`: the listed folder itself or its parent.
    pub is_self_or_parent: bool,
}

/// `type=file;size=12;modify=20240101120000; name with spaces`
pub fn parse_mlsx(line: &str) -> Option<Mlsx> {
    let line = line.trim_end_matches(['\r', '\n']);
    // Facts end with ';', then exactly one space, then the name (which may
    // itself contain spaces and semicolons).
    let (facts, name) = match line.strip_prefix(' ') {
        Some(name) => ("", name),
        None => line.split_once(' ')?,
    };
    if name.is_empty() {
        return None;
    }
    let mut kind = EntryKind::File;
    let mut is_dir = false;
    let mut self_or_parent = false;
    let mut size = 0;
    let mut modified = None;
    let mut unix_mode = None;
    let mut perm = None;
    for fact in facts.split(';').filter(|f| !f.is_empty()) {
        let Some((k, v)) = fact.split_once('=') else { continue };
        match k.to_ascii_lowercase().as_str() {
            "type" => {
                let v = v.to_ascii_lowercase();
                match v.as_str() {
                    "file" => {}
                    "dir" => (kind, is_dir) = (EntryKind::Dir, true),
                    "cdir" | "pdir" => (kind, is_dir, self_or_parent) = (EntryKind::Dir, true, true),
                    _ if v.starts_with("os.unix=symlink") || v.starts_with("os.unix=slink") => kind = EntryKind::Symlink,
                    _ => kind = EntryKind::Other,
                }
            }
            // `sizd` is a folder's own size: meaningless to show.
            "size" => size = v.parse().unwrap_or(0),
            "modify" => modified = parse_mdtm(v),
            "unix.mode" => unix_mode = u32::from_str_radix(v.trim_start_matches("0o"), 8).ok(),
            "perm" => perm = Some(v.to_ascii_lowercase()),
            _ => {}
        }
    }
    // The name of a MLST reply is the full path; callers override it.
    let readonly = match (unix_mode, &perm) {
        (Some(m), _) => m & 0o222 == 0,
        // RFC 3659: files are writable with "w" (or "a"ppendable), folders
        // accept new entries with "c".
        (None, Some(p)) if is_dir => !p.contains('c'),
        (None, Some(p)) => !p.contains('w') && !p.contains('a'),
        (None, None) => false,
    };
    let name = name.to_string();
    Some(Mlsx {
        entry: Entry { hidden: name.starts_with('.'), name, kind, is_dir, size: if is_dir { 0 } else { size }, modified, created: None, readonly },
        is_self_or_parent: self_or_parent,
    })
}

/// A `LIST` line in Unix `ls -l` or DOS style. `None` for lines that aren't
/// entries ("total 12", ".", "..").
pub fn parse_list(line: &str) -> Option<Entry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (file, posix) = match ListParser::parse_posix(line) {
        Ok(f) => (f, true),
        Err(_) => (ListParser::parse_dos(line).ok()?, false),
    };
    entry_from_list(&file, posix)
}

fn entry_from_list(f: &ListFile, posix: bool) -> Option<Entry> {
    let name = f.name().to_string();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let (kind, is_dir) = if f.is_directory() {
        (EntryKind::Dir, true)
    } else if f.is_symlink() {
        (EntryKind::Symlink, false)
    } else {
        (EntryKind::File, false)
    };
    let modified = f.modified().duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as i64).filter(|&ms| ms > 0);
    let readonly = posix && ![PosixPexQuery::Owner, PosixPexQuery::Group, PosixPexQuery::Others].into_iter().any(|q| f.can_write(q));
    Some(Entry { hidden: name.starts_with('.'), name, kind, is_dir, size: if is_dir { 0 } else { f.size() as u64 }, modified, created: None, readonly })
}

/// `YYYYMMDDHHMMSS[.sss]` (UTC) → milliseconds since the epoch.
pub fn parse_mdtm(s: &str) -> Option<i64> {
    let (main, frac) = s.split_once('.').unwrap_or((s, ""));
    if main.len() != 14 || !main.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| main[r].parse::<i64>().ok();
    let secs = days_from_civil(n(0..4)?, n(4..6)?, n(6..8)?) * 86_400 + n(8..10)? * 3600 + n(10..12)? * 60 + n(12..14)?;
    let ms = match frac.len() {
        0 => 0,
        _ => format!("{:0<3}", &frac[..frac.len().min(3)]).parse::<i64>().ok()?,
    };
    Some(secs * 1000 + ms)
}

/// Milliseconds since the epoch → `YYYYMMDDHHMMSS` (UTC), for `MFMT`/`MDTM`.
pub fn format_mdtm(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

// Howard Hinnant's civil calendar algorithms (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mlsd_lines() {
        let e = parse_mlsx("type=file;size=1234;modify=20240102030405.5;UNIX.mode=0444; my file; v2.txt").unwrap();
        assert_eq!(e.entry.name, "my file; v2.txt");
        assert_eq!((e.entry.kind, e.entry.size, e.entry.readonly), (EntryKind::File, 1234, true));
        assert_eq!(e.entry.modified, Some(1_704_164_645_500));
        assert!(!e.is_self_or_parent);

        let d = parse_mlsx("Type=dir;Modify=20240102030405;Perm=flcdmpe; .config").unwrap();
        assert!(d.entry.is_dir && d.entry.hidden && !d.entry.readonly);
        assert!(parse_mlsx("type=cdir;sizd=4096; .").unwrap().is_self_or_parent);
        assert!(parse_mlsx("type=pdir; /").unwrap().is_self_or_parent);
        assert_eq!(parse_mlsx("type=OS.unix=symlink;size=5; link").unwrap().entry.kind, EntryKind::Symlink);
        assert!(parse_mlsx("type=file;perm=r; ro.txt").unwrap().entry.readonly);
        // No facts at all.
        assert_eq!(parse_mlsx(" bare").unwrap().entry.name, "bare");
        assert!(parse_mlsx("garbage").is_none());
    }

    #[test]
    fn list_fallback_unix_and_dos() {
        let e = parse_list("-rw-r--r--    1 1000     1000         5120 Jan 02  2024 hello world.txt").unwrap();
        assert_eq!((e.name.as_str(), e.size, e.is_dir, e.readonly), ("hello world.txt", 5120, false, false));
        assert_eq!(e.modified, Some(1_704_153_600_000)); // 2024-01-02 00:00 UTC
        let d = parse_list("drwxr-xr-x    2 0        0            4096 Mar 04 12:30 Some Dir").unwrap();
        assert!(d.is_dir && d.kind == EntryKind::Dir && d.size == 0);
        let ro = parse_list("-r--r--r--    1 0        0               1 Jan 02  2024 .ro").unwrap();
        assert!(ro.readonly && ro.hidden);
        let l = parse_list("lrwxrwxrwx    1 0        0               7 Jan 02  2024 link -> target").unwrap();
        assert_eq!((l.name.as_str(), l.kind), ("link", EntryKind::Symlink));
        let dos = parse_list("10-19-20  03:19PM       <DIR>          pub").unwrap();
        assert!(dos.is_dir && dos.name == "pub");
        let dos = parse_list("04-08-14  03:09PM                  403 readme.txt").unwrap();
        assert_eq!((dos.name.as_str(), dos.size), ("readme.txt", 403));
        assert!(parse_list("total 12").is_none());
        assert!(parse_list("drwxr-xr-x    2 0        0            4096 Mar 04 12:30 .").is_none());
    }

    #[test]
    fn mdtm_round_trip() {
        assert_eq!(parse_mdtm("19700101000000"), Some(0));
        assert_eq!(parse_mdtm("20000229235959"), Some(951_868_799_000));
        assert_eq!(format_mdtm(951_868_799_000), "20000229235959");
        assert_eq!(format_mdtm(1_600_000_000_000), "20200913122640");
        assert_eq!(parse_mdtm(&format_mdtm(1_600_000_000_000)), Some(1_600_000_000_000));
        assert_eq!(parse_mdtm("2020"), None);
    }
}
