//! End-to-end previews of generated documents, through the Vfs.

use async_trait::async_trait;
use cx_core::{
    Capabilities, Connector, Credentials, CxError, Endpoint, Entry, Location, MemoryCredentials, Provider, ReadStream, Result, Scheme, Vfs, WriteMode, WriteStream,
};
use cx_local::LocalProvider;
use cx_office::{OfficePreview, OfficeRenderer};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

// ------------------------------------------------------------- helpers

fn vfs() -> Arc<Vfs> {
    Vfs::new(Arc::new(LocalProvider), Arc::new(MemoryCredentials::default()))
}

/// A 1×1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
    0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn zip_package(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in files {
        let method = if *name == "mimetype" { zip::CompressionMethod::Stored } else { zip::CompressionMethod::Deflated };
        w.start_file(*name, zip::write::SimpleFileOptions::default().compression_method(method)).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

async fn preview_file(name: &str, bytes: &[u8]) -> OfficePreview {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    let r = OfficeRenderer::new(tmp.path().join("cache")).with_libreoffice(None);
    r.preview(&vfs(), &Location::local(&path), false).await.unwrap()
}

async fn html_of(name: &str, bytes: &[u8]) -> (String, Option<String>, Option<u32>) {
    match preview_file(name, bytes).await {
        OfficePreview::Html { html, title, pages } => (html, title, pages),
        other => panic!("expected HTML, got {other:?}"),
    }
}

/// Every tag must be one we generate, with allowed attributes only, and
/// every URL a `data:image/` URI. Text is escaped, so a raw `<` always
/// starts one of our tags.
fn assert_sanitized(html: &str) {
    const TAGS: &[&str] = &[
        "!DOCTYPE", "html", "head", "meta", "title", "style", "body", "article", "main", "section", "nav", "div", "span", "p", "h1", "h2", "h3", "h4", "h5", "h6", "br",
        "hr", "ul", "ol", "li", "table", "thead", "tbody", "tr", "td", "th", "img", "sup", "input", "label", "details", "summary",
    ];
    const ATTRS: &[&str] = &["class", "style", "src", "alt", "colspan", "rowspan", "type", "start", "name", "id", "for", "checked", "charset", "content", "http-equiv", "html"];
    let lower = html.to_ascii_lowercase();
    assert!(!lower.contains("<script"), "script tag");
    assert!(!lower.contains("javascript:"), "javascript URL");
    assert!(!lower.contains("http://") && !lower.contains("https://"), "external URL");
    assert!(html.contains("Content-Security-Policy"));
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        let end = rest[start..].find('>').expect("unclosed tag") + start;
        let tag = &rest[start + 1..end];
        rest = &rest[end + 1..];
        let tag = tag.trim_start_matches('/');
        let mut parts = tag.splitn(2, |c: char| c.is_whitespace());
        let name = parts.next().unwrap();
        assert!(TAGS.contains(&name), "unexpected tag <{name}>");
        let mut attrs = parts.next().unwrap_or("").trim();
        while !attrs.is_empty() {
            let name_end = attrs.find(|c: char| c == '=' || c.is_whitespace()).unwrap_or(attrs.len());
            let aname = &attrs[..name_end];
            assert!(ATTRS.contains(&aname), "unexpected attribute {aname} in <{tag}>");
            attrs = attrs[name_end..].trim_start();
            if let Some(v) = attrs.strip_prefix('=') {
                let v = v.strip_prefix('"').expect("unquoted attribute");
                let close = v.find('"').expect("unterminated attribute");
                let value = &v[..close];
                if aname == "src" {
                    assert!(value.starts_with("data:image/"), "non-data src");
                }
                if aname == "style" {
                    assert!(!value.contains("url(") && !value.contains("expression"), "active CSS in style");
                }
                attrs = v[close + 1..].trim_start();
            }
        }
    }
}

fn pos(html: &str, needle: &str) -> usize {
    html.find(needle).unwrap_or_else(|| panic!("{needle:?} not in HTML"))
}

// ---------------------------------------------------------------- DOCX

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#;

fn docx(body: &str) -> Vec<u8> {
    let document = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W}><w:body>{body}<w:sectPr/></w:body></w:document>"#);
    let styles = format!(
        r#"<?xml version="1.0"?><w:styles {W}>
        <w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults>
        <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
        <w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/></w:style>
        <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:rPr><w:color w:val="2F5496"/></w:rPr></w:style>
        <w:style w:type="paragraph" w:styleId="Custom2"><w:name w:val="My Section"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style>
        </w:styles>"#
    );
    let numbering = format!(
        r#"<?xml version="1.0"?><w:numbering {W}>
        <w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum>
        <w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="lowerLetter"/></w:lvl></w:abstractNum>
        <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
        <w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
        </w:numbering>"#
    );
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
        <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
        <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>
        <Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
        <Relationship Id="rId6" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="https://evil.example/track.png" TargetMode="External"/>
        <Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="javascript:alert(1)" TargetMode="External"/>
        </Relationships>"#;
    let root_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
        <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
        </Relationships>"#;
    let core = r#"<?xml version="1.0"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Quarterly Report</dc:title></cp:coreProperties>"#;
    let app = r#"<?xml version="1.0"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Pages>3</Pages></Properties>"#;
    zip_package(&[
        ("[Content_Types].xml", br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#),
        ("_rels/.rels", root_rels.as_bytes()),
        ("docProps/core.xml", core.as_bytes()),
        ("docProps/app.xml", app.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/styles.xml", styles.as_bytes()),
        ("word/numbering.xml", numbering.as_bytes()),
        ("word/_rels/document.xml.rels", rels.as_bytes()),
        ("word/media/image1.png", PNG),
    ])
}

fn p(style: Option<&str>, runs: &str) -> String {
    let ppr = style.map(|s| format!(r#"<w:pPr><w:pStyle w:val="{s}"/></w:pPr>"#)).unwrap_or_default();
    format!("<w:p>{ppr}{runs}</w:p>")
}

fn r(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn li(num: u32, lvl: u32, text: &str) -> String {
    format!(r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{lvl}"/><w:numId w:val="{num}"/></w:numPr></w:pPr>{}</w:p>"#, r(text))
}

fn image_run(rid: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline><wp:extent cx="952500" cy="952500"/><wp:docPr id="1" name="Picture 1" descr="A dot"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="{rid}"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
    )
}

fn sample_docx() -> String {
    let mut body = String::new();
    body += &p(Some("Title"), &r("Annual Summary"));
    body += &p(Some("Heading1"), &r("Introduction"));
    body += &p(Some("Custom2"), &r("Outline Section"));
    body += &p(
        None,
        &[
            r#"<w:r><w:rPr><w:b/></w:rPr><w:t>Bold text</w:t></w:r>"#,
            r#"<w:r><w:rPr><w:i/></w:rPr><w:t>Italic text</w:t></w:r>"#,
            r#"<w:r><w:rPr><w:u w:val="single"/></w:rPr><w:t>Under</w:t></w:r>"#,
            r#"<w:r><w:rPr><w:strike/></w:rPr><w:t>Struck</w:t></w:r>"#,
            r#"<w:r><w:rPr><w:color w:val="FF0000"/><w:highlight w:val="yellow"/></w:rPr><w:t>Red on yellow</w:t></w:r>"#,
        ]
        .concat(),
    );
    body += &li(1, 0, "Bullet one");
    body += &li(1, 0, "Bullet two");
    body += &li(2, 0, "Step one");
    body += &li(2, 1, "Sub step");
    body += &li(2, 0, "Step two");
    body += r#"<w:tbl><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>Wide header</w:t></w:r></w:p></w:tc></w:tr>
        <w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/><w:shd w:fill="DDEEFF"/></w:tcPr><w:p><w:r><w:t>Tall cell</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B2</w:t></w:r></w:p></w:tc></w:tr>
        <w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc><w:tc><w:p><w:r><w:t>B3</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    body += &p(None, &format!("{}{}", r("Picture: "), image_run("rId5")));
    body += &p(None, &format!("{}{}", r("Page one ends"), r#"<w:r><w:br w:type="page"/></w:r>"#));
    body += &p(None, &r("Page two"));
    body += &p(None, r#"<w:r><w:instrText>PAGE</w:instrText></w:r>"#);
    // Complex field: instruction hidden, result shown.
    body += &p(
        None,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> HYPERLINK "javascript:alert(2)" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>field result</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
    );
    body
}

#[tokio::test]
async fn docx_structure() {
    let (html, title, pages) = html_of("report.docx", &docx(&sample_docx())).await;
    assert_eq!(title.as_deref(), Some("Quarterly Report"));
    assert_eq!(pages, Some(3));
    assert!(html.contains(r#"<h1 class="title">Annual Summary</h1>"#), "{html}");
    assert!(html.contains(r#"<h1><span style="color:#2f5496;">Introduction</span></h1>"#), "{html}");
    assert!(html.contains("<h2>Outline Section</h2>"));
    assert!(html.contains(r#"<span class="b ">Bold text</span>"#));
    assert!(html.contains(r#"<span class="i ">Italic text</span>"#));
    assert!(html.contains(r#"<span class="u ">Under</span>"#));
    assert!(html.contains(r#"<span class="s ">Struck</span>"#));
    assert!(html.contains(r#"<span style="color:#ff0000;background-color:#ffff00;">Red on yellow</span>"#));
    // Lists: bullets, then a numbered list with a nested lettered level.
    assert!(html.contains("<ul><li><p>Bullet one</p></li><li><p>Bullet two</p></li></ul>"), "{html}");
    assert!(html.contains(r#"<ol type="1"><li><p>Step one</p><ol type="a"><li><p>Sub step</p></li></ol></li><li><p>Step two</p></li></ol>"#), "{html}");
    // Table with merged cells.
    assert!(html.contains(r#"<td colspan="2"><p>Wide header</p></td>"#));
    assert!(html.contains(r#"<td rowspan="2" style="background-color:#ddeeff;"><p>Tall cell</p></td>"#), "{html}");
    assert!(pos(&html, "B2") < pos(&html, "B3"));
    // Image inlined as a data URI, sized from its extent (952500 EMU = 100 px).
    assert!(html.contains(r#"<img class="img" src="data:image/png;base64,"#));
    assert!(html.contains(r#"style="width:100px;" alt="A dot">"#));
    assert!(pos(&html, "Page one ends") < pos(&html, r#"<hr class="page-break">"#));
    assert!(pos(&html, r#"<hr class="page-break">"#) < pos(&html, "Page two"));
    assert!(html.contains("field result"));
    assert!(!html.contains("HYPERLINK") && !html.contains("PAGE<"));
    assert!(html.contains("@media (prefers-color-scheme: dark)"));
    assert_sanitized(&html);
}

#[tokio::test]
async fn docx_hostile_content_is_inert() {
    let mut body = String::new();
    body += &p(None, &r("&lt;script&gt;alert('x')&lt;/script&gt;"));
    body += &p(None, &r("&lt;img src=x onerror=alert(1)&gt;"));
    body += r#"<w:p><w:hyperlink r:id="rId9"><w:r><w:t>click me</w:t></w:r></w:hyperlink></w:p>"#;
    body += r#"<w:p><w:r><w:rPr><w:color w:val="FF0000&quot; onmouseover=&quot;alert(1)"/></w:rPr><w:t>color injection</w:t></w:r></w:p>"#;
    body += r#"<w:p><w:pPr><w:pStyle w:val="x&quot; onclick=&quot;alert(1)"/></w:pPr><w:r><w:t>style injection</w:t></w:r></w:p>"#;
    body += &p(None, &image_run("rId6")); // external image: never fetched
    body += r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr id="2" name="x" descr="&quot; onerror=&quot;alert(1)"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId5"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
    let (html, _, _) = html_of("evil.docx", &docx(&body)).await;
    assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"));
    assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
    assert!(html.contains(r#"<span class="link">click me</span>"#));
    assert!(html.contains("<p>color injection</p>"), "{html}");
    assert!(html.contains("style injection"));
    assert!(html.contains(r#"alt="&quot; onerror=&quot;alert(1)""#));
    assert!(!html.contains("evil.example"));
    assert_sanitized(&html);
}

#[tokio::test]
async fn docx_image_budget() {
    // 12 × 1 MB "PNGs": only the first 10 MB are inlined.
    let mut big = PNG.to_vec();
    big.resize(1 << 20, 0);
    let mut rels = String::new();
    let mut body = String::new();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..12 {
        rels += &format!(r#"<Relationship Id="img{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/{i}.png"/>"#);
        body += &p(None, &image_run(&format!("img{i}")));
        files.push((format!("word/media/{i}.png"), big.clone()));
    }
    let document = format!(r#"<w:document {W}><w:body>{body}</w:body></w:document>"#);
    let rels = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#);
    let mut all: Vec<(&str, &[u8])> = vec![("word/document.xml", document.as_bytes()), ("word/_rels/document.xml.rels", rels.as_bytes())];
    all.extend(files.iter().map(|(n, d)| (n.as_str(), d.as_slice())));
    let (html, _, _) = html_of("photos.docx", &zip_package(&all)).await;
    assert_eq!(html.matches("data:image/png").count(), 10);
    assert!(html.contains("Some images were left out"));
    assert!(html.len() < 16 << 20);
}

// ---------------------------------------------------------------- XLSX

fn sample_xlsx() -> Vec<u8> {
    use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Data").unwrap();
    ws.write_string(0, 0, "Name").unwrap();
    ws.write_string(0, 1, "Amount").unwrap();
    ws.write_string(0, 2, "When").unwrap();
    ws.write_string(1, 0, "Widget").unwrap();
    ws.write_number(1, 1, 1234.5).unwrap();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    ws.write_datetime_with_format(1, 2, ExcelDateTime::from_ymd(2024, 3, 15).unwrap(), &date).unwrap();
    ws.write_string(2, 0, "<script>alert(1)</script>").unwrap();
    ws.write_boolean(2, 1, true).unwrap();
    ws.merge_range(4, 0, 4, 2, "Merged total", &Format::new()).unwrap();
    let ws2 = wb.add_worksheet();
    ws2.set_name("Summary").unwrap();
    ws2.write_string(0, 0, "Second sheet").unwrap();
    wb.save_to_buffer().unwrap()
}

#[tokio::test]
async fn xlsx_sheets_tabs_and_cells() {
    let (html, _, pages) = html_of("book.xlsx", &sample_xlsx()).await;
    assert_eq!(pages, Some(2));
    // CSS-only tabs.
    assert!(html.contains(r#"<input type="radio" name="sheet" class="tabin" id="t0" checked>"#));
    assert!(html.contains(r#"<label for="t0" id="l0">Data</label><label for="t1" id="l1">Summary</label>"#));
    assert!(html.contains("#t1:checked~.panels #p1{display:block}"));
    // Column letters and row numbers.
    assert!(html.contains(r#"<th class="corner"></th><th>A</th><th>B</th><th>C</th>"#));
    assert!(html.contains("<tr><th>1</th><td>Name</td>"));
    assert!(html.contains(r#"<td class="n">1234.5</td>"#));
    assert!(html.contains(r#"<td class="n">2024-03-15</td>"#), "{html}");
    assert!(html.contains(r#"<td class="bool">TRUE</td>"#));
    assert!(html.contains(r#"<td colspan="3">Merged total</td>"#));
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(pos(&html, "Name") < pos(&html, "Second sheet"));
    assert_sanitized(&html);
}

#[tokio::test]
async fn xlsx_is_capped() {
    use rust_xlsxwriter::Workbook;
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    for row in 0..1200u32 {
        ws.write_number(row, 0, row as f64).unwrap();
    }
    ws.write_number(0, 119, 1.0).unwrap();
    let (html, _, _) = html_of("big.xlsx", &wb.save_to_buffer().unwrap()).await;
    assert!(html.contains("<tr><th>1000</th>"));
    assert!(!html.contains("<tr><th>1001</th>"));
    assert!(html.contains("<th>CV</th>") && !html.contains("<th>CW</th>"), "100 columns: A..CV");
    assert!(html.contains("truncated"));
    // A single sheet has no tab strip.
    assert!(!html.contains("tabin\" id"));
}

// ---------------------------------------------------------------- PPTX

const P: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

fn sp(ph: Option<&str>, xfrm: Option<(i64, i64, i64, i64)>, paras: &str) -> String {
    let ph = ph.map(|t| format!(r#"<p:ph type="{t}"/>"#)).unwrap_or_default();
    let xfrm = xfrm.map(|(x, y, w, h)| format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#)).unwrap_or_default();
    format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="s"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr>{xfrm}</p:spPr><p:txBody><a:bodyPr/>{paras}</p:txBody></p:sp>"#)
}

fn ap(text: &str) -> String {
    format!("<a:p><a:r><a:rPr lang=\"en-US\"/><a:t>{text}</a:t></a:r></a:p>")
}

fn sample_pptx() -> Vec<u8> {
    let rels = |body: &str| format!(r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{body}</Relationships>"#);
    let rel = |id: &str, kind: &str, target: &str| format!(r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#);
    let pres = format!(
        r#"<?xml version="1.0"?><p:presentation {P}><p:sldIdLst><p:sldId id="256" r:id="rId3"/><p:sldId id="257" r:id="rId2"/></p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/></p:presentation>"#
    );
    let layout = format!(
        r#"<p:sldLayout {P}><p:cSld><p:spTree>{}</p:spTree></p:cSld></p:sldLayout>"#,
        sp(Some("title"), Some((838200, 365125, 10515600, 1325563)), "")
    );
    let master = format!(
        r#"<p:sldMaster {P}><p:cSld><p:bg><p:bgPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></p:bgPr></p:bg><p:spTree/></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2"/><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz="4400"/></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:buChar char="•"/><a:defRPr sz="2800"/></a:lvl1pPr></p:bodyStyle></p:txStyles></p:sldMaster>"#
    );
    let theme = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:clrScheme name="x"><a:dk1><a:srgbClr val="112233"/></a:dk1><a:lt1><a:srgbClr val="FAFAFA"/></a:lt1></a:clrScheme></a:themeElements></a:theme>"#;
    // Slide 1: title from the layout's geometry, bulleted body, picture, notes.
    let slide1 = format!(
        r#"<p:sld {P}><p:cSld><p:spTree>{}{}<p:pic><p:nvPicPr><p:cNvPr id="4" name="pic"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/></p:blipFill><p:spPr><a:xfrm><a:off x="6096000" y="3429000"/><a:ext cx="3048000" cy="1714500"/></a:xfrm></p:spPr></p:pic></p:spTree></p:cSld></p:sld>"#,
        sp(Some("title"), None, &ap("First slide title")),
        sp(Some("body"), Some((838200, 1825625, 10515600, 4351338)), &format!("{}{}", ap("Point A"), ap("Point B")))
    );
    // Slide 2: text box without placeholder, a table, a script-like string.
    let slide2 = format!(
        r#"<p:sld {P}><p:cSld><p:spTree>{}{}<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="t"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="5000000"/><a:ext cx="6000000" cy="1000000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tr h="370840"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Cell 1</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Cell 2</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#,
        sp(Some("title"), None, &ap("Second slide title")),
        sp(None, Some((6096000, 0, 6096000, 685800)), r#"<a:p><a:r><a:rPr sz="1200" b="1"><a:solidFill><a:srgbClr val="C00000"/></a:solidFill></a:rPr><a:t>&lt;script&gt;boom&lt;/script&gt;</a:t></a:r></a:p>"#)
    );
    let notes = format!(r#"<p:notes {P}><p:cSld><p:spTree>{}</p:spTree></p:cSld></p:notes>"#, sp(Some("body"), None, &ap("Speaker notes here")));
    let files: Vec<(&str, String)> = vec![
        ("_rels/.rels", rels(&rel("rId1", "officeDocument", "ppt/presentation.xml"))),
        ("ppt/presentation.xml", pres),
        ("ppt/_rels/presentation.xml.rels", rels(&format!("{}{}", rel("rId2", "slide", "slides/slide2.xml"), rel("rId3", "slide", "slides/slide1.xml")))),
        ("ppt/slides/slide1.xml", slide1),
        (
            "ppt/slides/_rels/slide1.xml.rels",
            rels(&format!("{}{}{}", rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"), rel("rId2", "image", "../media/image1.png"), rel("rId3", "notesSlide", "../notesSlides/notesSlide1.xml"))),
        ),
        ("ppt/slides/slide2.xml", slide2),
        ("ppt/slides/_rels/slide2.xml.rels", rels(&rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"))),
        ("ppt/slideLayouts/slideLayout1.xml", layout),
        ("ppt/slideLayouts/_rels/slideLayout1.xml.rels", rels(&rel("rId1", "slideMaster", "../slideMasters/slideMaster1.xml"))),
        ("ppt/slideMasters/slideMaster1.xml", master),
        ("ppt/slideMasters/_rels/slideMaster1.xml.rels", rels(&rel("rId1", "theme", "../theme/theme1.xml"))),
        ("ppt/theme/theme1.xml", theme.to_string()),
        ("ppt/notesSlides/notesSlide1.xml", notes),
    ];
    let mut all: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (*n, d.as_bytes())).collect();
    all.push(("ppt/media/image1.png", PNG));
    zip_package(&all)
}

#[tokio::test]
async fn pptx_slides_in_order() {
    let (html, _, pages) = html_of("deck.pptx", &sample_pptx()).await;
    assert_eq!(pages, Some(2));
    // Order follows sldIdLst, not part names.
    let s1 = pos(&html, "First slide title");
    assert!(s1 < pos(&html, "Point A"));
    assert!(pos(&html, "Point A") < pos(&html, "Point B"));
    assert!(pos(&html, "Point B") < pos(&html, "Second slide title"));
    assert!(pos(&html, "Second slide title") < pos(&html, "Cell 1"));
    assert_eq!(html.matches("<div class=\"slide\"").count(), 2);
    assert!(html.contains("aspect-ratio:12192000/6858000;"));
    // Theme background (bg1 → lt1) and text color (tx1 → dk1).
    assert!(html.contains("background:#fafafa;color:#112233;"), "{html}");
    // Title inherits the layout's geometry: 838200 / 12192000 = 6.875 %.
    assert!(html.contains("left:6.875%;top:5.324%;"), "{html}");
    // Master bullets and sizes: 28 pt body → 28*12700/12192000*100 cqw.
    assert!(html.contains(r#"<span class="bu">•</span>Point A"#), "{html}");
    assert!(html.contains("font-size:2.917cqw"));
    assert!(html.contains(r#"<img class="pic" src="data:image/png;base64,"#));
    assert!(html.contains("<summary>Notes</summary><p>Speaker notes here</p>"));
    assert!(html.contains(r#"<span class="b " style="font-size:1.25cqw;color:#c00000;">&lt;script&gt;boom&lt;/script&gt;</span>"#), "{html}");
    assert!(html.contains("<td><p"));
    assert_sanitized(&html);
}

// ------------------------------------------------------------ OpenDocument

const ODF_NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0""#;

fn odf(mime: &str, content: &str) -> Vec<u8> {
    let meta = format!(r#"<office:document-meta {ODF_NS}><office:meta><dc:title>ODF Title</dc:title><meta:document-statistic meta:page-count="2"/></office:meta></office:document-meta>"#);
    zip_package(&[("mimetype", mime.as_bytes()), ("content.xml", content.as_bytes()), ("meta.xml", meta.as_bytes()), ("Pictures/dot.png", PNG)])
}

#[tokio::test]
async fn odt_text() {
    let content = format!(
        r##"<office:document-content {ODF_NS}><office:automatic-styles>
        <style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold" fo:color="#336699"/></style:style>
        <text:list-style style:name="L1"><text:list-level-style-number text:level="1" style:num-format="1"/></text:list-style>
        </office:automatic-styles><office:body><office:text>
        <text:h text:outline-level="1">ODT Heading</text:h>
        <text:p>Hello <text:span text:style-name="T1">bold blue</text:span> world<text:s text:c="2"/>end <text:a xlink:href="javascript:alert(1)">a link</text:a></text:p>
        <text:list text:style-name="L1"><text:list-item><text:p>First item</text:p></text:list-item><text:list-item><text:p>Second item</text:p></text:list-item></text:list>
        <text:list><text:list-item><text:p>Bullet</text:p></text:list-item></text:list>
        <table:table><table:table-row><table:table-cell table:number-columns-spanned="2"><text:p>Span</text:p></table:table-cell><table:covered-table-cell/></table:table-row>
        <table:table-row><table:table-cell><text:p>C1</text:p></table:table-cell><table:table-cell><text:p>C2</text:p></table:table-cell></table:table-row></table:table>
        <text:p><draw:frame svg:width="1in"><draw:image xlink:href="Pictures/dot.png"/></draw:frame><draw:frame><draw:image xlink:href="http://example.com/x.png"/></draw:frame></text:p>
        </office:text></office:body></office:document-content>"##
    );
    let (html, title, pages) = html_of("notes.odt", &odf("application/vnd.oasis.opendocument.text", &content)).await;
    assert_eq!(title.as_deref(), Some("ODF Title"));
    assert_eq!(pages, Some(2));
    assert!(html.contains("<h1>ODT Heading</h1>"));
    assert!(html.contains(r#"<p>Hello <span class="b " style="color:#336699;">bold blue</span> world  end <span class="link">a link</span></p>"#), "{html}");
    assert!(html.contains(r#"<ol type="1"><li><p>First item</p></li><li><p>Second item</p></li></ol>"#));
    assert!(html.contains("<ul><li><p>Bullet</p></li></ul>"));
    assert!(html.contains(r#"<td colspan="2"><p>Span</p></td>"#));
    assert!(pos(&html, "C1") < pos(&html, "C2"));
    assert!(html.contains(r#"<img class="img" src="data:image/png;base64,"#));
    assert!(html.contains("width:96px;"));
    assert_sanitized(&html);
}

#[tokio::test]
async fn odp_slides() {
    let frame = |class: &str, y: &str, text: &str| {
        format!(r#"<draw:frame presentation:class="{class}" svg:x="1cm" svg:y="{y}" svg:width="20cm" svg:height="3cm"><draw:text-box><text:p>{text}</text:p></draw:text-box></draw:frame>"#)
    };
    let content = format!(
        r#"<office:document-content {ODF_NS}><office:body><office:presentation>
        <draw:page draw:name="p1">{}{}</draw:page>
        <draw:page draw:name="p2">{}<presentation:notes><draw:frame presentation:class="notes"><draw:text-box><text:p>Note two</text:p></draw:text-box></draw:frame></presentation:notes></draw:page>
        </office:presentation></office:body></office:document-content>"#,
        frame("title", "1cm", "Impress first"),
        frame("outline", "5cm", "Impress body"),
        frame("title", "1cm", "Impress second")
    );
    let (html, _, pages) = html_of("talk.odp", &odf("application/vnd.oasis.opendocument.presentation", &content)).await;
    assert_eq!(pages, Some(2));
    assert!(pos(&html, "Impress first") < pos(&html, "Impress body"));
    assert!(pos(&html, "Impress body") < pos(&html, "Impress second"));
    assert!(html.contains("<summary>Notes</summary><p>Note two</p>"));
    assert_eq!(html.matches("<div class=\"slide\"").count(), 2);
    // 1 cm of a 28 cm page.
    assert!(html.contains("left:3.571%;"), "{html}");
    assert_sanitized(&html);
}

// ------------------------------------------------------------ CSV, RTF

#[tokio::test]
async fn csv_table() {
    let (html, _, _) = html_of("people.csv", b"name,age\nAlice,30\n\"Bob, Jr.\",<b>41</b>\n").await;
    assert!(html.contains("<tr><th>1</th><td>name</td><td>age</td></tr>"));
    assert!(html.contains(r#"<td>Alice</td><td class="n">30</td>"#));
    assert!(html.contains("<td>Bob, Jr.</td><td>&lt;b&gt;41&lt;/b&gt;</td>"));
    assert_sanitized(&html);
    let (tsv, _, _) = html_of("x.tsv", b"a\tb,c\n").await;
    assert!(tsv.contains("<td>a</td><td>b,c</td>"));
}

#[tokio::test]
async fn rtf_text() {
    let rtf = br#"{\rtf1\ansi\ansicpg1252{\fonttbl{\f0 Arial;}}{\colortbl;\red255\green0\blue0;}{\*\generator Hidden;}\f0 Hello {\b bold} {\cf1 red} caf\'e9 \u8364? {\field{\*\fldinst HYPERLINK "javascript:x"}{\fldrslt linked}}\par
\qc Centered\par
\pard\intbl A1\cell B1\cell\row
\pard After table\page Next page\par}"#;
    let (html, _, _) = html_of("letter.rtf", rtf).await;
    assert!(html.contains(r#"<span class="b ">bold</span>"#), "{html}");
    assert!(html.contains(r#"<span style="color:#ff0000;">red</span>"#), "{html}");
    assert!(html.contains("café €"));
    assert!(html.contains(r#"<span class="link ">linked</span>"#));
    assert!(html.contains(r#"<p style="text-align:center;">Centered</p>"#));
    assert!(html.contains("<table><tbody><tr><td><p>A1</p></td><td><p>B1</p></td></tr></tbody></table>"), "{html}");
    assert!(pos(&html, "After table") < pos(&html, r#"<hr class="page-break"><p>Next page"#));
    let body = &html[pos(&html, "<body")..];
    assert!(!body.contains("Arial") && !body.contains("Hidden"));
    assert_sanitized(&html);
}

// ---------------------------------------------------------- legacy files

fn compound(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    for (name, data) in streams {
        let mut s = cf.create_stream(name).unwrap();
        s.write_all(data).unwrap();
    }
    cf.flush().unwrap();
    cf.into_inner().into_inner()
}

fn legacy_doc(text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut word = vec![0u8; 0x800];
    word[0..2].copy_from_slice(&0xA5ECu16.to_le_bytes());
    word[0x0A..0x0C].copy_from_slice(&0x0200u16.to_le_bytes()); // 1Table
    word[0x4C..0x50].copy_from_slice(&(units.len() as u32).to_le_bytes());
    for u in &units {
        word.extend_from_slice(&u.to_le_bytes());
    }
    // Clx: one piece, uncompressed UTF-16 at 0x800.
    let mut plc = Vec::new();
    plc.extend_from_slice(&0u32.to_le_bytes());
    plc.extend_from_slice(&(units.len() as u32).to_le_bytes());
    plc.extend_from_slice(&[0, 0]);
    plc.extend_from_slice(&0x800u32.to_le_bytes());
    plc.extend_from_slice(&[0, 0]);
    let mut clx = vec![0x02];
    clx.extend_from_slice(&(plc.len() as u32).to_le_bytes());
    clx.extend_from_slice(&plc);
    let mut table = vec![0u8; 16];
    let fc = table.len() as u32;
    table.extend_from_slice(&clx);
    word[0x1A2..0x1A6].copy_from_slice(&fc.to_le_bytes());
    word[0x1A6..0x1AA].copy_from_slice(&(clx.len() as u32).to_le_bytes());
    compound(&[("/WordDocument", word), ("/1Table", table)])
}

fn record(ver_inst: u16, rtype: u16, body: &[u8]) -> Vec<u8> {
    let mut r = Vec::new();
    r.extend_from_slice(&ver_inst.to_le_bytes());
    r.extend_from_slice(&rtype.to_le_bytes());
    r.extend_from_slice(&(body.len() as u32).to_le_bytes());
    r.extend_from_slice(body);
    r
}

fn legacy_ppt() -> Vec<u8> {
    let chars = |s: &str| s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>();
    let mut list = Vec::new();
    for (title, body) in [("Old title one", "Body one"), ("Old title two", "Body two")] {
        list.extend(record(0, 0x03F3, &[0; 20]));
        list.extend(record(0, 0x0F9F, &0u32.to_le_bytes()));
        list.extend(record(0, 0x0FA0, &chars(title)));
        list.extend(record(0, 0x0F9F, &1u32.to_le_bytes()));
        list.extend(record(0, 0x0FA8, body.as_bytes()));
    }
    let doc = record(0x000F, 0x03E8, &record(0x000F, 0x0FF0, &list));
    compound(&[("/PowerPoint Document", doc)])
}

#[tokio::test]
async fn legacy_doc_text_without_libreoffice() {
    let (html, _, _) = html_of("old.doc", &legacy_doc("Legacy heading\rSecond paragraph \u{13} HYPERLINK \"x\" \u{14}link text\u{15}\r<script>")).await;
    assert!(html.contains("Text-only preview"));
    assert!(html.contains("<p>Legacy heading</p><p>Second paragraph link text</p>"), "{html}");
    assert!(!html.contains("HYPERLINK"));
    assert!(html.contains("&lt;script&gt;"));
    assert_sanitized(&html);
}

#[tokio::test]
async fn legacy_ppt_text_without_libreoffice() {
    let (html, _, pages) = html_of("old.ppt", &legacy_ppt()).await;
    assert_eq!(pages, Some(2));
    assert!(pos(&html, "Old title one") < pos(&html, "Body one"));
    assert!(pos(&html, "Body one") < pos(&html, "Old title two"));
    assert!(pos(&html, "Old title two") < pos(&html, "Body two"));
    assert_sanitized(&html);
}

// ------------------------------------------------------ errors and caps

#[tokio::test]
async fn unsupported_inputs() {
    let tmp = tempfile::tempdir().unwrap();
    let r = OfficeRenderer::new(tmp.path().join("cache")).with_libreoffice(None);
    let v = vfs();
    let write = |name: &str, bytes: &[u8]| {
        let p = tmp.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        Location::local(p)
    };
    for loc in [write("notes.txt", b"hi"), write("broken.docx", b"not a zip"), write("broken.xlsx", b"nope"), write("broken.doc", b"nope")] {
        let e = r.preview(&v, &loc, false).await.unwrap_err();
        assert!(matches!(e, CxError::Unsupported(_)), "{loc}: {e:?}");
    }
    assert!(matches!(r.preview(&v, &Location::local(tmp.path()), false).await, Err(CxError::Unsupported(_))));
    // Too large for the built-in renderers (sparse file, nothing is read).
    let big = tmp.path().join("huge.csv");
    std::fs::File::create(&big).unwrap().set_len(cx_office::MAX_BUILTIN_BYTES + 1).unwrap();
    assert!(matches!(r.preview(&v, &Location::local(&big), false).await, Err(CxError::Unsupported(_))));
    // Without LibreOffice, prefer_pdf still yields HTML.
    let doc = write("fine.docx", &docx(&p(None, "<w:r><w:t>ok</w:t></w:r>")));
    assert!(matches!(r.preview(&v, &doc, true).await.unwrap(), OfficePreview::Html { .. }));
}


#[tokio::test]
async fn preview_serializes_for_the_ui() {
    let json = serde_json::to_value(OfficePreview::Html { html: "<p>x</p>".into(), title: None, pages: Some(2) }).unwrap();
    assert_eq!(json, serde_json::json!({ "kind": "html", "html": "<p>x</p>", "title": null, "pages": 2 }));
    let json = serde_json::to_value(OfficePreview::Pdf { path: "/c/x.pdf".into() }).unwrap();
    assert_eq!(json, serde_json::json!({ "kind": "pdf", "path": "/c/x.pdf" }));
}

// ------------------------------------------------------------- remote

/// A "remote" server that is really a local folder, to exercise reads
/// through a non-local provider.
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

#[tokio::test]
async fn remote_files_are_read_through_the_vfs() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("book.xlsx"), sample_xlsx()).unwrap();
    let v = vfs();
    v.register(Arc::new(FakeConnector(tmp.path().to_path_buf())));
    let r = OfficeRenderer::new(tmp.path().join("cache")).with_libreoffice(None);
    let loc = Location::parse("sftp://test@server/book.xlsx").unwrap();
    match r.preview(&v, &loc, false).await.unwrap() {
        OfficePreview::Html { html, .. } => assert!(html.contains("Widget")),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------- LibreOffice

fn find_soffice() -> Option<PathBuf> {
    let r = OfficeRenderer::new(std::env::temp_dir());
    r.libreoffice().map(Path::to_path_buf)
}

/// Runs only where LibreOffice is installed.
#[tokio::test]
async fn libreoffice_pdf_when_installed() {
    let Some(soffice) = find_soffice() else {
        eprintln!("LibreOffice not installed; skipping PDF conversion test");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("report.docx");
    std::fs::write(&path, docx(&sample_docx())).unwrap();
    let r = OfficeRenderer::new(tmp.path().join("cache")).with_libreoffice(Some(soffice));
    let first = r.preview(&vfs(), &Location::local(&path), true).await.unwrap();
    let OfficePreview::Pdf { path: pdf } = &first else { panic!("expected a PDF, got {first:?}") };
    assert!(std::fs::read(pdf).unwrap().starts_with(b"%PDF"));
    // Second call hits the cache.
    assert_eq!(r.preview(&vfs(), &Location::local(&path), true).await.unwrap(), first);
    // Not preferring PDF still gives HTML for built-in formats.
    assert!(matches!(r.preview(&vfs(), &Location::local(&path), false).await.unwrap(), OfficePreview::Html { .. }));
}
