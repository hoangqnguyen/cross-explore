//! Sorting rows: folders first, then by the chosen key, then by name with
//! natural number ordering ("file2" before "file10"), like Finder and the
//! desktop UI's `Intl.Collator({numeric: true})`.
//!
//! Comparisons run on each item's pre-lowercased name, without allocating,
//! so sorting a 100 000-row folder stays well under a frame in release
//! builds.

use crate::folder::Item;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Type,
    Size,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Modified => "Date modified",
            SortKey::Type => "Type",
            SortKey::Size => "Size",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SortSpec {
    pub key: SortKey,
    pub desc: bool,
}

impl SortSpec {
    /// Clicking the same column flips the order; dates and sizes read best
    /// newest / largest first on the first click.
    pub fn toggled(self, key: SortKey) -> SortSpec {
        if self.key == key {
            SortSpec {
                key,
                desc: !self.desc,
            }
        } else {
            SortSpec {
                key,
                desc: matches!(key, SortKey::Modified | SortKey::Size),
            }
        }
    }
}

/// Compare two strings with runs of ASCII digits compared by value.
pub fn natural(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (x, y) = (a[i], b[j]);
        if x.is_ascii_digit() && y.is_ascii_digit() {
            let si = i;
            while i < a.len() && a[i] == b'0' {
                i += 1;
            }
            let sj = j;
            while j < b.len() && b[j] == b'0' {
                j += 1;
            }
            let (ni, nj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            // Longer run of significant digits is the larger number.
            let ord = (i - ni)
                .cmp(&(j - nj))
                .then_with(|| a[ni..i].cmp(&b[nj..j]));
            if ord != Ordering::Equal {
                return ord;
            }
            // Equal values: fewer leading zeros first.
            let zeros = (ni - si).cmp(&(nj - sj));
            if zeros != Ordering::Equal {
                return zeros;
            }
            continue;
        }
        if x != y {
            return x.cmp(&y);
        }
        i += 1;
        j += 1;
    }
    (a.len() - i).cmp(&(b.len() - j))
}

pub fn compare(spec: SortSpec, a: &Item, b: &Item) -> Ordering {
    if a.entry.is_dir != b.entry.is_dir {
        return if a.entry.is_dir {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let by_key = match spec.key {
        SortKey::Name => natural(&a.lname, &b.lname),
        SortKey::Size => a.entry.size.cmp(&b.entry.size),
        SortKey::Modified => a
            .entry
            .modified
            .unwrap_or(0)
            .cmp(&b.entry.modified.unwrap_or(0)),
        SortKey::Type => a.lext().cmp(b.lext()),
    };
    let by_key = if spec.desc { by_key.reverse() } else { by_key };
    by_key
        .then_with(|| natural(&a.lname, &b.lname))
        .then_with(|| a.entry.name.cmp(&b.entry.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "file2", "file1", "file02", "a", "file"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, vec!["a", "file", "file1", "file2", "file02", "file10"]);
    }

    #[test]
    fn toggling() {
        let s = SortSpec::default();
        assert_eq!(
            s.toggled(SortKey::Name),
            SortSpec {
                key: SortKey::Name,
                desc: true
            }
        );
        assert_eq!(
            s.toggled(SortKey::Size),
            SortSpec {
                key: SortKey::Size,
                desc: true
            }
        );
        assert_eq!(
            s.toggled(SortKey::Type),
            SortSpec {
                key: SortKey::Type,
                desc: false
            }
        );
    }
}
