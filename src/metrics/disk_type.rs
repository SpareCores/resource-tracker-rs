use serde::{Deserialize, Serialize};

/// Physical storage technology of the block device.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskType {
    /// NVMe SSD (PCIe-attached).
    Nvme,
    /// SATA/SAS SSD - /sys/block/<dev>/queue/rotational == 0.
    Ssd,
    /// Spinning hard disk - /sys/block/<dev>/queue/rotational == 1.
    Hdd,
}
