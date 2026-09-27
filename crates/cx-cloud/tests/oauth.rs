//! OAuth: sign-in URL construction and the loopback callback, with a
//! simulated browser hitting the callback and the mocks' token endpoint
//! checking the PKCE verifier.

mod common;

use common::dropbox_mock::DropboxMock;
use common::gdrive_mock::DriveMock;
use common::onedrive_mock::GraphMock;
use common::*;
use cx_cloud::{ClientConfig, Service, DROPBOX_REDIRECT_PORT};
use cx_core::{CxError, Scheme};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

fn params(url: &str) -> HashMap<String, String> {
    url::Url::parse(url).unwrap().query_pairs().into_owned().collect()
}

async fn mock_for(service: Service) -> Mock {
    let auth = Arc::new(Auth::default());
    match service {
        Service::GDrive => serve(Arc::new(DriveMock::default()), auth).await,
        Service::Dropbox => serve(Arc::new(DropboxMock::new(10)), auth).await,
        Service::OneDrive => serve(Arc::new(GraphMock::new(10)), auth).await,
    }
}

#[test]
fn client_config_defaults() {
    let d = ClientConfig::new(Service::Dropbox, "key");
    assert_eq!(d.redirect_port, Some(DROPBOX_REDIRECT_PORT), "Dropbox needs a registered, fixed port");
    assert_eq!(ClientConfig::new(Service::GDrive, "id").redirect_port, None);
    assert!(d.urls.api.starts_with("https://api.dropboxapi.com/"));
    assert_eq!(ClientConfig::new(Service::GDrive, "id").with_secret("").client_secret, None);
}

#[tokio::test]
async fn authorization_urls_carry_pkce_and_service_parameters() {
    for service in Service::ALL {
        let cfg = ClientConfig::new(service, "my-client").with_redirect_port(None);
        let req = cx_cloud::start_authorization(service, &cfg).await.unwrap();
        let p = params(&req.url);
        assert!(req.url.starts_with(&cfg.urls.authorize), "{}", req.url);
        assert_eq!(p["response_type"], "code");
        assert_eq!(p["client_id"], "my-client");
        assert_eq!(p["code_challenge_method"], "S256");
        assert_eq!(p["code_challenge"].len(), 43, "base64url SHA-256 without padding");
        assert_eq!(p["state"], req.state);
        assert!(p["redirect_uri"].starts_with("http://127.0.0.1:") && p["redirect_uri"].ends_with("/callback"), "{}", p["redirect_uri"]);
        assert_eq!(p["redirect_uri"], req.redirect_uri);
        let port: u16 = req.redirect_uri.trim_start_matches("http://127.0.0.1:").trim_end_matches("/callback").parse().unwrap();
        assert_ne!(port, 0);
        match service {
            Service::GDrive => {
                assert_eq!(p["scope"], "https://www.googleapis.com/auth/drive");
                assert_eq!((p["access_type"].as_str(), p["prompt"].as_str()), ("offline", "consent"));
            }
            Service::Dropbox => {
                assert_eq!(p["token_access_type"], "offline");
                assert!(!p.contains_key("scope"));
            }
            Service::OneDrive => {
                assert!(p["scope"].split(' ').any(|s| s == "offline_access"));
                assert!(p["scope"].contains("Files.ReadWrite.All"));
            }
        }
        // Two sign-ins never share state or verifier.
        let again = cx_cloud::start_authorization(service, &cfg).await.unwrap();
        assert_ne!(again.state, req.state);
        assert_ne!(params(&again.url)["code_challenge"], p["code_challenge"]);
    }
    assert!(cx_cloud::start_authorization_with_redirect(Service::GDrive, &ClientConfig::new(Service::GDrive, " "), "x:/cb").is_err(), "no client id");
}

/// Sign in through the loopback listener, as a browser would.
async fn sign_in(service: Service) -> (Mock, cx_cloud::SignedIn) {
    let mock = mock_for(service).await;
    let cfg = config(service, &mock).with_redirect_port(None);
    let req = cx_cloud::start_authorization(service, &cfg).await.unwrap();
    let p = params(&req.url);
    mock.auth.codes.lock().unwrap().insert("the-code".into(), (p["code_challenge"].clone(), req.redirect_uri.clone()));
    let (redirect, state) = (req.redirect_uri.clone(), req.state.clone());
    let waiting = tokio::spawn(req.wait(Duration::from_secs(10)));

    let base = redirect.trim_end_matches("/callback").to_string();
    let favicon = reqwest::get(format!("{base}/favicon.ico")).await.unwrap();
    assert_eq!(favicon.status(), 404, "stray requests don't end the wait");
    let forged = reqwest::get(format!("{redirect}?code=evil&state=not-it")).await.unwrap();
    assert_eq!(forged.status(), 400, "a wrong state is refused and the wait goes on");
    let ok = reqwest::get(format!("{redirect}?code=the-code&state={state}&scope=x")).await.unwrap();
    assert_eq!(ok.status(), 200);
    assert!(ok.text().await.unwrap().contains(&format!("Signed in to {}", service.label())));

    let signed = waiting.await.unwrap().unwrap();
    (mock, signed)
}

#[tokio::test]
async fn loopback_sign_in_exchanges_code_with_verifier() {
    for service in Service::ALL {
        let (mock, signed) = sign_in(service).await;
        assert_eq!(signed.account, "me@example.com", "{service:?}");
        assert_eq!(signed.service, service);
        assert_eq!(signed.tokens.refresh_token.as_deref(), Some("fresh-refresh"));
        assert!(signed.tokens.expires_at.is_some());
        assert_eq!(signed.tokens.client_id.as_deref(), Some("test-client"));

        // What the app stores is what connect() takes.
        let ep = signed.endpoint(Scheme::Davs);
        assert_eq!((ep.user.as_deref(), ep.host.as_str()), (Some("me"), "example.com"));
        let creds = signed.credentials();
        assert_eq!(creds.user, "me@example.com");
        let p = cx_cloud::open(service, &ep, Some(creds), Some(&config(service, &mock)), None).await.unwrap().into_provider();
        assert!(p.stat(&cx_core::Location::remote(ep, "/")).await.unwrap().is_dir);
    }
}

#[tokio::test]
async fn declined_sign_in_and_timeouts() {
    let mock = mock_for(Service::GDrive).await;
    let cfg = config(Service::GDrive, &mock).with_redirect_port(None);

    let req = cx_cloud::start_authorization(Service::GDrive, &cfg).await.unwrap();
    let url = format!("{}?error=access_denied&state={}", req.redirect_uri, req.state);
    let waiting = tokio::spawn(req.wait(Duration::from_secs(10)));
    let page = reqwest::get(url).await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(matches!(waiting.await.unwrap(), Err(CxError::Cancelled)));

    let req = cx_cloud::start_authorization(Service::GDrive, &cfg).await.unwrap();
    assert!(matches!(req.wait(Duration::from_millis(50)).await, Err(CxError::Cancelled)));
}

#[tokio::test]
async fn custom_redirect_completes_from_callback_url() {
    let mock = mock_for(Service::OneDrive).await;
    let cfg = config(Service::OneDrive, &mock);
    let req = cx_cloud::start_authorization_with_redirect(Service::OneDrive, &cfg, "crossexplore://oauth/callback").unwrap();
    let p = params(&req.url);
    assert_eq!(p["redirect_uri"], "crossexplore://oauth/callback");
    mock.auth.codes.lock().unwrap().insert("c2".into(), (p["code_challenge"].clone(), req.redirect_uri.clone()));
    // Without a loopback listener there is nothing to wait on.
    let twin = cx_cloud::start_authorization_with_redirect(Service::OneDrive, &cfg, "crossexplore://x").unwrap();
    assert!(matches!(twin.wait(Duration::from_millis(10)).await, Err(CxError::Unsupported(_))));
    let bad = req.complete("crossexplore://oauth/callback?code=c2&state=wrong").await;
    assert!(matches!(bad, Err(CxError::PermissionDenied(_))));
    let signed = req.complete(&format!("crossexplore://oauth/callback?code=c2&state={}", req.state)).await.unwrap();
    assert_eq!(signed.account, "me@example.com");
    // A code can't be used twice (the mock forgets it, as real servers do).
    assert!(req.complete(&format!("crossexplore://oauth/callback?code=c2&state={}", req.state)).await.is_err());
}
