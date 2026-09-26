//! From an `s3://` endpoint to the HTTP origin and signing region.
//!
//! The URI names the service host only (`s3://key@s3.eu-west-1.amazonaws.com`,
//! `s3://minio.lan:9000`), so two things have to be inferred:
//!
//! - **http or https.** https, except for a MinIO-style dev server: plain
//!   http is used when the host is loopback (`localhost`, `127.x`, `::1`) or
//!   a private address (`10/8`, `172.16/12`, `192.168/16`, `169.254/16`,
//!   IPv6 `fc00::/7` and `fe80::/10`) *and* an explicit port other than 443
//!   is given. Anything reachable by name, or on 443, gets TLS.
//! - **the region.** Signatures are scoped to a region, and AWS rejects the
//!   wrong one. It is read from well-known hosts (`s3.<region>.amazonaws.com`,
//!   `s3-<region>.amazonaws.com`, `s3.dualstack.<region>.amazonaws.com`,
//!   `s3.<region>.wasabisys.com`, `s3.<region>.backblazeb2.com`); Cloudflare
//!   R2 wants `auto`; everything else (MinIO and friends accept anything)
//!   gets `us-east-1`. Buckets that live elsewhere are corrected per bucket
//!   from the server's `x-amz-bucket-region` answer (see the `client` module).

use cx_core::Endpoint;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub(crate) const DEFAULT_REGION: &str = "us-east-1";

/// Where requests go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    /// `http(s)://host[:port]`, no trailing slash.
    pub origin: String,
    pub region: String,
}

pub(crate) fn target(ep: &Endpoint) -> Target {
    let scheme = if plain_http(&ep.host, ep.port) { "http" } else { "https" };
    let host = if ep.host.contains(':') { format!("[{}]", ep.host) } else { ep.host.clone() };
    let default_port = if scheme == "https" { 443 } else { 80 };
    let origin = match ep.port {
        Some(p) if p != default_port => format!("{scheme}://{host}:{p}"),
        _ => format!("{scheme}://{host}"),
    };
    Target { origin, region: region_for_host(&ep.host) }
}

/// See the module docs for the rule.
pub(crate) fn plain_http(host: &str, port: Option<u16>) -> bool {
    matches!(port, Some(p) if p != 443) && is_local(host)
}

fn is_local(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => is_local_v4(ip),
        Ok(IpAddr::V6(ip)) => ip.to_ipv4_mapped().is_some_and(is_local_v4) || is_local_v6(ip),
        Err(_) => false,
    }
}

fn is_local_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private() || ip.is_link_local()
}

fn is_local_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
}

/// Is this AWS itself (where a bucket in another region also needs another
/// host, not just another signature)?
pub(crate) fn is_aws(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h.ends_with(".amazonaws.com") || h.ends_with(".amazonaws.com.cn")
}

pub(crate) fn region_for_host(host: &str) -> String {
    let h = host.to_ascii_lowercase();
    if h.ends_with(".r2.cloudflarestorage.com") {
        return "auto".into();
    }
    let labels: Vec<&str> = h.split('.').collect();
    let known = |suffix: &[&str]| labels.len() > suffix.len() && labels.ends_with(suffix);
    if known(&["amazonaws", "com"]) || known(&["amazonaws", "com", "cn"]) {
        // s3.<region>.amazonaws.com, s3.dualstack.<region>…, s3-<region>…,
        // and the bucket-less forms of access points and FIPS endpoints.
        for (i, l) in labels.iter().enumerate() {
            if let Some(r) = l.strip_prefix("s3-").filter(|r| looks_like_region(r)) {
                return r.into();
            }
            if (l.starts_with("s3") || *l == "dualstack") && i + 1 < labels.len() && looks_like_region(labels[i + 1]) {
                return labels[i + 1].into();
            }
        }
        return DEFAULT_REGION.into();
    }
    if (known(&["wasabisys", "com"]) || known(&["backblazeb2", "com"])) && labels[0] == "s3" && labels.len() == 4 {
        return labels[1].into();
    }
    DEFAULT_REGION.into()
}

/// `eu-west-1`, `us-gov-west-1`, `ap-southeast-2`, `cn-north-1`…
fn looks_like_region(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() >= 3 && parts.last().is_some_and(|p| p.chars().all(|c| c.is_ascii_digit())) && parts[0].len() == 2 && parts[0].chars().all(|c| c.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cx_core::Scheme;

    fn ep(host: &str, port: Option<u16>) -> Endpoint {
        Endpoint { scheme: Scheme::S3, user: None, host: host.into(), port }
    }

    #[test]
    fn http_only_for_local_hosts_with_explicit_port() {
        assert_eq!(target(&ep("127.0.0.1", Some(9900))).origin, "http://127.0.0.1:9900");
        assert_eq!(target(&ep("localhost", Some(9000))).origin, "http://localhost:9000");
        assert_eq!(target(&ep("192.168.1.20", Some(9000))).origin, "http://192.168.1.20:9000");
        assert_eq!(target(&ep("::1", Some(9000))).origin, "http://[::1]:9000");
        assert_eq!(target(&ep("192.168.1.20", Some(80))).origin, "http://192.168.1.20");
        assert_eq!(target(&ep("127.0.0.1", None)).origin, "https://127.0.0.1");
        assert_eq!(target(&ep("127.0.0.1", Some(443))).origin, "https://127.0.0.1");
        assert_eq!(target(&ep("minio.example.com", Some(9000))).origin, "https://minio.example.com:9000");
        assert_eq!(target(&ep("8.8.8.8", Some(9000))).origin, "https://8.8.8.8:9000");
        assert_eq!(target(&ep("s3.amazonaws.com", None)).origin, "https://s3.amazonaws.com");
    }

    #[test]
    fn regions() {
        assert_eq!(region_for_host("s3.amazonaws.com"), "us-east-1");
        assert_eq!(region_for_host("s3.eu-west-1.amazonaws.com"), "eu-west-1");
        assert_eq!(region_for_host("s3-ap-southeast-2.amazonaws.com"), "ap-southeast-2");
        assert_eq!(region_for_host("s3.dualstack.us-west-2.amazonaws.com"), "us-west-2");
        assert_eq!(region_for_host("s3-fips.us-gov-west-1.amazonaws.com"), "us-gov-west-1");
        assert_eq!(region_for_host("s3.cn-north-1.amazonaws.com.cn"), "cn-north-1");
        assert_eq!(region_for_host("abc123.r2.cloudflarestorage.com"), "auto");
        assert_eq!(region_for_host("s3.eu-central-1.wasabisys.com"), "eu-central-1");
        assert_eq!(region_for_host("s3.us-west-004.backblazeb2.com"), "us-west-004");
        assert_eq!(region_for_host("127.0.0.1"), "us-east-1");
        assert_eq!(region_for_host("minio.lan"), "us-east-1");
    }
}
