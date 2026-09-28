//! OS thumbnailers for local files the pure-Rust decoders can't handle
//! (video, PDF, office documents, HEIC, fonts…). Each platform lives in its
//! own module so the FFI stays isolated from the rest of the crate.

use crate::cache::Cached;
use cx_core::Result;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
mod win;

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
mod linux;

/// What to ask the OS for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Want {
    /// A real preview of the content; fails rather than return a type icon.
    Thumbnail,
    /// The file's own icon (apps and programs carry one).
    Icon,
}

/// Ask the OS for a thumbnail of the local file at `path`, fitting in
/// `size_px`. `Unsupported` when the platform has nothing for it.
pub(crate) async fn thumbnail(path: PathBuf, size_px: u32, want: Want) -> Result<Cached> {
    #[cfg(target_os = "macos")]
    {
        macos::thumbnail(&path, size_px, want).await
    }
    #[cfg(windows)]
    {
        crate::read::blocking(move || win::thumbnail(&path, size_px, want)).await
    }
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
    {
        if want == Want::Icon {
            return Err(cx_core::CxError::Unsupported("app icons".into()));
        }
        crate::read::blocking(move || linux::thumbnail(&path, size_px)).await
    }
    #[cfg(not(any(target_os = "macos", windows, all(unix, not(any(target_os = "ios", target_os = "android"))))))]
    {
        let _ = (size_px, want);
        Err(cx_core::CxError::Unsupported(format!("no system thumbnailer for {}", path.display())))
    }
}
