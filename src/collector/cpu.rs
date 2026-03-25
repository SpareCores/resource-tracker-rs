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

fn utilization_pct(prev: &CpuTime, curr: &CpuTime) -> f64 {
    let total = cpu_total(curr).saturating_sub(cpu_total(prev)) as f64;
    let idle  = cpu_idle(curr).saturating_sub(cpu_idle(prev)) as f64;
    if total == 0.0 {
        return 0.0;
    }
    ((total - idle) / total * 100.0).clamp(0.0, 100.0)
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
    for proc in &all {
        if let Ok(stat) = proc.stat() {
            children.entry(stat.ppid).or_default().push(proc.pid);
        }
    }

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
                dir.filter_map(|e| e.ok())
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .chars()
                            .all(|c| c.is_ascii_digit())
                    })
                    .count() as u32
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
                total_cores: curr.per_core.len(),
                utilization_pct: 0.0,
                per_core_pct: vec![0.0; curr.per_core.len()],
                utime_secs: 0.0,
                stime_secs: 0.0,
                process_count,
                process_cores_used: self.pid.map(|_| 0.0),
                process_child_count: self.pid.map(|_| {
                    curr.proc_ticks.len().saturating_sub(1) as u32
                }),
            },

            Some(prev) => {
                // Per-interval CPU time deltas - matches Python resource-tracker's
                // utime/stime columns (Δ ticks / ticks_per_second).
                let utime_secs = (curr.total.user + curr.total.nice)
                    .saturating_sub(prev.total.user + prev.total.nice) as f64 / tps;
                let stime_secs = curr.total.system
                    .saturating_sub(prev.total.system) as f64 / tps;

                let per_core_pct = prev
                    .per_core
                    .iter()
                    .zip(curr.per_core.iter())
                    .map(|(p, c)| utilization_pct(p, c))
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
                    curr.proc_ticks.len().saturating_sub(1) as u32
                });

                CpuMetrics {
                    total_cores: curr.per_core.len(),
                    utilization_pct: utilization_pct(&prev.total, &curr.total),
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
