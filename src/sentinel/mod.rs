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

/// Per-phase timeouts for Sentinel API calls (no global timeout to avoid ureq DNS helper threads).
const API_CONNECT_TIMEOUT_SECS: u64 = 10;
/// Receive-response timeout for Sentinel API calls.
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
            .timeout_connect(Some(Duration::from_secs(API_CONNECT_TIMEOUT_SECS)))
            .timeout_recv_response(Some(Duration::from_secs(API_TIMEOUT_SECS)))
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
    /// Like [`Self::from_env`]'s agent, this uses per-phase timeouts only (no
    /// `timeout_global`) so ureq's DNS resolver stays synchronous and does not
    /// spawn a helper thread per lookup. Upload-specific bounds differ from the
    /// API agent's because S3 PUTs can transfer larger payloads.
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

    // T-NPROC-04: new_upload_agent() must not trigger a DNS resolver helper thread.
    // S3 upload URLs require DNS resolution.  A timeout_global would cause ureq to
    // spawn a helper thread per lookup, which panics under tight cgroup pids.max.
    // The request fails immediately (NXDOMAIN); we only check for absence of panic.
    #[test]
    fn test_upload_agent_does_not_panic_under_nproc_limit() {
        let threads: libc::rlim_t = match std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("Threads:"))
                    .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))
            }) {
            Some(n) => n,
            None => return,
        };
        let mut old = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, std::ptr::null(), &mut old) } != 0 {
            return;
        }
        let tight = libc::rlimit {
            rlim_cur: threads,
            rlim_max: old.rlim_max,
        };
        if unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, &tight, std::ptr::null_mut()) } != 0 {
            return;
        }

        // Use an S3-style hostname that requires DNS resolution.  The request
        // will fail (NXDOMAIN); we only verify no thread spawn panic occurs.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let agent = SentinelClient::new_upload_agent();
            let _ = agent.get("http://s3.amazonaws.com.invalid/").call();
        }));

        unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, &old, std::ptr::null_mut()) };

        assert!(
            result.is_ok(),
            "new_upload_agent() triggered a thread spawn under tight RLIMIT_NPROC; \
             check that timeout_global is not set"
        );
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
