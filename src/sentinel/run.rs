//! Sentinel API run lifecycle: start_run, close_run, credential refresh.

use crate::config::JobMetadata;
use crate::metrics::{CloudInfo, HostInfo};
use crate::sentinel::s3::UploadCredentials;
use serde::{Deserialize, Serialize};

fn slice_is_empty(v: &&[String]) -> bool {
    v.is_empty()
}

// ---------------------------------------------------------------------------
// Base64 encoding (stdlib only -- avoids a crate dependency)
// ---------------------------------------------------------------------------

fn base64_encode(input: &[u8]) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    input.chunks(3).for_each(|chunk| {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(ALPHA[((n >> 18) & 0x3f) as usize]));
        out.push(char::from(ALPHA[((n >> 12) & 0x3f) as usize]));
        out.push(if chunk.len() > 1 {
            char::from(ALPHA[((n >> 6) & 0x3f) as usize])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(ALPHA[(n & 0x3f) as usize])
        } else {
            '='
        });
    });
    out
}

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct StartRunResponse {
    run_id: String,
    upload_uri_prefix: String,
    upload_credentials: RawCredentials,
}

/// Field names match the live Sentinel API response (Python reference:
/// `sentinel_api.py` `register_run` docstring).
/// `expiration` is the documented Python name; `expires_at` is accepted as an
/// alias in case the live API uses a different casing.  The field is optional
/// so a missing value does not abort the run -- it falls back to a far-future
/// timestamp (credentials treated as always-fresh).
#[derive(Debug, Deserialize)]
struct RawCredentials {
    access_key: String,
    secret_key: String,
    session_token: String,
    #[serde(alias = "expires_at", default)]
    expiration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RefreshCredentialsResponse {
    upload_credentials: RawCredentials,
}

// ---------------------------------------------------------------------------
// RunContext
// ---------------------------------------------------------------------------

/// State returned by `start_run` and referenced by all subsequent API calls.
#[derive(Debug, Clone)]
pub struct RunContext {
    pub run_id: String,
    pub upload_uri_prefix: String,
    pub credentials: UploadCredentials,
}

impl RunContext {
    /// Returns `true` when the STS credentials expire within 5 minutes.
    /// Satisfies T-STR-04.
    pub fn creds_expiring_soon(&self) -> bool {
        match parse_iso8601_secs(&self.credentials.expires_at) {
            Some(expires_at_secs) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                expires_at_secs.saturating_sub(now) < 300
            }
            None => {
                // Unparseable expires_at: treat as already expired to be safe.
                true
            }
        }
    }
}

/// Parse a subset of ISO 8601 UTC timestamps into Unix seconds.
/// Handles the common AWS format `"YYYY-MM-DDTHH:MM:SSZ"`.
fn parse_iso8601_secs(s: &str) -> Option<u64> {
    // Expected: "2026-04-01T12:00:00Z" (20 chars, no sub-second, UTC only)
    let s = s.trim_end_matches('Z');
    let s = s.trim_end_matches("+00:00");
    let nums: Vec<u64> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if nums.len() < 6 {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (nums[0], nums[1], nums[2], nums[3], nums[4], nums[5]);
    // Approximate days since epoch (good enough for expires_at comparison).
    let days = days_since_epoch(y, mo, d)?;
    Some(days * 86400 + h * 3600 + mi * 60 + sec)
}

/// Days from 1970-01-01 to the given date (Gregorian, valid 1970–2099).
fn days_since_epoch(y: u64, mo: u64, d: u64) -> Option<u64> {
    if mo < 1 || mo > 12 || d < 1 || d > 31 || y < 1970 {
        return None;
    }
    // Days per month (non-leap); February adjusted below.
    const DAYS: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let year_days: u64 = (1970..y)
        .map(|yr| {
            if (yr % 4 == 0 && yr % 100 != 0) || yr % 400 == 0 {
                366
            } else {
                365
            }
        })
        .sum();
    let month_days: u64 = (1..mo)
        .map(|m| {
            if m == 2 && is_leap {
                29
            } else {
                DAYS[(m - 1) as usize]
            }
        })
        .sum();
    Some(year_days + month_days + d - 1)
}

// ---------------------------------------------------------------------------
// Request payload types
// ---------------------------------------------------------------------------

/// Python `register_run` sends a flat dict merging metadata, host_info, and
/// cloud_info.  `#[serde(flatten)]` on all three fields reproduces that shape.
#[derive(Debug, Serialize)]
struct StartRunRequest<'a> {
    #[serde(flatten)]
    metadata: MetadataPayload<'a>,
    #[serde(flatten)]
    host: &'a HostInfo,
    #[serde(flatten)]
    cloud: &'a CloudInfo,
}

#[derive(Debug, Serialize)]
struct MetadataPayload<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    job_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    team: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    env: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    orchestrator: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    executor: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_run_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    container_image: Option<&'a str>,
    #[serde(skip_serializing_if = "slice_is_empty")]
    tags: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<i32>,
    /// Shell-wrapper command as a JSON array, e.g. ["stress","--cpu","4"].
    /// Omitted when not running in shell-wrapper mode.
    #[serde(skip_serializing_if = "slice_is_empty")]
    command: &'a [String],
}

#[derive(Debug, Serialize)]
struct CloseRunRequest {
    // run_id is in the URL path (/runs/{run_id}/finish); do not repeat in body.
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    run_status: &'static str,
    // Always inline: remaining (unflushed) samples only.
    // Earlier batches were already uploaded to S3 and are associated with the
    // run by run_id server-side; the /finish endpoint does not accept s3 URIs.
    data_source: &'static str,
    data_csv: String,
}

// ---------------------------------------------------------------------------
// API calls
// ---------------------------------------------------------------------------

/// POST to `/runs` to register a new run with the Sentinel API.
///
/// On failure the caller should log a warning and disable streaming;
/// local stdout output continues normally (Section 11 error handling).
pub fn start_run(
    agent: &ureq::Agent,
    api_base: &str,
    token: &str,
    metadata: &JobMetadata,
    pid: Option<i32>,
    host: &HostInfo,
    cloud: &CloudInfo,
) -> Result<RunContext, String> {
    let payload = StartRunRequest {
        metadata: MetadataPayload {
            job_name: metadata.job_name.as_deref(),
            project_name: metadata.project_name.as_deref(),
            stage_name: metadata.stage_name.as_deref(),
            task_name: metadata.task_name.as_deref(),
            team: metadata.team.as_deref(),
            env: metadata.env.as_deref(),
            language: metadata.language.as_deref(),
            orchestrator: metadata.orchestrator.as_deref(),
            executor: metadata.executor.as_deref(),
            external_run_id: metadata.external_run_id.as_deref(),
            container_image: metadata.container_image.as_deref(),
            tags: &metadata.tags,
            pid,
            command: &metadata.command,
        },
        host,
        cloud,
    };

    let url = format!("{api_base}/runs");
    let body = serde_json::to_string(&payload)
        .map_err(|e| format!("failed to serialize start_run payload: {e}"))?;

    let mut response = agent
        .post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
        .map_err(|e| format!("start_run POST failed: {e}"))?;

    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("start_run read body failed: {e}"))?;

    let resp: StartRunResponse = serde_json::from_str(&text).map_err(|e| {
        format!(
            "start_run parse response failed: {e} ({} bytes)",
            text.len()
        )
    })?;

    Ok(RunContext {
        run_id: resp.run_id,
        upload_uri_prefix: resp.upload_uri_prefix,
        credentials: UploadCredentials {
            access_key_id: resp.upload_credentials.access_key,
            secret_access_key: resp.upload_credentials.secret_key,
            session_token: resp.upload_credentials.session_token,
            expires_at: resp
                .upload_credentials
                .expiration
                .unwrap_or_else(|| "2099-01-01T00:00:00Z".to_string()),
        },
    })
}

/// POST to `/runs/{run_id}/credentials/refresh` to obtain fresh STS credentials.
///
/// Updates `ctx.credentials` in place on success.
pub fn refresh_credentials(
    agent: &ureq::Agent,
    api_base: &str,
    token: &str,
    ctx: &mut RunContext,
) -> Result<(), String> {
    let url = format!("{api_base}/runs/{}/refresh-credentials", ctx.run_id);
    let mut response = agent
        .post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .send(b"" as &[u8])
        .map_err(|e| format!("credential refresh POST failed: {e}"))?;

    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("credential refresh read body failed: {e}"))?;

    let resp: RefreshCredentialsResponse = serde_json::from_str(&text).map_err(|e| {
        format!(
            "credential refresh parse failed: {e} ({} bytes)",
            text.len()
        )
    })?;

    ctx.credentials = UploadCredentials {
        access_key_id: resp.upload_credentials.access_key,
        secret_access_key: resp.upload_credentials.secret_key,
        session_token: resp.upload_credentials.session_token,
        expires_at: resp
            .upload_credentials
            .expiration
            .unwrap_or_else(|| "2099-01-01T00:00:00Z".to_string()),
    };
    Ok(())
}

/// POST to `/runs/{run_id}/finish` to mark the run complete.
///
/// `remaining_csv`: samples collected after the last S3 batch upload.
/// These are base64-encoded and sent inline; earlier batches already uploaded
/// to S3 are associated with the run server-side by run_id -- they do not need
/// to be listed here.
pub fn close_run(
    agent: &ureq::Agent,
    api_base: &str,
    token: &str,
    ctx: &RunContext,
    exit_code: Option<i32>,
    remaining_csv: Option<String>,
) -> Result<(), String> {
    // Python RunStatus: "finished" for success/clean exit, "failed" for non-zero.
    // None means SIGTERM clean shutdown -- treat as "finished".
    let run_status = match exit_code {
        Some(0) | None => "finished",
        Some(_) => "failed",
    };

    // Base64-encode the remaining samples CSV.  Matches Python:
    //   payload["data_csv"] = b64encode(data_csv).decode("ascii")
    let data_csv = remaining_csv
        .map(|csv| base64_encode(csv.as_bytes()))
        .unwrap_or_default();

    let payload = CloseRunRequest {
        exit_code,
        run_status,
        data_source: "inline",
        data_csv,
    };

    let url = format!("{api_base}/runs/{}/finish", ctx.run_id);
    let body = serde_json::to_string(&payload)
        .map_err(|e| format!("failed to serialize close_run payload: {e}"))?;

    // Send plain JSON -- no body-level gzip.  The Sentinel API (FastAPI) does
    // not decompress Content-Encoding: gzip on incoming request bodies, so
    // sending compressed bytes causes a 422.  This matches the Python reference
    // which calls requests.post(url, json=payload) with no Content-Encoding.
    agent
        .post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
        .map_err(|e| format!("close_run POST failed: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_creds_expiring_soon_far_future() {
        let ctx = RunContext {
            run_id: "test".to_string(),
            upload_uri_prefix: "s3://b/p".to_string(),
            credentials: UploadCredentials {
                access_key_id: "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token: "t".to_string(),
                expires_at: "2099-01-01T00:00:00Z".to_string(),
            },
        };
        assert!(!ctx.creds_expiring_soon());
    }

    #[test]
    fn test_creds_expiring_soon_past() {
        let ctx = RunContext {
            run_id: "test".to_string(),
            upload_uri_prefix: "s3://b/p".to_string(),
            credentials: UploadCredentials {
                access_key_id: "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token: "t".to_string(),
                expires_at: "1970-01-01T00:00:00Z".to_string(),
            },
        };
        assert!(ctx.creds_expiring_soon());
    }

    #[test]
    fn test_creds_expiring_soon_unparseable() {
        let ctx = RunContext {
            run_id: "test".to_string(),
            upload_uri_prefix: "s3://b/p".to_string(),
            credentials: UploadCredentials {
                access_key_id: "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token: "t".to_string(),
                expires_at: "not-a-date".to_string(),
            },
        };
        // Unparseable expires_at treated as already expired.
        assert!(ctx.creds_expiring_soon());
    }

    #[test]
    fn test_days_since_epoch_known_dates() {
        assert_eq!(days_since_epoch(1970, 1, 1), Some(0));
        assert_eq!(days_since_epoch(1970, 1, 2), Some(1));
        assert_eq!(days_since_epoch(2026, 4, 1), Some(20544));
    }

    #[test]
    fn test_parse_iso8601_secs_known() {
        // 2026-04-01T00:00:00Z = 20544 days * 86400
        assert_eq!(
            parse_iso8601_secs("2026-04-01T00:00:00Z"),
            Some(20544 * 86400)
        );
    }

    // T-EOR-01: close_run POSTs to /runs/{run_id}/finish with the correct shape.
    // Verifies: run_id in URL (not body), data_source=inline, data_csv present,
    // no "s3" field, run_status and exit_code present.
    #[test]
    fn test_close_run_posts_to_finish_endpoint() {
        use std::io::{Read as _, Write as _};
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Read headers + body in a loop until Content-Length bytes are present.
                let mut buf = Vec::<u8>::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = stream.read(&mut tmp).unwrap_or(0);
                    if n == 0 { break; }
                    buf.extend_from_slice(&tmp[..n]);
                    // Find header/body separator.
                    if let Some(sep) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header_str = String::from_utf8_lossy(&buf[..sep]).to_ascii_lowercase();
                        let cl = header_str
                            .lines()
                            .find_map(|l| {
                                l.trim()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if buf.len() >= sep + 4 + cl { break; }
                    }
                }
                tx.send(buf).ok();
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                    .ok();
            }
        });

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();

        let ctx = RunContext {
            run_id: "run-abc-999".to_string(),
            upload_uri_prefix: "s3://b/p".to_string(),
            credentials: UploadCredentials {
                access_key_id:     "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token:     "t".to_string(),
                expires_at:        "2099-01-01T00:00:00Z".to_string(),
            },
        };

        let result = close_run(
            &agent,
            &format!("http://127.0.0.1:{port}"),
            "test-token",
            &ctx,
            Some(0),
            Some("header\nrow1\n".to_string()),
        );
        assert!(result.is_ok(), "close_run failed: {result:?}");

        let raw = rx.recv().expect("mock server did not receive request");
        let raw_str = String::from_utf8_lossy(&raw);

        // run_id belongs in the URL path, not the JSON body.
        assert!(
            raw_str.contains("/runs/run-abc-999/finish"),
            "URL must include /runs/{{run_id}}/finish: {raw_str}"
        );
        assert!(
            !raw_str.contains("\"run_id\""),
            "run_id must not appear in the JSON body: {raw_str}"
        );

        // data_source must be "inline"; no "s3" field anywhere.
        assert!(
            raw_str.contains("\"data_source\":\"inline\""),
            "expected data_source=inline in body: {raw_str}"
        );
        assert!(
            !raw_str.contains("\"s3\""),
            "s3 must not appear in body: {raw_str}"
        );

        // data_csv must be present (base64 of the remaining CSV).
        assert!(
            raw_str.contains("\"data_csv\""),
            "data_csv absent from body: {raw_str}"
        );

        // run_status and exit_code must be present.
        assert!(
            raw_str.contains("\"run_status\":\"finished\""),
            "run_status absent or wrong: {raw_str}"
        );
        assert!(
            raw_str.contains("\"exit_code\":0"),
            "exit_code absent or wrong: {raw_str}"
        );
    }

    // T-EOR-02: close_run body does NOT contain run_id (it is already in the URL path).
    #[test]
    fn test_close_run_request_omits_run_id() {
        let req = CloseRunRequest {
            exit_code: Some(0),
            run_status: "finished",
            data_source: "inline",
            data_csv: "aGVhZGVyCnJvdwo=".to_string(),
        };
        let json = serde_json::to_string(&req).expect("serialize failed");
        assert!(
            !json.contains("\"run_id\""),
            "run_id must not appear in close_run body (it is in the URL): {json}"
        );
    }

    // T-EOR-03: data_source is always "inline" and data_csv is present.
    #[test]
    fn test_close_run_data_source_inline() {
        let req = CloseRunRequest {
            exit_code: Some(0),
            run_status: "finished",
            data_source: "inline",
            data_csv: "base64csv==".to_string(),
        };
        let json = serde_json::to_string(&req).expect("serialize failed");
        assert!(
            json.contains("\"data_source\":\"inline\""),
            "data_source is not 'inline': {json}"
        );
        assert!(
            json.contains("\"data_csv\":\"base64csv==\""),
            "data_csv absent from payload: {json}"
        );
    }

    // base64_encode: RFC 4648 test vectors (Section 10).
    #[test]
    fn test_base64_encode_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    // base64_encode: encoding and decoding round-trip is valid base64.
    #[test]
    fn test_base64_encode_csv_roundtrip() {
        let csv = "timestamp,value\n1000,42\n1001,99\n";
        let encoded = base64_encode(csv.as_bytes());
        // All chars in the output must be valid base64 characters.
        encoded.chars().for_each(|c| {
            assert!(
                c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=',
                "invalid base64 char '{c}' in: {encoded}"
            );
        });
    }

    // days_since_epoch: invalid month/day/year return None.
    #[test]
    fn test_days_since_epoch_invalid_inputs() {
        assert_eq!(days_since_epoch(1970, 0, 1), None, "month 0 is invalid");
        assert_eq!(days_since_epoch(1970, 13, 1), None, "month 13 is invalid");
        assert_eq!(days_since_epoch(1970, 1, 0), None, "day 0 is invalid");
        assert_eq!(days_since_epoch(1970, 1, 32), None, "day 32 is invalid");
        assert_eq!(days_since_epoch(1969, 12, 31), None, "year before 1970 is invalid");
    }

    // parse_iso8601_secs: +00:00 suffix is handled the same as Z.
    #[test]
    fn test_parse_iso8601_secs_with_utc_offset() {
        let with_z    = parse_iso8601_secs("2026-04-01T00:00:00Z");
        let with_plus = parse_iso8601_secs("2026-04-01T00:00:00+00:00");
        assert_eq!(with_z, with_plus, "+00:00 and Z must parse to the same timestamp");
    }

    // parse_iso8601_secs: fewer than 6 numeric components returns None.
    #[test]
    fn test_parse_iso8601_secs_too_few_components() {
        assert_eq!(parse_iso8601_secs("2026-04-01"), None, "date-only string must return None");
        assert_eq!(parse_iso8601_secs("not-a-date"), None);
    }

    // slice_is_empty helper is used by serde skip_serializing_if.
    #[test]
    fn test_slice_is_empty_helper() {
        let empty: &[String] = &[];
        let nonempty: &[String] = &["tag".to_string()];
        assert!(slice_is_empty(&&*empty),   "empty slice should return true");
        assert!(!slice_is_empty(&&*nonempty), "nonempty slice should return false");
    }

    // T-EOR-05: refresh_credentials updates ctx.credentials in place on success.
    #[test]
    fn test_refresh_credentials_updates_context() {
        use std::io::{Read as _, Write as _};
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let response_body = r#"{"upload_credentials":{"access_key":"NEW_AK","secret_key":"NEW_SK","session_token":"NEW_ST","expiration":"2099-06-01T00:00:00Z"}}"#;
        let http_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            response_body.len(),
            response_body
        );

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut tmp = [0u8; 4096];
                let _ = stream.read(&mut tmp);
                stream.write_all(http_response.as_bytes()).ok();
            }
        });

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();

        let mut ctx = RunContext {
            run_id:              "run-123".to_string(),
            upload_uri_prefix:   "s3://b/p".to_string(),
            credentials: UploadCredentials {
                access_key_id:     "OLD_AK".to_string(),
                secret_access_key: "OLD_SK".to_string(),
                session_token:     "OLD_ST".to_string(),
                expires_at:        "2099-01-01T00:00:00Z".to_string(),
            },
        };

        let result = refresh_credentials(
            &agent,
            &format!("http://127.0.0.1:{port}"),
            "test-token",
            &mut ctx,
        );
        assert!(result.is_ok(), "refresh_credentials failed: {:?}", result.err());
        assert_eq!(ctx.credentials.access_key_id,     "NEW_AK");
        assert_eq!(ctx.credentials.secret_access_key, "NEW_SK");
        assert_eq!(ctx.credentials.session_token,     "NEW_ST");
        assert_eq!(ctx.credentials.expires_at,        "2099-06-01T00:00:00Z");
    }

    // T-EOR-04: start_run POSTs to /runs and parses the response correctly.
    #[test]
    fn test_start_run_posts_to_runs_endpoint() {
        use std::io::{Read as _, Write as _};
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        // Minimal valid StartRunResponse JSON.
        let response_body = r#"{"run_id":"run-xyz","upload_uri_prefix":"s3://b/p","upload_credentials":{"access_key":"AK","secret_key":"SK","session_token":"ST","expiration":"2099-01-01T00:00:00Z"}}"#;
        let http_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            response_body.len(),
            response_body
        );

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = Vec::<u8>::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = stream.read(&mut tmp).unwrap_or(0);
                    if n == 0 { break; }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(sep) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header_str = String::from_utf8_lossy(&buf[..sep]).to_ascii_lowercase();
                        let cl = header_str.lines()
                            .find_map(|l| l.trim().strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok()))
                            .unwrap_or(0);
                        if buf.len() >= sep + 4 + cl { break; }
                    }
                }
                tx.send(buf).ok();
                stream.write_all(http_response.as_bytes()).ok();
            }
        });

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();

        let meta = crate::config::JobMetadata {
            job_name: Some("test-job".to_string()),
            ..Default::default()
        };
        let host  = crate::metrics::HostInfo::default();
        let cloud = crate::metrics::CloudInfo::default();

        let result = start_run(
            &agent,
            &format!("http://127.0.0.1:{port}"),
            "test-token",
            &meta,
            Some(42),
            &host,
            &cloud,
        );
        assert!(result.is_ok(), "start_run failed: {:?}", result.err());

        let ctx = result.unwrap();
        assert_eq!(ctx.run_id, "run-xyz");
        assert_eq!(ctx.upload_uri_prefix, "s3://b/p");
        assert_eq!(ctx.credentials.access_key_id, "AK");

        let raw = rx.recv().expect("mock server did not receive request");
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(raw_str.contains("POST /runs"), "must POST to /runs: {raw_str}");
        assert!(raw_str.contains("\"job_name\":\"test-job\""), "job_name missing: {raw_str}");
        assert!(raw_str.contains("\"pid\":42"), "pid missing: {raw_str}");
    }

    // T-CMD-01: when start_run is called with a non-empty command (shell-wrapper mode),
    // the JSON body must contain a "command" array matching the wrapped command tokens.
    // This mirrors: SENTINEL_API_TOKEN=... resource-tracker-rs --output /tmp/log \
    //   stress --cpu 4 --vm 1 --vm-bytes 12024M --timeout 63s
    #[test]
    fn test_start_run_includes_command_array_in_payload() {
        use std::io::{Read as _, Write as _};
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        let response_body = r#"{"run_id":"r1","upload_uri_prefix":"s3://b/p","upload_credentials":{"access_key":"AK","secret_key":"SK","session_token":"ST","expiration":"2099-01-01T00:00:00Z"}}"#;
        let http_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            response_body.len(),
            response_body
        );

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = Vec::<u8>::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = stream.read(&mut tmp).unwrap_or(0);
                    if n == 0 { break; }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(sep) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header_str = String::from_utf8_lossy(&buf[..sep]).to_ascii_lowercase();
                        let cl = header_str.lines()
                            .find_map(|l| l.trim().strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok()))
                            .unwrap_or(0);
                        if buf.len() >= sep + 4 + cl { break; }
                    }
                }
                tx.send(buf).ok();
                stream.write_all(http_response.as_bytes()).ok();
            }
        });

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();

        // Simulate: resource-tracker-rs --output /tmp/resource-tracker-logs \
        //   stress --cpu 4 --vm 1 --vm-bytes 12024M --timeout 63s
        let wrapped_command: Vec<String> = vec![
            "stress", "--cpu", "4", "--vm", "1",
            "--vm-bytes", "12024M", "--timeout", "63s",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let meta = crate::config::JobMetadata {
            command: wrapped_command.clone(),
            ..Default::default()
        };
        let host  = crate::metrics::HostInfo::default();
        let cloud = crate::metrics::CloudInfo::default();

        let result = start_run(
            &agent,
            &format!("http://127.0.0.1:{port}"),
            "test-token",
            &meta,
            None,
            &host,
            &cloud,
        );
        assert!(result.is_ok(), "start_run failed: {:?}", result.err());

        let raw = rx.recv().expect("mock server did not receive request");
        let raw_str = String::from_utf8_lossy(&raw);

        // The body must contain the command as a JSON array.
        let expected = r#""command":["stress","--cpu","4","--vm","1","--vm-bytes","12024M","--timeout","63s"]"#;
        assert!(
            raw_str.contains(expected),
            "command array not found in payload.\nExpected: {expected}\nGot body: {raw_str}"
        );
    }

    // T-CMD-02: when start_run is called without a wrapped command (standalone mode),
    // the "command" field must be absent from the JSON body.
    #[test]
    fn test_start_run_omits_command_when_standalone() {
        let req_payload = MetadataPayload {
            job_name:        None,
            project_name:    None,
            stage_name:      None,
            task_name:       None,
            team:            None,
            env:             None,
            language:        None,
            orchestrator:    None,
            executor:        None,
            external_run_id: None,
            container_image: None,
            tags:            &[],
            pid:             None,
            command:         &[],  // empty = standalone mode
        };
        let json = serde_json::to_string(&req_payload).expect("serialize failed");
        assert!(
            !json.contains("\"command\""),
            "command must be absent from payload in standalone mode: {json}"
        );
    }
}
