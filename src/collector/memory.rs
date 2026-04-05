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

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // T-MEM-01: collect() succeeds on a Linux host and total_mib > 0.
    #[test]
    fn test_memory_collect_ok_and_total_positive() {
        let m = MemoryCollector::new().collect().expect("collect() must succeed on Linux");
        assert!(m.total_mib > 0, "total_mib must be > 0, got {}", m.total_mib);
    }

    // T-MEM-02: used_pct is in 0..=100.
    #[test]
    fn test_memory_used_pct_in_range() {
        let m = MemoryCollector::new().collect().expect("collect() failed");
        assert!(
            m.used_pct >= 0.0 && m.used_pct <= 100.0,
            "used_pct out of range: {}",
            m.used_pct
        );
    }

    // T-MEM-03: free_mib and available_mib do not exceed total_mib.
    #[test]
    fn test_memory_free_and_available_le_total() {
        let m = MemoryCollector::new().collect().expect("collect() failed");
        assert!(
            m.free_mib <= m.total_mib,
            "free_mib {} > total_mib {}",
            m.free_mib,
            m.total_mib
        );
        assert!(
            m.available_mib <= m.total_mib,
            "available_mib {} > total_mib {}",
            m.available_mib,
            m.total_mib
        );
    }

    // T-MEM-04: swap fields are internally consistent.
    #[test]
    fn test_memory_swap_fields_consistent() {
        let m = MemoryCollector::new().collect().expect("collect() failed");
        assert!(
            m.swap_used_mib <= m.swap_total_mib,
            "swap_used_mib {} > swap_total_mib {}",
            m.swap_used_mib,
            m.swap_total_mib
        );
        if m.swap_total_mib == 0 {
            assert_eq!(m.swap_used_mib, 0, "swap_used_mib must be 0 when swap_total_mib is 0");
            assert_eq!(m.swap_used_pct, 0.0, "swap_used_pct must be 0.0 when swap_total_mib is 0");
        }
    }

    // T-MEM-05: collect() is deterministic (two calls both succeed).
    #[test]
    fn test_memory_collect_is_repeatable() {
        let c = MemoryCollector::new();
        let _ = c.collect().expect("first collect() failed");
        let _ = c.collect().expect("second collect() failed");
    }
}
