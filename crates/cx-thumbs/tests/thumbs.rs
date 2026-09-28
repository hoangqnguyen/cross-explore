use async_trait::async_trait;
use cx_core::{
    Capabilities, Connector, Credentials, CxError, Endpoint, Entry, Location, MemoryCredentials, Provider, ReadStream, Result, Scheme, Vfs,
    WriteMode, WriteStream,
};
use cx_local::LocalProvider;
use cx_thumbs::{media_info, preview_text, Thumbnailer};
use image::{GenericImageView, Rgb, RgbImage, Rgba, RgbaImage};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

fn vfs() -> Arc<Vfs> {
    Vfs::new(Arc::new(LocalProvider), Arc::new(MemoryCredentials::default()))
}

/// 200×100 JPEG, red left half / blue right half, with an EXIF block
/// saying "rotate 90° clockwise to display" (orientation 6).
fn jpeg_with_orientation(orientation: u16) -> Vec<u8> {
    let img = RgbImage::from_fn(200, 100, |x, _| if x < 100 { Rgb([255, 0, 0]) } else { Rgb([0, 0, 255]) });
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95).encode_image(&img).unwrap();

    let mut tiff = b"MM\0\x2A\0\0\0\x08".to_vec();
    tiff.extend_from_slice(&1u16.to_be_bytes()); // one IFD entry
    tiff.extend_from_slice(&0x0112u16.to_be_bytes()); // Orientation
    tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_be_bytes()); // count
    tiff.extend_from_slice(&orientation.to_be_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0u32.to_be_bytes()); // no next IFD
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(tiff);

    let mut out = jpeg[..2].to_vec(); // SOI
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend(app1);
    out.extend_from_slice(&jpeg[2..]);
    out
}

fn decode(bytes: &[u8]) -> image::DynamicImage {
    image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().unwrap().decode().unwrap()
}

#[tokio::test]
async fn photo_thumbnail_is_upright_and_fits() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("photo.jpg");
    std::fs::write(&path, jpeg_with_orientation(6)).unwrap();
    let thumbs = Thumbnailer::new(tmp.path().join("cache"), 10 << 20).unwrap();

    let t = thumbs.thumbnail(&vfs(), &Location::local(&path), 64).await.unwrap();
    assert_eq!(t.mime, "image/jpeg");
    assert_eq!((t.width, t.height), (32, 64), "rotated to portrait");
    let img = decode(&t.bytes);
    assert_eq!(img.dimensions(), (32, 64));
    // Rotating 90° clockwise puts the red left half on top.
    let top = img.get_pixel(16, 4);
    let bottom = img.get_pixel(16, 60);
    assert!(top[0] > 200 && top[2] < 60, "top {top:?}");
    assert!(bottom[2] > 200 && bottom[0] < 60, "bottom {bottom:?}");

    let info = media_info(&vfs(), &Location::local(&path)).await.unwrap();
    assert_eq!((info.width, info.height, info.orientation), (Some(200), Some(100), Some(6)));
    assert_eq!(info.format.as_deref(), Some("jpg"));
}

#[tokio::test]
async fn transparent_images_become_png_and_small_ones_are_not_upscaled() {
    let tmp = tempfile::tempdir().unwrap();
    let big = tmp.path().join("icon.png");
    RgbaImage::from_pixel(300, 150, Rgba([0, 128, 0, 100])).save(&big).unwrap();
    let small = tmp.path().join("tiny.png");
    RgbImage::from_pixel(20, 10, Rgb([1, 2, 3])).save(&small).unwrap();
    let thumbs = Thumbnailer::new(tmp.path().join("cache"), 10 << 20).unwrap();

    let t = thumbs.thumbnail(&vfs(), &Location::local(&big), 100).await.unwrap();
    assert_eq!((t.mime, t.width, t.height), ("image/png", 100, 50));
    let px = decode(&t.bytes).get_pixel(50, 25);
    assert_eq!(px[3], 100, "alpha kept");

    let t = thumbs.thumbnail(&vfs(), &Location::local(&small), 256).await.unwrap();
    assert_eq!((t.width, t.height), (20, 10));

    let json = serde_json::to_value(&t).unwrap();
    assert_eq!(json, serde_json::json!({ "mime": "image/jpeg", "width": 20, "height": 10 }));
}

#[tokio::test]
async fn second_request_is_served_from_the_disk_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.png");
    RgbImage::from_pixel(400, 400, Rgb([9, 9, 9])).save(&path).unwrap();
    let loc = Location::local(&path);
    let cache_dir = tmp.path().join("cache");

    let thumbs = Thumbnailer::new(&cache_dir, 10 << 20).unwrap();
    let first = thumbs.thumbnail(&vfs(), &loc, 128).await.unwrap();
    let second = thumbs.thumbnail(&vfs(), &loc, 128).await.unwrap();
    assert_eq!(first, second);
    let st = thumbs.stats();
    assert_eq!((st.hits, st.misses, st.entries), (1, 1, 1));

    // A different size is a different thumbnail.
    thumbs.thumbnail(&vfs(), &loc, 64).await.unwrap();
    assert_eq!(thumbs.stats().entries, 2);

    // The cache outlives the process.
    let reopened = Thumbnailer::new(&cache_dir, 10 << 20).unwrap();
    assert_eq!(reopened.thumbnail(&vfs(), &loc, 128).await.unwrap(), first);
    assert_eq!(reopened.stats().hits, 1);

    // Changing the file invalidates its thumbnail.
    RgbImage::from_pixel(300, 100, Rgb([9, 9, 9])).save(&path).unwrap();
    let changed = reopened.thumbnail(&vfs(), &loc, 128).await.unwrap();
    assert_eq!((changed.width, changed.height), (128, 43));
    assert_eq!(reopened.stats().misses, 1);
}

#[tokio::test]
async fn folders_and_unknown_files_are_unsupported() {
    let tmp = tempfile::tempdir().unwrap();
    let thumbs = Thumbnailer::new(tmp.path().join("cache"), 1 << 20).unwrap();
    let r = thumbs.thumbnail(&vfs(), &Location::local(tmp.path()), 64).await;
    assert!(matches!(r, Err(CxError::Unsupported(_))), "{r:?}");
    let broken = tmp.path().join("broken.png");
    std::fs::write(&broken, b"not a png at all").unwrap();
    assert!(thumbs.thumbnail(&vfs(), &Location::local(&broken), 64).await.is_err());
}

#[tokio::test]
async fn text_previews() {
    let tmp = tempfile::tempdir().unwrap();
    let v = vfs();
    let f = |name: &str, bytes: &[u8]| {
        let p = tmp.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        Location::local(p)
    };

    let p = preview_text(&v, &f("main.rs", "fn main() { println!(\"héllo\"); }\n".as_bytes()), 4096).await.unwrap();
    assert_eq!((p.encoding, p.truncated, p.language_guess), ("utf-8", false, Some("rust")));
    assert!(p.text.contains("héllo"));

    let mut utf16 = vec![0xFF, 0xFE];
    utf16.extend("Grüße".encode_utf16().flat_map(|u| u.to_le_bytes()));
    let p = preview_text(&v, &f("notes.txt", &utf16), 4096).await.unwrap();
    assert_eq!((p.text.as_str(), p.encoding), ("Grüße", "utf-16le"));

    let long = "line\n".repeat(1000);
    let p = preview_text(&v, &f("big.log", long.as_bytes()), 100).await.unwrap();
    assert!(p.truncated);
    assert_eq!(p.text.len(), 100);

    let r = preview_text(&v, &f("prog.bin", &[0x7f, b'E', b'L', b'F', 0, 0, 1, 2]), 4096).await;
    assert!(matches!(r, Err(CxError::Unsupported(_))), "{r:?}");

    let json = serde_json::to_value(preview_text(&v, &f("a.json", b"{}"), 10).await.unwrap()).unwrap();
    assert_eq!(json, serde_json::json!({ "text": "{}", "truncated": false, "encoding": "utf-8", "languageGuess": "json" }));
}

/// A "remote" server that is really a local folder, to exercise the
/// non-local code paths (reads through the provider, size cap).
struct FakeRemote {
    root: PathBuf,
}

impl FakeRemote {
    fn local(&self, loc: &Location) -> Location {
        Location::local(self.root.join(loc.posix_path().unwrap().trim_start_matches('/')))
    }
}

#[async_trait]
impl Provider for FakeRemote {
    fn scheme(&self) -> &'static str {
        "sftp"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        LocalProvider.list(&self.local(dir), sink).await
    }
    async fn stat(&self, loc: &Location) -> Result<Entry> {
        LocalProvider.stat(&self.local(loc)).await
    }
    async fn create_dir(&self, _: &Location, _: Option<&str>) -> Result<Entry> {
        Err(CxError::Unsupported("read-only".into()))
    }
    async fn move_to(&self, _: &Location, _: &Location) -> Result<()> {
        Err(CxError::Unsupported("read-only".into()))
    }
    async fn remove(&self, _: &Location) -> Result<()> {
        Err(CxError::Unsupported("read-only".into()))
    }
    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        LocalProvider.open_read(&self.local(loc), offset).await
    }
    async fn open_write(&self, _: &Location, _: WriteMode) -> Result<WriteStream> {
        Err(CxError::Unsupported("read-only".into()))
    }
}

struct FakeConnector(PathBuf);

#[async_trait]
impl Connector for FakeConnector {
    fn scheme(&self) -> Scheme {
        Scheme::Sftp
    }
    async fn connect(&self, _: &Endpoint, _: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(FakeRemote { root: self.0.clone() }))
    }
}

fn remote(path: &str) -> Location {
    Location::parse(&format!("sftp://test@server{path}")).unwrap()
}

#[tokio::test]
async fn remote_images_are_read_through_the_vfs() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("photo.jpg"), jpeg_with_orientation(1)).unwrap();
    std::fs::write(tmp.path().join("doc.pdf"), minimal_pdf()).unwrap();
    std::fs::write(tmp.path().join("readme.md"), b"# Hi\n").unwrap();
    let v = vfs();
    v.register(Arc::new(FakeConnector(tmp.path().to_path_buf())));
    let thumbs = Thumbnailer::new(tmp.path().join("cache"), 1 << 20).unwrap();

    let t = thumbs.thumbnail(&v, &remote("/photo.jpg"), 50).await.unwrap();
    assert_eq!((t.width, t.height), (50, 25));
    // No OS thumbnailer for remote files: the UI shows the type icon.
    assert!(matches!(thumbs.thumbnail(&v, &remote("/doc.pdf"), 50).await, Err(CxError::Unsupported(_))));

    let p = preview_text(&v, &remote("/readme.md"), 100).await.unwrap();
    assert_eq!((p.text.as_str(), p.language_guess), ("# Hi\n", Some("markdown")));
    let info = media_info(&v, &remote("/photo.jpg")).await.unwrap();
    assert_eq!((info.width, info.height, info.orientation), (Some(200), Some(100), Some(1)));
}

/// A one-page PDF with a filled rectangle, with a correct xref table.
fn minimal_pdf() -> Vec<u8> {
    let content = "0 0 1 rg 20 20 160 160 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
    out
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> Location {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    Location::local(p)
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn quicklook_thumbnails_pdf() {
    let tmp = tempfile::tempdir().unwrap();
    let loc = write(tmp.path(), "doc.pdf", &minimal_pdf());
    let thumbs = Thumbnailer::new(tmp.path().join("cache"), 10 << 20).unwrap();
    let t = thumbs.thumbnail(&vfs(), &loc, 128).await.unwrap();
    assert_eq!(t.mime, "image/png");
    assert!(t.width > 0 && t.height > 0 && t.width.max(t.height) <= 256, "{}x{}", t.width, t.height);
    let img = decode(&t.bytes);
    assert_eq!(img.dimensions(), (t.width, t.height));
    // The page's blue square shows up in the middle.
    let mid = img.get_pixel(t.width / 2, t.height / 2);
    assert!(mid[2] > 150 && mid[0] < 100, "{mid:?}");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn quicklook_request_can_be_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let loc = write(tmp.path(), "doc.pdf", &minimal_pdf());
    let thumbs = Thumbnailer::new(tmp.path().join("cache"), 10 << 20).unwrap();
    // Dropping the future cancels the QuickLook request; a later one works.
    let _ = tokio::time::timeout(std::time::Duration::from_micros(1), thumbs.thumbnail(&vfs(), &loc, 64)).await;
    assert!(thumbs.thumbnail(&vfs(), &loc, 64).await.is_ok());
}

/// App bundles get their own icon (QuickLook's icon representation).
#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_app_bundles_show_their_icon() {
    let app = Path::new("/System/Applications/Calculator.app");
    if !app.exists() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let thumbs = Thumbnailer::new(tmp.path().join("cache"), 10 << 20).unwrap();
    let t = thumbs.thumbnail(&vfs(), &Location::local(app), 128).await.unwrap();
    assert!(t.width >= 64 && t.height >= 64, "{}x{}", t.width, t.height);
    // Plain folders still have no thumbnail.
    assert!(thumbs.thumbnail(&vfs(), &Location::local(tmp.path()), 128).await.is_err());
}
