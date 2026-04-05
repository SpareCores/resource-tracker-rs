/// Smoke and behavioral tests for resource-tracker-rs output formats.
///
/// Most tests spawn the release binary, collect a small number of lines,
/// kill it, then assert on the content.  Using --interval 1 keeps wall-clock
/// time short: the warm-up takes ~1 s, the first sample appears immediately
/// after.
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const BINARY: &str = env!("CARGO_BIN_EXE_resource-tracker-rs");
const TIMEOUT: Duration = Duration::from_secs(10);

/// Spawn the binary with `args`, collect up to `n` stdout lines, then kill it.
/// Returns however many lines arrived before TIMEOUT.
/// Metrics are written to stderr (stdout is reserved for the tracked app).
fn collect_lines(args: &[&str], n: usize) -> Vec<String> {
    let mut child = Command::new(BINARY)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn resource-tracker-rs binary");

    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().take(n) {
            if tx.send(line.unwrap_or_default()).is_err() {
                break;
            }
        }
    });

    let mut lines = Vec::new();
    for _ in 0..n {
        match rx.recv_timeout(TIMEOUT) {
            Ok(line) => lines.push(line),
            Err(_) => break,
        }
    }

    child.kill().ok();
    child.wait().ok();
    lines
}

/// Spawn the binary with `args` in shell-wrapper mode and wait for it to exit
/// naturally (up to `timeout`).  Returns the exit status.
fn run_to_exit(args: &[&str], timeout: Duration) -> std::process::ExitStatus {
    let mut child = Command::new(BINARY)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn binary");

    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return status;
        }
        if std::time::Instant::now() > deadline {
            child.kill().ok();
            child.wait().ok();
            panic!("binary did not exit within {:?}", timeout);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------------------
// T-CFG-03: --interval 0 must exit with non-zero code
// ---------------------------------------------------------------------------

#[test]
fn test_interval_zero_exits_nonzero() {
    let status = Command::new(BINARY)
        .args(["--interval", "0"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("failed to spawn binary");
    assert!(
        !status.success(),
        "--interval 0 should exit with a non-zero exit code"
    );
}

// ---------------------------------------------------------------------------
// JSON output - general
// ---------------------------------------------------------------------------

#[test]
fn test_json_is_valid() {
    let lines = collect_lines(&["--interval", "1"], 1);
    assert_eq!(lines.len(), 1, "expected exactly 1 JSON line");
    let v: serde_json::Value =
        serde_json::from_str(&lines[0]).expect("output is not valid JSON");
    assert!(v.is_object(), "top-level value should be a JSON object");
}

#[test]
fn test_json_version_field_present() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let version_key = format!("{}-version", "resource-tracker-rs");
    assert!(
        v[&version_key].is_string(),
        "version field '{}' missing from JSON output",
        version_key
    );
}

#[test]
fn test_json_two_samples_have_nondecreasing_timestamps() {
    let lines = collect_lines(&["--interval", "1"], 2);
    assert_eq!(lines.len(), 2, "expected 2 JSON lines");
    let a: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let b: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    let ts_a = a["timestamp_secs"].as_u64().unwrap();
    let ts_b = b["timestamp_secs"].as_u64().unwrap();
    assert!(ts_b >= ts_a, "timestamps must be non-decreasing");
}

// ---------------------------------------------------------------------------
// T-CPU-01 / T-CPU-02 (updated): CpuMetrics - fractional cores, no total_cores
// ---------------------------------------------------------------------------

#[test]
fn test_json_cpu_fields_present() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();

    assert!(v["cpu"]["utilization_pct"].is_number(), "utilization_pct missing");
    assert!(v["cpu"]["per_core_pct"].is_array(),     "per_core_pct missing");
    assert!(v["cpu"]["utime_secs"].is_number(),      "utime_secs missing");
    assert!(v["cpu"]["stime_secs"].is_number(),      "stime_secs missing");
    assert!(v["cpu"]["process_count"].is_number(),   "process_count missing");
}

/// T-CPU-01: utilization_pct is fractional cores in [0, N_cores * 1.05].
/// It must NOT be clamped to 100 on multi-core machines.
#[test]
fn test_json_utilization_pct_is_fractional_cores_not_percentage() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();

    let pct = v["cpu"]["utilization_pct"]
        .as_f64()
        .expect("utilization_pct missing");
    let n_cores = v["cpu"]["per_core_pct"]
        .as_array()
        .expect("per_core_pct missing")
        .len();

    assert!(pct >= 0.0, "utilization_pct must be >= 0, got {pct}");
    // On a machine with > 1 core, the value can legitimately exceed 1.0.
    // It must not be clamped to 100 (which was the old percentage behavior).
    assert!(
        pct <= n_cores as f64 * 1.05,
        "utilization_pct ({pct}) must not greatly exceed n_cores ({n_cores})"
    );
}

/// T-CPU-02: total_cores must NOT appear in JSON (moved to host discovery).
#[test]
fn test_json_total_cores_field_absent() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert!(
        v["cpu"]["total_cores"].is_null(),
        "total_cores must not appear in cpu JSON -- it belongs in host discovery (Section 8.1)"
    );
}

#[test]
fn test_json_process_count_at_least_one() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let count = v["cpu"]["process_count"].as_u64().expect("process_count missing");
    assert!(count >= 1, "process_count should be >= 1 on any running Linux system");
}

// ---------------------------------------------------------------------------
// MemoryMetrics: MiB fields present, KiB fields absent
// ---------------------------------------------------------------------------

/// Verify all _mib fields are present with sane values.
#[test]
fn test_json_memory_fields_are_mib() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();

    let total_mib = v["memory"]["total_mib"]
        .as_u64()
        .expect("total_mib missing or not a number");
    assert!(
        total_mib >= 128,
        "total_mib ({total_mib}) should be >= 128 -- most machines have at least 128 MiB"
    );
    assert!(
        total_mib < 10_000_000,
        "total_mib ({total_mib}) is unreasonably large"
    );

    assert!(v["memory"]["free_mib"].is_number(),      "free_mib missing");
    assert!(v["memory"]["available_mib"].is_number(), "available_mib missing");
    assert!(v["memory"]["used_mib"].is_number(),      "used_mib missing");
    assert!(v["memory"]["buffers_mib"].is_number(),   "buffers_mib missing");
    assert!(v["memory"]["cached_mib"].is_number(),    "cached_mib missing");
    assert!(v["memory"]["active_mib"].is_number(),    "active_mib missing");
    assert!(v["memory"]["inactive_mib"].is_number(),  "inactive_mib missing");
}

/// Old _kib fields must not appear in output.
#[test]
fn test_json_memory_kib_fields_absent() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    ["total_kib", "free_kib", "available_kib", "used_kib",
     "buffers_kib", "cached_kib", "active_kib", "inactive_kib"].iter().for_each(|field| {
        assert!(
            v["memory"][field].is_null(),
            "old field '{field}' must not appear in memory JSON (renamed to _mib)"
        );
    });
}

// ---------------------------------------------------------------------------
// CSV output
// ---------------------------------------------------------------------------

const EXPECTED_HEADER: &str =
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
     process_gpu_usage,process_gpu_vram_mib,process_gpu_utilized";

#[test]
fn test_csv_header_matches_expected() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2, "expected header + at least 1 data row");
    assert_eq!(lines[0], EXPECTED_HEADER, "CSV header mismatch");
}

#[test]
fn test_csv_column_count_consistent() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);
    let header_count = lines[0].split(',').count();
    let row_count    = lines[1].split(',').count();
    assert_eq!(header_count, row_count, "header and row column counts differ");
}

/// cpu_usage must be fractional cores (>= 0, well below any percentage ceiling).
/// Since utilization_pct is now fractional cores, cpu_usage == utilization_pct directly.
#[test]
fn test_csv_cpu_usage_is_fractional_cores() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);

    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();
    let idx = headers.iter().position(|&h| h == "system_cpu_usage").unwrap();

    let cpu_usage: f64 = row[idx].parse().expect("system_cpu_usage: not f64");
    assert!(cpu_usage >= 0.0, "system_cpu_usage must be >= 0");
    // On any real machine the value is well below 1024; a percentage-scale bug
    // would push it to e.g. 62.5 on a loaded single core.
    // We check it is NOT an unreasonably large percentage-like number.
    let n_cpus = num_cpus::get();
    assert!(
        cpu_usage <= n_cpus as f64 * 1.05,
        "system_cpu_usage ({cpu_usage}) looks like a percentage rather than fractional cores \
         (n_cpus = {n_cpus})"
    );
}

#[test]
fn test_csv_values_parse_and_are_sane() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);

    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();

    let col = |name: &str| -> &str {
        let idx = headers.iter().position(|&h| h == name)
            .unwrap_or_else(|| panic!("column '{name}' not found in header"));
        row[idx]
    };

    let timestamp: u64 = col("timestamp").parse().expect("timestamp: not u64");
    assert!(timestamp > 0);

    let processes: u32 = col("system_processes").parse().expect("system_processes: not u32");
    assert!(processes >= 1, "system_processes should be >= 1");

    // memory columns are MiB values; they should be positive but much smaller
    // than old KiB values (total RAM is typically 1000-65536 MiB)
    let memory_free: u64 = col("system_memory_free_mib").parse().expect("system_memory_free_mib: not u64");
    assert!(memory_free > 0, "system_memory_free_mib should be > 0");
    assert!(memory_free < 10_000_000, "system_memory_free_mib looks like KiB, not MiB");

    let memory_used: u64 = col("system_memory_used_mib").parse().expect("system_memory_used_mib: not u64");
    assert!(memory_used > 0, "system_memory_used_mib should be > 0");
    assert!(memory_used < 10_000_000, "system_memory_used_mib looks like KiB, not MiB");

    let disk_total: f64 = col("system_disk_space_total_gb")
        .parse()
        .expect("system_disk_space_total_gb: not f64");
    assert!(disk_total > 0.0, "system_disk_space_total_gb should be > 0");

    let gpu_utilized: u32 = col("system_gpu_utilized").parse().expect("system_gpu_utilized: not u32");
    let _ = gpu_utilized; // 0 is valid on CPU-only hosts
}

#[test]
fn test_csv_two_rows_have_nondecreasing_timestamps() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 3);
    assert!(lines.len() >= 3, "expected header + 2 data rows");

    let headers: Vec<&str> = lines[0].split(',').collect();
    let ts_idx = headers.iter().position(|&h| h == "timestamp").unwrap();

    let ts1: u64 = lines[1].split(',').nth(ts_idx).unwrap().parse().unwrap();
    let ts2: u64 = lines[2].split(',').nth(ts_idx).unwrap().parse().unwrap();
    assert!(ts2 >= ts1, "timestamps must be non-decreasing");
}

/// Helper: parse a CSV row into a lookup closure by column name.
/// Returns a closure `col(name) -> &str`.
fn csv_row_col<'h, 'r>(
    headers: &'h [&'r str],
    row: &'r [&'r str],
) -> impl Fn(&str) -> &'r str + 'h {
    move |name: &str| {
        let idx = headers
            .iter()
            .position(|&h| h == name)
            .unwrap_or_else(|| panic!("column '{name}' not found in CSV header"));
        row[idx]
    }
}

/// T-DSK-01 (CSV): disk_read_bytes and disk_write_bytes are >= 0.
#[test]
fn test_csv_disk_io_bytes_nonneg() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);
    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();
    let col = csv_row_col(&headers, &row);

    let read:  u64 = col("system_disk_read_bytes").parse().expect("system_disk_read_bytes: not u64");
    let write: u64 = col("system_disk_write_bytes").parse().expect("system_disk_write_bytes: not u64");
    // u64 is always >= 0; these assertions guard against future type changes.
    let _ = (read, write);
}

/// T-NET-01 (CSV): net_recv_bytes and net_sent_bytes are >= 0.
#[test]
fn test_csv_net_bytes_nonneg() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);
    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();
    let col = csv_row_col(&headers, &row);

    let recv: u64 = col("system_net_recv_bytes").parse().expect("system_net_recv_bytes: not u64");
    let sent: u64 = col("system_net_sent_bytes").parse().expect("system_net_sent_bytes: not u64");
    let _ = (recv, sent);
}

/// T-DSK-02 (CSV): disk_space_used_gb + disk_space_free_gb <= disk_space_total_gb.
#[test]
fn test_csv_disk_space_invariant() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);
    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();
    let col = csv_row_col(&headers, &row);

    let total: f64 = col("system_disk_space_total_gb").parse().expect("system_disk_space_total_gb");
    let used:  f64 = col("system_disk_space_used_gb").parse().expect("system_disk_space_used_gb");
    let free:  f64 = col("system_disk_space_free_gb").parse().expect("system_disk_space_free_gb");
    assert!(
        used + free <= total * 1.001, // 0.1% tolerance for floating-point rounding
        "system_disk_space_used_gb({used:.4}) + system_disk_space_free_gb({free:.4}) > \
         system_disk_space_total_gb({total:.4})"
    );
}

/// T-MEM-01 (CSV): all memory columns parse as non-negative integers.
#[test]
fn test_csv_memory_fields_nonneg() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);
    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();
    let col = csv_row_col(&headers, &row);

    ["system_memory_free_mib", "system_memory_used_mib", "system_memory_buffers_mib",
     "system_memory_cached_mib", "system_memory_active_mib", "system_memory_inactive_mib"].iter().for_each(|name| {
        let v: u64 = col(name).parse().unwrap_or_else(|_| panic!("{name}: not u64"));
        let _ = v; // u64 is always >= 0; parse success is the key assertion
    });
}

/// cpu time fields (utime, stime) must parse as non-negative floats.
#[test]
fn test_csv_cpu_time_fields_nonneg() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);
    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();
    let col = csv_row_col(&headers, &row);

    let utime: f64 = col("system_utime").parse().expect("system_utime: not f64");
    let stime: f64 = col("system_stime").parse().expect("system_stime: not f64");
    assert!(utime >= 0.0, "system_utime must be >= 0, got {utime}");
    assert!(stime >= 0.0, "system_stime must be >= 0, got {stime}");
}

/// T-GPU-01 (CSV): gpu_usage and gpu_vram parse as non-negative floats;
/// gpu_utilized parses as a non-negative integer.
#[test]
fn test_csv_gpu_fields_nonneg() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);
    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();
    let col = csv_row_col(&headers, &row);

    let usage:    f64 = col("system_gpu_usage").parse().expect("system_gpu_usage: not f64");
    let vram:     f64 = col("system_gpu_vram_mib").parse().expect("system_gpu_vram_mib: not f64");
    let utilized: u32 = col("system_gpu_utilized").parse().expect("system_gpu_utilized: not u32");
    assert!(usage >= 0.0,    "system_gpu_usage must be >= 0, got {usage}");
    assert!(vram  >= 0.0,    "system_gpu_vram_mib must be >= 0, got {vram}");
    let _ = utilized; // u32 is always >= 0
}

// T-GPU-P4: process_gpu_vram_mib and process_gpu_utilized are present in the
// CSV header and, when a PID is tracked, their values are either empty (CPU-only
// host) or non-negative numbers.  process_gpu_usage is always empty.
#[test]
fn test_csv_process_gpu_columns_parse() {
    // Track the tracker's own PID so process columns are populated.
    let pid_str = std::process::id().to_string();
    let lines = collect_lines(
        &["--interval", "1", "--format", "csv", "--pid", &pid_str],
        2,
    );
    assert!(lines.len() >= 2, "expected at least 2 CSV lines");
    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();
    let col = csv_row_col(&headers, &row);

    // process_gpu_usage must always be empty (NVML per-process util unavailable).
    assert_eq!(col("process_gpu_usage"), "",
        "process_gpu_usage must always be empty");

    // process_gpu_vram_mib: empty on CPU-only hosts, non-negative f64 on GPU hosts.
    let vram_str = col("process_gpu_vram_mib");
    if !vram_str.is_empty() {
        let v: f64 = vram_str.parse()
            .unwrap_or_else(|_| panic!("process_gpu_vram_mib not a number: {vram_str:?}"));
        assert!(v >= 0.0, "process_gpu_vram_mib must be >= 0, got {v}");
    }

    // process_gpu_utilized: empty on CPU-only hosts, non-negative u32 on GPU hosts.
    let utilized_str = col("process_gpu_utilized");
    if !utilized_str.is_empty() {
        let _u: u32 = utilized_str.parse()
            .unwrap_or_else(|_| panic!("process_gpu_utilized not a u32: {utilized_str:?}"));
    }
}

// ---------------------------------------------------------------------------
// Shell-wrapper mode (Priority 2)
// ---------------------------------------------------------------------------

/// Shell-wrapper: tracker should exit with the child's exit code (0).
#[test]
fn test_shell_wrapper_propagates_exit_zero() {
    let status = run_to_exit(&["--interval", "1", "--", "true"], Duration::from_secs(8));
    assert_eq!(
        status.code(),
        Some(0),
        "tracker should exit 0 when wrapped command exits 0"
    );
}

/// Shell-wrapper: tracker should exit with the child's non-zero exit code.
#[test]
fn test_shell_wrapper_propagates_exit_nonzero() {
    let status = run_to_exit(&["--interval", "1", "--", "false"], Duration::from_secs(8));
    assert_ne!(
        status.code(),
        Some(0),
        "tracker should exit non-zero when wrapped command exits non-zero"
    );
}

/// Shell-wrapper: tracker emits at least one valid JSON sample while monitoring.
#[test]
fn test_shell_wrapper_emits_json_samples() {
    // sleep 5 gives enough time to collect one sample before we kill it
    let lines = collect_lines(&["--interval", "1", "--", "sleep", "5"], 1);
    assert!(!lines.is_empty(), "should emit at least one sample in wrapper mode");
    let v: serde_json::Value =
        serde_json::from_str(&lines[0]).expect("sample should be valid JSON");
    assert!(
        v["timestamp_secs"].as_u64().unwrap_or(0) > 0,
        "timestamp_secs should be a positive integer"
    );
}

// ---------------------------------------------------------------------------
// Section 9.3 metadata flags (Priority 2)
// ---------------------------------------------------------------------------

/// All Section 9.3 metadata flags must be accepted without error.
#[test]
fn test_all_metadata_flags_accepted() {
    let lines = collect_lines(
        &[
            "--interval",        "1",
            "--project-name",    "test-project",
            "--stage-name",      "eval",
            "--task-name",       "infer",
            "--team",            "ml-team",
            "--env",             "staging",
            "--language",        "rust",
            "--orchestrator",    "airflow",
            "--executor",        "k8s",
            "--external-run-id", "abc-123",
            "--container-image", "my-image:latest",
            "--tag",             "foo=bar",
            "--tag",             "baz=qux",
        ],
        1,
    );
    assert_eq!(lines.len(), 1, "binary should start and emit a sample with all metadata flags set");
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert!(v.is_object());
}

/// TRACKER_* environment variables must be accepted without error.
#[test]
fn test_tracker_env_vars_accepted() {
    let mut child = Command::new(BINARY)
        .args(["--interval", "1"])
        .env("TRACKER_JOB_NAME",     "env-job")
        .env("TRACKER_PROJECT_NAME", "env-project")
        .env("TRACKER_STAGE_NAME",   "env-stage")
        .env("TRACKER_TASK_NAME",    "env-task")
        .env("TRACKER_TEAM",         "env-team")
        .env("TRACKER_ENV",          "env-prod")
        .env("TRACKER_LANGUAGE",     "rust")
        .env("TRACKER_ORCHESTRATOR", "airflow")
        .env("TRACKER_EXECUTOR",     "k8s")
        .env("TRACKER_EXTERNAL_RUN_ID", "ext-42")
        .env("TRACKER_CONTAINER_IMAGE", "img:tag")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn binary");

    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        reader.lines().take(1).for_each(|line| {
            let _ = tx.send(line.unwrap_or_default());
        });
    });

    let line = rx.recv_timeout(TIMEOUT).expect("timed out waiting for first sample");
    child.kill().ok();
    child.wait().ok();

    let v: serde_json::Value =
        serde_json::from_str(&line).expect("output should be valid JSON");
    assert!(v.is_object(), "should emit a valid JSON object with env vars set");
}

/// --tag flag must be accepted when given multiple times.
#[test]
fn test_tag_flag_repeatable() {
    let lines = collect_lines(
        &[
            "--interval", "1",
            "--tag", "key1=value1",
            "--tag", "key2=value2",
            "--tag", "key3=value3",
        ],
        1,
    );
    assert_eq!(lines.len(), 1, "binary should start normally with multiple --tag flags");
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert!(v.is_object());
}

// ---------------------------------------------------------------------------
// T-OUT-02 / T-OUT-03: output metadata
// ---------------------------------------------------------------------------

/// T-OUT-02: timestamp_secs is a positive integer.
#[test]
fn test_json_timestamp_secs_is_positive_integer() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let ts = v["timestamp_secs"]
        .as_u64()
        .expect("timestamp_secs must be a non-negative integer");
    assert!(ts > 0, "timestamp_secs must be a positive integer, got {ts}");
}

/// T-OUT-03: resource-tracker-rs-version key present and is a semver string.
#[test]
fn test_json_version_key_is_semver() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let version_key = "resource-tracker-rs-version";
    let ver = v[version_key]
        .as_str()
        .unwrap_or_else(|| panic!("'{version_key}' missing or not a string"));
    // Must contain at least two dots (semver: major.minor.patch).
    assert!(
        ver.chars().filter(|&c| c == '.').count() >= 2,
        "version '{ver}' does not look like semver (major.minor.patch)"
    );
}

// ---------------------------------------------------------------------------
// T-CPU-03 / T-CPU-04: process metrics
// ---------------------------------------------------------------------------

/// T-CPU-03: Without --pid, process_cores_used and process_child_count are null.
#[test]
fn test_json_process_fields_null_without_pid() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert!(
        v["cpu"]["process_cores_used"].is_null(),
        "process_cores_used must be null without --pid"
    );
    assert!(
        v["cpu"]["process_child_count"].is_null(),
        "process_child_count must be null without --pid"
    );
}

/// T-CPU-04: With --pid <self>, process_cores_used is >= 0.
#[test]
fn test_json_process_cores_used_nonneg_with_pid() {
    // Use the current test process PID so it is guaranteed to be running.
    let pid = std::process::id().to_string();
    let lines = collect_lines(&["--interval", "1", "--pid", &pid], 1);
    assert!(!lines.is_empty(), "expected at least one sample with --pid");
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let cores_used = v["cpu"]["process_cores_used"]
        .as_f64()
        .expect("process_cores_used must be a number when --pid is supplied");
    assert!(cores_used >= 0.0, "process_cores_used must be >= 0, got {cores_used}");
}

// ---------------------------------------------------------------------------
// T-MEM-01 through T-MEM-04: memory invariants
// ---------------------------------------------------------------------------

/// T-MEM-01: free_mib + used_mib + buffers_mib + cached_mib <= total_mib.
#[test]
fn test_json_memory_components_dont_exceed_total() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let total    = v["memory"]["total_mib"].as_u64().expect("total_mib");
    let free     = v["memory"]["free_mib"].as_u64().expect("free_mib");
    let used     = v["memory"]["used_mib"].as_u64().expect("used_mib");
    let buffers  = v["memory"]["buffers_mib"].as_u64().expect("buffers_mib");
    let cached   = v["memory"]["cached_mib"].as_u64().expect("cached_mib");
    let sum = free + used + buffers + cached;
    assert!(
        sum <= total,
        "free({free}) + used({used}) + buffers({buffers}) + cached({cached}) = {sum} > total({total})"
    );
}

/// T-MEM-02: used_pct is in [0.0, 100.0].
#[test]
fn test_json_memory_used_pct_in_range() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let pct = v["memory"]["used_pct"].as_f64().expect("used_pct");
    assert!(pct >= 0.0 && pct <= 100.0, "used_pct must be in [0.0, 100.0], got {pct}");
}

/// T-MEM-03: swap_used_pct is 0.0 when swap_total_mib == 0 (skip if swap present).
#[test]
fn test_json_swap_used_pct_zero_when_no_swap() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let swap_total = v["memory"]["swap_total_mib"].as_u64().unwrap_or(0);
    if swap_total == 0 {
        let swap_pct = v["memory"]["swap_used_pct"].as_f64().expect("swap_used_pct");
        assert!(
            swap_pct == 0.0,
            "swap_used_pct must be 0.0 when swap_total_mib == 0, got {swap_pct}"
        );
    }
    // If swap is present, no assertion is needed; the field may be nonzero.
}

/// T-MEM-04: available_mib <= total_mib.
#[test]
fn test_json_memory_available_le_total() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let available = v["memory"]["available_mib"].as_u64().expect("available_mib");
    let total     = v["memory"]["total_mib"].as_u64().expect("total_mib");
    assert!(
        available <= total,
        "available_mib ({available}) must be <= total_mib ({total})"
    );
}

// ---------------------------------------------------------------------------
// T-NET-01 through T-NET-03: network metrics
// ---------------------------------------------------------------------------

/// T-NET-01: rx_bytes_per_sec and tx_bytes_per_sec are >= 0.0 for every interface.
#[test]
fn test_json_network_bytes_per_sec_nonneg() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let ifaces = v["network"].as_array().expect("network must be an array");
    ifaces.iter().for_each(|iface| {
        let name = iface["interface"].as_str().unwrap_or("?");
        let rx = iface["rx_bytes_per_sec"].as_f64().expect("rx_bytes_per_sec");
        let tx = iface["tx_bytes_per_sec"].as_f64().expect("tx_bytes_per_sec");
        assert!(rx >= 0.0, "rx_bytes_per_sec must be >= 0 for {name}, got {rx}");
        assert!(tx >= 0.0, "tx_bytes_per_sec must be >= 0 for {name}, got {tx}");
    });
}

/// T-NET-02: rx_bytes_total is non-decreasing across two consecutive samples.
#[test]
fn test_json_network_rx_bytes_total_nondecreasing() {
    let lines = collect_lines(&["--interval", "1"], 2);
    assert_eq!(lines.len(), 2, "expected 2 JSON samples");
    let a: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let b: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    let ifaces_a = a["network"].as_array().expect("network array");
    let ifaces_b = b["network"].as_array().expect("network array");
    ifaces_a.iter().for_each(|ia| {
        let name = ia["interface"].as_str().unwrap_or("");
        if let Some(ib) = ifaces_b.iter().find(|x| x["interface"].as_str() == Some(name)) {
            let total_a = ia["rx_bytes_total"].as_u64().unwrap_or(0);
            let total_b = ib["rx_bytes_total"].as_u64().unwrap_or(0);
            assert!(
                total_b >= total_a,
                "rx_bytes_total for {name} must not decrease: {total_a} -> {total_b}"
            );
        }
    });
}

/// T-NET-03: Loopback interface "lo" must not appear in network output.
#[test]
fn test_json_network_no_loopback_interface() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let ifaces = v["network"].as_array().expect("network must be an array");
    ifaces.iter().for_each(|iface| {
        let name = iface["interface"].as_str().unwrap_or("");
        assert_ne!(name, "lo", "loopback interface 'lo' must not appear in network output");
    });
}

// ---------------------------------------------------------------------------
// T-DSK-01 through T-DSK-03: disk metrics
// ---------------------------------------------------------------------------

/// T-DSK-01: read_bytes_per_sec and write_bytes_per_sec are >= 0.0 for every device.
#[test]
fn test_json_disk_bytes_per_sec_nonneg() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let disks = v["disk"].as_array().expect("disk must be an array");
    disks.iter().for_each(|disk| {
        let dev = disk["device"].as_str().unwrap_or("?");
        let r = disk["read_bytes_per_sec"].as_f64().expect("read_bytes_per_sec");
        let w = disk["write_bytes_per_sec"].as_f64().expect("write_bytes_per_sec");
        assert!(r >= 0.0, "read_bytes_per_sec must be >= 0 for {dev}, got {r}");
        assert!(w >= 0.0, "write_bytes_per_sec must be >= 0 for {dev}, got {w}");
    });
}

/// T-DSK-02: used_bytes + available_bytes <= total_bytes for every mount.
#[test]
fn test_json_disk_mount_space_invariant() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let disks = v["disk"].as_array().expect("disk must be an array");
    for disk in disks {
        let dev = disk["device"].as_str().unwrap_or("?");
        let mounts = match disk["mounts"].as_array() {
            Some(m) => m,
            None    => continue,
        };
        mounts.iter().for_each(|mount| {
            let mp    = mount["mount_point"].as_str().unwrap_or("?");
            let total = mount["total_bytes"].as_u64().expect("total_bytes");
            let used  = mount["used_bytes"].as_u64().expect("used_bytes");
            let avail = mount["available_bytes"].as_u64().expect("available_bytes");
            assert!(
                used + avail <= total,
                "used({used}) + avail({avail}) > total({total}) for {dev}:{mp}"
            );
        });
    }
}

/// T-DSK-03: capacity_bytes is > 0 when present (not null).
#[test]
fn test_json_disk_capacity_positive_when_present() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let disks = v["disk"].as_array().expect("disk must be an array");
    disks.iter().for_each(|disk| {
        let dev = disk["device"].as_str().unwrap_or("?");
        if let Some(cap) = disk["capacity_bytes"].as_u64() {
            assert!(cap > 0, "capacity_bytes must be > 0 when present, device {dev}");
        }
    });
}

// ---------------------------------------------------------------------------
// T-GPU-01: GPU vec is empty on CPU-only host
// ---------------------------------------------------------------------------

/// T-GPU-01: On a CPU-only host the gpu array is empty.
/// This test always passes on the development machine; it is a documentation
/// of the expected behavior and will fail if a GPU is unexpectedly reported.
#[test]
fn test_json_gpu_empty_on_cpu_only_host() {
    // Only assert empty if there is no GPU driver present.
    // Check for nvidia/amd GPU presence via /sys or /dev before asserting.
    let has_gpu = std::path::Path::new("/dev/nvidia0").exists()
        || std::path::Path::new("/dev/dri/renderD128").exists();

    if !has_gpu {
        let lines = collect_lines(&["--interval", "1"], 1);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        let gpu = v["gpu"].as_array().expect("gpu must be an array");
        assert!(gpu.is_empty(), "gpu array must be empty on a CPU-only host");
    }
}

// ---------------------------------------------------------------------------
// T-CLD-01: startup does not hang on non-cloud host
// ---------------------------------------------------------------------------

/// T-CLD-01: First sample arrives within 5 seconds even on a non-cloud host
/// where all IMDS probes fail (each probe has a 2s timeout; they run in parallel).
#[test]
fn test_first_sample_arrives_within_5s() {
    let start = std::time::Instant::now();
    let lines = collect_lines(&["--interval", "1"], 1);
    let elapsed = start.elapsed();
    assert!(!lines.is_empty(), "expected at least one sample");
    assert!(
        elapsed < Duration::from_secs(5),
        "first sample took {:?}, must arrive in < 5s (IMDS probes must not block startup)",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// T-CFG-04 / T-CFG-05 / T-CFG-06: TOML config file
// ---------------------------------------------------------------------------

fn write_temp_toml(content: &str) -> std::path::PathBuf {
    let name = format!(
        "rt-test-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    );
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, content).expect("failed to write temp TOML");
    path
}

/// T-CFG-04: TOML `interval_secs = 2` produces ~2s spacing between samples.
#[test]
fn test_toml_interval_secs_controls_sample_spacing() {
    let toml = write_temp_toml("[tracker]\ninterval_secs = 2\n");
    let config_path = toml.to_string_lossy().to_string();

    let start = std::time::Instant::now();
    let lines = collect_lines(&["--config", &config_path], 2);
    let elapsed = start.elapsed();

    let _ = std::fs::remove_file(&toml);

    assert_eq!(lines.len(), 2, "expected 2 samples");
    // With interval=2: warm-up=2s + first sample, sleep 2s, second sample ~= 4s total.
    // Allow generous bounds: [3s, 10s].
    assert!(
        elapsed >= Duration::from_secs(3),
        "elapsed {:?} too short for 2s interval (expected >= 3s)",
        elapsed
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "elapsed {:?} too long (expected < 10s)",
        elapsed
    );
}

/// T-CFG-05: CLI `--interval 2` overrides TOML `interval_secs = 5`.
/// Two samples must arrive in < 8s (not ~10s which a 5s interval would require).
#[test]
fn test_cli_interval_overrides_toml_interval() {
    let toml = write_temp_toml("[tracker]\ninterval_secs = 5\n");
    let config_path = toml.to_string_lossy().to_string();

    let start = std::time::Instant::now();
    let lines = collect_lines(&["--config", &config_path, "--interval", "2"], 2);
    let elapsed = start.elapsed();

    let _ = std::fs::remove_file(&toml);

    assert_eq!(lines.len(), 2, "expected 2 samples");
    // With CLI --interval 2 overriding TOML 5: two samples in ~4s.
    // If TOML were used, two samples would take ~10s.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {:?} suggests TOML interval (5s) was used instead of CLI (2s)",
        elapsed
    );
}

/// T-CFG-06: A nonexistent TOML config path silently falls back to defaults.
#[test]
fn test_missing_toml_config_falls_back_to_defaults() {
    let lines = collect_lines(
        &["--config", "/tmp/this-config-does-not-exist-rt-test.toml", "--interval", "1"],
        1,
    );
    assert_eq!(lines.len(), 1, "binary must start normally when config file is missing");
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert!(v.is_object(), "output must be valid JSON with fallback defaults");
}

// ---------------------------------------------------------------------------
// T-EOR-01: SIGTERM causes clean exit with code 0
// ---------------------------------------------------------------------------

/// T-EOR-01: On SIGTERM the binary flushes and exits with code 0.
#[test]
fn test_sigterm_exits_zero() {
    let mut child = Command::new(BINARY)
        .args(["--interval", "1"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn binary");

    // Wait for the first sample to confirm the binary is running.
    // The reader thread continues draining stderr after sending the first line
    // so the pipe stays open; dropping it after .take(1) would cause the binary's
    // next eprintln! to get a broken pipe (SIGPIPE / write error) on some platforms.
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        let mut sent = false;
        reader.lines().for_each(|line| {
            if !sent {
                let _ = tx.send(line.unwrap_or_default());
                sent = true;
            }
            // Keep reading so the pipe stays open until the binary exits.
        });
    });
    rx.recv_timeout(TIMEOUT).expect("binary did not emit a sample before SIGTERM");

    // Send SIGTERM.
    let pid = child.id().to_string();
    Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .expect("failed to send SIGTERM");

    // Wait for exit (up to 5s).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Ok(Some(s)) = child.try_wait() {
            break s;
        }
        if std::time::Instant::now() > deadline {
            child.kill().ok();
            child.wait().ok();
            panic!("binary did not exit within 5s after SIGTERM");
        }
        thread::sleep(Duration::from_millis(100));
    };

    assert_eq!(
        status.code(),
        Some(0),
        "binary must exit with code 0 after SIGTERM, got: {:?}",
        status.code()
    );
}

// ---------------------------------------------------------------------------
// Write the exact S3 batch file to disk for manual inspection
// ---------------------------------------------------------------------------

/// Runs the binary in CSV mode, captures 3 samples (header + 2 data rows),
/// gzip-compresses them exactly as the S3 upload path does, and writes the
/// result to /tmp/resource-tracker-batch-test.csv.gz.
///
/// Run with: cargo test write_s3_batch_to_disk -- --nocapture
/// Inspect:  gunzip -c /tmp/resource-tracker-batch-test.csv.gz
#[test]
fn test_write_s3_batch_to_disk() {
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    // header line + 2 data rows = 3 lines from the binary
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 3);
    assert!(lines.len() >= 2, "expected at least header + 1 data row, got {}", lines.len());

    let mut csv = String::new();
    lines.iter().for_each(|l| { csv.push_str(l); csv.push('\n'); });

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(csv.as_bytes()).expect("gzip write failed");
    let compressed = encoder.finish().expect("gzip finish failed");

    let path = "/tmp/resource-tracker-batch-test.csv.gz";
    std::fs::write(path, &compressed).expect("failed to write batch file");
    println!("wrote {} bytes ({} csv bytes) to {path}", compressed.len(), csv.len());
    println!("inspect: gunzip -c {path}");
}

// ---------------------------------------------------------------------------
// --quiet / --output / TRACKER_QUIET / TRACKER_OUTPUT
// ---------------------------------------------------------------------------

/// Spawn the binary, let it run for `duration`, kill it, and return every
/// non-empty line that appeared on stdout.  Used by output-sink tests where
/// `collect_lines` would block up to 10 s waiting for a line that never arrives.
/// Metrics are written to stderr; stdout is reserved for the tracked app.
fn run_for(args: &[&str], duration: Duration) -> Vec<String> {
    let mut child = Command::new(BINARY)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn binary");

    thread::sleep(duration);
    child.kill().ok();

    let out = child.wait_with_output().expect("wait_with_output failed");
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// Spawn the binary with `env_key=env_val` set, let it run for `duration`,
/// kill it, and return stderr lines.
fn run_for_with_env(args: &[&str], env_key: &str, env_val: &str, duration: Duration) -> Vec<String> {
    let mut child = Command::new(BINARY)
        .args(args)
        .env(env_key, env_val)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn binary");

    thread::sleep(duration);
    child.kill().ok();

    let out = child.wait_with_output().expect("wait_with_output failed");
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// Warm-up takes ~1 s; after 3 s a normal run has emitted at least 1 sample.
/// Used as the wait duration for output-sink tests.
const OUTPUT_TEST_WAIT: Duration = Duration::from_secs(3);

/// PID-namespaced temp path so parallel test runs never collide.
fn tmp_path(suffix: &str) -> String {
    std::env::temp_dir()
        .join(format!("rt_rs_{}_{}", std::process::id(), suffix))
        .to_string_lossy()
        .into_owned()
}

/// --quiet suppresses metric output (JSON/CSV lines) on stderr.
/// Non-metric warnings may still appear; we only assert no metric data.
#[test]
fn test_quiet_produces_no_stderr() {
    let lines = run_for(&["--quiet", "--interval", "1"], OUTPUT_TEST_WAIT);
    lines.iter().for_each(|l| {
        let first = l.chars().next().unwrap_or(' ');
        assert!(
            first != '{' && !first.is_ascii_digit(),
            "--quiet should suppress metric output, but got: {l:?}"
        );
    });
}

/// Without --quiet the binary does produce stderr metrics (sanity control for the above).
#[test]
fn test_no_quiet_produces_stderr() {
    let lines = run_for(&["--interval", "1"], OUTPUT_TEST_WAIT);
    assert!(
        !lines.is_empty(),
        "expected stderr metric output without --quiet"
    );
}

/// --output FILE writes JSON to the file and nothing to stderr.
#[test]
fn test_output_file_json() {
    let path = tmp_path("out.json");
    let stderr_lines = run_for(&["--output", &path, "--interval", "1"], OUTPUT_TEST_WAIT);

    assert!(
        stderr_lines.is_empty(),
        "--output should suppress stderr, got {} line(s)",
        stderr_lines.len()
    );

    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("output file {path} not readable: {e}"));
    std::fs::remove_file(&path).ok();

    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert!(!lines.is_empty(), "output file should contain at least one JSON line");
    let _: serde_json::Value = serde_json::from_str(lines[0])
        .unwrap_or_else(|e| panic!("first line of output file is not valid JSON: {e}\n{}", lines[0]));
}

/// --output FILE with --format csv writes header + rows to the file, nothing to stderr.
#[test]
fn test_output_file_csv() {
    let path = tmp_path("out.csv");
    let stderr_lines = run_for(
        &["--output", &path, "--format", "csv", "--interval", "1"],
        OUTPUT_TEST_WAIT,
    );

    assert!(
        stderr_lines.is_empty(),
        "--output should suppress stderr in CSV mode, got {} line(s)",
        stderr_lines.len()
    );

    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("output file {path} not readable: {e}"));
    std::fs::remove_file(&path).ok();

    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert!(lines.len() >= 2, "CSV output file should have header + at least 1 data row");
    assert_eq!(
        lines[0], EXPECTED_HEADER,
        "CSV header in output file does not match expected"
    );
}

/// TRACKER_QUIET=1 env var suppresses metric output (JSON/CSV lines).
/// Non-metric warnings may still appear on stderr; we only assert no metric data.
#[test]
fn test_tracker_quiet_env_var() {
    let lines = run_for_with_env(
        &["--interval", "1"],
        "TRACKER_QUIET", "1",
        OUTPUT_TEST_WAIT,
    );
    // Metric lines start with '{' (JSON) or a digit (CSV timestamp).
    lines.iter().for_each(|l| {
        let first = l.chars().next().unwrap_or(' ');
        assert!(
            first != '{' && !first.is_ascii_digit(),
            "TRACKER_QUIET=1 should suppress metric output, but got: {l:?}"
        );
    });
}

/// TRACKER_OUTPUT env var behaves the same as --output.
#[test]
fn test_tracker_output_env_var() {
    let path = tmp_path("env_out.json");
    let stderr_lines = run_for_with_env(
        &["--interval", "1"],
        "TRACKER_OUTPUT", &path,
        OUTPUT_TEST_WAIT,
    );

    assert!(
        stderr_lines.is_empty(),
        "TRACKER_OUTPUT should suppress stderr, got {} line(s)",
        stderr_lines.len()
    );

    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("output file {path} not readable: {e}"));
    std::fs::remove_file(&path).ok();

    assert!(
        content.lines().any(|l| !l.is_empty()),
        "output file written via TRACKER_OUTPUT should be non-empty"
    );
}
