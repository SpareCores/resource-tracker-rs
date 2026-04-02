# resource-tracker-rs -- Usage Guide

`resource-tracker-rs` is a lightweight Linux resource tracker. It polls CPU, memory,
disk, network, and GPU metrics at a configurable interval and emits
newline-delimited JSON (JSONL) to stdout.

---

## Quick start

```sh
# Build
cargo build --release

# Run with defaults (1-second interval)
./target/release/resource-tracker-rs

# Track a specific process tree (replace 1234 with the root PID)
./target/release/resource-tracker-rs --pid 1234 --job-name "my-benchmark"

# Change the polling interval to 10 seconds
./target/release/resource-tracker-rs --interval 10
```

Each line of output is a complete JSON object representing one sample:

```json
{
  "timestamp_secs": 1718000000,
  "job_name": "my-benchmark",
  "cpu": { "utilization_pct": 4.6, "per_core_pct": [12.5, 38.0, "..."], "process_cores_used": 3.8, "process_child_count": 4 },
  "memory": { "total_mib": 64000, "used_mib": 30468, "used_pct": 47.6, "free_mib": 2289, "available_mib": 18432, "buffers_mib": 263, "cached_mib": 8472, "active_mib": 8157, "inactive_mib": 7404, "swap_total_mib": 0, "swap_used_mib": 0, "swap_used_pct": 0.0 },
  "network": [{ "interface": "eth0", "rx_bytes_per_sec": 1200.0, "tx_bytes_per_sec": 400.0, "rx_bytes_total": 9834200, "tx_bytes_total": 312400, "driver": "virtio_net", "operstate": "up", "speed_mbps": 1000, "mtu": 1500, "mac_address": "02:00:00:aa:bb:cc" }],
  "disk": [{ "device": "nvme0n1", "model": "Samsung SSD 990 PRO", "device_type": "nvme", "capacity_bytes": 1000204886016, "read_bytes_per_sec": 0.0, "write_bytes_per_sec": 204800.0, "mounts": [{ "mount_point": "/", "filesystem": "ext4", "total_bytes": 999292796928, "used_bytes": 841676800000, "available_bytes": 142023000000, "used_pct": 84.2 }] }],
  "gpu": [{ "name": "NVIDIA GeForce RTX 4090", "utilization_pct": 98.0, "vram_used_pct": 72.3, "vram_used_bytes": 17394819072, "vram_total_bytes": 24026849280, "temperature_celsius": 74, "power_watts": 318.5, "frequency_mhz": 2520 }]
}
```

---

## CLI flags

| Flag              | Short | Default | Description |
|-------------------|-------|---------|-------------|
| `--job-name NAME` | `-n`  | _(none)_ | Label attached to every sample. Useful for identifying runs in aggregated output. |
| `--pid PID`       | `-p`  | _(none)_ | Root PID of the process tree to attribute CPU usage to. Includes all child processes. |
| `--interval SECS` | `-i`  | `1`     | How often to emit a sample, in seconds. |
| `--format FORMAT` | `-f`  | `json`  | Output format: `json` or `csv`. |
| `--config FILE`   | `-c`  | `resource-tracker-rs.toml` | Path to a TOML config file. Silently ignored if the file does not exist. |
| `--help`          | `-h`  |         | Print help. |
| `--version`       | `-V`  |         | Print version. |

**Precedence:** CLI flags > config file > built-in defaults.

---

## Config file (`resource-tracker-rs.toml`)

The TOML config file lets you persist settings so you don't have to repeat CLI
flags on every invocation. It is optional -- the tool works with no config file
at all. Any field set on the CLI overrides the corresponding field in the file.

The default lookup path is `resource-tracker-rs.toml` in the current working directory.
Use `--config /path/to/file.toml` to point elsewhere.

### Full reference

```toml
[job]
# Human-readable label for this tracking session.
# Appears as "job_name" in every emitted JSON sample.
# Useful when multiple runs are collected into the same data store so you can
# filter and group by job.
name = "gpu-benchmark-run-42"

# Root PID of the process to track.
# resource-tracker-rs will walk the full process tree (parent + all descendants)
# and sum their CPU tick usage to report process_cores_used.
# Leave unset to collect system-wide metrics only.
pid = 12345

[tracker]
# Sampling interval in seconds.  Lower values give finer resolution at the
# cost of more output volume and slightly higher observer overhead.
# Default: 1
interval_secs = 10
```

### Minimal example -- system-wide monitoring

```toml
[tracker]
interval_secs = 30
```

### Example -- named job with process tracking

```toml
[job]
name    = "my_job_i_want_to_track"
pid     = 98231

[tracker]
interval_secs = 5
```

---

## Sentinel API streaming and S3 output

When `SENTINEL_API_TOKEN` is set, the tracker registers the run with the
Sentinel API and streams metric batches to S3 in the background.
No network connections are ever made when the token is absent.

### How it works

1. At startup, `start_run` is called to register the run and obtain temporary
   S3 upload credentials from the Sentinel API.
2. A background upload thread wakes every `TRACKER_UPLOAD_INTERVAL` seconds
   (default 60), drains the in-memory sample buffer, serializes as CSV,
   gzip-compresses, and PUTs the file to the S3 prefix returned by the API.
3. On clean exit (SIGTERM, shell-wrapper child exits), any samples not yet
   uploaded are base64-encoded and sent inline to `finish_run` inside a
   gzip-compressed JSON body.  If S3 uploads did occur, only the S3 URIs
   are sent.

### Environment variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `SENTINEL_API_TOKEN` | Yes | -- | Bearer token for the Sentinel API. Streaming is disabled when absent or empty. |
| `SENTINEL_API_URL` | No | `https://api.sentinel.sparecores.net` | Override the Sentinel API base URL. |
| `TRACKER_UPLOAD_INTERVAL` | No | `60` | Seconds between S3 batch uploads. |

### Job metadata environment variables

All Section 9.3 metadata fields can be set via environment variable instead of
CLI flags.  Environment variables are overridden by the corresponding CLI flag
when both are supplied.

| Variable | CLI flag |
|---|---|
| `TRACKER_JOB_NAME` | `--job-name` |
| `TRACKER_PROJECT_NAME` | `--project-name` |
| `TRACKER_STAGE_NAME` | `--stage-name` |
| `TRACKER_TASK_NAME` | `--task-name` |
| `TRACKER_TEAM` | `--team` |
| `TRACKER_ENV` | `--env` |
| `TRACKER_LANGUAGE` | `--language` |
| `TRACKER_ORCHESTRATOR` | `--orchestrator` |
| `TRACKER_EXECUTOR` | `--executor` |
| `TRACKER_EXTERNAL_RUN_ID` | `--external-run-id` |
| `TRACKER_CONTAINER_IMAGE` | `--container-image` |

### Example

```sh
export SENTINEL_API_TOKEN="your-token-here"
export TRACKER_JOB_NAME="gpu-benchmark"
export TRACKER_UPLOAD_INTERVAL=30

./resource-tracker-rs --interval 1 -- python train.py
```

The tracker spawns `python train.py`, monitors it, uploads a gzip-compressed
CSV batch to S3 every 30 seconds, and calls `finish_run` when the script exits.

---

## When to use the config file vs CLI flags

| Situation | Recommended approach |
|---|---|
| One-off interactive run | CLI flags -- faster, no file to manage |
| Recurring job (cron, SLURM, systemd unit) | TOML file alongside the job definition |
| CI / benchmark pipeline | TOML file checked into the repository |
| Multiple named jobs on the same host | One TOML file per job, point to it with `--config` |
| Containerized workload | Set config via CLI flags in the `CMD` / `ENTRYPOINT` |

---

## Capturing output

Because samples are emitted as newline-delimited JSON to stdout, standard Unix
tools work directly with the output.

```sh
# Write to a file
./resource-tracker-rs > run.jsonl

# Tail live output
./resource-tracker-rs | tee run.jsonl

# Pretty-print with jq
./resource-tracker-rs | jq .

# Extract only CPU utilization over time
./resource-tracker-rs | jq '{ t: .timestamp_secs, cpu: .cpu.utilization_pct }'

# Watch GPU VRAM usage
./resource-tracker-rs --interval 1 | jq '.gpu[] | { name, vram_used_pct }'
```

---

## Shell-wrapper mode

Pass a command after `--` to have the tracker spawn and monitor it:

```sh
./resource-tracker-rs --interval 1 --job-name "training-run" -- python train.py --epochs 50
```

The tracker sets `--pid` automatically to the spawned child's PID, emits one
final sample when the child exits, then exits with the child's exit code.

**Rationale:** eliminates the two-process boilerplate (`tracker & python ...; wait`)
and guarantees the tracker always exits with the job's exit code, making it
transparent to CI systems.

---

## Process tree tracking (`--pid`)

When `--pid` is set, every sample includes two extra fields under `cpu`:

- `process_cores_used` -- fractional cores consumed by the process tree
  (e.g. `3.8` means the tree is using the equivalent of 3.8 full cores).
- `process_child_count` -- number of live child/descendant processes at the
  time of sampling (does not include the root PID itself).

If the tracked PID exits during a run, its contribution drops to zero and
`process_child_count` drops to zero. The tracker itself keeps running.

**Rationale:** Python's `SystemTracker` tracks only the calling process's own
ticks.  Rust walks the full `/proc` tree so multi-process and multi-threaded
workloads (e.g. PyTorch data-loader workers, MPI ranks, Spark executors) are
attributed correctly under a single root PID.

**Finding the PID of a running process:**

```sh
# By name
pgrep -x python

# Most recently launched
pgrep -n my-training-script

# Already know the command? Launch and capture PID
my-training-script &
./resource-tracker-rs --pid $! --job-name "training-run-1"
```

---

## GPU support

GPUs are detected automatically at startup via NVML (NVIDIA) and
`libamdgpu_top` (AMD). No configuration is needed. On hosts without GPU
hardware or without the relevant driver libraries installed, the `gpu` array
in each sample will be empty -- the tracker continues running normally.

Supported accelerators: NVIDIA GPUs (NVML), AMD GPUs (ROCm/AMDGPU).

**Rationale:** per-GPU temperature, power draw, and clock frequency are not
emitted by Python's `SystemTracker`. These fields enable thermal throttle
detection and power-efficiency analysis without a separate monitoring tool.

---

## Metrics reference

### `cpu`

| Field | Unit | Description |
|---|---|---|
| `utilization_pct` | fractional cores | Aggregate cores in use (0.0..N_cores). 4.6 on a 16-core host means ~4.6 vCPUs fully utilized. |
| `per_core_pct` | % each | Per-logical-core utilization array (0.0--100.0). |
| `utime_secs` | seconds | User+nice CPU time across all cores this interval. |
| `stime_secs` | seconds | System CPU time across all cores this interval. |
| `process_count` | count | Runnable processes (`procs_running` from `/proc/stat`). |
| `process_cores_used` | fractional cores | Cores consumed by tracked process tree (`null` if no PID). |
| `process_child_count` | count | Live descendant processes (`null` if no PID). |

### `memory`

All values in **mebibytes (MiB = 1,048,576 bytes)**.

| Field | Description |
|---|---|
| `total_mib` | Total installed RAM |
| `free_mib` | Truly free RAM (`MemFree` from `/proc/meminfo`) |
| `available_mib` | Free + reclaimable RAM (`MemAvailable`); better estimate of headroom |
| `used_mib` | `total - free - buffers - cached` (excludes reclaimable cache) |
| `used_pct` | Fraction of total RAM in use |
| `buffers_mib` | Kernel I/O buffer cache |
| `cached_mib` | Page cache including slab-reclaimable (`Cached + SReclaimable`) |
| `active_mib` | Active pages (recently accessed) |
| `inactive_mib` | Inactive pages (candidates for reclaim) |
| `swap_total_mib` | Total swap space (0 if no swap) |
| `swap_used_mib` | Used swap |
| `swap_used_pct` | Fraction of swap in use |

**Rationale:** Python's `SystemTracker` reports memory in KiB and omits
`available_mib`, `active_mib`, `inactive_mib`, `swap_*`.  Rust reports all
fields in MiB (matching Python resource-tracker PR #9) and adds
`available_mib` (`MemAvailable`) which is a more reliable headroom estimate
than `free_mib` alone on systems with large page caches.

### `disk` (one entry per whole-disk block device)

| Field | Unit | Description |
|---|---|---|
| `device` | -- | Kernel device name, e.g. `nvme0n1`, `sda` |
| `model` | -- | Drive model string from `/sys/block/` |
| `vendor` | -- | Vendor string from `/sys/block/` |
| `serial` | -- | Serial number or WWID |
| `device_type` | -- | `nvme`, `ssd`, or `hdd` |
| `capacity_bytes` | bytes | Raw device capacity |
| `mounts` | -- | Array of mounted filesystems on this device |
| `mounts[].mount_point` | -- | e.g. `/`, `/home` |
| `mounts[].filesystem` | -- | e.g. `ext4`, `xfs`, `btrfs` |
| `mounts[].total_bytes` | bytes | Filesystem total size |
| `mounts[].used_bytes` | bytes | Space in use |
| `mounts[].available_bytes` | bytes | Space available to non-root users |
| `mounts[].used_pct` | % | Fraction of filesystem in use |
| `read_bytes_per_sec` | bytes/s | Disk read throughput |
| `write_bytes_per_sec` | bytes/s | Disk write throughput |
| `read_bytes_total` | bytes | Cumulative bytes read since boot |
| `write_bytes_total` | bytes | Cumulative bytes written since boot |

**Rationale:** Python aggregates disk space across all mounts into three
scalar CSV columns.  Rust retains per-device, per-mount detail in the JSON
output, enabling per-volume capacity tracking and per-device I/O attribution
that the aggregated CSV cannot express.

### `network` (one entry per non-loopback interface)

| Field | Unit | Description |
|---|---|---|
| `interface` | -- | Interface name, e.g. `eth0`, `ens3` |
| `mac_address` | -- | Hardware MAC address |
| `driver` | -- | Kernel driver name, e.g. `igc`, `virtio_net` |
| `operstate` | -- | Link state: `up`, `down`, `unknown` |
| `speed_mbps` | Mbps | Negotiated link speed (-1 if not reported) |
| `mtu` | bytes | Maximum transmission unit |
| `rx_bytes_per_sec` | bytes/s | Received throughput |
| `tx_bytes_per_sec` | bytes/s | Transmitted throughput |
| `rx_bytes_total` | bytes | Cumulative bytes received since boot |
| `tx_bytes_total` | bytes | Cumulative bytes sent since boot |

**Rationale:** Python's `SystemTracker` emits only cumulative rx/tx byte
totals per interface.  Rust adds per-interval rates, driver identity,
link state, negotiated speed, and MTU, enabling network saturation and
driver-level diagnostics without a separate tool.

### `gpu` (one entry per detected accelerator)

| Field | Unit | Description |
|---|---|---|
| `uuid` | -- | Vendor-assigned device UUID |
| `name` | -- | Device name, e.g. `NVIDIA GeForce RTX 4090` |
| `device_type` | -- | `GPU`, `NPU`, `TPU`, etc. |
| `host_id` | -- | Host-level device identifier (PCIe slot or platform index) |
| `detail` | -- | Driver-specific key/value map (PCI IDs, ASIC name, driver version, ...) |
| `utilization_pct` | % | Core utilization |
| `vram_total_bytes` | bytes | Total VRAM |
| `vram_used_bytes` | bytes | Used VRAM |
| `vram_used_pct` | % | Fraction of VRAM in use |
| `temperature_celsius` | deg C | Die temperature |
| `power_watts` | W | Power draw |
| `frequency_mhz` | MHz | Core clock |
| `core_count` | count | Shader/compute cores (`null` if not reported) |
