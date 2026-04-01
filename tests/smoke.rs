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
fn collect_lines(args: &[&str], n: usize) -> Vec<String> {
    let mut child = Command::new(BINARY)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn resource-tracker-rs binary");

    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
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
fn interval_zero_exits_nonzero() {
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
fn json_is_valid() {
    let lines = collect_lines(&["--interval", "1"], 1);
    assert_eq!(lines.len(), 1, "expected exactly 1 JSON line");
    let v: serde_json::Value =
        serde_json::from_str(&lines[0]).expect("output is not valid JSON");
    assert!(v.is_object(), "top-level value should be a JSON object");
}

#[test]
fn json_version_field_present() {
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
fn json_two_samples_have_nondecreasing_timestamps() {
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
fn json_cpu_fields_present() {
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
fn json_utilization_pct_is_fractional_cores_not_percentage() {
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
fn json_total_cores_field_absent() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert!(
        v["cpu"]["total_cores"].is_null(),
        "total_cores must not appear in cpu JSON -- it belongs in host discovery (Section 8.1)"
    );
}

#[test]
fn json_process_count_at_least_one() {
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
fn json_memory_fields_are_mib() {
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
fn json_memory_kib_fields_absent() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    for field in &["total_kib", "free_kib", "available_kib", "used_kib",
                   "buffers_kib", "cached_kib", "active_kib", "inactive_kib"] {
        assert!(
            v["memory"][field].is_null(),
            "old field '{field}' must not appear in memory JSON (renamed to _mib)"
        );
    }
}

// ---------------------------------------------------------------------------
// CSV output
// ---------------------------------------------------------------------------

const EXPECTED_HEADER: &str =
    "timestamp,processes,utime,stime,cpu_usage,\
     memory_free,memory_used,memory_buffers,memory_cached,memory_active,memory_inactive,\
     disk_read_bytes,disk_write_bytes,\
     disk_space_total_gb,disk_space_used_gb,disk_space_free_gb,\
     net_recv_bytes,net_sent_bytes,\
     gpu_usage,gpu_vram,gpu_utilized";

#[test]
fn csv_header_matches_expected() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2, "expected header + at least 1 data row");
    assert_eq!(lines[0], EXPECTED_HEADER, "CSV header mismatch");
}

#[test]
fn csv_column_count_consistent() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);
    let header_count = lines[0].split(',').count();
    let row_count    = lines[1].split(',').count();
    assert_eq!(header_count, row_count, "header and row column counts differ");
}

/// cpu_usage must be fractional cores (>= 0, well below any percentage ceiling).
/// Since utilization_pct is now fractional cores, cpu_usage == utilization_pct directly.
#[test]
fn csv_cpu_usage_is_fractional_cores() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);

    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();
    let idx = headers.iter().position(|&h| h == "cpu_usage").unwrap();

    let cpu_usage: f64 = row[idx].parse().expect("cpu_usage: not f64");
    assert!(cpu_usage >= 0.0, "cpu_usage must be >= 0");
    // On any real machine the value is well below 1024; a percentage-scale bug
    // would push it to e.g. 62.5 on a loaded single core.
    // We check it is NOT an unreasonably large percentage-like number.
    let n_cpus = num_cpus::get();
    assert!(
        cpu_usage <= n_cpus as f64 * 1.05,
        "cpu_usage ({cpu_usage}) looks like a percentage rather than fractional cores \
         (n_cpus = {n_cpus})"
    );
}

#[test]
fn csv_values_parse_and_are_sane() {
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

    let processes: u32 = col("processes").parse().expect("processes: not u32");
    assert!(processes >= 1, "processes should be >= 1");

    // memory columns are MiB values; they should be positive but much smaller
    // than old KiB values (total RAM is typically 1000-65536 MiB)
    let memory_free: u64 = col("memory_free").parse().expect("memory_free: not u64");
    assert!(memory_free > 0, "memory_free should be > 0");
    assert!(memory_free < 10_000_000, "memory_free looks like KiB, not MiB");

    let memory_used: u64 = col("memory_used").parse().expect("memory_used: not u64");
    assert!(memory_used > 0, "memory_used should be > 0");
    assert!(memory_used < 10_000_000, "memory_used looks like KiB, not MiB");

    let disk_total: f64 = col("disk_space_total_gb")
        .parse()
        .expect("disk_space_total_gb: not f64");
    assert!(disk_total > 0.0, "disk_space_total_gb should be > 0");

    let gpu_utilized: u32 = col("gpu_utilized").parse().expect("gpu_utilized: not u32");
    let _ = gpu_utilized; // 0 is valid on CPU-only hosts
}

#[test]
fn csv_two_rows_have_nondecreasing_timestamps() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 3);
    assert!(lines.len() >= 3, "expected header + 2 data rows");

    let headers: Vec<&str> = lines[0].split(',').collect();
    let ts_idx = headers.iter().position(|&h| h == "timestamp").unwrap();

    let ts1: u64 = lines[1].split(',').nth(ts_idx).unwrap().parse().unwrap();
    let ts2: u64 = lines[2].split(',').nth(ts_idx).unwrap().parse().unwrap();
    assert!(ts2 >= ts1, "timestamps must be non-decreasing");
}

// ---------------------------------------------------------------------------
// Shell-wrapper mode (Priority 2)
// ---------------------------------------------------------------------------

/// Shell-wrapper: tracker should exit with the child's exit code (0).
#[test]
fn shell_wrapper_propagates_exit_zero() {
    let status = run_to_exit(&["--interval", "1", "--", "true"], Duration::from_secs(8));
    assert_eq!(
        status.code(),
        Some(0),
        "tracker should exit 0 when wrapped command exits 0"
    );
}

/// Shell-wrapper: tracker should exit with the child's non-zero exit code.
#[test]
fn shell_wrapper_propagates_exit_nonzero() {
    let status = run_to_exit(&["--interval", "1", "--", "false"], Duration::from_secs(8));
    assert_ne!(
        status.code(),
        Some(0),
        "tracker should exit non-zero when wrapped command exits non-zero"
    );
}

/// Shell-wrapper: tracker emits at least one valid JSON sample while monitoring.
#[test]
fn shell_wrapper_emits_json_samples() {
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
fn all_metadata_flags_accepted() {
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
fn tracker_env_vars_accepted() {
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
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn binary");

    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().take(1) {
            let _ = tx.send(line.unwrap_or_default());
        }
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
fn tag_flag_repeatable() {
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
