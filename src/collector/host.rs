use crate::metrics::{GpuMetrics, HostInfo};

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
    let ret = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
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
    content.lines().for_each(|line| {
        if line.starts_with("processor") {
            count += 1;
        } else if line.starts_with("model name")
            && model.is_none()
            && let Some((_, val)) = line.split_once(':')
        {
            model = Some(val.trim().to_string());
        }
    });
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
            let sectors: u64 = std::fs::read_to_string(format!("/sys/block/{}/size", name))
                .ok()?
                .trim()
                .parse()
                .ok()?;
            Some(sectors as f64 * 512.0 / 1_000_000_000.0)
        })
        .sum();
    if total > 0.0 { Some(total) } else { None }
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
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake_gpu(name: &str, vram_total_bytes: u64) -> GpuMetrics {
        GpuMetrics {
            uuid: "test-uuid".to_string(),
            name: name.to_string(),
            device_type: "GPU".to_string(),
            host_id: "0".to_string(),
            detail: HashMap::new(),
            utilization_pct: 0.0,
            vram_total_bytes,
            vram_used_bytes: 0,
            vram_used_pct: 0.0,
            temperature_celsius: 0,
            power_watts: 0.0,
            frequency_mhz: 0,
            core_count: None,
        }
    }

    // T-HOST-01: no-GPU path returns None for all GPU fields.
    #[test]
    fn test_collect_host_info_no_gpus_returns_none_gpu_fields() {
        let info = collect_host_info(&[]);
        assert!(
            info.host_gpu_model.is_none(),
            "host_gpu_model must be None when no GPUs"
        );
        assert!(
            info.host_gpu_count.is_none(),
            "host_gpu_count must be None when no GPUs"
        );
        assert!(
            info.host_gpu_vram_mib.is_none(),
            "host_gpu_vram_mib must be None when no GPUs"
        );
    }

    // T-HOST-02: one GPU sets model, count, and VRAM correctly.
    #[test]
    fn test_collect_host_info_one_gpu_sets_fields() {
        // 8 GiB = 8192 MiB
        let gpu = fake_gpu("TestGPU X100", 8 * 1_073_741_824);
        let info = collect_host_info(&[gpu]);
        assert_eq!(info.host_gpu_model.as_deref(), Some("TestGPU X100"));
        assert_eq!(info.host_gpu_count, Some(1));
        assert_eq!(info.host_gpu_vram_mib, Some(8192));
    }

    // T-HOST-03: two GPUs sum VRAM and report count = 2.
    #[test]
    fn test_collect_host_info_two_gpus_sums_vram() {
        let gpu1 = fake_gpu("GPU A", 4 * 1_073_741_824); // 4 GiB
        let gpu2 = fake_gpu("GPU B", 4 * 1_073_741_824); // 4 GiB
        let info = collect_host_info(&[gpu1, gpu2]);
        assert_eq!(info.host_gpu_count, Some(2));
        assert_eq!(info.host_gpu_vram_mib, Some(8192)); // 8 GiB total
    }

    // T-HOST-04: hostname is non-empty on any standard Linux host.
    #[test]
    fn test_collect_host_info_hostname_present() {
        let info = collect_host_info(&[]);
        assert!(
            info.host_name
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "host_name should be a non-empty string on a standard Linux host"
        );
    }

    // T-HOST-05: host_vcpus is present and positive.
    #[test]
    fn test_collect_host_info_vcpus_positive() {
        let info = collect_host_info(&[]);
        let vcpus = info.host_vcpus.unwrap_or(0);
        assert!(
            vcpus > 0,
            "host_vcpus must be > 0, got {:?}",
            info.host_vcpus
        );
    }
}
