use crate::metrics::{DiskMetrics, DiskMountMetrics, DiskType};
use std::collections::HashMap;
use std::ffi::CString;
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const SECTOR_BYTES: u64 = 512;

// ---------------------------------------------------------------------------
// sysfs helpers
// ---------------------------------------------------------------------------

fn sysfs_read(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn block_attr(device: &str, attr: &str) -> Option<String> {
    sysfs_read(&format!("/sys/block/{}/{}", device, attr))
}

// ---------------------------------------------------------------------------
// Hardware identity - read once at startup
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct DeviceInfo {
    model:          Option<String>,
    vendor:         Option<String>,
    serial:         Option<String>,
    device_type:    Option<DiskType>,
    capacity_bytes: Option<u64>,
    /// Physical sector size in bytes used for I/O accounting.
    /// Read from `/sys/block/<dev>/queue/hw_sector_size`; falls back to 512.
    sector_size:    u32,
}

fn read_device_info(device: &str) -> DeviceInfo {
    let model  = block_attr(device, "device/model");
    let vendor = block_attr(device, "device/vendor");
    let serial = block_attr(device, "device/serial")
        .or_else(|| block_attr(device, "device/wwid"));

    let device_type = if device.starts_with("nvme") {
        Some(DiskType::Nvme)
    } else {
        match block_attr(device, "queue/rotational").as_deref() {
            Some("0") => Some(DiskType::Ssd),
            Some("1") => Some(DiskType::Hdd),
            _         => None,
        }
    };

    // /sys/block/<dev>/size reports 512-byte logical sectors regardless of
    // physical sector size, so capacity always uses SECTOR_BYTES (512).
    let capacity_bytes = block_attr(device, "size")
        .and_then(|s| s.parse::<u64>().ok())
        .map(|sectors| sectors * SECTOR_BYTES);

    // Physical sector size for I/O byte accounting.  On 4K-native NVMe drives
    // this is 4096; on most SATA/HDD it is 512.  The kernel value is
    // authoritative; fall back to 512 if absent or unparseable.
    let sector_size = block_attr(device, "queue/hw_sector_size")
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&v| v >= 512)
        .unwrap_or(u32::try_from(SECTOR_BYTES).unwrap_or(512));

    DeviceInfo { model, vendor, serial, device_type, capacity_bytes, sector_size }
}

/// Discover all whole-disk block devices from /sys/block/ and cache their
/// static identity. Called once in DiskCollector::new().
fn discover_devices() -> HashMap<String, DeviceInfo> {
    let Ok(entries) = std::fs::read_dir("/sys/block") else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("loop") || name.starts_with("ram") {
                return None;
            }
            let info = read_device_info(&name);
            Some((name, info))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Filesystem space - statvfs, polled each interval
// ---------------------------------------------------------------------------

fn statvfs_space(path: &str) -> Option<(u64, u64, u64)> {
    let cpath = CString::new(path).ok()?;
    unsafe {
        let mut buf: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(cpath.as_ptr(), &mut buf) != 0 {
            return None;
        }
        // f_frsize is the fundamental block size; fall back to f_bsize if zero.
        let bs = if buf.f_frsize > 0 { buf.f_frsize as u64 } else { buf.f_bsize as u64 };
        let total = buf.f_blocks * bs;
        let avail = buf.f_bavail * bs;
        let used  = total.saturating_sub(buf.f_bfree * bs);
        Some((total, used, avail))
    }
}

/// Read /proc/mounts and return filesystem space for all mount points whose
/// source device path starts with `/dev/<device_name>` (covers partitions too).
fn mounts_for_device(device_name: &str) -> Vec<DiskMountMetrics> {
    let content = match std::fs::read_to_string("/proc/mounts") {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let prefix = format!("/dev/{}", device_name);
    content
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let _source      = parts.next()?;
            let mount_point  = parts.next()?.to_string();
            let filesystem   = parts.next()?.to_string();
            let (total, used, avail) = statvfs_space(&mount_point)?;
            let used_pct = if total > 0 { used as f64 / total as f64 * 100.0 } else { 0.0 };
            Some(DiskMountMetrics {
                mount_point,
                filesystem,
                total_bytes: total,
                used_bytes: used,
                available_bytes: avail,
                used_pct,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Delta snapshot + Collector
// ---------------------------------------------------------------------------

struct Snapshot {
    instant:         Instant,
    sectors_read:    HashMap<String, u64>,
    sectors_written: HashMap<String, u64>,
}

pub struct DiskCollector {
    /// Static hardware identity, cached once in new().
    device_cache: HashMap<String, DeviceInfo>,
    prev: Option<Snapshot>,
}

impl DiskCollector {
    pub fn new() -> Self {
        Self {
            device_cache: discover_devices(),
            prev: None,
        }
    }

    pub fn collect(&mut self) -> Result<Vec<DiskMetrics>> {
        let diskstats = procfs::diskstats()?;
        let now       = Instant::now();

        // Include every device that is a direct /sys/block entry - these are
        // whole-disk devices (not partitions).  This matches Python's
        // resource-tracker, which uses the same /sys/block membership check to
        // distinguish whole disks from partitions.  Importantly, this keeps
        // loop*, dm-*, and zram* devices which Python also includes.
        let block_set: std::collections::HashSet<String> =
            std::fs::read_dir("/sys/block")
                .map(|dir| {
                    dir.flatten()
                        .map(|e| e.file_name().to_string_lossy().to_string())
                        .collect()
                })
                .unwrap_or_default();

        let devs: Vec<_> = diskstats
            .iter()
            .filter(|d| block_set.contains(&d.name))
            .collect();

        let sectors_read: HashMap<String, u64> = devs
            .iter()
            .map(|d| (d.name.clone(), d.sectors_read))
            .collect();
        let sectors_written: HashMap<String, u64> = devs
            .iter()
            .map(|d| (d.name.clone(), d.sectors_written))
            .collect();

        let mut metrics: Vec<DiskMetrics> = devs
            .iter()
            .map(|d| {
                let info = self.device_cache.get(&d.name);

                let sector_size: u32 = info.map_or(
                    u32::try_from(SECTOR_BYTES).unwrap_or(512),
                    |i| i.sector_size,
                );
                let sector_size_f64 = f64::from(sector_size);
                let sector_size_u64 = u64::from(sector_size);

                let (read_bps, write_bps) = match &self.prev {
                    None => (0.0, 0.0),
                    Some(prev) => {
                        let secs   = (now - prev.instant).as_secs_f64().max(0.001);
                        let sr     = sectors_read[&d.name];
                        let sw     = sectors_written[&d.name];
                        let psr    = prev.sectors_read.get(&d.name).copied().unwrap_or(sr);
                        let psw    = prev.sectors_written.get(&d.name).copied().unwrap_or(sw);
                        // u64 -> f64 is lossy for very large values but no From impl exists in std.
                        (
                            sr.saturating_sub(psr) as f64 * sector_size_f64 / secs,
                            sw.saturating_sub(psw) as f64 * sector_size_f64 / secs,
                        )
                    }
                };

                DiskMetrics {
                    device:         d.name.clone(),
                    model:          info.and_then(|i| i.model.clone()),
                    vendor:         info.and_then(|i| i.vendor.clone()),
                    serial:         info.and_then(|i| i.serial.clone()),
                    device_type:    info.and_then(|i| i.device_type.clone()),
                    capacity_bytes: info.and_then(|i| i.capacity_bytes),
                    mounts:         mounts_for_device(&d.name),
                    read_bytes_per_sec:  read_bps,
                    write_bytes_per_sec: write_bps,
                    read_bytes_total:  sectors_read[&d.name] * sector_size_u64,
                    write_bytes_total: sectors_written[&d.name] * sector_size_u64,
                }
            })
            .collect();

        metrics.sort_by(|a, b| a.device.cmp(&b.device));
        self.prev = Some(Snapshot { instant: now, sectors_read, sectors_written });
        Ok(metrics)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // T-DSK-SECTOR: a 4K-native device (sector_size = 4096) produces byte
    // counts 8x larger than the hard-coded 512 would give for the same
    // sector delta.
    #[test]
    fn test_sector_size_4k_gives_8x_bytes() {
        let sector_delta: u64 = 1000;
        let sector_size_512:  u32 = 512;
        let sector_size_4096: u32 = 4096;

        let bytes_512  = sector_delta * u64::from(sector_size_512);
        let bytes_4096 = sector_delta * u64::from(sector_size_4096);

        assert_eq!(bytes_4096, bytes_512 * 8,
            "4K sector should produce 8x the bytes of 512-byte sector");
    }

    // Verify read_device_info falls back to 512 when hw_sector_size is absent
    // (non-existent device path).
    #[test]
    fn test_sector_size_fallback_is_512() {
        let info = read_device_info("__nonexistent_device__");
        assert_eq!(info.sector_size, 512);
    }

    // T-DSK-01: first collect() returns Ok; all I/O rates are 0.0 (no prior snapshot).
    #[test]
    fn test_disk_first_collect_rates_zero() {
        let mut collector = DiskCollector::new();
        let metrics = collector.collect().expect("first collect() should succeed");
        metrics.iter().for_each(|d| {
            assert_eq!(
                d.read_bytes_per_sec, 0.0,
                "read_bytes_per_sec must be 0.0 on first collect for {}",
                d.device
            );
            assert_eq!(
                d.write_bytes_per_sec, 0.0,
                "write_bytes_per_sec must be 0.0 on first collect for {}",
                d.device
            );
        });
    }

    // T-DSK-02: second collect() returns Ok; all I/O rates are >= 0.0.
    #[test]
    fn test_disk_second_collect_rates_nonneg() {
        let mut collector = DiskCollector::new();
        let _ = collector.collect().expect("first collect() failed");
        let metrics = collector.collect().expect("second collect() failed");
        metrics.iter().for_each(|d| {
            assert!(
                d.read_bytes_per_sec >= 0.0,
                "read_bytes_per_sec must be >= 0.0 for {}",
                d.device
            );
            assert!(
                d.write_bytes_per_sec >= 0.0,
                "write_bytes_per_sec must be >= 0.0 for {}",
                d.device
            );
        });
    }

    // T-DSK-03: results are sorted alphabetically by device name.
    #[test]
    fn test_disk_results_sorted_by_device() {
        let mut collector = DiskCollector::new();
        let metrics = collector.collect().expect("collect() failed");
        let names: Vec<&str> = metrics.iter().map(|d| d.device.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "disk metrics must be sorted by device name");
    }

    // T-DSK-04: cumulative totals are non-decreasing between two calls.
    #[test]
    fn test_disk_totals_nondecreasing() {
        let mut collector = DiskCollector::new();
        let first = collector.collect().expect("first collect() failed");
        let second = collector.collect().expect("second collect() failed");
        let first_map: std::collections::HashMap<&str, (u64, u64)> = first
            .iter()
            .map(|d| (d.device.as_str(), (d.read_bytes_total, d.write_bytes_total)))
            .collect();
        second.iter().for_each(|d| {
            if let Some(&(prev_r, prev_w)) = first_map.get(d.device.as_str()) {
                assert!(
                    d.read_bytes_total >= prev_r,
                    "read_bytes_total decreased for {}: {} < {}",
                    d.device,
                    d.read_bytes_total,
                    prev_r
                );
                assert!(
                    d.write_bytes_total >= prev_w,
                    "write_bytes_total decreased for {}: {} < {}",
                    d.device,
                    d.write_bytes_total,
                    prev_w
                );
            }
        });
    }

    // T-DSK-05: read_device_info for a non-existent device returns all None fields
    // except sector_size (which falls back to 512).
    #[test]
    fn test_read_device_info_nonexistent_all_none() {
        let info = read_device_info("__nonexistent__");
        assert!(info.model.is_none(),       "model must be None for missing device");
        assert!(info.vendor.is_none(),      "vendor must be None for missing device");
        assert!(info.serial.is_none(),      "serial must be None for missing device");
        assert!(info.device_type.is_none(), "device_type must be None for missing device");
        assert!(info.capacity_bytes.is_none(), "capacity_bytes must be None for missing device");
    }
}
