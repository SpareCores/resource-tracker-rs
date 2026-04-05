use serde::{Deserialize, Serialize};

/// Memory snapshot from /proc/meminfo.
/// All values are in **mebibytes (MiB = 1_048_576 bytes)**, standardized to
/// match Python resource-tracker PR #9 which also adopted MiB throughout.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_mib: u64,
    /// Truly free RAM (`MemFree` from /proc/meminfo). Matches Python `memory_free`.
    pub free_mib: u64,
    /// Free + reclaimable RAM (`MemAvailable` from /proc/meminfo). Superset field.
    pub available_mib: u64,
    /// Used RAM excluding buffers/cache (`MemTotal - MemFree - Buffers - Cached`).
    /// Matches Python `memory_used`.
    pub used_mib: u64,
    /// Fraction of total RAM in use (0.0–100.0).
    pub used_pct: f64,
    /// Memory used by kernel I/O buffers (MiB).
    pub buffers_mib: u64,
    /// Memory used by the page cache including slab-reclaimable (`Cached + SReclaimable`).
    /// Matches Python `memory_cached`.
    pub cached_mib: u64,
    pub swap_total_mib: u64,
    pub swap_used_mib: u64,
    /// Fraction of swap in use (0.0–100.0). 0.0 when no swap is configured.
    pub swap_used_pct: f64,
    /// Memory used by active pages (MiB). Matches Python's `memory_active`.
    pub active_mib: u64,
    /// Memory used by inactive pages (MiB). Matches Python's `memory_inactive`.
    pub inactive_mib: u64,
}
