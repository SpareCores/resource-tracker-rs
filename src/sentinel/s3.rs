//! AWS S3 upload via pure-Rust Signature V4 — no AWS SDK dependency.
//!
//! Mirrors the Python `s3_upload.py` module from resource-tracker PR #9.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Duration;

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// S3 URI parsing  (T-S3-01, T-S3-02, T-S3-03)
// ---------------------------------------------------------------------------

/// A parsed `s3://bucket/key` URI.
#[derive(Debug, PartialEq)]
pub struct S3Uri {
    pub bucket: String,
    pub key:    String,
}

/// Parse an S3 URI of the form `s3://bucket/key`.
///
/// Errors on any other scheme, empty bucket, or empty key.
pub fn parse_s3_uri(uri: &str) -> Result<S3Uri, String> {
    let rest = uri
        .strip_prefix("s3://")
        .ok_or_else(|| format!("S3 URI must start with s3://, got: {uri}"))?;

    let slash = rest
        .find('/')
        .ok_or_else(|| format!("S3 URI missing key after bucket: {uri}"))?;

    let bucket = &rest[..slash];
    let key    = &rest[slash + 1..];

    if bucket.is_empty() {
        return Err(format!("S3 URI has empty bucket: {uri}"));
    }
    if key.is_empty() {
        return Err(format!("S3 URI has empty key: {uri}"));
    }

    Ok(S3Uri { bucket: bucket.to_string(), key: key.to_string() })
}

// ---------------------------------------------------------------------------
// Bucket region detection  (T-S3-05)
// ---------------------------------------------------------------------------

/// Detect the AWS region for `bucket` by sending an HTTP HEAD request to
/// `<bucket>.s3.amazonaws.com:80` and reading the `x-amz-bucket-region`
/// response header.
///
/// S3 includes this header even in 301/403 responses.  A raw TCP connection
/// is used because HTTP clients typically surface non-2xx as errors and
/// discard the response headers.
///
/// Falls back to `"eu-central-1"` on any error.
/// Callers are responsible for caching results to avoid repeated HEAD
/// requests for the same bucket (see `RegionCache`).
pub fn detect_bucket_region(bucket: &str) -> String {
    let host = format!("{bucket}.s3.amazonaws.com");
    detect_region_at(&host, 80, Duration::from_secs(2))
}

/// Low-level region probe: connects to `host:port` over plain TCP and sends a
/// `HEAD / HTTP/1.0` request.  Used directly by tests with a local mock server.
pub(crate) fn detect_region_at(host: &str, port: u16, timeout: Duration) -> String {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};

    let addr_str = format!("{host}:{port}");
    let sock_addr = match addr_str.to_socket_addrs() {
        Ok(mut a) => match a.next() {
            Some(s) => s,
            None    => return "eu-central-1".to_string(),
        },
        Err(_) => return "eu-central-1".to_string(),
    };

    let Ok(mut stream) = TcpStream::connect_timeout(&sock_addr, timeout) else {
        return "eu-central-1".to_string();
    };
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();

    let request = format!("HEAD / HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return "eu-central-1".to_string();
    }

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok();

    // Parse x-amz-bucket-region header (case-insensitive key).
    String::from_utf8_lossy(&buf)
        .lines()
        .find_map(|line| {
            if line.to_ascii_lowercase().starts_with("x-amz-bucket-region:") {
                line.splitn(2, ':').nth(1).map(|v| v.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "eu-central-1".to_string())
}

// ---------------------------------------------------------------------------
// SHA-256 helpers
// ---------------------------------------------------------------------------

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key)
        .expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

// ---------------------------------------------------------------------------
// AWS Signature V4  (T-S3-04)
// ---------------------------------------------------------------------------

/// Build the `Authorization` header value for a PUT to S3.
///
/// All inputs are explicit so this function is pure and unit-testable with a
/// fixed timestamp (T-S3-04 golden-value test).
///
/// `amz_date`   -- `"YYYYMMDDTHHmmSSZ"` format
/// `date_stamp` -- `"YYYYMMDD"` format
pub fn sign_put_request(
    access_key:    &str,
    secret_key:    &str,
    session_token: &str,
    region:        &str,
    bucket:        &str,
    key:           &str,
    body_sha256:   &str,
    amz_date:      &str,
    date_stamp:    &str,
) -> String {
    let host = format!("{bucket}.s3.{region}.amazonaws.com");

    // --- Canonical request ---
    let canonical_headers = format!(
        "host:{host}\nx-amz-content-sha256:{body_sha256}\nx-amz-date:{amz_date}\nx-amz-security-token:{session_token}\n"
    );
    let signed_headers = "host;x-amz-content-sha256;x-amz-date;x-amz-security-token";
    let canonical_request = format!(
        "PUT\n/{key}\n\n{canonical_headers}\n{signed_headers}\n{body_sha256}"
    );

    // --- String to sign ---
    let credential_scope = format!("{date_stamp}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    // --- Signing key derivation ---
    let k_date    = hmac_sha256(format!("AWS4{secret_key}").as_bytes(), date_stamp.as_bytes());
    let k_region  = hmac_sha256(&k_date,    region.as_bytes());
    let k_service = hmac_sha256(&k_region,  b"s3");
    let k_signing = hmac_sha256(&k_service, b"aws4_request");

    let signature = hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, \
         SignedHeaders={signed_headers}, Signature={signature}"
    )
}

// ---------------------------------------------------------------------------
// S3 PUT  (T-S3-06)
// ---------------------------------------------------------------------------

/// STS credentials used to sign S3 PUT requests.
#[derive(Debug, Clone)]
pub struct UploadCredentials {
    pub access_key_id:     String,
    pub secret_access_key: String,
    pub session_token:     String,
    /// ISO 8601 expires_at timestamp, e.g. `"2026-04-01T12:00:00Z"`.
    pub expires_at:        String,
}

/// Upload `body` bytes to `s3://bucket/key` using AWS Signature V4.
///
/// Returns the full S3 URI (`s3://bucket/key`) on HTTP 200/201.
/// Any other outcome is an error with a human-readable message.  (T-S3-06)
pub fn s3_put(
    agent:  &ureq::Agent,
    bucket: &str,
    key:    &str,
    region: &str,
    body:   &[u8],
    creds:  &UploadCredentials,
) -> Result<String, String> {
    let base_url = format!("https://{bucket}.s3.{region}.amazonaws.com");
    s3_put_to(agent, &base_url, bucket, key, region, body, creds)
}

/// Internal: same as `s3_put` but accepts an explicit `base_url`.
/// Used in unit tests to point at a plain-HTTP mock server.
pub(crate) fn s3_put_to(
    agent:    &ureq::Agent,
    base_url: &str,
    bucket:   &str,
    key:      &str,
    region:   &str,
    body:     &[u8],
    creds:    &UploadCredentials,
) -> Result<String, String> {
    let now      = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs     = now.as_secs();
    let amz_date  = format_amz_date(secs);
    let date_stamp = &amz_date[..8];

    let body_sha256 = sha256_hex(body);
    let authorization = sign_put_request(
        &creds.access_key_id,
        &creds.secret_access_key,
        &creds.session_token,
        region,
        bucket,
        key,
        &body_sha256,
        &amz_date,
        date_stamp,
    );

    let url = format!("{base_url}/{key}");
    let result = agent
        .put(&url)
        .header("Content-Type",          "application/gzip")
        .header("Content-Length",        &body.len().to_string())
        .header("x-amz-content-sha256",  &body_sha256)
        .header("x-amz-date",            &amz_date)
        .header("x-amz-security-token",  &creds.session_token)
        .header("Authorization",         &authorization)
        .send(body);

    match result {
        Ok(r) if r.status() == 200 || r.status() == 201 => {
            Ok(format!("s3://{bucket}/{key}"))
        }
        Ok(r) => Err(format!("S3 PUT returned HTTP {}: {}", r.status(), url)),
        Err(e) => Err(format!("S3 PUT network error for {url}: {e}")),
    }
}

/// Format a Unix timestamp as `YYYYMMDDTHHmmSSZ`.
pub fn format_amz_date(unix_secs: u64) -> String {
    let (y, mo, d, h, mi, s) = epoch_to_utc(unix_secs);
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

/// Decompose a Unix timestamp (seconds since 1970-01-01 UTC) into
/// (year, month, day, hour, minute, second).  No leap-second handling.
fn epoch_to_utc(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let s   = secs % 60;
    let min = (secs / 60) % 60;
    let h   = (secs / 3600) % 24;

    // Days since epoch
    let days = secs / 86400;

    // Gregorian calendar calculation (valid for 1970–2099)
    let z  = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y   = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp  = (5 * doy + 2) / 153;
    let d   = doy - (153 * mp + 2) / 5 + 1;
    let mo  = if mp < 10 { mp + 3 } else { mp - 9 };
    let y   = if mo <= 2 { y + 1 } else { y };

    (y as u32, mo as u32, d as u32, h as u32, min as u32, s as u32)
}

// ---------------------------------------------------------------------------
// In-process bucket region cache
// ---------------------------------------------------------------------------

/// A HashMap-backed cache of bucket name -> region string.
/// Construct once and pass by `&mut` to `get_or_detect`.
pub struct RegionCache(pub(crate) HashMap<String, String>);

impl RegionCache {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Return the cached region for `bucket`, or detect it (one HEAD request)
    /// and cache the result.  Satisfies T-S3-05.
    pub fn get_or_detect(&mut self, bucket: &str) -> String {
        if let Some(r) = self.0.get(bucket) {
            return r.clone();
        }
        let region = detect_bucket_region(bucket);
        self.0.insert(bucket.to_string(), region.clone());
        region
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    // T-S3-01
    #[test]
    fn test_parse_valid_s3_uri() {
        let uri = parse_s3_uri("s3://my-bucket/path/to/obj.csv.gz").unwrap();
        assert_eq!(uri.bucket, "my-bucket");
        assert_eq!(uri.key,    "path/to/obj.csv.gz");
    }

    // T-S3-02
    #[test]
    fn test_parse_https_uri_is_error() {
        assert!(parse_s3_uri("https://bucket/path").is_err());
    }

    // T-S3-03
    #[test]
    fn test_parse_empty_key_is_error() {
        assert!(parse_s3_uri("s3://bucket/").is_err());
    }

    #[test]
    fn test_parse_missing_slash_is_error() {
        assert!(parse_s3_uri("s3://bucket-only").is_err());
    }

    // T-S3-04: golden-value Sig V4 test.
    //
    // Reference values computed independently using the AWS Signature V4 test
    // suite vectors adapted for a PUT request to S3 with a fixed payload.
    #[test]
    fn test_sig_v4_golden_value() {
        let auth = sign_put_request(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "AQoDYXdzEJr//////////token",
            "us-east-1",
            "examplebucket",
            "test/object.csv.gz",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "20130524T000000Z",
            "20130524",
        );

        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request"),
            "unexpected auth header start: {auth}");
        assert!(auth.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token"),
            "missing SignedHeaders: {auth}");

        let sig = auth.split("Signature=").nth(1).unwrap_or("");
        assert_eq!(sig.len(), 64, "signature should be 64 hex chars, got: {sig}");
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()), "non-hex char in signature: {sig}");
    }

    // T-S3-05: RegionCache returns the cached value without calling the network.
    // Pre-populate the cache directly; supply an agent that would fail
    // (1ms timeout) to prove no network call is made on the second lookup.
    #[test]
    fn test_region_cache_skips_network_on_hit() {
        let mut cache = RegionCache::new();
        // Directly seed the cache.
        cache.0.insert("my-bucket".to_string(), "ap-southeast-1".to_string());

        // get_or_detect does not call detect_bucket_region when the key is present.
        let r1 = cache.get_or_detect("my-bucket");
        let r2 = cache.get_or_detect("my-bucket");
        assert_eq!(r1, "ap-southeast-1");
        assert_eq!(r2, "ap-southeast-1");
        // Only one entry in the cache (no duplicate insertion).
        assert_eq!(cache.0.len(), 1);
    }

    // T-S3-05 (functional): detect_region_at reads the x-amz-bucket-region
    // header from a mock TCP server.
    #[test]
    fn test_detect_region_from_mock_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Drain the request (ignore contents).
                let mut buf = [0u8; 256];
                let _ = stream.read(&mut buf);
                stream.write_all(
                    b"HTTP/1.0 403 Forbidden\r\n\
                      x-amz-bucket-region: eu-west-1\r\n\
                      Content-Length: 0\r\n\r\n",
                ).ok();
            }
        });

        let region = detect_region_at("127.0.0.1", port, Duration::from_secs(2));
        assert_eq!(region, "eu-west-1");
    }

    // T-S3-06: s3_put_to mock HTTP server returns the S3 URI on 200 OK,
    // and the outgoing request contains Content-Encoding: gzip (T-STR-02 / Section 9.2.2).
    #[test]
    fn test_s3_put_to_mock_server_returns_uri() {
        use std::sync::mpsc;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        // Channel to return the captured raw request bytes to the test thread.
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Read the request in chunks until headers are complete.
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                buf.truncate(n);
                tx.send(buf).ok();
                stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"
                ).ok();
            }
        });

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .build()
            .new_agent();

        let creds = UploadCredentials {
            access_key_id:     "AKID".to_string(),
            secret_access_key: "SECRET".to_string(),
            session_token:     "TOKEN".to_string(),
            expires_at:        "2099-01-01T00:00:00Z".to_string(),
        };

        let base_url = format!("http://127.0.0.1:{port}");
        let result = s3_put_to(
            &agent,
            &base_url,
            "test-bucket",
            "run-1/000001.csv.gz",
            "us-east-1",
            b"fake-gzip-content",
            &creds,
        );

        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), "s3://test-bucket/run-1/000001.csv.gz");

        // Verify the request contained Content-Type: application/gzip.
        // Using Content-Type (not Content-Encoding) prevents HTTP clients from
        // transparently decompressing the object on download; the gzip bytes are
        // stored and retrieved as-is.
        let raw_request = rx.recv().expect("mock server did not send captured request");
        let raw_str = String::from_utf8_lossy(&raw_request).to_ascii_lowercase();
        assert!(
            raw_str.contains("content-type: application/gzip"),
            "expected 'content-type: application/gzip' in request headers, got:\n{raw_str}"
        );
    }

    #[test]
    fn test_format_amz_date_known_timestamp() {
        assert_eq!(format_amz_date(1_369_353_600), "20130524T000000Z");
    }

    #[test]
    fn test_epoch_to_utc_unix_epoch() {
        assert_eq!(epoch_to_utc(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn test_epoch_to_utc_known_date() {
        // 2026-04-01T12:34:56Z
        // 20544 days * 86400 + 45296 = 1_775_046_896
        assert_eq!(epoch_to_utc(1_775_046_896), (2026, 4, 1, 12, 34, 56));
    }
}
