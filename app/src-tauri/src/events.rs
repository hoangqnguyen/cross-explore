//! One app-wide event stream to the UI (jobs, devices, peers), delivered over
//! a Tauri channel the UI opens once with `subscribe`.

use serde::Serialize;
use serde_json::Value;
use std::sync::Mutex;
use tauri::ipc::Channel;

#[derive(Default)]
pub struct Events {
    channel: Mutex<Option<Channel<Value>>>,
}

impl Events {
    pub fn set(&self, ch: Channel<Value>) {
        *self.channel.lock().unwrap() = Some(ch);
    }

    /// Send `{"type": kind, ...payload}`.
    pub fn emit(&self, kind: &str, payload: impl Serialize) {
        if self.channel.lock().unwrap().is_none() {
            return;
        }
        self.emit_value(kind, serde_json::to_value(payload).unwrap_or(Value::Null));
    }

    /// [`emit`](Self::emit) for a payload that is already a JSON value.
    pub fn emit_value(&self, kind: &str, mut v: Value) {
        let Some(ch) = self.channel.lock().unwrap().clone() else {
            return;
        };
        if let Value::Object(map) = &mut v {
            map.insert("type".into(), Value::String(kind.into()));
        } else {
            v = serde_json::json!({ "type": kind, "value": v });
        }
        let _ = ch.send(v);
    }
}
