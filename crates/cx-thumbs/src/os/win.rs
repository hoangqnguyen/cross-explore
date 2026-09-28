//! Windows Shell thumbnails (`IShellItemImageFactory`): the same images
//! Explorer shows, from every installed thumbnail handler (video, PDF,
//! Office, HEIC with the codec pack…).
//!
//! Runs on a blocking worker thread, which joins the multithreaded COM
//! apartment for the duration of the call.

use crate::cache::Cached;
use crate::image_thumb::{encode, resize};
use cx_core::{CxError, Result};
use image::{DynamicImage, RgbaImage};
use std::ffi::c_void;
use std::path::Path;
use windows::core::HSTRING;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, IBindCtx, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY, SIIGBF_THUMBNAILONLY};

struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: paired with a successful CoInitializeEx on this thread.
        unsafe { CoUninitialize() };
    }
}

struct BitmapGuard(HBITMAP);

impl Drop for BitmapGuard {
    fn drop(&mut self) {
        // SAFETY: the bitmap was returned to us by GetImage and is ours to free.
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

fn win_err(e: windows::core::Error) -> CxError {
    CxError::Unsupported(format!("Shell thumbnail: {e}"))
}

pub(crate) fn thumbnail(path: &Path, size_px: u32, want: super::Want) -> Result<Cached> {
    // SAFETY: straightforward COM/GDI calls; every handle is released by a guard.
    unsafe {
        let _com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok().then_some(ComGuard);
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None::<&IBindCtx>).map_err(win_err)?;
        let side = size_px as i32;
        // THUMBNAILONLY: fail instead of returning the generic type icon,
        // which the UI draws itself.
        // Programs: ICONONLY gives the icon embedded in the .exe/.msi/.lnk.
        let flags = if want == super::Want::Icon { SIIGBF(SIIGBF_ICONONLY.0 | SIIGBF_BIGGERSIZEOK.0) } else { SIIGBF(SIIGBF_THUMBNAILONLY.0 | SIIGBF_BIGGERSIZEOK.0) };
        let hbmp = factory.GetImage(SIZE { cx: side, cy: side }, flags).map_err(win_err)?;
        let _bmp = BitmapGuard(hbmp);

        let mut bm = BITMAP::default();
        if GetObjectW(hbmp.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut BITMAP as *mut c_void)) == 0 {
            return Err(CxError::Io("Shell thumbnail: GetObject failed".into()));
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        if w <= 0 || h <= 0 {
            return Err(CxError::Unsupported("Shell thumbnail: empty bitmap".into()));
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative: top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; w as usize * h as usize * 4];
        let hdc = GetDC(None);
        let lines = GetDIBits(hdc, hbmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, hdc);
        if lines == 0 {
            return Err(CxError::Io("Shell thumbnail: GetDIBits failed".into()));
        }
        bgra_premultiplied_to_rgba(&mut buf);
        let img = RgbaImage::from_raw(w as u32, h as u32, buf).ok_or_else(|| CxError::Io("Shell thumbnail: bad buffer".into()))?;
        encode(resize(DynamicImage::ImageRgba8(img), size_px)?)
    }
}

/// Shell bitmaps are BGRA with premultiplied alpha, and many handlers leave
/// alpha at 0 for opaque images; treat an all-zero alpha channel as opaque.
fn bgra_premultiplied_to_rgba(buf: &mut [u8]) {
    let opaque = buf.chunks_exact(4).all(|p| p[3] == 0);
    for p in buf.chunks_exact_mut(4) {
        p.swap(0, 2);
        if opaque {
            p[3] = 255;
        } else if p[3] > 0 && p[3] < 255 {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}
