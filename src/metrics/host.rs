use serde::{Deserialize, Serialize};

/// Machine-level host properties collected once at startup.
/// All fields are optional; collection failure is silently swallowed.
/// Used in the Sentinel API registration payload (Section 9.1).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostInfo {
    /// AWS: `/sys/class/dmi/id/board_asset_tag`; fallback: `/etc/machine-id`.
    pub host_id: Option<String>,
    /// Hostname from `gethostname(3)`.
    #[serde(rename = "host_hostname")]
    pub host_name: Option<String>,
    /// First non-loopback IPv4 address from `getifaddrs(3)`.
    pub host_ip: Option<String>,
    /// `"dedicated"` or `"shared"`. Heuristic TBD; currently always `None`.
    #[serde(rename = "host_server_allocation")]
    pub host_allocation: Option<String>,
    /// Count of logical CPUs from `/proc/cpuinfo` processor entries.
    pub host_vcpus: Option<u32>,
    /// CPU model string from `/proc/cpuinfo`.
    pub host_cpu_model: Option<String>,
    /// Total physical RAM: `MemTotal / 1024` from `/proc/meminfo`.
    pub host_memory_mib: Option<u64>,
    /// Name of the first detected GPU/NPU/TPU.
    pub host_gpu_model: Option<String>,
    /// Count of detected GPUs.
    pub host_gpu_count: Option<u32>,
    /// Sum of `vram_total_bytes / 1_048_576` across all GPUs.
    /// Serialized as `host_gpu_memory_mib` to match the Sentinel API field name.
    #[serde(rename = "host_gpu_memory_mib")]
    pub host_gpu_vram_mib: Option<u64>,
    /// Sum of block device capacities in GB (non-loop, non-ram devices).
    pub host_storage_gb: Option<f64>,
}

/// Cloud instance metadata collected via IMDS probes at startup.
/// All fields are `None` on non-cloud hosts or when the IMDS probe times out.
/// Used in the Sentinel API registration payload (Section 9.1).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudInfo {
    /// Cloud provider identifier: `"aws"`, `"gcp"`, or `"azure"`.
    pub cloud_vendor_id: Option<String>,
    /// AWS account ID (from EC2 identity credentials endpoint).
    pub cloud_account_id: Option<String>,
    /// AWS region, e.g. `"us-east-1"`.
    pub cloud_region_id: Option<String>,
    /// AWS availability zone, e.g. `"us-east-1a"`.
    pub cloud_zone_id: Option<String>,
    /// AWS instance type, e.g. `"t3.micro"`.
    pub cloud_instance_type: Option<String>,
}
