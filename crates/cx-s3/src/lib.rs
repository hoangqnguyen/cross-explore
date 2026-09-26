//! S3-compatible object storage (AWS S3, MinIO, Cloudflare R2, Backblaze B2,
//! Wasabi…) as a Cross Explore [`Provider`](cx_core::Provider).
//!
//! Locations are `s3://[access-key@]endpoint-host[:port]/bucket/key/path`:
//! the host is the service endpoint, `/` lists the buckets and the first path
//! segment is the bucket (like an SMB share). The access key id is the user
//! and the secret key the password. See the `endpoint` module for how http vs https
//! and the signing region are chosen.
//!
//! ## Why no SDK
//!
//! The candidates were `aws-sdk-s3` (the smithy runtime is a large tree
//! and brings its own HTTP stack), `opendal` (a whole storage abstraction to
//! use one service of) and `rust-s3` (lightest of the three, but it pins
//! reqwest 0.12, so a second hyper/rustls stack next to the reqwest 0.13
//! the workspace already uses, and it has no ranged *streaming* GET). What a
//! file explorer needs from S3 is a dozen REST calls, so this crate talks to
//! the API directly on the workspace's reqwest (rustls) with a ~60 line
//! SigV4 signer checked against AWS's published examples. That keeps full
//! control over what matters here: path-style addressing for any endpoint,
//! page-by-page streaming listings, ranged streaming downloads, and a
//! multipart upload that aborts when the writer is dropped.
//!
//! ## What maps to what
//!
//! - "folders" are key prefixes ending in `/`. A zero-byte `prefix/` object
//!   (the "folder marker" most tools create) makes an empty folder exist; it
//!   is never shown as an entry of its own.
//! - list: `ListBuckets` at `/`; `ListObjectsV2` with delimiter `/` below,
//!   one page per batch (the first page is small), `CommonPrefixes` as
//!   folders and `Contents` as files.
//! - stat: `HEAD` the object; failing that, a folder if any key starts with
//!   `key/`. Buckets are `HEAD`ed.
//! - create_dir: a folder marker (a bucket when creating at `/`).
//! - move_to: server-side copy then delete (every key under a folder;
//!   `CopyObject`, or `UploadPartCopy` above 5 GiB). Never overwrites.
//! - remove: `DeleteObject`, and `DeleteObjects` (1000 keys per call) for
//!   everything under a folder. A bucket is only deleted when empty.
//! - open_read: `GET` with `Range`, streamed. open_write: see the `upload` module.
//!   `Append` is unsupported.
//! - copy_within: server-side copy of a file (`Ok(false)` for folders so the
//!   caller walks them).
//! - set_modified is a no-op (S3 sets `LastModified` itself), there is no
//!   watch (callers poll), no trash and no free-space figure.

mod endpoint;
mod client;
mod provider;
mod sign;
mod upload;
mod util;
mod xml;

pub use provider::{S3Connector, S3Provider};
