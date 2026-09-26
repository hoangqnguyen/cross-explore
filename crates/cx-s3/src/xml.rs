//! The XML S3 answers with, parsed into a tiny tree.
//!
//! Unlike a WebDAV PROPFIND, an S3 answer is small and bounded (a listing
//! page is at most 1000 keys), so it is read whole and walked as a tree;
//! streaming happens one page at a time instead. Namespaces are ignored:
//! services differ in whether they send one, and element names are unique
//! enough without.

use crate::util::{decode_url, parse_iso8601};
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Default)]
pub(crate) struct Node {
    pub name: String,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Trimmed text of the first child called `name`.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }
}

pub(crate) fn parse(body: &str) -> Result<Node, String> {
    let mut reader = Reader::from_str(body);
    let mut stack: Vec<Node> = vec![Node::default()];
    loop {
        match reader.read_event().map_err(|e| format!("bad XML from server: {e}"))? {
            Event::Start(e) => stack.push(Node { name: local(e.local_name().as_ref()), ..Node::default() }),
            Event::Empty(e) => {
                let n = Node { name: local(e.local_name().as_ref()), ..Node::default() };
                stack.last_mut().expect("root").children.push(n);
            }
            Event::End(_) => {
                let n = stack.pop().expect("balanced");
                let Some(parent) = stack.last_mut() else { return Err("bad XML from server: unbalanced".into()) };
                parent.children.push(n);
            }
            Event::Text(t) => stack.last_mut().expect("root").text.push_str(&t),
            Event::CData(t) => stack.last_mut().expect("root").text.push_str(&t),
            Event::GeneralRef(r) => {
                let text = &mut stack.last_mut().expect("root").text;
                match r.resolve_char_ref() {
                    Ok(Some(c)) => text.push(c),
                    _ => text.push_str(quick_xml::escape::resolve_predefined_entity(&r).unwrap_or("")),
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let mut root = stack.pop().filter(|_| stack.is_empty()).ok_or("bad XML from server: truncated")?;
    root.children.pop().ok_or_else(|| "empty answer from server".to_string())
}

fn local(name: &str) -> String {
    name.to_string()
}

/// An `<Error>` document.
#[derive(Debug, Default, Clone)]
pub(crate) struct S3Error {
    pub code: String,
    pub message: String,
    /// Where AWS says the bucket really is (`AuthorizationHeaderMalformed`,
    /// `PermanentRedirect`).
    pub region: Option<String>,
}

pub(crate) fn error(root: &Node) -> Option<S3Error> {
    (root.name == "Error").then(|| S3Error {
        code: root.text("Code").unwrap_or_default().to_string(),
        message: root.text("Message").unwrap_or_default().to_string(),
        region: root.text("Region").map(str::to_owned),
    })
}

pub(crate) struct Bucket {
    pub name: String,
    pub created: Option<i64>,
}

pub(crate) fn buckets(root: &Node) -> Vec<Bucket> {
    let Some(list) = root.child("Buckets") else { return Vec::new() };
    list.all("Bucket")
        .filter_map(|b| Some(Bucket { name: b.text("Name")?.to_string(), created: b.text("CreationDate").and_then(parse_iso8601) }))
        .collect()
}

pub(crate) struct Object {
    pub key: String,
    pub size: u64,
    pub modified: Option<i64>,
}

/// One page of `ListObjectsV2`.
#[derive(Default)]
pub(crate) struct Page {
    pub objects: Vec<Object>,
    pub prefixes: Vec<String>,
    /// Set while more pages follow.
    pub next: Option<String>,
}

pub(crate) fn page(root: &Node) -> Page {
    let url = root.text("EncodingType").is_some_and(|e| e.eq_ignore_ascii_case("url"));
    let dec = |s: &str| if url { decode_url(s) } else { s.to_string() };
    let truncated = root.text("IsTruncated").is_some_and(|t| t.eq_ignore_ascii_case("true"));
    Page {
        objects: root
            .all("Contents")
            .filter_map(|c| {
                Some(Object {
                    key: dec(c.child("Key").map(|k| k.text.as_str())?),
                    size: c.text("Size").and_then(|s| s.parse().ok()).unwrap_or(0),
                    modified: c.text("LastModified").and_then(parse_iso8601),
                })
            })
            .collect(),
        prefixes: root.all("CommonPrefixes").filter_map(|p| p.child("Prefix").map(|k| dec(&k.text))).collect(),
        next: root.text("NextContinuationToken").filter(|t| truncated && !t.is_empty()).map(str::to_owned),
    }
}

/// Keys that `DeleteObjects` could not delete, with the reason.
pub(crate) fn delete_errors(root: &Node) -> Vec<(String, S3Error)> {
    root.all("Error")
        .map(|e| {
            let err = S3Error { code: e.text("Code").unwrap_or_default().into(), message: e.text("Message").unwrap_or_default().into(), region: None };
            (e.text("Key").unwrap_or_default().to_string(), err)
        })
        .collect()
}

/// Escape text for an XML body we send.
pub(crate) fn escape(s: &str) -> String {
    quick_xml::escape::escape(s).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_page() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>b</Name><Prefix>a%2F</Prefix>
<KeyCount>3</KeyCount><MaxKeys>1000</MaxKeys><Delimiter>%2F</Delimiter><IsTruncated>true</IsTruncated>
<NextContinuationToken>tok==</NextContinuationToken><EncodingType>url</EncodingType>
<Contents><Key>a%2F</Key><LastModified>2024-01-02T03:04:05.678Z</LastModified><ETag>&quot;d41d&quot;</ETag><Size>0</Size></Contents>
<Contents><Key>a%2Fx+y%2Bz.txt</Key><LastModified>2024-01-02T03:04:05.000Z</LastModified><Size>12</Size></Contents>
<CommonPrefixes><Prefix>a%2Fsub%2F</Prefix></CommonPrefixes></ListBucketResult>"#;
        let p = page(&parse(body).unwrap());
        assert_eq!(p.objects.len(), 2);
        assert_eq!(p.objects[0].key, "a/");
        assert_eq!(p.objects[1].key, "a/x y+z.txt");
        assert_eq!(p.objects[1].size, 12);
        assert_eq!(p.objects[0].modified, Some(1_704_164_645_678));
        assert_eq!(p.prefixes, vec!["a/sub/".to_string()]);
        assert_eq!(p.next.as_deref(), Some("tok=="));
    }

    #[test]
    fn errors_and_buckets() {
        let e = error(&parse("<Error><Code>NoSuchKey</Code><Message>gone &amp; lost</Message></Error>").unwrap()).unwrap();
        assert_eq!(e.code, "NoSuchKey");
        assert_eq!(e.message, "gone & lost");
        let b = buckets(
            &parse("<ListAllMyBucketsResult><Owner><ID>x</ID></Owner><Buckets><Bucket><Name>one</Name><CreationDate>2024-01-01T00:00:00Z</CreationDate></Bucket><Bucket><Name>two</Name></Bucket></Buckets></ListAllMyBucketsResult>").unwrap(),
        );
        assert_eq!(b.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["one", "two"]);
    }
}
