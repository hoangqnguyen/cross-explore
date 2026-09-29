//! In-process image thumbnails: decode with `image`, turn upright using the
//! EXIF orientation, shrink with `fast_image_resize` (SIMD, several times
//! faster than `image`'s own resize) and encode.
//!
//! Output format: PNG when the image has an alpha channel (icons, UI
//! screenshots), JPEG q85 otherwise (photos), which keeps photo thumbnails
//! around a tenth of the size of a PNG.

use crate::cache::Cached;
use cx_core::{CxError, Result};
use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::io::{BufRead, BufReader, Cursor, Seek};
use std::path::Path;

/// Extensions the pure-Rust decoders handle.
const DECODABLE: &[&str] = &["jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "webp", "bmp", "dib", "tif", "tiff", "ico"];

pub(crate) fn is_decodable(ext: &str) -> bool {
    DECODABLE.contains(&ext)
}

pub(crate) fn from_path(path: &Path, size_px: u32) -> Result<Cached> {
    let f = std::fs::File::open(path).map_err(|e| CxError::from_io(e, path.display()))?;
    let reader = ImageReader::new(BufReader::new(f)).with_guessed_format().map_err(|e| CxError::from_io(e, path.display()))?;
    make(reader, size_px)
}

pub(crate) fn from_bytes(bytes: &[u8], size_px: u32) -> Result<Cached> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| CxError::io("image", e))?;
    make(reader, size_px)
}

fn make<R: BufRead + Seek>(reader: ImageReader<R>, size_px: u32) -> Result<Cached> {
    let mut decoder = reader.into_decoder().map_err(unsupported)?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let img = DynamicImage::from_decoder(decoder).map_err(unsupported)?;
    // Resize before rotating: rotating the small image is far cheaper, and
    // "fit into a square" gives the same box either way.
    let mut img = resize(img, size_px)?;
    img.apply_orientation(orientation);
    encode(img)
}

fn unsupported(e: image::ImageError) -> CxError {
    CxError::Unsupported(format!("image: {e}"))
}

/// Largest size that fits in `max`×`max`, keeping the aspect ratio, never
/// larger than the original.
pub(crate) fn fit(w: u32, h: u32, max: u32) -> (u32, u32) {
    if w <= max && h <= max {
        return (w.max(1), h.max(1));
    }
    let scale = max as f64 / w.max(h) as f64;
    (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1))
}

pub(crate) fn resize(img: DynamicImage, max: u32) -> Result<DynamicImage> {
    let alpha = img.color().has_alpha();
    let src = if alpha { DynamicImage::ImageRgba8(img.into_rgba8()) } else { DynamicImage::ImageRgb8(img.into_rgb8()) };
    let (w, h) = fit(src.width(), src.height(), max);
    if (w, h) == (src.width(), src.height()) {
        return Ok(src);
    }
    let mut dst = DynamicImage::new(w, h, src.color());
    Resizer::new()
        .resize(&src, &mut dst, &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::CatmullRom)))
        .map_err(|e| CxError::io("resize", e))?;
    Ok(dst)
}

pub(crate) fn encode(img: DynamicImage) -> Result<Cached> {
    let (width, height) = (img.width(), img.height());
    let mut out = Vec::new();
    let mime = if img.color().has_alpha() {
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|e| CxError::io("png", e))?;
        "image/png"
    } else {
        let rgb = img.into_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
            .encode_image(&rgb)
            .map_err(|e| CxError::io("jpeg", e))?;
        "image/jpeg"
    };
    Ok(Cached { bytes: out, mime, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit(4000, 3000, 256), (256, 192));
        assert_eq!(fit(3000, 4000, 256), (192, 256));
        assert_eq!(fit(100, 50, 256), (100, 50));
        assert_eq!(fit(10000, 1, 100), (100, 1));
    }
}

/// Diagnostic, run by hand: decode every image under CX_SCAN_DIRS
/// (colon-separated) and report any that panic the decoder.
#[cfg(test)]
mod scan {
    #[test]
    #[ignore]
    fn scan_for_decoder_panics() {
        let Ok(dirs) = std::env::var("CX_SCAN_DIRS") else { return };
        std::panic::set_hook(Box::new(|_| {}));
        let (mut n, mut bad) = (0, 0);
        let mut stack: Vec<std::path::PathBuf> = dirs.split(':').map(Into::into).collect();
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if stack.len() < 5000 && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                        stack.push(p);
                    }
                    continue;
                }
                let ext = crate::extension(&p.to_string_lossy());
                if !super::is_decodable(&ext) {
                    continue;
                }
                n += 1;
                for size in [64u32, 256] {
                    if std::panic::catch_unwind(|| super::from_path(&p, size)).is_err() {
                        bad += 1;
                        println!("PANIC {} @{size}", p.display());
                        break;
                    }
                }
            }
        }
        println!("scanned {n} images, {bad} panicked");
    }
}
