//! The preview pane and Quick Look: text and code (highlighted), Markdown
//! as text, folder and archive listings, image metadata, and file info for
//! everything else. Loaded in the background; only the newest request for
//! the cursor row is kept.

use crate::format;
use crate::highlight::{highlight, Line};
use cx_core::{Entry, Location};
use cx_engine::Engine;
use std::sync::Arc;

/// Bytes of text read for a preview.
pub const MAX_TEXT: usize = 256 * 1024;
/// Rows shown for folder and archive previews.
pub const MAX_LISTING: usize = 500;

#[derive(Debug, Clone)]
pub enum Content {
    Loading,
    Text {
        lines: Vec<Line>,
        language: Option<String>,
        truncated: bool,
        encoding: String,
    },
    Listing {
        entries: Vec<Entry>,
        total: usize,
    },
    Image {
        info: Vec<(String, String)>,
    },
    None,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct Preview {
    pub uri: String,
    pub entry: Entry,
    pub content: Content,
    pub scroll: usize,
}

fn binary_ext(name: &str) -> bool {
    matches!(
        format::category(&Entry {
            name: name.into(),
            kind: cx_core::EntryKind::File,
            is_dir: false,
            size: 0,
            modified: None,
            created: None,
            hidden: false,
            readonly: false,
            executable: false
        }),
        format::Category::Image
            | format::Category::Audio
            | format::Category::Video
            | format::Category::Executable
    ) || matches!(
        format::ext(name).to_ascii_lowercase().as_str(),
        "pdf"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "pages"
            | "numbers"
            | "key"
            | "epub"
            | "odt"
            | "ods"
            | "odp"
            | "so"
            | "dylib"
            | "dll"
            | "o"
            | "a"
            | "class"
            | "pyc"
            | "wasm"
            | "ttf"
            | "otf"
            | "woff"
            | "woff2"
            | "sqlite"
            | "db"
            | "iso"
            | "img"
            | "bin"
    )
}

/// Load a preview for `entry` at `uri`.
pub async fn load(engine: Arc<Engine>, uri: String, entry: Entry) -> Content {
    let is_archive = !entry.is_dir && cx_archive::is_archive(&entry.name);
    if entry.is_dir || is_archive {
        let target = if is_archive {
            format!("archive://{uri}!/")
        } else {
            uri.clone()
        };
        let mut entries = Vec::new();
        let result = engine
            .list_dir(&target, |ev| {
                if let cx_engine::ListEvent::Batch { entries: b } = ev {
                    entries.extend(b);
                }
                true
            })
            .await;
        return match result {
            Ok(total) => {
                let mut items: Vec<crate::folder::Item> =
                    entries.into_iter().map(crate::folder::Item::new).collect();
                items.sort_by(|a, b| crate::sort::compare(Default::default(), a, b));
                Content::Listing {
                    entries: items
                        .into_iter()
                        .take(MAX_LISTING)
                        .map(|i| i.entry)
                        .collect(),
                    total,
                }
            }
            Err(e) => Content::Error(e.to_string()),
        };
    }
    if format::category(&entry) == format::Category::Image {
        return match engine.media_info(&uri).await {
            Ok(m) => {
                let mut info = Vec::new();
                if let Some(f) = m.format {
                    info.push(("Format".into(), f));
                }
                if let (Some(w), Some(h)) = (m.width, m.height) {
                    info.push(("Dimensions".into(), format!("{w} × {h}")));
                }
                for (k, v) in [
                    ("Taken", m.date_taken),
                    (
                        "Camera",
                        m.camera_make.map(|mk| {
                            format!("{mk} {}", m.camera_model.clone().unwrap_or_default())
                                .trim()
                                .to_string()
                        }),
                    ),
                    ("Lens", m.lens_model),
                ] {
                    if let Some(v) = v {
                        info.push((k.into(), v));
                    }
                }
                if let Some(o) = m.orientation.filter(|o| *o > 1) {
                    info.push(("Orientation".into(), format!("EXIF {o}")));
                }
                Content::Image { info }
            }
            Err(e) => Content::Error(e.to_string()),
        };
    }
    if binary_ext(&entry.name) {
        return Content::None;
    }
    match engine.preview_text(&uri, MAX_TEXT).await {
        Ok(t) => {
            let language = t
                .language
                .clone()
                .or_else(|| cx_thumbs::language_for_name(&entry.name).map(str::to_owned));
            let lines = highlight(&t.text, language.as_deref());
            Content::Text {
                lines,
                language,
                truncated: t.truncated,
                encoding: t.encoding,
            }
        }
        Err(cx_core::CxError::Unsupported(_)) => Content::None,
        Err(e) => Content::Error(e.to_string()),
    }
}

/// Key facts about an entry, for the info block.
pub fn facts(
    uri: &str,
    e: &Entry,
    tags: &[String],
    dir_size: Option<u64>,
) -> Vec<(String, String)> {
    let mut out = vec![("Kind".to_string(), format::type_label(e))];
    if e.is_dir {
        if let Some(s) = dir_size {
            out.push((
                "Size".into(),
                format!("{} ({} bytes)", format::size(s), format::count(s as usize)),
            ));
        }
    } else {
        out.push((
            "Size".into(),
            format!(
                "{} ({} bytes)",
                format::size(e.size),
                format::count(e.size as usize)
            ),
        ));
    }
    out.push(("Modified".into(), format::date_long(e.modified)));
    if e.created.is_some() {
        out.push(("Created".into(), format::date_long(e.created)));
    }
    let where_ = Location::parse(uri)
        .ok()
        .and_then(|l| l.parent())
        .map(|p| crate::util::display(&p.uri()))
        .unwrap_or_default();
    out.push(("Where".into(), where_));
    if !tags.is_empty() {
        out.push(("Tags".into(), tags.join(", ")));
    }
    if e.readonly {
        out.push(("Access".into(), "Read-only".into()));
    }
    if e.hidden {
        out.push(("Hidden".into(), "Yes".into()));
    }
    out
}
