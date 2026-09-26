use serde::Serialize;
use std::io;

#[derive(Debug, Clone, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "camelCase")]
pub enum CxError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid location: {0}")]
    InvalidLocation(String),
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The endpoint needs (different) credentials. The UI asks for them and
    /// retries after `connect`.
    #[error("sign-in required for {uri}")]
    #[serde(rename_all = "camelCase")]
    AuthRequired { uri: String, user: Option<String>, reason: String },
    /// An SSH (or peer) host presented a key we have not seen before.
    #[error("unknown host key for {host}")]
    #[serde(rename_all = "camelCase")]
    HostKeyUnknown { uri: String, host: String, key_type: String, fingerprint: String, changed: bool },
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(String),
}

impl CxError {
    pub fn from_io(err: io::Error, context: impl std::fmt::Display) -> Self {
        let ctx = context.to_string();
        match err.kind() {
            io::ErrorKind::NotFound => CxError::NotFound(ctx),
            io::ErrorKind::PermissionDenied => CxError::PermissionDenied(ctx),
            io::ErrorKind::AlreadyExists => CxError::AlreadyExists(ctx),
            _ => CxError::Io(format!("{ctx}: {err}")),
        }
    }

    pub fn io(context: impl std::fmt::Display, err: impl std::fmt::Display) -> Self {
        CxError::Io(format!("{context}: {err}"))
    }
}

impl From<CxError> for io::Error {
    fn from(e: CxError) -> io::Error {
        let kind = match &e {
            CxError::NotFound(_) => io::ErrorKind::NotFound,
            CxError::PermissionDenied(_) => io::ErrorKind::PermissionDenied,
            CxError::AlreadyExists(_) => io::ErrorKind::AlreadyExists,
            _ => io::ErrorKind::Other,
        };
        io::Error::new(kind, e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CxError>;
