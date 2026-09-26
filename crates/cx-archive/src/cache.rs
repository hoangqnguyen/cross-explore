//! Getting a container onto the local disk.
//!
//! The codecs need a seekable local file. A local archive is read in place;
//! a remote one (SFTP, SMB, another archive, …) is downloaded once into the
//! cache directory, named after its URI and its size + mtime, so a changed
//! file is fetched again while an unchanged one is reused across sessions.

use cx_core::{CxError, Entry, Location, Result, Vfs};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// FNV-1a: stable across Rust versions (unlike `DefaultHasher`), which
/// matters for file names that must survive restarts.
fn fnv(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in *p {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn cache_name(loc: &Location, stat: &Entry) -> (String, String) {
    let prefix = format!("{:016x}-", fnv(&[loc.uri().as_bytes()]));
    let version = fnv(&[&stat.size.to_le_bytes(), &stat.modified.unwrap_or(0).to_le_bytes()]);
    // Keep the original name's extension(s) so format detection by name
    // still works on the cached copy.
    let name = loc.name();
    let ext = crate::format::ArchiveFormat::from_name(&name).map(|f| f.extension()).unwrap_or("bin");
    (prefix.clone(), format!("{prefix}{version:016x}.{ext}"))
}

/// Stream `loc` into the local file `dst`.
pub(crate) async fn download(vfs: &Vfs, loc: &Location, dst: &Path) -> Result<()> {
    let provider = vfs.provider(loc).await?;
    let mut src = provider.open_read(loc, 0).await?;
    let mut out = tokio::fs::File::create(dst).await.map_err(|e| CxError::from_io(e, dst.display()))?;
    tokio::io::copy(&mut src, &mut out).await.map_err(|e| CxError::io(format!("downloading {loc}"), e))?;
    out.flush().await.map_err(|e| CxError::from_io(e, dst.display()))?;
    Ok(())
}

/// The local file holding `loc`'s bytes: the file itself when local,
/// otherwise a cached download (fetched now if missing or outdated).
pub(crate) async fn materialize(vfs: &Vfs, loc: &Location, stat: &Entry, cache_dir: &Path) -> Result<(PathBuf, bool)> {
    if let Some(p) = loc.local_path() {
        return Ok((p.to_path_buf(), true));
    }
    let dir = cache_dir.join("containers");
    tokio::fs::create_dir_all(&dir).await.map_err(|e| CxError::from_io(e, dir.display()))?;
    let (prefix, name) = cache_name(loc, stat);
    let path = dir.join(&name);
    if tokio::fs::metadata(&path).await.is_ok() {
        return Ok((path, false));
    }
    // Older versions of the same container are useless now.
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(de)) = rd.next_entry().await {
            let n = de.file_name().to_string_lossy().into_owned();
            if n.starts_with(&prefix) && n != name {
                let _ = tokio::fs::remove_file(de.path()).await;
            }
        }
    }
    let part = dir.join(format!("{name}.part"));
    if let Err(e) = download(vfs, loc, &part).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, &path).await.map_err(|e| CxError::from_io(e, path.display()))?;
    Ok((path, false))
}
