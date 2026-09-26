//! FTP and explicit FTPS for Cross Explore, on `suppaftp` (tokio + rustls).
//!
//! [`FtpConnector`] serves `ftp://` and `ftps://` locations. Each connected
//! server gets a small pool of control connections (FTP runs one command at a
//! time per connection), listings prefer `MLSD` and fall back to parsing
//! `LIST`, and passive mode is used throughout.
//!
//! FTPS certificates that no public CA vouches for (the usual case for home
//! servers) go through the same trust-on-first-use flow as SSH host keys:
//! the connect fails with [`CxError::HostKeyUnknown`](cx_core::CxError) and
//! [`trust_host_key`] records the user's decision.

mod connector;
pub mod listing;
mod pool;
mod provider;
pub mod tls;

pub use connector::FtpConnector;
pub use provider::{FtpProvider, POOL_SIZE};
pub use tls::trust_host_key;
