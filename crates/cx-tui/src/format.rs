//! Human-readable sizes, dates, type labels and file categories (which
//! drive colors and icons). Mirrors the desktop UI's `format.ts`.

use chrono::{Datelike, Local, TimeZone};
use cx_core::Entry;

/// "0 B", "999 B", "1.2 KB", "34.5 MB" (decimal units, like Finder).
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["KB", "MB", "GB", "TB", "PB", "EB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1000.0;
    let mut i = 0;
    while v >= 999.95 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Group thousands: 12345 → "12,345".
pub fn count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

/// Modification date for list columns: "Today 14:03", "Yesterday 09:12",
/// "Mar 4 10:00" this year, "2021-03-04" before. Always 16 chars or fewer.
pub fn date(ms: Option<i64>) -> String {
    let Some(ms) = ms else { return String::new() };
    let Some(t) = Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let now = Local::now();
    let days = (now.date_naive() - t.date_naive()).num_days();
    match days {
        0 => t.format("Today %H:%M").to_string(),
        1 => t.format("Yesterday %H:%M").to_string(),
        _ if t.year() == now.year() && days > 0 => t.format("%b %e %H:%M").to_string(),
        _ => t.format("%Y-%m-%d").to_string(),
    }
}

/// Full timestamp for info panes.
pub fn date_long(ms: Option<i64>) -> String {
    ms.and_then(|ms| Local.timestamp_millis_opt(ms).single()).map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_else(|| "—".into())
}

pub fn duration(secs: f64) -> String {
    let s = secs.round() as u64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// Lower-cased extension ("" for folders and dotfiles without one).
pub fn ext(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[i + 1..],
        _ => "",
    }
}

/// What kind of file this is, for color and icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Dir,
    Archive,
    Image,
    Audio,
    Video,
    Code,
    Document,
    Executable,
    Symlink,
    Other,
}

pub fn category(e: &Entry) -> Category {
    if e.is_dir {
        return Category::Dir;
    }
    let x = ext(&e.name).to_ascii_lowercase();
    let x = x.as_str();
    if cx_archive::is_archive(&e.name) {
        return Category::Archive;
    }
    match x {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "heif" | "svg" | "ico" | "raw" | "cr2" | "nef" | "dng" | "avif" | "psd" => Category::Image,
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "opus" | "aiff" | "wma" => Category::Audio,
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg" => Category::Video,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "rtf" | "txt" | "md" | "markdown" | "pages" | "numbers" | "key" | "epub" | "csv" => Category::Document,
        "exe" | "msi" | "app" | "dmg" | "pkg" | "deb" | "rpm" | "appimage" | "bat" | "cmd" | "com" => Category::Executable,
        _ if cx_thumbs::language_for_name(&e.name).is_some() => Category::Code,
        _ if e.kind == cx_core::EntryKind::Symlink => Category::Symlink,
        _ => Category::Other,
    }
}

/// "Folder", "PDF document", "PNG image", "Rust source", …
pub fn type_label(e: &Entry) -> String {
    if e.is_dir {
        return if e.kind == cx_core::EntryKind::Symlink { "Folder alias".into() } else { "Folder".into() };
    }
    let x = ext(&e.name);
    if x.is_empty() {
        return if e.kind == cx_core::EntryKind::Symlink { "Alias".into() } else { "File".into() };
    }
    let up = x.to_ascii_uppercase();
    match category(e) {
        Category::Image => format!("{up} image"),
        Category::Audio => format!("{up} audio"),
        Category::Video => format!("{up} video"),
        Category::Archive => format!("{up} archive"),
        Category::Code => match cx_thumbs::language_for_name(&e.name) {
            Some(l) => format!("{} source", capitalize(l)),
            None => format!("{up} file"),
        },
        Category::Document if up == "PDF" => "PDF document".into(),
        Category::Document if up == "TXT" => "Plain text".into(),
        Category::Document if up == "MD" || up == "MARKDOWN" => "Markdown".into(),
        Category::Document => format!("{up} document"),
        _ => format!("{up} file"),
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Nerd Font glyph for an entry (only used when the setting is on).
pub fn nerd_icon(e: &Entry) -> &'static str {
    match category(e) {
        Category::Dir => "\u{f07b}",
        Category::Archive => "\u{f410}",
        Category::Image => "\u{f1c5}",
        Category::Audio => "\u{f1c7}",
        Category::Video => "\u{f1c8}",
        Category::Code => "\u{f121}",
        Category::Document => match ext(&e.name).to_ascii_lowercase().as_str() {
            "pdf" => "\u{f1c1}",
            "md" | "markdown" => "\u{f48a}",
            _ => "\u{f0f6}",
        },
        Category::Executable => "\u{f489}",
        Category::Symlink => "\u{f0c1}",
        Category::Other => "\u{f15b}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_counts() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1_500), "1.5 KB");
        assert_eq!(size(123_456_789), "123 MB");
        assert_eq!(size(999_999), "1.0 MB");
        assert_eq!(count(1234567), "1,234,567");
        assert_eq!(plural(1, "item", "items"), "1 item");
        assert_eq!(duration(75.0), "1m 15s");
    }

    #[test]
    fn extensions() {
        assert_eq!(ext("a.tar.gz"), "gz");
        assert_eq!(ext(".bashrc"), "");
        assert_eq!(ext("noext"), "");
    }
}
