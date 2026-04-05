# Specification Proposal — `resource-tracker-rs`

 - Status: Proposal / Work-in-Progress
 - Date: 2026-03-30
 - Based on:  README.md (SpareCores), `src/` prototype, Python PR #9, `s3_upload.py`
 - AI large language model tools were used throughout research, specification, and implementation phases of this project to accelerate and improve the quality of the work.

---

## 0. Conventions

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHOULD**,
**SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this document are
to be interpreted as described in [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119).

A **verifiable requirement** is one that can be confirmed by an automated test
without manual inspection.  Every normative statement below (MUST/SHALL) is
intended to be verifiable.

---

## 1. Purpose and Scope

`resource-tracker-rs` is a lightweight, statically self-contained Linux binary that:

1. Polls system- and process-level resource utilization at a configurable interval.
2. Emits structured samples to stdout (JSON Lines or CSV).
3. Optionally streams those samples to the <u>Sentinel API</u> (SpareCores data
   ingestion endpoint) via gzip-compressed (CVS, TSV, or JSONL) files uploaded to S3 using
   temporary STS credentials.

The binary is intended as a drop-in CLI wrapper: run it alongside any process
and it will transparently record how that process consumes hardware.

**Out of scope (v1):** macOS, Windows, eBPF, EBPF-based tracing, container
image introspection beyond environment variables, multi-host federation.

---

## 2. Platform Requirements

| Requirement          | Detail                                                                                               |
|----------------------|------------------------------------------------------------------------------------------------------|
| Operating System     | Linux only (kernel ≥ 4.18 recommended for full `/proc` coverage)                                     |
| CPU Architectures    | x86_64 and aarch64 (ARM64)                                                                           |
| Linkage              | Dynamic linkage for GPU libraries; all other code statically linked or carried as crate dependencies |
| Minimum Rust Edition | 2024                                                                                                 |

GPU support MUST NOT be required for the binary to build or run.  
On a CPU-only host `GpuCollector::collect()` SHALL return an empty `Vec` and no error.

---

## 3. Configuration

### 3.1 Precedence (highest to lowest)

```
CLI flags  >  TOML config file  >  built-in defaults
```

> **Future enhancement:** Support `RESOURCE_TRACKER_`-prefixed environment
> variables (e.g. `RESOURCE_TRACKER_INTERVAL`, `RESOURCE_TRACKER_FORMAT`) as
> an additional configuration layer between CLI flags and the TOML file.
> Environment variables are more practical than file-based config for
> containerized and scripted workloads and are preferred for the Sentinel
> integration use case.

### 3.2 CLI Parameters

The binary MUST accept the following flags via a command line parser:

| Short | Long               | Type     | Default                    | Description                                             |
|-------|--------------------|----------|----------------------------|---------------------------------------------------------|
| `-n`  | `--job-name`       | `String` | none                       | Human-readable label attached to every sample           |
| `-p`  | `--pid`            | `i32`    | none                       | Root PID of the process tree to track (CPU attribution) |
| `-i`  | `--interval`       | `u64`    | `1`                        | Polling interval in seconds (≥ 1)                       |
| `-c`  | `--config`         | path     | `resource-tracker-rs.toml` | Path to TOML config file                                |
| `-f`  | `--format`         | enum     | `json`                     | Output format: `json` or `csv`                          |
|       | `--version`        | flag     |                            | Print binary version and exit                           |

All metadata fields listed in Section 9.3 (job_name, project_name, stage_name, etc.)
MUST also be accepted as CLI flags.  See Section 9.3 for the full flag and environment
variable table.

**Shell-wrapper mode (MVP target):** The binary SHOULD support being used as a
transparent process wrapper, where the command to monitor is passed as trailing
arguments after a `--` separator or as positional arguments:

```shell
resource-tracker-rs Rscript model.R
resource-tracker-rs -- python train.py --epochs 10
```

In this mode the binary spawns the given command as a child process, sets
`--pid` to that child's PID automatically, and exits when the child exits
(propagating the child exit code).  This is a significant usability improvement
over the Python implementation and is a first-class v1 goal.

`--interval` MUST be > 0. Values of 0 SHALL be rejected with a non-zero exit code and a descriptive error message.


### 3.3 TOML Config File

The config file is optional.  If the file does not exist or cannot be parsed,
the binary MUST continue using defaults (no error, no warning).

Schema:

```toml
[job]
name = "my-benchmark"   # String; optional
pid  = 12345            # i32;   optional

[tracker]
interval_secs = 5       # u64;   optional; default 1
```

Unrecognized keys MUST be silently ignored.

### 3.4 Verifiable Configuration Tests

- `T-CFG-01`: Running with no flags produces valid JSON Lines output on stdout.
- `T-CFG-02`: `--format csv` emits a header line matching the exact column list in Section 6.2 before the first data row.
- `T-CFG-03`: `--interval 0` exits with code ≠ 0.
- `T-CFG-04`: A TOML file with `[tracker] interval_secs = 3` results in
  samples separated by ≈ 3 seconds when no `--interval` flag is provided.
- `T-CFG-05`: A CLI `--interval 2` overrides a TOML `interval_secs = 5`.
- `T-CFG-06`: A missing TOML file path silently falls back to defaults.

---

## 4. Startup Behavior

On startup the binary MUST:

1. Parse configuration (Section 3).
2. Initialize all collectors.
3. Execute one warm-up collection pass to prime delta state in stateful collectors (`CpuCollector`, `NetworkCollector`, `DiskCollector`).
4. Sleep exactly one full interval.
5. Emit the CSV header (if format = CSV) <u>before</u> the first data row.
6. Enter the polling loop (Section 5).

The warm-up pass result MUST NOT be emitted to stdout.

---

## 5. Polling Loop

The loop MUST:

1. Record `timestamp_secs` = current Unix time as `u64` (seconds since UNIX epoch, UTC).
2. Collect all metric subsystems (Section 6.1) in the order: CPU, Memory, Network, Disk, GPU.
3. Serialize and emit one line to stdout per the chosen format (Section 6.2, Section 6.3).
4. Sleep the configured interval.
5. Repeat indefinitely until killed.

Collection of any subsystem MUST NOT block the other subsystems.  Failures in
optional subsystems (GPU) MUST be surfaced as empty/zero values, not panics.

---

## 6. Data Model

### 6.1 Sample

A `Sample` is a point-in-time snapshot of all tracked resources.

```rust
pub struct Sample {
    pub timestamp_secs: u64,          // Unix time (seconds)
    pub job_name:       Option<String>,
    pub cpu:            CpuMetrics,
    pub memory:         MemoryMetrics,
    pub network:        Vec<NetworkMetrics>,  // one per interface
    pub disk:           Vec<DiskMetrics>,     // one per block device
    pub gpu:            Vec<GpuMetrics>,      // one per GPU; empty if none
}
```

#### 6.1.1 CpuMetrics

Source: `/proc/stat` tick deltas; `/proc/<pid>/stat` for process tracking.

> **Note:** `total_cores` (logical CPU count) is a static host property that
> rarely changes.  It belongs in the host discovery snapshot (Section 8.1) rather than
> in every per-second sample.  It is referenced here only for computing
> `cpu_usage` in the CSV output (Section 7.2).

| Field                 | Type          | Unit             | Source                 | Notes                                                              |
|-----------------------|---------------|------------------|------------------------|--------------------------------------------------------------------|
| `utilization_pct`     | `f64`         | fractional cores | `/proc/stat`           | Aggregate utilization expressed as cores-in-use (0.0..N_cores)     |
| `per_core_pct`        | `Vec<f64>`    | %                | `/proc/stat`           | Per logical CPU percentage; len == `host_vcpus`; range 0.0–100.0  |
| `utime_secs`          | `f64`         | seconds          | `/proc/stat`           | Δ(user+nice ticks) / ticks_per_second for this interval            |
| `stime_secs`          | `f64`         | seconds          | `/proc/stat`           | Δ(system ticks) / ticks_per_second for this interval               |
| `process_count`       | `u32`         | count            | `/proc` numeric dirs   | Number of running processes visible to the OS                      |
| `process_cores_used`  | `Option<f64>` | fractional cores | `/proc/<pid>/stat`     | None when no PID tracked                                           |
| `process_child_count` | `Option<u32>` | count            | `/proc/<pid>/stat`     | Descendant count; excludes root PID; None when no PID tracked      |

**Computation rules:**

- `utilization_pct` = `(Δtotal − Δidle) / Δtotal × N_cores` where N_cores is
  the logical CPU count from host discovery.  The result is expressed as
  **fractional cores in use** (e.g. 4.6 on a 16-core host means ~4.6 vCPUs
  are fully utilized).  Do NOT clamp this value; values very slightly above
  N_cores are valid under kernel accounting rounding.
  Δtotal = Δ(user + nice + system + idle + iowait + irq + softirq + steal).
  Δidle = Δ(idle + iowait).
- `utime_secs` = Δ(user + nice) / `ticks_per_second`.
- `stime_secs` = Δ(system) / `ticks_per_second`.
- `process_cores_used` = Σ Δ(utime+stime) for root PID and all descendants /
  (elapsed_wall_clock_seconds × ticks_per_second).  Must be ≥ 0.
- On the first collection call (no previous snapshot), all delta-based fields
  MUST return 0.  The caller MUST discard this result (warm-up pass).

**Verifiable CpuMetrics Tests:**

- `T-CPU-01`: `utilization_pct` is in [0.0, N_cores] for all samples (N_cores from host discovery).
- `T-CPU-02`: `len(per_core_pct)` == `host_vcpus` for all samples.
- `T-CPU-03`: When `--pid` is not set, `process_cores_used` and `process_child_count` are `None`.
- `T-CPU-04`: When `--pid <self>` is set, `process_cores_used` ≥ 0.
- `T-CPU-05`: `process_count` ≥ 1 on any running Linux system.
- `T-CPU-06`: First `collect()` call returns 0.0 for all delta fields.

#### 6.1.2 MemoryMetrics

Source: `/proc/meminfo`.  All values in **mebibytes (MiB = 1024 × 1024 bytes)**,
standardized to match Python `resource-tracker` PR #9 which also adopts MiB
throughout.

| Field            | Type  | Unit | `/proc/meminfo` key(s)                  | Notes                                         |
|------------------|-------|------|-----------------------------------------|-----------------------------------------------|
| `total_mib`      | `u64` | MiB  | `MemTotal`                              |                                               |
| `free_mib`       | `u64` | MiB  | `MemFree`                               | Truly free RAM                                |
| `available_mib`  | `u64` | MiB  | `MemAvailable`                          | Free + reclaimable                            |
| `used_mib`       | `u64` | MiB  | `MemTotal − MemFree − Buffers − Cached` | Matches Python `memory_used`                  |
| `used_pct`       | `f64` | %    | derived                                 | `used_mib / total_mib × 100`; range 0.0–100.0 |
| `buffers_mib`    | `u64` | MiB  | `Buffers`                               | Kernel I/O buffers                            |
| `cached_mib`     | `u64` | MiB  | `Cached + SReclaimable`                 | Page cache + slab reclaimable                 |
| `swap_total_mib` | `u64` | MiB  | `SwapTotal`                             |                                               |
| `swap_used_mib`  | `u64` | MiB  | `SwapTotal − SwapFree`                  |                                               |
| `swap_used_pct`  | `f64` | %    | derived                                 | 0.0 when `SwapTotal` == 0                     |
| `active_mib`     | `u64` | MiB  | `Active`                                |                                               |
| `inactive_mib`   | `u64` | MiB  | `Inactive`                              |                                               |

**Verifiable MemoryMetrics Tests:**

- `T-MEM-01`: `free_mib + used_mib + buffers_mib + cached_mib ≤ total_mib` (accounting for kernel reserved memory).
- `T-MEM-02`: `used_pct` is in [0.0, 100.0].
- `T-MEM-03`: `swap_used_pct` is 0.0 when `swap_total_mib` == 0.
- `T-MEM-04`: `available_mib ≤ total_mib`.

#### 6.1.3 NetworkMetrics

Source: `/proc/net/dev` (throughput), `/sys/class/net/<iface>/` (identity/link state).
One `NetworkMetrics` record per non-loopback interface.

> **Architecture note:** Fields such as `mac_address`, `driver`, `operstate`,
> `speed_mbps`, and `mtu` are static properties that do not change every
> second.  They are candidates for promotion to a host-discovery snapshot
> (Section 8.1) rather than being repeated in every per-second sample.  This
> applies similarly to static fields in Section 6.1.4 (disk) and Section 6.1.5
> (GPU).  The current spec includes them here for completeness; a future
> revision should separate static identity fields from dynamic rate fields.

| Field              | Type             | Unit    | Source                                         | Notes                         |
|--------------------|------------------|---------|------------------------------------------------|-------------------------------|
| `interface`        | `String`         | —       | interface name                                 | e.g. `"eth0"`                 |
| `mac_address`      | `Option<String>` | —       | `/sys/class/net/<iface>/address`               | `"00:11:22:33:44:55"`         |
| `driver`           | `Option<String>` | —       | `/sys/class/net/<iface>/device/driver` symlink | e.g. `"igc"`                  |
| `operstate`        | `Option<String>` | —       | `/sys/class/net/<iface>/operstate`             | `"up"`, `"down"`, `"unknown"` |
| `speed_mbps`       | `Option<i64>`    | Mbps    | `/sys/class/net/<iface>/speed`                 | −1 when not reported          |
| `mtu`              | `Option<u32>`    | bytes   | `/sys/class/net/<iface>/mtu`                   |                               |
| `rx_bytes_per_sec` | `f64`            | bytes/s | `/proc/net/dev` Δ                              | Rate for this interval        |
| `tx_bytes_per_sec` | `f64`            | bytes/s | `/proc/net/dev` Δ                              | Rate for this interval        |
| `rx_bytes_total`   | `u64`            | bytes   | `/proc/net/dev`                                | Cumulative since boot         |
| `tx_bytes_total`   | `u64`            | bytes   | `/proc/net/dev`                                | Cumulative since boot         |

**Verifiable NetworkMetrics Tests:**

- `T-NET-01`: `rx_bytes_per_sec` ≥ 0.0 and `tx_bytes_per_sec` ≥ 0.0 for all interfaces.
- `T-NET-02`: `rx_bytes_total` monotonically non-decreasing between consecutive samples (absent interface reset).
- `T-NET-03`: The loopback interface (`lo`) is NOT included in the output.

#### 6.1.4 DiskMetrics

Source: `/proc/diskstats` (throughput), `/sys/block/<dev>/` (identity),
`statvfs(3)` (space).  One `DiskMetrics` record per block device (excluding
partitions and device-mapper synthetic devices unless mounted independently).

| Field                 | Type                    | Unit    | Source                                     | Notes                      |
|-----------------------|-------------------------|---------|--------------------------------------------|----------------------------|
| `device`              | `String`                | —       | kernel device name                         | e.g. `"sda"`, `"nvme0n1"`  |
| `model`               | `Option<String>`        | —       | `/sys/block/<dev>/device/model`            |                            |
| `vendor`              | `Option<String>`        | —       | `/sys/block/<dev>/device/vendor`           |                            |
| `serial`              | `Option<String>`        | —       | `/sys/block/<dev>/device/wwid` or `serial` |                            |
| `device_type`         | `Option<DiskType>`      | —       | `/sys/block/<dev>/queue/rotational`        | `Nvme`, `Ssd`, or `Hdd`; `None` when type cannot be determined |
| `capacity_bytes`      | `Option<u64>`           | bytes   | `/sys/block/<dev>/size × 512`              |                            |
| `mounts`              | `Vec<DiskMountMetrics>` | —       | `statvfs(3)`                               | One per mount point        |
| `read_bytes_per_sec`  | `f64`                   | bytes/s | `/proc/diskstats` Δ                        |                            |
| `write_bytes_per_sec` | `f64`                   | bytes/s | `/proc/diskstats` Δ                        |                            |
| `read_bytes_total`    | `u64`                   | bytes   | `/proc/diskstats` sectors × sector_size    | Cumulative since boot; see sector size note |
| `write_bytes_total`   | `u64`                   | bytes   | `/proc/diskstats` sectors × sector_size    | Cumulative since boot; see sector size note |

`DiskMountMetrics` fields:

| Field             | Type     | Unit  | Notes                                        |
|-------------------|----------|-------|----------------------------------------------|
| `mount_point`     | `String` | —     | e.g. `"/"`                                   |
| `filesystem`      | `String` | —     | Filesystem type from `/proc/mounts`; e.g. `"ext4"`, `"xfs"` |
| `total_bytes`     | `u64`    | bytes | `statvfs.f_blocks × f_bsize`                 |
| `available_bytes` | `u64`    | bytes | `statvfs.f_bavail × f_bsize` (unprivileged)  |
| `used_bytes`      | `u64`    | bytes | `total_bytes − (statvfs.f_bfree × f_bsize)`  |
| `used_pct`        | `f64`    | %     | `used_bytes / total_bytes × 100`; 0.0 when total == 0 |

> **Sector size note:** The current implementation hard-codes 512 bytes/sector for
> `/proc/diskstats` conversions.  Python's `get_sector_sizes()` reads
> `/sys/block/<dev>/queue/hw_sector_size` (fallback 512).  On 4K-native drives
> (some NVMe) the Rust code will under-count I/O bytes by up to 8×.  A future
> fix should read `/sys/block/<dev>/queue/logical_block_size` at startup and use
> the actual sector size.  See implementation plan P-DSK-SECTOR.

**Verifiable DiskMetrics Tests:**

- `T-DSK-01`: `read_bytes_per_sec` ≥ 0.0 and `write_bytes_per_sec` ≥ 0.0.
- `T-DSK-02`: For each mount, `used_bytes + available_bytes ≤ total_bytes`.
- `T-DSK-03`: `capacity_bytes` (when Some) > 0.

#### 6.1.5 GpuMetrics

Source: NVML (`nvml-wrapper` crate, runtime-loads `libnvidia-ml.so`) for
NVIDIA GPUs; `libamdgpu_top` (runtime-loads `libdrm`) for AMD GPUs.

| Field                 | Type                     | Unit  | Notes                                                             |
|-----------------------|--------------------------|-------|-------------------------------------------------------------------|
| `uuid`                | `String`                 | —     | Stable vendor UUID; AMD uses PCI bus address                      |
| `name`                | `String`                 | —     | Human-readable device name                                        |
| `device_type`         | `String`                 | —     | `"GPU"`, `"NPU"`, `"TPU"`                                         |
| `host_id`             | `String`                 | —     | Host-level device identifier                                      |
| `detail`              | `HashMap<String,String>` | —     | Vendor-specific extras (driver version, PCI bus ID, ROCm version) |
| `utilization_pct`     | `f64`                    | %     | Core utilization; range 0.0–100.0                                 |
| `vram_total_bytes`    | `u64`                    | bytes |                                                                   |
| `vram_used_bytes`     | `u64`                    | bytes |                                                                   |
| `vram_used_pct`       | `f64`                    | %     | `vram_used / vram_total × 100`; 0.0 when total == 0               |
| `temperature_celsius` | `u32`                    | °C    | Die temperature                                                   |
| `power_watts`         | `f64`                    | W     | NVML reports mW; converted to W                                   |
| `frequency_mhz`       | `u32`                    | MHz   | Core/graphics clock                                               |
| `core_count`          | `Option<u32>`            | count | Shader/compute cores; None if not reported                        |

**AMD-specific:** When `/sys/module/amdgpu` does not exist the AMD collection path MUST be skipped entirely (no panic).

**NVIDIA-specific:** `power_watts` = raw NVML milliwatt value / 1000.

**Verifiable GpuMetrics Tests:**

- `T-GPU-01`: On a CPU-only host, `gpu` Vec is empty and no error is returned.
- `T-GPU-02`: `utilization_pct` is in [0.0, 100.0] for each GPU.
- `T-GPU-03`: `vram_used_bytes ≤ vram_total_bytes` for each GPU.
- `T-GPU-04`: `vram_used_pct` is 0.0 when `vram_total_bytes` == 0.
- `T-GPU-05`: On a host with AMD GPU, `uuid` equals the PCI bus address string.

---

## 7. Output Formats

### 7.1 JSON Lines (default)

Each sample is emitted as a single JSON object followed by `\n`.  The binary
MUST include a version field keyed as `"<crate-name>-version"` with the value
being the Cargo package version string.

Example (abbreviated):

```json
{"timestamp_secs":1743300000,"job_name":null,"cpu":{...},"memory":{...},"network":[...],"disk":[...],"gpu":[],"resource-tracker-rs-version":"0.1.0"}
```

Requirements:

- `T-OUT-01`: Each line MUST be valid JSON parseable with any standard JSON library.
- `T-OUT-02`: `timestamp_secs` MUST be present and be a positive integer.
- `T-OUT-03`: The version key `"resource-tracker-rs-version"` MUST be present.
- `T-OUT-04`: Consecutive samples MUST have non-decreasing `timestamp_secs`.

### 7.2 CSV Format

CSV is the **primary and required** output format for Sentinel S3 streaming
(Section 9.2.2).  It uses the same column names and units as the Python
`resource-tracker` so the Sentinel backend can ingest both without schema
changes.  When uploaded to S3 the CSV content MUST be gzip-compressed and the
object key MUST carry the extension `.csv.gz`.

When `--format csv` is selected for stdout output the raw (uncompressed) CSV
bytes are written.  Gzip compression is applied only when writing the S3 batch
upload payload (Section 9.2.2).

When `--format csv` is selected:

- The header line MUST be emitted **exactly once**, before the first data row.
- The header MUST match the following column names in this exact order:

```
timestamp,processes,utime,stime,cpu_usage,memory_free,memory_used,memory_buffers,memory_cached,memory_active,memory_inactive,disk_read_bytes,disk_write_bytes,disk_space_total_gb,disk_space_used_gb,disk_space_free_gb,net_recv_bytes,net_sent_bytes,gpu_usage,gpu_vram,gpu_utilized
```

Column definitions:

| CSV Column            | Source Field          | Unit             | Computation                                                             |
|-----------------------|-----------------------|------------------|-------------------------------------------------------------------------|
| `timestamp`           | `timestamp_secs`      | Unix seconds     | direct                                                                  |
| `processes`           | `cpu.process_count`   | count            | direct                                                                  |
| `utime`               | `cpu.utime_secs`      | seconds          | direct; 3 decimal places                                                |
| `stime`               | `cpu.stime_secs`      | seconds          | direct; 3 decimal places                                                |
| `cpu_usage`           | `cpu.utilization_pct` | fractional cores | `utilization_pct` directly; field is already in fractional cores (0..N_cores); 4 decimal places |
| `memory_free`         | `memory.free_mib`     | MiB              | direct                                                                  |
| `memory_used`         | `memory.used_mib`     | MiB              | direct                                                                  |
| `memory_buffers`      | `memory.buffers_mib`  | MiB              | direct                                                                  |
| `memory_cached`       | `memory.cached_mib`   | MiB              | direct                                                                  |
| `memory_active`       | `memory.active_mib`   | MiB              | direct                                                                  |
| `memory_inactive`     | `memory.inactive_mib` | MiB              | direct                                                                  |
| `disk_read_bytes`     | disk subsystem        | bytes            | Σ `read_bytes_per_sec × interval_secs` across all devices; integer      |
| `disk_write_bytes`    | disk subsystem        | bytes            | Σ `write_bytes_per_sec × interval_secs` across all devices; integer     |
| `disk_space_total_gb` | disk mounts           | GB (10⁹)         | Σ `total_bytes / 1_000_000_000` across all mounts; 6 decimal places     |
| `disk_space_used_gb`  | disk mounts           | GB (10⁹)         | `disk_space_total_gb − disk_space_free_gb`; 6 decimal places            |
| `disk_space_free_gb`  | disk mounts           | GB (10⁹)         | Σ `available_bytes / 1_000_000_000` across all mounts; 6 decimal places |
| `net_recv_bytes`      | network subsystem     | bytes            | Σ `rx_bytes_per_sec × interval_secs` across all interfaces; integer     |
| `net_sent_bytes`      | network subsystem     | bytes            | Σ `tx_bytes_per_sec × interval_secs` across all interfaces; integer     |
| `gpu_usage`           | gpu subsystem         | fractional GPUs  | Σ `utilization_pct / 100` across all GPUs; 4 decimal places             |
| `gpu_vram`            | gpu subsystem         | MiB              | Σ `vram_used_bytes / 1_048_576`; 4 decimal places                       |
| `gpu_utilized`        | gpu subsystem         | count            | count of GPUs where `utilization_pct > 0.0`                             |

**Verifiable CSV Tests:**

- `T-CSV-01`: Header is emitted exactly once, as the first line.
- `T-CSV-02`: Column count per data row equals column count in header.
- `T-CSV-03`: `cpu_usage` column equals `utilization_pct` directly (field is already fractional cores, 0..N_cores) to 4 dp.
- `T-CSV-04`: `disk_space_used_gb = disk_space_total_gb − disk_space_free_gb` for all rows.
- `T-CSV-05`: CSV output for a given sample is byte-for-byte reproducible (deterministic).
- `T-CSV-06`: No trailing commas; no quoted fields (all values are numbers or bare identifiers).

---

## 8. Host and Cloud Discovery

The binary SHOULD collect machine-level metadata once at startup and include it
in the Sentinel API registration payload (Section 9.1).  Collected fields use the prefix `host_` or `cloud_`.

### 8.1 Host Discovery

All fields are optional; collection failure MUST be silently swallowed.

| Field               | Type             | Source                                                                |
|---------------------|------------------|-----------------------------------------------------------------------|
| `host_id`           | `Option<String>` | AWS: `/sys/class/dmi/id/board_asset_tag`; fallback: `/etc/machine-id` |
| `host_name`         | `Option<String>` | `gethostname(3)`                                                      |
| `host_ip`           | `Option<String>` | First non-loopback IPv4 from `getifaddrs(3)`                          |
| `host_allocation`   | `Option<String>` | `"dedicated"` or `"shared"`; heuristic TBD                            |
| `host_vcpus`        | `Option<u32>`    | Count of logical CPUs (`/proc/cpuinfo` processor entries)             |
| `host_cpu_model`    | `Option<String>` | `/proc/cpuinfo` `model name` field                                    |
| `host_memory_mib`   | `Option<u64>`    | `MemTotal / 1024` from `/proc/meminfo`                                |
| `host_gpu_model`    | `Option<String>` | First GPU name from `GpuCollector`                                    |
| `host_gpu_count`    | `Option<u32>`    | Length of GPU Vec                                                     |
| `host_gpu_vram_mib` | `Option<u64>`    | Sum of `vram_total_bytes / 1_048_576` across all GPUs                 |
| `host_storage_gb`   | `Option<f64>`    | Sum of `capacity_bytes / 1_000_000_000` across all block devices      |

Users MUST be able to suppress any field by setting the corresponding
environment variable to `"0"` or `""` (exact mechanism TBD in implementation).

### 8.2 Cloud Discovery

Cloud metadata is probed by making HTTP GET requests to each cloud provider's
Instance Metadata Service (IMDS) with a short timeout (≤ 2 seconds per
provider).  Probes MUST be attempted in the background and MUST NOT delay
the first sample emission.

| Field                 | Probe endpoint                                                                                                        | Notes                                     |
|-----------------------|-----------------------------------------------------------------------------------------------------------------------|-------------------------------------------|
| `cloud_vendor_id`     | AWS: `169.254.169.254/latest/meta-data/`; GCP: `metadata.google.internal`; Azure: `169.254.169.254/metadata/instance` | Infer vendor from which endpoint responds |
| `cloud_account_id`    | AWS: `/latest/meta-data/identity-credentials/ec2/info`                                                                |                                           |
| `cloud_region_id`     | AWS: `/latest/meta-data/placement/region`                                                                             |                                           |
| `cloud_zone_id`       | AWS: `/latest/meta-data/placement/availability-zone`                                                                  |                                           |
| `cloud_instance_type` | AWS: `/latest/meta-data/instance-type`                                                                                |                                           |

**Verifiable Cloud Discovery Tests:**

- `T-CLD-01`: On a non-cloud host, all `cloud_*` fields are `None` and the binary does not hang for more than 5 seconds total on startup.
- `T-CLD-02`: IMDS probe timeout is ≤ 2 seconds per provider.

---

## 9. Sentinel API Streaming (Extra Component)

Activation is gated on the `SENTINEL_API_TOKEN` environment variable being set.

> **Resolved design decisions:**
> 1. Streaming is enabled automatically whenever `SENTINEL_API_TOKEN` is set; no additional flag needed.
> 2. Upload format is `csv.gz` only; `jsonl.gz` is not supported.
> 3. Streaming is not separately configurable via TOML or CLI beyond the token env var.
> 4. On network unavailability: `start_run` logs a warning and disables streaming; local stdout output continues normally (see Section 11 error handling).


### 9.1 Authentication

The binary MUST read the API token from the environment variable
`SENTINEL_API_TOKEN`.  Every Sentinel API request MUST include the HTTP header:

```
Authorization: Bearer <token>
```

If `SENTINEL_API_TOKEN` is not set, all streaming functionality MUST be silently disabled.  Local stdout emission continues normally.

### 9.2 Run Lifecycle

#### 9.2.1 Start of Run

At startup (after host/cloud discovery), the binary MUST POST to the data ingestion endpoint to register a new Run.

POST `/runs` (default base URL: `https://api.sentinel.sparecores.net`).

Request payload (JSON, Content-Type: `application/json`): all metadata, host, and
cloud fields are merged into a **flat** top-level object (no nesting):

```json
{
  "job_name": "...",
  "project_name": "...",
  "pid": 12345,
  "host_vcpus": 8,
  "cloud_vendor_id": "aws",
  ...
}
```

Response fields the binary MUST store:

| Response Field                      | Type                | Usage                                  |
|-------------------------------------|---------------------|----------------------------------------|
| `run_id`                            | `String`            | Referenced in all subsequent API calls |
| `upload_uri_prefix`                 | `String`            | S3 URI prefix for metric uploads       |
| `upload_credentials.access_key`     | `String`            | STS credential                         |
| `upload_credentials.secret_key`     | `String`            | STS credential                         |
| `upload_credentials.session_token`  | `String`            | STS credential                         |
| `upload_credentials.expiration`     | `String` (ISO 8601) | STS credential expiry; optional        |

#### 9.2.2 Batch Upload (Background Thread)

The binary MUST start a background thread that:

1. Every **60 seconds** (configurable, default 60), takes all samples collected since the previous upload.
2. Serializes them as CSV (same column layout as Section 7.2) -- CSV is the only accepted format for the Sentinel S3 bucket.
3. Gzip-compresses the CSV bytes.
4. Generates a unique S3 object key under `upload_uri_prefix`:
   `<upload_uri_prefix>/<run_id>/<batch_seq_number>.csv.gz`
5. Uploads via AWS Signature V4 (Section 10).
6. Appends the uploaded URI to an internal list `uploaded_uris`.

If STS credentials are within **5 minutes** of `expiration`, the binary MUST refresh
them by POSTing to `/runs/{run_id}/refresh-credentials` before attempting the upload.

Upload failures MUST be retried at least once with exponential back-off before
being recorded as errors.  After 3 consecutive upload failures the background
thread MUST log a warning and continue buffering (data is not lost).

**Verifiable Streaming Tests:**

- `T-STR-01`: Without `SENTINEL_API_TOKEN`, no HTTP connection is made.
- `T-STR-02`: A batch upload request contains `Content-Encoding: gzip` and the body decompresses to valid CSV or JSONL.
- `T-STR-03`: `uploaded_uris` contains the S3 URIs of all successfully uploaded batches.
- `T-STR-04`: Credential refresh is triggered when ≤ 5 minutes remain before expires_at.

#### 9.2.3 End of Run

When the tracked process terminates (or the binary receives SIGTERM), the binary MUST:

> **SIGINT note:** An explicit SIGINT handler is not installed.  When the binary
> is used in shell-wrapper mode, Ctrl-C is delivered to the entire process group,
> so both the child and the tracker receive SIGINT and exit together.  Explicit
> SIGTERM forwarding to the child process is a future enhancement.

1. Flush any remaining samples as a final batch upload (if `uploaded_uris` is non-empty).
2. POST to `/runs/{run_id}/finish` to close the Run, including:
   - `run_id`
   - `exit_code` (i32, if tracked process exited cleanly; else None)
   - `run_status` enum: `"finished"` (exit 0 or SIGTERM) or `"failed"` (non-zero exit)
   - `data_source`:
     - `"s3"` + `data_uris: Vec<String>` if any S3 uploads succeeded.
     - `"inline"` + `data_csv: <base64(gzip(csv))>` for short runs with no S3 uploads.

**Verifiable End-of-Run Tests:**

- `T-EOR-01`: On SIGTERM, the binary exits with code 0 after flushing remaining data.
- `T-EOR-02`: The close-run request body contains `run_id` matching the start-run response.
- `T-EOR-03`: `data_source` is `"inline"` when no S3 uploads occurred.
- `T-EOR-04`: `data_source` is `"s3"` when at least one S3 upload succeeded.

### 9.3 Metadata Fields

The following metadata MAY be supplied by the user via CLI flags or environment
variables.  All are optional strings unless noted.

| Field             | CLI Flag            | Env Variable              |
|-------------------|---------------------|---------------------------|
| `job_name`        | `--job-name`        | `TRACKER_JOB_NAME`        |
| `project_name`    | `--project-name`    | `TRACKER_PROJECT_NAME`    |
| `stage_name`      | `--stage-name`      | `TRACKER_STAGE_NAME`      |
| `task_name`       | `--task-name`       | `TRACKER_TASK_NAME`       |
| `team`            | `--team`            | `TRACKER_TEAM`            |
| `env`             | `--env`             | `TRACKER_ENV`             |
| `language`        | `--language`        | `TRACKER_LANGUAGE`        |
| `orchestrator`    | `--orchestrator`    | `TRACKER_ORCHESTRATOR`    |
| `executor`        | `--executor`        | `TRACKER_EXECUTOR`        |
| `external_run_id` | `--external-run-id` | `TRACKER_EXTERNAL_RUN_ID` |
| `container_image` | `--container-image` | `TRACKER_CONTAINER_IMAGE` |

Users MUST also be able to supply arbitrary key-value tags via repeated `--tag key=value` flags.

---

## 10. S3 Upload — AWS Signature V4

The upload is implemented in pure Rust **without any AWS SDK dependency** (zero
additional transitive deps for this path).  The implementation mirrors the
Python `s3_upload.py` module from PR #9.

### 10.1 URI Parsing

An S3 URI has the form `s3://bucket/path/to/object`.  Parsing MUST:

- Require scheme == `"s3"`.
- Require a non-empty bucket name.
- Require a non-empty key (path after bucket).
- Return an error for any other form.

### 10.2 Bucket Region Detection

If the upload region is not supplied, the binary MUST determine it by sending
an HTTP HEAD request to `https://<bucket>.s3.amazonaws.com/` and reading the
`x-amz-bucket-region` response header.  The header is present even on 3xx/4xx
responses.  Results MUST be cached in-process for the lifetime of the run.
Default fallback: `"eu-central-1"`.

### 10.3 Request Construction

A PUT request to `https://<bucket>.s3.<region>.amazonaws.com/<key>` with:

- `Content-Length`: byte count of body.
- `x-amz-content-sha256`: SHA-256 hex of body.
- `x-amz-date`: `YYYYMMDDTHHmmSSZ` UTC.
- `x-amz-security-token`: STS session token.
- `Authorization`: AWS4-HMAC-SHA256 signature (see Section 10.4).

### 10.4 AWS Signature V4

Signing key derivation:

```
kDate    = HMAC-SHA256("AWS4" + secret_key, date_stamp)
kRegion  = HMAC-SHA256(kDate, region)
kService = HMAC-SHA256(kRegion, "s3")
kSigning = HMAC-SHA256(kService, "aws4_request")
```

Canonical request:

```
PUT
/<key>

host:<bucket>.s3.<region>.amazonaws.com
x-amz-content-sha256:<payload_hash>
x-amz-date:<amz_date>
x-amz-security-token:<session_token>

host;x-amz-content-sha256;x-amz-date;x-amz-security-token
<payload_hash>
```

String to sign:

```
AWS4-HMAC-SHA256
<amz_date>
<date_stamp>/<region>/s3/aws4_request
<canonical_request_sha256>
```

Authorization header:

```
AWS4-HMAC-SHA256 Credential=<access_key>/<credential_scope>, SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token, Signature=<hex_sig>
```

### 10.5 Upload Success Criteria

HTTP 200 or 201 response from S3 = success.  Any other status = error (with
response body included in the error message).

### 10.6 Verifiable S3 Upload Tests

- `T-S3-01`: `parse_s3_uri("s3://bucket/path/obj")` returns `("bucket", "path/obj")`.
- `T-S3-02`: `parse_s3_uri("https://bucket/path")` returns an error.
- `T-S3-03`: `parse_s3_uri("s3://bucket/")` returns an error (empty key).
- `T-S3-04`: Given known access_key, secret_key, session_token, region, and a
  fixed timestamp, the generated `Authorization` header MUST match a
  pre-computed golden value.
- `T-S3-05`: Bucket region cache prevents duplicate HEAD requests for the same bucket.
- `T-S3-06`: An upload to a mock S3 server returns the S3 URI on success.

---

## 11. Error Handling

| Scenario                                       | Required behavior                                                 |
|------------------------------------------------|-------------------------------------------------------------------|
| `/proc` file is unreadable for a single metric | Return 0 / None for that field; do not abort                      |
| GPU library absent                             | GPU Vec is empty; no error propagated                             |
| Sentinel API unreachable at start              | Log warning; streaming disabled; local output continues           |
| S3 upload fails                                | Retry once; after 3 consecutive failures log warning and continue |
| Config TOML parse error                        | Silently fall back to defaults                                    |
| `--interval 0`                                 | Exit with code ≠ 0 before starting collectors                     |
| Tracked PID not found                          | `process_cores_used` = None; do not abort                         |

The binary MUST NEVER panic in production code.  `expect()` is only permissible during development; 
all `expect()` calls MUST be replaced with proper error handling before v1.0 release.

---

## 12. Non-Functional Requirements

| Requirement                        | Target                                                  |
|------------------------------------|---------------------------------------------------------|
| Binary size                        | < 15 MiB stripped (CPU-only build)                      |
| Startup latency                    | < 1 × configured interval before first sample           |
| CPU overhead of the tracker itself | < 1% of one core at 1-second interval on a 4-core host  |
| Memory footprint                   | < 20 MiB RSS at steady state                            |
| Stdout buffering                   | Each line MUST be flushed atomically (no partial lines) |

---

## 13. Compatibility with Python `resource-tracker`

The CSV output format MUST maintain byte-for-byte column-name compatibility
with the Python `SystemTracker` output so that the Sentinel API backend can
ingest both without schema changes.

Confirmed equivalent columns (see Section 7.2 for derivation):

| Python column         | Rust CSV column       | Python unit      | Rust unit        |
|-----------------------|-----------------------|------------------|------------------|
| `timestamp`           | `timestamp`           | Unix seconds     | Unix seconds     |
| `processes`           | `processes`           | count            | count            |
| `utime`               | `utime`               | seconds          | seconds          |
| `stime`               | `stime`               | seconds          | seconds          |
| `cpu_usage`           | `cpu_usage`           | fractional cores | fractional cores |
| `memory_free`         | `memory_free`         | MiB              | MiB              |
| `memory_used`         | `memory_used`         | MiB              | MiB              |
| `memory_buffers`      | `memory_buffers`      | MiB              | MiB              |
| `memory_cached`       | `memory_cached`       | MiB              | MiB              |
| `memory_active`       | `memory_active`       | MiB              | MiB              |
| `memory_inactive`     | `memory_inactive`     | MiB              | MiB              |
| `disk_read_bytes`     | `disk_read_bytes`     | bytes/interval   | bytes/interval   |
| `disk_write_bytes`    | `disk_write_bytes`    | bytes/interval   | bytes/interval   |
| `disk_space_total_gb` | `disk_space_total_gb` | GB (10⁹)         | GB (10⁹)         |
| `disk_space_used_gb`  | `disk_space_used_gb`  | GB (10⁹)         | GB (10⁹)         |
| `disk_space_free_gb`  | `disk_space_free_gb`  | GB (10⁹)         | GB (10⁹)         |
| `net_recv_bytes`      | `net_recv_bytes`      | bytes/interval   | bytes/interval   |
| `net_sent_bytes`      | `net_sent_bytes`      | bytes/interval   | bytes/interval   |
| `gpu_usage`           | `gpu_usage`           | fractional GPUs  | fractional GPUs  |
| `gpu_vram`            | `gpu_vram`            | MiB              | MiB              |
| `gpu_utilized`        | `gpu_utilized`        | count            | count            |

**Verifiable compatibility test:**

- `T-COMPAT-01`: Run Python and Rust trackers in parallel on the same host for
  60 seconds.  For each interval, the difference between corresponding scalar
  columns MUST be within 5% of the Python value (allowing for measurement-time
  skew).

---

## 14. Open Questions / Future Work

1. **eBPF integration**: Using `aya-rs` or `libbpf-rs` for sub-millisecond
   tracing (CPU saturation, IPC, TLB misses, cache hit rates) — currently
   considered v2.
2. **Process-level memory (PSS)**: Preferred over RSS; requires reading
   `/proc/<pid>/smaps_rollup` which may be slow for large processes.
3. **Per-process disk and network I/O**: `/proc/<pid>/io` and network
   namespaces; currently only system-wide.
4. **Configurable metric suppression**: Allow users to opt out of fields
   containing PII (e.g. `host_ip`, hostname).
5. **ARM-specific GPU support**: Apple Metal not in scope (Linux only);
   Qualcomm Adreno / Mali GPU metrics TBD.
6. **Static linking of NVML**: Currently not possible; NVML requires a
   dynamically loaded vendor library.
7. **Heartbeat endpoint**: Periodic ping to Sentinel API while tracking is
   active (distinct from batch S3 uploads).
