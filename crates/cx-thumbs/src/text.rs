//! Text previews for Quick Look of text and code files.
//!
//! Only the first `max_bytes` are read, so previewing a multi-gigabyte log
//! (or a file on a slow server) stays instant. Binary files are rejected
//! rather than shown as mojibake; the UI falls back to the file's icon.

use crate::read::read_prefix;
use cx_core::{CxError, Location, Result, Vfs};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextPreview {
    pub text: String,
    /// The file is longer than what was read.
    pub truncated: bool,
    /// "utf-8", "utf-16le", "utf-16be" or "windows-1252".
    pub encoding: &'static str,
    /// Syntax highlighting hint from the file name ("rust", "json", …).
    pub language_guess: Option<&'static str>,
}

/// Read up to `max_bytes` of `loc` as text. Fails with `Unsupported` for
/// binary files and folders.
pub async fn preview_text(vfs: &Vfs, loc: &Location, max_bytes: usize) -> Result<TextPreview> {
    let provider = vfs.provider(loc).await?;
    let entry = provider.stat(loc).await?;
    if entry.is_dir {
        return Err(CxError::Unsupported("text preview of a folder".into()));
    }
    // One extra byte tells us whether there is more, even when the size
    // reported by the provider is stale or missing.
    let bytes = read_prefix(vfs, loc, max_bytes as u64 + 1).await?;
    let truncated = bytes.len() > max_bytes;
    let bytes = &bytes[..bytes.len().min(max_bytes)];
    let (text, encoding) = decode(bytes, truncated)?;
    Ok(TextPreview { text, truncated, encoding, language_guess: language_for_name(&loc.name()) })
}

/// Decode bytes that may be cut off at an arbitrary point.
pub(crate) fn decode(bytes: &[u8], truncated: bool) -> Result<(String, &'static str)> {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return Ok((String::from_utf8_lossy(trim_partial_utf8(rest, truncated)).into_owned(), "utf-8"));
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return Ok((utf16(rest, u16::from_le_bytes), "utf-16le"));
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return Ok((utf16(rest, u16::from_be_bytes), "utf-16be"));
    }
    if looks_binary(bytes) {
        return Err(CxError::Unsupported("binary file".into()));
    }
    let valid = trim_partial_utf8(bytes, truncated);
    match std::str::from_utf8(valid) {
        Ok(s) => Ok((s.to_string(), "utf-8")),
        // Not UTF-8: legacy 8-bit text, most often Windows-1252/Latin-1.
        Err(_) => Ok((bytes.iter().map(|&b| cp1252(b)).collect(), "windows-1252")),
    }
}

/// Drop a multi-byte character cut in half by the read limit.
fn trim_partial_utf8(bytes: &[u8], truncated: bool) -> &[u8] {
    if !truncated {
        return bytes;
    }
    match std::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => &bytes[..e.valid_up_to()],
        _ => bytes,
    }
}

fn utf16(bytes: &[u8], word: fn([u8; 2]) -> u16) -> String {
    let units = bytes.chunks_exact(2).map(|c| word([c[0], c[1]]));
    let mut s: String = char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect();
    // A surrogate pair split by the read limit decodes to one replacement char.
    if s.ends_with(char::REPLACEMENT_CHARACTER) {
        s.pop();
    }
    s
}

/// NUL bytes, or lots of control characters, in the first 8 KiB.
fn looks_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return true;
    }
    let control = head.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b)).count();
    control * 10 > head.len().max(1)
}

fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™',
        'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9f => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Language id (as used by common highlighters) for a file name.
pub fn language_for_name(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let by_name = match lower.as_str() {
        "makefile" | "gnumakefile" => Some("makefile"),
        "dockerfile" | "containerfile" => Some("dockerfile"),
        "cmakelists.txt" => Some("cmake"),
        "cargo.lock" | "pipfile" => Some("toml"),
        "gemfile" | "rakefile" | "podfile" => Some("ruby"),
        ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => Some("shell"),
        ".gitignore" | ".dockerignore" | ".gitattributes" => Some("ignore"),
        _ => None,
    };
    if by_name.is_some() {
        return by_name;
    }
    Some(match crate::extension(&lower).as_str() {
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "jsx",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "svelte" => "svelte",
        "vue" => "vue",
        "py" | "pyw" | "pyi" => "python",
        "rb" => "ruby",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "m" | "mm" => "objectivec",
        "cs" => "csharp",
        "fs" | "fsx" => "fsharp",
        "php" => "php",
        "pl" | "pm" => "perl",
        "lua" => "lua",
        "r" => "r",
        "dart" => "dart",
        "scala" => "scala",
        "hs" => "haskell",
        "ex" | "exs" => "elixir",
        "erl" => "erlang",
        "clj" | "cljs" => "clojure",
        "zig" => "zig",
        "nim" => "nim",
        "sh" | "bash" | "zsh" | "fish" | "ksh" => "shell",
        "ps1" | "psm1" => "powershell",
        "bat" | "cmd" => "batch",
        "html" | "htm" | "xhtml" => "html",
        "css" => "css",
        "scss" | "sass" => "scss",
        "less" => "less",
        "json" | "jsonc" | "json5" | "geojson" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" | "plist" | "xsd" | "xsl" | "svg" => "xml",
        "ini" | "cfg" | "conf" | "properties" => "ini",
        "md" | "markdown" | "mdx" => "markdown",
        "rst" => "rst",
        "tex" | "sty" => "latex",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        "proto" => "protobuf",
        "diff" | "patch" => "diff",
        "csv" => "csv",
        "tsv" => "tsv",
        "log" => "log",
        "txt" | "text" => "plaintext",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_boms_and_legacy_text() {
        assert_eq!(decode(b"\xEF\xBB\xBFhi", false).unwrap(), ("hi".into(), "utf-8"));
        assert_eq!(decode(b"\xFF\xFEh\0i\0", false).unwrap(), ("hi".into(), "utf-16le"));
        assert_eq!(decode(b"\xFE\xFF\0h\0i", false).unwrap(), ("hi".into(), "utf-16be"));
        assert_eq!(decode(b"caf\xe9 \x80", false).unwrap(), ("café €".into(), "windows-1252"));
    }

    #[test]
    fn cut_characters_are_dropped_not_mangled() {
        let s = "añb".as_bytes();
        assert_eq!(decode(&s[..2], true).unwrap().0, "a");
    }

    #[test]
    fn binary_is_rejected() {
        assert!(matches!(decode(b"\x7fELF\x02\x01\x01\0\0\0", false), Err(CxError::Unsupported(_))));
        assert!(matches!(decode(&[1, 2, 3, 4, 5, 6, 7, 8, b'a'], false), Err(CxError::Unsupported(_))));
    }

    #[test]
    fn languages() {
        assert_eq!(language_for_name("main.RS"), Some("rust"));
        assert_eq!(language_for_name("Dockerfile"), Some("dockerfile"));
        assert_eq!(language_for_name("photo.jpg"), None);
        assert_eq!(language_for_name("README"), None);
    }
}
