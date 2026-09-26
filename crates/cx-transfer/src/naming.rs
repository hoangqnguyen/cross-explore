//! Explorer-style names for duplicates.

use cx_core::{CxError, Location, Provider, Result};

/// "report.pdf" → ("report", ".pdf"). Folders and dot-files have no extension.
fn split_ext(name: &str, is_dir: bool) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Name for the `n`-th duplicate made next to the original:
/// "a - Copy.txt", "a - Copy (2).txt", …
pub fn copy_name(name: &str, is_dir: bool, n: u32) -> String {
    let (stem, ext) = split_ext(name, is_dir);
    if n <= 1 { format!("{stem} - Copy{ext}") } else { format!("{stem} - Copy ({n}){ext}") }
}

/// "Keep both" name: "a (2).txt", "a (3).txt", … (`n` starts at 2).
pub fn numbered_name(name: &str, is_dir: bool, n: u32) -> String {
    let (stem, ext) = split_ext(name, is_dir);
    format!("{stem} ({n}){ext}")
}

/// First candidate (for n = first, first+1, …) that does not exist in `dir`.
pub(crate) async fn free_name(p: &dyn Provider, dir: &Location, first: u32, candidate: impl Fn(u32) -> String) -> Result<String> {
    for n in first..first + 10_000 {
        let name = candidate(n);
        match p.stat(&dir.join(&name)).await {
            Err(CxError::NotFound(_)) => return Ok(name),
            Ok(_) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(CxError::AlreadyExists(candidate(first)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_style_names() {
        assert_eq!(copy_name("a.txt", false, 1), "a - Copy.txt");
        assert_eq!(copy_name("a.txt", false, 2), "a - Copy (2).txt");
        assert_eq!(copy_name("my.dir", true, 1), "my.dir - Copy");
        assert_eq!(copy_name(".bashrc", false, 1), ".bashrc - Copy");
        assert_eq!(numbered_name("a.tar.gz", false, 2), "a.tar (2).gz");
        assert_eq!(numbered_name("Photos", true, 3), "Photos (3)");
    }
}
