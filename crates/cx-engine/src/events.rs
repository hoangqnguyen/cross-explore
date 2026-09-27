//! One app-wide event stream (jobs, devices, offers, peer status). The front
//! end subscribes once with [`Emitter::set`]; the desktop app forwards each
//! event over a Tauri channel as JSON, the terminal UI over an mpsc channel.

use crate::jobs::JobView;
use crate::peer::PeerStatus;
use cx_discovery::Device;
use cx_peer::OfferFileInfo;
use serde::Serialize;
use std::sync::{Arc, Mutex};

/// Who is offering files, as shown in the accept/decline prompt.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OfferPeer {
    pub id: String,
    pub name: String,
}

/// Files another device wants to send us.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IncomingOffer {
    pub id: String,
    pub from: OfferPeer,
    pub files: Vec<OfferFileInfo>,
    pub total: u64,
}

/// Serialized as `{"type": "job", "job": {...}}` and so on: exactly what the
/// desktop UI's `subscribe` handler expects.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
// Moved straight to the front end a few times a second; boxing the job
// would only add an allocation per progress tick.
#[allow(clippy::large_enum_variant)]
pub enum EngineEvent {
    /// A job was added or changed.
    Job { job: JobView },
    /// The nearby-device list changed (the whole list).
    Devices { devices: Vec<Device> },
    /// Another device offers files.
    Offer { offer: IncomingOffer },
    /// Peer mode was (re)started, or a pairing/connection changed.
    Peer { status: PeerStatus },
}

pub type EventSink = Arc<dyn Fn(EngineEvent) + Send + Sync>;

/// Holds the subscriber. Events before the first subscription are dropped:
/// the front end asks for current state (jobs, devices, peer status) when it
/// subscribes.
#[derive(Default)]
pub struct Emitter {
    sink: Mutex<Option<EventSink>>,
}

impl Emitter {
    pub fn set(&self, sink: EventSink) {
        *self.sink.lock().unwrap() = Some(sink);
    }

    pub fn emit(&self, e: EngineEvent) {
        let sink = self.sink.lock().unwrap().clone();
        if let Some(s) = sink {
            s(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_like_the_desktop_ui_expects() {
        let e = EngineEvent::Offer { offer: IncomingOffer { id: "o".into(), from: OfferPeer { id: "d".into(), name: "Mac".into() }, files: vec![], total: 3 } };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "offer");
        assert_eq!(v["offer"]["from"]["name"], "Mac");
        let e = EngineEvent::Devices { devices: vec![] };
        assert_eq!(serde_json::to_value(&e).unwrap()["type"], "devices");
    }
}
