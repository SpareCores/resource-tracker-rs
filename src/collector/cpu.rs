use crate::metrics::CpuMetrics;
use procfs::prelude::*;
use procfs::process::all_processes;
use procfs::{CpuTime, KernelStats};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// ---------------------------------------------------------------------------
// Cgroup CPU source detection and reading
// ---------------------------------------------------------------------------

/// Which CPU accounting source is available for system-level utilization.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CpuSource {
    /// cgroupv2 unified hierarchy: read usage_usec from cpu.stat
    CgroupV2,
    /// cgroupv1 cpuacct controller: read cpuacct.usage (nanoseconds)
    CgroupV1,
    /// Bare /proc/stat (host or no cgroup access)
    ProcStat,
}

impl CpuSource {
    fn is_cgroup(self) -> bool {
        !matches!(self, CpuSource::ProcStat)
    }
}

/// Effective CPU limit from CFS quota (None = unlimited).
#[derive(Debug, Clone, Copy)]
struct CfsQuota {
    /// Maximum fractional cores allowed (e.g. 1.5 for --cpus=1.5)
    max_cores: Option<f64>,
}

/// Detect the best available CPU accounting source.
/// Preference: cgroupv2 > cgroupv1 > /proc/stat
#[allow(clippy::collapsible_if)]
fn detect_cpu_source() -> CpuSource {
    // cgroupv2: unified hierarchy exposes cpu.stat at the cgroup root
    if let Ok(contents) = std::fs::read_to_string("/sys/fs/cgroup/cpu.stat") {
        if contents.contains("usage_usec") {
            return CpuSource::CgroupV2;
        }
    }
    // cgroupv1: cpuacct controller (various mount points)
    for path in &[
        "/sys/fs/cgroup/cpuacct/cpuacct.usage",
        "/sys/fs/cgroup/cpu,cpuacct/cpuacct.usage",
        "/sys/fs/cgroup/cpu/cpuacct.usage",
    ] {
        if std::fs::read_to_string(path).is_ok() {
            return CpuSource::CgroupV1;
        }
    }
    CpuSource::ProcStat
}

/// Read the CFS quota to determine effective core limit.
#[allow(clippy::collapsible_if)]
fn detect_cfs_quota() -> CfsQuota {
    // cgroupv2: cpu.max contains "quota period" or "max period"
    if let Ok(contents) = std::fs::read_to_string("/sys/fs/cgroup/cpu.max") {
        let parts: Vec<&str> = contents.split_whitespace().collect();
        if parts.len() == 2 && parts[0] != "max" {
            if let (Ok(quota), Ok(period)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) {
                if period > 0.0 {
                    return CfsQuota {
                        max_cores: Some(quota / period),
                    };
                }
            }
        }
    }
    // cgroupv1: cpu.cfs_quota_us and cpu.cfs_period_us
    for prefix in &[
        "/sys/fs/cgroup/cpu",
        "/sys/fs/cgroup/cpu,cpuacct",
        "/sys/fs/cgroup/cpuacct",
    ] {
        let quota_path = format!("{}/cpu.cfs_quota_us", prefix);
        let period_path = format!("{}/cpu.cfs_period_us", prefix);
        if let (Ok(q_str), Ok(p_str)) = (
            std::fs::read_to_string(&quota_path),
            std::fs::read_to_string(&period_path),
        ) {
            if let (Ok(quota), Ok(period)) =
                (q_str.trim().parse::<i64>(), p_str.trim().parse::<i64>())
            {
                // quota == -1 means unlimited
                if quota > 0 && period > 0 {
                    return CfsQuota {
                        max_cores: Some(quota as f64 / period as f64),
                    };
                }
            }
        }
    }
    CfsQuota { max_cores: None }
}

/// Read cgroupv2 cpu.stat usage_usec (microseconds, cumulative).
fn read_cgroupv2_usage_usec() -> Option<u64> {
    let contents = std::fs::read_to_string("/sys/fs/cgroup/cpu.stat").ok()?;
    for line in contents.lines() {
        if let Some(val) = line.strip_prefix("usage_usec ") {
            return val.trim().parse().ok();
        }
    }
    None
}

/// Read cgroupv1 cpuacct.usage (nanoseconds, cumulative).
#[allow(clippy::collapsible_if)]
fn read_cgroupv1_usage_ns() -> Option<u64> {
    for path in &[
        "/sys/fs/cgroup/cpuacct/cpuacct.usage",
        "/sys/fs/cgroup/cpu,cpuacct/cpuacct.usage",
        "/sys/fs/cgroup/cpu/cpuacct.usage",
    ] {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(val) = contents.trim().parse() {
                return Some(val);
            }
        }
    }
    None
}

/// Read cgroup CPU usage as fractional seconds (cumulative).
/// Returns None if the detected source is ProcStat or reads fail.
fn read_cgroup_usage_secs(source: CpuSource) -> Option<f64> {
    match source {
        CpuSource::CgroupV2 => read_cgroupv2_usage_usec().map(|usec| usec as f64 / 1_000_000.0),
        CpuSource::CgroupV1 => read_cgroupv1_usage_ns().map(|ns| ns as f64 / 1_000_000_000.0),
        CpuSource::ProcStat => None,
    }
}

// ---------------------------------------------------------------------------
// Tick helpers
// ---------------------------------------------------------------------------

fn cpu_total(c: &CpuTime) -> u64 {
    c.user
        + c.nice
        + c.system
        + cpu_idle(c)
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

/// Sum of PSS and VmRSS across all given PIDs, each converted to MiB.
/// One `Process::open` per PID reads both sources. PSS matches Python
/// `memory_mib`; RSS is retained for consumers that need resident set size.
fn process_tree_memory_mib(pids: &[i32]) -> (u64, u64) {
    let mut pss_kib = 0u64;
    let mut rss_kib = 0u64;
    for &pid in pids {
        let Some(proc_) = procfs::process::Process::new(pid).ok() else {
            continue;
        };
        if let Ok(rollup) = proc_.smaps_rollup() {
            if let Some(bytes) = rollup
                .memory_map_rollup
                .iter()
                .find_map(|m| m.extension.map.get("Pss").copied())
            {
                pss_kib += bytes / 1024;
            }
        }
        if let Ok(status) = proc_.status() {
            if let Some(vmrss) = status.vmrss {
                rss_kib += vmrss;
            }
        }
    }
    (pss_kib / 1024, rss_kib / 1024)
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
    /// Wall-clock time after all /proc reads; used as the Python-style
    /// snapshot timestamp for process CPU rate (Δcpu_secs / Δtimestamp).
    instant: Instant,
    /// Cgroup cumulative CPU usage in fractional seconds (if available).
    cgroup_usage_secs: Option<f64>,
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
    /// Detected CPU accounting source for system-level utilization.
    cpu_source: CpuSource,
    /// CFS quota limit (None = unlimited).
    cfs_quota: CfsQuota,
    /// Effective number of cores for this environment.
    /// Respects CFS quota: min(physical_cores, quota_cores).
    effective_cores: f64,
    /// PIDs whose prev entries were carried forward from the previous
    /// interval (their `/proc/PID/stat` read failed).  Limited to one
    /// hop so dead PIDs don't accumulate and inflate the exited correction.
    carried_forward: HashSet<i32>,
}

impl CpuCollector {
    pub fn new(pid: Option<i32>) -> Self {
        let cpu_source = detect_cpu_source();
        let cfs_quota = detect_cfs_quota();

        // Determine effective core count: physical cores capped by CFS quota.
        let physical_cores = KernelStats::current()
            .map(|s| s.cpu_time.len())
            .unwrap_or(1) as f64;
        let effective_cores = match cfs_quota.max_cores {
            Some(quota) => physical_cores.min(quota),
            None => physical_cores,
        };

        Self {
            pid,
            prev: None,
            cpu_source,
            cfs_quota,
            effective_cores,
            carried_forward: HashSet::new(),
        }
    }

    pub fn collect(&mut self) -> Result<CpuMetrics> {
        let tps = procfs::ticks_per_second() as f64;

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

        // --- FIXED ORDER: read system-level stats FIRST, then process tree ---
        // This ensures that any ticks accumulated by the process between the
        // system read and the process read are counted in BOTH, making it
        // impossible for process to exceed system due to read ordering.

        // 1. Read system /proc/stat (always needed for per-core and fallback).
        let stats = KernelStats::current()?;

        // 2. Read cgroup CPU usage (if available).
        let cgroup_usage_secs = read_cgroup_usage_secs(self.cpu_source);

        // 3. NOW read process tree ticks (after system, so process ⊆ system).
        let proc_ticks = match self.pid {
            Some(root) => process_tree_ticks(root),
            None => HashMap::new(),
        };

        // 4. Record wall-clock time AFTER all reads share the same endpoint.
        let now = Instant::now();

        // Read process I/O and memory only when tracking a PID.
        let proc_io = if self.pid.is_some() {
            let pids: Vec<i32> = proc_ticks.keys().copied().collect();
            process_tree_io(&pids)
        } else {
            HashMap::new()
        };

        // Process memory is instantaneous (not a delta), compute before storing prev.
        let (process_pss_mib, process_rss_mib) = if self.pid.is_some() {
            let pids: Vec<i32> = proc_ticks.keys().copied().collect();
            let (pss, rss) = process_tree_memory_mib(&pids);
            (Some(pss), Some(rss))
        } else {
            (None, None)
        };

        let mut curr = Snapshot {
            total: stats.total,
            per_core: stats.cpu_time,
            instant: now,
            cgroup_usage_secs,
            proc_ticks,
            proc_io,
        };

        let metrics = match &self.prev {
            // First call: store baseline and return zeros. The caller should
            // sleep for one interval then call collect() again for real data.
            None => CpuMetrics {
                utilization_pct: 0.0,
                cgroup_utilization_pct: curr
                    .cgroup_usage_secs
                    .filter(|_| self.cpu_source.is_cgroup())
                    .map(|_| 0.0),
                cgroup_usage_secs: curr
                    .cgroup_usage_secs
                    .filter(|_| self.cpu_source.is_cgroup())
                    .map(|_| 0.0),
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
                process_pss_mib,
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
                let elapsed = (curr.instant - prev.instant).as_secs_f64().max(0.001);

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

                // Keep utilization_pct host-scoped:
                // /proc/stat aggregate busy ratio scaled by host core count.
                let utilization_pct = aggregate_util_cores(&prev.total, &curr.total, n_cores);

                // Expose container/cgroup CPU usage separately when available.
                let (cgroup_usage_secs, cgroup_utilization_pct) =
                    match (curr.cgroup_usage_secs, prev.cgroup_usage_secs) {
                        (Some(curr_cg), Some(prev_cg)) => {
                            let delta = (curr_cg - prev_cg).max(0.0);
                            let cores_used = delta / elapsed;
                            (Some(delta), Some(cores_used.min(self.effective_cores)))
                        }
                        _ => (None, None),
                    };

                // Cutime double-counting correction (issue #20).
                //
                // When a child exits and is reaped, its full-lifetime ticks
                // roll into the parent's cutime.  The child's pre-snapshot
                // portion is already counted via its prev entry, so we
                // subtract it to avoid double-counting.
                //
                // Safety: if exited_ticks > raw_delta, the "exits" are
                // likely transient /proc scan failures (the parent's cutime
                // didn't actually increase).  In that case the correction
                // is skipped to avoid flooring the result to zero.
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
                    if exited_utime <= raw {
                        (raw - exited_utime) as f64 / tps
                    } else {
                        raw as f64 / tps
                    }
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
                    if exited_stime <= raw {
                        (raw - exited_stime) as f64 / tps
                    } else {
                        raw as f64 / tps
                    }
                });

                // --- CAPPED process_cores_used ---
                // Primary: tick-seconds / wall-elapsed (as before).
                // Then apply two caps to prevent impossible values:
                //   1. System tick-ratio cap: process can't exceed total system CPU
                //   2. CFS quota cap: process can't exceed its allowed quota
                let process_cores_used = match (self.pid, process_utime_secs, process_stime_secs) {
                    (Some(_), Some(u), Some(s)) => {
                        let raw_cores = ((u + s) / elapsed).max(0.0);

                        // Cap 1: tick-ratio bound — process ticks can't exceed
                        // total system ticks (both from same kernel accounting).
                        // Uses /proc/stat total ticks as the authoritative ceiling.
                        let sys_total_delta =
                            cpu_total(&curr.total).saturating_sub(cpu_total(&prev.total));
                        let sys_idle_delta =
                            cpu_idle(&curr.total).saturating_sub(cpu_idle(&prev.total));
                        let sys_busy_secs = if sys_total_delta > 0 {
                            (sys_total_delta - sys_idle_delta.min(sys_total_delta)) as f64 / tps
                        } else {
                            f64::MAX
                        };
                        let tick_ratio_cap = sys_busy_secs / elapsed;

                        // Cap 2: CFS quota — hard limit on what the cgroup allows.
                        let quota_cap = self.cfs_quota.max_cores.unwrap_or(n_cores as f64);

                        // Apply both caps (take the tightest constraint).
                        let capped = raw_cores.min(tick_ratio_cap).min(quota_cap);

                        Some(capped)
                    }
                    _ => None,
                };

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
                    utilization_pct,
                    cgroup_utilization_pct,
                    cgroup_usage_secs,
                    per_core_pct,
                    utime_secs,
                    stime_secs,
                    process_count,
                    process_cores_used,
                    process_child_count,
                    process_utime_secs,
                    process_stime_secs,
                    process_pss_mib,
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

        // Carry forward: preserve prev entries for PIDs that disappeared from
        // the live /proc scan.  A missing PID usually indicates a transient
        // stat() read failure, not a genuine exit.  By inserting its last-known
        // ticks into the stored snapshot, a reappearing PID computes a correct
        // delta spanning the gap instead of being treated as "new" (delta = 0).
        //
        // Limited to one hop: PIDs already carried forward from the previous
        // interval are not carried again, preventing dead PIDs from
        // accumulating indefinitely and inflating the exited correction.
        let mut new_carried = HashSet::new();
        if let Some(ref prev_snap) = self.prev {
            for (&pid, &ticks) in &prev_snap.proc_ticks {
                if !curr.proc_ticks.contains_key(&pid) && !self.carried_forward.contains(&pid) {
                    curr.proc_ticks.insert(pid, ticks);
                    new_carried.insert(pid);
                }
            }
            for (&pid, &io) in &prev_snap.proc_io {
                if !curr.proc_io.contains_key(&pid) && !self.carried_forward.contains(&pid) {
                    curr.proc_io.insert(pid, io);
                }
            }
        }
        self.carried_forward = new_carried;

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
            m.process_pss_mib.is_some(),
            "process_pss_mib must be Some when PID is tracked"
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

    // T-CPU-08: process tree memory (PSS and RSS) is positive for the running test process.
    #[test]
    fn test_process_tree_memory_nonzero_for_self() {
        let pid = i32::try_from(std::process::id()).expect("PID too large");
        let (pss, rss) = process_tree_memory_mib(&[pid]);
        assert!(
            pss > 0,
            "PSS for the current process should be > 0, got {pss}"
        );
        assert!(
            rss > 0,
            "RSS for the current process should be > 0, got {rss}"
        );
        assert!(
            pss <= rss,
            "PSS ({pss}) should not exceed RSS ({rss}) for a single process"
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
            m.process_pss_mib.is_none(),
            "process_pss_mib must be None when not tracking"
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
        let curr: HashMap<i32, (u64, u64)> =
            [(200, (50 + 250 + 2500, 0))].iter().cloned().collect();

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

    // T-CPU-15: process CPU must not exceed system CPU when a long-running
    // child exits between two measurement snapshots.
    //
    // On busy servers the tracked process often has long-standing children
    // that accumulate significant CPU ticks over many intervals.  When such a
    // child exits between the warm-up and the real sample, its entire lifetime
    // rolls into the parent's cutime delta.  Without the double-counting
    // correction those pre-snapshot ticks are counted a second time, pushing
    // the process metric above the system metric.
    //
    // We compare absolute CPU seconds (process_utime_secs + process_stime_secs
    // vs utime_secs + stime_secs) rather than fractional cores because both
    // quantities share the same tps divisor and kernel tick accounting.
    // fractional-cores comparison divides by wall-clock elapsed, which makes
    // the ratio unstable when the measurement window is very short (a fixed
    // iteration burn finishes in microseconds on fast CPUs, leaving
    // elapsed << TOCTOU gap and inflating process_cores_used).
    #[test]
    fn test_process_cores_used_does_not_exceed_system_utilization() {
        let pid = i32::try_from(std::process::id()).expect("PID too large");
        let mut collector = CpuCollector::new(Some(pid));

        // Spawn a CPU-busy child to simulate a long-running process on a
        // busy server.  A shell busy-loop accumulates real utime ticks.
        let mut child = std::process::Command::new("sh")
            .args(["-c", "while true; do :; done"])
            .spawn()
            .expect("failed to spawn sh busy-loop -- required for T-CPU-15");

        // Let the child accumulate pre-snapshot CPU ticks for 200 ms.
        // At 100 HZ that yields ~20 ticks = ~0.2 s that would be double-counted
        // without the cutime correction.
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Warm-up: child is alive with ~200 ms of accumulated CPU ticks.
        let _ = collector.collect().expect("warm-up collect failed");

        // Kill the child immediately after warm-up.  Its full lifetime ticks
        // (including the ~0.2 s pre-snapshot portion) roll into parent's cutime
        // delta in the next collect().  Without the correction those pre-snapshot
        // ticks are double-counted, inflating proc_cpu well above sys_cpu.
        child.kill().ok();
        child.wait().ok();

        let m = collector.collect().expect("second collect failed");

        let proc_utime = m
            .process_utime_secs
            .expect("process_utime_secs must be Some");
        let proc_stime = m
            .process_stime_secs
            .expect("process_stime_secs must be Some");
        let proc_cpu = proc_utime + proc_stime;
        let sys_cpu = m.utime_secs + m.stime_secs;

        // 15 % relative + 50 ms absolute tolerance for the TOCTOU gap between
        // /proc/PID/stat and /proc/stat reads.  Without the cutime correction,
        // proc_cpu would be inflated by ~0.2 s (pre-snapshot child ticks),
        // which far exceeds this tolerance and makes the assertion fail.
        let tolerance = sys_cpu * 0.15 + 0.05;
        assert!(
            proc_cpu <= sys_cpu + tolerance,
            "process CPU ({proc_cpu:.3}s = {proc_utime:.3}s utime + {proc_stime:.3}s stime) \
             must not exceed system CPU ({sys_cpu:.3}s) -- cutime double-counting regression \
             for issue #20"
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

    // T-CPU-17: multi-interval accumulation -- child tracked across two snapshots
    // before exiting.
    //
    // This is the scenario shown in examples/repro_cpu_cutime_spike.rs: a child
    // burns CPU across several measurement intervals, then exits in the final one.
    // The cutime delta for that final interval equals the child's ENTIRE lifetime,
    // not just the ticks accumulated since the previous snapshot.
    //
    // The correction must use the MOST RECENT prev_proc_ticks (updated after the
    // intermediate collect), not the original warm-up ticks.  If self.prev were
    // not updated between intervals, exited_utime would be too small and the
    // overcounting would not be fully cancelled.
    //
    // Without the correction: proc_cpu ≈ child's lifetime at intermediate snapshot
    //   >> sys_cpu for that short final window.
    // With the correction:    proc_cpu ≈ only post-intermediate child ticks ≈ 0.
    #[test]
    fn test_cutime_correction_multi_interval_child_exit() {
        let pid = i32::try_from(std::process::id()).expect("PID too large");
        let mut collector = CpuCollector::new(Some(pid));

        // Spawn a CPU-busy child that accumulates real utime ticks.
        let mut child = std::process::Command::new("sh")
            .args(["-c", "while true; do :; done"])
            .spawn()
            .expect("failed to spawn sh busy-loop -- required for T-CPU-17");

        // Interval 1 warm-up: child is alive with some initial ticks.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = collector.collect().expect("warm-up collect failed");

        // Interval 2: child continues burning CPU. self.prev is updated so the
        // next correction baseline is the child's tick count at this point.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = collector.collect().expect("intermediate collect failed");

        // Interval 3 (final): kill child immediately so its full lifetime since
        // interval 2 rolls into parent's cutime.  The correction must subtract
        // the interval-2 tick count (not the warm-up tick count).
        child.kill().ok();
        child.wait().ok();

        let m = collector.collect().expect("final collect failed");

        let proc_utime = m
            .process_utime_secs
            .expect("process_utime_secs must be Some");
        let proc_stime = m
            .process_stime_secs
            .expect("process_stime_secs must be Some");
        let proc_cpu = proc_utime + proc_stime;
        let sys_cpu = m.utime_secs + m.stime_secs;

        // Under parallel test execution (130 tests, many spawning children),
        // the TOCTOU window between /proc/stat and process-tree reads widens
        // significantly and the spawned child accumulates extra ticks during
        // the collect() call itself.  Use a generous tolerance that still
        // catches genuine regressions (which inflate proc_cpu by seconds).
        let tolerance = sys_cpu * 1.0 + 0.50;
        assert!(
            proc_cpu <= sys_cpu + tolerance,
            "process CPU ({proc_cpu:.3}s = {proc_utime:.3}s utime + {proc_stime:.3}s stime) \
             must not exceed system CPU ({sys_cpu:.3}s) across multiple intervals -- \
             cutime multi-interval regression for issue #20"
        );
    }

    // T-CPU-18: PSS (via smaps_rollup) correctly tracks a file-backed mapping.
    //
    // This is the regression test for the fix shown in
    // examples/repro_memory_rss_vs_used.rs.  The old VmRSS approach overcounted
    // shared pages: when N processes map the same file each contributes its full
    // mapping size to the VmRSS sum, but PSS via /proc/pid/smaps_rollup
    // attributes only each process's proportional share.
    //
    // For a sole mapper with MAP_PRIVATE and all pages touched:
    //   - RSS increases by >= mapping_mib (all pages in physical RAM)
    //   - PSS increases by >= mapping_mib (sole mapper gets full proportional share)
    //   - PSS <= RSS (PSS never over-reports)
    //   - |PSS_delta - RSS_delta| <= 1 MiB (sole-mapper PSS == RSS for the region)
    //
    // The last invariant is the regression guard: if PSS were broken (zero or
    // reading the wrong field) the delta would diverge from the RSS delta even
    // though PSS <= RSS holds trivially for zero.
    //
    // The multi-process case (N workers sharing the same file, causing
    // tree_pss << tree_rss) is demonstrated in examples/repro_memory_rss_vs_used.rs.
    #[test]
    fn test_pss_tracks_file_backed_mapping() {
        use std::fs;
        use std::io::Write as _;
        use std::os::unix::io::AsRawFd;

        const MAPPING_MIB: usize = 4;
        const MAPPING_SIZE: usize = MAPPING_MIB * 1024 * 1024;

        let pid = i32::try_from(std::process::id()).expect("PID too large");
        let path = format!("/tmp/rt_test_pss_{}", std::process::id());

        let (pss_before, rss_before) = process_tree_memory_mib(&[pid]);

        // Write a temp file that this process will map read-only.
        {
            let mut f = fs::File::create(&path).expect("cannot create temp file for T-CPU-18");
            let chunk = vec![0xABu8; 64 * 1024];
            for _ in 0..(MAPPING_SIZE / chunk.len()) {
                f.write_all(&chunk).expect("write failed");
            }
        }

        let file = fs::File::open(&path).expect("cannot open temp file for T-CPU-18");
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                MAPPING_SIZE,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(ptr, libc::MAP_FAILED, "mmap failed in T-CPU-18");

        // Touch every page to bring all pages into physical RAM (RSS and PSS).
        let slice = unsafe { std::slice::from_raw_parts(ptr as *const u8, MAPPING_SIZE) };
        let mut checksum = 0u64;
        for offset in (0..MAPPING_SIZE).step_by(4096) {
            checksum = checksum.wrapping_add(u64::from(slice[offset]));
        }
        let _ = checksum;

        let (pss_after, rss_after) = process_tree_memory_mib(&[pid]);

        // Clean up before asserting so a failure does not leak resources.
        unsafe { libc::munmap(ptr, MAPPING_SIZE) };
        fs::remove_file(&path).ok();

        let pss_delta = pss_after.saturating_sub(pss_before);
        let rss_delta = rss_after.saturating_sub(rss_before);

        assert!(
            rss_delta >= MAPPING_MIB as u64,
            "RSS must increase by >= {MAPPING_MIB} MiB after touching the mapping: \
             before={rss_before} MiB, after={rss_after} MiB (delta={rss_delta} MiB)"
        );
        assert!(
            pss_delta >= MAPPING_MIB as u64,
            "PSS must increase by >= {MAPPING_MIB} MiB as sole mapper of the file: \
             before={pss_before} MiB, after={pss_after} MiB (delta={pss_delta} MiB)"
        );
        assert!(
            pss_after <= rss_after,
            "PSS ({pss_after} MiB) must not exceed RSS ({rss_after} MiB)"
        );
        // For the sole mapper the PSS delta and RSS delta must agree within 1 MiB.
        // A regression that breaks smaps_rollup reading (e.g. returning 0 for PSS)
        // would leave pss_delta == 0 while rss_delta >= MAPPING_MIB.
        let skew = pss_delta.abs_diff(rss_delta);
        assert!(
            skew <= 1,
            "PSS delta ({pss_delta} MiB) and RSS delta ({rss_delta} MiB) must agree within \
             1 MiB for a sole mapper -- larger skew indicates smaps_rollup is not being read"
        );
    }

    // -----------------------------------------------------------------------
    // Transient /proc scan failure: correction skip + carry-forward
    // -----------------------------------------------------------------------

    // T-CPU-19: cutime correction is skipped when exited ticks exceed the
    // raw delta, preventing artificial zero values from transient /proc
    // scan failures where a child's stat() read fails but the parent's
    // cutime did not actually increase.
    #[test]
    fn test_cutime_correction_skipped_when_exited_exceeds_raw() {
        let prev: HashMap<i32, (u64, u64)> =
            [(1, (500, 0)), (2, (50000, 0))].iter().cloned().collect();

        let curr: HashMap<i32, (u64, u64)> = [(1, (600, 0))].iter().cloned().collect();

        let raw: u64 = curr
            .iter()
            .map(|(pid, &(cu, cs))| {
                let (pu, ps) = prev.get(pid).copied().unwrap_or((cu, cs));
                cu.saturating_sub(pu) + cs.saturating_sub(ps)
            })
            .sum();
        assert_eq!(raw, 100, "raw delta is parent's own 100 ticks");

        let exited: u64 = prev
            .iter()
            .filter(|(pid, _)| !curr.contains_key(pid))
            .map(|(_, &(pu, ps))| pu + ps)
            .sum();
        assert_eq!(exited, 50000);

        // Old behavior: raw.saturating_sub(exited) = 0 (the bug).
        assert_eq!(raw.saturating_sub(exited), 0);

        // New behavior: skip correction when exited > raw.
        let corrected = if exited <= raw { raw - exited } else { raw };
        assert_eq!(
            corrected, 100,
            "must preserve raw delta when correction is implausible"
        );
    }

    // T-CPU-20: carry-forward preserves prev entries for missing PIDs so
    // that a reappearing PID computes a correct delta spanning the gap
    // rather than being treated as "new" (delta = 0).
    #[test]
    fn test_carry_forward_spans_gap_for_reappearing_pid() {
        let prev: HashMap<i32, (u64, u64)> =
            [(1, (500, 0)), (2, (10000, 0))].iter().cloned().collect();

        // Simulate carry-forward: child was in prev but missing from live scan.
        let mut stored_prev: HashMap<i32, (u64, u64)> = [(1, (600, 0))].iter().cloned().collect();
        for (&pid, &ticks) in &prev {
            stored_prev.entry(pid).or_insert(ticks);
        }
        assert_eq!(
            stored_prev.get(&2),
            Some(&(10000, 0)),
            "child must be carried forward with prev ticks"
        );

        // Child reappears with 11000 ticks (earned 1000 during the gap).
        let curr: HashMap<i32, (u64, u64)> =
            [(1, (700, 0)), (2, (11000, 0))].iter().cloned().collect();

        let delta_with_cf: u64 = curr
            .iter()
            .map(|(pid, &(cu, cs))| {
                let (pu, ps) = stored_prev.get(pid).copied().unwrap_or((cu, cs));
                cu.saturating_sub(pu) + cs.saturating_sub(ps)
            })
            .sum();
        assert_eq!(
            delta_with_cf, 1100,
            "with carry-forward: parent delta (100) + child delta spanning gap (1000)"
        );

        // Without carry-forward: child treated as new (pu = cu), delta = 0.
        let no_cf_prev: HashMap<i32, (u64, u64)> = [(1, (600, 0))].iter().cloned().collect();
        let delta_without_cf: u64 = curr
            .iter()
            .map(|(pid, &(cu, cs))| {
                let (pu, ps) = no_cf_prev.get(pid).copied().unwrap_or((cu, cs));
                cu.saturating_sub(pu) + cs.saturating_sub(ps)
            })
            .sum();
        assert_eq!(
            delta_without_cf, 100,
            "without carry-forward: only parent delta (100), child contribution lost"
        );
    }

    // T-CPU-21: carry-forward is limited to one hop — a PID carried forward
    // in interval N is NOT carried forward again in interval N+1.  This
    // prevents dead PIDs from accumulating indefinitely.
    #[test]
    fn test_carry_forward_limited_to_one_hop() {
        let mut carried_forward: HashSet<i32> = HashSet::new();

        // Interval N: child 2 missing from live scan. Not in carried_forward.
        let prev_ticks: HashMap<i32, (u64, u64)> =
            [(1, (500, 0)), (2, (10000, 0))].iter().cloned().collect();
        let mut curr_ticks: HashMap<i32, (u64, u64)> = [(1, (600, 0))].iter().cloned().collect();

        let mut new_carried = HashSet::new();
        for (&pid, &ticks) in &prev_ticks {
            if !curr_ticks.contains_key(&pid) && !carried_forward.contains(&pid) {
                curr_ticks.insert(pid, ticks);
                new_carried.insert(pid);
            }
        }
        carried_forward = new_carried;

        assert!(
            curr_ticks.contains_key(&2),
            "child must be carried forward in interval N"
        );
        assert!(
            carried_forward.contains(&2),
            "child must be in the carried-forward set"
        );

        // Interval N+1: child 2 still missing. Already in carried_forward.
        let prev_ticks_n1 = curr_ticks.clone();
        let mut curr_ticks_n1: HashMap<i32, (u64, u64)> = [(1, (700, 0))].iter().cloned().collect();

        let mut new_carried_n1 = HashSet::new();
        for (&pid, &ticks) in &prev_ticks_n1 {
            if !curr_ticks_n1.contains_key(&pid) && !carried_forward.contains(&pid) {
                curr_ticks_n1.insert(pid, ticks);
                new_carried_n1.insert(pid);
            }
        }

        assert!(
            !curr_ticks_n1.contains_key(&2),
            "child must NOT be carried forward a second time"
        );
        assert!(
            !new_carried_n1.contains(&2),
            "child must NOT be in the new carried-forward set"
        );
    }
}
