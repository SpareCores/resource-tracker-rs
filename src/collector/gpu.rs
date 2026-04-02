use crate::metrics::GpuMetrics;
use libamdgpu_top::{
    AMDGPU::{GpuMetrics as AmdHwMetrics, MetricsInfo},
    stat::GpuActivity,
    DevicePath,
};
use nvml_wrapper::{
    enum_wrappers::device::{Clock, TemperatureSensor},
    Nvml,
};
use std::collections::HashMap;
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

/// Read a u64 value from a single-line sysfs attribute file.
fn read_sysfs_u64(path: impl AsRef<Path>) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}
