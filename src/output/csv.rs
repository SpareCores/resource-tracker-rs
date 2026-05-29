use crate::metrics::Sample;

/// CSV header using the same `system_`/`process_` prefix convention as
/// Python resource-tracker.  System columns (21) cover host-wide metrics;
/// process columns (11) cover the tracked PID tree.
///
/// Unit notes:
///   system_cpu_usage    - fractional cores (0..N), same as Python
///   system_memory_*_mib - mebibytes (MiB = 1,048,576 bytes)
///   system_disk_*       - bytes per interval, same as Python
///   system_net_*        - bytes per interval, same as Python
///   system_disk_space_* - GB summed across all block-device mounts
///   system_gpu_vram_mib - MiB, same as Python
///   process_cpu_usage   - fractional cores consumed by tracked PID tree
///
/// Process fields not yet collected are emitted as empty strings.
pub fn csv_header() -> &'static str {
    "timestamp,\
     system_processes,system_utime,system_stime,system_cpu_usage,\
     system_memory_free_mib,system_memory_used_mib,system_memory_buffers_mib,\
     system_memory_cached_mib,system_memory_active_mib,system_memory_inactive_mib,\
     system_disk_read_bytes,system_disk_write_bytes,\
     system_disk_space_total_gb,system_disk_space_used_gb,system_disk_space_free_gb,\
     system_net_recv_bytes,system_net_sent_bytes,\
     system_gpu_usage,system_gpu_vram_mib,system_gpu_utilized,\
     process_pid,process_children,process_utime,process_stime,process_cpu_usage,\
     process_memory_mib,process_disk_read_bytes,process_disk_write_bytes,\
     process_gpu_usage,process_gpu_vram_mib,process_gpu_utilized"
}

/// Serialize a `Sample` as a single CSV row (no newline).
///
/// `interval_secs` is required to convert bytes/sec rates into per-interval
/// byte counts, matching Python resource-tracker's convention.
///
/// Process fields not yet collected are emitted as empty strings.
/// All process fields are empty when no PID is being tracked.
pub fn sample_to_csv_row(s: &Sample, interval_secs: u64) -> String {
    // system_cpu_usage: host-level utilization in fractional cores (0..N_cores)
    let cpu_usage = s.cpu.utilization_pct;

    // Disk I/O: per-interval byte counts (rate × actual_interval ≈ bytes in this window).
    // Prefer actual_interval_ms from the sample when available; fall back to the
    // configured nominal interval so the first sample (which has no prior baseline)
    // still produces a reasonable estimate.
    let secs = s
        .actual_interval_ms
        .map(|ms| ms as f64 / 1000.0)
        .unwrap_or_else(|| f64::from(u32::try_from(interval_secs).unwrap_or(u32::MAX)));
    let disk_read: u64 = s
        .disk
        .iter()
        .map(|d| (d.read_bytes_per_sec * secs) as u64)
        .sum();
    let disk_write: u64 = s
        .disk
        .iter()
        .map(|d| (d.write_bytes_per_sec * secs) as u64)
        .sum();

    // Disk space: sum all mounts; used = total - free (includes root-reserved blocks)
    let disk_space_total: f64 = s
        .disk
        .iter()
        .flat_map(|d| d.mounts.iter())
        .map(|m| m.total_bytes as f64 / 1_000_000_000.0)
        .sum();
    let disk_space_free: f64 = s
        .disk
        .iter()
        .flat_map(|d| d.mounts.iter())
        .map(|m| m.available_bytes as f64 / 1_000_000_000.0)
        .sum();
    let disk_space_used = disk_space_total - disk_space_free;

    // Network I/O: per-interval byte counts
    let net_recv: u64 = s
        .network
        .iter()
        .map(|n| (n.rx_bytes_per_sec * secs) as u64)
        .sum();
    let net_sent: u64 = s
        .network
        .iter()
        .map(|n| (n.tx_bytes_per_sec * secs) as u64)
        .sum();

    // GPU system aggregates
    let gpu_usage: f64 = s.gpu.iter().map(|g| g.utilization_pct / 100.0).sum();
    let gpu_vram: f64 = s
        .gpu
        .iter()
        .map(|g| g.vram_used_bytes as f64 / 1_048_576.0)
        .sum();
    let gpu_utilized: u32 =
        u32::try_from(s.gpu.iter().filter(|g| g.utilization_pct > 0.0).count()).unwrap_or(0);

    // System columns (21): same layout and values as before, new names in header.
    let system_row = format!(
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
    );

    // Process columns (11): empty when not tracked or not yet collected.
    let opt_u32 = |v: Option<u32>| v.map_or(String::new(), |x| x.to_string());
    let opt_i32 = |v: Option<i32>| v.map_or(String::new(), |x| x.to_string());
    let opt_f4 = |v: Option<f64>| v.map_or(String::new(), |x| format!("{x:.4}"));

    let opt_u64 = |v: Option<u64>| v.map_or(String::new(), |x| x.to_string());

    let process_row = [
        opt_i32(s.tracked_pid),
        opt_u32(s.cpu.process_child_count),
        opt_f4(s.cpu.process_utime_secs),
        opt_f4(s.cpu.process_stime_secs),
        opt_f4(s.cpu.process_cores_used),
        opt_u64(s.cpu.process_pss_mib),
        opt_u64(s.cpu.process_disk_read_bytes),
        opt_u64(s.cpu.process_disk_write_bytes),
        opt_f4(s.cpu.process_gpu_usage),
        opt_f4(s.cpu.process_gpu_vram_mib),
        opt_u32(s.cpu.process_gpu_utilized),
    ]
    .join(",");

    format!("{system_row},{process_row}")
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
            actual_interval_ms: None,
            job_name: None,
            tracked_pid: None,
            cpu: CpuMetrics {
                utilization_pct: 2.5,
                cgroup_utilization_pct: None,
                cgroup_usage_secs: None,
                utime_secs: 1.234,
                stime_secs: 0.567,
                process_count: 42,
                per_core_pct: vec![],
                process_cores_used: None,
                process_child_count: None,
                process_utime_secs: None,
                process_stime_secs: None,
                process_pss_mib: None,
                process_rss_mib: None,
                process_disk_read_bytes: None,
                process_disk_write_bytes: None,
                process_gpu_usage: None,
                process_gpu_vram_mib: None,
                process_gpu_utilized: None,
                process_tree_pids: vec![],
            },
            memory: MemoryMetrics {
                total_mib: 8192,
                free_mib: 1000,
                available_mib: 2000,
                used_mib: 2000,
                used_pct: 25.0,
                buffers_mib: 100,
                cached_mib: 500,
                swap_total_mib: 0,
                swap_used_mib: 0,
                swap_used_pct: 0.0,
                active_mib: 1500,
                inactive_mib: 300,
            },
            network: vec![],
            disk: vec![],
            gpu: vec![],
        }
    }

    // T-CSV-01: header is the first line and contains no embedded newlines.
    #[test]
    fn test_csv_header_is_first_line_no_embedded_newline() {
        let h = csv_header();
        assert!(
            h.starts_with("timestamp,"),
            "header must start with 'timestamp,'"
        );
        assert!(
            !h.contains('\n'),
            "header must not contain an embedded newline"
        );
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

    // T-CSV-03: system_cpu_usage column equals host utilization_pct to 4 dp.
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
        // Column order: timestamp(0),system_processes(1),system_utime(2),
        //   system_stime(3),system_cpu_usage(4),...
        let cols: Vec<&str> = row.split(',').collect();
        let cpu_usage: f64 = cols[4]
            .parse()
            .unwrap_or_else(|_| panic!("system_cpu_usage column is not numeric: {:?}", cols[4]));
        assert!(
            (cpu_usage - 3.1415_f64).abs() < 0.00005,
            "system_cpu_usage {cpu_usage:.4} does not match utilization_pct 3.1415"
        );
    }

    // T-CSV-04: disk_space_used_gb == disk_space_total_gb - disk_space_free_gb.
    #[test]
    fn test_csv_disk_space_used_equals_total_minus_free() {
        let mut sample = minimal_sample();
        sample.disk = vec![DiskMetrics {
            device: "sda".to_string(),
            model: None,
            vendor: None,
            serial: None,
            device_type: None,
            capacity_bytes: None,
            mounts: vec![DiskMountMetrics {
                mount_point: "/".to_string(),
                filesystem: "ext4".to_string(),
                total_bytes: 100_000_000_000,
                used_bytes: 60_000_000_000,
                available_bytes: 40_000_000_000,
                used_pct: 60.0,
            }],
            read_bytes_per_sec: 0.0,
            write_bytes_per_sec: 0.0,
            read_bytes_total: 0,
            write_bytes_total: 0,
        }];
        let row = sample_to_csv_row(&sample, 1);
        // Column order: ...system_disk_space_total_gb(13),system_disk_space_used_gb(14),
        //   system_disk_space_free_gb(15),...  (indices unchanged from original layout)
        let cols: Vec<&str> = row.split(',').collect();
        let total: f64 = cols[13].parse().unwrap();
        let used: f64 = cols[14].parse().unwrap();
        let free: f64 = cols[15].parse().unwrap();
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

    // T-CSV-07: process_gpu_usage, process_gpu_vram_mib, and process_gpu_utilized
    // are emitted at columns 29, 30, and 31 when set.
    #[test]
    fn test_csv_process_gpu_fields_emitted_when_set() {
        let mut sample = minimal_sample();
        sample.tracked_pid = Some(42);
        sample.cpu.process_gpu_usage = Some(0.55);
        sample.cpu.process_gpu_vram_mib = Some(83.1875);
        sample.cpu.process_gpu_utilized = Some(1);

        let row = sample_to_csv_row(&sample, 1);
        let cols: Vec<&str> = row.split(',').collect();

        assert_eq!(cols[29], "0.5500", "process_gpu_usage mismatch");
        assert_eq!(cols[30], "83.1875", "process_gpu_vram_mib mismatch");
        assert_eq!(cols[31], "1", "process_gpu_utilized mismatch");
    }

    // T-CSV-08: process GPU columns are empty strings when no PID is tracked.
    #[test]
    fn test_csv_process_gpu_fields_empty_when_untracked() {
        let sample = minimal_sample(); // tracked_pid = None, all process fields None

        let row = sample_to_csv_row(&sample, 1);
        let cols: Vec<&str> = row.split(',').collect();

        assert_eq!(cols[29], "", "process_gpu_usage must be empty when None");
        assert_eq!(cols[30], "", "process_gpu_vram_mib must be empty when None");
        assert_eq!(cols[31], "", "process_gpu_utilized must be empty when None");
    }

    // T-CSV-06: no quoted fields; header has no trailing comma.
    // Note: data rows may end with ',' when trailing process fields are empty
    // (no PID tracked).  This is valid CSV -- empty fields after the last comma
    // represent null values, not a formatting error.
    #[test]
    fn test_csv_no_trailing_commas_no_quoted_fields() {
        let row = sample_to_csv_row(&minimal_sample(), 1);
        assert!(!row.contains('"'), "double-quoted field in row: {row}");
        assert!(!row.contains('\''), "single-quoted field in row: {row}");
        let h = csv_header();
        assert!(!h.ends_with(','), "trailing comma in header");
        assert!(!h.contains('"'), "double-quoted field in header");
    }

    // T-CSV-09: sample_to_csv_row uses actual_interval_ms for disk/network byte
    // conversion when Some, ignoring the nominal interval_secs argument.
    //
    // Setup: disk reports 1000 B/s; nominal interval = 1 s; actual interval = 2 s.
    // Expected: system_disk_read_bytes = 2000 (rate × actual), not 1000 (rate × nominal).
    #[test]
    fn test_csv_rate_conversion_uses_actual_interval_when_present() {
        use crate::metrics::DiskMetrics;
        let mut sample = minimal_sample();
        sample.actual_interval_ms = Some(2000); // 2 s actual
        sample.disk = vec![DiskMetrics {
            device: "sda".to_string(),
            model: None,
            vendor: None,
            serial: None,
            device_type: None,
            capacity_bytes: None,
            mounts: vec![],
            read_bytes_per_sec: 1000.0,
            write_bytes_per_sec: 500.0,
            read_bytes_total: 0,
            write_bytes_total: 0,
        }];

        // Column 11 = system_disk_read_bytes, column 12 = system_disk_write_bytes.
        let row = sample_to_csv_row(&sample, 1); // nominal = 1 s
        let cols: Vec<&str> = row.split(',').collect();
        let read: u64 = cols[11]
            .parse()
            .unwrap_or_else(|_| panic!("system_disk_read_bytes not u64: {:?}", cols[11]));
        let write: u64 = cols[12]
            .parse()
            .unwrap_or_else(|_| panic!("system_disk_write_bytes not u64: {:?}", cols[12]));
        assert_eq!(
            read, 2000,
            "system_disk_read_bytes must use actual interval (2 s → 2000 B), not nominal (1 s → 1000 B)"
        );
        assert_eq!(
            write, 1000,
            "system_disk_write_bytes must use actual interval (2 s → 1000 B), not nominal (1 s → 500 B)"
        );
    }

    // T-CSV-10: sample_to_csv_row falls back to the nominal interval_secs when
    // actual_interval_ms is None (first sample -- no prior baseline exists).
    //
    // Setup: disk reports 1000 B/s; actual_interval_ms = None; nominal = 3 s.
    // Expected: system_disk_read_bytes = 3000 (rate × nominal).
    #[test]
    fn test_csv_rate_conversion_falls_back_to_nominal_when_actual_absent() {
        use crate::metrics::DiskMetrics;
        let mut sample = minimal_sample();
        sample.actual_interval_ms = None;
        sample.disk = vec![DiskMetrics {
            device: "sda".to_string(),
            model: None,
            vendor: None,
            serial: None,
            device_type: None,
            capacity_bytes: None,
            mounts: vec![],
            read_bytes_per_sec: 1000.0,
            write_bytes_per_sec: 0.0,
            read_bytes_total: 0,
            write_bytes_total: 0,
        }];

        let row = sample_to_csv_row(&sample, 3); // nominal = 3 s, no actual
        let cols: Vec<&str> = row.split(',').collect();
        let read: u64 = cols[11]
            .parse()
            .unwrap_or_else(|_| panic!("system_disk_read_bytes not u64: {:?}", cols[11]));
        assert_eq!(
            read, 3000,
            "system_disk_read_bytes must use nominal interval (3 s → 3000 B) when actual_interval_ms is None"
        );
    }

    // T-CSV-11: actual_interval_ms does NOT add a new column to the CSV row.
    // The field is JSON-only; the CSV column count must remain unchanged.
    #[test]
    fn test_csv_actual_interval_ms_does_not_add_column() {
        let mut with_interval = minimal_sample();
        with_interval.actual_interval_ms = Some(1234);
        let without_interval = minimal_sample(); // actual_interval_ms = None

        let row_with = sample_to_csv_row(&with_interval, 1);
        let row_without = sample_to_csv_row(&without_interval, 1);

        assert_eq!(
            row_with.split(',').count(),
            row_without.split(',').count(),
            "actual_interval_ms must not add a column to the CSV row"
        );
        assert_eq!(
            row_with.split(',').count(),
            csv_header().split(',').count(),
            "CSV row column count must equal header column count"
        );
    }
}
