use serde::{Deserialize, Serialize};

/// Filesystem-level space for one mount point on a block device.
/// Values from statvfs(3); reported in bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskMountMetrics {
    pub mount_point: String,
    /// Filesystem type as reported in /proc/mounts (e.g. "ext4", "xfs", "btrfs").
    pub filesystem: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    /// Fraction of total capacity in use (0.0–100.0).
    pub used_pct: f64,
}
