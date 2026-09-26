//! Linux (and other desktop Unix) has no system thumbnail API a process can
//! call directly, so use the same command-line tools file managers rely on,
//! when installed: `ffmpegthumbnailer` for video and `pdftoppm` (poppler)
//! for PDF. Missing tools simply mean "unsupported".

use crate::cache::Cached;
use cx_core::{CxError, Result};
use std::path::Path;
use std::process::{Command, Stdio};

const VIDEO: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "3gp", "ogv"];

pub(crate) fn thumbnail(path: &Path, size_px: u32) -> Result<Cached> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = crate::extension(&name);
    let dir = tempfile::tempdir().map_err(|e| CxError::io("temp dir", e))?;
    let size = size_px.to_string();
    let png = if VIDEO.contains(&ext.as_str()) {
        let out = dir.path().join("t.png");
        run(Command::new("ffmpegthumbnailer").arg("-i").arg(path).arg("-o").arg(&out).args(["-s", &size, "-c", "png"]))?;
        out
    } else if ext == "pdf" {
        let prefix = dir.path().join("t");
        run(Command::new("pdftoppm").args(["-png", "-singlefile", "-f", "1", "-l", "1", "-scale-to", &size]).arg(path).arg(&prefix))?;
        prefix.with_extension("png")
    } else {
        return Err(CxError::Unsupported(format!("no system thumbnailer for {name}")));
    };
    let img = image::open(&png).map_err(|e| CxError::io("thumbnail", e))?;
    crate::image_thumb::encode(crate::image_thumb::resize(img, size_px)?)
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CxError::Unsupported(format!("{:?} is not installed", cmd.get_program()))
        } else {
            CxError::io(format!("{:?}", cmd.get_program()), e)
        }
    })?;
    if !status.success() {
        return Err(CxError::Unsupported(format!("{:?} failed ({status})", cmd.get_program())));
    }
    Ok(())
}
