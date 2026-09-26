//! The name-search query and its compiled form.

use crate::fold::fold;
use cx_core::{Entry, EntryKind, Location, Result};
use serde::{Deserialize, Serialize};

/// How `text` is interpreted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchMode {
    /// `/…/` is a regex, text with `*`, `?` or `[` is a glob, anything else a substring.
    #[default]
    Auto,
    /// Case- and diacritic-insensitive substring of the name.
    Substring,
    /// Whole-name glob (`*.pdf`, `IMG_????.jpg`), case-insensitive.
    Glob,
    /// Regex searched anywhere in the name, case-insensitive unless the
    /// pattern opts out with `(?-i)`.
    Regex,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KindFilter {
    #[default]
    Any,
    File,
    Dir,
}

/// A recursive name search. Every field has a default, so the UI only sends
/// what the user set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchQuery {
    /// Empty matches every name (useful with filters alone).
    pub text: String,
    pub mode: MatchMode,
    pub kind: KindFilter,
    /// Extensions without the dot, any case (`["pdf", "JPG"]`). Empty = any.
    pub extensions: Vec<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Milliseconds since the Unix epoch, inclusive.
    pub modified_after: Option<i64>,
    pub modified_before: Option<i64>,
    pub include_hidden: bool,
    /// How many levels of subfolders to descend into: 0 searches only the
    /// root's own entries. `None` = unlimited.
    pub max_depth: Option<u32>,
    pub max_results: Option<usize>,
    /// Folder names not to descend into (e.g. `node_modules`, `.git`),
    /// compared case-insensitively. Matching folders can still be hits.
    pub exclude_dirs: Vec<String>,
}

enum NameMatcher {
    All,
    Substring(String),
    Glob(globset::GlobMatcher),
    Regex(regex::Regex),
}

impl NameMatcher {
    fn new(text: &str, mode: MatchMode) -> Result<NameMatcher> {
        if text.is_empty() {
            return Ok(NameMatcher::All);
        }
        let mode = match mode {
            MatchMode::Auto if text.len() > 2 && text.starts_with('/') && text.ends_with('/') => {
                return Self::regex(&text[1..text.len() - 1]);
            }
            MatchMode::Auto if text.contains(['*', '?', '[']) => MatchMode::Glob,
            MatchMode::Auto => MatchMode::Substring,
            m => m,
        };
        match mode {
            MatchMode::Glob => {
                let glob = globset::GlobBuilder::new(text)
                    .case_insensitive(true)
                    .literal_separator(false)
                    .build()
                    .map_err(crate::bad_pattern)?;
                Ok(NameMatcher::Glob(glob.compile_matcher()))
            }
            MatchMode::Regex => Self::regex(text),
            _ => Ok(NameMatcher::Substring(fold(text))),
        }
    }

    fn regex(pattern: &str) -> Result<NameMatcher> {
        let re = regex::RegexBuilder::new(pattern).case_insensitive(true).build().map_err(crate::bad_pattern)?;
        Ok(NameMatcher::Regex(re))
    }

    fn is_match(&self, name: &str) -> bool {
        match self {
            NameMatcher::All => true,
            NameMatcher::Substring(needle) => fold(name).contains(needle.as_str()),
            NameMatcher::Glob(g) => g.is_match(name),
            NameMatcher::Regex(r) => r.is_match(name),
        }
    }
}

/// A compiled [`SearchQuery`].
pub(crate) struct Filter {
    name: NameMatcher,
    kind: KindFilter,
    extensions: Vec<String>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    modified_after: Option<i64>,
    modified_before: Option<i64>,
    pub include_hidden: bool,
    exclude_dirs: Vec<String>,
}

impl Filter {
    pub fn new(q: &SearchQuery) -> Result<Filter> {
        Ok(Filter {
            name: NameMatcher::new(q.text.trim(), q.mode)?,
            kind: q.kind,
            extensions: q.extensions.iter().map(|e| e.trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()).collect(),
            min_size: q.min_size,
            max_size: q.max_size,
            modified_after: q.modified_after,
            modified_before: q.modified_before,
            include_hidden: q.include_hidden,
            exclude_dirs: q.exclude_dirs.iter().map(|d| d.to_lowercase()).collect(),
        })
    }

    /// Is `e` a hit? (Folders are still descended into when they aren't.)
    pub fn matches(&self, e: &Entry) -> bool {
        if e.hidden && !self.include_hidden {
            return false;
        }
        match self.kind {
            KindFilter::File if e.is_dir => return false,
            KindFilter::Dir if !e.is_dir => return false,
            _ => {}
        }
        if !self.extensions.is_empty() {
            if e.is_dir {
                return false;
            }
            let ext = e.name.rsplit_once('.').map(|(_, x)| x.to_lowercase());
            if !ext.is_some_and(|x| self.extensions.contains(&x)) {
                return false;
            }
        }
        // Size filters only make sense for files.
        if !e.is_dir {
            if self.min_size.is_some_and(|m| e.size < m) || self.max_size.is_some_and(|m| e.size > m) {
                return false;
            }
        } else if self.min_size.is_some() || self.max_size.is_some() {
            return false;
        }
        if self.modified_after.is_some() || self.modified_before.is_some() {
            let Some(t) = e.modified else { return false };
            if self.modified_after.is_some_and(|a| t < a) || self.modified_before.is_some_and(|b| t > b) {
                return false;
            }
        }
        self.name.is_match(&e.name)
    }

    /// Should the walk go into `e`? Symlinked folders are never followed:
    /// they can form loops and usually point at something indexed elsewhere.
    pub fn descend(&self, e: &Entry) -> bool {
        e.is_dir && e.kind == EntryKind::Dir && (self.include_hidden || !e.hidden) && !self.is_excluded(&e.name)
    }

    pub fn is_excluded(&self, name: &str) -> bool {
        !self.exclude_dirs.is_empty() && self.exclude_dirs.contains(&name.to_lowercase())
    }
}

/// Pseudo file systems and OS plumbing that are never useful to search and
/// can be huge or endless. Skipped unless the search starts inside them.
#[cfg(target_os = "macos")]
const NOISE: &[&str] = &["/System/Volumes", "/dev", "/private/var/vm"];
#[cfg(all(unix, not(target_os = "macos")))]
const NOISE: &[&str] = &["/proc", "/sys", "/dev"];
#[cfg(not(unix))]
const NOISE: &[&str] = &[];

pub(crate) fn is_noise(loc: &Location) -> bool {
    loc.local_path().is_some_and(|p| NOISE.iter().any(|n| p == std::path::Path::new(n)))
}
