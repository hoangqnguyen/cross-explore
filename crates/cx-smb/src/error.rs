//! Maps smb2 errors onto [`CxError`] so the UI can react by kind (ask for a
//! password, offer "replace?", show "not found") instead of by message text.

use cx_core::CxError;
use smb2::types::status::NtStatus;
use smb2::{Error, ErrorKind};

/// Convert an smb2 error, naming what we were doing in `ctx` (usually a URI).
pub(crate) fn map(err: Error, ctx: impl std::fmt::Display) -> CxError {
    let ctx = ctx.to_string();
    match err.kind() {
        ErrorKind::NotFound => CxError::NotFound(ctx),
        ErrorKind::AlreadyExists => CxError::AlreadyExists(ctx),
        ErrorKind::AccessDenied => CxError::PermissionDenied(ctx),
        ErrorKind::InvalidName => CxError::InvalidName(ctx),
        ErrorKind::Unsupported => CxError::Unsupported(format!("{ctx}: {err}")),
        ErrorKind::Cancelled => CxError::Cancelled,
        ErrorKind::ConnectionLost | ErrorKind::TimedOut | ErrorKind::SessionExpired => {
            CxError::Connection(format!("{ctx}: {err}"))
        }
        _ => CxError::Io(format!("{ctx}: {err}")),
    }
}

/// The raw NTSTATUS of a protocol error, when there is one.
pub(crate) fn status(err: &Error) -> Option<NtStatus> {
    match err {
        Error::Protocol { status, .. } => Some(*status),
        _ => None,
    }
}

/// Errors after which the cached tree connect (or the whole session) is no
/// longer usable, so the operation is worth one retry on a fresh one.
pub(crate) fn is_stale(err: &Error) -> bool {
    matches!(err.kind(), ErrorKind::ConnectionLost | ErrorKind::SessionExpired)
        || matches!(status(err), Some(s) if s == NtStatus::NETWORK_NAME_DELETED || s == NtStatus::USER_SESSION_DELETED)
}
