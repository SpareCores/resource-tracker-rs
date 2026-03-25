/// Smoke tests for resource-tracker-rs output formats.
///
/// Each test spawns the release binary with --interval 1, collects a small
/// number of lines, kills the process, then asserts on the content.  Using
/// interval=1 keeps wall-clock time short: warm-up takes ~1 s, first row
/// appears immediately after.
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const BINARY: &str = env!("CARGO_BIN_EXE_resource-tracker-rs");
const TIMEOUT: Duration = Duration::from_secs(10);

/// Spawn the binary with `args`, collect `n` stdout lines, then kill it.
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

// ---------------------------------------------------------------------------
// JSON tests
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
fn json_cpu_fields() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();

    assert!(v["cpu"]["total_cores"].as_u64().unwrap_or(0) >= 1);
    assert!(v["cpu"]["utilization_pct"].is_number());
    assert!(v["cpu"]["per_core_pct"].is_array());

    // Python-equivalent fields
    let process_count = v["cpu"]["process_count"].as_u64()
        .expect("process_count missing or not a number");
    assert!(process_count >= 1, "process_count should be >= 1");

    let utime = v["cpu"]["utime_secs"].as_f64()
        .expect("utime_secs missing or not a number");
    assert!(utime > 0.0, "utime_secs should be > 0");

    let stime = v["cpu"]["stime_secs"].as_f64()
        .expect("stime_secs missing or not a number");
    assert!(stime > 0.0, "stime_secs should be > 0");
}

#[test]
fn json_memory_fields() {
    let lines = collect_lines(&["--interval", "1"], 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();

    assert!(v["memory"]["total_kib"].as_u64().unwrap_or(0) > 0);
    assert!(v["memory"]["used_kib"].is_number());
    assert!(v["memory"]["available_kib"].is_number());
    assert!(v["memory"]["buffers_kib"].is_number());
    assert!(v["memory"]["cached_kib"].is_number());

    // Python-equivalent fields
    let active = v["memory"]["active_kib"].as_u64()
        .expect("active_kib missing or not a number");
    assert!(active > 0, "active_kib should be > 0");

    let inactive = v["memory"]["inactive_kib"].as_u64()
        .expect("inactive_kib missing or not a number");
    assert!(inactive > 0, "inactive_kib should be > 0");
}

#[test]
fn json_two_samples_are_independent() {
    let lines = collect_lines(&["--interval", "1"], 2);
    assert_eq!(lines.len(), 2, "expected 2 JSON lines");

    let a: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let b: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();

    let ts_a = a["timestamp_secs"].as_u64().unwrap();
    let ts_b = b["timestamp_secs"].as_u64().unwrap();
    assert!(ts_b >= ts_a, "timestamps should be non-decreasing");
}

// ---------------------------------------------------------------------------
// CSV tests
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
    assert_eq!(header_count, row_count, "header and row have different column counts");
}

#[test]
fn csv_values_parse_and_are_sane() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 2);
    assert!(lines.len() >= 2);

    let headers: Vec<&str> = lines[0].split(',').collect();
    let row:     Vec<&str> = lines[1].split(',').collect();

    let col = |name: &str| -> &str {
        let idx = headers.iter().position(|&h| h == name)
            .unwrap_or_else(|| panic!("column '{}' not found in header", name));
        row[idx]
    };

    let timestamp: u64 = col("timestamp").parse().expect("timestamp: not u64");
    assert!(timestamp > 0);

    let processes: u32 = col("processes").parse().expect("processes: not u32");
    assert!(processes >= 1, "processes should be >= 1");

    let utime: f64 = col("utime").parse().expect("utime: not f64");
    assert!(utime > 0.0, "utime should be > 0");

    let stime: f64 = col("stime").parse().expect("stime: not f64");
    assert!(stime > 0.0, "stime should be > 0");

    let cpu_usage: f64 = col("cpu_usage").parse().expect("cpu_usage: not f64");
    assert!(cpu_usage >= 0.0, "cpu_usage should be >= 0");

    let memory_free: u64 = col("memory_free").parse().expect("memory_free: not u64");
    assert!(memory_free > 0);

    let memory_used: u64 = col("memory_used").parse().expect("memory_used: not u64");
    assert!(memory_used > 0);

    let memory_active: u64 = col("memory_active").parse().expect("memory_active: not u64");
    assert!(memory_active > 0, "memory_active should be > 0");

    let memory_inactive: u64 = col("memory_inactive").parse().expect("memory_inactive: not u64");
    assert!(memory_inactive > 0, "memory_inactive should be > 0");

    let disk_total: f64 = col("disk_space_total_gb").parse().expect("disk_space_total_gb: not f64");
    assert!(disk_total > 0.0, "disk_space_total_gb should be > 0");

    let gpu_utilized: u32 = col("gpu_utilized").parse().expect("gpu_utilized: not u32");
    let _ = gpu_utilized; // 0 is valid on CPU-only hosts
}

#[test]
fn csv_two_rows_have_increasing_timestamps() {
    let lines = collect_lines(&["--interval", "1", "--format", "csv"], 3);
    assert!(lines.len() >= 3, "expected header + 2 data rows");

    let headers: Vec<&str> = lines[0].split(',').collect();
    let ts_idx = headers.iter().position(|&h| h == "timestamp").unwrap();

    let ts1: u64 = lines[1].split(',').nth(ts_idx).unwrap().parse().unwrap();
    let ts2: u64 = lines[2].split(',').nth(ts_idx).unwrap().parse().unwrap();
    assert!(ts2 >= ts1, "timestamps should be non-decreasing");
}
