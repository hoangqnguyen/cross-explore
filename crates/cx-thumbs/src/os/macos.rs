//! QuickLook thumbnails (`QLThumbnailGenerator`): the same previews Finder
//! shows, for every type that has a QuickLook extension installed.
//!
//! The generator is asynchronous and reports through a completion handler
//! on one of its own queues; the handler converts the CGImage to PNG right
//! there and hands the bytes to a tokio oneshot. If the awaiting future is
//! dropped, the request is cancelled so QuickLook stops working on it.

use crate::cache::Cached;
use block2::RcBlock;
use cx_core::{CxError, Result};
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSDictionary, NSError, NSString, NSURL};
use objc2_quick_look_thumbnailing::{
    QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes, QLThumbnailGenerator, QLThumbnailRepresentation,
};
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::oneshot;

pub(crate) async fn thumbnail(path: &Path, size_px: u32) -> Result<Cached> {
    let Some(path_str) = path.to_str() else {
        return Err(CxError::Unsupported(format!("non-UTF-8 path {}", path.display())));
    };
    let (tx, rx) = oneshot::channel::<Result<Cached>>();
    // Objective-C objects aren't `Send`, so they must not live across the
    // `.await`; only the cancel guard (below) does.
    let mut pending = {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));
        let side = size_px as f64;
        // SAFETY: plain initializer with valid arguments.
        let request = unsafe {
            QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
                QLThumbnailGenerationRequest::alloc(),
                &url,
                CGSize { width: side, height: side },
                1.0,
                // Real thumbnails only: generic type icons are drawn by the UI.
                QLThumbnailGenerationRequestRepresentationTypes::Thumbnail,
            )
        };
        let tx = Mutex::new(Some(tx));
        let handler = RcBlock::new(move |rep: *mut QLThumbnailRepresentation, err: *mut NSError| {
            // SAFETY: QuickLook passes either a valid representation or a valid error.
            let result = unsafe { convert(rep, err) };
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(result);
            }
        });
        // SAFETY: the generator is thread-safe; QuickLook copies the block.
        let generator = unsafe { QLThumbnailGenerator::sharedGenerator() };
        unsafe { generator.generateBestRepresentationForRequest_completionHandler(&request, &handler) };
        Pending { generator, request, done: false }
    };
    let result = rx.await;
    pending.done = true;
    result.map_err(|_| CxError::Io("QuickLook dropped the request".into()))?
}

/// Cancels an unfinished request when the awaiting future is dropped.
struct Pending {
    generator: Retained<QLThumbnailGenerator>,
    request: Retained<QLThumbnailGenerationRequest>,
    done: bool,
}

// SAFETY: QLThumbnailGenerator is documented as thread-safe, and the request
// is immutable after creation; we only pass it back to `cancelRequest`.
unsafe impl Send for Pending {}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.done {
            // SAFETY: cancelling a request we submitted to this generator.
            unsafe { self.generator.cancelRequest(&self.request) };
        }
    }
}

unsafe fn convert(rep: *mut QLThumbnailRepresentation, err: *mut NSError) -> Result<Cached> {
    let Some(rep) = rep.as_ref() else {
        let msg = err.as_ref().map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "no thumbnail".into());
        return Err(CxError::Unsupported(format!("QuickLook: {msg}")));
    };
    let cg = rep.CGImage();
    let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    let data = bitmap
        .representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        .ok_or_else(|| CxError::Io("QuickLook: PNG encoding failed".into()))?;
    Ok(Cached { bytes: data.to_vec(), mime: "image/png", width: bitmap.pixelsWide() as u32, height: bitmap.pixelsHigh() as u32 })
}
