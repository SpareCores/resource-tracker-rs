pub mod cpu;
pub mod disk;
pub mod disk_mount;
pub mod disk_type;
pub mod gpu;
pub mod host;
pub mod memory;
pub mod network;

pub use cpu::CpuMetrics;
pub use disk::DiskMetrics;
pub use disk_mount::DiskMountMetrics;
pub use disk_type::DiskType;
pub use gpu::GpuMetrics;
pub use host::{CloudInfo, HostInfo};
pub use memory::MemoryMetrics;
pub use network::NetworkMetrics;

use serde::{Deserialize, Serialize};

/// A single point-in-time observation of all tracked resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    /// Unix timestamp (seconds) when this sample was taken.
    pub timestamp_secs: u64,
    /// Optional job label supplied via CLI or config file.
    pub job_name: Option<String>,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub network: Vec<NetworkMetrics>,
    pub disk: Vec<DiskMetrics>,
    /// One entry per detected GPU/NPU/TPU. Empty on CPU-only hosts.
    pub gpu: Vec<GpuMetrics>,
}
