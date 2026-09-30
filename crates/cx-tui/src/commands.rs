//! Every user action, defined once with its label, group and shortcuts per
//! keymap. The keyboard, the command palette, menus and the help screen all
//! read this table, so a shortcut shown anywhere is the one that works.
//!
//! Terminals can't send every combination (see [`crate::keys`]), so most
//! commands list a legacy-friendly key first and the desktop app's key
//! (which needs the kitty keyboard protocol) after it.

use crate::keys::KeyCombo;
use crate::settings::Keymap;
use crate::sort::SortKey;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    // navigate
    Back,
    Forward,
    Up,
    HomePage,
    UserHome,
    GoTo,
    Reload,
    PlacesLeft,
    PlacesRight,
    Hotlist,
    RecentFolders,
    ToggleBookmark,
    // tabs & panes
    NewTab,
    CloseTab,
    ReopenTab,
    NextTab,
    PrevTab,
    TabN(u8),
    ToggleDual,
    SwitchPane,
    SwapPanes,
    Mirror,
    // file
    Open,
    OpenInNewTab,
    OpenExternal,
    Edit,
    QuickLook,
    Rename,
    MultiRename,
    NewFolder,
    Trash,
    Delete,
    Copy,
    Cut,
    Paste,
    Duplicate,
    Undo,
    CopyToOther,
    MoveToOther,
    CopyTo,
    MoveTo,
    CopyPath,
    ShowInFolder,
    Reveal,
    Shell,
    TerminalWindow,
    CalcSize,
    Compress,
    Extract,
    Tags,
    SendTo,
    DiffFiles,
    CompareDirs,
    // selection
    SelectAll,
    SelectNone,
    InvertSelection,
    SelectPattern,
    DeselectPattern,
    // view
    ViewDetails,
    ViewBrief,
    ToggleView,
    TogglePreview,
    ToggleHidden,
    ToggleStripes,
    CollapseAll,
    ExpandAll,
    Filter,
    Search,
    SortBy(SortKey),
    SortMenu,
    // network
    Connect,
    Disconnect,
    Pair,
    PeerSettings,
    Scan,
    // app
    Transfers,
    Palette,
    Help,
    Settings,
    SwitchKeymap,
    SaveWorkspace,
    CycleTheme,
    ToggleIcons,
    Quit,
}

pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub action: Action,
    pub both: &'static [&'static str],
    pub explorer: &'static [&'static str],
    pub commander: &'static [&'static str],
}

const fn c(
    id: &'static str,
    label: &'static str,
    group: &'static str,
    action: Action,
    both: &'static [&'static str],
    explorer: &'static [&'static str],
    commander: &'static [&'static str],
) -> Command {
    Command {
        id,
        label,
        group,
        action,
        both,
        explorer,
        commander,
    }
}

const NONE: &[&str] = &[];

pub static COMMANDS: &[Command] = &[
    // ---- navigate ----
    c(
        "nav.back",
        "Back",
        "Navigate",
        Action::Back,
        &["Alt+Left"],
        NONE,
        NONE,
    ),
    c(
        "nav.forward",
        "Forward",
        "Navigate",
        Action::Forward,
        &["Alt+Right"],
        NONE,
        NONE,
    ),
    c(
        "nav.up",
        "Enclosing folder",
        "Navigate",
        Action::Up,
        &["Backspace", "Alt+Up"],
        NONE,
        NONE,
    ),
    c(
        "nav.home",
        "Go to Home page",
        "Go",
        Action::HomePage,
        &["Alt+Home"],
        NONE,
        NONE,
    ),
    c(
        "nav.userHome",
        "Go to your home folder",
        "Go",
        Action::UserHome,
        &["Ctrl+Home"],
        NONE,
        NONE,
    ),
    c(
        "nav.editPath",
        "Go to folder or path…",
        "Go",
        Action::GoTo,
        &["Ctrl+L", "Ctrl+G", "Alt+D"],
        NONE,
        NONE,
    ),
    c(
        "nav.reload",
        "Refresh",
        "Navigate",
        Action::Reload,
        &["Ctrl+R"],
        &["F5"],
        NONE,
    ),
    c(
        "nav.placesLeft",
        "Places & drives (left pane)…",
        "Go",
        Action::PlacesLeft,
        &["Alt+F1"],
        NONE,
        NONE,
    ),
    c(
        "nav.placesRight",
        "Places & drives (right pane)…",
        "Go",
        Action::PlacesRight,
        &["Alt+F2"],
        NONE,
        NONE,
    ),
    c(
        "nav.hotlist",
        "Favorites (hotlist)…",
        "Go",
        Action::Hotlist,
        &["Ctrl+D"],
        NONE,
        NONE,
    ),
    c(
        "nav.recent",
        "Recent folders…",
        "Go",
        Action::RecentFolders,
        &["Alt+Down"],
        NONE,
        NONE,
    ),
    c(
        "bookmark.toggle",
        "Add to / remove from Favorites",
        "Go",
        Action::ToggleBookmark,
        &["Ctrl+B"],
        NONE,
        NONE,
    ),
    // ---- tabs & panes ----
    c(
        "tab.new",
        "New tab",
        "Tabs",
        Action::NewTab,
        &["Ctrl+T"],
        NONE,
        NONE,
    ),
    c(
        "tab.close",
        "Close tab",
        "Tabs",
        Action::CloseTab,
        &["Ctrl+W"],
        NONE,
        NONE,
    ),
    c(
        "tab.reopen",
        "Reopen closed tab",
        "Tabs",
        Action::ReopenTab,
        &["Ctrl+Shift+T", "Alt+Shift+T"],
        NONE,
        NONE,
    ),
    c(
        "tab.next",
        "Next tab",
        "Tabs",
        Action::NextTab,
        &["Ctrl+Tab", "Ctrl+PageDown", "Alt+]"],
        NONE,
        NONE,
    ),
    c(
        "tab.prev",
        "Previous tab",
        "Tabs",
        Action::PrevTab,
        &["Ctrl+Shift+Tab", "Ctrl+PageUp", "Alt+["],
        NONE,
        NONE,
    ),
    c(
        "tab.1",
        "Go to tab 1",
        "Tabs",
        Action::TabN(1),
        &["Alt+1"],
        NONE,
        NONE,
    ),
    c(
        "tab.2",
        "Go to tab 2",
        "Tabs",
        Action::TabN(2),
        &["Alt+2"],
        NONE,
        NONE,
    ),
    c(
        "tab.3",
        "Go to tab 3",
        "Tabs",
        Action::TabN(3),
        &["Alt+3"],
        NONE,
        NONE,
    ),
    c(
        "tab.4",
        "Go to tab 4",
        "Tabs",
        Action::TabN(4),
        &["Alt+4"],
        NONE,
        NONE,
    ),
    c(
        "tab.5",
        "Go to tab 5",
        "Tabs",
        Action::TabN(5),
        &["Alt+5"],
        NONE,
        NONE,
    ),
    c(
        "tab.6",
        "Go to tab 6",
        "Tabs",
        Action::TabN(6),
        &["Alt+6"],
        NONE,
        NONE,
    ),
    c(
        "tab.7",
        "Go to tab 7",
        "Tabs",
        Action::TabN(7),
        &["Alt+7"],
        NONE,
        NONE,
    ),
    c(
        "tab.8",
        "Go to tab 8",
        "Tabs",
        Action::TabN(8),
        &["Alt+8"],
        NONE,
        NONE,
    ),
    c(
        "tab.9",
        "Go to last tab",
        "Tabs",
        Action::TabN(9),
        &["Alt+9"],
        NONE,
        NONE,
    ),
    c(
        "pane.dual",
        "Toggle dual pane",
        "Panes",
        Action::ToggleDual,
        &["F9", "Ctrl+\\"],
        NONE,
        NONE,
    ),
    c(
        "pane.switch",
        "Switch to other pane",
        "Panes",
        Action::SwitchPane,
        &["Tab"],
        NONE,
        NONE,
    ),
    c(
        "pane.swap",
        "Swap panes",
        "Panes",
        Action::SwapPanes,
        &["Ctrl+U"],
        NONE,
        NONE,
    ),
    c(
        "pane.mirror",
        "Show this folder in the other pane",
        "Panes",
        Action::Mirror,
        &["Alt+M", "Ctrl+Shift+M"],
        NONE,
        NONE,
    ),
    // ---- file ----
    c(
        "file.open",
        "Open",
        "File",
        Action::Open,
        &["Enter"],
        NONE,
        NONE,
    ),
    c(
        "file.openTab",
        "Open in new tab",
        "File",
        Action::OpenInNewTab,
        &["Alt+Enter", "Ctrl+Enter"],
        NONE,
        NONE,
    ),
    c(
        "file.openExternal",
        "Open with default app",
        "File",
        Action::OpenExternal,
        &["Alt+O"],
        NONE,
        NONE,
    ),
    c(
        "file.edit",
        "Edit in $EDITOR",
        "File",
        Action::Edit,
        &["F4"],
        NONE,
        NONE,
    ),
    c(
        "file.quicklook",
        "Quick Look",
        "File",
        Action::QuickLook,
        &["F3"],
        &["Space"],
        NONE,
    ),
    c(
        "file.rename",
        "Rename",
        "File",
        Action::Rename,
        &["F2"],
        NONE,
        &["Shift+F6"],
    ),
    c(
        "file.multiRename",
        "Rename multiple…",
        "Tools",
        Action::MultiRename,
        &["Ctrl+M", "Shift+F2"],
        NONE,
        NONE,
    ),
    c(
        "file.newFolder",
        "New folder",
        "File",
        Action::NewFolder,
        &["F7", "Ctrl+N"],
        NONE,
        NONE,
    ),
    c(
        "file.trash",
        "Move to Trash",
        "File",
        Action::Trash,
        &["Delete", "F8"],
        NONE,
        NONE,
    ),
    c(
        "file.delete",
        "Delete permanently…",
        "File",
        Action::Delete,
        &["Shift+Delete", "Shift+F8"],
        NONE,
        NONE,
    ),
    c(
        "edit.copy",
        "Copy",
        "Edit",
        Action::Copy,
        &["Ctrl+C"],
        NONE,
        NONE,
    ),
    c(
        "edit.cut",
        "Cut",
        "Edit",
        Action::Cut,
        &["Ctrl+X"],
        NONE,
        NONE,
    ),
    c(
        "edit.paste",
        "Paste",
        "Edit",
        Action::Paste,
        &["Ctrl+V"],
        NONE,
        NONE,
    ),
    c(
        "edit.duplicate",
        "Duplicate",
        "Edit",
        Action::Duplicate,
        &["Alt+Shift+D"],
        NONE,
        &["Shift+F5"],
    ),
    c(
        "edit.undo",
        "Undo",
        "Edit",
        Action::Undo,
        &["Ctrl+Z"],
        NONE,
        NONE,
    ),
    c(
        "file.copyOther",
        "Copy to other pane",
        "Panes",
        Action::CopyToOther,
        NONE,
        NONE,
        &["F5"],
    ),
    c(
        "file.moveOther",
        "Move to other pane",
        "Panes",
        Action::MoveToOther,
        NONE,
        NONE,
        &["F6"],
    ),
    c(
        "file.copyTo",
        "Copy to…",
        "File",
        Action::CopyTo,
        &["Alt+C"],
        &["Shift+F5"],
        NONE,
    ),
    c(
        "file.moveTo",
        "Move to…",
        "File",
        Action::MoveTo,
        &["Alt+X"],
        &["Shift+F6"],
        NONE,
    ),
    c(
        "file.copyPath",
        "Copy path",
        "File",
        Action::CopyPath,
        &["Alt+Shift+C", "Ctrl+Shift+C"],
        NONE,
        NONE,
    ),
    c(
        "file.showInFolder",
        "Show in enclosing folder",
        "File",
        Action::ShowInFolder,
        &["Alt+L"],
        NONE,
        NONE,
    ),
    c(
        "file.reveal",
        "Show in system file manager",
        "File",
        Action::Reveal,
        &["Alt+R"],
        NONE,
        NONE,
    ),
    c(
        "file.shell",
        "Open shell here",
        "Tools",
        Action::Shell,
        &["Ctrl+O"],
        NONE,
        NONE,
    ),
    c(
        "file.terminal",
        "Open in Terminal app",
        "Tools",
        Action::TerminalWindow,
        NONE,
        NONE,
        NONE,
    ),
    c(
        "file.calcSize",
        "Calculate folder sizes",
        "Tools",
        Action::CalcSize,
        &["Alt+Shift+Enter"],
        NONE,
        NONE,
    ),
    c(
        "file.compress",
        "Compress to ZIP…",
        "Tools",
        Action::Compress,
        NONE,
        NONE,
        &["Alt+F5"],
    ),
    c(
        "file.extract",
        "Extract here",
        "Tools",
        Action::Extract,
        NONE,
        NONE,
        &["Alt+F9"],
    ),
    c(
        "file.tags",
        "Tags…",
        "File",
        Action::Tags,
        &["Alt+G", "Ctrl+Alt+G"],
        NONE,
        NONE,
    ),
    c(
        "file.sendTo",
        "Send to device…",
        "Network",
        Action::SendTo,
        NONE,
        NONE,
        NONE,
    ),
    c(
        "file.diff",
        "Compare files",
        "Tools",
        Action::DiffFiles,
        &["Ctrl+Alt+C"],
        NONE,
        NONE,
    ),
    c(
        "file.compareDirs",
        "Compare & sync folders",
        "Tools",
        Action::CompareDirs,
        &["Alt+K", "Ctrl+Shift+K"],
        NONE,
        NONE,
    ),
    // ---- selection ----
    c(
        "sel.all",
        "Select all",
        "Edit",
        Action::SelectAll,
        &["Ctrl+A"],
        NONE,
        NONE,
    ),
    c(
        "sel.none",
        "Select none",
        "Edit",
        Action::SelectNone,
        &["Alt+A", "Ctrl+Shift+A"],
        NONE,
        NONE,
    ),
    c(
        "sel.invert",
        "Invert selection",
        "Edit",
        Action::InvertSelection,
        &["Alt+I"],
        NONE,
        &["*"],
    ),
    c(
        "sel.pattern",
        "Select by pattern…",
        "Edit",
        Action::SelectPattern,
        &["Alt+="],
        NONE,
        &["+"],
    ),
    c(
        "sel.unpattern",
        "Deselect by pattern…",
        "Edit",
        Action::DeselectPattern,
        &["Alt+-"],
        NONE,
        &["-"],
    ),
    // ---- view ----
    c(
        "view.details",
        "Details view",
        "View",
        Action::ViewDetails,
        &["Ctrl+F2"],
        NONE,
        NONE,
    ),
    c(
        "view.brief",
        "Brief view (names in columns)",
        "View",
        Action::ViewBrief,
        &["Ctrl+F1"],
        NONE,
        NONE,
    ),
    c(
        "view.toggle",
        "Switch Details / Brief view",
        "View",
        Action::ToggleView,
        &["Alt+V"],
        NONE,
        NONE,
    ),
    c(
        "view.preview",
        "Toggle preview pane",
        "View",
        Action::TogglePreview,
        &["Alt+P", "Ctrl+Shift+P"],
        NONE,
        NONE,
    ),
    c(
        "view.hidden",
        "Toggle hidden items",
        "View",
        Action::ToggleHidden,
        &["Alt+H", "Ctrl+H", "Ctrl+."],
        NONE,
        NONE,
    ),
    c(
        "view.stripes",
        "Toggle alternating row colors",
        "View",
        Action::ToggleStripes,
        NONE,
        NONE,
        NONE,
    ),
    c(
        "view.collapseAll",
        "Collapse all folders",
        "View",
        Action::CollapseAll,
        &["Alt+Shift+Left", "Ctrl+Alt+Left"],
        NONE,
        NONE,
    ),
    c(
        "view.expandAll",
        "Expand folder and all subfolders",
        "View",
        Action::ExpandAll,
        &["Shift+Right", "Alt+Shift+Right"],
        NONE,
        NONE,
    ),
    c(
        "view.find",
        "Filter this folder",
        "View",
        Action::Filter,
        &["Ctrl+F", "/"],
        NONE,
        NONE,
    ),
    c(
        "view.search",
        "Search in subfolders / find text…",
        "Tools",
        Action::Search,
        &["Alt+F7", "Ctrl+Shift+F"],
        NONE,
        NONE,
    ),
    c(
        "sort.name",
        "Sort by name",
        "View",
        Action::SortBy(SortKey::Name),
        &["Ctrl+F3"],
        NONE,
        NONE,
    ),
    c(
        "sort.type",
        "Sort by type",
        "View",
        Action::SortBy(SortKey::Type),
        &["Ctrl+F4"],
        NONE,
        NONE,
    ),
    c(
        "sort.date",
        "Sort by date modified",
        "View",
        Action::SortBy(SortKey::Modified),
        &["Ctrl+F5"],
        NONE,
        NONE,
    ),
    c(
        "sort.size",
        "Sort by size",
        "View",
        Action::SortBy(SortKey::Size),
        &["Ctrl+F6"],
        NONE,
        NONE,
    ),
    c(
        "sort.menu",
        "Sort by…",
        "View",
        Action::SortMenu,
        &["Alt+S"],
        NONE,
        NONE,
    ),
    // ---- network ----
    c(
        "net.connect",
        "Connect to server…",
        "Network",
        Action::Connect,
        &["Ctrl+K"],
        NONE,
        NONE,
    ),
    c(
        "net.disconnect",
        "Disconnect from this server",
        "Network",
        Action::Disconnect,
        NONE,
        NONE,
        NONE,
    ),
    c(
        "net.pair",
        "Pair a device…",
        "Network",
        Action::Pair,
        NONE,
        NONE,
        NONE,
    ),
    c(
        "net.peer",
        "Sharing & paired devices…",
        "Network",
        Action::PeerSettings,
        NONE,
        NONE,
        NONE,
    ),
    c(
        "net.scan",
        "Scan for nearby devices",
        "Network",
        Action::Scan,
        NONE,
        NONE,
        NONE,
    ),
    // ---- app ----
    c(
        "transfers.show",
        "Show transfers",
        "App",
        Action::Transfers,
        &["Ctrl+J"],
        NONE,
        NONE,
    ),
    c(
        "app.palette",
        "Command palette…",
        "App",
        Action::Palette,
        &["Ctrl+P"],
        NONE,
        NONE,
    ),
    c(
        "app.help",
        "Keyboard shortcuts",
        "App",
        Action::Help,
        &["F1"],
        NONE,
        NONE,
    ),
    c(
        "app.settings",
        "Settings…",
        "App",
        Action::Settings,
        &["Alt+,", "Ctrl+,"],
        NONE,
        NONE,
    ),
    c(
        "app.keymap",
        "Switch keyboard style (Explorer / Commander)",
        "App",
        Action::SwitchKeymap,
        NONE,
        NONE,
        NONE,
    ),
    c(
        "app.saveWorkspace",
        "Save workspace…",
        "App",
        Action::SaveWorkspace,
        NONE,
        NONE,
        NONE,
    ),
    c(
        "app.theme",
        "Next color theme",
        "App",
        Action::CycleTheme,
        NONE,
        NONE,
        NONE,
    ),
    c(
        "app.icons",
        "Toggle Nerd Font icons",
        "App",
        Action::ToggleIcons,
        NONE,
        NONE,
        NONE,
    ),
    c(
        "app.quit",
        "Quit",
        "App",
        Action::Quit,
        &["F10", "Ctrl+Q"],
        NONE,
        NONE,
    ),
];

struct Compiled {
    both: Vec<Vec<KeyCombo>>,
    explorer: Vec<Vec<KeyCombo>>,
    commander: Vec<Vec<KeyCombo>>,
}

fn compiled() -> &'static Compiled {
    static C: OnceLock<Compiled> = OnceLock::new();
    C.get_or_init(|| {
        let parse = |keys: &[&str]| {
            keys.iter()
                .filter_map(|k| KeyCombo::parse(k))
                .collect::<Vec<_>>()
        };
        Compiled {
            both: COMMANDS.iter().map(|c| parse(c.both)).collect(),
            explorer: COMMANDS.iter().map(|c| parse(c.explorer)).collect(),
            commander: COMMANDS.iter().map(|c| parse(c.commander)).collect(),
        }
    })
}

pub fn by_action(a: Action) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.action == a)
}

/// Shortcut strings for a command under a keymap (keymap-specific first).
pub fn keys_for(cmd: &Command, keymap: Keymap) -> Vec<&'static str> {
    let specific = match keymap {
        Keymap::Explorer => cmd.explorer,
        Keymap::Commander => cmd.commander,
    };
    specific.iter().chain(cmd.both.iter()).copied().collect()
}

/// First shortcut, formatted, for menus and the palette.
pub fn shortcut(a: Action, keymap: Keymap) -> Option<String> {
    let cmd = by_action(a)?;
    keys_for(cmd, keymap)
        .first()
        .and_then(|k| KeyCombo::parse(k))
        .map(|k| k.to_string())
}

/// Every command bound to `combo`, keymap-specific bindings first (so
/// Commander's F5 wins over Explorer's F5 = Refresh). The caller runs the
/// first one that applies.
pub fn resolve(combo: &KeyCombo, keymap: Keymap) -> Vec<Action> {
    let c = compiled();
    let specific = match keymap {
        Keymap::Explorer => &c.explorer,
        Keymap::Commander => &c.commander,
    };
    let mut out: Vec<Action> = COMMANDS
        .iter()
        .zip(specific)
        .filter(|(_, k)| k.contains(combo))
        .map(|(cmd, _)| cmd.action)
        .collect();
    out.extend(
        COMMANDS
            .iter()
            .zip(&c.both)
            .filter(|(_, k)| k.contains(combo))
            .map(|(cmd, _)| cmd.action),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn k(s: &str) -> KeyCombo {
        KeyCombo::parse(s).unwrap()
    }

    #[test]
    fn every_binding_parses_and_ids_are_unique() {
        let mut ids = HashSet::new();
        for cmd in COMMANDS {
            assert!(ids.insert(cmd.id), "duplicate id {}", cmd.id);
            for key in cmd.both.iter().chain(cmd.explorer).chain(cmd.commander) {
                assert!(
                    KeyCombo::parse(key).is_some(),
                    "{} has a bad key {key}",
                    cmd.id
                );
            }
        }
    }

    #[test]
    fn keymaps_resolve_like_the_desktop_app() {
        assert_eq!(resolve(&k("F5"), Keymap::Commander)[0], Action::CopyToOther);
        assert_eq!(resolve(&k("F5"), Keymap::Explorer)[0], Action::Reload);
        assert_eq!(
            resolve(&k("Space"), Keymap::Explorer),
            vec![Action::QuickLook]
        );
        assert!(
            resolve(&k("Space"), Keymap::Commander).is_empty(),
            "Commander's Space toggles selection (handled by the list)"
        );
        assert_eq!(
            resolve(&k("Shift+F6"), Keymap::Commander)[0],
            Action::Rename
        );
        assert_eq!(resolve(&k("Shift+F6"), Keymap::Explorer)[0], Action::MoveTo);
        assert_eq!(
            resolve(&k("+"), Keymap::Commander),
            vec![Action::SelectPattern]
        );
        assert!(
            resolve(&k("+"), Keymap::Explorer).is_empty(),
            "types into the filter"
        );
        assert_eq!(
            resolve(&k("Ctrl+\\"), Keymap::Explorer),
            vec![Action::ToggleDual]
        );
        assert_eq!(
            resolve(&k("Ctrl+F4"), Keymap::Commander),
            vec![Action::SortBy(SortKey::Type)]
        );
        assert_eq!(
            resolve(&k("Alt+3"), Keymap::Commander),
            vec![Action::TabN(3)]
        );
        assert_eq!(
            shortcut(Action::Palette, Keymap::Explorer).as_deref(),
            Some("Ctrl+P")
        );
        assert_eq!(
            shortcut(Action::CopyToOther, Keymap::Commander).as_deref(),
            Some("F5")
        );
    }

    #[test]
    fn no_key_is_bound_twice_within_a_keymap() {
        for keymap in [Keymap::Explorer, Keymap::Commander] {
            let mut seen = std::collections::HashMap::new();
            for cmd in COMMANDS {
                // Specific bindings override shared ones on purpose.
                let specific = match keymap {
                    Keymap::Explorer => cmd.explorer,
                    Keymap::Commander => cmd.commander,
                };
                for key in specific {
                    let prev = seen.insert(k(key), cmd.id);
                    assert!(
                        prev.is_none(),
                        "{key} bound to {} and {} in {keymap:?}",
                        prev.unwrap(),
                        cmd.id
                    );
                }
            }
            let mut shared = std::collections::HashMap::new();
            for cmd in COMMANDS {
                for key in cmd.both {
                    let prev = shared.insert(k(key), cmd.id);
                    assert!(
                        prev.is_none(),
                        "{key} bound to {} and {}",
                        prev.unwrap(),
                        cmd.id
                    );
                }
            }
        }
    }
}
