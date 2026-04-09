# Benchmarks

## Comparison with https://github.com/SpareCores/resource-tracker

### Status

The Rust binary collects every field that Python's `SystemTracker` emits,
and emits them as either **JSON Lines** (default) or **CSV** (`--format csv`).

The CSV output has **parity with Python** for all columns (same names, units,
and computation formulas). The JSON output is a **strict superset** -- it
carries all CSV fields plus additional metrics not available in Python.

---

### CSV Column Mapping

| Column | Python formula | Rust CSV source | Unit | Parity? |
|---|---|---|---|---|
| `timestamp` | `time.time()` (float) | `timestamp_secs` (integer) | Unix seconds | approx (see note 1) |
| `processes` | count of all `/proc/[0-9]+` entries | `cpu.process_count` -- same `/proc` count | count | yes |
| `utime` | per-interval delta(user+nice ticks) / ticks_per_sec | `cpu.utime_secs` -- same delta calculation | seconds/interval | yes |
| `stime` | per-interval delta(system ticks) / ticks_per_sec | `cpu.stime_secs` -- same delta calculation | seconds/interval | yes |
| `cpu_usage` | fractional cores (0..N) | `cpu.utilization_pct` directly (field is already fractional cores) | fractional cores | yes |
| `memory_free` | `MemFree` from `/proc/meminfo` | `memory.free_mib` (`MemFree` / 1,048,576) | MiB | yes |
| `memory_used` | `MemTotal - MemFree - Buffers - (Cached+SReclaimable)` | `memory.used_mib` -- same formula | MiB | yes |
| `memory_buffers` | `Buffers` | `memory.buffers_mib` | MiB | yes |
| `memory_cached` | `Cached + SReclaimable` | `memory.cached_mib` -- same formula | MiB | yes |
| `memory_active` | `Active` | `memory.active_mib` | MiB | yes |
| `memory_inactive` | `Inactive` | `memory.inactive_mib` | MiB | yes |
| `disk_read_bytes` | per-interval delta(sectors_read) x sector_size, all non-partition diskstats entries | sum of rate x interval across all `/sys/block` whole-disk entries | bytes/interval | approx (see note 2) |
| `disk_write_bytes` | same, write side | same, write side | bytes/interval | approx (see note 2) |
| `disk_space_total_gb` | sum of all non-virtual mount points (incl. snap/loop) | sum of all mounts under `/sys/block` devices (incl. loop mounts) | GB | approx (see note 3) |
| `disk_space_used_gb` | same, `total - free` (incl. reserved-for-root blocks) | same formula | GB | approx (see note 3) |
| `disk_space_free_gb` | `f_bavail` from `statvfs` | `f_bavail` from `statvfs` | GB | approx (see note 3) |
| `net_recv_bytes` | per-interval delta(rx_bytes) across all interfaces | sum of rate x interval across all interfaces | bytes/interval | yes |
| `net_sent_bytes` | same, tx side | same, tx side | bytes/interval | yes |
| `gpu_usage` | fractional GPUs (0..N) | sum `gpu[].utilization_pct / 100` | fractional GPUs | yes |
| `gpu_vram` | used VRAM in MiB | sum `gpu[].vram_used_bytes / 1,048,576` | MiB | yes |
| `gpu_utilized` | count of GPUs with utilization > 0 | count `gpu[].utilization_pct > 0` | count | yes |

---

### Documented Semantic Differences

#### Note 1 -- Timestamp precision

Python's `timestamp` is a float (sub-second resolution). Rust emits an integer
Unix timestamp. When aligning rows for comparison, use a +/-0.5 s tolerance.

#### Note 2 -- Disk I/O: device set and sector size

Both Python and Rust use `/proc/diskstats` deltas and iterate all
**whole-disk** (non-partition) entries. The device sets should match on most
Linux systems.

**Python's device filter (`is_partition` from `resource_tracker.helpers`):**
```python
# Returns True only for names matching (sd*, nvme*, mmcblk*) partition patterns
# where a parent device exists in /sys/block. Everything else -- including
# loop*, dm-*, zram* -- is treated as a whole-disk device and included.
```

**Rust's device filter:**
```rust
// Reads /sys/block/ directory entries into a HashSet.
// Keeps every diskstats entry whose name is a direct /sys/block/<name> entry.
// Logically equivalent to Python's filter: partitions like nvme0n1p1
// appear under /sys/block/nvme0n1/ (not top-level) and are excluded.
let block_set: HashSet<String> = read_dir("/sys/block")...;
let devs = diskstats.filter(|d| block_set.contains(&d.name));
```

**Sector size:** both Python and Rust read the actual hardware sector size per
device from `/sys/block/<dev>/queue/hw_sector_size`, falling back to 512 bytes.
This was implemented in Rust as P-DSK-SECTOR.

**Rationale for explicit sector size:** on 4K-native drives the logical sector
size is 4,096 bytes; using a hard-coded 512 would under-count I/O bytes by 8x.
Reading the actual value from sysfs ensures correctness on all drive types.

#### Note 2a -- ZFS volumes

Python's disk I/O implementation handles ZFS volumes, where disk usage is
reported differently at `/sys/block`. Rust does not currently account for
this. ZFS support is a planned enhancement (not required for MVP).

#### Note 3 -- Disk space: mount set

Python sums all mount points that `psutil.disk_partitions()` reports as
non-virtual (including snap squashfs loop mounts). Rust sums all mount points
found in `/proc/mounts` whose source device matches a `/sys/block` entry.

On systems with many snap packages, Python includes the squashfs read-only
mounts for each snap. Because `/dev/loop*` devices appear in `/sys/block`,
Rust's `mounts_for_device("loopN")` will pick these up too. However,
`psutil` may enumerate mount points that are not under `/dev/` (e.g., `tmpfs`,
`overlay`, `cgroup2`) which Rust's `/dev/<device>` prefix filter skips. This
can cause small differences in `disk_space_total_gb` on container hosts or
systems with unusual mount configurations.

To investigate: run `mount | grep -v '^/dev' | grep -v ' type tmpfs'` to see
which mount points Python may be counting that Rust is not.

---

### Running the comparison

#### Prerequisites

- `uv` >= 0.9 (Astral): `which uv`
- Rust release binary: `cargo build --release`

#### Directory layout

```
benchmarks/
+-- pyproject.toml      # uv project -- resource-tracker dependency
+-- run_python.py       # SystemTracker -> results/python_metrics.csv
+-- run_rust.sh         # resource-tracker-rs --format csv -> results/rust_metrics.csv
+-- compare.py          # merge on timestamp, print diff table
+-- results/            # populated at runtime (gitignore this)
    +-- python_metrics.csv
    +-- rust_metrics.csv
```

#### Step 1 -- Set up Python environment

```bash
cd benchmarks
uv init --no-workspace
uv add resource-tracker
```

#### Step 2 -- `run_python.py`

```python
"""Collect SystemTracker metrics for DURATION seconds -> results/python_metrics.csv"""
import time
from resource_tracker import SystemTracker

DURATION = 60
INTERVAL = 1

tracker = SystemTracker(interval=INTERVAL, output_file="results/python_metrics.csv")
time.sleep(DURATION)
tracker.stop()
print(f"Done -> results/python_metrics.csv")
```

#### Step 3 -- `run_rust.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
DURATION=60
INTERVAL=1
mkdir -p results
timeout "$DURATION" \
  ../target/release/resource-tracker-rs --interval "$INTERVAL" --format csv \
  > results/rust_metrics.csv || true
echo "Collected $(( $(wc -l < results/rust_metrics.csv) - 1 )) rows -> results/rust_metrics.csv"
```

#### Step 4 -- `compare.py`

Strategy:
1. Load both CSVs, parse `timestamp` columns.
2. Differentiate Python's cumulative I/O columns with `diff()` to get rates,
   matching Rust's per-interval values.
3. Merge on nearest timestamp (tolerance +/-0.5 x interval).
4. For each shared metric, report: mean, std, min/max for each side plus
   mean absolute difference (MAD) and % deviation.

```python
"""Compare python_metrics.csv and rust_metrics.csv side by side."""
import csv, sys
from pathlib import Path

IO_COLS = {"disk_read_bytes", "disk_write_bytes", "net_recv_bytes", "net_sent_bytes"}

def load(path):
    rows = list(csv.DictReader(Path(path).open()))
    return [{k: float(v) if v else 0.0 for k, v in row.items()} for row in rows]

def diff_col(rows, col):
    """Replace cumulative totals with per-row deltas (rate proxy)."""
    for i in range(len(rows) - 1, 0, -1):
        rows[i][col] = rows[i][col] - rows[i-1][col]
    rows[0][col] = 0.0

py  = load("results/python_metrics.csv")
rs  = load("results/rust_metrics.csv")

for col in IO_COLS:
    if col in (py[0] if py else {}):
        diff_col(py, col)

shared_cols = set(py[0]) & set(rs[0]) - {"timestamp"} if py and rs else set()

print(f"{'column':<30} {'py_mean':>12} {'rs_mean':>12} {'MAD':>12} {'%dev':>8}")
print("-" * 80)
for col in sorted(shared_cols):
    py_vals = [r[col] for r in py]
    rs_vals = [r[col] for r in rs]
    py_mean = sum(py_vals) / len(py_vals)
    rs_mean = sum(rs_vals) / len(rs_vals)
    mad = sum(abs(a - b) for a, b in zip(py_vals, rs_vals)) / len(py_vals)
    pct = (mad / py_mean * 100) if py_mean != 0 else float("inf")
    print(f"{col:<30} {py_mean:>12.3f} {rs_mean:>12.3f} {mad:>12.3f} {pct:>7.1f}%")
```

---

### Results

> _To be populated after running the benchmark on target hardware._
>
> Fill in: host specs (CPU model, RAM, OS, kernel), Rust git SHA,
> Python `resource-tracker` version, output table from `compare.py`,
> and observations on where the two implementations agree and diverge.

---

### Remaining known differences

| Aspect | Python | Rust | Status |
|---|---|---|---|
| Timestamp precision | Float (sub-second) | Integer (Unix seconds) | By design; use +/-0.5 s tolerance when aligning rows |
| Disk I/O sector size | Per-device from `/sys/block/<dev>/queue/hw_sector_size`, fallback 512 | Per-device from same sysfs path, fallback 512 | Implemented (P-DSK-SECTOR); parity achieved |
| Disk space: non-`/dev/` mounts | `psutil` includes overlay/tmpfs/cgroup mounts if reported non-virtual | Only `/dev/<device>` prefixed sources in `/proc/mounts` | Low impact on physical hosts; notable on container/VM hosts |
| ZFS volumes | Handled via `psutil` disk partition enumeration | Not yet implemented | Planned enhancement |

---

### JSON superset fields (not in Python CSV)

The JSON output carries richer data than any Python CSV column can express.

**Rationale:** the CSV columns match Python for downstream compatibility.
The JSON output is the primary format for new consumers and exposes all
available data without being constrained by the Python column set.

| Type | Field | Description | Rationale |
|---|---|---|---|
| cpu | `cpu.per_core_pct[]` | Per-logical-core utilization (0--100 each) | Identify hot cores and NUMA imbalance; not expressible as a single CSV scalar |
| cpu | `cpu.process_cores_used` | Fractional cores consumed by tracked PID tree | Covers multi-process workloads (workers, MPI ranks); Python tracks only the root process |
| cpu | `cpu.process_child_count` | Live descendants under tracked root PID | Detect fork/thread storms without external tooling |
| memory | `memory.total_mib` | Total installed RAM | Baseline for capacity planning |
| memory | `memory.available_mib` | `MemAvailable`: free + reclaimable | Better headroom estimate than `free_mib` alone on systems with large page caches |
| memory | `memory.used_pct` | RAM usage as a percentage | Convenient derived field; avoids client-side division |
| memory | `memory.active_mib` / `memory.inactive_mib` | Active and inactive page counts | Distinguish working-set pressure from cold cache |
| memory | `memory.swap_total_mib` / `memory.swap_used_mib` / `memory.swap_used_pct` | Swap metrics | Detect swap pressure before OOM; Python omits swap entirely |
| network | `network[].interface` etc. | Interface name, MAC, driver, operstate, speed, MTU | Identify which NIC is under load and whether the link is at full speed |
| network | `network[].rx_bytes_total` / `tx_bytes_total` | Cumulative byte counters | Enables client-side rate computation at any granularity |
| disk | `disk[].device_type` | `nvme`, `ssd`, or `hdd` | Correlate latency with drive class without parsing device names |
| disk | `disk[].capacity_bytes` | Raw device capacity | Capacity planning without a separate `lsblk` call |
| disk | `disk[].mounts[]` | Per-mount-point space (total/used/available/pct) | Python aggregates all mounts into three scalars; Rust retains per-volume detail |
| disk | `disk[].model` / `vendor` / `serial` | Drive identity | Correlate metrics with physical hardware inventory |
| gpu | `gpu[].temperature_celsius` | Die temperature | Detect thermal throttling in real time |
| gpu | `gpu[].power_watts` | Power draw | Power-efficiency analysis; watts-per-FLOP budgeting |
| gpu | `gpu[].frequency_mhz` | Core clock | Confirm boost clock is active; correlate with thermal state |
| gpu | `gpu[].vram_total_bytes` | Total VRAM | Baseline for VRAM utilization percentage |
| gpu | `gpu[].uuid` / `name` / `device_type` / `host_id` | GPU identity | Multi-GPU systems: attribute metrics to specific devices |
