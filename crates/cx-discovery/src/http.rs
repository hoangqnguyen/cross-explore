//! A deliberately tiny HTTP/1.1 client: one request per connection,
//! `Connection: close`, bounded body. Discovery only needs to fetch UPnP
//! description XML, send a WebDAV `OPTIONS`, and talk to Tailscale's
//! LocalAPI socket — not worth a full HTTP stack, and keeping it here makes
//! it obvious that no credentials are ever sent.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Parse a complete (or truncated-at-EOF) response, de-chunking if needed.
pub(crate) fn parse_response(raw: &[u8]) -> Option<Response> {
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..head_end]).ok()?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next()?;
    let mut parts = status_line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    let status = parts.next()?.parse().ok()?;
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let mut body = raw[head_end + 4..].to_vec();
    let chunked = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked"));
    if chunked {
        body = dechunk(&body);
    } else if let Some(len) = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse::<usize>().ok()) {
        body.truncate(len);
    }
    Some(Response { status, headers, body })
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(line_end) = data.windows(2).position(|w| w == b"\r\n") {
        let size_str = std::str::from_utf8(&data[..line_end]).unwrap_or("0");
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        data = &data[line_end + 2..];
        if size == 0 {
            break;
        }
        let take = size.min(data.len());
        out.extend_from_slice(&data[..take]);
        data = data.get(take + 2..).unwrap_or(&[]);
    }
    out
}

/// Send one request on `stream` and read until EOF, `max` bytes, or the
/// deadline — whatever arrived is parsed, so a server that keeps the socket
/// open after a complete `OPTIONS` reply still counts.
pub(crate) async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, method: &str, host: &str, path: &str, max: usize, deadline: Instant) -> io::Result<Response> {
    let req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: CrossExplore-Discovery/1\r\nAccept: */*\r\nConnection: close\r\n\r\n");
    timeout_at(deadline, stream.write_all(req.as_bytes())).await??;
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    loop {
        match tokio::time::timeout_at(deadline, stream.read(&mut chunk)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() >= max || (method == "OPTIONS" && buf.windows(4).any(|w| w == b"\r\n\r\n")) {
                    break;
                }
            }
            Ok(Err(e)) if buf.is_empty() => return Err(e),
            Ok(Err(_)) => break,
        }
    }
    parse_response(&buf).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not an HTTP response"))
}

async fn timeout_at<F: std::future::Future>(deadline: Instant, f: F) -> io::Result<F::Output> {
    tokio::time::timeout_at(deadline, f).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out"))
}

/// `GET http://host[:port]/path` (plain HTTP only; used for UPnP XML).
pub(crate) async fn get(url: &str, limit: Duration, max: usize) -> io::Result<Response> {
    let rest = url.strip_prefix("http://").ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "only http:// is supported"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let host = crate::util::url_host(url).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no host"))?;
    let port = authority.rsplit_once(':').filter(|(h, _)| !h.ends_with(':') && !authority.ends_with(']')).and_then(|(_, p)| p.parse().ok()).unwrap_or(80);
    let deadline = Instant::now() + limit;
    let stream = timeout_at(deadline, TcpStream::connect((host.as_str(), port))).await??;
    exchange(stream, "GET", authority, path, max, deadline).await
}

/// `OPTIONS /` against `ip:port`, over TLS when `tls` is set. Returns the
/// response so the caller can look for a `DAV:` header.
pub(crate) async fn options(ip: IpAddr, port: u16, host: &str, tls: bool, limit: Duration) -> io::Result<Response> {
    let deadline = Instant::now() + limit;
    let tcp = timeout_at(deadline, TcpStream::connect(SocketAddr::new(ip, port))).await??;
    let authority = if port == 80 || port == 443 { host.to_string() } else { format!("{host}:{port}") };
    if !tls {
        return exchange(tcp, "OPTIONS", &authority, "/", 16 * 1024, deadline).await;
    }
    let server_name = match host.parse::<IpAddr>() {
        Ok(ip) => rustls::pki_types::ServerName::IpAddress(ip.into()),
        Err(_) => rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?,
    };
    let connector = tokio_rustls::TlsConnector::from(tls_config());
    let stream = timeout_at(deadline, connector.connect(server_name, tcp)).await??;
    exchange(stream, "OPTIONS", &authority, "/", 16 * 1024, deadline).await
}

/// TLS without certificate verification. NAS boxes almost always serve
/// self-signed certificates, and all we do over this connection is read
/// response headers of an unauthenticated `OPTIONS` — nothing secret is
/// sent, so there is nothing for an impostor to steal. The real WebDAV
/// provider verifies certificates properly when the user connects.
fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = rustls::ClientConfig::builder_with_provider(provider.clone())
                .with_safe_default_protocol_versions()
                .expect("ring supports the default protocol versions")
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider)))
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

#[derive(Debug)]
struct AcceptAnyCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_chunked() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\nDAV: 1, 2\r\nContent-Length: 5\r\n\r\nhello-extra").unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.header("dav"), Some("1, 2"));
        assert_eq!(r.body, b"hello");
        let r = parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n<ro\r\n3;x=y\r\not/\r\n0\r\n\r\n").unwrap();
        assert_eq!(r.body, b"<root/");
        assert!(parse_response(b"SSH-2.0-OpenSSH_9.6\r\n\r\n").is_none());
    }

    #[tokio::test]
    async fn exchange_over_a_duplex_stream() {
        let (client, mut server) = tokio::io::duplex(4096);
        let srv = tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let n = server.read(&mut buf).await.unwrap();
            assert!(std::str::from_utf8(&buf[..n]).unwrap().starts_with("OPTIONS / HTTP/1.1\r\nHost: nas:5005\r\n"));
            server.write_all(b"HTTP/1.1 200 OK\r\nDAV: 1,2\r\n\r\n").await.unwrap();
            // Keep the connection open: OPTIONS must not wait for EOF.
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let r = exchange(client, "OPTIONS", "nas:5005", "/", 4096, Instant::now() + Duration::from_secs(2)).await.unwrap();
        assert_eq!(r.header("DAV"), Some("1,2"));
        srv.abort();
    }
}
