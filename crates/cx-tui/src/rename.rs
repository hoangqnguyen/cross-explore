//! Total Commander's multi-rename tool, as in the desktop app's dialog:
//! name and extension masks with tokens, find & replace (plain or regex),
//! case changes and a counter. Pure functions, so the live preview and the
//! tests run the exact code that renames.
//!
//! Tokens: `[N]` name without extension, `[E]` extension, `[N2-5]` /
//! `[E1]` character ranges (1-based), `[C]` counter, `[YMD]` modification
//! date, `[hms]` modification time, `[P]` parent folder name.

use chrono::{Local, TimeZone};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseMode {
    #[default]
    Keep,
    Lower,
    Upper,
    Title,
}

impl CaseMode {
    pub fn label(self) -> &'static str {
        match self {
            CaseMode::Keep => "Unchanged",
            CaseMode::Lower => "lowercase",
            CaseMode::Upper => "UPPERCASE",
            CaseMode::Title => "Title Case",
        }
    }

    pub fn next(self) -> CaseMode {
        match self {
            CaseMode::Keep => CaseMode::Lower,
            CaseMode::Lower => CaseMode::Upper,
            CaseMode::Upper => CaseMode::Title,
            CaseMode::Title => CaseMode::Keep,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenameSpec {
    pub name_mask: String,
    pub ext_mask: String,
    pub search: String,
    pub replace: String,
    pub regex: bool,
    pub case: CaseMode,
    pub start: i64,
    pub step: i64,
    pub digits: usize,
    /// For `[P]`.
    pub parent: String,
}

impl Default for RenameSpec {
    fn default() -> Self {
        RenameSpec {
            name_mask: "[N]".into(),
            ext_mask: "[E]".into(),
            search: String::new(),
            replace: String::new(),
            regex: false,
            case: CaseMode::Keep,
            start: 1,
            step: 1,
            digits: 2,
            parent: String::new(),
        }
    }
}

/// What gets renamed: the name, whether it's a folder, and its mtime.
#[derive(Debug, Clone)]
pub struct Source<'a> {
    pub name: &'a str,
    pub is_dir: bool,
    pub modified: Option<i64>,
}

fn split(name: &str, is_dir: bool) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if !is_dir && i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

fn pad(n: i64, digits: usize) -> String {
    if n < 0 {
        format!("-{:0width$}", -n, width = digits)
    } else {
        format!("{n:0digits$}")
    }
}

fn apply_mask(
    spec: &RenameSpec,
    mask: &str,
    stem: &str,
    ext: &str,
    i: usize,
    src: &Source,
) -> String {
    let date = src
        .modified
        .and_then(|ms| Local.timestamp_millis_opt(ms).single());
    let mut out = String::new();
    let mut rest = mask;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let Some(close) = after.find(']') else {
            out.push_str(after);
            return out;
        };
        let token = &after[1..close];
        match expand(spec, token, stem, ext, i, date) {
            Some(s) => out.push_str(&s),
            None => out.push_str(&after[..=close]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn expand(
    spec: &RenameSpec,
    token: &str,
    stem: &str,
    ext: &str,
    i: usize,
    date: Option<chrono::DateTime<Local>>,
) -> Option<String> {
    match token {
        "C" => return Some(pad(spec.start + i as i64 * spec.step, spec.digits)),
        "YMD" => {
            return Some(
                date.map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default(),
            )
        }
        "hms" => {
            return Some(
                date.map(|d| d.format("%H.%M.%S").to_string())
                    .unwrap_or_default(),
            )
        }
        "P" => return Some(spec.parent.clone()),
        _ => {}
    }
    let (kind, range) = token.split_at(token.chars().next().map(|c| c.len_utf8())?);
    let src = match kind {
        "N" => stem,
        "E" => ext,
        _ => return None,
    };
    if range.is_empty() {
        return Some(src.to_string());
    }
    let (a, b) = match range.split_once('-') {
        Some((a, b)) => (a.parse::<usize>().ok()?, Some(b.parse::<usize>().ok()?)),
        None => (range.parse::<usize>().ok()?, None),
    };
    let chars: Vec<char> = src.chars().collect();
    let from = a.saturating_sub(1);
    Some(match b {
        // JS `slice(from, b)`: characters from..b (b exclusive, 1-based a).
        Some(b) => chars
            .get(from..b.min(chars.len()).max(from))
            .map(|s| s.iter().collect())
            .unwrap_or_default(),
        None => chars.get(from).map(|c| c.to_string()).unwrap_or_default(),
    })
}

fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut boundary = true;
    for c in s.to_lowercase().chars() {
        if boundary && c.is_alphabetic() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        boundary = c.is_whitespace() || "_-.(".contains(c);
    }
    out
}

/// The new name of item `i`.
pub fn transform(spec: &RenameSpec, i: usize, src: &Source, re: Option<&regex::Regex>) -> String {
    let (stem, ext) = split(src.name, src.is_dir);
    let name = apply_mask(spec, &spec.name_mask, stem, ext, i, src);
    let new_ext = if src.is_dir {
        String::new()
    } else {
        apply_mask(spec, &spec.ext_mask, stem, ext, i, src)
    };
    let mut full = if new_ext.is_empty() {
        name
    } else {
        format!("{name}.{new_ext}")
    };
    if !spec.search.is_empty() {
        full = match (spec.regex, re) {
            (true, Some(re)) => re.replace_all(&full, spec.replace.as_str()).into_owned(),
            // An incomplete regex while typing: leave the name alone.
            (true, None) => full,
            (false, _) => full.replace(&spec.search, &spec.replace),
        };
    }
    match spec.case {
        CaseMode::Keep => full,
        CaseMode::Lower => full.to_lowercase(),
        CaseMode::Upper => full.to_uppercase(),
        CaseMode::Title => title_case(&full),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub from: String,
    pub to: String,
    /// "invalid" (empty or contains a separator) or "duplicate", else "".
    pub problem: &'static str,
}

/// The preview: every item's new name and whether it can be applied.
/// `others` are names in the folder that are not being renamed.
pub fn plan(spec: &RenameSpec, items: &[Source], others: &[&str]) -> Vec<Planned> {
    let re = if spec.regex && !spec.search.is_empty() {
        regex::Regex::new(&spec.search).ok()
    } else {
        None
    };
    let out: Vec<(String, String)> = items
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.to_string(), transform(spec, i, s, re.as_ref())))
        .collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, to) in &out {
        *counts.entry(to.to_lowercase()).or_default() += 1;
    }
    let renaming: std::collections::HashSet<String> =
        items.iter().map(|s| s.name.to_lowercase()).collect();
    let taken: std::collections::HashSet<String> = others
        .iter()
        .map(|n| n.to_lowercase())
        .filter(|n| !renaming.contains(n))
        .collect();
    out.into_iter()
        .map(|(from, to)| {
            let l = to.to_lowercase();
            let problem = if to.is_empty()
                || to.contains('/')
                || to.contains('\0')
                || to == "."
                || to == ".."
            {
                "invalid"
            } else if counts.get(&l).copied().unwrap_or(0) > 1 || taken.contains(&l) {
                "duplicate"
            } else {
                ""
            };
            Planned { from, to, problem }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(name: &str) -> Source<'_> {
        Source {
            name,
            is_dir: false,
            modified: None,
        }
    }

    fn one(spec: &RenameSpec, name: &str, i: usize) -> String {
        let re = regex::Regex::new(&spec.search).ok();
        transform(spec, i, &src(name), re.as_ref())
    }

    #[test]
    fn masks_and_counter() {
        let spec = RenameSpec {
            name_mask: "Photo_[C]_[N]".into(),
            ..Default::default()
        };
        assert_eq!(one(&spec, "IMG_1.JPG", 0), "Photo_01_IMG_1.JPG");
        assert_eq!(one(&spec, "IMG_2.JPG", 1), "Photo_02_IMG_2.JPG");
        let spec = RenameSpec {
            name_mask: "[N1-3]".into(),
            ext_mask: "[E1]".into(),
            ..Default::default()
        };
        assert_eq!(one(&spec, "abcdef.txt", 0), "abc.t");
        let spec = RenameSpec {
            name_mask: "[N2]-[P]".into(),
            parent: "Trip".into(),
            start: 5,
            step: 5,
            digits: 3,
            ..Default::default()
        };
        assert_eq!(one(&spec, "xyz.md", 0), "y-Trip.md");
        let spec = RenameSpec {
            name_mask: "[C]".into(),
            start: 5,
            step: 5,
            digits: 3,
            ..Default::default()
        };
        assert_eq!(one(&spec, "a.md", 2), "015.md");
        let spec = RenameSpec {
            name_mask: "[X]".into(),
            ..Default::default()
        };
        assert_eq!(
            one(&spec, "a.md", 0),
            "[X].md",
            "unknown tokens stay literal"
        );
    }

    #[test]
    fn date_tokens_use_the_modification_time() {
        let ms = Local
            .with_ymd_and_hms(2024, 3, 9, 7, 5, 1)
            .unwrap()
            .timestamp_millis();
        let spec = RenameSpec {
            name_mask: "[YMD] [hms]".into(),
            ..Default::default()
        };
        let s = Source {
            name: "a.jpg",
            is_dir: false,
            modified: Some(ms),
        };
        assert_eq!(transform(&spec, 0, &s, None), "2024-03-09 07.05.01.jpg");
    }

    #[test]
    fn replace_regex_and_case() {
        let spec = RenameSpec {
            search: " ".into(),
            replace: "_".into(),
            case: CaseMode::Lower,
            ..Default::default()
        };
        assert_eq!(one(&spec, "My File.TXT", 0), "my_file.txt");
        let spec = RenameSpec {
            search: r"(\d+)".into(),
            replace: "#$1".into(),
            regex: true,
            ..Default::default()
        };
        assert_eq!(one(&spec, "a12b3.txt", 0), "a#12b#3.txt");
        let spec = RenameSpec {
            case: CaseMode::Title,
            ..Default::default()
        };
        assert_eq!(one(&spec, "hello wORLD-foo.txt", 0), "Hello World-Foo.Txt");
        let spec = RenameSpec {
            search: "(".into(),
            regex: true,
            ..Default::default()
        };
        let re = regex::Regex::new(&spec.search).ok();
        assert_eq!(
            transform(&spec, 0, &src("a(.txt"), re.as_ref()),
            "a(.txt",
            "bad regex leaves names alone"
        );
    }

    #[test]
    fn folders_keep_dots_and_plan_flags_problems() {
        let spec = RenameSpec {
            name_mask: "x".into(),
            ..Default::default()
        };
        let items = [
            Source {
                name: "v1.2",
                is_dir: true,
                modified: None,
            },
            src("a.txt"),
            src("b.txt"),
        ];
        let p = plan(&spec, &items, &["x.txt", "other"]);
        assert_eq!(p[0].to, "x");
        assert_eq!(p[0].problem, "");
        assert_eq!(p[1].problem, "duplicate");
        let spec = RenameSpec {
            name_mask: "[N]".into(),
            ext_mask: "bak".into(),
            ..Default::default()
        };
        let p = plan(&spec, &[src("a.txt")], &["a.bak"]);
        assert_eq!(p[0].problem, "duplicate", "clashes with a file that stays");
        let spec = RenameSpec {
            name_mask: "".into(),
            ext_mask: "".into(),
            ..Default::default()
        };
        assert_eq!(plan(&spec, &[src("a.txt")], &[])[0].problem, "invalid");
    }
}
