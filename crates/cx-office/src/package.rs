//! Zip-based packages: OOXML (Open Packaging Conventions) and OpenDocument.
//!
//! Parts are read fully into memory with a per-part cap, so a "zip bomb"
//! (a tiny entry that inflates to gigabytes) fails cleanly instead of
//! exhausting memory. OOXML parts reference each other through `.rels`
//! files; [`Package::rels`] resolves those targets to absolute part names.

use crate::xml::{self, El};
use cx_core::{CxError, Result};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use zip::ZipArchive;

/// Largest single part we inflate. Word's document.xml for a 1,000-page
/// report is a few tens of megabytes.
pub(crate) const MAX_PART_BYTES: u64 = 96 << 20;

pub(crate) struct Package {
    zip: ZipArchive<Cursor<Vec<u8>>>,
    /// Lower-cased part name → actual name, since producers disagree on
    /// case and the spec says part names are case-insensitive.
    names: HashMap<String, String>,
}

/// One relationship from a `.rels` part.
#[derive(Debug, Clone)]
pub(crate) struct Rel {
    pub id: String,
    /// Last path segment of the relationship type URI (`image`, `slide`…).
    pub kind: String,
    /// Absolute part name without leading slash, or the raw target when
    /// external.
    pub target: String,
    pub external: bool,
}

impl Package {
    pub fn open(bytes: Vec<u8>) -> Result<Package> {
        let zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| CxError::Unsupported(format!("not a valid document package: {e}")))?;
        let names = zip.file_names().map(|n| (n.trim_start_matches('/').to_ascii_lowercase(), n.to_string())).collect();
        Ok(Package { zip, names })
    }

    /// Bytes of a part, `None` if missing, error if unreadable or too big.
    pub fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        let Some(real) = self.names.get(&name.trim_start_matches('/').to_ascii_lowercase()).cloned() else {
            return Ok(None);
        };
        let file = self.zip.by_name(&real).map_err(|e| CxError::io(&real, e))?;
        let mut out = Vec::with_capacity(file.size().min(MAX_PART_BYTES) as usize);
        file.take(MAX_PART_BYTES + 1).read_to_end(&mut out).map_err(|e| CxError::io(&real, e))?;
        if out.len() as u64 > MAX_PART_BYTES {
            return Err(CxError::Unsupported(format!("{real} is too large to preview")));
        }
        Ok(Some(out))
    }

    /// A part parsed as XML (`None` if missing).
    pub fn xml(&mut self, name: &str) -> Result<Option<El>> {
        match self.read(name)? {
            Some(bytes) => xml::parse(&bytes).map(Some).map_err(|e| CxError::Unsupported(format!("{name}: {e}"))),
            None => Ok(None),
        }
    }

    /// Relationships of `part` (from `<dir>/_rels/<file>.rels`). Missing or
    /// broken rels simply mean "no relationships".
    pub fn rels(&mut self, part: &str) -> Vec<Rel> {
        let part = part.trim_start_matches('/');
        let (dir, file) = match part.rsplit_once('/') {
            Some((d, f)) => (d, f),
            None => ("", part),
        };
        let rels_name = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
        let Ok(Some(tree)) = self.xml(&rels_name) else { return Vec::new() };
        let Some(root) = xml::root(&tree) else { return Vec::new() };
        root.elements()
            .filter(|e| e.name.ends_with("Relationship"))
            .filter_map(|e| {
                let id = e.attr("Id")?.to_string();
                let target = e.attr("Target")?;
                let kind = e.attr("Type").unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
                let external = e.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
                let target = if external { target.to_string() } else { resolve(dir, target) };
                Some(Rel { id, kind, target, external })
            })
            .collect()
    }

    /// The part the package's root relationship of type `kind` points at
    /// (`officeDocument` → `word/document.xml`, `ppt/presentation.xml`…).
    pub fn main_part(&mut self, kind: &str) -> Option<String> {
        self.rels("").into_iter().find(|r| r.kind == kind && !r.external).map(|r| r.target)
    }

    /// `dc:title` from `docProps/core.xml`, if any.
    pub fn core_title(&mut self) -> Option<String> {
        let tree = self.xml("docProps/core.xml").ok()??;
        let t = tree.find("dc:title")?.text();
        let t = t.trim();
        (!t.is_empty()).then(|| t.to_string())
    }
}

/// Resolve a relative target against the directory of the source part.
/// Absolute targets start at the package root. `..` never escapes the root.
pub(crate) fn resolve(dir: &str, target: &str) -> String {
    let target = target.split(['#', '?']).next().unwrap_or("");
    let mut parts: Vec<&str> = if target.starts_with('/') { Vec::new() } else { dir.split('/').filter(|s| !s.is_empty()).collect() };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Relationship lookup by id.
pub(crate) fn rel_by_id<'a>(rels: &'a [Rel], id: &str) -> Option<&'a Rel> {
    rels.iter().find(|r| r.id == id)
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn targets_resolve() {
        assert_eq!(resolve("word", "media/image1.png"), "word/media/image1.png");
        assert_eq!(resolve("ppt/slides", "../media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve("ppt/slides", "/ppt/media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve("", "../../../etc/passwd"), "etc/passwd");
    }
}
