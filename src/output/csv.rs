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

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{CpuMetrics, DiskMetrics, DiskMountMetrics, MemoryMetrics, Sample};

    fn minimal_sample() -> Sample {
        Sample {
            timestamp_secs: 1_000_000,
            job_name:       None,
            cpu: CpuMetrics {
                utilization_pct:     2.5,
                utime_secs:          1.234,
                stime_secs:          0.567,
                process_count:       42,
                per_core_pct:        vec![],
                process_cores_used:  None,
                process_child_count: None,
            },
            memory: MemoryMetrics {
                total_mib:      8192,
                free_mib:       1000,
                available_mib:  2000,
                used_mib:       2000,
                used_pct:       25.0,
                buffers_mib:    100,
                cached_mib:     500,
                swap_total_mib: 0,
                swap_used_mib:  0,
                swap_used_pct:  0.0,
                active_mib:     1500,
                inactive_mib:   300,
            },
            network: vec![],
            disk:    vec![],
            gpu:     vec![],
        }
    }

    // T-CSV-01: header is the first line and contains no embedded newlines.
    #[test]
    fn test_csv_header_is_first_line_no_embedded_newline() {
        let h = csv_header();
        assert!(h.starts_with("timestamp,"), "header must start with 'timestamp,'");
        assert!(!h.contains('\n'), "header must not contain an embedded newline");
    }

    // T-CSV-02: column count in each data row equals column count in header.
    #[test]
    fn test_csv_row_column_count_matches_header() {
        let header_cols = csv_header().split(',').count();
        let row = sample_to_csv_row(&minimal_sample(), 1);
        let row_cols = row.split(',').count();
        assert_eq!(
            row_cols, header_cols,
            "header has {header_cols} columns but row has {row_cols}: {row}"
        );
    }

    // T-CSV-03: cpu_usage column equals utilization_pct (already fractional cores) to 4 dp.
    //
    // NOTE: The Specification.md table formula reads "utilization_pct / 100 × total_cores"
    // which is stale.  PR #1 Changelog explicitly corrected this:
    //   "Was: utilization_pct / 100.0 * total_cores; Now: utilization_pct directly
    //    (field is already in fractional cores)."
    // The CpuMetrics field definition in the spec and in metrics/cpu.rs both confirm
    // utilization_pct is in range 0.0..N_cores, not 0.0..100.0.
    // This test verifies the actual (correct) behavior.
    #[test]
    fn test_csv_cpu_usage_is_utilization_pct_direct() {
        let mut sample = minimal_sample();
        sample.cpu.utilization_pct = 3.1415;
        let row = sample_to_csv_row(&sample, 1);
        // Column order: timestamp(0),processes(1),utime(2),stime(3),cpu_usage(4),...
        let cols: Vec<&str> = row.split(',').collect();
        let cpu_usage: f64 = cols[4].parse()
            .unwrap_or_else(|_| panic!("cpu_usage column is not numeric: {:?}", cols[4]));
        assert!(
            (cpu_usage - 3.1415_f64).abs() < 0.00005,
            "cpu_usage {cpu_usage:.4} does not match utilization_pct 3.1415"
        );
    }

    // T-CSV-04: disk_space_used_gb == disk_space_total_gb - disk_space_free_gb.
    #[test]
    fn test_csv_disk_space_used_equals_total_minus_free() {
        let mut sample = minimal_sample();
        sample.disk = vec![DiskMetrics {
            device:            "sda".to_string(),
            model:             None,
            vendor:            None,
            serial:            None,
            device_type:       None,
            capacity_bytes:    None,
            mounts: vec![DiskMountMetrics {
                mount_point:     "/".to_string(),
                filesystem:      "ext4".to_string(),
                total_bytes:     100_000_000_000,
                used_bytes:      60_000_000_000,
                available_bytes: 40_000_000_000,
                used_pct:        60.0,
            }],
            read_bytes_per_sec:  0.0,
            write_bytes_per_sec: 0.0,
            read_bytes_total:    0,
            write_bytes_total:   0,
        }];
        let row = sample_to_csv_row(&sample, 1);
        // Column order: ...disk_space_total_gb(13),disk_space_used_gb(14),disk_space_free_gb(15),...
        let cols: Vec<&str> = row.split(',').collect();
        let total: f64 = cols[13].parse().unwrap();
        let used:  f64 = cols[14].parse().unwrap();
        let free:  f64 = cols[15].parse().unwrap();
        assert!(
            (used - (total - free)).abs() < 1e-9,
            "disk_space_used_gb {used:.6} != total {total:.6} - free {free:.6}"
        );
    }

    // T-CSV-05: output is byte-for-byte reproducible for the same sample.
    #[test]
    fn test_csv_output_is_deterministic() {
        let sample = minimal_sample();
        let r1 = sample_to_csv_row(&sample, 1);
        let r2 = sample_to_csv_row(&sample, 1);
        assert_eq!(r1, r2, "csv row output is not deterministic");
    }

    // T-CSV-06: no trailing commas; no quoted fields.
    #[test]
    fn test_csv_no_trailing_commas_no_quoted_fields() {
        let row = sample_to_csv_row(&minimal_sample(), 1);
        assert!(!row.ends_with(','),  "trailing comma in row: {row}");
        assert!(!row.contains('"'),   "double-quoted field in row: {row}");
        assert!(!row.contains('\''),  "single-quoted field in row: {row}");
        let h = csv_header();
        assert!(!h.ends_with(','), "trailing comma in header");
        assert!(!h.contains('"'),  "double-quoted field in header");
    }
}
