use crate::metrics::CpuMetrics;
use procfs::prelude::*;
use procfs::process::all_processes;
use procfs::{CpuTime, KernelStats};
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
        cpu_total(prev),
        cpu_idle(prev),
        cpu_total(curr),
        cpu_idle(curr),
    )
    .clamp(0.0, 100.0)
}

/// Aggregate utilization expressed as fractional cores in use (0.0..n_cores).
/// Not clamped: kernel rounding can produce values very slightly above n_cores.
fn aggregate_util_cores(prev: &CpuTime, curr: &CpuTime, n_cores: usize) -> f64 {
    util_pct_from_ticks(
        cpu_total(prev),
        cpu_idle(prev),
        cpu_total(curr),
        cpu_idle(curr),
    ) / 100.0
        * n_cores as f64
}

/// Pure math: percentage of non-idle ticks between two snapshots (0.0–100.0
/// before any clamping).  Takes raw pre-computed totals/idles so it can be
/// unit-tested without constructing a `CpuTime` (which has private fields).
fn util_pct_from_ticks(prev_total: u64, prev_idle: u64, curr_total: u64, curr_idle: u64) -> f64 {
    let delta_total = curr_total.saturating_sub(prev_total) as f64;
    let delta_idle = curr_idle.saturating_sub(prev_idle) as f64;
    if delta_total == 0.0 {
        return 0.0;
    }
    (delta_total - delta_idle) / delta_total * 100.0
}

// ---------------------------------------------------------------------------
// Process-tree helpers
// ---------------------------------------------------------------------------

/// Returns a map of { pid to (utime, stime) } for every process in the tree
/// rooted at `root_pid` (root included).  Processes that have already exited
/// are silently skipped: this is a TOCTOU race we accept.
fn process_tree_ticks(root_pid: i32) -> HashMap<i32, (u64, u64)> {
    // Collect all readable processes in one pass.
    let all: Vec<_> = match all_processes() {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => return HashMap::new(),
    };

    // Single .stat() read per process: build both the parent->children map and
    // the pid->(utime+cutime, stime+cstime) map in one pass to halve /proc I/O.
    //
    // cutime/cstime (CPU time of waited-for children) is included so that
    // short-lived child processes that both start AND exit within a single
    // measurement interval are still captured: once a child is reaped its
    // ticks roll up into the parent's cutime/cstime.
    //
    // Double-counting guard: if a process was alive at the previous snapshot
    // and exits before the current one, its pre-snapshot ticks are already in
    // prev_proc_ticks AND will re-appear via the parent's cutime delta.
    // CpuCollector::collect() subtracts the prev ticks of all such exited
    // processes to cancel that overcounting.
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    let ticks_for: HashMap<i32, (u64, u64)> = all
        .iter()
        .filter_map(|proc| {
            proc.stat().ok().map(|s| {
                children.entry(s.ppid).or_default().push(proc.pid);
                let user = s.utime + u64::try_from(s.cutime).unwrap_or(0);
                let system = s.stime + u64::try_from(s.cstime).unwrap_or(0);
                (proc.pid, (user, system))
            })
        })
        .collect();

    // BFS from root_pid, collecting (utime, stime) for every reachable node.
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

/// Sum of VmRSS (kB) across all given PIDs, converted to MiB.
/// Processes that have exited or whose /proc/pid/status is unreadable are skipped.
fn process_tree_rss_mib(pids: &[i32]) -> u64 {
    let total_kib: u64 = pids
        .iter()
        .filter_map(|&pid| {
            procfs::process::Process::new(pid)
                .ok()
                .and_then(|p| p.status().ok())
                .and_then(|s| s.vmrss)
        })
        .sum();
    total_kib / 1024
}

/// Per-process cumulative disk I/O bytes from /proc/pid/io.
/// Returns { pid -> (read_bytes, write_bytes) }.
/// PIDs whose /proc/pid/io is unreadable (e.g. different UID without ptrace)
/// are silently omitted -- the delta for those PIDs will be 0.
fn process_tree_io(pids: &[i32]) -> HashMap<i32, (u64, u64)> {
    pids.iter()
        .filter_map(|&pid| {
            let io = procfs::process::Process::new(pid).ok()?.io().ok()?;
            Some((pid, (io.read_bytes, io.write_bytes)))
        })
        .collect()
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
    /// { pid -> (utime, stime) } for root process + all descendants.
    /// Empty when no PID is being tracked.
    proc_ticks: HashMap<i32, (u64, u64)>,
    /// { pid -> (read_bytes, write_bytes) } from /proc/pid/io.
    /// Empty when no PID is tracked or /proc/pid/io is unreadable.
    proc_io: HashMap<i32, (u64, u64)>,
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

        let tps = procfs::ticks_per_second() as f64;

        // Total number of existing processes - matches Python resource-tracker's
        // `processes` column.  Counted by listing numeric entries in /proc,
        // which is O(n_procs) but cheap for a polling interval.
        let process_count = std::fs::read_dir("/proc")
            .map(|dir| {
                let n = dir
                    .filter_map(|e| e.ok())
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
            None => HashMap::new(),
        };

        // Read process I/O and RSS only when tracking a PID.
        let proc_io = if self.pid.is_some() {
            let pids: Vec<i32> = proc_ticks.keys().copied().collect();
            process_tree_io(&pids)
        } else {
            HashMap::new()
        };

        // RSS is instantaneous (not a delta), so compute it before storing prev.
        let process_rss_mib = if self.pid.is_some() {
            let pids: Vec<i32> = proc_ticks.keys().copied().collect();
            Some(process_tree_rss_mib(&pids))
        } else {
            None
        };

        // Capture wall-clock time AFTER all /proc reads.  process_tree_ticks()
        // scans every entry in /proc and can take variable time on a loaded
        // server.  Capturing Instant::now() before those reads (the previous
        // arrangement) caused `elapsed` to undercount the actual measurement
        // window, making process_cores_used and process_utime_secs appear larger
        // than system_cpu_usage (issue #20).
        let now = Instant::now();

        let curr = Snapshot {
            total: stats.total,
            per_core: stats.cpu_time,
            instant: now,
            proc_ticks,
            proc_io,
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
                process_child_count: self
                    .pid
                    .map(|_| u32::try_from(curr.proc_ticks.len().saturating_sub(1)).unwrap_or(0)),
                process_utime_secs: self.pid.map(|_| 0.0),
                process_stime_secs: self.pid.map(|_| 0.0),
                process_rss_mib,
                process_disk_read_bytes: self.pid.map(|_| 0),
                process_disk_write_bytes: self.pid.map(|_| 0),
                process_gpu_usage: None, // filled by main.rs after GPU query
                process_gpu_vram_mib: None, // filled by main.rs after GPU query
                process_gpu_utilized: None,
                process_tree_pids: curr.proc_ticks.keys().copied().collect(),
            },

            Some(prev) => {
                let n_cores = curr.per_core.len();

                // Per-interval CPU time deltas - matches Python resource-tracker's
                // utime/stime columns (delta ticks / ticks_per_second).
                let utime_secs = (curr.total.user + curr.total.nice)
                    .saturating_sub(prev.total.user + prev.total.nice)
                    as f64
                    / tps;
                let stime_secs = curr.total.system.saturating_sub(prev.total.system) as f64 / tps;

                let per_core_pct = prev
                    .per_core
                    .iter()
                    .zip(curr.per_core.iter())
                    .map(|(p, c)| core_util_pct(p, c))
                    .collect();

                // Cutime double-counting correction (issue #20, bug 1).
                //
                // proc_ticks stores (utime + cutime, stime + cstime) so that
                // short-lived children that start AND exit within one interval are
                // captured via the parent's cutime delta.
                //
                // Side-effect: if a child was alive at the prev snapshot, its
                // pre-snapshot ticks appear both in prev_proc_ticks[child] AND
                // in the parent's cutime delta once the child is reaped.  That
                // double-counts the child's pre-snapshot ticks.
                //
                // Fix: sum the prev ticks of every PID in prev that is absent
                // from curr (it exited), then subtract that sum from the raw
                // delta.  This cancels exactly the overcounting without
                // affecting short-lived processes (which were never in prev, so
                // their prev ticks are zero).
                let (exited_utime, exited_stime): (u64, u64) = if self.pid.is_some() {
                    prev.proc_ticks
                        .iter()
                        .filter(|(pid, _)| !curr.proc_ticks.contains_key(pid))
                        .fold((0u64, 0u64), |(au, as_), (_, &(pu, ps))| {
                            (au + pu, as_ + ps)
                        })
                } else {
                    (0, 0)
                };

                // Fractional cores = total (utime+stime) tick delta / (elapsed × tps)
                let process_cores_used = self.pid.map(|_| {
                    let elapsed = (curr.instant - prev.instant).as_secs_f64().max(0.001);
                    let raw: u64 = curr
                        .proc_ticks
                        .iter()
                        .map(|(pid, &(cu, cs))| {
                            let (pu, ps) = prev.proc_ticks.get(pid).copied().unwrap_or((cu, cs));
                            cu.saturating_sub(pu) + cs.saturating_sub(ps)
                        })
                        .sum();
                    let delta = raw.saturating_sub(exited_utime + exited_stime);
                    (delta as f64 / (elapsed * tps)).max(0.0)
                });

                let process_child_count = self
                    .pid
                    .map(|_| u32::try_from(curr.proc_ticks.len().saturating_sub(1)).unwrap_or(0));

                // Per-tree utime and stime deltas (seconds this interval).
                let process_utime_secs = self.pid.map(|_| {
                    let raw: u64 = curr
                        .proc_ticks
                        .iter()
                        .map(|(pid, &(cu, _))| {
                            let pu = prev.proc_ticks.get(pid).map(|&(u, _)| u).unwrap_or(cu);
                            cu.saturating_sub(pu)
                        })
                        .sum();
                    raw.saturating_sub(exited_utime) as f64 / tps
                });

                let process_stime_secs = self.pid.map(|_| {
                    let raw: u64 = curr
                        .proc_ticks
                        .iter()
                        .map(|(pid, &(_, cs))| {
                            let ps = prev.proc_ticks.get(pid).map(|&(_, s)| s).unwrap_or(cs);
                            cs.saturating_sub(ps)
                        })
                        .sum();
                    raw.saturating_sub(exited_stime) as f64 / tps
                });

                // Per-interval disk I/O deltas across the process tree.
                let process_disk_read_bytes = self.pid.map(|_| {
                    curr.proc_io
                        .iter()
                        .map(|(pid, &(cr, _))| {
                            let pr = prev.proc_io.get(pid).map(|&(r, _)| r).unwrap_or(cr);
                            cr.saturating_sub(pr)
                        })
                        .sum::<u64>()
                });

                let process_disk_write_bytes = self.pid.map(|_| {
                    curr.proc_io
                        .iter()
                        .map(|(pid, &(_, cw))| {
                            let pw = prev.proc_io.get(pid).map(|&(_, w)| w).unwrap_or(cw);
                            cw.saturating_sub(pw)
                        })
                        .sum::<u64>()
                });

                CpuMetrics {
                    utilization_pct: aggregate_util_cores(&prev.total, &curr.total, n_cores),
                    per_core_pct,
                    utime_secs,
                    stime_secs,
                    process_count,
                    process_cores_used,
                    process_child_count,
                    process_utime_secs,
                    process_stime_secs,
                    process_rss_mib,
                    process_disk_read_bytes,
                    process_disk_write_bytes,
                    process_gpu_usage: None, // filled by main.rs after GPU query
                    process_gpu_vram_mib: None, // filled by main.rs after GPU query
                    process_gpu_utilized: None,
                    process_tree_pids: curr.proc_ticks.keys().copied().collect(),
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
        assert!(cores > 3.9, "expected close to 4.0, got {cores}");
        assert!(
            cores < 4.05,
            "should not greatly exceed n_cores, got {cores}"
        );
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

    // T-CPU-07: first collect() with PID tracking returns Some for process fields.
    #[test]
    fn test_first_collect_with_pid_returns_some_process_fields() {
        let pid = i32::try_from(std::process::id()).expect("PID too large");
        let mut collector = CpuCollector::new(Some(pid));
        let m = collector.collect().expect("collect() failed");
        assert!(
            m.process_cores_used.is_some(),
            "process_cores_used must be Some when PID is tracked"
        );
        assert!(
            m.process_child_count.is_some(),
            "process_child_count must be Some when PID is tracked"
        );
        assert!(
            m.process_rss_mib.is_some(),
            "process_rss_mib must be Some when PID is tracked"
        );
        assert!(
            m.process_utime_secs.is_some(),
            "process_utime_secs must be Some when PID is tracked"
        );
        assert!(
            m.process_stime_secs.is_some(),
            "process_stime_secs must be Some when PID is tracked"
        );
        assert!(
            m.process_disk_read_bytes.is_some(),
            "process_disk_read_bytes must be Some when PID is tracked"
        );
        assert!(
            m.process_disk_write_bytes.is_some(),
            "process_disk_write_bytes must be Some when PID is tracked"
        );
    }

    // T-CPU-08: process_tree_rss_mib returns a positive value for the running test process.
    #[test]
    fn test_process_tree_rss_mib_nonzero_for_self() {
        let pid = i32::try_from(std::process::id()).expect("PID too large");
        let rss = process_tree_rss_mib(&[pid]);
        assert!(
            rss > 0,
            "RSS for the current process should be > 0, got {rss}"
        );
    }

    // T-CPU-09: process_tree_ticks contains the root PID.
    // PID 1 (init/systemd) is used because it is always present and readable
    // on any Linux host. Using std::process::id() is unreliable under
    // llvm-cov instrumentation: the instrumented binary's own /proc entry
    // can be transiently unreadable when many tests run in parallel.
    #[test]
    fn test_process_tree_ticks_contains_root_pid() {
        let ticks = process_tree_ticks(1);
        assert!(
            ticks.contains_key(&1),
            "process_tree_ticks(1) must contain PID 1 (init/systemd is always present)"
        );
    }

    // T-CPU-10: second collect() with PID tracking produces non-negative cores.
    #[test]
    fn test_second_collect_with_pid_nonneg_cores() {
        let pid = i32::try_from(std::process::id()).expect("PID too large");
        let mut collector = CpuCollector::new(Some(pid));
        let _ = collector.collect().expect("first collect() failed");
        let m = collector.collect().expect("second collect() failed");
        let cores = m
            .process_cores_used
            .expect("process_cores_used must be Some");
        assert!(
            cores >= 0.0,
            "process_cores_used must be >= 0.0, got {cores}"
        );
    }

    // T-CPU-11: second collect() with no PID still returns None for all process fields.
    #[test]
    fn test_second_collect_no_pid_all_process_fields_none() {
        let mut collector = CpuCollector::new(None);
        let _ = collector.collect().expect("first collect() failed");
        let m = collector.collect().expect("second collect() failed");
        assert!(
            m.process_cores_used.is_none(),
            "process_cores_used must be None when not tracking"
        );
        assert!(
            m.process_child_count.is_none(),
            "process_child_count must be None when not tracking"
        );
        assert!(
            m.process_rss_mib.is_none(),
            "process_rss_mib must be None when not tracking"
        );
        assert!(
            m.process_utime_secs.is_none(),
            "process_utime_secs must be None when not tracking"
        );
        assert!(
            m.process_stime_secs.is_none(),
            "process_stime_secs must be None when not tracking"
        );
        assert!(
            m.process_disk_read_bytes.is_none(),
            "process_disk_read_bytes must be None when not tracking"
        );
        assert!(
            m.process_disk_write_bytes.is_none(),
            "process_disk_write_bytes must be None when not tracking"
        );
    }

    // T-CPU-12: process_count > 0 (at least one process is always visible).
    #[test]
    fn test_process_count_positive() {
        let mut collector = CpuCollector::new(None);
        let m = collector.collect().expect("collect() failed");
        assert!(
            m.process_count > 0,
            "process_count must be > 0, got {}",
            m.process_count
        );
    }

    // -----------------------------------------------------------------------
    // Issue #20 regression tests: process CPU must never exceed system CPU
    // -----------------------------------------------------------------------

    // T-CPU-13: cutime correction formula -- direct arithmetic verification.
    //
    // A child with 500 pre-snapshot user ticks exits between samples and is
    // reaped by its parent.  The parent's cutime delta therefore covers the
    // child's full 2500-tick lifetime.  The raw delta overcounts by 500 (the
    // pre-snapshot portion already counted via the child's prev entry).
    // The correction must subtract exactly those 500 ticks.
    #[test]
    fn test_cutime_correction_cancels_exited_child_ticks() {
        let prev: HashMap<i32, (u64, u64)> = [
            (200, (50, 0)),  // parent: 50 own ticks at warm-up
            (100, (500, 0)), // child:  500 ticks at warm-up
        ]
        .iter()
        .cloned()
        .collect();

        // Between samples: child accumulates 2000 more ticks then exits.
        // Parent's cutime = child's full lifetime = 500 + 2000 = 2500.
        // Parent runs 250 own ticks.
        let curr: HashMap<i32, (u64, u64)> = [(200, (50 + 250 + 2500, 0))]
            .iter()
            .cloned()
            .collect();

        let raw: u64 = curr
            .iter()
            .map(|(pid, &(cu, cs))| {
                let (pu, ps) = prev.get(pid).copied().unwrap_or((cu, cs));
                cu.saturating_sub(pu) + cs.saturating_sub(ps)
            })
            .sum();
        assert_eq!(
            raw, 2750,
            "raw delta must include the double-counted pre-snapshot child ticks"
        );

        let exited: u64 = prev
            .iter()
            .filter(|(pid, _)| !curr.contains_key(pid))
            .map(|(_, &(pu, ps))| pu + ps)
            .sum();
        assert_eq!(
            exited, 500,
            "exited ticks must equal the child's pre-snapshot tick count"
        );

        let corrected = raw.saturating_sub(exited);
        // Correct answer: parent own delta (250) + child post-snapshot delta (2000) = 2250.
        assert_eq!(
            corrected, 2250,
            "corrected delta must exclude the child's pre-snapshot ticks"
        );
    }

    // T-CPU-14: cutime correction handles cascaded exits.
    //
    // Both a child and grandchild exit between samples.  Root's cutime ends up
    // containing the full lifetimes of both.  Subtracting all exited PIDs'
    // pre-snapshot ticks must leave only the ticks actually earned in the
    // interval regardless of exit depth.
    #[test]
    fn test_cutime_correction_handles_cascaded_exits() {
        let prev: HashMap<i32, (u64, u64)> = [
            (7, (0, 0)),   // root:        no prior ticks
            (8, (100, 0)), // child:       100 pre-snapshot ticks
            (9, (200, 0)), // grandchild:  200 pre-snapshot ticks
        ]
        .iter()
        .cloned()
        .collect();

        // Grandchild earns 50 ticks and exits; reaped by child.
        //   child cutime → 200 + 50 = 250.
        // Child earns 50 own ticks then exits; reaped by root.
        //   child lifetime = 100 + 50 + 250 = 400.
        //   root cutime → 400.
        // Root earns 30 own ticks.
        let curr: HashMap<i32, (u64, u64)> = [(7, (30 + 400, 0))].iter().cloned().collect();

        let raw: u64 = curr
            .iter()
            .map(|(pid, &(cu, cs))| {
                let (pu, ps) = prev.get(pid).copied().unwrap_or((cu, cs));
                cu.saturating_sub(pu) + cs.saturating_sub(ps)
            })
            .sum();
        // raw = 430; overcounts by child_prev (100) + grandchild_prev (200) = 300.
        assert_eq!(raw, 430);

        let exited: u64 = prev
            .iter()
            .filter(|(pid, _)| !curr.contains_key(pid))
            .map(|(_, &(pu, ps))| pu + ps)
            .sum();
        assert_eq!(
            exited, 300,
            "exited = child pre-snap (100) + grandchild pre-snap (200)"
        );

        let corrected = raw.saturating_sub(exited);
        // Correct: root own (30) + child own delta (50) + grandchild own delta (50) = 130.
        assert_eq!(corrected, 130);
    }

    // T-CPU-15: process_cores_used must not exceed system utilization_pct.
    //
    // The process tree is a subset of the system, so process_cores_used (in
    // fractional cores) must never be larger than system utilization_pct
    // (also in fractional cores).  This is the invariant violated by issue #20.
    // A small tolerance covers minor /proc timing jitter between the two reads.
    #[test]
    fn test_process_cores_used_does_not_exceed_system_utilization() {
        let pid = i32::try_from(std::process::id()).expect("PID too large");
        let mut collector = CpuCollector::new(Some(pid));

        let _ = collector.collect().expect("warm-up collect failed");

        // Burn a small amount of CPU so the sample contains non-trivial ticks.
        let _: u64 = (0u64..500_000).map(|x| x.wrapping_mul(x)).sum();

        let m = collector.collect().expect("second collect failed");

        let proc_cores = m
            .process_cores_used
            .expect("process_cores_used must be Some when a PID is tracked");
        let sys_cores = m.utilization_pct; // both in fractional cores (0..N)

        // 10% tolerance for the TOCTOU gap between /proc/stat and /proc/PID/stat.
        let tolerance = sys_cores * 0.10 + 0.05;
        assert!(
            proc_cores <= sys_cores + tolerance,
            "process_cores_used ({proc_cores:.4}) must not exceed system utilization_pct \
             ({sys_cores:.4}) -- regression for issue #20"
        );
    }

    // T-CPU-16: process_utime_secs must not exceed system utime_secs after a
    // child process exits between the warm-up and the real sample.
    //
    // This directly exercises the cutime double-counting bug from issue #20:
    // without the correction, the child's pre-snapshot ticks are counted twice
    // (once via the child's prev entry, once via the parent's cutime delta),
    // pushing process_utime_secs above utime_secs on an otherwise idle system.
    #[test]
    fn test_process_utime_no_double_count_after_child_exits() {
        let pid = i32::try_from(std::process::id()).expect("PID too large");
        let mut collector = CpuCollector::new(Some(pid));

        // Spawn a child that burns a little CPU then exits naturally.
        // `sh` must be available on any Linux host used for testing.
        let mut child = std::process::Command::new("sh")
            .args(["-c", "for i in $(seq 1 20000); do :; done"])
            .spawn()
            .expect("failed to spawn sh -- required for T-CPU-16");

        // Let the child accumulate real ticks before the warm-up snapshot so
        // there is a meaningful pre-snapshot tick count to double-count.
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Warm-up: child is alive; its ticks are stored in prev_proc_ticks.
        let _ = collector.collect().expect("warm-up collect failed");

        // Reap the child.  Its full-lifetime ticks roll into parent's cutime.
        let _ = child.wait().expect("failed to wait for child");

        // Real collect: child is absent from curr_proc_ticks but parent's
        // cutime has grown by the child's entire lifetime.  Without the
        // correction the overcounting would inflate process_utime_secs.
        let m = collector.collect().expect("second collect failed");

        let proc_utime = m
            .process_utime_secs
            .expect("process_utime_secs must be Some when a PID is tracked");
        let sys_utime = m.utime_secs;

        // Allow 5% relative + 50 ms absolute tolerance for /proc timing jitter.
        let tolerance = sys_utime * 0.05 + 0.05;
        assert!(
            proc_utime <= sys_utime + tolerance,
            "process_utime_secs ({proc_utime:.3}s) exceeds system utime_secs ({sys_utime:.3}s) -- \
             cutime double-counting regression (issue #20)"
        );
    }
}
