//! OAuth tokens and how they ride in [`Credentials`].
//!
//! The app already keeps per-endpoint credentials in the OS keychain through
//! its `CredentialStore`, so tokens are stored the same way instead of in a
//! file of our own: the user is the account (e-mail) and the "password" is
//! the tokens as JSON. The client id that obtained them is kept alongside,
//! because a refresh token only works with the client it was issued to.

use crate::service::Service;
use crate::util::now_ms;
use cx_core::{Credentials, Secret};
use serde::{Deserialize, Serialize};

/// Refresh this long before the access token actually expires, so a request
/// never goes out with a token that dies in flight.
pub(crate) const EXPIRY_MARGIN_MS: i64 = 60_000;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub service: Service,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Milliseconds since the epoch; `None` when the server gave no lifetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// The OAuth client the tokens were issued to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

// Tokens are secrets: keep them out of logs and panic messages.
impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("service", &self.service)
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "…"))
            .field("expires_at", &self.expires_at)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl Tokens {
    /// Is the access token expired, or about to be?
    pub fn expired(&self) -> bool {
        self.expires_at.is_some_and(|t| t - EXPIRY_MARGIN_MS <= now_ms())
    }

    /// Pack into credentials for the credential store.
    pub fn to_credentials(&self, account: &str) -> Credentials {
        let json = serde_json::to_string(self).expect("tokens serialize");
        Credentials { user: account.to_string(), secret: Secret::Password { password: json } }
    }

    /// Unpack credentials made by [`Tokens::to_credentials`]. `None` for
    /// anything else (for example a password typed into a generic dialog).
    pub fn from_credentials(creds: &Credentials) -> Option<Tokens> {
        match &creds.secret {
            Secret::Password { password } => serde_json::from_str(password).ok(),
            _ => None,
        }
    }

    /// Fold a token-endpoint answer (code exchange or refresh) into tokens.
    /// Refresh answers often omit the refresh token (Google) or rotate it
    /// (Microsoft), so an absent one keeps the old.
    pub(crate) fn from_response(service: Service, client_id: &str, v: &serde_json::Value, previous_refresh: Option<String>) -> Option<Tokens> {
        let access_token = v.get("access_token")?.as_str()?.to_string();
        let refresh_token = v.get("refresh_token").and_then(|r| r.as_str()).map(str::to_owned).or(previous_refresh);
        let expires_in = v.get("expires_in").and_then(|e| e.as_i64().or_else(|| e.as_str().and_then(|s| s.parse().ok())));
        Some(Tokens { service, access_token, refresh_token, expires_at: expires_in.map(|s| now_ms() + s * 1000), client_id: Some(client_id.to_string()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_round_trip_and_expiry() {
        let t = Tokens { service: Service::Dropbox, access_token: "a".into(), refresh_token: Some("r".into()), expires_at: Some(now_ms() + 3_600_000), client_id: Some("c".into()) };
        let c = t.to_credentials("me@example.com");
        assert_eq!(c.user, "me@example.com");
        assert_eq!(Tokens::from_credentials(&c), Some(t.clone()));
        assert!(!t.expired());
        assert!(Tokens { expires_at: Some(now_ms() + 10_000), ..t.clone() }.expired());
        assert!(Tokens::from_credentials(&Credentials::password("u", "hunter2")).is_none());
        assert!(!format!("{t:?}").contains("\"a\""));
    }

    #[test]
    fn refresh_keeps_the_old_refresh_token() {
        let v = serde_json::json!({"access_token": "new", "expires_in": 3599, "token_type": "Bearer"});
        let t = Tokens::from_response(Service::GDrive, "cid", &v, Some("old-refresh".into())).unwrap();
        assert_eq!(t.refresh_token.as_deref(), Some("old-refresh"));
        assert!(t.expires_at.unwrap() > now_ms());
    }
}
