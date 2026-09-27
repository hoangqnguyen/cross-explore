//! Cross Explore in the terminal.
//!
//! The same engine as the desktop app ([`cx_engine`]): every protocol,
//! transfers with conflicts and undo, archives, search, tags, nearby
//! devices and peer mode. The UI follows the desktop app's two keyboard
//! styles (Explorer/Finder and Total Commander) and its feature set, adapted
//! to a terminal: dual panes with tabs, a Finder-style outline, a Brief
//! multi-column view, Quick Look, a preview pane, a command palette.
//!
//! State and behaviour ([`app`]) are separate from drawing ([`ui`]): the
//! app handles keys, mouse and background results; the UI only reads it.
//! Background work runs on tokio and reports back through [`msg::Msg`], so
//! drawing never waits for a disk or a server.

pub mod app;
pub mod commands;
pub mod dialog;
pub mod folder;
pub mod format;
pub mod highlight;
pub mod input;
pub mod keys;
pub mod msg;
pub mod pattern;
pub mod preview;
pub mod rename;
pub mod settings;
pub mod sort;
pub mod tab;
pub mod term;
pub mod ui;
pub mod util;

pub use app::App;
