use crate::metrics::{CloudInfo, GpuMetrics, HostInfo};
use std::time::Duration;
use ureq::config::Config as UreqConfig;

// ---------------------------------------------------------------------------
// Host discovery helpers
// ---------------------------------------------------------------------------

fn read_host_id() -> Option<String> {
    // AWS instances expose a stable asset tag at this DMI path.
    let tag = std::fs::read_to_string("/sys/class/dmi/id/board_asset_tag")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "Not Specified");
    if tag.is_some() {
        return tag;
    }
    // Generic fallback: systemd machine-id.
    std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_host_name() -> Option<String> {
    let mut buf = vec![0u8; 256];
    let ret = unsafe {
        libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len())
    };
    if ret != 0 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..len].to_vec())
        .ok()
        .filter(|s| !s.is_empty())
}

/// First non-loopback IPv4 address discovered via `getifaddrs(3)`.
fn read_host_ip() -> Option<String> {
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return None;
        }
        let mut result: Option<String> = None;
        let mut ptr = ifap;
        while !ptr.is_null() {
            let ifa = &*ptr;
            if !ifa.ifa_addr.is_null() {
                let family = (*ifa.ifa_addr).sa_family as i32;
                if family == libc::AF_INET {
                    let addr = ifa.ifa_addr as *const libc::sockaddr_in;
                    // s_addr is stored in network byte order in memory.
                    // to_ne_bytes() on x86_64 returns the memory bytes directly,
                    // which are already in dotted-decimal order.
                    let bytes = (*addr).sin_addr.s_addr.to_ne_bytes();
                    if bytes[0] != 127 {
                        result = Some(format!(
                            "{}.{}.{}.{}",
                            bytes[0], bytes[1], bytes[2], bytes[3]
                        ));
                        break;
                    }
                }
            }
            ptr = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
        result
    }
}

/// Returns (vcpu_count, cpu_model) by parsing `/proc/cpuinfo` once.
fn read_vcpus_and_model() -> (Option<u32>, Option<String>) {
    let content = match std::fs::read_to_string("/proc/cpuinfo") {
        Ok(c) => c,
        Err(_) => return (None, None),
    };
    let mut count: u32 = 0;
    let mut model: Option<String> = None;
    for line in content.lines() {
        if line.starts_with("processor") {
            count += 1;
        } else if line.starts_with("model name") && model.is_none() {
            if let Some(val) = line.splitn(2, ':').nth(1) {
                model = Some(val.trim().to_string());
            }
        }
    }
    let vcpus = if count > 0 { Some(count) } else { None };
    (vcpus, model)
}

/// `MemTotal` from `/proc/meminfo` converted to MiB.
fn read_memory_mib() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in content.lines() {
        if line.starts_with("MemTotal:") {
            // Value is in KiB; divide by 1024 to get MiB.
            let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
            return Some(kib / 1024);
        }
    }
    None
}

/// Sum of all non-loop, non-ram block device capacities in GB.
fn read_storage_gb() -> Option<f64> {
    let entries = std::fs::read_dir("/sys/block").ok()?;
    let total: f64 = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("loop") || name.starts_with("ram") {
                return None;
            }
            // /sys/block/<dev>/size reports 512-byte sectors.
            let sectors: u64 = std::fs::read_to_string(
                format!("/sys/block/{}/size", name),
            )
            .ok()?
            .trim()
            .parse()
            .ok()?;
            Some(sectors as f64 * 512.0 / 1_000_000_000.0)
        })
        .sum();
    if total > 0.0 {
        Some(total)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Public host collection
// ---------------------------------------------------------------------------

/// Collect all host-level metadata. Fast (no network I/O).
/// Takes a snapshot of GPU info already gathered so GPU-derived fields
/// (`host_gpu_model`, `host_gpu_count`, `host_gpu_vram_mib`) can be populated
/// without re-querying the GPU driver.
pub fn collect_host_info(gpus: &[GpuMetrics]) -> HostInfo {
    let (host_vcpus, host_cpu_model) = read_vcpus_and_model();

    let (host_gpu_model, host_gpu_count, host_gpu_vram_mib) = if gpus.is_empty() {
        (None, None, None)
    } else {
        let model = Some(gpus[0].name.clone());
        let count = u32::try_from(gpus.len()).ok();
        let vram_mib: u64 = gpus.iter().map(|g| g.vram_total_bytes / 1_048_576).sum();
        (model, count, Some(vram_mib))
    };

    HostInfo {
        host_id: read_host_id(),
        host_name: read_host_name(),
        host_ip: read_host_ip(),
        host_allocation: None, // heuristic TBD
        host_vcpus,
        host_cpu_model,
        host_memory_mib: read_memory_mib(),
        host_gpu_model,
        host_gpu_count,
        host_gpu_vram_mib,
        host_storage_gb: read_storage_gb(),
    }
}

// ---------------------------------------------------------------------------
// Cloud discovery (background thread, non-blocking)
// ---------------------------------------------------------------------------

const IMDS_TIMEOUT: Duration = Duration::from_secs(2);

/// Make a GET request and return the trimmed response body, or `None` on error.
fn imds_get(agent: &ureq::Agent, url: &str) -> Option<String> {
    agent
        .get(url)
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Attempt AWS IMDS. Returns a populated `CloudInfo` if this is an AWS host,
/// otherwise returns `CloudInfo::default()`.
fn probe_aws() -> CloudInfo {
    let config = UreqConfig::builder()
        .timeout_global(Some(IMDS_TIMEOUT))
        .build();
    let agent = config.new_agent();
    const BASE: &str = "http://169.254.169.254";

    // Probe the metadata root; if it fails, this is not an AWS host.
    if agent
        .get(&format!("{}/latest/meta-data/", BASE))
        .call()
        .is_err()
    {
        return CloudInfo::default();
    }

    // AWS confirmed. Fetch the remaining fields individually.
    let cloud_region_id = imds_get(
        &agent,
        &format!("{}/latest/meta-data/placement/region", BASE),
    );
    let cloud_zone_id = imds_get(
        &agent,
        &format!("{}/latest/meta-data/placement/availability-zone", BASE),
    );
    let cloud_instance_type = imds_get(
        &agent,
        &format!("{}/latest/meta-data/instance-type", BASE),
    );
    // Account ID lives inside a JSON document; extract the AccountId field.
    let cloud_account_id = imds_get(
        &agent,
        &format!("{}/latest/meta-data/identity-credentials/ec2/info", BASE),
    )
    .and_then(|body| {
        // Simple extraction without pulling in a JSON parser: find "AccountId":"..."
        let marker = "\"AccountId\":\"";
        let start = body.find(marker)? + marker.len();
        let end = body[start..].find('"')? + start;
        Some(body[start..end].to_string())
    });

    CloudInfo {
        cloud_vendor_id: Some("aws".to_string()),
        cloud_account_id,
        cloud_region_id,
        cloud_zone_id,
        cloud_instance_type,
    }
}

/// Probe GCP IMDS. Returns `true` if this is a GCP host.
fn probe_gcp() -> bool {
    let config = UreqConfig::builder()
        .timeout_global(Some(IMDS_TIMEOUT))
        .build();
    let agent = config.new_agent();
    agent
        .get("http://metadata.google.internal/computeMetadata/v1/")
        .header("Metadata-Flavor", "Google")
        .call()
        .is_ok()
}

/// Probe Azure IMDS. Returns `true` if this is an Azure host.
fn probe_azure() -> bool {
    let config = UreqConfig::builder()
        .timeout_global(Some(IMDS_TIMEOUT))
        .build();
    let agent = config.new_agent();
    agent
        .get("http://169.254.169.254/metadata/instance?api-version=2021-02-01")
        .header("Metadata", "true")
        .call()
        .is_ok()
}

/// Probe all three cloud providers in parallel and return the first match.
/// Each probe has a ≤ 2-second timeout; total wall time is bounded by the
/// slowest thread (at most 2 seconds when no provider responds).
fn probe_cloud() -> CloudInfo {
    let aws   = std::thread::spawn(probe_aws);
    let gcp   = std::thread::spawn(probe_gcp);
    let azure = std::thread::spawn(probe_azure);

    let aws_result = aws.join().unwrap_or_default();
    if aws_result.cloud_vendor_id.is_some() {
        let _ = gcp.join();
        let _ = azure.join();
        return aws_result;
    }

    if gcp.join().unwrap_or(false) {
        let _ = azure.join();
        return CloudInfo {
            cloud_vendor_id: Some("gcp".to_string()),
            ..CloudInfo::default()
        };
    }

    if azure.join().unwrap_or(false) {
        return CloudInfo {
            cloud_vendor_id: Some("azure".to_string()),
            ..CloudInfo::default()
        };
    }

    CloudInfo::default()
}

/// Spawn a background thread that probes cloud IMDS endpoints.
/// The caller should join the handle after the warm-up sleep so that cloud
/// discovery runs concurrently with the first sample interval.
pub fn spawn_cloud_discovery() -> std::thread::JoinHandle<CloudInfo> {
    std::thread::spawn(probe_cloud)
}
