//! Servers and devices: connecting (with the sign-in and host-key prompts
//! in between), disconnecting, pairing and peer sharing, incoming offers.

use super::App;
use crate::dialog::{AfterAuth, Dialog, HostKey, MenuAction, MenuItem};
use crate::msg::Update;
use crate::settings::Bookmark;
use cx_core::{Credentials, CxError, Secret};

fn creds(user: &str, password: &str, key: &str) -> Option<Credentials> {
    if user.is_empty() && password.is_empty() && key.is_empty() {
        return None;
    }
    let secret = if !key.is_empty() {
        Secret::Key { path: key.to_string(), passphrase: (!password.is_empty()).then(|| password.to_string()) }
    } else if !password.is_empty() {
        Secret::Password { password: password.to_string() }
    } else {
        Secret::None
    };
    Some(Credentials { user: user.to_string(), secret })
}

impl App {
    /// Submit the connect dialog (top of the dialog stack).
    pub(crate) fn connect_submit(&mut self) {
        let Some(Dialog::Connect(f)) = self.dialogs.last_mut() else { return };
        f.absorb_uri();
        let Some(uri) = f.uri() else {
            f.error = Some("Enter a server name or address".into());
            return;
        };
        f.busy = true;
        f.error = None;
        let credentials = if f.anonymous { None } else { creds(f.user.text.trim(), &f.password.text, f.key_file.text.trim()) };
        let remember = f.remember;
        let save = f.save.then(|| {
            let host = f.host.text.trim().to_string();
            let path = f.path.text.trim().trim_start_matches('/').to_string();
            Bookmark { name: if path.is_empty() { host.clone() } else { format!("{host}/{path}") }, uri: uri.clone() }
        });
        self.spawn(move |engine| async move {
            let r = engine.connect_server(&uri, credentials, remember).await;
            Box::new(move |app: &mut App| app.on_connected(uri, r, save)) as Update
        });
    }

    fn on_connected(&mut self, uri: String, r: cx_core::Result<()>, save: Option<Bookmark>) {
        let form = self.dialogs.iter_mut().rev().find_map(|d| if let Dialog::Connect(f) = d { Some(f) } else { None });
        match r {
            Ok(()) => {
                self.dialogs.retain(|d| !matches!(d, Dialog::Connect(_)));
                if let Some(s) = save {
                    if !self.settings.servers.iter().any(|x| x.uri == s.uri) {
                        self.settings.servers.push(s);
                        self.save_settings();
                    }
                }
                self.navigate(&uri);
            }
            Err(CxError::HostKeyUnknown { uri: key_uri, host, key_type, fingerprint, changed }) => {
                if let Some(f) = form {
                    f.busy = false;
                }
                self.dialogs.push(Dialog::HostKey(HostKey { uri: if key_uri.is_empty() { uri } else { key_uri }, host, key_type, fingerprint, changed, then: AfterAuth::Connect, on_trust: !changed }));
            }
            Err(CxError::AuthRequired { reason, .. }) => {
                if let Some(f) = form {
                    f.busy = false;
                    f.error = Some(if reason.is_empty() { "Wrong user name or password".into() } else { reason });
                    f.focus = 4;
                }
            }
            Err(e) => {
                if let Some(f) = form {
                    f.busy = false;
                    f.error = Some(e.to_string());
                }
            }
        }
    }

    /// Submit the sign-in prompt.
    pub(crate) fn sign_in_submit(&mut self) {
        let Some(Dialog::SignIn(s)) = self.dialogs.last_mut() else { return };
        s.busy = true;
        s.error = None;
        let uri = s.uri.clone();
        let c = creds(s.user.text.trim(), &s.password.text, s.key_file.text.trim()).unwrap_or_else(Credentials::anonymous);
        let remember = s.remember;
        let then = s.then.clone();
        self.spawn(move |engine| async move {
            let r = engine.connect_server(&uri, Some(c), remember).await;
            Box::new(move |app: &mut App| {
                let dlg = app.dialogs.iter_mut().rev().find_map(|d| if let Dialog::SignIn(s) = d { Some(s) } else { None });
                match r {
                    Ok(()) => {
                        app.dialogs.retain(|d| !matches!(d, Dialog::SignIn(_)));
                        app.after_auth(&uri, then);
                    }
                    Err(CxError::HostKeyUnknown { uri: k, host, key_type, fingerprint, changed }) => {
                        if let Some(s) = dlg {
                            s.busy = false;
                        }
                        app.dialogs.push(Dialog::HostKey(HostKey { uri: if k.is_empty() { uri } else { k }, host, key_type, fingerprint, changed, then: AfterAuth::Reload, on_trust: !changed }));
                    }
                    Err(e) => {
                        if let Some(s) = dlg {
                            s.busy = false;
                            s.error = Some(match e {
                                CxError::AuthRequired { reason, .. } if !reason.is_empty() => reason,
                                CxError::AuthRequired { .. } => "Wrong user name or password".into(),
                                e => e.to_string(),
                            });
                        }
                    }
                }
            }) as Update
        });
    }

    /// The user trusted a server key.
    pub(crate) fn trust_key(&mut self, hk: HostKey) {
        let r = self.engine.trust_host_key(&hk.uri, &hk.key_type, &hk.fingerprint);
        if self.report(r).is_none() {
            return;
        }
        match hk.then {
            AfterAuth::Connect => self.connect_submit(),
            then => self.after_auth(&hk.uri, then),
        }
    }

    fn after_auth(&mut self, uri: &str, then: AfterAuth) {
        match then {
            AfterAuth::Navigate(u) => self.navigate(&u),
            AfterAuth::Connect => self.connect_submit(),
            AfterAuth::Reload => {
                // Re-list everything on that server.
                let host = cx_core::Location::parse(uri).ok().and_then(|l| l.endpoint().map(|e| (e.scheme, e.host.clone())));
                let tokens: Vec<u64> = self
                    .panes
                    .iter()
                    .flat_map(|p| p.tabs.iter())
                    .flat_map(|t| t.folders())
                    .filter(|f| cx_core::Location::parse(&f.uri).ok().and_then(|l| l.endpoint().map(|e| (e.scheme, e.host.clone()))) == host)
                    .map(|f| f.token)
                    .collect();
                for t in tokens {
                    self.reload_token(t);
                }
            }
        }
    }

    pub(crate) fn disconnect(&mut self, uri: String) {
        self.spawn(move |engine| async move {
            let r = engine.disconnect_server(&uri).await;
            Box::new(move |app: &mut App| {
                if app.report(r).is_some() {
                    app.toast(format!("Disconnected from {}", crate::util::display(&uri)));
                    app.navigate(crate::tab::HOME_URI);
                }
            }) as Update
        });
    }

    // ---- peer mode ----

    pub(crate) fn pair_submit(&mut self) {
        let Some(Dialog::Pair(f)) = self.dialogs.last_mut() else { return };
        let (addr, code) = (f.address.text.trim().to_string(), f.code.text.trim().to_string());
        if addr.is_empty() || code.is_empty() {
            f.error = Some("Enter the device's address and the code it shows".into());
            return;
        }
        f.busy = true;
        f.error = None;
        self.spawn(move |engine| async move {
            let r = engine.peer_pair(&addr, &code).await;
            Box::new(move |app: &mut App| match r {
                Ok(p) => {
                    app.dialogs.retain(|d| !matches!(d, Dialog::Pair(_)));
                    app.toast(format!("Paired with {}", p.name));
                    app.refresh_peer();
                }
                Err(e) => {
                    if let Some(Dialog::Pair(f)) = app.dialogs.last_mut() {
                        f.busy = false;
                        f.error = Some(e.to_string());
                    }
                }
            }) as Update
        });
    }

    pub(crate) fn peer_set_enabled(&mut self, on: bool) {
        self.spawn(move |engine| async move {
            let r = engine.peer_set_enabled(on).await;
            Box::new(move |app: &mut App| {
                if let Some(s) = app.report(r) {
                    app.peer = Some(s);
                }
            }) as Update
        });
    }

    pub(crate) fn peer_set_auto_trust(&mut self, on: bool) {
        self.spawn(move |engine| async move {
            let r = engine.peer_set_auto_trust(on).await;
            Box::new(move |app: &mut App| {
                if let Some(s) = app.report(r) {
                    app.peer = Some(s);
                }
            }) as Update
        });
    }

    pub(crate) fn peer_set_shares(&mut self, shares: Vec<cx_peer::Share>) {
        self.spawn(move |engine| async move {
            let r = engine.peer_set_shares(shares).await;
            Box::new(move |app: &mut App| {
                if let Some(s) = app.report(r) {
                    app.peer = Some(s);
                }
            }) as Update
        });
    }

    pub(crate) fn peer_forget(&mut self, id: String) {
        self.spawn(move |engine| async move {
            let r = engine.peer_forget(&id).await;
            Box::new(move |app: &mut App| {
                if let Some(s) = app.report(r) {
                    app.peer = Some(s);
                }
            }) as Update
        });
    }

    pub(crate) fn peer_show_code(&mut self) {
        self.spawn(|engine| async move {
            let r = engine.peer_pair_code().await;
            Box::new(move |app: &mut App| {
                if let Some(code) = app.report(r) {
                    let addr = app.peer.as_ref().map(|p| format!("port {}", p.port)).unwrap_or_default();
                    app.dialogs.push(Dialog::Info { title: "Pairing code".into(), lines: vec![String::new(), format!("      {code}"), String::new(), "Type this code on the other device (palette → Pair a device).".into(), format!("Sharing must be on; this device listens on {addr}.")] });
                }
            }) as Update
        });
    }

    /// Share the current folder with paired devices.
    pub(crate) fn peer_share_current(&mut self) {
        let Some(path) = cx_core::Location::parse(self.tab().dir_uri()).ok().and_then(|l| l.local_path().map(|p| p.to_path_buf())) else {
            self.toast("Only local folders can be shared");
            return;
        };
        let mut shares = self.peer.as_ref().map(|p| p.shares.clone()).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Share".into());
        if shares.iter().any(|s| s.path == path) {
            self.toast("Already shared");
            return;
        }
        shares.push(cx_peer::Share { name, path, read_only: true });
        self.peer_set_shares(shares);
    }

    pub(crate) fn respond_offer(&mut self, id: String, accept: bool, dest: Option<String>) {
        self.spawn(move |engine| async move {
            let r = engine.peer_respond(&id, accept, dest.as_deref()).await;
            Box::new(move |app: &mut App| {
                app.report(r);
            }) as Update
        });
    }

    /// Items of the peer dialog, rebuilt from the latest status.
    pub fn peer_items(&self) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let Some(p) = &self.peer else {
            items.push(MenuItem::new("Peer mode is starting or unavailable", "", MenuAction::None));
            return items;
        };
        items.push(MenuItem::new(if p.enabled { "Sharing: on" } else { "Sharing: off" }, "Enter toggles", MenuAction::None));
        items.push(MenuItem::new(if p.tailnet_auto_trust { "Trust my Tailscale devices: on" } else { "Trust my Tailscale devices: off" }, "Enter toggles", MenuAction::None));
        items.push(MenuItem::new("Show pairing code", "", MenuAction::None));
        items.push(MenuItem::new("Pair with a device…", "", MenuAction::None));
        items.push(MenuItem::new("Share the current folder", "", MenuAction::None));
        items.push(MenuItem::header(format!("Shared folders ({})", p.shares.len())));
        for s in &p.shares {
            items.push(MenuItem::new(format!("{}{}", s.name, if s.read_only { "  (read-only)" } else { "" }), format!("{}  · r toggles read-only · Del removes", s.path.display()), MenuAction::None));
        }
        items.push(MenuItem::header(format!("Paired devices ({})", p.trusted.len())));
        for t in &p.trusted {
            items.push(MenuItem::new(t.name.clone(), format!("{} · Del forgets", t.id), MenuAction::None));
        }
        items
    }
}
