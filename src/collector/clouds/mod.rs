use crate::metrics::CloudInfo;
use std::time::Duration;
use ureq::config::Config as UreqConfig;

mod alicloud;
mod aws;
mod azure;
mod gcp;
mod hetzner;
mod ovh;
mod upcloud;

// ---------------------------------------------------------------------------
// Shared IMDS helpers (available to all cloud submodules via `super::`)
// ---------------------------------------------------------------------------

/// Upper bound for each HTTP call made by a vendor probe.
const IMDS_TIMEOUT: Duration = Duration::from_secs(1);

fn new_imds_agent() -> ureq::Agent {
    // Per-phase timeouts instead of timeout_global: timeout_global's thread-
    // creation behavior is an undocumented ureq internal that could change
    // across versions. Per-phase timeouts are a safer, more explicit contract:
    // each phase (connect, recv_response) is bounded independently with no
    // reliance on ureq internals. T-UREQ-01 verifies no extra threads are
    // spawned per request (a regression guard against ureq version bumps).
    UreqConfig::builder()
        .timeout_connect(Some(IMDS_TIMEOUT))
        .timeout_recv_response(Some(IMDS_TIMEOUT))
        .build()
        .new_agent()
}

fn imds_get(agent: &ureq::Agent, url: &str) -> Option<String> {
    imds_get_headers(agent, url, &[])
}

fn imds_get_headers(agent: &ureq::Agent, url: &str, headers: &[(&str, &str)]) -> Option<String> {
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.call()
        .ok()
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Probe orchestration
// ---------------------------------------------------------------------------

/// Precedence order: AWS → GCP → Azure → Hetzner → UpCloud → AliCloud → OVH.
/// To add a new cloud: implement `pub fn probe() -> Option<CloudInfo>` in a new
/// submodule, declare it above, and append it here.
const PROBES: &[fn() -> Option<CloudInfo>] = &[
    aws::probe,
    gcp::probe,
    azure::probe,
    hetzner::probe,
    upcloud::probe,
    alicloud::probe,
    ovh::probe,
];

/// Run vendor probes; parallel when threads are available, serial fallback otherwise.
///
/// Join order follows the `PROBES` precedence list: the first successful probe
/// wins. Each HTTP call uses [`IMDS_TIMEOUT`]. Per-vendor threads are only used
/// when [`crate::thread_util::spawn_named`] succeeds so EAGAIN under tight PID
/// limits falls back to sequential probes on the caller thread.
pub fn probe_cloud() -> CloudInfo {
    let mut handles = Vec::new();
    let mut deferred = Vec::new();

    for &p in PROBES {
        match crate::thread_util::spawn_named("cloud-probe", p) {
            Some(h) => handles.push(h),
            None => deferred.push(p),
        }
    }

    for handle in handles {
        if let Ok(Some(info)) = handle.join() {
            return info;
        }
    }
    for p in deferred {
        if let Some(info) = p() {
            return info;
        }
    }
    CloudInfo::default()
}

/// Spawn a background thread that probes cloud IMDS endpoints.
///
/// Call this **before** the warm-up sleep so probes run **in parallel** with the
/// main thread's warm-up (stateful collector priming + one `interval` sleep).
/// Poll the returned [`Receiver`] with `try_recv()` after warm-up; if probes
/// finished during the sleep the result is waiting immediately.
///
/// Returns `None` when no thread could be created (EAGAIN under PID limits); the
/// caller should treat cloud info as permanently unavailable in that case.
///
/// [`Receiver`]: std::sync::mpsc::Receiver
pub fn spawn_cloud_discovery() -> Option<std::sync::mpsc::Receiver<CloudInfo>> {
    let (tx, rx) = std::sync::mpsc::channel();
    // Drop the JoinHandle: the thread detaches and sends its result via tx.
    // If spawn fails (EAGAIN under PID limits), `?` returns None to the caller.
    crate::thread_util::spawn_named("cloud-discovery", move || {
        let _ = tx.send(probe_cloud());
    })?;
    Some(rx)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // T-CLOUD-01: spawn_cloud_discovery resolves without panic.
    // Each vendor's HTTP calls use IMDS_TIMEOUT; all vendor probes run in parallel.
    #[test]
    fn test_spawn_cloud_discovery_joins_without_panic() {
        let cloud = match spawn_cloud_discovery() {
            Some(rx) => rx.recv().unwrap_or_default(),
            None => probe_cloud(),
        };
        let _cloud = cloud;
        // Result may be default (no cloud) or populated (running on a cloud VM).
        // Either outcome is valid; the test only checks for no panic.
    }

    // -----------------------------------------------------------------------
    // T-UREQ-01: ureq thread-count regression guard
    // -----------------------------------------------------------------------
    //
    // Verifies that requests made with new_imds_agent() (per-phase timeouts)
    // do not spawn extra threads beyond the request thread itself. This guards
    // against ureq version bumps that could introduce new thread creation per
    // request, which would consume PID budget under tight pids.max limits.
    //
    // Linux-only: thread count is read from /proc/self/status.
    // A slow mock server (200 ms response delay) holds the connection open so
    // any short-lived helper thread is still alive when we sample mid-request.
    //
    // NOTE: Testing that timeout_global spawns extra threads (as a negative
    // control) is not practical in unit tests because ureq 3.x does not spawn
    // additional threads for timeout_global on HTTP connections to localhost.
    // The per-phase approach is retained as it provides explicit phase-level
    // bounds without relying on undocumented ureq internals.

    /// Read the `Threads:` field from /proc/self/status.
    #[cfg(target_os = "linux")]
    fn thread_count() -> usize {
        std::fs::read_to_string("/proc/self/status")
            .unwrap_or_default()
            .lines()
            .find(|l| l.starts_with("Threads:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    }

    /// Bind a TCP listener on 127.0.0.1:0, spawn a thread that accepts one
    /// connection, sleeps `delay`, then sends a minimal HTTP 200 response.
    /// Returns the port number.
    #[cfg(target_os = "linux")]
    fn slow_mock_server(delay: std::time::Duration) -> u16 {
        use std::io::Write;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                std::thread::sleep(delay);
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
        });
        port
    }

    // T-UREQ-01: new_imds_agent() requests must not spawn extra threads.
    // The request runs in a background thread; baseline + 1 (that thread itself)
    // is the allowed maximum. If this fails after a ureq version bump, the agent
    // configuration should be reviewed and ureq should be pinned if necessary.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_per_phase_timeout_no_helper_thread() {
        use std::sync::mpsc;
        let port = slow_mock_server(std::time::Duration::from_millis(200));
        let url = format!("http://localhost:{port}");
        let baseline = thread_count();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            let agent = new_imds_agent();
            let _ = agent.get(&url).call();
            let _ = done_tx.send(());
        });
        // Wait for the request to reach the mock server before counting threads.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let during = thread_count();
        done_rx.recv().unwrap();
        assert!(
            during <= baseline + 1,
            "ureq spawned extra thread(s) under per-phase timeout: \
             baseline={baseline} during={during}"
        );
    }
}
