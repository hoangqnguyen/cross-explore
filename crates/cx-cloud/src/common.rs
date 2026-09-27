//! Bits every provider needs: getting the path out of a location and turning
//! a response body into a `ReadStream`.

use crate::api::Api;
use crate::util::describe;
use cx_core::{CxError, Location, ReadStream, Result};
use futures_util::TryStreamExt;
use reqwest::{Response, StatusCode};
use tokio::io::AsyncReadExt;
use tokio_util::io::StreamReader;

/// The POSIX path of a remote location. The endpoint's scheme is not
/// checked: the VFS only routes this provider's own locations here, and
/// tests address providers with placeholder schemes.
pub(crate) fn remote_path(loc: &Location) -> Result<&str> {
    match loc {
        Location::Remote { path, .. } => Ok(path),
        _ => Err(CxError::InvalidLocation(loc.uri())),
    }
}

/// `(parent path, name)` of a non-root path.
pub(crate) fn split_parent(path: &str) -> Option<(&str, &str)> {
    let p = path.trim_end_matches('/');
    let (parent, name) = p.rsplit_once('/')?;
    if name.is_empty() {
        return None;
    }
    Some((if parent.is_empty() { "/" } else { parent }, name))
}

/// Stream a (2xx) response body.
pub(crate) fn body_stream(resp: Response, uri: String) -> ReadStream {
    let body = resp.bytes_stream().map_err(move |e| std::io::Error::other(format!("{uri}: {}", describe(&e))));
    Box::pin(StreamReader::new(body))
}

/// Stream a download that was asked for with `Range: bytes=offset-`.
/// 416 means the offset is at (or past) the end: nothing left to read. A
/// server that ignores the range gets the leading bytes skipped here.
pub(crate) async fn ranged_body(api: &Api, resp: Response, offset: u64, uri: String) -> Result<ReadStream> {
    let status = resp.status();
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        return Ok(Box::pin(tokio::io::empty()));
    }
    if !status.is_success() {
        return Err(api.error(resp, &uri).await);
    }
    let mut stream = body_stream(resp, uri.clone());
    if offset > 0 && status != StatusCode::PARTIAL_CONTENT {
        skip(&mut stream, offset, &uri).await?;
    }
    Ok(stream)
}

/// Read and drop the first `n` bytes (for bodies that can't be ranged, such
/// as Google Docs exports). An offset past the end leaves an empty stream,
/// which is what a reader resuming there should see.
pub(crate) async fn skip(stream: &mut ReadStream, n: u64, uri: &str) -> Result<()> {
    tokio::io::copy(&mut stream.take(n), &mut tokio::io::sink()).await.map_err(|e| CxError::io(uri, e))?;
    Ok(())
}
