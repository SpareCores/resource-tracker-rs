use crate::metrics::Sample;

/// CSV header in parity with Python resource-tracker's SystemTracker columns;
/// the Rust binary is a functional superset of the Python version.
///
/// Unit notes vs. Python SystemTracker:
///   cpu_usage        - fractional cores (0..N), same as Python
///   memory_*         - mebibytes (MiB), standardized in Python PR #9
///   disk_read/write  - bytes per interval, same as Python
///   net_recv/sent    - bytes per interval, same as Python
///   disk_space_*     - GB summed across all block-device mounts (same method as Python)
///   gpu_vram         - MiB, same as Python
pub fn csv_header() -> &'static str {
    "timestamp,processes,utime,stime,cpu_usage,\
     memory_free,memory_used,memory_buffers,memory_cached,memory_active,memory_inactive,\
     disk_read_bytes,disk_write_bytes,\
     disk_space_total_gb,disk_space_used_gb,disk_space_free_gb,\
     net_recv_bytes,net_sent_bytes,\
     gpu_usage,gpu_vram,gpu_utilized"
}

/// Serialize a `Sample` as a single CSV row (no newline).
///
/// `interval_secs` is required to convert bytes/sec rates into per-interval
/// byte counts, matching Python resource-tracker's convention.
pub fn sample_to_csv_row(s: &Sample, interval_secs: u64) -> String {
    // cpu_usage: utilization_pct is already in fractional cores (0..N_cores)
    let cpu_usage = s.cpu.utilization_pct;

    // disk I/O: per-interval byte counts, matching Python's convention
    // (rate × interval ≈ bytes transferred in this sampling window)
    let secs = f64::from(u32::try_from(interval_secs).unwrap_or(u32::MAX));
    let disk_read: u64  = s.disk.iter().map(|d| (d.read_bytes_per_sec  * secs) as u64).sum();
    let disk_write: u64 = s.disk.iter().map(|d| (d.write_bytes_per_sec * secs) as u64).sum();

    // disk space: sum all mounts across all devices, matching Python's
    // SystemTracker convention.
    // used = total − free  (includes reserved-for-root blocks, same as Python)
    let disk_space_total: f64 = s.disk.iter()
        .flat_map(|d| d.mounts.iter())
        .map(|m| m.total_bytes as f64 / 1_000_000_000.0)
        .sum();
    let disk_space_free: f64 = s.disk.iter()
        .flat_map(|d| d.mounts.iter())
        .map(|m| m.available_bytes as f64 / 1_000_000_000.0)
        .sum();
    let disk_space_used = disk_space_total - disk_space_free;

    // network I/O: per-interval byte counts, matching Python's convention
    let net_recv: u64 = s.network.iter().map(|n| (n.rx_bytes_per_sec * secs) as u64).sum();
    let net_sent: u64 = s.network.iter().map(|n| (n.tx_bytes_per_sec * secs) as u64).sum();

    // GPU: fractional utilization, VRAM in MiB, count of active GPUs
    let gpu_usage: f64    = s.gpu.iter().map(|g| g.utilization_pct / 100.0).sum();
    let gpu_vram: f64     = s.gpu.iter().map(|g| g.vram_used_bytes as f64 / 1_048_576.0).sum();
    let gpu_utilized: u32 = u32::try_from(
        s.gpu.iter().filter(|g| g.utilization_pct > 0.0).count()
    ).unwrap_or(0);

    format!(
        "{},{},{:.3},{:.3},{:.4},{},{},{},{},{},{},{},{},{:.6},{:.6},{:.6},{},{},{:.4},{:.4},{}",
        s.timestamp_secs,
        s.cpu.process_count,
        s.cpu.utime_secs,
        s.cpu.stime_secs,
        cpu_usage,
        s.memory.free_mib,
        s.memory.used_mib,
        s.memory.buffers_mib,
        s.memory.cached_mib,
        s.memory.active_mib,
        s.memory.inactive_mib,
        disk_read,
        disk_write,
        disk_space_total,
        disk_space_used,
        disk_space_free,
        net_recv,
        net_sent,
        gpu_usage,
        gpu_vram,
        gpu_utilized,
    )
}
