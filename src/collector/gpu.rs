use crate::metrics::GpuMetrics;
use libamdgpu_top::{
    AMDGPU::{GpuMetrics as AmdHwMetrics, MetricsInfo},
    stat::GpuActivity,
    DevicePath,
};
use nvml_wrapper::{
    enum_wrappers::device::{Clock, TemperatureSensor},
    enums::device::UsedGpuMemory,
    Nvml,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Collects per-GPU metrics from NVIDIA (via NVML) and AMD (via libamdgpu_top).
///
/// Both backends load their native libraries at runtime:
/// - NVML via `libloading` (`libnvidia-ml.so`) — absent on non-NVIDIA hosts.
/// - libdrm via the `libdrm_dynamic_loading` feature — absent on non-AMD hosts.
///
/// On a CPU-only host `collect()` returns an empty Vec with no error.
pub struct GpuCollector {
    nvml: Option<Nvml>,
}

impl GpuCollector {
    pub fn new() -> Self {
        Self {
            nvml: Nvml::init().ok(),
        }
    }

    pub fn collect(&self) -> Result<Vec<GpuMetrics>> {
        let mut metrics = Vec::new();
        self.collect_nvidia(&mut metrics);
        self.collect_amd(&mut metrics);
        Ok(metrics)
    }

    /// Return `(process_gpu_vram_mib, process_gpu_utilized)` for the given PIDs.
    ///
    /// `pids` is the tracked process tree (root + descendants) as u32 values.
    ///
    /// NVIDIA: queries NVML running-compute and running-graphics process lists
    /// for each device; sums `used_gpu_memory` for matched PIDs.
    ///
    /// AMD: reads `/proc/{pid}/fdinfo` for each PID, parses `drm-memory-vram`
    /// and `drm-pdev` from DRM fdinfo entries (Linux kernel >= 5.17), and
    /// matches against known AMD device PCI addresses.
    ///
    /// Returns `(None, None)` when no GPU is present on the host.
    /// Returns `(Some(0.0), Some(0))` when a GPU is present but the process
    /// tree has no allocations.
    pub fn process_gpu_info(&self, pids: &[u32]) -> (Option<f64>, Option<u32>) {
        let mut total_vram_bytes: u64 = 0;
        let mut n_utilized: u32 = 0;
        let mut any_gpu = false;

        // --- NVIDIA via NVML -------------------------------------------------
        if let Some(ref nvml) = self.nvml {
            any_gpu = true;
            let pid_set: HashSet<u32> = pids.iter().copied().collect();
            let count = nvml.device_count().unwrap_or(0);

            (0..count).for_each(|i| {
                let Ok(device) = nvml.device_by_index(i) else { return };
                let procs: Vec<_> = device
                    .running_compute_processes()
                    .unwrap_or_default()
                    .into_iter()
                    .chain(device.running_graphics_processes().unwrap_or_default())
                    .collect();

                let mut device_vram: u64 = 0;
                let mut found = false;
                procs
                    .iter()
                    .filter(|p| pid_set.contains(&p.pid))
                    .for_each(|p| {
                        found = true;
                        if let UsedGpuMemory::Used(bytes) = p.used_gpu_memory {
                            device_vram += bytes;
                        }
                    });

                if found {
                    n_utilized += 1;
                    total_vram_bytes += device_vram;
                }
            });
        }

        // --- AMD via /proc/pid/fdinfo ----------------------------------------
        // DRM fdinfo (kernel >= 5.17): each open DRM fd exposes drm-memory-vram
        // and drm-pdev so we can attribute VRAM per process and per device.
        if std::path::Path::new("/sys/module/amdgpu").exists() {
            any_gpu = true;

            // Collect PCI addresses of all known AMD devices (lowercase for
            // case-insensitive comparison with kernel fdinfo drm-pdev values).
            let amd_pci_addrs: HashSet<String> = DevicePath::get_device_path_list()
                .into_iter()
                .map(|dp| format!("{}", dp.pci).to_lowercase())
                .collect();

            // Track which PCI addresses have any VRAM allocated by the tree.
            let mut utilized_pcis: HashSet<String> = HashSet::new();

            pids.iter().for_each(|&pid| {
                let fdinfo_dir = format!("/proc/{pid}/fdinfo");
                let Ok(entries) = std::fs::read_dir(&fdinfo_dir) else { return };
                entries.filter_map(|e| e.ok()).for_each(|entry| {
                    let Ok(content) = std::fs::read_to_string(entry.path()) else { return };

                    // Only process amdgpu DRM file descriptors.
                    if !content
                        .lines()
                        .any(|l| l.starts_with("drm-driver:") && l.contains("amdgpu"))
                    {
                        return;
                    }

                    // Match drm-pdev against our known AMD GPU list.
                    let pdev = content
                        .lines()
                        .find(|l| l.starts_with("drm-pdev:"))
                        .and_then(|l| l.split_whitespace().nth(1))
                        .map(|s| s.to_lowercase());
                    let Some(pdev) = pdev else { return };
                    if !amd_pci_addrs.contains(&pdev) { return; }

                    // Parse drm-memory-vram (value in KiB, unit label "KiB").
                    content
                        .lines()
                        .find(|l| l.starts_with("drm-memory-vram:"))
                        .and_then(|l| l.split_whitespace().nth(1))
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|kib| {
                            total_vram_bytes += kib * 1024;
                            utilized_pcis.insert(pdev.clone());
                        });
                });
            });

            n_utilized += u32::try_from(utilized_pcis.len()).unwrap_or(0);
        }

        if !any_gpu {
            return (None, None);
        }

        let vram_mib = total_vram_bytes as f64 / 1_048_576.0;
        (Some(vram_mib), Some(n_utilized))
    }

    /// Return `(process_gpu_vram_mib, process_gpu_utilized)` summed across ALL
    /// GPU processes on the host (no PID filter).  Used when tracking is not
    /// scoped to a specific PID so the full system-wide GPU allocation is
    /// reported in the `process_` CSV columns.
    ///
    /// NVIDIA: sums `used_gpu_memory` for every running compute and graphics
    /// process across all devices; counts each device that has at least one
    /// process as "utilized".
    ///
    /// AMD: reads `mem_info_vram_used` from sysfs for each device (the kernel
    /// already provides the system-wide VRAM used value there).
    ///
    /// Returns `(None, None)` when no GPU is present on the host.
    pub fn all_gpu_process_info(&self) -> (Option<f64>, Option<u32>) {
        let mut total_vram_bytes: u64 = 0;
        let mut n_utilized: u32 = 0;
        let mut any_gpu = false;

        // --- NVIDIA via NVML -------------------------------------------------
        if let Some(ref nvml) = self.nvml {
            any_gpu = true;
            let count = nvml.device_count().unwrap_or(0);

            (0..count).for_each(|i| {
                let Ok(device) = nvml.device_by_index(i) else { return };
                let procs: Vec<_> = device
                    .running_compute_processes()
                    .unwrap_or_default()
                    .into_iter()
                    .chain(device.running_graphics_processes().unwrap_or_default())
                    .collect();

                if procs.is_empty() {
                    return;
                }
                n_utilized += 1;
                procs.iter().for_each(|p| {
                    if let UsedGpuMemory::Used(bytes) = p.used_gpu_memory {
                        total_vram_bytes += bytes;
                    }
                });
            });
        }

        // --- AMD via sysfs ---------------------------------------------------
        if std::path::Path::new("/sys/module/amdgpu").exists() {
            any_gpu = true;

            DevicePath::get_device_path_list().into_iter().for_each(|dp| {
                let used = read_sysfs_u64(dp.sysfs_path.join("mem_info_vram_used"));
                if used > 0 {
                    total_vram_bytes += used;
                    n_utilized += 1;
                }
            });
        }

        if !any_gpu {
            return (None, None);
        }

        let vram_mib = total_vram_bytes as f64 / 1_048_576.0;
        (Some(vram_mib), Some(n_utilized))
    }

    // -----------------------------------------------------------------------
    // NVIDIA — NVML runtime-loaded via libloading
    // -----------------------------------------------------------------------

    fn collect_nvidia(&self, out: &mut Vec<GpuMetrics>) {
        let Some(ref nvml) = self.nvml else { return };

        let count = nvml.device_count().unwrap_or(0);
        let driver_version = nvml.sys_driver_version().unwrap_or_default();

        for i in 0..count {
            let Ok(device) = nvml.device_by_index(i) else { continue };

            let name = device.name().unwrap_or_default();
            let uuid = device.uuid().unwrap_or_else(|_| format!("nvidia-{i}"));

            let utilization_pct = device
                .utilization_rates()
                .map(|u| u.gpu as f64)
                .unwrap_or(0.0);

            let memory = device.memory_info().ok();
            let vram_total_bytes = memory.as_ref().map(|m| m.total).unwrap_or(0);
            let vram_used_bytes = memory.as_ref().map(|m| m.used).unwrap_or(0);
            let vram_used_pct = if vram_total_bytes > 0 {
                vram_used_bytes as f64 / vram_total_bytes as f64 * 100.0
            } else {
                0.0
            };

            let temperature_celsius = device
                .temperature(TemperatureSensor::Gpu)
                .unwrap_or(0);

            // NVML reports power in milliwatts; convert to watts.
            let power_watts = device
                .power_usage()
                .map(|mw| mw as f64 / 1000.0)
                .unwrap_or(0.0);

            let frequency_mhz = device.clock_info(Clock::Graphics).unwrap_or(0);

            let mut detail: HashMap<String, String> = HashMap::new();
            if !driver_version.is_empty() {
                detail.insert("driver_version".to_string(), driver_version.clone());
            }
            if let Ok(pci) = device.pci_info() {
                detail.insert("pci_bus_id".to_string(), pci.bus_id);
            }

            out.push(GpuMetrics {
                uuid,
                name,
                device_type: "GPU".to_string(),
                host_id: i.to_string(),
                detail,
                utilization_pct,
                vram_total_bytes,
                vram_used_bytes,
                vram_used_pct,
                temperature_celsius,
                power_watts,
                frequency_mhz,
                core_count: None,
            });
        }
    }

    // -----------------------------------------------------------------------
    // AMD — libdrm runtime-loaded via libdrm_dynamic_loading feature.
    // Dynamic metrics are read from the hardware gpu_metrics sysfs file;
    // VRAM is read from per-device sysfs attributes (no DRM ioctl needed).
    // -----------------------------------------------------------------------

    fn collect_amd(&self, out: &mut Vec<GpuMetrics>) {
        // libamdgpu_top panics when the amdgpu kernel module is not loaded.
        // `catch_unwind` cannot help here because the release profile uses
        // `panic = "abort"`.  Guard by checking the module's sysfs entry
        // before calling into the library at all.
        if !std::path::Path::new("/sys/module/amdgpu").exists() {
            return;
        }

        DevicePath::get_device_path_list().into_iter().for_each(|dp| {
            // VRAM: standard AMD GPU sysfs attributes, always available.
            let vram_total_bytes = read_sysfs_u64(dp.sysfs_path.join("mem_info_vram_total"));
            let vram_used_bytes = read_sysfs_u64(dp.sysfs_path.join("mem_info_vram_used"));
            let vram_used_pct = if vram_total_bytes > 0 {
                vram_used_bytes as f64 / vram_total_bytes as f64 * 100.0
            } else {
                0.0
            };

            // Hardware gpu_metrics file: preferred source for dynamic metrics.
            let hw = AmdHwMetrics::get_from_sysfs_path(&dp.sysfs_path).ok();

            let utilization_pct = hw
                .as_ref()
                .and_then(|m: &AmdHwMetrics| m.get_average_gfx_activity())
                .map(|u| u as f64)
                .unwrap_or_else(|| {
                    // Fallback: sysfs gpu_busy_percent (older kernels / APUs).
                    GpuActivity::get_from_sysfs(&dp.sysfs_path)
                        .gfx
                        .unwrap_or(0) as f64
                });

            let frequency_mhz: u32 = hw
                .as_ref()
                .and_then(|m: &AmdHwMetrics| m.get_average_gfxclk_frequency())
                .map(u32::from)
                .unwrap_or(0);

            // get_temperature_edge() returns millidegrees on some ASICs.
            let temperature_celsius: u32 = hw
                .as_ref()
                .and_then(|m: &AmdHwMetrics| m.get_temperature_edge())
                .map(|t| u32::from(if t > 1000 { t / 1000 } else { t }))
                .unwrap_or(0);

            // get_average_socket_power() returns whole watts directly.
            let power_watts = hw
                .as_ref()
                .and_then(|m: &AmdHwMetrics| m.get_average_socket_power())
                .map(|w| w as f64)
                .unwrap_or(0.0);

            // AMD GPUs have no stable UUID; use PCI bus address instead.
            let host_id = format!("{}", dp.pci);

            let mut detail: HashMap<String, String> = HashMap::new();
            detail.insert("pci_bus".to_string(), host_id.clone());
            if let Some(rocm) = libamdgpu_top::get_rocm_version() {
                detail.insert("rocm_version".to_string(), format!("{rocm:?}"));
            }

            out.push(GpuMetrics {
                uuid: host_id.clone(),
                name: dp.device_name.clone(),
                device_type: "GPU".to_string(),
                host_id,
                detail,
                utilization_pct,
                vram_total_bytes,
                vram_used_bytes,
                vram_used_pct,
                temperature_celsius,
                power_watts,
                frequency_mhz,
                core_count: None,
            });
        });
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // T-GPU-P1: process_gpu_info with an empty PID list must return (None, None)
    // on a CPU-only host, or (Some(0.0), Some(0)) on a GPU host -- never panic,
    // and always return matching Some/None for both fields.
    #[test]
    fn test_process_gpu_info_empty_pids_consistent() {
        let collector = GpuCollector::new();
        let (vram, utilized) = collector.process_gpu_info(&[]);
        match (vram, utilized) {
            (None, None) => {}  // CPU-only host
            (Some(v), Some(u)) => {
                assert_eq!(v, 0.0, "empty PID list must produce 0.0 VRAM");
                assert_eq!(u, 0,   "empty PID list must produce 0 utilized GPUs");
            }
            _ => panic!("vram_mib and gpu_utilized must both be Some or both be None"),
        }
    }

    // T-GPU-P2: process_gpu_info with the current process PID must not panic
    // and must return a consistent shape: (None, None) on CPU-only hosts, or
    // (Some(v), Some(u)) with v >= 0.0 on GPU hosts.
    #[test]
    fn test_process_gpu_info_real_pid_does_not_panic() {
        let collector = GpuCollector::new();
        let pid = std::process::id();
        let (vram, utilized) = collector.process_gpu_info(&[pid]);
        match (vram, utilized) {
            (None, None) => {}
            (Some(v), Some(u)) => {
                assert!(v >= 0.0, "vram_mib must be non-negative, got {v}");
                let _ = u; // test process is unlikely to hold GPU allocations
            }
            _ => panic!("vram_mib and gpu_utilized must both be Some or both be None"),
        }
    }

    // T-GPU-P3: on a CPU-only host (no NVML, no /sys/module/amdgpu),
    // any PID list must return (None, None).  Skipped on GPU hosts.
    #[test]
    fn test_process_gpu_info_no_gpu_returns_none() {
        let nvml_unavailable = Nvml::init().is_err();
        let amd_absent = !std::path::Path::new("/sys/module/amdgpu").exists();
        if !nvml_unavailable || !amd_absent {
            // Host has a GPU; this test is not applicable.
            return;
        }
        let collector = GpuCollector::new();
        let (vram, utilized) = collector.process_gpu_info(&[1, 2, 3]);
        assert_eq!((vram, utilized), (None, None),
            "CPU-only host must return (None, None) for any PID list");
    }

    // T-GPU-A1: all_gpu_process_info() must not panic and must return a
    // consistent shape on any host: (None, None) on CPU-only, or
    // (Some(v), Some(u)) with v >= 0.0 on GPU hosts.
    #[test]
    fn test_all_gpu_process_info_consistent() {
        let collector = GpuCollector::new();
        let (vram, utilized) = collector.all_gpu_process_info();
        match (vram, utilized) {
            (None, None) => {}  // CPU-only host
            (Some(v), Some(u)) => {
                assert!(v >= 0.0, "vram_mib must be non-negative, got {v}");
                let _ = u;
            }
            _ => panic!("vram_mib and gpu_utilized must both be Some or both be None"),
        }
    }

    // T-GPU-A2: on a CPU-only host, all_gpu_process_info() must return (None, None).
    // Skipped on GPU hosts.
    #[test]
    fn test_all_gpu_process_info_no_gpu_returns_none() {
        let nvml_unavailable = Nvml::init().is_err();
        let amd_absent = !std::path::Path::new("/sys/module/amdgpu").exists();
        if !nvml_unavailable || !amd_absent {
            return;
        }
        let collector = GpuCollector::new();
        let result = collector.all_gpu_process_info();
        assert_eq!(result, (None, None),
            "CPU-only host must return (None, None)");
    }

    // T-GPU-A3: on a GPU host, all_gpu_process_info() must return Some for both
    // fields, with vram_mib >= 0.0.  Skipped on CPU-only hosts.
    #[test]
    fn test_all_gpu_process_info_gpu_host_returns_some() {
        let nvml_available = Nvml::init().is_ok();
        let amd_present = std::path::Path::new("/sys/module/amdgpu").exists();
        if !nvml_available && !amd_present {
            return;  // CPU-only host; not applicable
        }
        let collector = GpuCollector::new();
        let (vram, utilized) = collector.all_gpu_process_info();
        assert!(vram.is_some(),    "GPU host: vram_mib must be Some, got None");
        assert!(utilized.is_some(), "GPU host: gpu_utilized must be Some, got None");
        assert!(vram.unwrap() >= 0.0, "GPU host: vram_mib must be non-negative");
    }

    // T-GPU-A4: all_gpu_process_info() must return >= the vram reported for an
    // empty PID list via process_gpu_info() (which returns Some(0.0) on GPU hosts).
    // Verifies that the no-PID path is strictly broader than a zero-PID-set query.
    #[test]
    fn test_all_gpu_process_info_ge_empty_pid_query() {
        let nvml_available = Nvml::init().is_ok();
        let amd_present = std::path::Path::new("/sys/module/amdgpu").exists();
        if !nvml_available && !amd_present {
            return;
        }
        let collector = GpuCollector::new();
        let (all_vram, _) = collector.all_gpu_process_info();
        let (pid_vram, _) = collector.process_gpu_info(&[]);
        // process_gpu_info(&[]) returns Some(0.0) on a GPU host; all_gpu_process_info
        // must return >= 0.0 (can be 0.0 if no GPU processes are running).
        if let (Some(av), Some(pv)) = (all_vram, pid_vram) {
            assert!(av >= pv,
                "all_gpu_process_info vram ({av}) must be >= process_gpu_info([]) vram ({pv})");
        }
    }

    // T-GPU-C1: collect() does not panic and returns Ok on any host.
    #[test]
    fn test_gpu_collect_does_not_panic() {
        let collector = GpuCollector::new();
        let result = collector.collect();
        assert!(result.is_ok(), "collect() must return Ok on any host, got: {:?}", result.err());
    }

    // T-GPU-C2: all returned GpuMetrics entries have non-empty uuid, name, and device_type.
    #[test]
    fn test_gpu_collect_identity_fields_nonempty() {
        let collector = GpuCollector::new();
        let gpus = collector.collect().expect("collect() failed");
        gpus.iter().for_each(|g| {
            assert!(!g.uuid.is_empty(),        "uuid must not be empty");
            assert!(!g.name.is_empty(),        "name must not be empty for uuid={}", g.uuid);
            assert!(!g.device_type.is_empty(), "device_type must not be empty for uuid={}", g.uuid);
        });
    }

    // T-GPU-C3: utilization_pct is in range 0.0..=100.0 for all reported GPUs.
    #[test]
    fn test_gpu_collect_utilization_in_range() {
        let collector = GpuCollector::new();
        let gpus = collector.collect().expect("collect() failed");
        gpus.iter().for_each(|g| {
            assert!(
                g.utilization_pct >= 0.0 && g.utilization_pct <= 100.0,
                "utilization_pct out of range for {}: {}",
                g.uuid,
                g.utilization_pct
            );
        });
    }

    // T-GPU-C4: vram_used_bytes does not exceed vram_total_bytes.
    #[test]
    fn test_gpu_collect_vram_used_le_total() {
        let collector = GpuCollector::new();
        let gpus = collector.collect().expect("collect() failed");
        gpus.iter().for_each(|g| {
            assert!(
                g.vram_used_bytes <= g.vram_total_bytes,
                "vram_used_bytes {} > vram_total_bytes {} for {}",
                g.vram_used_bytes,
                g.vram_total_bytes,
                g.uuid
            );
        });
    }
}

/// Read a u64 value from a single-line sysfs attribute file.
fn read_sysfs_u64(path: impl AsRef<Path>) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}
