//! WebDAV servers as a Cross Explore [`Provider`](cx_core::Provider).
//!
//! `dav://host/path` talks http, `davs://host/path` https (`http://` and
//! `https://` URIs parse to the same schemes). Paths are the server's URL
//! paths, so `davs://cloud.example/remote.php/dav/files/me/` works as is.
//!
//! Built directly on `reqwest` (rustls) rather than `reqwest_dav`: we need a
//! streaming PROPFIND parser (rows appear while a big folder is still
//! arriving), streamed uploads and downloads, control over redirects (reqwest
//! turns a redirected PROPFIND into a GET), and Digest auth, none of which
//! `reqwest_dav` offers in a form we could use.
//!
//! What maps to what:
//! - list/stat: `PROPFIND` (Depth 1 / 0), parsed incrementally ([`propfind`]).
//! - create_dir: `MKCOL`; move_to/copy_within: `MOVE`/`COPY` with
//!   `Overwrite: F` (412 → `AlreadyExists`); remove: `DELETE` (recursive).
//! - open_read: `GET` with `Range` for offsets; open_write: `PUT` with a
//!   chunked body fed from the `AsyncWrite` ([`io`]). `Append` is
//!   unsupported (PUT replaces the whole file).
//! - set_modified: best-effort `PROPPATCH` of `getlastmodified`.
//! - free_space: RFC 4331 quota properties, when the server reports them.
//! - watch: none (WebDAV has no change notification): callers poll.

mod auth;
mod client;
mod io;
mod propfind;
mod provider;

pub use provider::{DavConnector, DavProvider};
