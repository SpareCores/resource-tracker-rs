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
    UreqConfig::builder()
        .timeout_global(Some(IMDS_TIMEOUT))
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
#[allow(dead_code)]
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
}
