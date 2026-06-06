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
    // No timeout_global: ureq spawns a DNS resolver helper thread when any
    // global/per-call/resolve timeout is set. Per-phase timeouts bound each
    // phase without triggering that spawn, avoiding EAGAIN under PID limits.
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
/// Join the handle **after** warm-up to read results; if probes finished during
/// sleep, `join` returns immediately.
///
/// Returns `None` when no thread could be created; the caller should invoke
/// [`probe_cloud`] on the main thread after warm-up instead.
pub fn spawn_cloud_discovery() -> Option<std::thread::JoinHandle<CloudInfo>> {
    crate::thread_util::spawn_named("cloud-discovery", probe_cloud)
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
            Some(handle) => handle.join().expect("cloud discovery thread panicked"),
            None => probe_cloud(),
        };
        let _cloud = cloud;
        // Result may be default (no cloud) or populated (running on a cloud VM).
        // Either outcome is valid; the test only checks for no panic.
    }

    /// Tighten RLIMIT_NPROC to the current thread count so the next OS thread
    /// spawn in this process fails with EAGAIN.  Returns the old limit for
    /// restoration, or None if /proc or prlimit is unavailable (skip caller test).
    fn tighten_nproc() -> Option<libc::rlimit> {
        let threads: libc::rlim_t = std::fs::read_to_string("/proc/self/status")
            .ok()?
            .lines()
            .find(|l| l.starts_with("Threads:"))
            .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))?;
        let mut old = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, std::ptr::null(), &mut old) } != 0 {
            return None;
        }
        let tight = libc::rlimit {
            rlim_cur: threads,
            rlim_max: old.rlim_max,
        };
        if unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, &tight, std::ptr::null_mut()) } != 0 {
            return None;
        }
        Some(old)
    }

    fn restore_nproc(old: libc::rlimit) {
        unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, &old, std::ptr::null_mut()) };
    }

    // T-NPROC-02: new_imds_agent() must not trigger a DNS resolver helper thread.
    // ureq spawns a helper thread per DNS lookup when timeout_global (or any
    // global/per-call/resolve timeout) is set.  The GCP probe uses the hostname
    // metadata.google.internal which requires DNS resolution, so this test would
    // panic under tight RLIMIT_NPROC if timeout_global were present on the agent.
    // The request fails immediately (NXDOMAIN on non-GCP hosts); we only check
    // for absence of panic.
    #[test]
    fn test_imds_agent_does_not_panic_under_nproc_limit() {
        let Some(old) = tighten_nproc() else { return };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let agent = new_imds_agent();
            let _ = agent.get("http://metadata.google.internal/").call();
        }));

        restore_nproc(old);

        assert!(
            result.is_ok(),
            "new_imds_agent() triggered a thread spawn under tight RLIMIT_NPROC; \
             check that timeout_global is not set on the IMDS agent"
        );
    }

    // T-NPROC-03: probe_cloud() serial fallback must complete without panic when
    // no threads can be spawned.  All spawn_named calls return None so probes
    // run serially on the caller thread.  On a non-cloud host every probe returns
    // None; CloudInfo::default() is the result.
    #[test]
    fn test_probe_cloud_serial_fallback_does_not_panic() {
        let Some(old) = tighten_nproc() else { return };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(probe_cloud));

        restore_nproc(old);

        assert!(
            result.is_ok(),
            "probe_cloud() panicked under tight RLIMIT_NPROC"
        );
    }
}
