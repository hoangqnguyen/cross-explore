//! SFTP / FTP connectors (registered once their crates are merged in).

use crate::state::App;
use cx_core::{CxError, Result, Vfs};
use std::path::Path;

pub fn register(_vfs: &Vfs, _data_dir: &Path) {}

pub fn trust(_app: &App, _uri: &str, _key_type: &str, _fingerprint: &str) -> Result<()> {
    Err(CxError::Unsupported("SFTP".into()))
}
