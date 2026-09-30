use base64::Engine as _;
use serde::ser::{Serialize, SerializeStruct, Serializer};

/// What a session reports to its listener.
///
/// Serialized (for a Tauri `Channel<TermEvent>`) as
///
/// ```json
/// {"kind":"output","data":"G1szMm1oaQ=="}   // base64 of the raw PTY bytes
/// {"kind":"exit","code":0}                   // code is null when killed by a signal
/// {"kind":"authenticated","method":"password","copyId":true}
/// ```
///
/// Why base64: Tauri channels carry serde values as JSON, where a `Vec<u8>`
/// becomes an array of numbers (up to 4 bytes of JSON per byte plus parse
/// cost). Base64 is 1.33× and decodes natively in the webview:
/// `Uint8Array.fromBase64(data)` (or `Uint8Array.from(atob(data), c =>
/// c.charCodeAt(0))`), then `xterm.write(bytes)`. The bytes are raw terminal
/// output: they may split a UTF-8 sequence or an escape sequence across
/// events, which xterm.js (and a streaming `TextDecoder`) handle.
///
/// An app that wants zero-copy can match on the enum itself and send
/// `Output` bytes as `tauri::ipc::InvokeResponseBody::Raw` instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    /// A coalesced chunk of terminal output.
    Output(Vec<u8>),
    /// The shell (or ssh) exited; always the last event of a session.
    /// `code` is `None` when the process was killed by a signal (e.g. by
    /// [`Terminals::close`](crate::Terminals::close)) or its status is unknown.
    Exit { code: Option<i32> },
    /// `ssh` finished authenticating. `method` is OpenSSH's name
    /// (`publickey`, `password`, `keyboard-interactive`, `other`). `copy_id`
    /// is true when the user typed a password and this session has a control
    /// socket, so a key can be installed without asking again.
    Authenticated { method: String, copy_id: bool },
}

impl Serialize for TermEvent {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("TermEvent", 3)?;
        match self {
            TermEvent::Output(bytes) => {
                st.serialize_field("kind", "output")?;
                st.serialize_field(
                    "data",
                    &base64::engine::general_purpose::STANDARD.encode(bytes),
                )?;
            }
            TermEvent::Exit { code } => {
                st.serialize_field("kind", "exit")?;
                st.serialize_field("code", code)?;
            }
            TermEvent::Authenticated { method, copy_id } => {
                st.serialize_field("kind", "authenticated")?;
                st.serialize_field("method", method)?;
                st.serialize_field("copyId", copy_id)?;
            }
        }
        st.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape() {
        let out = serde_json::to_string(&TermEvent::Output(b"hi\x1b[0m".to_vec())).unwrap();
        assert_eq!(out, r#"{"kind":"output","data":"aGkbWzBt"}"#);
        assert_eq!(
            serde_json::to_string(&TermEvent::Exit { code: Some(3) }).unwrap(),
            r#"{"kind":"exit","code":3}"#
        );
        assert_eq!(
            serde_json::to_string(&TermEvent::Exit { code: None }).unwrap(),
            r#"{"kind":"exit","code":null}"#
        );
        assert_eq!(
            serde_json::to_string(&TermEvent::Authenticated {
                method: "password".into(),
                copy_id: true
            })
            .unwrap(),
            r#"{"kind":"authenticated","method":"password","copyId":true}"#
        );
    }
}
