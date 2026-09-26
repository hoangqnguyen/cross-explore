//! Fuzzy ranking for the command palette (actions, favorites, devices,
//! recent folders): a few hundred candidates, re-ranked on every keystroke.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FuzzyMatch {
    /// Index into the candidate slice.
    pub index: usize,
    pub score: u32,
    /// Sorted UTF-16 code-unit offsets of the matched characters, ready to
    /// highlight with JavaScript string indices.
    pub positions: Vec<u32>,
}

/// Rank `candidates` against `query`; non-matches are left out.
///
/// Smart case (an uppercase letter in the query makes it case-sensitive) and
/// Unicode normalization (é matches e) follow fzf/nucleo conventions; spaces
/// separate terms that must all match, in any order. Sorted by score, then by
/// shorter candidate, then by original order so results don't jitter. An
/// empty query returns every candidate in order with score 0.
pub fn fuzzy_rank(query: &str, candidates: &[String]) -> Vec<FuzzyMatch> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut indices = Vec::new();
    let mut out: Vec<FuzzyMatch> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, cand)| {
            indices.clear();
            let score = pattern.indices(Utf32Str::new(cand, &mut buf), &mut matcher, &mut indices)?;
            indices.sort_unstable();
            indices.dedup();
            Some(FuzzyMatch { index, score, positions: to_utf16(cand, &indices) })
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| candidates[a.index].len().cmp(&candidates[b.index].len()))
            .then_with(|| a.index.cmp(&b.index))
    });
    out
}

/// Map sorted char indices to UTF-16 offsets.
fn to_utf16(s: &str, char_indices: &[u32]) -> Vec<u32> {
    if s.is_ascii() {
        return char_indices.to_vec();
    }
    let mut out = Vec::with_capacity(char_indices.len());
    let mut want = char_indices.iter().peekable();
    let mut units = 0u32;
    for (i, c) in s.chars().enumerate() {
        match want.peek() {
            Some(&&w) if w as usize == i => {
                out.push(units);
                want.next();
            }
            None => break,
            _ => {}
        }
        units += c.len_utf16() as u32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(q: &str, c: &[&str]) -> Vec<String> {
        let c: Vec<String> = c.iter().map(|s| s.to_string()).collect();
        fuzzy_rank(q, &c).into_iter().map(|m| c[m.index].clone()).collect()
    }

    #[test]
    fn ranks_prefix_and_contiguous_first() {
        let r = names("dl", &["Delete", "Downloads", "New folder", "Duplicate tab"]);
        assert!(!r.contains(&"New folder".to_string()));
        assert_eq!(r.len(), 3);
        let r = names("down", &["Go down a level", "Downloads", "Dropbox"]);
        assert_eq!(r[0], "Downloads");
        assert!(!r.contains(&"Dropbox".to_string()));
    }

    #[test]
    fn positions_are_utf16_offsets() {
        let c = vec!["😀 Tài liệu".to_string()];
        let m = &fuzzy_rank("tai", &c)[0];
        // The emoji takes two UTF-16 units, the space one: "T" is at 3.
        assert_eq!(m.positions, vec![3, 4, 5]);
    }

    #[test]
    fn empty_query_keeps_everything() {
        let c = vec!["b".to_string(), "a".to_string()];
        let r = fuzzy_rank("", &c);
        assert_eq!(r.iter().map(|m| m.index).collect::<Vec<_>>(), vec![0, 1]);
    }
}
