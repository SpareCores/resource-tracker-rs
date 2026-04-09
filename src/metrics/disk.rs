use crate::metrics::{DiskMountMetrics, DiskType};
use serde::{Deserialize, Serialize};

/// Per-device disk metrics: static identity (cached once) + dynamic throughput
/// and filesystem space (polled each interval).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskMetrics {
    // ------------------------------------------------------------------
    // Identity - read from /sys/block/<dev>/ once at startup.
    // ------------------------------------------------------------------
    pub device: String,
    /// e.g. "Samsung SSD 870 EVO 1TB", from `/sys/block/<dev>/device/model`.
    pub model: Option<String>,
    /// e.g. "ATA", from `/sys/block/<dev>/device/vendor`.
    pub vendor: Option<String>,
    /// World-Wide ID or serial, from `/sys/block/<dev>/device/wwid` or `serial`.
    pub serial: Option<String>,
    pub device_type: Option<DiskType>,
    /// Total raw device capacity in bytes (`/sys/block/<dev>/size` × 512).
    pub capacity_bytes: Option<u64>,

    // ------------------------------------------------------------------
    // Filesystem space - updated each poll via statvfs(3).
    // One entry per mount point that belongs to this device.
    // ------------------------------------------------------------------
    pub mounts: Vec<DiskMountMetrics>,

    // ------------------------------------------------------------------
    // Throughput - derived from /proc/diskstats sector deltas.
    // ------------------------------------------------------------------
    pub read_bytes_per_sec: f64,
    pub write_bytes_per_sec: f64,
    /// Cumulative bytes read since boot (raw /proc/diskstats sector count × 512).
    /// Matches Python resource-tracker's `disk_read_bytes` column.
    pub read_bytes_total: u64,
    /// Cumulative bytes written since boot (raw /proc/diskstats sector count × 512).
    /// Matches Python resource-tracker's `disk_write_bytes` column.
    pub write_bytes_total: u64,
}
