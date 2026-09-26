//! AWS Signature Version 4, the request signing every S3-compatible service
//! accepts. It is a fixed recipe of SHA-256 and HMAC over a canonical form of
//! the request, so it is implemented here directly rather than through an
//! SDK (see the crate docs for why).
//!
//! Bodies are either hashed (small XML and uploads we hold in memory) or sent
//! as `UNSIGNED-PAYLOAD`; see the `client` module.

use crate::util::{hex, sha256_hex};
use hmac::{KeyInit, Mac};

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

/// An access key pair.
#[derive(Clone)]
pub(crate) struct Keys {
    pub access: String,
    pub secret: String,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys").field("access", &self.access).finish_non_exhaustive()
    }
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// What goes into a signature. `headers` are the headers to sign, with
/// lowercase names; they must include `host`, `x-amz-date` and
/// `x-amz-content-sha256` and be sent exactly as given.
pub(crate) struct Signable<'a> {
    pub method: &'a str,
    /// The encoded URL path (`/bucket/key%20name`).
    pub path: &'a str,
    /// `(name, value)` pairs, not yet encoded.
    pub query: &'a [(String, String)],
    pub headers: &'a [(String, String)],
    pub payload_hash: &'a str,
    /// `YYYYMMDDTHHMMSSZ`, as in `x-amz-date`.
    pub amz_date: &'a str,
    pub region: &'a str,
}

/// The canonical query string: pairs encoded, then sorted.
pub(crate) fn canonical_query(query: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query.iter().map(|(k, v)| (crate::util::encode(k), crate::util::encode(v))).collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// The `Authorization` header value.
pub(crate) fn authorization(keys: &Keys, s: &Signable<'_>) -> String {
    let mut headers: Vec<(String, String)> = s.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string())).collect();
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    let canonical = format!("{}\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{}", s.method, s.path, canonical_query(s.query), s.payload_hash);

    let date = &s.amz_date[..8];
    let scope = format!("{date}/{}/s3/aws4_request", s.region);
    let to_sign = format!("AWS4-HMAC-SHA256\n{}\n{scope}\n{}", s.amz_date, sha256_hex(canonical.as_bytes()));

    let k_date = hmac(format!("AWS4{}", keys.secret).as_bytes(), date);
    let k_region = hmac(&k_date, s.region);
    let k_service = hmac(&k_region, "s3");
    let k_signing = hmac(&k_service, "aws4_request");
    let signature = hex(&hmac(&k_signing, &to_sign));
    format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}", keys.access)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn keys() -> Keys {
        Keys { access: "AKIAIOSFODNN7EXAMPLE".into(), secret: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into() }
    }

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// "Example: GET Object" from the AWS SigV4 documentation for S3.
    #[test]
    fn aws_get_object_example() {
        let headers = h(&[
            ("Host", "examplebucket.s3.amazonaws.com"),
            ("Range", "bytes=0-9"),
            ("x-amz-content-sha256", EMPTY),
            ("x-amz-date", "20130524T000000Z"),
        ]);
        let s = Signable { method: "GET", path: "/test.txt", query: &[], headers: &headers, payload_hash: EMPTY, amz_date: "20130524T000000Z", region: "us-east-1" };
        let auth = authorization(&keys(), &s);
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    /// "Example: GET Bucket (List Objects)" from the same page: exercises the
    /// query string.
    #[test]
    fn aws_list_objects_example() {
        let headers = h(&[("host", "examplebucket.s3.amazonaws.com"), ("x-amz-content-sha256", EMPTY), ("x-amz-date", "20130524T000000Z")]);
        let query = h(&[("prefix", "J"), ("max-keys", "2")]);
        let s = Signable { method: "GET", path: "/", query: &query, headers: &headers, payload_hash: EMPTY, amz_date: "20130524T000000Z", region: "us-east-1" };
        assert!(authorization(&keys(), &s).ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"));
    }
}
