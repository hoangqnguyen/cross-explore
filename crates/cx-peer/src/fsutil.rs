//! Small file helpers shared by the stores and the receiver.

use cx_core::{CxError, Result};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Replace `path` atomically (write a sibling, then rename) so a crash never
/// leaves a half-written trust store behind.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_with(path, bytes, false)
}

/// Like [`write_atomic`], readable only by the owner (secret keys).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    write_with(path, bytes, true)
}

fn write_with(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let err = |e: io::Error| CxError::from_io(e, path.display());
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut f = opts.open(&tmp).map_err(err)?;
    f.write_all(bytes).map_err(err)?;
    f.sync_all().map_err(err)?;
    drop(f);
    fs::rename(&tmp, path).map_err(err)
}

/// "name.ext", "name (2).ext", "name (3).ext", ... : the first candidate for
/// which `try_create` does not report `AlreadyExists`.
pub fn keep_both<T>(dir: &Path, name: &str, mut try_create: impl FnMut(&Path) -> io::Result<T>) -> io::Result<(PathBuf, T)> {
    let (stem, ext) = split_ext(name);
    for n in 1..10_000 {
        let candidate = if n == 1 { name.to_string() } else { format!("{stem} ({n}){ext}") };
        let path = dir.join(&candidate);
        match try_create(&path) {
            Ok(v) => return Ok((path, v)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, name.to_string()))
}

/// Split "archive.tar.gz" as ("archive.tar", ".gz"); dot-files have no extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_both_numbers_before_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        let create = |p: &Path| fs::OpenOptions::new().write(true).create_new(true).open(p);
        let names: Vec<String> = (0..3)
            .map(|_| keep_both(dir.path(), "photo.jpg", create).unwrap().0.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["photo.jpg", "photo (2).jpg", "photo (3).jpg"]);
        let (p, _) = keep_both(dir.path(), ".env", create).unwrap();
        assert_eq!(p.file_name().unwrap(), ".env");
        let (p, _) = keep_both(dir.path(), ".env", create).unwrap();
        assert_eq!(p.file_name().unwrap(), ".env (2)");
    }
}
