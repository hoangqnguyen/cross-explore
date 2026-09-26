//! SFTP for Cross Explore, on `russh` + `russh-sftp` (pure Rust, async).
//!
//! [`SftpConnector`] opens connections for `sftp://` locations and hands out
//! [`SftpProvider`]s. Host keys are checked against `~/.ssh/known_hosts` and
//! the app's own store. An unknown or changed key fails the connect with
//! [`CxError::HostKeyUnknown`](cx_core::CxError::HostKeyUnknown); the UI shows
//! the fingerprint, and if the user accepts, calls [`trust_host_key`] with
//! the values from that error and connects again.

mod connector;
pub mod known_hosts;
mod provider;
mod session;

pub use connector::SftpConnector;
pub use known_hosts::trust_host_key;
pub use provider::SftpProvider;
