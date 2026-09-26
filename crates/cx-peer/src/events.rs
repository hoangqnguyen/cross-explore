//! Events for the UI. They are serialized with camelCase fields and a
//! `type` tag, since the app forwards them to the web view unchanged.

use crate::protocol::OfferFile;
use crate::trust::TrustedDevice;
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PeerRef {
    pub device_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OfferFileInfo {
    pub name: String,
    pub size: u64,
}

impl From<&OfferFile> for OfferFileInfo {
    fn from(f: &OfferFile) -> Self {
        OfferFileInfo { name: f.name.clone(), size: f.size }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OfferState {
    /// Delivered; waiting for the receiver to accept or decline.
    Pending,
    Accepted,
    Declined,
    Transferring,
    Completed,
    Failed,
    /// The other side went away or gave up before finishing.
    Cancelled,
}

/// One remote operation, as written to the audit log.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuditRecord {
    /// Milliseconds since the Unix epoch.
    pub time: i64,
    pub device_id: String,
    pub name: String,
    pub addr: String,
    pub op: String,
    pub path: Option<String>,
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PeerEvent {
    #[serde(rename_all = "camelCase")]
    IncomingOffer { offer_id: String, from: PeerRef, files: Vec<OfferFileInfo>, total: u64 },
    #[serde(rename_all = "camelCase")]
    OfferProgress {
        offer_id: String,
        direction: Direction,
        peer: PeerRef,
        state: OfferState,
        /// Bytes transferred so far, over all files.
        bytes: u64,
        total: u64,
        /// File being transferred (receiver: its final name on disk).
        file: Option<String>,
        /// Receiver: where the files landed, once completed.
        saved: Vec<String>,
        error: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    PairingCompleted { device: TrustedDevice },
    #[serde(rename_all = "camelCase")]
    PeerConnected { device_id: String, name: String, addr: String, direction: Direction },
    #[serde(rename_all = "camelCase")]
    PeerDisconnected { device_id: String, name: String, reason: String, direction: Direction },
    RemoteAccess(AuditRecord),
}

pub type EventHandler = Arc<dyn Fn(PeerEvent) + Send + Sync>;

pub fn ignore_events() -> EventHandler {
    Arc::new(|_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_tagged_camel_case_json() {
        let e = PeerEvent::IncomingOffer {
            offer_id: "o1".into(),
            from: PeerRef { device_id: "abc".into(), name: "Mac".into() },
            files: vec![OfferFileInfo { name: "a.txt".into(), size: 3 }],
            total: 3,
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "incomingOffer");
        assert_eq!(v["offerId"], "o1");
        assert_eq!(v["from"]["deviceId"], "abc");
        let a = PeerEvent::RemoteAccess(AuditRecord {
            time: 1,
            device_id: "d".into(),
            name: "n".into(),
            addr: "a".into(),
            op: "list".into(),
            path: Some("/S".into()),
            ok: true,
            error: None,
        });
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["type"], "remoteAccess");
        assert_eq!(v["deviceId"], "d");
    }
}
