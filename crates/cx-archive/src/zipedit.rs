//! Editing zip archives by rewriting them.
//!
//! Zip has no safe in-place delete or rename, so every edit copies the
//! archive to a new file, raw-copying the untouched members (no
//! recompression, so it runs at disk speed), and the new file then replaces
//! the old one. Good enough for a first version; appending new members in
//! place would be a later optimisation.

use crate::index::sanitize;
use crate::time::ms_to_dos;
use crate::walk::zip_err;
use cx_core::{CxError, Result};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Seek, Write};
use std::path::{Path, PathBuf};
use zip::write::FullFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[derive(Debug, Clone)]
pub(crate) enum Edit {
    AddDir { key: String, modified: i64 },
    /// Add a file (replacing a member with the same path) from a local file.
    AddFile { key: String, src: PathBuf, modified: i64 },
    /// Remove a member, or a folder and everything in it.
    Remove { key: String },
    /// Rename a member, or a folder and everything in it.
    Rename { from: String, to: String },
}

/// `key` is `prefix` itself or inside it.
pub(crate) fn is_under(key: &str, prefix: &str) -> bool {
    key == prefix || (key.len() > prefix.len() && key.starts_with(prefix) && key.as_bytes()[prefix.len()] == b'/')
}

/// Formats that don't shrink: storing them saves a lot of CPU for nothing.
fn already_compressed(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(
        ext.as_str(),
        "zip" | "7z" | "rar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "avif" | "mp3" | "m4a" | "aac" | "ogg" | "opus" | "flac" | "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "docx" | "xlsx" | "pptx" | "jar" | "apk" | "dmg"
    )
}

/// Options for a new member: deflate (or store), a DOS time plus the "UT"
/// extra field so other tools see the right UTC time, zip64 when needed.
pub(crate) fn member_options(name: &str, modified: Option<i64>, size: u64) -> FullFileOptions<'static> {
    let method = if already_compressed(name) { CompressionMethod::Stored } else { CompressionMethod::Deflated };
    let mut opts = FullFileOptions::default().compression_method(method).large_file(size >= u32::MAX as u64);
    if let Some(ms) = modified {
        opts = opts.last_modified_time(ms_to_dos(ms));
        let secs = ms.div_euclid(1000);
        if (0..=u32::MAX as i64).contains(&secs) {
            let mut ut = vec![1u8]; // flags: modification time present
            ut.extend_from_slice(&(secs as u32).to_le_bytes());
            let _ = opts.add_extra_data(0x5455, ut, false);
        }
    }
    opts
}

pub(crate) fn dir_options(modified: Option<i64>) -> FullFileOptions<'static> {
    let mut o = member_options("", modified, 0);
    o = o.compression_method(CompressionMethod::Stored);
    o
}

/// Write `src` with `edit` applied into `dst`.
pub(crate) fn rewrite(src: &Path, dst: File, edit: &Edit) -> Result<()> {
    let ctx = src.display();
    let zerr = |e| zip_err(e, &ctx);
    let mut archive = zip::ZipArchive::new(BufReader::new(File::open(src).map_err(|e| CxError::from_io(e, &ctx))?)).map_err(zerr)?;
    let mut out = ZipWriter::new(BufWriter::new(dst));
    for i in 0..archive.len() {
        let f = archive.by_index_raw(i).map_err(zerr)?;
        let key = sanitize(f.name());
        let Some(key) = key else {
            // Unsafe names are invisible in the UI; keep them untouched.
            out.raw_copy_file(f).map_err(zerr)?;
            continue;
        };
        match edit {
            Edit::Remove { key: p } if is_under(&key, p) => {}
            Edit::AddFile { key: k, .. } | Edit::AddDir { key: k, .. } if &key == k => {}
            Edit::Rename { from, to } if is_under(&key, from) => {
                let mut name = format!("{to}{}", &key[from.len()..]);
                if f.is_dir() {
                    name.push('/');
                }
                out.raw_copy_file_rename(f, name).map_err(zerr)?;
            }
            _ => out.raw_copy_file(f).map_err(zerr)?,
        }
    }
    match edit {
        Edit::AddDir { key, modified } => {
            out.add_directory(format!("{key}/"), dir_options(Some(*modified))).map_err(zerr)?;
        }
        Edit::AddFile { key, src: file, modified } => {
            let mut input = File::open(file).map_err(|e| CxError::from_io(e, file.display()))?;
            let size = input.metadata().map(|m| m.len()).unwrap_or(0);
            out.start_file(key.as_str(), member_options(key, Some(*modified), size)).map_err(zerr)?;
            io::copy(&mut input, &mut out).map_err(|e| CxError::from_io(e, file.display()))?;
        }
        _ => {}
    }
    finish(out, &ctx)
}

/// Finish a zip and flush it to disk.
pub(crate) fn finish<W: Write + Seek>(out: ZipWriter<BufWriter<W>>, ctx: &impl std::fmt::Display) -> Result<()> {
    let buffered = out.finish().map_err(|e| zip_err(e, ctx))?;
    buffered.into_inner().map_err(|e| CxError::from_io(e.into_error(), ctx))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_matching() {
        assert!(is_under("a/b", "a"));
        assert!(is_under("a", "a"));
        assert!(!is_under("ab", "a"));
        assert!(already_compressed("x.JPG"));
        assert!(!already_compressed("notes.txt"));
    }
}
