//! Test doubles shared by Cross Explore crates.
//!
//! [`MemProvider`] is a complete in-memory [`Provider`](cx_core::Provider)
//! that can pretend to be a slow or flaky remote: add per-chunk delays, cap
//! throughput, or make streams fail after a number of bytes to exercise
//! resume logic. [`MemConnector`] plugs it into a [`Vfs`](cx_core::Vfs) under
//! any remote scheme, so code under test goes through the same connection
//! registry as real SFTP/SMB/… endpoints.

mod connector;
mod provider;
mod stream;

pub use connector::{mem_endpoint, mem_vfs, MemConnector};
pub use provider::MemProvider;
