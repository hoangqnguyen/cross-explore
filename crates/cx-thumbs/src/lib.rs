//! Thumbnails, Quick Look text previews and media info for Cross Explore.
//!
//! Images are decoded in-process with the pure-Rust `image` crate (so they
//! work for any provider, remote ones included, and on every platform).
//! Everything else (video, PDF, office documents, HEIC, fonts…) goes to the
//! OS thumbnailer for local files: QuickLook on macOS, the Shell's
//! `IShellItemImageFactory` on Windows, and `ffmpegthumbnailer`/`pdftoppm`
//! on Linux when they are installed. Results land in a disk LRU cache keyed
//! by URI, size, modification time and file size, so a folder that was
//! scrolled once paints instantly the next time.

mod cache;
mod image_thumb;
mod media;
mod os;
mod read;
mod text;
mod thumbnailer;

pub use cache::{CacheStats, DiskCache};
pub use media::{media_info, MediaInfo};
pub use text::{language_for_name, preview_text, TextPreview};
pub use thumbnailer::{Thumb, Thumbnailer, MAX_REMOTE_IMAGE_BYTES};

/// Lower-cased extension of a file name ("" when there is none).
pub(crate) fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}
