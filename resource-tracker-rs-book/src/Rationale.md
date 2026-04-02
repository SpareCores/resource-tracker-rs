# Rationale

`resource-tracker-rs` is a Rust rewrite of the Python
[resource-tracker](https://github.com/SpareCores/resource-tracker) library.
It preserves full CSV column parity with the Python implementation while adding
new capabilities that are difficult or impossible to express in the original.

---

## Why Rust

| Property | Python `resource-tracker` | `resource-tracker-rs` |
|---|---|---|
| Runtime dependency | Python interpreter + `psutil` | Single static binary |
| Startup overhead | ~200-500 ms | < 5 ms |
| Observer CPU overhead | ~0.5-1% per core | < 0.1% per core |
| Memory footprint | ~30-60 MiB (interpreter) | ~2-4 MiB |
| Deployment | pip / uv install | Copy binary |

The lower observer overhead matters when tracking short-lived or
CPU-intensive workloads where the tracker itself would otherwise appear
in the numbers it is collecting.

---

## New user-facing functionality

### Shell-wrapper mode

```sh
./resource-tracker-rs --interval 1 -- python train.py --epochs 50
```

Pass any command after `--` and the tracker spawns it, sets `--pid`
automatically, emits one final sample on exit, and forwards the child's
exit code. This eliminates the two-process boilerplate
(`tracker & child; wait`) and makes the tracker transparent to CI systems
and schedulers that check exit codes.

### Full process tree tracking (`--pid`)

Python's `SystemTracker` attributes CPU ticks only to the root process.
Rust walks the full `/proc` tree and sums every descendant (workers,
threads, MPI ranks, Spark executors) under the given root PID. Two fields
appear in every JSON sample when `--pid` is active:

- `cpu.process_cores_used` -- fractional cores consumed by the whole tree
- `cpu.process_child_count` -- live descendant count at each sample

### Sentinel API streaming and S3 upload

When `SENTINEL_API_TOKEN` is set, the tracker registers the run, streams
gzip-compressed CSV batches to S3 every `TRACKER_UPLOAD_INTERVAL` seconds
(default 60), and posts a `finish_run` call on clean exit. No network
connections are made when the token is absent.

### TOML config file + environment variable overrides

All settings (interval, job name, PID, metadata) can be persisted in a
`resource-tracker-rs.toml` file alongside the job definition. Every field
also has a `TRACKER_*` environment variable override, which is convenient
for containerized or CI environments where config files are impractical.

---

## Richer metrics (JSON superset)

The CSV output matches Python column-for-column. The JSON output carries
additional fields not expressible as Python CSV scalars.

### CPU

- `per_core_pct[]` -- per-logical-core utilization; identifies hot cores
  and NUMA imbalance
- `utilization_pct` expressed as **fractional cores** (0.0..N_cores),
  not a percentage clamped to 100; more useful on multi-core hosts

### Memory

- `available_mib` (`MemAvailable`) -- free + reclaimable; a more reliable
  headroom estimate than `free_mib` on systems with large page caches
- `swap_total_mib`, `swap_used_mib`, `swap_used_pct` -- swap pressure
  visible before OOM; Python omits swap entirely
- `active_mib` / `inactive_mib` -- distinguish working-set pressure from
  cold cache

### Disk

- Per-device, per-mount detail instead of three aggregated scalars;
  enables per-volume capacity tracking and per-device I/O attribution
- `device_type` (`nvme`, `ssd`, `hdd`), `model`, `vendor`, `serial` --
  correlate metrics with physical hardware without a separate `lsblk` call
- Per-device hardware sector size read from sysfs; correct byte counts on
  4K-native drives where a hard-coded 512 would under-count I/O by 8x

### Network

- Per-interval rates (`rx_bytes_per_sec`, `tx_bytes_per_sec`) in addition
  to cumulative totals; no client-side diff required
- `driver`, `operstate`, `speed_mbps`, `mtu` per interface; identify which
  NIC is under load and whether the link is running at full negotiated speed

### GPU (NVIDIA and AMD)

Python emits no GPU metrics at all. Rust supports both NVIDIA (NVML) and
AMD (ROCm/AMDGPU) accelerators via runtime dynamic loading, with no
build-time driver dependencies. Additional fields beyond utilization and VRAM:

- `temperature_celsius` -- detect thermal throttling in real time
- `power_watts` -- power-efficiency analysis; watts-per-FLOP budgeting
- `frequency_mhz` -- confirm boost clock is active; correlate with thermal
  state
- `uuid`, `name`, `host_id` -- attribute metrics to specific devices in
  multi-GPU systems
