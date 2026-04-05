use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Per-GPU metrics sourced from all-smi (NVIDIA NVML / AMD / TPU / …).
/// Populated only when a supported GPU library is detected at runtime;
/// an empty Vec means no GPUs are present or no driver is installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuMetrics {
    // ------------------------------------------------------------------
    // Identity - reported by the driver, stable for the lifetime of the run.
    // ------------------------------------------------------------------

    /// Vendor-assigned device UUID (stable across reboots for physical GPUs).
    pub uuid: String,
    /// Human-readable device name, e.g. "NVIDIA GeForce RTX 4090".
    pub name: String,
    /// Device class reported by all-smi: "GPU", "NPU", "TPU", etc.
    /// Equivalent to the "kind" of accelerator.
    pub device_type: String,
    /// Host-level device identifier (slot, bus address, or platform index).
    pub host_id: String,
    /// Additional platform-specific identity fields keyed by the driver.
    /// For NVIDIA these typically include PCI device/vendor IDs and the
    /// driver version; for AMD, the ASIC name and PCIe topology; for TPU,
    /// the chip revision.  Keys and values vary by vendor.
    pub detail: HashMap<String, String>,

    // ------------------------------------------------------------------
    // Dynamic metrics - polled each interval.
    // ------------------------------------------------------------------

    /// Core utilisation (0.0–100.0).
    pub utilization_pct: f64,
    /// Total VRAM in bytes.
    pub vram_total_bytes: u64,
    /// Used VRAM in bytes.
    pub vram_used_bytes: u64,
    /// Fraction of VRAM in use (0.0–100.0).
    pub vram_used_pct: f64,
    /// Die temperature in degrees Celsius.
    pub temperature_celsius: u32,
    /// Power draw in watts.
    pub power_watts: f64,
    /// Core clock frequency in MHz.
    pub frequency_mhz: u32,
    /// Number of shader/compute cores, if reported by the driver.
    pub core_count: Option<u32>,
}
