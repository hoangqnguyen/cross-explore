//! IPC surface for the UI, one module per area.

pub mod files;
pub mod jobs;
pub mod net;
pub mod peer;
pub mod search;
pub mod selftest;
pub mod system;
pub mod tags;

use crate::state::App;
use std::sync::Arc;

pub type AppState<'a> = tauri::State<'a, Arc<App>>;
