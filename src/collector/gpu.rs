use crate::metrics::GpuMetrics;
use all_smi::prelude::*;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Wraps all-smi with graceful degradation: if GPU libraries are not installed
/// (NVML, ROCm, etc.) AllSmi::new() will fail and every collect() call will
/// return an empty Vec rather than an error.  The binary remains usable on
/// CPU-only hosts.
pub struct GpuCollector {
    smi: Option<AllSmi>,
}

impl GpuCollector {
    pub fn new() -> Self {
        Self {
            smi: AllSmi::new().ok(),
        }
    }

    pub fn collect(&self) -> Result<Vec<GpuMetrics>> {
        let Some(ref smi) = self.smi else {
            return Ok(vec![]);
        };

        let metrics = smi
            .get_gpu_info()
            .into_iter()
            .map(|gpu| {
                let vram_used_pct = if gpu.total_memory > 0 {
                    gpu.used_memory as f64 / gpu.total_memory as f64 * 100.0
                } else {
                    0.0
                };
                GpuMetrics {
                    uuid: gpu.uuid,
                    name: gpu.name,
                    device_type: gpu.device_type,
                    host_id: gpu.host_id,
                    detail: gpu.detail,
                    utilization_pct: gpu.utilization,
                    vram_total_bytes: gpu.total_memory,
                    vram_used_bytes: gpu.used_memory,
                    vram_used_pct,
                    temperature_celsius: gpu.temperature,
                    power_watts: gpu.power_consumption,
                    frequency_mhz: gpu.frequency,
                    core_count: gpu.gpu_core_count,
                }
            })
            .collect();

        Ok(metrics)
    }
}
