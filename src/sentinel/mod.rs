//! Sentinel API streaming (Section 9).
//!
//! Activation is gated on `SENTINEL_API_TOKEN` being set in the environment.
//! When the token is absent, `SentinelClient::from_env()` returns `None` and
//! no HTTP connections are ever made (T-STR-01).

pub mod run;
pub mod s3;
pub mod upload;

pub use run::{close_run, start_run, RunContext};
pub use upload::{samples_to_csv, BatchUploader};

use ureq::config::Config as UreqConfig;
use std::time::Duration;

/// Default Sentinel API base URL.  Override with `SENTINEL_API_URL`.
const DEFAULT_API_BASE: &str = "https://api.sentinel.sparecores.net";

/// HTTP timeout for Sentinel API calls (not S3 uploads, which are separate).
const API_TIMEOUT_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// SentinelClient
// ---------------------------------------------------------------------------

/// A configured HTTP client for the Sentinel API.
///
/// Constructed only when `SENTINEL_API_TOKEN` is present; every call site
/// gates on `Option<SentinelClient>` so no HTTP is attempted without a token.
#[derive(Clone)]
pub struct SentinelClient {
    pub token:    String,
    pub api_base: String,
    pub agent:    ureq::Agent,
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
        let api_base = std::env::var("SENTINEL_API_URL")
            .unwrap_or_else(|_| DEFAULT_API_BASE.to_string());

        let agent = UreqConfig::builder()
            .timeout_global(Some(Duration::from_secs(API_TIMEOUT_SECS)))
            .build()
            .new_agent();

        Some(Self { token, api_base, agent })
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
        unsafe { std::env::remove_var("SENTINEL_API_TOKEN"); }
        assert!(
            SentinelClient::from_env().is_none(),
            "expected None when SENTINEL_API_TOKEN is unset"
        );
    }

    #[test]
    fn test_empty_token_returns_none() {
        // SAFETY: single-threaded test; no concurrent env access.
        unsafe { std::env::set_var("SENTINEL_API_TOKEN", ""); }
        let result = SentinelClient::from_env();
        unsafe { std::env::remove_var("SENTINEL_API_TOKEN"); }
        assert!(result.is_none(), "expected None when SENTINEL_API_TOKEN is empty string");
    }
}
