//! Back / forward from mouse side buttons (MX Master and friends) and
//! trackpad swipes on macOS. WKWebView doesn't pass the side buttons on to
//! the page, so we catch them natively, before the web view sees them, and
//! tell the UI to navigate.

use crate::events::Events;
use std::sync::Arc;

#[cfg(target_os = "macos")]
pub fn install(events: Arc<Events>) {
    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
    use std::ptr::NonNull;

    let block = RcBlock::new(move |ev: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a valid event for this call.
        let e = unsafe { ev.as_ref() };
        let dir = match e.r#type() {
            // Button numbers: 0 left, 1 right, 2 middle, 3 back, 4 forward.
            t if t == NSEventType::OtherMouseDown => match e.buttonNumber() {
                3 => Some("back"),
                4 => Some("forward"),
                _ => None,
            },
            // Swipe between pages: deltaX > 0 means back (like Safari/Finder).
            t if t == NSEventType::Swipe => {
                let dx = e.deltaX();
                if dx > 0.0 {
                    Some("back")
                } else if dx < 0.0 {
                    Some("forward")
                } else {
                    None
                }
            }
            _ => None,
        };
        match dir {
            Some(dir) => {
                events.emit("nav", serde_json::json!({ "dir": dir }));
                std::ptr::null_mut() // handled: don't pass it on
            }
            None => ev.as_ptr(),
        }
    });
    // SAFETY: called on the main thread during setup; the monitor lives for
    // the rest of the app, so the returned token is intentionally leaked.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::OtherMouseDown | NSEventMask::Swipe,
            &block,
        )
    };
    std::mem::forget(monitor);
    std::mem::forget(block);
}

/// Elsewhere the web view delivers side buttons as mouse buttons 3 / 4 and
/// the UI handles them itself.
#[cfg(not(target_os = "macos"))]
pub fn install(_events: Arc<Events>) {}
