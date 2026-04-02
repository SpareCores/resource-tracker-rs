use serde::{Deserialize, Serialize};

/// CPU metrics derived from /proc/stat tick deltas.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CpuMetrics {
    /// Aggregate CPU utilization expressed as fractional cores in use (0.0..N_cores).
    /// e.g. 4.6 on a 16-core host means ~4.6 vCPUs are fully utilized.
    /// Not clamped; values very slightly above N_cores are valid under kernel rounding.
    /// N_cores is available via host discovery (host_vcpus).
    pub utilization_pct: f64,
    /// Per-core utilization indexed by logical CPU number (0.0–100.0 each).
    pub per_core_pct: Vec<f64>,
    /// User+nice mode CPU time consumed across all cores in this interval (seconds).
    /// Equivalent to Δ(user+nice ticks) / ticks_per_second.
    /// Matches Python resource-tracker's `utime` column.
    pub utime_secs: f64,
    /// System mode CPU time consumed across all cores in this interval (seconds).
    /// Equivalent to Δ(system ticks) / ticks_per_second.
    /// Matches Python resource-tracker's `stime` column.
    pub stime_secs: f64,
    /// Number of processes currently in a runnable state (from /proc/stat
    /// `procs_running`). Matches Python resource-tracker's `processes` column.
    pub process_count: u32,
    /// Fractional cores actively consumed by the tracked process tree
    /// (root process + all descendants), derived from `/proc/<pid>/stat` tick
    /// deltas divided by elapsed wall-clock ticks.
    /// e.g. 2.0 means the tree is consuming the equivalent of 2 full cores.
    /// None when no process PID is being tracked.
    pub process_cores_used: Option<f64>,
    /// Number of live descendant processes under the tracked root PID.
    /// Does not include the root process itself.
    /// None when no process PID is being tracked.
    pub process_child_count: Option<u32>,
}
