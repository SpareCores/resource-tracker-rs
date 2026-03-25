use serde::{Deserialize, Serialize};

/// Memory snapshot from /proc/meminfo.
/// All values are in **kibibytes (KiB = 1024 bytes)**, matching the unit
/// reported by the kernel. Note: /proc/meminfo labels these "kB" but the
/// kernel has always meant 1024-byte units.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_kib: u64,
    /// Truly free RAM (`MemFree` from /proc/meminfo). Matches Python `memory_free`.
    pub free_kib: u64,
    /// Free + reclaimable RAM (`MemAvailable` from /proc/meminfo). Superset field.
    pub available_kib: u64,
    /// Used RAM excluding buffers/cache (`MemTotal - MemFree - Buffers - Cached`).
    /// Matches Python `memory_used`.
    pub used_kib: u64,
    /// Fraction of total RAM in use (0.0–100.0).
    pub used_pct: f64,
    /// Memory used by kernel I/O buffers (kibibytes).
    pub buffers_kib: u64,
    /// Memory used by the page cache including slab-reclaimable (`Cached + SReclaimable`).
    /// Matches Python `memory_cached`.
    pub cached_kib: u64,
    pub swap_total_kib: u64,
    pub swap_used_kib: u64,
    /// Fraction of swap in use (0.0–100.0). 0.0 when no swap is configured.
    pub swap_used_pct: f64,
    /// Memory used by active pages (kibibytes). Matches Python's `memory_active`.
    pub active_kib: u64,
    /// Memory used by inactive pages (kibibytes). Matches Python's `memory_inactive`.
    pub inactive_kib: u64,
}
