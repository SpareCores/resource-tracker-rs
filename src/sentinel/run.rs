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
}

#[derive(Debug, Serialize)]
#[serde(tag = "data_source", rename_all = "snake_case")]
enum DataSource {
    S3 {
        data_uris: Vec<String>,
    },
    /// Inline base64-encoded CSV.
    /// The HTTP body carrying this is gzip-compressed at the transport level
    /// (`Content-Encoding: gzip`), so no inner gzip on the field itself.
    /// Matches Python `DataSource.inline` / `finish_run(data_source="inline", data_csv=...)`.
    Inline {
        data_csv: String,
    },
}

#[derive(Debug, Serialize)]
struct CloseRunRequest {
    run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    run_status: &'static str,
    #[serde(flatten)]
    data: DataSource,
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
/// `uploaded_uris`: S3 URIs of all successfully uploaded batches.  When empty,
/// `remaining_csv` is base64-encoded and placed in `data_source = "inline"`.
/// The whole JSON body is gzip-compressed with `Content-Encoding: gzip`,
/// matching the Python `finish_run` behaviour.
pub fn close_run(
    agent: &ureq::Agent,
    api_base: &str,
    token: &str,
    ctx: &RunContext,
    exit_code: Option<i32>,
    uploaded_uris: &[String],
    remaining_csv: Option<String>,
) -> Result<(), String> {
    // Python RunStatus: "finished" for success/clean exit, "failed" for non-zero.
    // None means SIGTERM clean shutdown -- treat as "finished".
    let run_status = match exit_code {
        Some(0) | None => "finished",
        Some(_) => "failed",
    };

    let data = if !uploaded_uris.is_empty() {
        DataSource::S3 {
            data_uris: uploaded_uris.to_vec(),
        }
    } else {
        // Base64-encode the raw CSV.  The whole HTTP body is gzip-compressed
        // below, so no inner gzip here.  Matches Python:
        //   payload["data_csv"] = b64encode(data_csv).decode("ascii")
        let encoded = remaining_csv
            .map(|csv| base64_encode(csv.as_bytes()))
            .unwrap_or_default();
        DataSource::Inline { data_csv: encoded }
    };

    let payload = CloseRunRequest {
        run_id: ctx.run_id.clone(),
        exit_code,
        run_status,
        data,
    };

    let url = format!("{api_base}/runs/{}/finish", ctx.run_id);
    let body = serde_json::to_string(&payload)
        .map_err(|e| format!("failed to serialize close_run payload: {e}"))?;

    // Gzip-compress the entire JSON payload and declare Content-Encoding: gzip.
    // Compressing the whole body (not only the data_csv field) reduces wire size
    // and is consistent with how the Python reference sends batch uploads.
    let compressed = super::upload::gzip_compress(body.as_bytes())
        .map_err(|e| format!("close_run gzip failed: {e}"))?;

    agent
        .post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Content-Encoding", "gzip")
        .header("Content-Length", &compressed.len().to_string())
        .send(&compressed)
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

    // T-EOR-02: close_run request body contains run_id matching the start_run response.
    // Verified by serializing a CloseRunRequest and checking the JSON payload.
    #[test]
    fn test_close_run_request_contains_run_id() {
        let req = CloseRunRequest {
            run_id: "run-abc-123".to_string(),
            exit_code: Some(0),
            run_status: "finished",
            data: DataSource::Inline {
                data_csv: "aGVhZGVyCnJvdwo=".to_string(),
            },
        };
        let json = serde_json::to_string(&req).expect("serialize failed");
        assert!(
            json.contains("\"run_id\":\"run-abc-123\""),
            "run_id absent from close_run payload: {json}"
        );
    }

    // T-EOR-03: data_source is "inline" when no S3 uploads occurred.
    #[test]
    fn test_close_run_data_source_inline_when_no_uploads() {
        let data = DataSource::Inline {
            data_csv: "base64encodedgzip==".to_string(),
        };
        let json = serde_json::to_string(&data).expect("serialize failed");
        assert!(
            json.contains("\"data_source\":\"inline\""),
            "data_source is not 'inline': {json}"
        );
    }

    // T-EOR-04: data_source is "s3" when at least one S3 upload succeeded.
    #[test]
    fn test_close_run_data_source_s3_when_uploads_present() {
        let data = DataSource::S3 {
            data_uris: vec!["s3://my-bucket/run/000001.csv.gz".to_string()],
        };
        let json = serde_json::to_string(&data).expect("serialize failed");
        assert!(
            json.contains("\"data_source\":\"s3\""),
            "data_source is not 's3': {json}"
        );
    }
}
