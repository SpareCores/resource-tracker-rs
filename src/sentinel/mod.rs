//! Sentinel API streaming (Section 9).
//!
//! Activation is gated on `SENTINEL_API_TOKEN` being set in the environment.
//! When the token is absent, `SentinelClient::from_env()` returns `None` and
//! no HTTP connections are ever made (T-STR-01).

pub mod run;
pub mod s3;
pub mod upload;

pub use run::{RunContext, close_run, start_run};
pub use upload::{BatchUploader, samples_to_csv};

use std::time::Duration;
use ureq::config::Config as UreqConfig;

/// Default Sentinel API base URL.  Override with `SENTINEL_API_URL`.
const DEFAULT_API_BASE: &str = "https://api.sentinel.sparecores.net";

/// HTTP timeout for Sentinel API calls (not S3 uploads, which are separate).
const API_TIMEOUT_SECS: u64 = 30;

/// Per-phase timeouts for the background S3 upload agent (no global timeout).
const UPLOAD_CONNECT_TIMEOUT_SECS: u64 = 10;
const UPLOAD_RECV_RESPONSE_TIMEOUT_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// SentinelClient
// ---------------------------------------------------------------------------

/// A configured HTTP client for the Sentinel API.
///
/// Constructed only when `SENTINEL_API_TOKEN` is present; every call site
/// gates on `Option<SentinelClient>` so no HTTP is attempted without a token.
#[derive(Clone)]
pub struct SentinelClient {
    pub token: String,
    pub api_base: String,
    pub agent: ureq::Agent,
}

impl SentinelClient {
    /// Return `Some(SentinelClient)` when `SENTINEL_API_TOKEN` is set in the
    /// environment, otherwise `None`.
    ///
    /// `SENTINEL_API_URL` overrides the default API base URL.
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("SENTINEL_API_TOKEN").ok()?;
        if token.is_empty() {
            return None;
        }
        let api_base =
            std::env::var("SENTINEL_API_URL").unwrap_or_else(|_| DEFAULT_API_BASE.to_string());

        let agent = UreqConfig::builder()
            .timeout_global(Some(Duration::from_secs(API_TIMEOUT_SECS)))
            .build()
            .new_agent();

        Some(Self {
            token,
            api_base,
            agent,
        })
    }

    /// HTTP agent for the background S3 upload loop.
    ///
    /// Unlike [`Self::from_env`]'s agent, this has no [`UreqConfig::timeout_global`]
    /// so ureq's DNS resolver uses the synchronous path and does not spawn a helper
    /// thread per lookup (ureq only spawns for resolve when Global/PerCall/Resolve
    /// timeouts apply; connect/recv timeouts do not). Per-phase timeouts still bound
    /// stalled connects and response headers without unbounded hangs.
    pub fn new_upload_agent() -> ureq::Agent {
        UreqConfig::builder()
            .timeout_connect(Some(Duration::from_secs(UPLOAD_CONNECT_TIMEOUT_SECS)))
            .timeout_recv_response(Some(Duration::from_secs(UPLOAD_RECV_RESPONSE_TIMEOUT_SECS)))
            .build()
            .new_agent()
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // T-STR-01: Without SENTINEL_API_TOKEN, no HTTP connection is made.
    //
    // The guard is `SentinelClient::from_env()` returning `None` when the token
    // is absent.  Every HTTP call site in main.rs is gated on `Option<SentinelClient>`,
    // so None here provably prevents all HTTP connections.
    #[test]
    fn test_no_token_returns_none() {
        // SAFETY: single-threaded test; no concurrent env access.
        unsafe {
            std::env::remove_var("SENTINEL_API_TOKEN");
        }
        assert!(
            SentinelClient::from_env().is_none(),
            "expected None when SENTINEL_API_TOKEN is unset"
        );
    }

    #[test]
    fn test_empty_token_returns_none() {
        // SAFETY: single-threaded test; no concurrent env access.
        unsafe {
            std::env::set_var("SENTINEL_API_TOKEN", "");
        }
        let result = SentinelClient::from_env();
        unsafe {
            std::env::remove_var("SENTINEL_API_TOKEN");
        }
        assert!(
            result.is_none(),
            "expected None when SENTINEL_API_TOKEN is empty string"
        );
    }

    // T-STR-07: a non-empty token returns Some with the correct token and default URL.
    #[test]
    fn test_valid_token_returns_some_with_defaults() {
        // SAFETY: single-threaded test; no concurrent env access.
        unsafe {
            std::env::set_var("SENTINEL_API_TOKEN", "my-test-token");
        }
        unsafe {
            std::env::remove_var("SENTINEL_API_URL");
        }
        let result = SentinelClient::from_env();
        unsafe {
            std::env::remove_var("SENTINEL_API_TOKEN");
        }
        let client = result.expect("expected Some when SENTINEL_API_TOKEN is non-empty");
        assert_eq!(client.token, "my-test-token");
        assert_eq!(client.api_base, DEFAULT_API_BASE);
    }

    // T-STR-08: SENTINEL_API_URL overrides the default API base URL.
    #[test]
    fn test_api_url_env_override() {
        // SAFETY: single-threaded test; no concurrent env access.
        unsafe {
            std::env::set_var("SENTINEL_API_TOKEN", "tok");
        }
        unsafe {
            std::env::set_var("SENTINEL_API_URL", "http://localhost:9999");
        }
        let result = SentinelClient::from_env();
        unsafe {
            std::env::remove_var("SENTINEL_API_TOKEN");
        }
        unsafe {
            std::env::remove_var("SENTINEL_API_URL");
        }
        let client = result.expect("expected Some when token is set");
        assert_eq!(client.api_base, "http://localhost:9999");
    }
}
