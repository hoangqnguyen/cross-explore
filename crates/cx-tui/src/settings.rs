//! Preferences and the saved session, as JSON in the config folder
//! (`~/.config/cross-explore/tui.json`, `~/Library/Application Support/…`
//! on macOS, `%APPDATA%\…` on Windows). Unknown or missing fields fall back
//! to defaults, so older files keep loading.

use crate::sort::SortSpec;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Keymap {
    /// Explorer / Finder-like keys.
    #[default]
    Explorer,
    /// Total Commander F-keys.
    Commander,
}

impl Keymap {
    pub fn label(self) -> &'static str {
        match self {
            Keymap::Explorer => "Explorer",
            Keymap::Commander => "Commander",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ThemeName {
    /// Dark when the terminal supports true color, else the 16-color theme.
    #[default]
    Auto,
    Dark,
    Light,
    /// The terminal's own 16 colors and background.
    Terminal,
}

impl ThemeName {
    pub fn label(self) -> &'static str {
        match self {
            ThemeName::Auto => "Auto",
            ThemeName::Dark => "Dark",
            ThemeName::Light => "Light",
            ThemeName::Terminal => "Terminal colors",
        }
    }

    pub fn next(self) -> ThemeName {
        match self {
            ThemeName::Auto => ThemeName::Dark,
            ThemeName::Dark => ThemeName::Light,
            ThemeName::Light => ThemeName::Terminal,
            ThemeName::Terminal => ThemeName::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ViewMode {
    #[default]
    Details,
    /// Total Commander's "Brief": names in several columns.
    Brief,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedTab {
    pub uri: String,
    #[serde(default)]
    pub view: ViewMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SavedPane {
    pub tabs: Vec<SavedTab>,
    pub active: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub dual: bool,
    pub panes: Vec<SavedPane>,
    #[serde(default)]
    pub active_pane: usize,
}

/// A named set of tabs, reopened from the palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    pub session: Session,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub keymap: Keymap,
    pub theme: ThemeName,
    pub nerd_icons: bool,
    pub show_hidden: bool,
    pub stripes: bool,
    pub dual: bool,
    pub preview_pane: bool,
    pub sort: SortSpec,
    pub default_view: ViewMode,
    pub confirm_trash: bool,
    pub confirm_permanent_delete: bool,
    /// Ask before F5 / F6 copy or move to the other pane.
    pub confirm_transfer: bool,
    pub bookmarks: Vec<Bookmark>,
    pub servers: Vec<Bookmark>,
    pub recent: Vec<String>,
    /// Where "Copy to…" / "Move to…" last sent things, newest first.
    pub recent_destinations: Vec<String>,
    pub session: Option<Session>,
    pub workspaces: Vec<Workspace>,
    pub restore_session: bool,
    pub mouse: bool,
    /// Also put copied local files on the system clipboard (and paste
    /// files copied in Finder / Explorer).
    pub os_clipboard: bool,
    /// The macOS Full Disk Access tip was shown.
    pub fda_tip_shown: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            keymap: Keymap::Explorer,
            theme: ThemeName::Auto,
            nerd_icons: false,
            show_hidden: false,
            stripes: true,
            dual: false,
            preview_pane: false,
            sort: SortSpec::default(),
            default_view: ViewMode::Details,
            confirm_trash: false,
            confirm_permanent_delete: true,
            confirm_transfer: true,
            bookmarks: Vec::new(),
            servers: Vec::new(),
            recent: Vec::new(),
            recent_destinations: Vec::new(),
            session: None,
            workspaces: Vec::new(),
            restore_session: true,
            mouse: true,
            os_clipboard: true,
            fda_tip_shown: false,
        }
    }
}

const RECENT_MAX: usize = 30;
const RECENT_DEST_MAX: usize = 12;

fn push_front(list: &mut Vec<String>, uri: &str, max: usize) {
    list.retain(|u| u != uri);
    list.insert(0, uri.to_string());
    list.truncate(max);
}

impl Settings {
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("cross-explore").join("tui.json"))
    }

    pub fn load(path: &Path) -> Settings {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        // Write then rename, so a crash mid-write never loses the settings.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(tmp, path)
    }

    pub fn add_recent(&mut self, uri: &str) {
        push_front(&mut self.recent, uri, RECENT_MAX);
    }

    pub fn add_recent_destination(&mut self, uri: &str) {
        push_front(&mut self.recent_destinations, uri, RECENT_DEST_MAX);
    }

    pub fn is_bookmarked(&self, uri: &str) -> bool {
        self.bookmarks.iter().any(|b| b.uri == uri)
    }

    /// Add or remove; returns true when added.
    pub fn toggle_bookmark(&mut self, name: &str, uri: &str) -> bool {
        if self.is_bookmarked(uri) {
            self.bookmarks.retain(|b| b.uri != uri);
            false
        } else {
            self.bookmarks.push(Bookmark { name: name.into(), uri: uri.into() });
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tolerant_loading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/tui.json");
        let mut s = Settings { keymap: Keymap::Commander, ..Default::default() };
        assert!(s.toggle_bookmark("Docs", "file:///docs"));
        s.add_recent("file:///a");
        s.add_recent("file:///b");
        s.add_recent("file:///a");
        assert_eq!(s.recent, vec!["file:///a", "file:///b"]);
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        std::fs::write(&path, br#"{"keymap":"commander","unknown":1}"#).unwrap();
        let l = Settings::load(&path);
        assert_eq!(l.keymap, Keymap::Commander);
        assert!(l.stripes, "missing fields take defaults");
        assert!(!s.toggle_bookmark("Docs", "file:///docs"));
    }
}
