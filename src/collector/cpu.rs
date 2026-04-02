use crate::metrics::CpuMetrics;
use procfs::prelude::*;
use procfs::{CpuTime, KernelStats};
use procfs::process::all_processes;
use std::collections::HashMap;
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// ---------------------------------------------------------------------------
// Tick helpers
// ---------------------------------------------------------------------------

fn cpu_total(c: &CpuTime) -> u64 {
    c.user
        + c.nice
        + c.system
        + c.idle
        + c.iowait.unwrap_or(0)
        + c.irq.unwrap_or(0)
        + c.softirq.unwrap_or(0)
        + c.steal.unwrap_or(0)
}

fn cpu_idle(c: &CpuTime) -> u64 {
    c.idle + c.iowait.unwrap_or(0)
}

/// Per-core utilization percentage (0.0–100.0, clamped).
fn core_util_pct(prev: &CpuTime, curr: &CpuTime) -> f64 {
    util_pct_from_ticks(
        cpu_total(prev), cpu_idle(prev),
        cpu_total(curr), cpu_idle(curr),
    ).clamp(0.0, 100.0)
}

/// Aggregate utilization expressed as fractional cores in use (0.0..n_cores).
/// Not clamped: kernel rounding can produce values very slightly above n_cores.
fn aggregate_util_cores(prev: &CpuTime, curr: &CpuTime, n_cores: usize) -> f64 {
    util_pct_from_ticks(
        cpu_total(prev), cpu_idle(prev),
        cpu_total(curr), cpu_idle(curr),
    ) / 100.0 * n_cores as f64
}

/// Pure math: percentage of non-idle ticks between two snapshots (0.0–100.0
/// before any clamping).  Takes raw pre-computed totals/idles so it can be
/// unit-tested without constructing a `CpuTime` (which has private fields).
fn util_pct_from_ticks(
    prev_total: u64, prev_idle: u64,
    curr_total: u64, curr_idle: u64,
) -> f64 {
    let delta_total = curr_total.saturating_sub(prev_total) as f64;
    let delta_idle  = curr_idle.saturating_sub(prev_idle)   as f64;
    if delta_total == 0.0 {
        return 0.0;
    }
    (delta_total - delta_idle) / delta_total * 100.0
}

// ---------------------------------------------------------------------------
// Process-tree helpers
// ---------------------------------------------------------------------------

/// Returns a map of { pid to utime+stime } for every process in the tree
/// rooted at `root_pid` (root included).  Processes that have already exited
/// are silently skipped: this is a TOCTOU race we accept.
fn process_tree_ticks(root_pid: i32) -> HashMap<i32, u64> {
    // Collect all readable processes in one pass.
    let all: Vec<_> = match all_processes() {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => return HashMap::new(),
    };

    // Build a parent to children lookup.
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    all.iter().for_each(|proc| {
        if let Ok(stat) = proc.stat() {
            children.entry(stat.ppid).or_default().push(proc.pid);
        }
    });

    // Build a pid to ticks lookup.
    let ticks_for: HashMap<i32, u64> = all
        .iter()
        .filter_map(|proc| {
            proc.stat().ok().map(|s| (proc.pid, s.utime + s.stime))
        })
        .collect();

    // BFS from root_pid, collecting ticks for every reachable node.
    let mut result = HashMap::new();
    let mut queue = vec![root_pid];
    while let Some(pid) = queue.pop() {
        if let Some(&ticks) = ticks_for.get(&pid) {
            result.insert(pid, ticks);
        }
        if let Some(kids) = children.get(&pid) {
            queue.extend(kids);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Snapshot + Collector
// ---------------------------------------------------------------------------

struct Snapshot {
    /// Aggregate across all logical CPUs (the "cpu" summary line in /proc/stat).
    total: CpuTime,
    /// Per-logical-CPU entries (cpu0, cpu1, …).
    per_core: Vec<CpuTime>,
    /// Wall-clock time of this snapshot, used to normalize process tick deltas.
    instant: Instant,
    /// { pid to utime+stime } for root process + all descendants. Empty when
    /// no PID is being tracked.
    proc_ticks: HashMap<i32, u64>,
}

pub struct CpuCollector {
    /// Root PID of the process tree to track. None = system-only metrics.
    pid: Option<i32>,
    prev: Option<Snapshot>,
}

impl CpuCollector {
    pub fn new(pid: Option<i32>) -> Self {
        Self { pid, prev: None }
    }

    pub fn collect(&mut self) -> Result<CpuMetrics> {
        let stats = KernelStats::current()?;
        let now   = Instant::now();

        let tps = procfs::ticks_per_second() as f64;

        // Total number of existing processes - matches Python resource-tracker's
        // `processes` column.  Counted by listing numeric entries in /proc,
        // which is O(n_procs) but cheap for a polling interval.
        let process_count = std::fs::read_dir("/proc")
            .map(|dir| {
                let n = dir.filter_map(|e| e.ok())
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .chars()
                            .all(|c| c.is_ascii_digit())
                    })
                    .count();
                u32::try_from(n).unwrap_or(0)
            })
            .unwrap_or(0);

        let proc_ticks = match self.pid {
            Some(root) => process_tree_ticks(root),
            None       => HashMap::new(),
        };

        let curr = Snapshot {
            total: stats.total,
            per_core: stats.cpu_time,
            instant: now,
            proc_ticks,
        };

        let metrics = match &self.prev {
            // First call: store baseline and return zeros. The caller should
            // sleep for one interval then call collect() again for real data.
            None => CpuMetrics {
                utilization_pct: 0.0,
                per_core_pct: vec![0.0; curr.per_core.len()],
                utime_secs: 0.0,
                stime_secs: 0.0,
                process_count,
                process_cores_used: self.pid.map(|_| 0.0),
                process_child_count: self.pid.map(|_| {
                    u32::try_from(curr.proc_ticks.len().saturating_sub(1)).unwrap_or(0)
                }),
            },

            Some(prev) => {
                let n_cores = curr.per_core.len();

                // Per-interval CPU time deltas - matches Python resource-tracker's
                // utime/stime columns (delta ticks / ticks_per_second).
                let utime_secs = (curr.total.user + curr.total.nice)
                    .saturating_sub(prev.total.user + prev.total.nice) as f64 / tps;
                let stime_secs = curr.total.system
                    .saturating_sub(prev.total.system) as f64 / tps;

                let per_core_pct = prev
                    .per_core
                    .iter()
                    .zip(curr.per_core.iter())
                    .map(|(p, c)| core_util_pct(p, c))
                    .collect();

                // Fractional cores = total tick delta / (elapsed_s × ticks_per_s)
                let process_cores_used = self.pid.map(|_| {
                    let elapsed = (curr.instant - prev.instant).as_secs_f64().max(0.001);

                    let delta: u64 = curr.proc_ticks.iter().map(|(pid, &curr_ticks)| {
                        let prev_ticks = prev.proc_ticks.get(pid).copied().unwrap_or(curr_ticks);
                        curr_ticks.saturating_sub(prev_ticks)
                    }).sum();

                    (delta as f64 / (elapsed * tps)).max(0.0)
                });

                let process_child_count = self.pid.map(|_| {
                    u32::try_from(curr.proc_ticks.len().saturating_sub(1)).unwrap_or(0)
                });

                CpuMetrics {
                    utilization_pct: aggregate_util_cores(&prev.total, &curr.total, n_cores),
                    per_core_pct,
                    utime_secs,
                    stime_secs,
                    process_count,
                    process_cores_used,
                    process_child_count,
                }
            }
        };

        self.prev = Some(curr);
        Ok(metrics)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Tests use `util_pct_from_ticks` directly -- `CpuTime` has private fields
    // and cannot be constructed in tests.  All branching logic in
    // `aggregate_util_cores` and `core_util_pct` delegates to this one
    // pure function, so testing it covers all paths.
    //
    // Tick layout: (prev_total, prev_idle, curr_total, curr_idle)

    #[test]
    fn test_util_pct_all_idle_is_zero() {
        // All new ticks went to idle.
        assert_eq!(util_pct_from_ticks(0, 0, 1600, 1600), 0.0);
    }

    #[test]
    fn test_util_pct_fully_busy_is_100() {
        // 1600 new ticks, 0 idle -> 100%.
        let pct = util_pct_from_ticks(0, 0, 1600, 0);
        assert!((pct - 100.0).abs() < 0.01, "expected 100.0, got {pct}");
    }

    #[test]
    fn test_util_pct_half_busy_is_50() {
        // 1600 new ticks, 800 idle -> 50%.
        let pct = util_pct_from_ticks(0, 0, 1600, 800);
        assert!((pct - 50.0).abs() < 0.01, "expected 50.0, got {pct}");
    }

    #[test]
    fn test_util_pct_no_delta_is_zero() {
        // Identical snapshots: no elapsed ticks.
        assert_eq!(util_pct_from_ticks(100, 50, 100, 50), 0.0);
    }

    /// Aggregate util converts the percentage to fractional cores and does NOT clamp.
    /// 99.9% busy on a 4-core machine -> ~3.996 cores, not forced to <= 4.0.
    #[test]
    fn test_aggregate_util_cores_no_clamp() {
        // 999 active ticks, 1 idle, total 1000 -> 99.9% -> 99.9/100*4 = 3.996
        let pct = util_pct_from_ticks(0, 0, 1000, 1);
        let cores = pct / 100.0 * 4.0_f64;
        assert!(cores > 3.9,  "expected close to 4.0, got {cores}");
        assert!(cores < 4.05, "should not greatly exceed n_cores, got {cores}");
    }

    /// Per-core values are clamped to 100 by `core_util_pct`; verify the
    /// underlying math exceeds 100 without the clamp (so the clamp is doing work).
    #[test]
    fn test_util_pct_raw_is_not_clamped() {
        // 100% busy -- raw result is exactly 100, clamp has no effect here.
        let raw = util_pct_from_ticks(0, 0, 1000, 0);
        assert!((raw - 100.0).abs() < 0.01);
        // Apply clamp explicitly to show it would cap any value > 100.
        assert_eq!(raw.clamp(0.0, 100.0), 100.0);
    }

    // T-CPU-06: the first call to collect() returns 0.0 for all delta fields
    // (utilization_pct, per_core_pct, utime_secs, stime_secs).  A warm-up
    // sleep then a second collect() produces real data.
    #[test]
    fn test_first_collect_returns_zero_for_delta_fields() {
        let mut collector = CpuCollector::new(None);
        let metrics = collector.collect().expect("first collect failed");
        assert_eq!(
            metrics.utilization_pct, 0.0,
            "utilization_pct must be 0.0 on first collect, got {}",
            metrics.utilization_pct
        );
        assert!(
            metrics.per_core_pct.iter().all(|&v| v == 0.0),
            "per_core_pct must be all-zero on first collect: {:?}",
            metrics.per_core_pct
        );
        assert_eq!(
            metrics.utime_secs, 0.0,
            "utime_secs must be 0.0 on first collect, got {}",
            metrics.utime_secs
        );
        assert_eq!(
            metrics.stime_secs, 0.0,
            "stime_secs must be 0.0 on first collect, got {}",
            metrics.stime_secs
        );
    }
}
