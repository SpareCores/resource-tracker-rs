use serde::{Deserialize, Serialize};

/// Per-interface metrics: static identity (cached once) + dynamic throughput
/// and link state (polled each interval).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMetrics {
    // ------------------------------------------------------------------
    // Identity - read from /sys/class/net/<iface>/ once at startup.
    // ------------------------------------------------------------------
    pub interface: String,
    /// MAC address, e.g. "00:11:22:33:44:55".
    pub mac_address: Option<String>,
    /// Kernel driver name, e.g. "igc", "virtio_net".
    pub driver: Option<String>,

    // ------------------------------------------------------------------
    // Link state - polled each interval (operstate can flap; speed can
    // change on auto-negotiation).
    // ------------------------------------------------------------------
    /// "up", "down", "unknown", etc. from `/sys/class/net/<iface>/operstate`.
    pub operstate: Option<String>,
    /// Link speed in Mbps. -1 when the driver does not report it.
    pub speed_mbps: Option<i64>,
    /// MTU in bytes.
    pub mtu: Option<u32>,

    // ------------------------------------------------------------------
    // Throughput - derived from /proc/net/dev byte deltas.
    // ------------------------------------------------------------------
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    /// Cumulative bytes received since boot (raw /proc/net/dev counter).
    /// Matches Python resource-tracker's `net_recv_bytes` column.
    pub rx_bytes_total: u64,
    /// Cumulative bytes sent since boot (raw /proc/net/dev counter).
    /// Matches Python resource-tracker's `net_sent_bytes` column.
    pub tx_bytes_total: u64,
}
