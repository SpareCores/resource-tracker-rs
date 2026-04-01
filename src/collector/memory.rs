use crate::metrics::MemoryMetrics;
use procfs::prelude::*;
use procfs::Meminfo;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Stateless: each call is a fresh snapshot from /proc/meminfo.
/// Memory hardware type (DDR4/DDR5) requires SMBIOS/DMI parsing which needs
/// elevated privileges - deferred to a later phase.
pub struct MemoryCollector;

impl MemoryCollector {
    pub fn new() -> Self {
        Self
    }

    pub fn collect(&self) -> Result<MemoryMetrics> {
        let info = Meminfo::current()?;

        // procfs 0.18 converts /proc/meminfo "kB" values to bytes internally.
        // Divide by 1_048_576 to convert to mebibytes (MiB), standardized to
        // match Python resource-tracker PR #9.
        let to_mib = |bytes: u64| bytes / 1_048_576;

        let total_mib     = to_mib(info.mem_total);
        let free_mib      = to_mib(info.mem_free);
        let available_mib = to_mib(info.mem_available.unwrap_or(info.mem_free));
        let buffers_mib   = to_mib(info.buffers);
        let cached_mib    = to_mib(info.cached)
            + to_mib(info.s_reclaimable.unwrap_or(0));
        // Python formula: MemTotal - MemFree - Buffers - (Cached + SReclaimable)
        let used_mib = total_mib
            .saturating_sub(free_mib)
            .saturating_sub(buffers_mib)
            .saturating_sub(cached_mib);
        let used_pct = if total_mib > 0 {
            used_mib as f64 / total_mib as f64 * 100.0
        } else {
            0.0
        };

        let swap_total_mib = to_mib(info.swap_total);
        let swap_used_mib  = swap_total_mib.saturating_sub(to_mib(info.swap_free));
        let swap_used_pct  = if swap_total_mib > 0 {
            swap_used_mib as f64 / swap_total_mib as f64 * 100.0
        } else {
            0.0
        };

        Ok(MemoryMetrics {
            total_mib,
            free_mib,
            available_mib,
            used_mib,
            used_pct,
            buffers_mib,
            cached_mib,
            swap_total_mib,
            swap_used_mib,
            swap_used_pct,
            active_mib:   to_mib(info.active),
            inactive_mib: to_mib(info.inactive),
        })
    }
}
