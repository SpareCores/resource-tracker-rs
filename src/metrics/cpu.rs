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
    /// User-mode CPU seconds consumed by the process tree this interval.
    /// Sum of utime tick deltas / ticks_per_second across all tree members.
    /// None when no PID is tracked.
    pub process_utime_secs: Option<f64>,
    /// System-mode CPU seconds consumed by the process tree this interval.
    /// Sum of stime tick deltas / ticks_per_second across all tree members.
    /// None when no PID is tracked.
    pub process_stime_secs: Option<f64>,
    /// Resident set size of the process tree (sum of VmRSS from /proc/pid/status)
    /// in MiB, sampled at each interval (not a delta).
    /// None when no PID is tracked.
    pub process_rss_mib: Option<u64>,
    /// Disk bytes actually read from storage by the process tree this interval.
    /// Delta of /proc/pid/io read_bytes across all tree members.
    /// None when no PID is tracked or /proc/pid/io is unreadable.
    pub process_disk_read_bytes: Option<u64>,
    /// Disk bytes actually written to storage by the process tree this interval.
    /// Delta of /proc/pid/io write_bytes across all tree members.
    /// None when no PID is tracked or /proc/pid/io is unreadable.
    pub process_disk_write_bytes: Option<u64>,
    /// Total VRAM consumed by the tracked process tree across all GPUs (MiB).
    /// NVIDIA: sum of used_gpu_memory from NVML running-process lists.
    /// AMD: sum of drm-memory-vram from /proc/pid/fdinfo for matched devices.
    /// None when no PID is tracked or no GPU is present on the host.
    pub process_gpu_vram_mib: Option<f64>,
    /// Number of GPUs on which at least one process in the tracked tree has
    /// allocated VRAM or appears in the running-process list.
    /// None when no PID is tracked or no GPU is present on the host.
    pub process_gpu_utilized: Option<u32>,
    /// PIDs in the tracked process tree (root + all descendants).
    /// Populated by CpuCollector; used by main.rs to query per-process GPU stats.
    /// Skipped in JSON/CSV output -- internal routing field only.
    #[serde(skip)]
    pub process_tree_pids: Vec<i32>,
}
