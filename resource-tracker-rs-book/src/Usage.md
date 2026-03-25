# resource-tracker-rs — Usage Guide

`resource-tracker-rs` is a lightweight Linux resource tracker. It polls CPU, memory,
disk, network, and GPU metrics at a configurable interval and emits
newline-delimited JSON (JSONL) to stdout.

---

## Quick start

```sh
# Build
cargo build --release

# Run with defaults (5-second interval, no job label)
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
  "cpu": { "total_cores": 16, "utilization_pct": 42.1, "per_core_pct": [...], "process_cores_used": 3.8, "process_child_count": 4 },
  "memory": { "total_kib": 65536000, "used_kib": 31200000, "used_pct": 47.6, ... },
  "network": [{ "interface": "eth0", "rx_bytes_per_sec": 1200.0, "tx_bytes_per_sec": 400.0, ... }],
  "disk": [{ "device": "nvme0n1", "model": "Samsung SSD 990 PRO", "read_bytes_per_sec": 0.0, ... }],
  "gpu": [{ "name": "NVIDIA GeForce RTX 4090", "utilization_pct": 98.0, "vram_used_pct": 72.3, ... }]
}
```

---

## CLI flags

| Flag              | Short | Default           | Description                                                                           |
|-------------------|-------|-------------------|---------------------------------------------------------------------------------------|
| `--job-name NAME` | `-n`  | _(none)_          | Label attached to every sample. Useful for identifying runs in aggregated output.     |
| `--pid PID`       | `-p`  | _(none)_          | Root PID of the process tree to attribute CPU usage to. Includes all child processes. |
| `--interval SECS` | `-i`  | `5`               | How often to emit a sample, in seconds.                                               |
| `--config FILE`   | `-c`  | `sparecores.toml` | Path to a TOML config file. Silently ignored if the file does not exist.              |
| `--help`          | `-h`  |                   | Print help.                                                                           |
| `--version`       | `-V`  |                   | Print version.                                                                        |

**Precedence:** CLI flags > config file > built-in defaults.

---

## Config file (`resource-tracker-rs.toml`)

The TOML config file lets you persist settings so you don't have to repeat CLI
flags on every invocation. It is optional — the tool works with no config file
at all. Any field set on the CLI overrides the corresponding field in the file.

The default lookup path is `sparecores.toml` in the current working directory.
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
# Default: 5
interval_secs = 10
```

### Minimal example — system-wide monitoring

```toml
[tracker]
interval_secs = 30
```

### Example — named job with process tracking

```toml
[job]
name    = "my_job_i_want_to_track"
pid     = 98231

[tracker]
interval_secs = 5
```

---

## When to use the config file vs CLI flags

| Situation                                 | Recommended approach                                 |
|-------------------------------------------|------------------------------------------------------|
| One-off interactive run                   | CLI flags — faster, no file to manage                |
| Recurring job (cron, SLURM, systemd unit) | TOML file alongside the job definition               |
| CI / benchmark pipeline                   | TOML file checked into the repository                |
| Multiple named jobs on the same host      | One TOML file per job, point to it with `--config`   |
| Containerized workload                    | Set config via CLI flags in the `CMD` / `ENTRYPOINT` |

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

# Extract only CPU utilisation over time
./resource-tracker-rs | jq '{ t: .timestamp_secs, cpu: .cpu.utilization_pct }'

# Watch GPU VRAM usage
./resource-tracker-rs --interval 1 | jq '.gpu[] | { name, vram_used_pct }'
```

---

## Process tree tracking (`--pid`)

When `--pid` is set, every sample includes two extra fields under `cpu`:

- `process_cores_used` — fractional cores consumed by the process tree
  (e.g. `3.8` means the tree is using the equivalent of 3.8 full cores).
- `process_child_count` — number of live child/descendant processes at the
  time of sampling (does not include the root PID itself).

If the tracked PID exits during a run, its contribution drops to zero and
`process_child_count` drops to zero. The tracker itself keeps running.

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

GPUs are detected automatically at startup via `all-smi`. No configuration is
needed. On hosts without GPU hardware or without the relevant driver libraries
installed (NVIDIA NVML, AMD ROCm), the `gpu` array in each sample will be
empty — the tracker continues running normally.

Supported accelerators: NVIDIA GPUs (NVML), AMD GPUs (ROCm/AMDGPU),
Google TPUs, Intel Gaudi, Furiosa NPUs, Tenstorrent accelerators.

---

## Metrics reference

### `cpu`
| Field                 | Unit   | Description                                                      |
|-----------------------|--------|------------------------------------------------------------------|
| `total_cores`         | count  | Total logical CPUs visible to the OS                             |
| `utilization_pct`     | %      | Aggregate utilisation across all cores                           |
| `per_core_pct`        | % each | Per-logical-core utilisation array                               |
| `process_cores_used`  | cores  | Fractional cores used by tracked process tree (`null` if no PID) |
| `process_child_count` | count  | Live descendant processes (`null` if no PID)                     |

### `memory`
All values in **kibibytes (KiB = 1024 bytes)**.

| Field            | Description                             |
|------------------|-----------------------------------------|
| `total_kib`      | Total installed RAM                     |
| `available_kib`  | Immediately reclaimable by applications |
| `used_kib`       | `total_kib - available_kib`             |
| `used_pct`       | Fraction of total RAM in use            |
| `buffers_kib`    | Kernel I/O buffer cache                 |
| `cached_kib`     | Page cache (file data cached in RAM)    |
| `swap_total_kib` | Total swap space (0 if no swap)         |
| `swap_used_kib`  | Used swap                               |
| `swap_used_pct`  | Fraction of swap in use                 |

### `disk` (one entry per whole-disk block device)
| Field                      | Unit    | Description                                 |
|----------------------------|---------|---------------------------------------------|
| `device`                   | —       | Kernel device name, e.g. `nvme0n1`, `sda`   |
| `model`                    | —       | Drive model string from `/sys/block/`       |
| `vendor`                   | —       | Vendor string from `/sys/block/`            |
| `serial`                   | —       | Serial number or WWID                       |
| `device_type`              | —       | `nvme`, `ssd`, or `hdd`                     |
| `capacity_bytes`           | bytes   | Raw device capacity                         |
| `mounts`                   | —       | Array of mounted filesystems on this device |
| `mounts[].mount_point`     | —       | e.g. `/`, `/home`                           |
| `mounts[].filesystem`      | —       | e.g. `ext4`, `xfs`, `btrfs`                 |
| `mounts[].total_bytes`     | bytes   | Filesystem total size                       |
| `mounts[].used_bytes`      | bytes   | Space in use                                |
| `mounts[].available_bytes` | bytes   | Space available to non-root users           |
| `mounts[].used_pct`        | %       | Fraction of filesystem in use               |
| `read_bytes_per_sec`       | bytes/s | Disk read throughput                        |
| `write_bytes_per_sec`      | bytes/s | Disk write throughput                       |

### `network` (one entry per non-loopback interface)
| Field              | Unit    | Description                                  |
|--------------------|---------|----------------------------------------------|
| `interface`        | —       | Interface name, e.g. `eth0`, `ens3`          |
| `mac_address`      | —       | Hardware MAC address                         |
| `driver`           | —       | Kernel driver name, e.g. `igc`, `virtio_net` |
| `operstate`        | —       | Link state: `up`, `down`, `unknown`          |
| `speed_mbps`       | Mbps    | Negotiated link speed (-1 if not reported)   |
| `mtu`              | bytes   | Maximum transmission unit                    |
| `rx_bytes_per_sec` | bytes/s | Received throughput                          |
| `tx_bytes_per_sec` | bytes/s | Transmitted throughput                       |

### `gpu` (one entry per detected accelerator)
| Field                 | Unit  | Description                                                           |
|-----------------------|-------|-----------------------------------------------------------------------|
| `uuid`                | —     | Vendor-assigned device UUID                                           |
| `name`                | —     | Device name, e.g. `NVIDIA GeForce RTX 4090`                           |
| `device_type`         | —     | `GPU`, `NPU`, `TPU`, etc.                                             |
| `host_id`             | —     | Host-level device identifier (PCIe slot or platform index)            |
| `detail`              | —     | Driver-specific key/value map (PCI IDs, ASIC name, driver version, …) |
| `utilization_pct`     | %     | Core utilisation                                                      |
| `vram_total_bytes`    | bytes | Total VRAM                                                            |
| `vram_used_bytes`     | bytes | Used VRAM                                                             |
| `vram_used_pct`       | %     | Fraction of VRAM in use                                               |
| `temperature_celsius` | °C    | Die temperature                                                       |
| `power_watts`         | W     | Power draw                                                            |
| `frequency_mhz`       | MHz   | Core clock                                                            |
| `core_count`          | count | Shader/compute cores (`null` if not reported)                         |
