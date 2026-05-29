pub mod cpu;
pub mod disk;
pub mod disk_mount;
pub mod disk_type;
pub mod gpu;
pub mod host;
pub mod memory;
pub mod network;

pub use cpu::CpuMetrics;
pub use disk::DiskMetrics;
pub use disk_mount::DiskMountMetrics;
pub use disk_type::DiskType;
pub use gpu::GpuMetrics;
pub use host::{CloudInfo, HostInfo};
pub use memory::MemoryMetrics;
pub use network::NetworkMetrics;

use serde::{Deserialize, Serialize};

/// A single point-in-time observation of all tracked resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    /// Unix timestamp (seconds) when this sample was taken.
    pub timestamp_secs: u64,
    /// Actual elapsed milliseconds since the previous sample was collected.
    /// None for the very first real sample (no prior collection to compare against).
    /// Included in JSON output when present; not a CSV column.
    /// Use this -- not the configured interval -- when converting rates to per-interval counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_interval_ms: Option<u64>,
    /// Optional job label supplied via CLI or config file.
    pub job_name: Option<String>,
    /// Root PID of the process tree being tracked, if any.
    /// Carried here so the CSV serializer can emit `process_pid` without
    /// needing access to `Config`.
    pub tracked_pid: Option<i32>,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub network: Vec<NetworkMetrics>,
    pub disk: Vec<DiskMetrics>,
    /// One entry per detected GPU/NPU/TPU. Empty on CPU-only hosts.
    pub gpu: Vec<GpuMetrics>,
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::CpuMetrics;

    fn minimal_sample(actual_ms: Option<u64>) -> Sample {
        Sample {
            timestamp_secs: 1_700_000_000,
            actual_interval_ms: actual_ms,
            job_name: None,
            tracked_pid: None,
            cpu: CpuMetrics::default(),
            memory: MemoryMetrics::default(),
            network: vec![],
            disk: vec![],
            gpu: vec![],
        }
    }

    // T-SAMPLE-01: actual_interval_ms is present in JSON when Some.
    // Downstream consumers (e.g. dashboards) rely on this field to compute
    // accurate per-interval rates when the scheduler did not wake the tracker
    // on time.
    #[test]
    fn test_actual_interval_ms_present_in_json_when_some() {
        let sample = minimal_sample(Some(1042));
        let v = serde_json::to_value(&sample).expect("serialize failed");
        let ms = v["actual_interval_ms"]
            .as_u64()
            .expect("actual_interval_ms must be a number in JSON when Some");
        assert_eq!(ms, 1042, "actual_interval_ms value must round-trip through JSON");
    }

    // T-SAMPLE-02: actual_interval_ms is absent from JSON when None.
    // The first real sample has no prior baseline; the field must not appear
    // rather than emitting a JSON null (skip_serializing_if = "Option::is_none").
    #[test]
    fn test_actual_interval_ms_absent_from_json_when_none() {
        let sample = minimal_sample(None);
        let v = serde_json::to_value(&sample).expect("serialize failed");
        assert!(
            v["actual_interval_ms"].is_null(),
            "actual_interval_ms must not appear in JSON when None, got: {:?}",
            v["actual_interval_ms"]
        );
    }

    // T-SAMPLE-03: timestamp_secs round-trips through JSON as a positive integer.
    // This documents the guarantee that the field encodes wall-clock Unix seconds
    // and is not affected by the configured or actual interval.
    #[test]
    fn test_timestamp_secs_round_trips_through_json() {
        let sample = minimal_sample(Some(999));
        let v = serde_json::to_value(&sample).expect("serialize failed");
        let ts = v["timestamp_secs"]
            .as_u64()
            .expect("timestamp_secs must be a u64 in JSON");
        assert_eq!(ts, 1_700_000_000, "timestamp_secs must round-trip exactly");
    }
}
