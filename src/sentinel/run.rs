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

#[cfg(test)]
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

/// Format a Unix timestamp as an ISO 8601 UTC string (`"YYYY-MM-DDTHH:MM:SSZ"`).
/// Accurate for 1970-2199.
fn unix_secs_to_iso8601(secs: u64) -> String {
    let tod = secs % 86400;
    let mut days = secs / 86400;
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;

    let mut year = 1970u64;
    loop {
        let is_leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let yd = if is_leap { 366u64 } else { 365u64 };
        if days < yd { break; }
        days -= yd;
        year += 1;
    }
    let is_leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    const MDAYS: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u64;
    loop {
        let dim = if month == 2 && is_leap { 29u64 } else { MDAYS[(month - 1) as usize] };
        if days < dim { break; }
        days -= dim;
        month += 1;
    }
    let day = days + 1;
    format!("{year:04}-{month:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    unix_secs_to_iso8601(secs)
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

// See https://api.sentinel.sparecores.net/docs#/Resource%20Tracker/finish_run_runs__run_id__finish_post
// The /finish endpoint accepts two discriminated variants keyed on data_source:
//   "inline" → RunFinishInline  (data_csv: raw CSV string, additionalProperties:false)
//   "s3"     → RunFinishS3      (data_uris: [s3://...], additionalProperties:false)
// run_id goes in the URL path only -- do not repeat in body.

/// Payload for RunFinishInline: remaining samples sent as a raw CSV string.
/// Used when no S3 batches were uploaded (short runs or all S3 failures).
#[derive(Debug, Serialize)]
struct CloseRunInlineRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    run_status: &'static str,
    /// Exact finish time in ISO 8601 UTC format, e.g. "2026-04-03T12:00:00Z".
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at: Option<String>,
    /// Discriminator value -- must be exactly "inline".
    data_source: &'static str,
    /// Raw CSV string -- NOT base64-encoded.  The API schema type is plain string.
    data_csv: String,
}

/// Payload for RunFinishS3: data was already uploaded to S3 in batches.
/// Used when at least one S3 batch was successfully uploaded.
/// The final remaining samples must have been flushed to S3 before calling
/// close_run (the BatchUploader performs this flush on shutdown).
#[derive(Debug, Serialize)]
struct CloseRunS3Request {
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    run_status: &'static str,
    /// Exact finish time in ISO 8601 UTC format, e.g. "2026-04-03T12:00:00Z".
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at: Option<String>,
    /// Discriminator value -- must be exactly "s3".
    data_source: &'static str,
    /// S3 URIs of all uploaded batch files (including the final flush).
    data_uris: Vec<String>,
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
/// Dispatches to the correct schema variant based on whether S3 uploads occurred:
///
/// - `uploaded_uris` non-empty → `RunFinishS3` (`data_source="s3"`, `data_uris=[...]`).
///   The `BatchUploader` already flushed the final remaining samples to S3 on
///   shutdown before this function is called, so `uploaded_uris` contains every batch.
///   `remaining_csv` is ignored in this path.
///
/// - `uploaded_uris` empty → `RunFinishInline` (`data_source="inline"`, `data_csv=...`).
///   Used for short runs or when all S3 uploads failed.  `remaining_csv` is the
///   raw CSV of all collected samples (never base64-encoded).
pub fn close_run(
    agent: &ureq::Agent,
    api_base: &str,
    token: &str,
    ctx: &RunContext,
    exit_code: Option<i32>,
    remaining_csv: Option<String>,
    uploaded_uris: &[String],
) -> Result<(), String> {
    // "finished" for success/clean exit (including SIGTERM), "failed" for non-zero.
    let run_status = match exit_code {
        Some(0) | None => "finished",
        Some(_) => "failed",
    };
    let finished_at = Some(now_iso8601());

    let url = format!("{api_base}/runs/{}/finish", ctx.run_id);
    let body = if uploaded_uris.is_empty() {
        // Inline route: no S3 batches uploaded.
        // data_csv must be the raw CSV string (not base64) per the API schema.
        let payload = CloseRunInlineRequest {
            exit_code,
            run_status,
            finished_at,
            data_source: "inline",
            data_csv: remaining_csv.unwrap_or_default(),
        };
        serde_json::to_string(&payload)
            .map_err(|e| format!("failed to serialize close_run inline payload: {e}"))?
    } else {
        // S3 route: all batches (including final flush) are in uploaded_uris.
        let payload = CloseRunS3Request {
            exit_code,
            run_status,
            finished_at,
            data_source: "s3",
            data_uris: uploaded_uris.to_vec(),
        };
        serde_json::to_string(&payload)
            .map_err(|e| format!("failed to serialize close_run s3 payload: {e}"))?
    };

    // Send plain JSON -- no body-level gzip.  The Sentinel API (FastAPI) does
    // not decompress Content-Encoding: gzip on incoming request bodies, so
    // sending compressed bytes causes a 422.  This matches the Python reference
    // which calls requests.post(url, json=payload) with no Content-Encoding.
    let response = agent
        .post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
        .map_err(|e| format!("close_run POST failed: {e}"))?;

    let status = response.status();
    if status != 200 {
        return Err(format!("close_run received HTTP {status}: expected 200"));
    }

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
        // follow schema guidance of https://api.sentinel.sparecores.net/docs#/Resource%20Tracker/finish_run_runs__run_id__finish_post to PASS
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
                    if n == 0 {
                        break;
                    }
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
                        if buf.len() >= sep + 4 + cl {
                            break;
                        }
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
                access_key_id: "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token: "t".to_string(),
                expires_at: "2099-01-01T00:00:00Z".to_string(),
            },
        };

        let result = close_run(
            &agent,
            &format!("http://127.0.0.1:{port}"),
            "test-token",
            &ctx,
            Some(0),
            Some("header\nrow1\n".to_string()),
            &[], // no S3 uploads → inline route
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

        // data_csv must be present as a raw CSV string (not base64-encoded).
        assert!(
            raw_str.contains("\"data_csv\""),
            "data_csv absent from body: {raw_str}"
        );
        assert!(
            raw_str.contains("header"),
            "data_csv must contain raw CSV content (not base64): {raw_str}"
        );

        // finished_at must be present as an ISO 8601 UTC timestamp.
        assert!(
            raw_str.contains("\"finished_at\""),
            "finished_at absent from body: {raw_str}"
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
        let req = CloseRunInlineRequest {
            exit_code: Some(0),
            run_status: "finished",
            finished_at: Some("2026-04-03T12:00:00Z".to_string()),
            data_source: "inline",
            data_csv: "timestamp,cpu\n1000,42\n".to_string(),
        };
        let json = serde_json::to_string(&req).expect("serialize failed");
        assert!(
            !json.contains("\"run_id\""),
            "run_id must not appear in close_run body (it is in the URL): {json}"
        );
    }

    // T-EOR-03: data_source is "inline", data_csv is raw CSV (not base64), finished_at present.
    #[test]
    fn test_close_run_data_source_inline() {
        let raw_csv = "timestamp,cpu\n1000,42\n";
        let req = CloseRunInlineRequest {
            exit_code: Some(0),
            run_status: "finished",
            finished_at: Some("2026-04-03T12:00:00Z".to_string()),
            data_source: "inline",
            data_csv: raw_csv.to_string(),
        };
        let json = serde_json::to_string(&req).expect("serialize failed");
        assert!(
            json.contains("\"data_source\":\"inline\""),
            "data_source is not 'inline': {json}"
        );
        // data_csv must be the raw string -- not base64.
        assert!(
            json.contains("timestamp"),
            "data_csv must contain raw CSV content (not base64): {json}"
        );
        assert!(
            json.contains("\"finished_at\":\"2026-04-03T12:00:00Z\""),
            "finished_at absent or wrong: {json}"
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
        assert_eq!(
            days_since_epoch(1969, 12, 31),
            None,
            "year before 1970 is invalid"
        );
    }

    // parse_iso8601_secs: +00:00 suffix is handled the same as Z.
    #[test]
    fn test_parse_iso8601_secs_with_utc_offset() {
        let with_z = parse_iso8601_secs("2026-04-01T00:00:00Z");
        let with_plus = parse_iso8601_secs("2026-04-01T00:00:00+00:00");
        assert_eq!(
            with_z, with_plus,
            "+00:00 and Z must parse to the same timestamp"
        );
    }

    // parse_iso8601_secs: fewer than 6 numeric components returns None.
    #[test]
    fn test_parse_iso8601_secs_too_few_components() {
        assert_eq!(
            parse_iso8601_secs("2026-04-01"),
            None,
            "date-only string must return None"
        );
        assert_eq!(parse_iso8601_secs("not-a-date"), None);
    }

    // slice_is_empty helper is used by serde skip_serializing_if.
    #[test]
    fn test_slice_is_empty_helper() {
        let empty: &[String] = &[];
        let nonempty: &[String] = &["tag".to_string()];
        assert!(slice_is_empty(&&*empty), "empty slice should return true");
        assert!(
            !slice_is_empty(&&*nonempty),
            "nonempty slice should return false"
        );
    }

    // unix_secs_to_iso8601: known Unix timestamps produce the expected ISO 8601 string.
    #[test]
    fn test_unix_secs_to_iso8601_known_values() {
        // Unix epoch itself.
        assert_eq!(unix_secs_to_iso8601(0), "1970-01-01T00:00:00Z");
        // 2000-01-01T00:00:00Z = 946684800 s
        assert_eq!(unix_secs_to_iso8601(946684800), "2000-01-01T00:00:00Z");
        // 2026-04-03T00:00:00Z -- today's date from context; days = 56*365 + 14 leap days + 92
        // Easier to round-trip via parse_iso8601_secs.
        let secs = parse_iso8601_secs("2026-04-03T15:30:45Z").expect("parse failed");
        assert_eq!(unix_secs_to_iso8601(secs), "2026-04-03T15:30:45Z");
    }

    // unix_secs_to_iso8601: leap-day boundary (2000-02-29 exists, 1900-02-29 did not
    // but we only go back to 1970 so just verify 2000-02-29 round-trips).
    #[test]
    fn test_unix_secs_to_iso8601_leap_day() {
        let secs = parse_iso8601_secs("2000-02-29T12:00:00Z").expect("parse failed");
        assert_eq!(unix_secs_to_iso8601(secs), "2000-02-29T12:00:00Z");
    }

    // now_iso8601: returns a non-empty string that parses back successfully.
    #[test]
    fn test_now_iso8601_parses() {
        let s = now_iso8601();
        assert!(!s.is_empty(), "now_iso8601 must not be empty");
        assert!(s.ends_with('Z'), "now_iso8601 must end with Z: {s}");
        let secs = parse_iso8601_secs(&s);
        assert!(secs.is_some(), "now_iso8601 output must parse back: {s}");
    }

    // T-EOR-06: finished_at is omitted from the JSON when set to None.
    #[test]
    fn test_close_run_finished_at_omitted_when_none() {
        let req = CloseRunInlineRequest {
            exit_code: None,
            run_status: "finished",
            finished_at: None,
            data_source: "inline",
            data_csv: "".to_string(),
        };
        let json = serde_json::to_string(&req).expect("serialize failed");
        assert!(
            !json.contains("\"finished_at\""),
            "finished_at must be absent when None: {json}"
        );
    }

    // ---------------------------------------------------------------------------
    // Spec-driven tests for /runs/{run_id}/finish (RunFinishInline schema).
    // See https://api.sentinel.sparecores.net/openapi.json -- RunFinishInline.
    // Required fields: data_source (const "inline"), data_csv (raw string).
    // Optional fields: exit_code (int|null), run_status ("finished"|"failed"),
    //                  finished_at (date-time string|null).
    // additionalProperties: false -- no extra fields allowed.
    // ---------------------------------------------------------------------------

    // Build a realistic RunFinishResponse JSON body, mirroring the real Sentinel API
    // response shape (RunFinishResponse = {run: RunResponse, processing: RunFinishProcessing}).
    // The real API's 200 response always has both fields; "processing.status" is required.
    fn finish_response_json(run_id: &str, run_status: &str, exit_code: Option<i32>) -> String {
        let exit_code_field = match exit_code {
            Some(c) => format!(",\"exit_code\":{c}"),
            None => String::new(),
        };
        format!(
            r#"{{"run":{{"run_id":"{run_id}","created_at":"2026-04-03T10:00:00Z","heartbeat_at":"2026-04-03T10:00:30Z","finished_at":"2026-04-03T10:01:00Z","run_status":"{run_status}","tag_count":0,"tags":[]{exit_code_field}}},"processing":{{"status":"ok","rows":1,"files":null,"duration_ms":5.0,"error":null}}}}"#
        )
    }

    // Helper: spin up a mock TCP server that mimics the real Sentinel /finish endpoint:
    // reads the full request, validates presence of required fields, returns a proper
    // RunFinishResponse JSON on success (200) or a 422 JSON error if required fields
    // are missing.  Returns the parsed request body JSON for assertions.
    fn capture_close_run_body(
        exit_code: Option<i32>,
        csv: Option<&str>,
    ) -> String {
        use std::io::{Read as _, Write as _};
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

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
                        let cl = header_str.lines().find_map(|l| {
                            l.trim().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())
                        }).unwrap_or(0);
                        if buf.len() >= sep + 4 + cl { break; }
                    }
                }
                // Parse the body and decide: 200 with RunFinishResponse, or 422.
                let body_start = buf.windows(4).position(|w| w == b"\r\n\r\n")
                    .map(|p| p + 4).unwrap_or(buf.len());
                let body_str = String::from_utf8_lossy(&buf[body_start..]);
                let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(serde_json::Value::Null);
                // Real API requires data_source and data_csv for RunFinishInline.
                let valid = parsed.get("data_source").and_then(|v| v.as_str()) == Some("inline")
                    && parsed.get("data_csv").is_some();
                let (status_line, resp_body) = if valid {
                    let run_status = parsed.get("run_status").and_then(|v| v.as_str()).unwrap_or("finished");
                    let ec = parsed.get("exit_code").and_then(|v| v.as_i64()).map(|v| v as i32);
                    (
                        "HTTP/1.1 200 OK",
                        finish_response_json("run-spec-test", run_status, ec),
                    )
                } else {
                    (
                        "HTTP/1.1 422 Unprocessable Entity",
                        r#"{"detail":[{"loc":["body"],"msg":"field required","type":"value_error.missing"}]}"#.to_string(),
                    )
                };
                tx.send(buf).ok();
                let http = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    resp_body.len(), resp_body
                );
                stream.write_all(http.as_bytes()).ok();
            }
        });

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();
        let ctx = RunContext {
            run_id: "run-spec-test".to_string(),
            upload_uri_prefix: "s3://b/p".to_string(),
            credentials: UploadCredentials {
                access_key_id: "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token: "t".to_string(),
                expires_at: "2099-01-01T00:00:00Z".to_string(),
            },
        };
        let _ = close_run(
            &agent,
            &format!("http://127.0.0.1:{port}"),
            "token",
            &ctx,
            exit_code,
            csv.map(String::from),
            &[], // no S3 uploads → inline route
        );
        let raw = rx.recv().expect("mock server did not capture request");
        // Extract the JSON body (everything after \r\n\r\n).
        let body_start = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4).unwrap_or(0);
        String::from_utf8_lossy(&raw[body_start..]).to_string()
    }

    // T-FIN-01: run_status is "finished" when exit_code is 0 (clean success).
    #[test]
    fn test_close_run_run_status_finished_for_zero_exit() {
        let body = capture_close_run_body(Some(0), None);
        let v: serde_json::Value = serde_json::from_str(&body).expect("body is not JSON");
        assert_eq!(
            v["run_status"], "finished",
            "run_status must be 'finished' for exit_code=0: {body}"
        );
        assert_eq!(
            v["exit_code"], 0,
            "exit_code must be 0 in payload: {body}"
        );
    }

    // T-FIN-02: run_status is "finished" when exit_code is None (SIGTERM shutdown).
    #[test]
    fn test_close_run_run_status_finished_for_sigterm() {
        let body = capture_close_run_body(None, None);
        let v: serde_json::Value = serde_json::from_str(&body).expect("body is not JSON");
        assert_eq!(
            v["run_status"], "finished",
            "run_status must be 'finished' when exit_code is None (SIGTERM): {body}"
        );
        // exit_code is skipped when None -- it must not appear in the payload.
        assert!(
            v.get("exit_code").is_none(),
            "exit_code must be absent when None (spec: optional integer): {body}"
        );
    }

    // T-FIN-03: run_status is "failed" for any non-zero exit code.
    #[test]
    fn test_close_run_run_status_failed_for_nonzero_exit() {
        for code in [1, 2, 127, 130, 255] {
            let body = capture_close_run_body(Some(code), None);
            let v: serde_json::Value = serde_json::from_str(&body).expect("body is not JSON");
            assert_eq!(
                v["run_status"], "failed",
                "run_status must be 'failed' for exit_code={code}: {body}"
            );
        }
    }

    // T-FIN-04: data_csv is a raw CSV string, not base64.
    // Spec: data_csv type is string, description "Raw CSV string".
    #[test]
    fn test_close_run_data_csv_is_raw_csv_not_base64() {
        let raw_csv = "timestamp,cpu_pct\n1743638400,42.5\n1743638401,44.0\n";
        let body = capture_close_run_body(Some(0), Some(raw_csv));
        let v: serde_json::Value = serde_json::from_str(&body).expect("body is not JSON");
        let data_csv = v["data_csv"].as_str().expect("data_csv must be a string");
        assert!(
            data_csv.contains("timestamp"),
            "data_csv must be raw CSV (contains header row): {data_csv}"
        );
        assert!(
            data_csv.contains("42.5"),
            "data_csv must be raw CSV (contains data values): {data_csv}"
        );
        // A base64-encoded CSV would not contain commas or newlines.
        assert!(
            data_csv.contains(','),
            "data_csv must contain CSV commas (not base64): {data_csv}"
        );
    }

    // T-FIN-05: finished_at is present and parses as a valid ISO 8601 UTC timestamp.
    // Spec: finished_at is an optional date-time field.
    #[test]
    fn test_close_run_finished_at_is_valid_iso8601() {
        let body = capture_close_run_body(Some(0), None);
        let v: serde_json::Value = serde_json::from_str(&body).expect("body is not JSON");
        let fa = v["finished_at"].as_str().expect("finished_at must be a string");
        assert!(fa.ends_with('Z'), "finished_at must end with Z (UTC): {fa}");
        let secs = parse_iso8601_secs(fa);
        assert!(
            secs.is_some(),
            "finished_at must be a parseable ISO 8601 timestamp: {fa}"
        );
        // Must be recent -- within a few seconds of now.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let diff = now.abs_diff(secs.unwrap());
        assert!(
            diff < 60,
            "finished_at must be close to current time (diff={diff}s): {fa}"
        );
    }

    // T-FIN-06: close_run handles a realistic RunFinishResponse JSON without error.
    // Spec: 200 response body is RunFinishResponse {run, processing}.
    #[test]
    fn test_close_run_handles_valid_run_finish_response() {
        use std::io::{Read as _, Write as _};
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        // Minimal valid RunFinishResponse per OpenAPI spec.
        let response_body = r#"{
            "run": {
                "run_id": "01959e3a-0001-0000-0000-000000000000",
                "created_at": "2026-04-03T10:00:00Z",
                "heartbeat_at": "2026-04-03T10:00:30Z",
                "finished_at": "2026-04-03T10:01:00Z",
                "run_status": "finished",
                "tag_count": 0,
                "tags": []
            },
            "processing": {
                "status": "ok",
                "rows": 60,
                "files": null,
                "duration_ms": 12.5,
                "error": null
            }
        }"#;
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
        let ctx = RunContext {
            run_id: "01959e3a-0001-0000-0000-000000000000".to_string(),
            upload_uri_prefix: "s3://b/p".to_string(),
            credentials: UploadCredentials {
                access_key_id: "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token: "t".to_string(),
                expires_at: "2099-01-01T00:00:00Z".to_string(),
            },
        };
        let result = close_run(
            &agent,
            &format!("http://127.0.0.1:{port}"),
            "test-token",
            &ctx,
            Some(0),
            Some("timestamp,cpu_pct\n1743638400,42.5\n".to_string()),
            &[], // no S3 uploads → inline route
        );
        assert!(result.is_ok(), "close_run must succeed for a 200 response: {result:?}");
    }

    // T-FIN-07: no extra fields are sent beyond what the spec allows
    // (RunFinishInline has additionalProperties: false).
    #[test]
    fn test_close_run_no_extra_fields_in_payload() {
        let body = capture_close_run_body(Some(0), Some("ts,v\n1,2\n"));
        let v: serde_json::Value = serde_json::from_str(&body).expect("body is not JSON");
        let obj = v.as_object().expect("payload must be a JSON object");
        let allowed: std::collections::HashSet<&str> = [
            "exit_code", "run_status", "finished_at", "data_source", "data_csv",
        ].iter().copied().collect();
        for key in obj.keys() {
            assert!(
                allowed.contains(key.as_str()),
                "unexpected field '{key}' in payload -- not allowed by RunFinishInline schema (additionalProperties: false)"
            );
        }
        // Required fields must always be present.
        assert!(obj.contains_key("data_source"), "data_source is required");
        assert!(obj.contains_key("data_csv"), "data_csv is required");
    }

    // ---------------------------------------------------------------------------
    // S3 route tests for close_run (RunFinishS3 schema variant).
    // ---------------------------------------------------------------------------

    // Helper: call close_run with S3 URIs, return the captured request body JSON.
    fn capture_close_run_s3_body(
        exit_code: Option<i32>,
        uris: &[&str],
    ) -> String {
        use std::io::{Read as _, Write as _};
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = Vec::<u8>::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = stream.read(&mut tmp).unwrap_or(0);
                    if n == 0 { break; }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(sep) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let hdr = String::from_utf8_lossy(&buf[..sep]).to_ascii_lowercase();
                        let cl = hdr.lines().find_map(|l| {
                            l.trim().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())
                        }).unwrap_or(0);
                        if buf.len() >= sep + 4 + cl { break; }
                    }
                }
                // Real API: 200 RunFinishResponse for a valid S3 payload.
                let body_start = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4).unwrap_or(buf.len());
                let body_str = String::from_utf8_lossy(&buf[body_start..]);
                let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap_or(serde_json::Value::Null);
                let valid = parsed.get("data_source").and_then(|v| v.as_str()) == Some("s3")
                    && parsed.get("data_uris").is_some();
                let run_status = parsed.get("run_status").and_then(|v| v.as_str()).unwrap_or("finished");
                let ec = parsed.get("exit_code").and_then(|v| v.as_i64()).map(|v| v as i32);
                let (status_line, resp_body) = if valid {
                    ("HTTP/1.1 200 OK", finish_response_json("run-s3-test", run_status, ec))
                } else {
                    ("HTTP/1.1 422 Unprocessable Entity",
                     r#"{"detail":[{"msg":"field required","type":"value_error.missing"}]}"#.to_string())
                };
                tx.send(buf).ok();
                let http = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    resp_body.len(), resp_body
                );
                stream.write_all(http.as_bytes()).ok();
            }
        });

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();
        let ctx = RunContext {
            run_id: "run-s3-test".to_string(),
            upload_uri_prefix: "s3://b/p".to_string(),
            credentials: UploadCredentials {
                access_key_id: "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token: "t".to_string(),
                expires_at: "2099-01-01T00:00:00Z".to_string(),
            },
        };
        let uploaded: Vec<String> = uris.iter().map(|s| (*s).to_string()).collect();
        let _ = close_run(
            &agent,
            &format!("http://127.0.0.1:{port}"),
            "token",
            &ctx,
            exit_code,
            None, // remaining_csv unused in S3 route
            &uploaded,
        );
        let raw = rx.recv().expect("mock server did not capture request");
        let body_start = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4).unwrap_or(0);
        String::from_utf8_lossy(&raw[body_start..]).to_string()
    }

    // T-S3R-01: when uploaded_uris is non-empty, data_source is "s3" and
    // data_uris contains the URIs (RunFinishS3 schema variant).
    #[test]
    fn test_close_run_uses_s3_route_when_uris_present() {
        let uris = &[
            "s3://my-bucket/prefix/run-abc/000000.csv.gz",
            "s3://my-bucket/prefix/run-abc/000001.csv.gz",
        ];
        let body = capture_close_run_s3_body(Some(0), uris);
        let v: serde_json::Value = serde_json::from_str(&body).expect("body is not JSON");
        assert_eq!(v["data_source"], "s3", "data_source must be 's3' when URIs present: {body}");
        let arr = v["data_uris"].as_array().expect("data_uris must be a JSON array");
        assert_eq!(arr.len(), 2, "data_uris must have 2 elements: {body}");
        assert_eq!(arr[0], "s3://my-bucket/prefix/run-abc/000000.csv.gz");
        assert_eq!(arr[1], "s3://my-bucket/prefix/run-abc/000001.csv.gz");
        // data_csv must NOT be present in the S3 route (additionalProperties: false).
        assert!(!body.contains("\"data_csv\""), "data_csv must be absent in S3 route: {body}");
    }

    // T-S3R-02: S3 route payload contains no extra fields beyond RunFinishS3 schema.
    // RunFinishS3 has additionalProperties: false.
    #[test]
    fn test_close_run_s3_no_extra_fields() {
        let uris = &["s3://bucket/prefix/run/000000.csv.gz"];
        let body = capture_close_run_s3_body(Some(0), uris);
        let v: serde_json::Value = serde_json::from_str(&body).expect("body is not JSON");
        let obj = v.as_object().expect("payload must be a JSON object");
        let allowed: std::collections::HashSet<&str> = [
            "exit_code", "run_status", "finished_at", "data_source", "data_uris",
        ].iter().copied().collect();
        for key in obj.keys() {
            assert!(
                allowed.contains(key.as_str()),
                "unexpected field '{key}' in S3 route payload (additionalProperties: false): {body}"
            );
        }
        assert!(obj.contains_key("data_source"), "data_source is required in S3 route");
        assert!(obj.contains_key("data_uris"), "data_uris is required in S3 route");
    }

    // T-S3R-03: when uploaded_uris is empty, data_source is "inline" (not "s3")
    // and data_csv is present.  Confirms route dispatch is based on uploaded_uris.
    #[test]
    fn test_close_run_uses_inline_route_when_no_uris() {
        let body = capture_close_run_body(Some(0), Some("ts,cpu\n1,2\n"));
        let v: serde_json::Value = serde_json::from_str(&body).expect("body is not JSON");
        assert_eq!(v["data_source"], "inline", "data_source must be 'inline' when no URIs: {body}");
        assert!(v.get("data_csv").is_some(), "data_csv must be present for inline route: {body}");
        assert!(v.get("data_uris").is_none(), "data_uris must be absent for inline route: {body}");
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
            run_id: "run-123".to_string(),
            upload_uri_prefix: "s3://b/p".to_string(),
            credentials: UploadCredentials {
                access_key_id: "OLD_AK".to_string(),
                secret_access_key: "OLD_SK".to_string(),
                session_token: "OLD_ST".to_string(),
                expires_at: "2099-01-01T00:00:00Z".to_string(),
            },
        };

        let result = refresh_credentials(
            &agent,
            &format!("http://127.0.0.1:{port}"),
            "test-token",
            &mut ctx,
        );
        assert!(
            result.is_ok(),
            "refresh_credentials failed: {:?}",
            result.err()
        );
        assert_eq!(ctx.credentials.access_key_id, "NEW_AK");
        assert_eq!(ctx.credentials.secret_access_key, "NEW_SK");
        assert_eq!(ctx.credentials.session_token, "NEW_ST");
        assert_eq!(ctx.credentials.expires_at, "2099-06-01T00:00:00Z");
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
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
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
                        if buf.len() >= sep + 4 + cl {
                            break;
                        }
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
        let host = crate::metrics::HostInfo::default();
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
        assert!(
            raw_str.contains("POST /runs"),
            "must POST to /runs: {raw_str}"
        );
        assert!(
            raw_str.contains("\"job_name\":\"test-job\""),
            "job_name missing: {raw_str}"
        );
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
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
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
                        if buf.len() >= sep + 4 + cl {
                            break;
                        }
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
            "stress",
            "--cpu",
            "4",
            "--vm",
            "1",
            "--vm-bytes",
            "12024M",
            "--timeout",
            "63s",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let meta = crate::config::JobMetadata {
            command: wrapped_command.clone(),
            ..Default::default()
        };
        let host = crate::metrics::HostInfo::default();
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
            job_name: None,
            project_name: None,
            stage_name: None,
            task_name: None,
            team: None,
            env: None,
            language: None,
            orchestrator: None,
            executor: None,
            external_run_id: None,
            container_image: None,
            tags: &[],
            pid: None,
            command: &[], // empty = standalone mode
        };
        let json = serde_json::to_string(&req_payload).expect("serialize failed");
        assert!(
            !json.contains("\"command\""),
            "command must be absent from payload in standalone mode: {json}"
        );
    }

    // ---------------------------------------------------------------------------
    // Integration test: hits the REAL Sentinel API.
    //
    // Requires:
    //   SENTINEL_API_TOKEN  -- a valid bearer token
    //   SENTINEL_API_BASE   -- optional; defaults to https://api.sentinel.sparecores.net
    //
    // Run explicitly (skipped in normal `cargo test`):
    //   cargo test test_real_api_finish_run -- --include-ignored
    //   SENTINEL_API_TOKEN=<token> cargo test test_real_api_finish_run -- --include-ignored
    // ---------------------------------------------------------------------------

    // T-INT-01: start_run + close_run against the real API both return Ok(()).
    // Verifies end-to-end that:
    //   - start_run registers a new run and returns a RunContext with a run_id.
    //   - close_run POSTs the correct RunFinishInline payload and receives 200.
    // Runs automatically when SENTINEL_API_TOKEN is set; skips otherwise.
    #[test]
    fn test_real_api_finish_run_returns_ok() {
        use crate::config::JobMetadata;
        use crate::metrics::{CloudInfo, CpuMetrics, HostInfo, MemoryMetrics, Sample};
        use crate::sentinel::upload::samples_to_csv;
        use std::time::Duration;

        let token = match std::env::var("SENTINEL_API_TOKEN") {
            Ok(t) if !t.is_empty() => t,
            _ => {
                eprintln!("skip: SENTINEL_API_TOKEN not set or empty");
                return;
            }
        };
        let api_base = std::env::var("SENTINEL_API_BASE")
            .unwrap_or_else(|_| "https://api.sentinel.sparecores.net".to_string());
        eprintln!("T-INT-01: using api_base={api_base}");

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();

        let metadata = JobMetadata {
            job_name: Some("integration-test-close-run".to_string()),
            ..Default::default()
        };
        let host = HostInfo::default();
        let cloud = CloudInfo::default();

        // Step 1: register a new run.
        eprintln!("T-INT-01: calling start_run...");
        let ctx = match start_run(&agent, &api_base, &token, &metadata, None, &host, &cloud) {
            Ok(c) => {
                eprintln!("T-INT-01: start_run ok -- run_id={}", c.run_id);
                c
            }
            Err(e) => panic!("start_run failed: {e}"),
        };
        assert!(!ctx.run_id.is_empty(), "run_id must be non-empty");

        // Step 2: build a proper sample using the real CSV format so that
        // the column names match what the API expects.
        let timestamp_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let sample = Sample {
            timestamp_secs,
            job_name: Some("integration-test-close-run".to_string()),
            tracked_pid: None,
            cpu: CpuMetrics::default(),
            memory: MemoryMetrics::default(),
            network: vec![],
            disk: vec![],
            gpu: vec![],
        };
        let csv = samples_to_csv(&[sample], 1);
        eprintln!("T-INT-01: csv preview (first 120 chars): {}", &csv[..csv.len().min(120)]);

        // Step 3: finish the run with the inline CSV (no S3 uploads in this test).
        eprintln!("T-INT-01: calling close_run...");
        let result = close_run(
            &agent,
            &api_base,
            &token,
            &ctx,
            Some(0),
            Some(csv),
            &[], // no S3 uploads → inline route
        );
        match &result {
            Ok(()) => eprintln!("T-INT-01: close_run ok -- 200 received"),
            Err(e) => eprintln!("T-INT-01: close_run FAILED: {e}"),
        }
        assert!(result.is_ok(), "close_run must return Ok (200) against the real API: {result:?}");
    }
}
