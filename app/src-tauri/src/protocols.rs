//! Custom URI schemes that let the web view load bytes straight from any
//! provider: `cxfile://localhost/<uri>` (with HTTP Range support, so video
//! and audio can seek) and `cxthumb://localhost/<size>/<version>/<uri>`.

use crate::state::App;
use cx_core::{CxError, Location};
use std::sync::Arc;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, UriSchemeContext, UriSchemeResponder, Wry};
use tokio::io::AsyncReadExt;

/// Largest body served without a Range request (bigger files are streamed
/// by the web view in ranges anyway).
const MAX_FULL: u64 = 64 << 20;
/// Largest single range response.
const MAX_RANGE: u64 = 8 << 20;

fn decode(path: &str) -> String {
    percent_encoding::percent_decode_str(path.trim_start_matches('/')).decode_utf8_lossy().into_owned()
}

fn error(status: StatusCode, msg: impl Into<String>) -> Response<Vec<u8>> {
    Response::builder().status(status).header(header::CONTENT_TYPE, "text/plain").header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*").body(msg.into().into_bytes()).unwrap()
}

fn status_for(e: &CxError) -> StatusCode {
    match e {
        CxError::NotFound(_) => StatusCode::NOT_FOUND,
        CxError::PermissionDenied(_) | CxError::AuthRequired { .. } => StatusCode::FORBIDDEN,
        CxError::Unsupported(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn parse_range(h: &str, size: u64) -> Option<(u64, u64)> {
    let spec = h.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        return Some((size.saturating_sub(n), size.saturating_sub(1)));
    }
    let start: u64 = a.parse().ok()?;
    let end = if b.is_empty() { size.saturating_sub(1) } else { b.parse::<u64>().ok()?.min(size.saturating_sub(1)) };
    (start <= end).then_some((start, end))
}

async fn serve_file(app: Arc<App>, req: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let uri = decode(req.uri().path());
    let Ok(loc) = Location::parse(&uri) else { return error(StatusCode::BAD_REQUEST, "bad location") };
    let result = async {
        let provider = app.vfs.provider(&loc).await?;
        let entry = provider.stat(&loc).await?;
        let size = entry.size;
        let mime = mime_guess::from_path(&entry.name).first_or_octet_stream().to_string();
        let range = req.headers().get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|h| parse_range(h, size));
        let (start, end, partial) = match range {
            Some((s, e)) => (s, e.min(s + MAX_RANGE - 1), true),
            None if size <= MAX_FULL => (0, size.saturating_sub(1), false),
            None => (0, MAX_RANGE - 1, true),
        };
        let len = if size == 0 { 0 } else { end - start + 1 };
        let mut buf = Vec::with_capacity(len as usize);
        if len > 0 {
            let reader = provider.open_read(&loc, start).await?;
            reader.take(len).read_to_end(&mut buf).await.map_err(|e| CxError::io("read failed", e))?;
        }
        let mut res = Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::CACHE_CONTROL, "no-cache");
        if partial {
            res = res.status(StatusCode::PARTIAL_CONTENT).header(header::CONTENT_RANGE, format!("bytes {start}-{}/{size}", start + buf.len().max(1) as u64 - 1));
        }
        Ok::<_, CxError>(res.body(buf).unwrap())
    }
    .await;
    result.unwrap_or_else(|e| error(status_for(&e), e.to_string()))
}

async fn serve_thumb(app: Arc<App>, req: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = req.uri().path().trim_start_matches('/');
    let mut parts = path.splitn(3, '/');
    let (Some(size), Some(_version), Some(rest)) = (parts.next(), parts.next(), parts.next()) else { return error(StatusCode::BAD_REQUEST, "bad thumbnail path") };
    let size: u32 = size.parse().unwrap_or(256);
    let Ok(loc) = Location::parse(&decode(rest)) else { return error(StatusCode::BAD_REQUEST, "bad location") };
    match app.thumbs.thumbnail(&app.vfs, &loc, size).await {
        Ok(t) => Response::builder()
            .header(header::CONTENT_TYPE, t.mime)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            // The version (mtime) is part of the URL, so this can be cached forever.
            .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
            .body(t.bytes)
            .unwrap(),
        Err(e) => error(status_for(&e), e.to_string()),
    }
}

pub fn file_protocol(ctx: UriSchemeContext<'_, Wry>, req: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let app = ctx.app_handle().state::<Arc<App>>().inner().clone();
    tauri::async_runtime::spawn(async move { responder.respond(serve_file(app, req).await) });
}

pub fn thumb_protocol(ctx: UriSchemeContext<'_, Wry>, req: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let app = ctx.app_handle().state::<Arc<App>>().inner().clone();
    tauri::async_runtime::spawn(async move { responder.respond(serve_thumb(app, req).await) });
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=0-5000", 1000), Some((0, 999)));
        assert_eq!(parse_range("bytes=500-100", 1000), None);
    }
}
