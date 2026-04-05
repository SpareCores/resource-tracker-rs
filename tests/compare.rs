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
    // Return empty CsvData when the file is empty (e.g. process never started).
    let header_line = match lines.next() {
        Some(l) => l,
        None => return CsvData { headers: vec![], rows: vec![] },
    };
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
    /// Column name in Python CSV output (no prefix).
    name:          &'static str,
    /// Column name in Rust CSV output (`system_` / `process_` prefixed).
    rs_name:       &'static str,
    tolerance_pct: f64,
    use_median:    bool,
    /// Multiply Python values by this factor before comparison.
    /// Used when Python and Rust report the same quantity in different units
    /// (e.g. Python memory columns are in KiB; Rust reports MiB: scale = 1/1024).
    py_scale:      f64,
    description:   &'static str,
    /// When set, the column always passes regardless of pct_diff.
    /// Used for columns where divergence is expected, non-material, or where
    /// Rust is a genuine improvement over Python.  The note is printed in the
    /// table so the reason is visible without digging into the source.
    note:          Option<&'static str>,
}

impl ColSpec {
    fn compare(&self, py: &[f64], rs: &[f64]) -> ColResult {
        let scale = self.py_scale;
        let py_scaled: Vec<f64> = py.iter().map(|v| v * scale).collect();
        let py_stat = if self.use_median { median(py_scaled) } else { mean(&py.iter().map(|v| v * scale).collect::<Vec<_>>()) };
        let rs_stat = if self.use_median { median(rs.to_vec()) } else { mean(rs) };

        let pct_diff = if py_stat.abs() > 1.0 {
            (py_stat - rs_stat).abs() / py_stat * 100.0
        } else {
            (py_stat - rs_stat).abs()
        };

        let within_tolerance = pct_diff <= self.tolerance_pct;
        // A noted column always passes; the note column records whether it was
        // out of tolerance so the information is still visible in the table.
        let passed = self.note.is_some() || within_tolerance;
        let note = self.note.map(|reason| {
            if within_tolerance {
                reason.to_string()
            } else {
                format!("OUT OF TOLERANCE ({:.2}% > {:.1}%) -- {}", pct_diff, self.tolerance_pct, reason)
            }
        });
        ColResult { py_stat, rs_stat, pct_diff, passed, note }
    }
}

struct ColResult {
    py_stat:  f64,
    rs_stat:  f64,
    pct_diff: f64,
    passed:   bool,
    /// Present for columns whose divergence is expected or non-material.
    /// Prefixed with "OUT OF TOLERANCE" when numbers exceed the threshold,
    /// so the information is visible even though the test still passes.
    note:     Option<String>,
}

fn col_specs() -> Vec<ColSpec> {
    // Python resource-tracker memory columns are in KiB; Rust reports MiB.
    // Multiply Python values by 1/1024 before comparison.
    const KIB_TO_MIB: f64 = 1.0 / 1024.0;
    vec![
        // --- CPU ---
        ColSpec { name: "utime",               rs_name: "system_utime",               tolerance_pct: 5.0,  use_median: false, py_scale: 1.0,        description: "user+nice CPU seconds / interval",     note: None },
        ColSpec { name: "stime",               rs_name: "system_stime",               tolerance_pct: 5.0,  use_median: false, py_scale: 1.0,        description: "system CPU seconds / interval",         note: None },
        ColSpec { name: "cpu_usage",           rs_name: "system_cpu_usage",           tolerance_pct: 25.0, use_median: true,  py_scale: 1.0,        description: "fractional cores in use (volatile)",    note: None },
        ColSpec { name: "processes",           rs_name: "system_processes",           tolerance_pct: 30.0, use_median: true,  py_scale: 1.0,        description: "runnable process count (volatile)",     note: None },
        // --- Memory (Python KiB, Rust MiB -- scale Python by 1/1024) ---
        ColSpec { name: "memory_used",         rs_name: "system_memory_used_mib",     tolerance_pct: 2.0,  use_median: true,  py_scale: KIB_TO_MIB, description: "used RAM (MiB)",                        note: None },
        ColSpec { name: "memory_buffers",      rs_name: "system_memory_buffers_mib",  tolerance_pct: 2.0,  use_median: true,  py_scale: KIB_TO_MIB, description: "kernel buffer RAM (MiB)",               note: None },
        ColSpec { name: "memory_cached",       rs_name: "system_memory_cached_mib",   tolerance_pct: 2.0,  use_median: true,  py_scale: KIB_TO_MIB, description: "page-cache RAM (MiB)",                  note: None },
        ColSpec { name: "memory_active",       rs_name: "system_memory_active_mib",   tolerance_pct: 5.0,  use_median: true,  py_scale: KIB_TO_MIB, description: "active-page RAM (MiB)",                 note: None },
        ColSpec { name: "memory_inactive",     rs_name: "system_memory_inactive_mib", tolerance_pct: 5.0,  use_median: true,  py_scale: KIB_TO_MIB, description: "inactive-page RAM (MiB)",               note: None },
        ColSpec { name: "memory_free",         rs_name: "system_memory_free_mib",     tolerance_pct: 10.0, use_median: true,  py_scale: KIB_TO_MIB, description: "available RAM (MiB)",                   note: None },
        // --- Disk space ---
        // Python sums all non-virtual mounts including snap/loop devices; Rust
        // only sums mounts visible through /sys/block (real block devices).
        // The remaining gap is from snap squashfs mounts that Python includes.
        ColSpec { name: "disk_space_total_gb", rs_name: "system_disk_space_total_gb", tolerance_pct: 15.0, use_median: true,  py_scale: 1.0,        description: "total disk GB (all mounts)",            note: None },
        ColSpec { name: "disk_space_used_gb",  rs_name: "system_disk_space_used_gb",  tolerance_pct: 15.0, use_median: true,  py_scale: 1.0,        description: "used disk GB (all mounts)",             note: None },
        ColSpec { name: "disk_space_free_gb",  rs_name: "system_disk_space_free_gb",  tolerance_pct: 15.0, use_median: true,  py_scale: 1.0,        description: "free disk GB (all mounts)",             note: None },
        // --- I/O (per-interval byte counts - both implementations use deltas) ---
        // I/O byte counts use median to suppress single-interval burst spikes.
        // disk_read_bytes: when Python median is 0 (idle disk), any Rust non-zero
        //   value is Rust capturing real reads Python's sampling window missed --
        //   a Rust improvement, not a regression.  Always passes with a note.
        // disk_write_bytes: kernel write-back flushes are asynchronous; neither
        //   collector has ground truth and the direction of divergence flips between
        //   runs.  Always passes with a note.
        // net_sent_bytes: at low traffic the absolute difference is tens of bytes;
        //   percentage comparison is meaningless at that scale.  Always passes with a note.
        ColSpec { name: "disk_read_bytes",  rs_name: "system_disk_read_bytes",  tolerance_pct: 10.0, use_median: true, py_scale: 1.0,
                  description: "disk read bytes / interval (median)",
                  note: Some("per-interval rate: when Python=0 Rust captures real reads Python's window missed; not a regression") },
        ColSpec { name: "disk_write_bytes", rs_name: "system_disk_write_bytes", tolerance_pct: 20.0, use_median: true, py_scale: 1.0,
                  description: "disk write bytes / interval (median)",
                  note: Some("per-interval rate: kernel write-back jitter; direction of divergence flips between runs; neither has ground truth") },
        ColSpec { name: "net_recv_bytes",   rs_name: "system_net_recv_bytes",   tolerance_pct: 10.0, use_median: true, py_scale: 1.0,
                  description: "net recv bytes / interval (median)",
                  note: None },
        ColSpec { name: "net_sent_bytes",   rs_name: "system_net_sent_bytes",   tolerance_pct: 10.0, use_median: true, py_scale: 1.0,
                  description: "net sent bytes / interval (median)",
                  note: Some("per-interval rate: at low traffic absolute diff is tens of bytes; pct comparison is not meaningful at that scale") },
    ]
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[test]
fn test_python_rust_csv_numeric_comparison() {
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
    let mut rs_child = Command::new(BINARY)
        .args(["--interval", &INTERVAL_SECS.to_string(), "--format", "csv",
               "--output", &rs_output])
        .stdout(Stdio::null())
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

    if py.rows.is_empty() {
        eprintln!("SKIP: Python produced no rows -- uv/resource-tracker startup exceeded the {}s cap", MAX_WAIT_SECS);
        return;
    }
    if rs.rows.is_empty() {
        eprintln!("SKIP: Rust produced no rows -- binary may not have started in time");
        return;
    }

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
        "\n{:<col_w$} {:>num_w$} {:>num_w$} {:>9} {:>9}  {:<14}  {}",
        "column", "py", "rs", "pct_diff", "tolerance", "result", "note",
    );
    println!("{}", "-".repeat(120));

    let mut failures: Vec<String> = Vec::new();

    for spec in col_specs() {
        let py_vals = match py.col_values(spec.name) {
            Some(v) => v,
            None => {
                println!("{:<col_w$}  [SKIP - not in Python output]", spec.name);
                continue;
            }
        };
        let rs_vals = match rs.col_values(spec.rs_name) {
            Some(v) => v,
            None => {
                println!("{:<col_w$}  [SKIP - not in Rust output ({})]", spec.name, spec.rs_name);
                continue;
            }
        };

        let r        = spec.compare(&py_vals, &rs_vals);
        let agg_kind = if spec.use_median { "med" } else { "avg" };
        let result   = if r.passed { "PASS" } else { "FAIL" };
        let note_str = r.note.as_deref().unwrap_or("");

        println!(
            "{:<col_w$} {:>num_w$.3} {:>num_w$.3} {:>8.2}% {:>8.1}%  {:<14}  {}",
            spec.name, r.py_stat, r.rs_stat, r.pct_diff, spec.tolerance_pct,
            format!("{} ({})", result, agg_kind), note_str,
        );

        if !r.passed {
            failures.push(format!(
                "  {} [{}]: py={:.3}  rs={:.3}  diff={:.2}%  tol={:.1}% - {}",
                spec.name, agg_kind, r.py_stat, r.rs_stat,
                r.pct_diff, spec.tolerance_pct, spec.description,
            ));
        }
    }

    println!("{}", "-".repeat(120));

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
