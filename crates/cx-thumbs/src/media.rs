//! Cheap media facts for the info pane: image dimensions (from the header,
//! without decoding pixels) and the EXIF fields people actually look at.

use crate::read::{blocking, read_prefix};
use cx_core::{CxError, Location, Result, Vfs};
use image::ImageReader;
use serde::Serialize;
use std::io::{BufRead, BufReader, Cursor, Seek, SeekFrom};

/// Headers and EXIF blocks sit near the start of a file; this much is
/// enough for remote files without downloading them whole.
const REMOTE_PREFIX: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    /// Container format by content ("jpeg", "png", …).
    pub format: Option<String>,
    /// Stored pixel size (before EXIF rotation).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// EXIF DateTimeOriginal as local time, `YYYY-MM-DDTHH:MM:SS`.
    pub date_taken: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    /// EXIF orientation 1–8 (1 = upright).
    pub orientation: Option<u16>,
}

/// Media facts for an image file. `Unsupported` when nothing is recognised.
pub async fn media_info(vfs: &Vfs, loc: &Location) -> Result<MediaInfo> {
    let info = match loc.local_path() {
        Some(path) => {
            let path = path.to_path_buf();
            blocking(move || {
                let f = std::fs::File::open(&path).map_err(|e| CxError::from_io(e, path.display()))?;
                Ok(inspect(BufReader::new(f)))
            })
            .await?
        }
        None => {
            let bytes = read_prefix(vfs, loc, REMOTE_PREFIX).await?;
            blocking(move || Ok(inspect(Cursor::new(bytes)))).await?
        }
    };
    if info == MediaInfo::default() {
        return Err(CxError::Unsupported(format!("no media info for {}", loc.name())));
    }
    Ok(info)
}

pub(crate) fn inspect<R: BufRead + Seek>(mut r: R) -> MediaInfo {
    let mut info = MediaInfo::default();
    if let Ok(reader) = ImageReader::new(&mut r).with_guessed_format() {
        info.format = reader.format().and_then(|f| f.extensions_str().first()).map(|s| s.to_string());
        if let Ok((w, h)) = reader.into_dimensions() {
            info.width = Some(w);
            info.height = Some(h);
        }
    }
    if r.seek(SeekFrom::Start(0)).is_err() {
        return info;
    }
    if let Ok(exif) = exif::Reader::new().read_from_container(&mut r) {
        let text = |tag| {
            let f = exif.get_field(tag, exif::In::PRIMARY)?;
            match &f.value {
                exif::Value::Ascii(v) => v.first().map(|s| String::from_utf8_lossy(s).trim().to_string()).filter(|s| !s.is_empty()),
                _ => None,
            }
        };
        info.camera_make = text(exif::Tag::Make);
        info.camera_model = text(exif::Tag::Model);
        info.lens_model = text(exif::Tag::LensModel);
        info.orientation = exif
            .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
            .and_then(|f| f.value.get_uint(0))
            .map(|v| v as u16);
        info.date_taken = [exif::Tag::DateTimeOriginal, exif::Tag::DateTime].iter().find_map(|&tag| {
            let f = exif.get_field(tag, exif::In::PRIMARY)?;
            let exif::Value::Ascii(v) = &f.value else { return None };
            let d = exif::DateTime::from_ascii(v.first()?).ok()?;
            Some(format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", d.year, d.month, d.day, d.hour, d.minute, d.second))
        });
    }
    info
}
