/// Cross-implementation comparison tests.
///
/// Runs Python resource-tracker (via `uv`) and resource-tracker-rs (`--format csv`)
/// simultaneously for a short duration, then aligns their CSV output by
/// timestamp and computes a numeric error assessment for each shared column.
///
/// Each column has an explicit tolerance.  The test prints a full comparison
/// table (always visible with `cargo test -- --nocapture`) and fails if any
/// column exceeds its tolerance.
///
/// Requires: `uv` on PATH.  The test is skipped gracefully if uv is absent.
use std::process::{Command, Stdio};
use std::time::Duration;

const BINARY: &str = env!("CARGO_BIN_EXE_resource-tracker-rs");

/// How long to collect from each implementation.
const DURATION_SECS: u64 = 10;
const INTERVAL_SECS: u64 = 1;
/// Hard wall-clock cap - kill both children if either hangs past this.
const MAX_WAIT_SECS: u64 = 10;

/// Python script: drives SystemTracker, writes CSV to argv[1] for argv[2] seconds.
const PYTHON_RUNNER: &str = r#"
import sys, time
from resource_tracker import SystemTracker

output_path = sys.argv[1]
duration    = float(sys.argv[2])

tracker = SystemTracker(interval=1, output_file=output_path, autostart=True)
time.sleep(duration)
tracker.stop()
"#;

// ---------------------------------------------------------------------------
// Temp-file paths - namespaced by PID so concurrent runs and different users
// never collide.
// ---------------------------------------------------------------------------

fn tmp(suffix: &str) -> String {
    let tmp_dir = std::env::temp_dir();
    let pid = std::process::id();
    tmp_dir
        .join(format!("sparecores_compare_{}_{}", pid, suffix))
        .to_string_lossy()
        .into_owned()
}

// ---------------------------------------------------------------------------
// CSV helpers
// ---------------------------------------------------------------------------

struct CsvData {
    headers: Vec<String>,
    rows:    Vec<Vec<f64>>,
}

impl CsvData {
    fn col_values(&self, name: &str) -> Option<Vec<f64>> {
        let idx = self.headers.iter().position(|h| h == name)?;
        Some(self.rows.iter().map(|r| r.get(idx).copied().unwrap_or(0.0)).collect())
    }
}

fn parse_csv(path: &str) -> CsvData {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path, e));
    let mut lines = content.lines();

    // Strip surrounding quotes from Python's quoted header.
    let header_line = lines.next().expect("CSV has no header");
    let headers: Vec<String> = header_line
        .split(',')
        .map(|s| s.trim_matches('"').to_string())
        .collect();

    let rows: Vec<Vec<f64>> = lines
        .filter(|l| !l.is_empty())
        .map(|line| {
            line.split(',')
                .map(|v| v.trim_matches('"').parse::<f64>().unwrap_or(0.0))
                .collect()
        })
        .collect();

    CsvData { headers, rows }
}

fn median(mut vals: Vec<f64>) -> f64 {
    if vals.is_empty() { return 0.0; }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = vals.len() / 2;
    if vals.len() % 2 == 0 { (vals[mid - 1] + vals[mid]) / 2.0 } else { vals[mid] }
}

fn mean(vals: &[f64]) -> f64 {
    if vals.is_empty() { return 0.0; }
    vals.iter().sum::<f64>() / vals.len() as f64
}

// ---------------------------------------------------------------------------
// uv discovery - no hardcoded usernames; resolves via $HOME then PATH
// ---------------------------------------------------------------------------

fn find_uv() -> Option<String> {
    // $HOME/.local/bin/uv is the default install location on Linux x86-64 and
    // aarch64 regardless of the username.
    let home_candidate = std::env::var("HOME")
        .ok()
        .map(|h| format!("{}/.local/bin/uv", h));

    let system_candidates = ["/usr/local/bin/uv", "/usr/bin/uv"];

    for candidate in home_candidate.iter().map(String::as_str).chain(system_candidates) {
        if std::path::Path::new(candidate).exists() {
            return Some(candidate.to_string());
        }
    }

    // Last resort: search PATH.
    Command::new("which")
        .arg("uv")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Ensure `resource-tracker` is in uv's package cache before the timed window
/// opens - avoids download/install time eating into the 10 s cap.
/// Returns false if the install fails (test is then skipped).
fn ensure_resource_tracker_cached(uv: &str) -> bool {
    Command::new(uv)
        .args(["run", "--with", "resource-tracker", "python", "-c",
               "import resource_tracker"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Query a Python one-liner via uv and return trimmed stdout, or a fallback.
fn uv_query(uv: &str, code: &str, fallback: &str) -> String {
    Command::new(uv)
        .args(["run", "--with", "resource-tracker", "python", "-c", code])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

// ---------------------------------------------------------------------------
// Comparison spec
// ---------------------------------------------------------------------------

struct ColSpec {
    name:          &'static str,
    tolerance_pct: f64,
    use_median:    bool,
    description:   &'static str,
}

impl ColSpec {
    fn compare(&self, py: &[f64], rs: &[f64]) -> ColResult {
        let py_stat = if self.use_median { median(py.to_vec()) } else { mean(py) };
        let rs_stat = if self.use_median { median(rs.to_vec()) } else { mean(rs) };

        let pct_diff = if py_stat.abs() > 1.0 {
            (py_stat - rs_stat).abs() / py_stat * 100.0
        } else {
            (py_stat - rs_stat).abs()
        };

        ColResult { py_stat, rs_stat, pct_diff, passed: pct_diff <= self.tolerance_pct }
    }
}

struct ColResult {
    py_stat:  f64,
    rs_stat:  f64,
    pct_diff: f64,
    passed:   bool,
}

fn col_specs() -> Vec<ColSpec> {
    vec![
        // --- CPU ---
        ColSpec { name: "utime",               tolerance_pct: 5.0,  use_median: false, description: "user+nice CPU seconds / interval" },
        ColSpec { name: "stime",               tolerance_pct: 5.0,  use_median: false, description: "system CPU seconds / interval" },
        ColSpec { name: "cpu_usage",           tolerance_pct: 25.0, use_median: true,  description: "fractional cores in use (volatile)" },
        ColSpec { name: "processes",           tolerance_pct: 30.0, use_median: true,  description: "runnable process count (volatile)" },
        // --- Memory ---
        ColSpec { name: "memory_used",         tolerance_pct: 2.0,  use_median: true,  description: "used RAM (KiB)" },
        ColSpec { name: "memory_buffers",      tolerance_pct: 2.0,  use_median: true,  description: "kernel buffer RAM (KiB)" },
        ColSpec { name: "memory_cached",       tolerance_pct: 2.0,  use_median: true,  description: "page-cache RAM (KiB)" },
        ColSpec { name: "memory_active",       tolerance_pct: 5.0,  use_median: true,  description: "active-page RAM (KiB)" },
        ColSpec { name: "memory_inactive",     tolerance_pct: 5.0,  use_median: true,  description: "inactive-page RAM (KiB)" },
        ColSpec { name: "memory_free",         tolerance_pct: 10.0, use_median: true,  description: "available RAM (KiB)" },
        // --- Disk space ---
        // Python sums all non-virtual mounts including snap/loop devices; Rust
        // only sums mounts visible through /sys/block (real block devices).
        // The remaining gap is from snap squashfs mounts that Python includes.
        ColSpec { name: "disk_space_total_gb", tolerance_pct: 15.0, use_median: true,  description: "total disk GB (all mounts)" },
        ColSpec { name: "disk_space_used_gb",  tolerance_pct: 15.0, use_median: true,  description: "used disk GB (all mounts)" },
        ColSpec { name: "disk_space_free_gb",  tolerance_pct: 15.0, use_median: true,  description: "free disk GB (all mounts)" },
        // --- I/O (per-interval byte counts - both implementations use deltas) ---
        ColSpec { name: "disk_read_bytes",     tolerance_pct: 10.0, use_median: false, description: "disk read bytes / interval" },
        ColSpec { name: "disk_write_bytes",    tolerance_pct: 10.0, use_median: false, description: "disk write bytes / interval" },
        ColSpec { name: "net_recv_bytes",      tolerance_pct: 10.0, use_median: false, description: "net recv bytes / interval" },
        ColSpec { name: "net_sent_bytes",      tolerance_pct: 10.0, use_median: false, description: "net sent bytes / interval" },
    ]
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[test]
fn python_rust_csv_numeric_comparison() {
    let uv = match find_uv() {
        Some(u) => u,
        None => {
            eprintln!("SKIP: uv not found - install from https://docs.astral.sh/uv/");
            return;
        }
    };

    // Warm up the uv cache before the timed window to avoid install delays
    // eating into the 10 s cap (matters on first run and on slow ARM devices).
    if !ensure_resource_tracker_cached(&uv) {
        eprintln!("SKIP: could not install resource-tracker via uv");
        return;
    }

    // Query versions now that the cache is warm - no network needed.
    let python_version      = uv_query(&uv, "import sys; print(sys.version.split()[0])", "unknown");
    let rt_version          = uv_query(&uv, "import resource_tracker; print(resource_tracker.__version__)", "unknown");
    let rust_binary_version = Command::new(BINARY)
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // PID-namespaced temp files - no collisions between users or parallel runs.
    let py_script = tmp("py_runner.py");
    let py_output = tmp("python_metrics.csv");
    let rs_output = tmp("rust_metrics.csv");

    std::fs::write(&py_script, PYTHON_RUNNER).expect("failed to write Python runner script");

    // -----------------------------------------------------------------------
    // Start both collectors simultaneously
    // -----------------------------------------------------------------------
    let rs_file = std::fs::File::create(&rs_output).expect("failed to create rust output file");
    let mut rs_child = Command::new(BINARY)
        .args(["--interval", &INTERVAL_SECS.to_string(), "--format", "csv"])
        .stdout(rs_file)
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn Rust binary");

    let mut py_child = Command::new(&uv)
        .args([
            "run", "--with", "resource-tracker",
            "python", &py_script, &py_output,
            &DURATION_SECS.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn Python via uv");

    // Hard cap: sleep MAX_WAIT_SECS then kill both regardless of state.
    std::thread::sleep(Duration::from_secs(MAX_WAIT_SECS));
    rs_child.kill().ok();
    py_child.kill().ok();
    rs_child.wait().ok();
    py_child.wait().ok();

    // Clean up temp script.
    std::fs::remove_file(&py_script).ok();

    // -----------------------------------------------------------------------
    // Parse
    // -----------------------------------------------------------------------
    let py = parse_csv(&py_output);
    let rs = parse_csv(&rs_output);

    assert!(
        !py.rows.is_empty(),
        "Python produced no rows - resource-tracker may have failed to start in time"
    );
    assert!(!rs.rows.is_empty(), "Rust produced no rows");

    // -----------------------------------------------------------------------
    // Header
    // -----------------------------------------------------------------------
    println!("\n========== resource-tracker-rs vs resource-tracker comparison ==========");
    println!("  Python version      : {}", python_version);
    println!("  resource-tracker    : {}", rt_version);
    println!("  resource-tracker-rs       : {}", rust_binary_version);
    println!("  duration / interval : {}s / {}s", DURATION_SECS, INTERVAL_SECS);
    println!("  Python rows         : {}", py.rows.len());
    println!("  Rust rows           : {}", rs.rows.len());
    println!("===================================================================");

    // -----------------------------------------------------------------------
    // Comparison table
    // -----------------------------------------------------------------------
    let col_w = 22usize;
    let num_w = 14usize;
    println!(
        "\n{:<col_w$} {:>num_w$} {:>num_w$} {:>9} {:>9}  {}",
        "column", "py", "rs", "pct_diff", "tolerance", "result",
    );
    println!("{}", "-".repeat(90));

    let mut failures: Vec<String> = Vec::new();

    for spec in col_specs() {
        let py_vals = match py.col_values(spec.name) {
            Some(v) => v,
            None => {
                println!("{:<col_w$}  [SKIP - not in Python output]", spec.name);
                continue;
            }
        };
        let rs_vals = match rs.col_values(spec.name) {
            Some(v) => v,
            None => {
                println!("{:<col_w$}  [SKIP - not in Rust output]", spec.name);
                continue;
            }
        };

        let r        = spec.compare(&py_vals, &rs_vals);
        let agg_kind = if spec.use_median { "med" } else { "avg" };
        let result   = if r.passed { "PASS" } else { "FAIL" };

        println!(
            "{:<col_w$} {:>num_w$.3} {:>num_w$.3} {:>8.2}% {:>8.1}%  {} ({})",
            spec.name, r.py_stat, r.rs_stat, r.pct_diff, spec.tolerance_pct, result, agg_kind,
        );

        if !r.passed {
            failures.push(format!(
                "  {} [{}]: py={:.3}  rs={:.3}  diff={:.2}%  tol={:.1}% - {}",
                spec.name, agg_kind, r.py_stat, r.rs_stat,
                r.pct_diff, spec.tolerance_pct, spec.description,
            ));
        }
    }

    println!("{}", "-".repeat(90));

    // Clean up output files.
    std::fs::remove_file(&py_output).ok();
    std::fs::remove_file(&rs_output).ok();

    if !failures.is_empty() {
        panic!(
            "\n{} column(s) exceeded tolerance:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
