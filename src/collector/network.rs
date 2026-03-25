use crate::metrics::NetworkMetrics;
use procfs::net::dev_status;
use std::collections::HashMap;
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// ---------------------------------------------------------------------------
// sysfs helpers
// ---------------------------------------------------------------------------

fn sysfs_read(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn net_attr(iface: &str, attr: &str) -> Option<String> {
    sysfs_read(&format!("/sys/class/net/{}/{}", iface, attr))
}

// ---------------------------------------------------------------------------
// Hardware identity - read once at startup
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct InterfaceInfo {
    mac_address: Option<String>,
    /// Kernel driver name resolved from the /sys/class/net/<if>/device/driver
    /// symlink basename, e.g. "igc", "virtio_net", "e1000e".
    driver: Option<String>,
}

fn read_interface_info(iface: &str) -> InterfaceInfo {
    let mac_address = net_attr(iface, "address");

    // The driver symlink points to something like
    // ../../../../bus/pci/drivers/igc - we just want the basename.
    let driver = std::fs::read_link(format!("/sys/class/net/{}/device/driver", iface))
        .ok()
        .and_then(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
        });

    InterfaceInfo { mac_address, driver }
}

/// Discover all non-loopback interfaces and cache their static identity.
/// Called once in NetworkCollector::new().
fn discover_interfaces() -> HashMap<String, InterfaceInfo> {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name == "lo" {
                return None;
            }
            let info = read_interface_info(&name);
            Some((name, info))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Dynamic link state - polled each interval
// ---------------------------------------------------------------------------

fn read_operstate(iface: &str) -> Option<String> {
    net_attr(iface, "operstate")
}

fn read_speed_mbps(iface: &str) -> Option<i64> {
    net_attr(iface, "speed")?.parse().ok()
}

fn read_mtu(iface: &str) -> Option<u32> {
    net_attr(iface, "mtu")?.parse().ok()
}

// ---------------------------------------------------------------------------
// Delta snapshot + Collector
// ---------------------------------------------------------------------------

struct Snapshot {
    instant:  Instant,
    rx_bytes: HashMap<String, u64>,
    tx_bytes: HashMap<String, u64>,
}

pub struct NetworkCollector {
    /// Static hardware identity, cached once in new().
    iface_cache: HashMap<String, InterfaceInfo>,
    prev: Option<Snapshot>,
}

impl NetworkCollector {
    pub fn new() -> Self {
        Self {
            iface_cache: discover_interfaces(),
            prev: None,
        }
    }

    pub fn collect(&mut self) -> Result<Vec<NetworkMetrics>> {
        let devs = dev_status()?;
        let now  = Instant::now();

        let rx_bytes: HashMap<String, u64> = devs
            .iter()
            .map(|(name, s)| (name.clone(), s.recv_bytes))
            .collect();
        let tx_bytes: HashMap<String, u64> = devs
            .iter()
            .map(|(name, s)| (name.clone(), s.sent_bytes))
            .collect();

        let mut metrics: Vec<NetworkMetrics> = devs
            .keys()
            .filter(|n| *n != "lo")
            .map(|name| {
                let info = self.iface_cache.get(name);

                let (rx_bps, tx_bps) = match &self.prev {
                    None => (0.0, 0.0),
                    Some(prev) => {
                        let secs   = (now - prev.instant).as_secs_f64().max(0.001);
                        let rx     = rx_bytes[name];
                        let tx     = tx_bytes[name];
                        let prx    = prev.rx_bytes.get(name).copied().unwrap_or(rx);
                        let ptx    = prev.tx_bytes.get(name).copied().unwrap_or(tx);
                        (
                            rx.saturating_sub(prx) as f64 / secs,
                            tx.saturating_sub(ptx) as f64 / secs,
                        )
                    }
                };

                NetworkMetrics {
                    interface:    name.clone(),
                    mac_address:  info.and_then(|i| i.mac_address.clone()),
                    driver:       info.and_then(|i| i.driver.clone()),
                    operstate:    read_operstate(name),
                    speed_mbps:   read_speed_mbps(name),
                    mtu:          read_mtu(name),
                    rx_bytes_per_sec: rx_bps,
                    tx_bytes_per_sec: tx_bps,
                    rx_bytes_total: rx_bytes[name],
                    tx_bytes_total: tx_bytes[name],
                }
            })
            .collect();

        metrics.sort_by(|a, b| a.interface.cmp(&b.interface));
        self.prev = Some(Snapshot { instant: now, rx_bytes, tx_bytes });
        Ok(metrics)
    }
}
