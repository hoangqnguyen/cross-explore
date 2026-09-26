use serde::Serialize;
use std::io;

#[derive(Debug, thiserror::Error, Serialize)]
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
}

pub type Result<T> = std::result::Result<T, CxError>;
