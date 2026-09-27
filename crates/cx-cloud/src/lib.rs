//! Cloud storage as Cross Explore [`Provider`](cx_core::Provider)s: Google
//! Drive, Dropbox and OneDrive, spoken to directly over their HTTP APIs, so
//! they work on phones and on machines without the vendors' sync apps.
//!
//! ## Locations
//!
//! - `gdrive://me@gmail.com/My Drive/…`, plus the virtual roots
//!   `/Shared drives/<drive>/…` and `/Shared with me/…`
//! - `dropbox://me@example.com/…`
//! - `onedrive://me@outlook.com/…` (personal and work/school accounts, both
//!   through Microsoft Graph's `me/drive`)
//!
//! The account's e-mail address is the authority: its local part is the
//! endpoint user and its domain the host (see [`account_endpoint`]), so the
//! URI reads as the address and credentials are kept per account.
//!
//! ## Signing in
//!
//! OAuth 2.0 for installed apps, with PKCE and a loopback redirect (see the
//! [`oauth`] module). Tokens are stored as the endpoint's credentials (user
//! = account, password = the tokens as JSON), so they end up in the OS
//! keychain like any other password. A connection with missing or dead
//! tokens fails with `CxError::AuthRequired`, which the UI answers by
//! running the sign-in flow.
//!
//! Client IDs are the user's (or the distributor's): each service requires
//! an app registration. They come from settings or from `CX_GDRIVE_CLIENT_ID`
//! / `CX_GDRIVE_CLIENT_SECRET`, `CX_DROPBOX_APP_KEY` and `CX_ONEDRIVE_CLIENT_ID`.
//!
//! ## Common behavior
//!
//! Requests back off on 429/503 (honoring `Retry-After`) and refresh the
//! access token on 401. Downloads and uploads stream: downloads are ranged
//! reads, uploads are cut into chunks (see the `upload` module). None of the
//! services can append, so `WriteMode::Append` is unsupported, and changes
//! are found by polling.
//!
//! ## Why no SDKs
//!
//! There is no maintained Rust SDK for Drive or OneDrive that fits (the
//! generated Google clients drag in hyper 0.14-era stacks and yup-oauth2),
//! and what a file explorer needs is a couple of dozen REST calls per
//! service. Talking to the APIs on the workspace's reqwest (as cx-s3 does)
//! keeps one HTTP/TLS stack and full control over streaming and retries.

mod api;
mod common;
mod connector;
mod dropbox;
mod gdrive;
pub mod oauth;
mod onedrive;
mod service;
mod tokens;
mod upload;
mod util;

pub use api::TokenHook;
pub use connector::{open, CloudConnector, CloudProvider};
pub use dropbox::DropboxProvider;
pub use gdrive::{trash_marker_id, GDriveProvider, MY_DRIVE, SHARED_DRIVES, SHARED_WITH_ME};
pub use oauth::{account_endpoint, complete_authorization, endpoint_account, start_authorization, start_authorization_with_redirect, AuthRequest, SignedIn};
pub use onedrive::OneDriveProvider;
pub use service::{ClientConfig, RetryPolicy, Service, ServiceUrls, DROPBOX_REDIRECT_PORT, DROPBOX_SCHEME, GDRIVE_SCHEME, ONEDRIVE_SCHEME};
pub use tokens::Tokens;

/// The URI schemes this crate serves: `["gdrive", "dropbox", "onedrive"]`.
pub fn schemes() -> [&'static str; 3] {
    Service::ALL.map(Service::scheme)
}
