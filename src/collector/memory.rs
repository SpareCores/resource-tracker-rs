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
        // Divide by 1024 to restore the conventional kibibyte (KiB) unit that
        // /proc/meminfo reports and that Python resource-tracker exposes.
        let to_kib = |bytes: u64| bytes / 1024;

        let total_kib     = to_kib(info.mem_total);
        let free_kib      = to_kib(info.mem_free);
        let available_kib = to_kib(info.mem_available.unwrap_or(info.mem_free));
        let buffers_kib   = to_kib(info.buffers);
        let cached_kib    = to_kib(info.cached)
            + to_kib(info.s_reclaimable.unwrap_or(0));
        // Python formula: MemTotal - MemFree - Buffers - (Cached + SReclaimable)
        let used_kib = total_kib
            .saturating_sub(free_kib)
            .saturating_sub(buffers_kib)
            .saturating_sub(cached_kib);
        let used_pct = if total_kib > 0 {
            used_kib as f64 / total_kib as f64 * 100.0
        } else {
            0.0
        };

        let swap_total_kib = to_kib(info.swap_total);
        let swap_used_kib  = swap_total_kib.saturating_sub(to_kib(info.swap_free));
        let swap_used_pct  = if swap_total_kib > 0 {
            swap_used_kib as f64 / swap_total_kib as f64 * 100.0
        } else {
            0.0
        };

        Ok(MemoryMetrics {
            total_kib,
            free_kib,
            available_kib,
            used_kib,
            used_pct,
            buffers_kib,
            cached_kib,
            swap_total_kib,
            swap_used_kib,
            swap_used_pct,
            active_kib:   to_kib(info.active),
            inactive_kib: to_kib(info.inactive),
        })
    }
}
