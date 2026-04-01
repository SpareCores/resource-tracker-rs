# Benchmarks

## Comparison with https://github.com/SpareCores/resource-tracker

### Status

The Rust binary collects every field that Python's `SystemTracker` emits,
and emits them as either **JSON Lines** (default) or **CSV** (`--format csv`).

The CSV output has **parity with Python** for all columns (same names, units,
and computation formulas). The JSON output is a **strict superset** — it
carries all CSV fields plus additional metrics not available in Python.

---

### CSV Column Mapping

| Column                | Python formula                                            | Rust CSV source                                                         | Unit             | Parity? |
|-----------------------|-----------------------------------------------------------|-------------------------------------------------------------------------|------------------|---------|
| `timestamp`           | `time.time()` (float)                                     | `timestamp_secs` (integer)                                              | Unix seconds     | ≈ (see note 1) |
| `processes`           | count of all `/proc/[0-9]+` entries                       | `cpu.process_count` — same `/proc` count                                | count            | ✓       |
| `utime`               | per-interval Δ(user+nice ticks) / ticks_per_sec           | `cpu.utime_secs` — same delta calculation                               | seconds/interval | ✓       |
| `stime`               | per-interval Δ(system ticks) / ticks_per_sec              | `cpu.stime_secs` — same delta calculation                               | seconds/interval | ✓       |
| `cpu_usage`           | fractional cores (0..N)                                   | `cpu.utilization_pct / 100 × total_cores`                               | fractional cores | ✓       |
| `memory_free`         | `MemFree` from `/proc/meminfo`                            | `memory.free_kib` (`MemFree`) — exact match                             | KiB              | ✓       |
| `memory_used`         | `MemTotal − MemFree − Buffers − (Cached+SReclaimable)`    | `memory.used_kib` — same formula                                        | KiB              | ✓       |
| `memory_buffers`      | `Buffers`                                                 | `memory.buffers_kib`                                                    | KiB              | ✓       |
| `memory_cached`       | `Cached + SReclaimable`                                   | `memory.cached_kib` — same formula                                      | KiB              | ✓       |
| `memory_active`       | `Active`                                                  | `memory.active_kib`                                                     | KiB              | ✓       |
| `memory_inactive`     | `Inactive`                                                | `memory.inactive_kib`                                                   | KiB              | ✓       |
| `disk_read_bytes`     | per-interval Δ(sectors_read) × sector_size, all non-partition diskstats entries | sum of rate × interval across all `/sys/block` whole-disk entries | bytes/interval | ≈ (see note 2) |
| `disk_write_bytes`    | same, write side                                          | same, write side                                                        | bytes/interval   | ≈ (see note 2) |
| `disk_space_total_gb` | sum of all non-virtual mount points (incl. snap/loop)     | sum of all mounts under `/sys/block` devices (incl. loop mounts)        | GB               | ≈ (see note 3) |
| `disk_space_used_gb`  | same, `total − free` (incl. reserved-for-root blocks)    | same formula                                                            | GB               | ≈ (see note 3) |
| `disk_space_free_gb`  | `f_bavail` from `statvfs`                                 | `f_bavail` from `statvfs`                                               | GB               | ≈ (see note 3) |
| `net_recv_bytes`      | per-interval Δ(rx_bytes) across all interfaces            | sum of rate × interval across all interfaces                            | bytes/interval   | ✓       |
| `net_sent_bytes`      | same, tx side                                             | same, tx side                                                           | bytes/interval   | ✓       |
| `gpu_usage`           | fractional GPUs (0..N)                                    | sum `gpu[].utilization_pct / 100`                                       | fractional GPUs  | ✓       |
| `gpu_vram`            | used VRAM in MiB                                          | sum `gpu[].vram_used_bytes / 1_048_576`                                 | MiB              | ✓       |
| `gpu_utilized`        | count of GPUs with utilization > 0                        | count `gpu[].utilization_pct > 0`                                       | count            | ✓       |

---

### Documented Semantic Differences

#### Note 1 — Timestamp precision

Python's `timestamp` is a float (sub-second resolution). Rust emits an integer
Unix timestamp. When aligning rows for comparison, use a ±0.5 s tolerance.

#### Note 2 — Disk I/O: device set

Both Python and Rust use `/proc/diskstats` deltas and iterate all
**whole-disk** (non-partition) entries. The device sets should match on most
Linux systems.

**Python's device filter (`is_partition` from `resource_tracker.helpers`):**
```python
# Returns True only for names matching (sd*, nvme*, mmcblk*) partition patterns
# where a parent device exists in /sys/block. Everything else — including
# loop*, dm-*, zram* — is treated as a whole-disk device and included.
```

**Rust's device filter:**
```rust
// Reads /sys/block/ directory entries into a HashSet.
// Keeps every diskstats entry whose name is a direct /sys/block/<name> entry.
// This is logically equivalent to Python's filter: partitions like nvme0n1p1
// appear under /sys/block/nvme0n1/ (not as top-level entries) and are excluded.
// loop*, dm-*, zram* are top-level /sys/block entries and are included.
let block_set: HashSet<String> = read_dir("/sys/block")...;
let devs = diskstats.filter(|d| block_set.contains(&d.name));
```

**Remaining potential discrepancy:** Python uses `get_sector_sizes()` to look
up the actual sector size per device (from `/sys/block/<dev>/queue/hw_sector_size`),
falling back to 512. Rust always uses 512 bytes/sector. On most modern NVMe/SSD
drives the logical sector size is 512 bytes, but on some drives it is 4096
(4K-native). If any tracked device has a non-512-byte sector size, Rust will
under-count I/O bytes by a factor of up to 8×.

To fix: read `/sys/block/<dev>/queue/hw_sector_size` (or `logical_block_size`)
at startup and use it in the delta calculation instead of the hard-coded 512.

#### Note 2a — ZFS volumes

Python's disk I/O implementation handles ZFS volumes, where disk usage is
reported differently at `/sys/block`.  Rust does not currently account for
this.  ZFS support is a planned enhancement (not required for MVP) and should
be tracked in the specification and todo list.

#### Note 3 — Disk space: mount set

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

- `uv` ≥ 0.9 (Astral): `which uv`
- Rust release binary: `just build_release`

#### Directory layout

```
benchmarks/
├── pyproject.toml      # uv project — resource-tracker dependency
├── run_python.py       # SystemTracker → results/python_metrics.csv
├── run_rust.sh         # resource-tracker-rs --format csv → results/rust_metrics.csv
├── compare.py          # merge on timestamp, print diff table
└── results/            # populated at runtime (gitignore this)
    ├── python_metrics.csv
    └── rust_metrics.csv
```

#### Step 1 — Set up Python environment

```bash
cd benchmarks
uv init --no-workspace
uv add resource-tracker
```

#### Step 2 — `run_python.py`

```python
"""Collect SystemTracker metrics for DURATION seconds → results/python_metrics.csv"""
import time
from resource_tracker import SystemTracker

DURATION = 60
INTERVAL = 1

tracker = SystemTracker(interval=INTERVAL, output_file="results/python_metrics.csv")
time.sleep(DURATION)
tracker.stop()
print(f"Done → results/python_metrics.csv")
```

#### Step 3 — `run_rust.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
DURATION=60
INTERVAL=1
mkdir -p results
timeout "$DURATION" \
  ../target/release/resource-tracker-rs --interval "$INTERVAL" --format csv \
  > results/rust_metrics.csv || true
echo "Collected $(( $(wc -l < results/rust_metrics.csv) - 1 )) rows → results/rust_metrics.csv"
```

#### Step 4 — `compare.py`

Strategy:
1. Load both CSVs, parse `timestamp` columns.
2. Differentiate Python's cumulative I/O columns with `diff()` to get rates,
   matching Rust's per-interval values.
3. Merge on nearest timestamp (tolerance ±0.5 × interval).
4. For each shared metric, report: mean, std, min/max for each side plus
   mean absolute difference (MAD) and % deviation.

```python
"""Compare python_metrics.csv and rust_metrics.csv side by side."""
import csv, sys
from pathlib import Path

IO_COLS = {"disk_read_bytes", "disk_write_bytes", "net_recv_bytes", "net_sent_bytes"}

def load(path):
    rows = list(csv.DictReader(Path(path).open()))
    # convert all values to float where possible
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

#### Step 5 — Justfile recipes

```justfile
# Install Python resource-tracker via uv
bench_setup:
    cd benchmarks && uv sync

# Run both trackers simultaneously for 60 s, CSV output on both sides
bench_run:
    mkdir -p benchmarks/results
    bash benchmarks/run_rust.sh &
    cd benchmarks && uv run python run_python.py
    wait

# Compare outputs and print diff table
bench_compare:
    cd benchmarks && uv run python compare.py

# Full pipeline
benchmark: bench_setup bench_run bench_compare
```

---

### Results

> _To be populated after running `just benchmark` on target hardware._
>
> Fill in: host specs (CPU model, RAM, OS, kernel), Rust git SHA,
> Python `resource-tracker` version, output table from `compare.py`,
> and observations on where the two implementations agree and diverge.

---

### Remaining known differences

| Aspect                         | Python                                           | Rust                                                    | Fix / investigation path                                             |
|--------------------------------|--------------------------------------------------|---------------------------------------------------------|----------------------------------------------------------------------|
| Timestamp precision            | Float (sub-second)                               | Integer (Unix seconds)                                  | Use ±0.5 s tolerance when aligning comparison rows                   |
| Disk I/O sector size           | Per-device from `/sys/block/<dev>/queue/hw_sector_size`, fallback 512 | Hard-coded 512 bytes/sector                | Read `/sys/block/<dev>/queue/logical_block_size` at startup; multiply delta sectors by actual sector size |
| Disk space: non-`/dev/` mounts | `psutil` includes overlay/tmpfs/cgroup mounts if reported non-virtual | Only `/dev/<device>` prefixed sources in `/proc/mounts` | Low impact on physical hosts; notable on container/VM hosts          |

### JSON superset fields (not in Python CSV)

The JSON output carries richer data than any Python CSV column can express.

| Type      | Description                                                              | Field                          |
|-----------|--------------------------------------------------------------------------|--------------------------------|
| cpu       | Per-logical-core utilization percentage                                  | `cpu.per_core_pct[]`           |
| cpu       | Fractional cores consumed by a tracked PID tree                          | `cpu.process_cores_used`       |
| cpu       | Live descendants under tracked PID                                       | `cpu.process_child_count`      |
| memory    | Total installed RAM                                                      | `memory.total_kib`             |
| memory    | `MemAvailable` — free + reclaimable (more useful than `MemFree` for headroom estimates) | `memory.available_kib` |
| memory    | RAM usage as a percentage                                                | `memory.used_pct`              |
| memory    | Swap total, used, percentage                                             | `memory.swap_*`                |
| network   | Interface name, MAC, driver, operstate, speed, MTU                       | `network[].interface`          |
| disk      | NVMe / SSD / HDD classification                                          | `disk[].device_type`           |
| disk      | Raw device capacity                                                      | `disk[].capacity_bytes`        |
| disk      | Per-mount-point space (total/used/available/pct)                         | `disk[].mounts[]`              |
| gpu       | GPU identity fields                                                      | `gpu[].uuid/name/device_type`  |
| gpu       | Die temperature                                                          | `gpu[].temperature_celsius`    |
| gpu       | Power draw                                                               | `gpu[].power_watts`            |
| gpu       | Core clock                                                               | `gpu[].frequency_mhz`          |
| gpu       | Total VRAM                                                               | `gpu[].vram_total_bytes`       |
