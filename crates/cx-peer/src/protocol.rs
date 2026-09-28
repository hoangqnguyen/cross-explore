//! Wire protocol.
//!
//! Every request opens its own bidirectional QUIC stream, so a slow listing,
//! a watch that stays open for hours and a multi-gigabyte transfer never wait
//! on each other. A stream starts with one [`Request`] frame from the
//! client; the server answers with one or more [`Response`] frames. Some
//! requests switch the stream to raw bytes after the first response:
//!
//! | request      | server → client                              | client → server        |
//! |--------------|----------------------------------------------|------------------------|
//! | `List`       | `Entries`… then `ListEnd`                    |                        |
//! | `Watch`      | `Ok`, then `Changes`… until the client stops |                        |
//! | `ReadRange`  | `Ready{size}`, then raw bytes to the end     |                        |
//! | `Write`      | `Ready`, … final `Ok`/`Err` after the bytes  | raw bytes, then FIN    |
//! | `Offer`      | `Pending`, then `Accepted`/`Declined`, … `Ok`| raw file bytes         |
//! | `PairStart`  | `PairChallenge`, … `Ok`                      | `PairFinish`           |
//!
//! Frames are a little-endian `u32` length followed by a postcard body.
//! The version in [`Hello`] lets future versions negotiate; ALPN
//! (`cx-peer/1`) already keeps incompatible majors apart.

use cx_core::{Change, CxError, Entry, Result, TrashedItem, WriteMode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// 2: entries carry `executable`.
pub const PROTOCOL_VERSION: u32 = 2;

/// Largest control frame accepted. Bulk data never goes through frames.
pub const MAX_FRAME: usize = 8 << 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hello {
    pub version: u32,
    pub device_id: String,
    pub name: String,
    pub os: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WireWriteMode {
    CreateNew,
    Truncate,
    Append,
}

impl From<WriteMode> for WireWriteMode {
    fn from(m: WriteMode) -> Self {
        match m {
            WriteMode::CreateNew => WireWriteMode::CreateNew,
            WriteMode::Truncate => WireWriteMode::Truncate,
            WriteMode::Append => WireWriteMode::Append,
        }
    }
}

impl From<WireWriteMode> for WriteMode {
    fn from(m: WireWriteMode) -> Self {
        match m {
            WireWriteMode::CreateNew => WriteMode::CreateNew,
            WireWriteMode::Truncate => WriteMode::Truncate,
            WireWriteMode::Append => WriteMode::Append,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OfferFile {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Request {
    Hello(Hello),
    /// SPAKE2 first message, bound to both TLS keys.
    PairStart { spake: Vec<u8>, name: String, os: String },
    PairFinish { confirm: [u8; 32] },
    /// Only honoured on connections authenticated with this device's own
    /// key (the `cx pair-code` command talking to a running `cx serve`).
    NewPairingCode,
    ListShares,
    List { path: String },
    Stat { path: String },
    CreateDir { dir: String, name: Option<String> },
    Move { src: String, dst: String },
    Remove { path: String },
    Trash { dir: String, names: Vec<String> },
    ReadRange { path: String, offset: u64 },
    Write { path: String, mode: WireWriteMode },
    SetModified { path: String, ms: i64 },
    CopyWithin { src: String, dst: String },
    Watch { path: String },
    FreeSpace { path: String },
    Hash { path: String },
    Offer { offer_id: String, files: Vec<OfferFile>, total: u64 },
}

impl Request {
    /// Short operation name for the audit log.
    pub fn op(&self) -> &'static str {
        match self {
            Request::Hello(_) => "hello",
            Request::PairStart { .. } | Request::PairFinish { .. } => "pair",
            Request::NewPairingCode => "newPairingCode",
            Request::ListShares => "listShares",
            Request::List { .. } => "list",
            Request::Stat { .. } => "stat",
            Request::CreateDir { .. } => "createDir",
            Request::Move { .. } => "move",
            Request::Remove { .. } => "remove",
            Request::Trash { .. } => "trash",
            Request::ReadRange { .. } => "read",
            Request::Write { .. } => "write",
            Request::SetModified { .. } => "setModified",
            Request::CopyWithin { .. } => "copy",
            Request::Watch { .. } => "watch",
            Request::FreeSpace { .. } => "freeSpace",
            Request::Hash { .. } => "hash",
            Request::Offer { .. } => "offer",
        }
    }

    /// The main path the request touches, for the audit log.
    pub fn path(&self) -> Option<String> {
        match self {
            Request::List { path }
            | Request::Stat { path }
            | Request::Remove { path }
            | Request::ReadRange { path, .. }
            | Request::Write { path, .. }
            | Request::SetModified { path, .. }
            | Request::Watch { path }
            | Request::FreeSpace { path }
            | Request::Hash { path } => Some(path.clone()),
            Request::CreateDir { dir, name } => Some(match name {
                Some(n) => cx_core::location::join_posix(dir, n),
                None => dir.clone(),
            }),
            Request::Move { src, dst } | Request::CopyWithin { src, dst } => Some(format!("{src} -> {dst}")),
            Request::Trash { dir, names } => Some(format!("{dir} [{}]", names.join(", "))),
            Request::Offer { files, .. } => Some(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShareInfo {
    pub name: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Response {
    Ok,
    Err(WireError),
    Hello { hello: Hello, trusted: bool },
    PairChallenge { spake: Vec<u8>, confirm: [u8; 32] },
    PairingCode { code: String, expires_at: i64 },
    Shares(Vec<ShareInfo>),
    Entries(Vec<Entry>),
    ListEnd { total: u64 },
    Entry(Entry),
    Trashed(Vec<TrashedItem>),
    Space(Option<(u64, u64)>),
    Hash(String),
    Copied(bool),
    Changes(Vec<WireChange>),
    /// Raw bytes follow (reads: `size` is the byte count still to come).
    Ready { size: Option<u64> },
    Pending,
    Accepted,
    Declined,
}

impl Response {
    pub fn err(e: CxError) -> Response {
        Response::Err(e.into())
    }
}

/// [`cx_core::Change`] is only `Serialize` (it goes to the UI); this is its
/// round-trippable twin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WireChange {
    Upsert(Entry),
    Remove(String),
    Reset,
}

impl From<Change> for WireChange {
    fn from(c: Change) -> Self {
        match c {
            Change::Upsert { entry } => WireChange::Upsert(entry),
            Change::Remove { name } => WireChange::Remove(name),
            Change::Reset => WireChange::Reset,
        }
    }
}

impl From<WireChange> for Change {
    fn from(c: WireChange) -> Self {
        match c {
            WireChange::Upsert(entry) => Change::Upsert { entry },
            WireChange::Remove(name) => Change::Remove { name },
            WireChange::Reset => Change::Reset,
        }
    }
}

/// [`CxError`] on the wire, variant for variant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WireError {
    NotFound(String),
    PermissionDenied(String),
    AlreadyExists(String),
    InvalidLocation(String),
    InvalidName(String),
    Unsupported(String),
    AuthRequired { uri: String, user: Option<String>, reason: String },
    HostKeyUnknown { uri: String, host: String, key_type: String, fingerprint: String, changed: bool },
    Connection(String),
    Cancelled,
    Io(String),
}

impl From<CxError> for WireError {
    fn from(e: CxError) -> Self {
        match e {
            CxError::NotFound(s) => WireError::NotFound(s),
            CxError::PermissionDenied(s) => WireError::PermissionDenied(s),
            CxError::AlreadyExists(s) => WireError::AlreadyExists(s),
            CxError::InvalidLocation(s) => WireError::InvalidLocation(s),
            CxError::InvalidName(s) => WireError::InvalidName(s),
            CxError::Unsupported(s) => WireError::Unsupported(s),
            CxError::AuthRequired { uri, user, reason } => WireError::AuthRequired { uri, user, reason },
            CxError::HostKeyUnknown { uri, host, key_type, fingerprint, changed } => WireError::HostKeyUnknown { uri, host, key_type, fingerprint, changed },
            CxError::Connection(s) => WireError::Connection(s),
            CxError::Cancelled => WireError::Cancelled,
            CxError::Io(s) => WireError::Io(s),
        }
    }
}

impl From<WireError> for CxError {
    fn from(e: WireError) -> Self {
        match e {
            WireError::NotFound(s) => CxError::NotFound(s),
            WireError::PermissionDenied(s) => CxError::PermissionDenied(s),
            WireError::AlreadyExists(s) => CxError::AlreadyExists(s),
            WireError::InvalidLocation(s) => CxError::InvalidLocation(s),
            WireError::InvalidName(s) => CxError::InvalidName(s),
            WireError::Unsupported(s) => CxError::Unsupported(s),
            WireError::AuthRequired { uri, user, reason } => CxError::AuthRequired { uri, user, reason },
            WireError::HostKeyUnknown { uri, host, key_type, fingerprint, changed } => CxError::HostKeyUnknown { uri, host, key_type, fingerprint, changed },
            WireError::Connection(s) => CxError::Connection(s),
            WireError::Cancelled => CxError::Cancelled,
            WireError::Io(s) => CxError::Io(s),
        }
    }
}

pub fn conn_err(e: impl std::fmt::Display) -> CxError {
    CxError::Connection(e.to_string())
}

pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, msg: &T) -> Result<()> {
    let body = postcard::to_stdvec(msg).map_err(|e| CxError::Io(format!("encoding message: {e}")))?;
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf).await.map_err(conn_err)
}

/// Read one frame; `Ok(None)` when the stream ended cleanly before it.
pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<Option<T>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        let n = r.read(&mut len[got..]).await.map_err(conn_err)?;
        if n == 0 {
            return if got == 0 { Ok(None) } else { Err(CxError::Connection("stream ended inside a frame".into())) };
        }
        got += n;
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(CxError::Connection(format!("frame too large ({len} bytes)")));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await.map_err(conn_err)?;
    postcard::from_bytes(&body).map(Some).map_err(|e| CxError::Connection(format!("bad message: {e}")))
}

/// Read a frame that must be there.
pub async fn expect_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    read_frame(r).await?.ok_or_else(|| CxError::Connection("peer closed the stream".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_and_reject_oversize() {
        let mut buf = Vec::new();
        let req = Request::List { path: "/Photos/2024".into() };
        write_frame(&mut buf, &req).await.unwrap();
        write_frame(&mut buf, &Response::err(CxError::NotFound("x".into()))).await.unwrap();
        let mut r = &buf[..];
        assert_eq!(read_frame::<_, Request>(&mut r).await.unwrap(), Some(req));
        let resp: Response = expect_frame(&mut r).await.unwrap();
        assert!(matches!(CxError::from(match resp { Response::Err(e) => e, _ => unreachable!() }), CxError::NotFound(_)));
        assert_eq!(read_frame::<_, Request>(&mut r).await.unwrap(), None);

        let huge = ((MAX_FRAME + 1) as u32).to_le_bytes();
        assert!(read_frame::<_, Request>(&mut &huge[..]).await.is_err());
    }
}
