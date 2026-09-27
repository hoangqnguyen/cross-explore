//! Office document previews for Cross Explore (Quick Look and the preview
//! pane).
//!
//! [`OfficeRenderer::preview`] turns Word, Excel, PowerPoint, OpenDocument,
//! RTF and CSV files into an [`OfficePreview`]:
//!
//! * **HTML** from built-in pure-Rust renderers. These work on every
//!   platform (iOS and Android included) and for files on any provider,
//!   since the bytes are read through the [`Vfs`]. The HTML is
//!   self-contained and sanitized by construction (see the `html` module): no
//!   scripts, event handlers or external URLs can come out of a document.
//! * **PDF** from LibreOffice, when it is installed (desktop only) and the
//!   caller prefers fidelity. Conversions are cached on disk by URI, size
//!   and modification time.
//!
//! Legacy binary .doc/.ppt files get a PDF when LibreOffice is available and
//! a text-only preview otherwise; .xls is read natively.

mod docx;
mod html;
mod legacy;
mod libreoffice;
mod odf;
mod package;
mod pptx;
mod rtf;
mod sheet;
mod xml;

use cx_core::{CxError, Entry, Location, Result, Vfs};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// Built-in renderers read the whole file into memory; larger files are
/// refused (the UI shows the type icon) unless LibreOffice renders them.
pub const MAX_BUILTIN_BYTES: u64 = 50 << 20;

/// Largest file handed to LibreOffice (remote ones are downloaded first).
pub const MAX_CONVERT_BYTES: u64 = 200 << 20;

/// A LibreOffice conversion taking longer than this is killed.
pub const CONVERT_TIMEOUT: Duration = Duration::from_secs(60);

/// Cached PDFs kept (least recently produced ones are removed first).
const MAX_CACHED_PDFS: usize = 64;

/// A rendered preview.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OfficePreview {
    /// A complete, self-contained HTML page: inline CSS (light and dark),
    /// images as `data:` URIs, no scripts, no event handlers, no external
    /// URLs (hyperlinks are shown as text), plus a Content-Security-Policy
    /// forbidding all of those. Safe to load into a web view as `srcdoc`.
    #[serde(rename_all = "camelCase")]
    Html {
        html: String,
        /// The document's own title (from its metadata), if it has one.
        title: Option<String>,
        /// Pages (Word/Writer, from the saved statistics), slides, or sheets.
        pages: Option<u32>,
    },
    /// A PDF rendered by LibreOffice, cached on disk. Highest fidelity.
    #[serde(rename_all = "camelCase")]
    Pdf { path: PathBuf },
}

/// Output of a built-in renderer.
pub(crate) struct Rendered {
    pub html: String,
    pub title: Option<String>,
    pub pages: Option<u32>,
}

/// File formats, by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Docx,
    Workbook,
    Pptx,
    Odt,
    Odp,
    Rtf,
    Csv,
    Tsv,
    Doc,
    Ppt,
}

impl Format {
    fn from_name(name: &str) -> Option<Format> {
        let ext = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
            _ => return None,
        };
        Some(match ext.as_str() {
            "docx" | "docm" | "dotx" | "dotm" => Format::Docx,
            "xlsx" | "xlsm" | "xltx" | "xltm" | "xlsb" | "xls" | "xla" | "xlam" | "ods" | "ots" => Format::Workbook,
            "pptx" | "pptm" | "potx" | "potm" | "ppsx" | "ppsm" => Format::Pptx,
            "odt" | "ott" => Format::Odt,
            "odp" | "otp" => Format::Odp,
            "rtf" => Format::Rtf,
            "csv" => Format::Csv,
            "tsv" | "tab" => Format::Tsv,
            "doc" | "dot" => Format::Doc,
            "ppt" | "pps" | "pot" => Format::Ppt,
            _ => return None,
        })
    }

    /// Worth converting with LibreOffice (plain tables gain nothing).
    fn convertible(self) -> bool {
        !matches!(self, Format::Csv | Format::Tsv)
    }

    /// The built-in renderer is only a text extraction: prefer LibreOffice
    /// whenever it's there.
    fn text_only(self) -> bool {
        matches!(self, Format::Doc | Format::Ppt)
    }

    fn render(self, bytes: Vec<u8>, name: &str) -> Result<Rendered> {
        match self {
            Format::Docx => docx::render(bytes, name),
            Format::Workbook => sheet::render_workbook(bytes, name),
            Format::Pptx => pptx::render(bytes, name),
            Format::Odt => odf::render_text(bytes, name),
            Format::Odp => odf::render_presentation(bytes, name),
            Format::Rtf => rtf::render(&bytes, name),
            Format::Csv => sheet::render_csv(&bytes, name, false),
            Format::Tsv => sheet::render_csv(&bytes, name, true),
            Format::Doc => legacy::render_doc(bytes, name),
            Format::Ppt => legacy::render_ppt(bytes, name),
        }
    }
}

/// Renders office documents; one per app, shared by all previews.
pub struct OfficeRenderer {
    cache_dir: PathBuf,
    soffice: OnceLock<Option<PathBuf>>,
    /// One LibreOffice conversion at a time (they share a profile).
    convert_lock: tokio::sync::Mutex<()>,
}

impl OfficeRenderer {
    /// `cache_dir` holds converted PDFs and LibreOffice's private profile;
    /// it is created on first use. LibreOffice is looked for once, lazily.
    pub fn new(cache_dir: impl Into<PathBuf>) -> OfficeRenderer {
        OfficeRenderer { cache_dir: cache_dir.into(), soffice: OnceLock::new(), convert_lock: tokio::sync::Mutex::new(()) }
    }

    /// Use this LibreOffice executable (`None`: never use LibreOffice)
    /// instead of searching for one.
    pub fn with_libreoffice(self, soffice: Option<PathBuf>) -> OfficeRenderer {
        let _ = self.soffice.set(soffice);
        self
    }

    /// The LibreOffice executable in use, if any (always `None` on phones).
    pub fn libreoffice(&self) -> Option<&Path> {
        self.soffice.get_or_init(libreoffice::detect).as_deref()
    }

    /// Whether `name` (by extension) is an office document this crate can
    /// preview. Legacy .doc/.ppt are included: without LibreOffice they get
    /// a text-only preview.
    pub fn supports(name: &str) -> bool {
        Format::from_name(name).is_some()
    }

    /// Preview `loc`. With `prefer_pdf` and LibreOffice available, the file
    /// is converted to PDF (falling back to HTML if that fails); otherwise
    /// the built-in renderer produces HTML. Legacy .doc/.ppt always try
    /// LibreOffice first.
    ///
    /// `Unsupported` means "no preview for this file" (unknown type, too
    /// large, damaged, encrypted); the UI shows the type icon instead.
    pub async fn preview(&self, vfs: &Vfs, loc: &Location, prefer_pdf: bool) -> Result<OfficePreview> {
        let provider = vfs.provider(loc).await?;
        let entry = provider.stat(loc).await?;
        if entry.is_dir {
            return Err(CxError::Unsupported(format!("{} is a folder", entry.name)));
        }
        let format = Format::from_name(&entry.name).ok_or_else(|| CxError::Unsupported(format!("no office preview for {}", entry.name)))?;

        if (prefer_pdf || format.text_only()) && format.convertible() {
            if let Some(soffice) = self.libreoffice().map(Path::to_path_buf) {
                // A failed conversion still leaves the built-in preview.
                if let Ok(path) = self.pdf(vfs, loc, &entry, &soffice).await {
                    return Ok(OfficePreview::Pdf { path });
                }
            }
        }

        if entry.size > MAX_BUILTIN_BYTES {
            return Err(CxError::Unsupported(format!("{} is too large to preview", entry.name)));
        }
        let bytes = read_all(vfs, loc, MAX_BUILTIN_BYTES).await?;
        let name = entry.name.clone();
        let r = tokio::task::spawn_blocking(move || format.render(bytes, &name)).await.map_err(|e| CxError::Io(format!("preview worker failed: {e}")))??;
        Ok(OfficePreview::Html { html: r.html, title: r.title, pages: r.pages })
    }

    /// Convert with LibreOffice, through the PDF cache.
    async fn pdf(&self, vfs: &Vfs, loc: &Location, entry: &Entry, soffice: &Path) -> Result<PathBuf> {
        if entry.size > MAX_CONVERT_BYTES {
            return Err(CxError::Unsupported(format!("{} is too large to convert", entry.name)));
        }
        let pdf_dir = self.cache_dir.join("pdf");
        let key = cache_key(&format!("{}\n{}\n{}", loc.uri(), entry.size, entry.modified.unwrap_or(0)));
        let cached = pdf_dir.join(format!("{key}.pdf"));
        if is_nonempty_file(&cached) {
            return Ok(cached);
        }

        let _guard = self.convert_lock.lock().await;
        // Someone may have converted it while we waited.
        if is_nonempty_file(&cached) {
            return Ok(cached);
        }
        let work_root = self.cache_dir.join("work");
        tokio::fs::create_dir_all(&work_root).await.map_err(|e| CxError::from_io(e, work_root.display()))?;
        tokio::fs::create_dir_all(&pdf_dir).await.map_err(|e| CxError::from_io(e, pdf_dir.display()))?;
        let work = tempfile::Builder::new().prefix("convert-").tempdir_in(&work_root).map_err(|e| CxError::from_io(e, work_root.display()))?;

        // LibreOffice picks the importer from the extension, so keep it.
        let input = match loc.local_path() {
            Some(p) => p.to_path_buf(),
            None => {
                let ext: String = entry.name.rsplit_once('.').map(|(_, e)| e).unwrap_or("bin").chars().filter(char::is_ascii_alphanumeric).take(8).collect();
                let dest = work.path().join(format!("document.{ext}"));
                download(vfs, loc, &dest, MAX_CONVERT_BYTES).await?;
                dest
            }
        };
        let out_dir = work.path().join("out");
        tokio::fs::create_dir_all(&out_dir).await.map_err(|e| CxError::from_io(e, out_dir.display()))?;
        let profile = self.cache_dir.join("lo-profile");
        let soffice = soffice.to_path_buf();
        let produced = tokio::task::spawn_blocking(move || libreoffice::convert(&soffice, &input, &profile, &out_dir, CONVERT_TIMEOUT))
            .await
            .map_err(|e| CxError::Io(format!("conversion worker failed: {e}")))??;
        if tokio::fs::rename(&produced, &cached).await.is_err() {
            tokio::fs::copy(&produced, &cached).await.map_err(|e| CxError::from_io(e, cached.display()))?;
        }
        drop(work);
        let dir = pdf_dir.clone();
        let _ = tokio::task::spawn_blocking(move || prune(&dir, MAX_CACHED_PDFS)).await;
        Ok(cached)
    }
}

fn is_nonempty_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// Read a whole file through the Vfs, refusing more than `max` bytes.
async fn read_all(vfs: &Vfs, loc: &Location, max: u64) -> Result<Vec<u8>> {
    let provider = vfs.provider(loc).await?;
    let stream = provider.open_read(loc, 0).await?;
    let mut buf = Vec::new();
    stream.take(max + 1).read_to_end(&mut buf).await.map_err(|e| CxError::from_io(e, loc))?;
    if buf.len() as u64 > max {
        return Err(CxError::Unsupported(format!("{} is too large to preview", loc.name())));
    }
    Ok(buf)
}

/// Copy a (remote) file to `dest`, refusing more than `max` bytes.
async fn download(vfs: &Vfs, loc: &Location, dest: &Path, max: u64) -> Result<()> {
    let provider = vfs.provider(loc).await?;
    let stream = provider.open_read(loc, 0).await?;
    let mut file = tokio::fs::File::create(dest).await.map_err(|e| CxError::from_io(e, dest.display()))?;
    let n = tokio::io::copy(&mut stream.take(max + 1), &mut file).await.map_err(|e| CxError::from_io(e, loc))?;
    if n > max {
        return Err(CxError::Unsupported(format!("{} is too large to convert", loc.name())));
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await.map_err(|e| CxError::from_io(e, dest.display()))?;
    Ok(())
}

/// Stable 128-bit hex digest (two FNV-1a passes) for cache file names.
fn cache_key(s: &str) -> String {
    let fnv = |seed: u64| s.bytes().fold(seed, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
    format!("{:016x}{:016x}", fnv(0xcbf2_9ce4_8422_2325), fnv(0x6c62_272e_07bb_0142))
}

/// Keep the `keep` most recently modified files in `dir`.
fn prune(dir: &Path, keep: usize) -> Result<()> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| CxError::from_io(e, dir.display()))?
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= keep {
        return Ok(());
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_extension() {
        for n in ["a.docx", "B.DOCM", "c.xlsx", "d.xls", "e.ods", "f.pptx", "g.odt", "h.odp", "i.rtf", "j.csv", "k.tsv", "l.doc", "m.ppt"] {
            assert!(OfficeRenderer::supports(n), "{n}");
        }
        for n in ["a.pdf", "docx", ".docx", "x.pages", "y.txt"] {
            assert!(!OfficeRenderer::supports(n), "{n}");
        }
    }

    #[test]
    fn cache_keys_are_stable() {
        assert_eq!(cache_key("a"), cache_key("a"));
        assert_ne!(cache_key("a"), cache_key("b"));
        assert_eq!(cache_key("x").len(), 32);
    }
}
