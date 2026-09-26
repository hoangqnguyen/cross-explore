//! SMB2/3 file shares as a Cross Explore [`Provider`](cx_core::Provider).
//!
//! Locations are `smb://[user@]host[:port]/Share/dir/file`: `/` is the server
//! and lists its shares (NetShareEnumAll over srvsvc), the first path segment
//! is the share.
//!
//! # Why the `smb2` crate
//!
//! Three Rust options exist:
//! - **`smb2`** (MIT/Apache, pure Rust): compound requests, pipelined reads
//!   and writes with an adaptive window, `'static` streaming reader/writer
//!   handles, CHANGE_NOTIFY watcher, srvsvc share enumeration,
//!   FSCTL_SRV_COPYCHUNK, DFS, SMB 3.x signing/encryption, in-place
//!   reconnect, and an integration suite against Samba. Its `Connection` is
//!   a cheap clone multiplexing one session, so operations run concurrently
//!   without a client-wide lock. Built for a file manager (Cmdr).
//! - **`smb`** (MIT, pure Rust): broad protocol coverage (multi-channel,
//!   QUIC, RDMA) but one request at a time, which makes large transfers slow
//!   over any latency, and a thin test suite.
//! - **`pavao`**: bindings to libsmbclient, i.e. a C dependency and GPLv3 —
//!   not acceptable for the default build.
//!
//! `smb2` is the clear fit. Where its high-level API drops something we need
//! (file attributes for hidden/read-only, paged listings, setting times) the
//! [`wire`] module builds the requests from its public message types.
//!
//! # Limitations
//! - smb2's share enumeration filters out admin/special shares (`C$`,
//!   `IPC$`); they can still be opened by typing their path, and any `$`
//!   share that shows up is marked hidden.
//! - No trash (SMB has none), no POSIX permissions.
//! - Kerberos is not wired up yet (NTLM only).

mod connect;
mod error;
mod io;
mod provider;
mod session;
mod watch;
mod wire;

pub use connect::SmbConnector;
pub use provider::SmbProvider;
