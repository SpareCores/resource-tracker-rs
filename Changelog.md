# Changelog

## [Unreleased]

### Python reference alignment (2026-04-01)

#### `src/sentinel/mod.rs` -- API base URL
- Corrected `DEFAULT_API_BASE` from `https://sentinel.sparecores.com` to
  `https://api.sentinel.sparecores.net` (matches `sentinel_api.py`).

#### `src/sentinel/run.rs` -- endpoint paths, payload shape, status values, encoding
- `start_run` payload: changed from nested `{metadata:{...}, host:{...}, cloud:{...}}`
  to flat dict using `#[serde(flatten)]` on all three fields (matches Python
  `register_run` which merges all dicts at the top level).
- `refresh_credentials` endpoint: `/runs/{id}/credentials/refresh` →
  `/runs/{id}/refresh-credentials`.
- `close_run` endpoint: `/runs/{id}/close` → `/runs/{id}/finish`.
- `run_status` values: `"success"`/`"failure"`/`"unknown"` →
  `"finished"`/`"failed"` (matches Python `RunStatus` enum).
- `DataSource::Local` renamed to `DataSource::Inline`; serde value `"local"` →
  `"inline"` (matches Python `DataSource.inline`).
- `data_csv` encoding: inline fallback now gzip-compresses then base64-encodes the
  CSV before sending (matches Python `b64encode(data_csv)`).
- `RawCredentials` field names corrected to `access_key`, `secret_key`,
  `session_token` (matches live API response); `expiration` made
  `Option<String>` with `#[serde(alias = "expires_at")]` so missing or
  differently-named fields fall back to `"2099-01-01T00:00:00Z"` instead of
  aborting.
- Parse error messages no longer include the raw response body; replaced with
  byte-count only (`{N} bytes`) to prevent STS credentials leaking to stderr.

### Phase 5 -- Remaining Work (2026-04-01)

#### P-S3-CONTENT-ENCODING: `Content-Encoding: gzip` added to S3 PUT (`src/sentinel/s3.rs`)
- Added `.header("Content-Encoding", "gzip")` to the `s3_put_to` call chain.
- Extended T-S3-06 (`s3_put_to_mock_server_returns_uri`) to capture the raw
  request bytes from the mock TCP server via `mpsc::channel` and assert that
  `content-encoding: gzip` is present (case-insensitive).

#### P-S3-BACKOFF: Exponential backoff for S3 upload retry (`src/sentinel/upload.rs`)
- Replaced the single flat 2s retry with two retries: retry 1 after 2s, retry 2
  after 4s (Section 9.2.2: "retry at least once with exponential back-off").
- Error message now includes `retry1:` / `retry2:` labels for log readability.

#### Release-build warnings eliminated (`src/main.rs`, `src/config.rs`, `src/sentinel/`)
- `handle_sigterm as libc::sighandler_t` -- added intermediate `*const ()` cast to
  silence `function_casts_as_integer` lint (compiler-suggested fix).
- Removed unused `pub const DEFAULT_UPLOAD_TIMEOUT_SECS` from `config.rs`.
- Removed unused `request_shutdown` method from `BatchUploader`; callers already
  hold the `Arc<AtomicBool>` via `shutdown_flag()`.
- Removed unused `pub use` re-exports (`refresh_credentials`, `UploadCredentials`,
  `SampleBuffer`) from `sentinel/mod.rs`.
- Release build now compiles with zero warnings.

#### P-TEST-SMOKE: Missing spec tests added (`tests/smoke.rs`, `src/collector/cpu.rs`)

Binary-level integration tests (19 new in `tests/smoke.rs`):
- T-CPU-03: `process_cores_used` and `process_child_count` are null without `--pid`
- T-CPU-04: `process_cores_used >= 0` when `--pid <self>` is supplied
- T-MEM-01: `free_mib + used_mib + buffers_mib + cached_mib <= total_mib`
- T-MEM-02: `used_pct` in [0.0, 100.0]
- T-MEM-03: `swap_used_pct == 0.0` when `swap_total_mib == 0` (skipped if swap present)
- T-MEM-04: `available_mib <= total_mib`
- T-NET-01: `rx_bytes_per_sec >= 0` and `tx_bytes_per_sec >= 0` per interface
- T-NET-02: `rx_bytes_total` non-decreasing across two consecutive samples
- T-NET-03: loopback `lo` absent from network array
- T-DSK-01: `read_bytes_per_sec >= 0` and `write_bytes_per_sec >= 0` per device
- T-DSK-02: `used_bytes + available_bytes <= total_bytes` per mount
- T-DSK-03: `capacity_bytes > 0` when present
- T-GPU-01: `gpu` array empty on CPU-only host (skipped when GPU device detected)
- T-OUT-02: `timestamp_secs` is a positive integer
- T-OUT-03: `resource-tracker-rs-version` is a semver string
- T-CLD-01: first sample arrives within 5s on a non-cloud host
- T-CFG-04: TOML `interval_secs = 2` controls sample spacing (~4s for 2 samples)
- T-CFG-05: CLI `--interval 2` overrides TOML `interval_secs = 5` (2 samples in < 8s)
- T-CFG-06: nonexistent TOML config path silently falls back to defaults
- T-EOR-01: SIGTERM causes the binary to exit with code 0

CSV integration tests (6 new in `tests/smoke.rs`):
- `csv_disk_io_bytes_nonneg`: `disk_read_bytes` and `disk_write_bytes` parse as u64
- `csv_net_bytes_nonneg`: `net_recv_bytes` and `net_sent_bytes` parse as u64
- `csv_disk_space_invariant`: `disk_space_used_gb + disk_space_free_gb <= disk_space_total_gb`
- `csv_memory_fields_nonneg`: all six memory columns parse as non-negative u64
- `csv_cpu_time_fields_nonneg`: `utime >= 0` and `stime >= 0`
- `csv_gpu_fields_nonneg`: `gpu_usage >= 0`, `gpu_vram >= 0`, `gpu_utilized` parses

Unit test (1 new in `src/collector/cpu.rs`):
- T-CPU-06: first `collect()` returns 0.0 for all delta fields
  (`utilization_pct`, `per_core_pct`, `utime_secs`, `stime_secs`)

#### P-DSK-SECTOR: Per-device sector size for disk I/O accounting (`src/collector/disk.rs`)
- Added `sector_size: u32` to `DeviceInfo`.
- `read_device_info` reads `/sys/block/<dev>/queue/hw_sector_size`; falls back to 512.
- `collect()` uses per-device `sector_size` for `read_bytes_per_sec`,
  `write_bytes_per_sec`, `read_bytes_total`, and `write_bytes_total`.
  Capacity bytes still use the fixed 512 (kernel reports `/sys/block/<dev>/size`
  in 512-byte logical sectors regardless of physical sector size).
- `sector_size` stored as `u32` so `f64::from(sector_size)` and
  `u64::from(sector_size)` avoid `as` casts (per project convention).
- Two new unit tests: `T-DSK-SECTOR` (`sector_size_4k_gives_8x_bytes`) and
  `sector_size_fallback_is_512`.

---

### Priority 4 -- Sentinel API Streaming: tests and spec fixes (2026-04-01)

#### Spec corrections (`resource-tracker-rs-book/src/Specification.md`)
- T-CSV-03: corrected stale formula `utilization_pct / 100 × total_cores` to
  `utilization_pct` directly; field is already fractional cores (0..N_cores).
  Confirmed by PR #1 Changelog entry.
- Column table: updated `cpu_usage` computation note to match code.
- Memory column entries: updated field names and units from `*_kib / KiB`
  to `*_mib / MiB` to match the rename made in Priority 1.

#### `src/output/csv.rs` -- T-CSV-01 through T-CSV-06
- `csv_header_is_first_line_no_embedded_newline` (T-CSV-01)
- `csv_row_column_count_matches_header` (T-CSV-02)
- `csv_cpu_usage_is_utilization_pct_direct` (T-CSV-03): annotated stale spec formula
- `csv_disk_space_used_equals_total_minus_free` (T-CSV-04)
- `csv_output_is_deterministic` (T-CSV-05)
- `csv_no_trailing_commas_no_quoted_fields` (T-CSV-06)

#### `src/sentinel/upload.rs` -- T-STR-02 + completeness check
- `gzip_compress_decompresses_to_valid_csv` (T-STR-02): verifies gzip magic bytes,
  round-trip decompression, header as first line, and per-row column count.
- `samples_to_csv_all_lines_end_with_newline`: every line (header and data) ends `\n`.
- Fixed call site: `region_cache.get_or_detect(&bucket, &agent)` corrected to
  `region_cache.get_or_detect(&bucket)` after `RegionCache` API was updated.

#### `src/sentinel/run.rs` -- T-EOR-02, T-EOR-03, T-EOR-04
- `close_run_request_contains_run_id` (T-EOR-02)
- `close_run_data_source_local_when_no_uploads` (T-EOR-03)
- `close_run_data_source_s3_when_uploads_present` (T-EOR-04)

#### `src/sentinel/mod.rs` -- T-STR-01
- `no_token_returns_none` (T-STR-01): `from_env()` returns `None` without token.
- `empty_token_returns_none`: empty-string token also returns `None`.

#### `src/sentinel/s3.rs` -- bug fix
- Added `use std::io::{Read, Write};` in test module (was missing `Read`).
- Corrected `epoch_to_utc_known_date` test: timestamp `1_743_510_896` was 2025-04-01,
  not 2026-04-01; corrected to `1_775_046_896`.

---

### Priority 3 -- Host and Cloud Discovery (2026-04-01)

#### `HostInfo` and `CloudInfo` structs added (`src/metrics/host.rs`)
- `HostInfo` holds all Section 8.1 fields: `host_id`, `host_name`, `host_ip`,
  `host_allocation`, `host_vcpus`, `host_cpu_model`, `host_memory_mib`,
  `host_gpu_model`, `host_gpu_count`, `host_gpu_vram_mib`, `host_storage_gb`.
- `CloudInfo` holds all Section 8.2 fields: `cloud_vendor_id`, `cloud_account_id`,
  `cloud_region_id`, `cloud_zone_id`, `cloud_instance_type`.
- Both structs derive `Default`; all fields are `Option<_>` so collection
  failure is silently swallowed.

#### Host discovery (`src/collector/host.rs`)
- `collect_host_info(gpus)` collects local host metadata synchronously at startup.
  - `host_id`: tries `/sys/class/dmi/id/board_asset_tag` (AWS), falls back to `/etc/machine-id`.
  - `host_name`: `gethostname(3)` via `libc`.
  - `host_ip`: first non-loopback IPv4 from `getifaddrs(3)` via `libc` (unsafe block).
  - `host_allocation`: `None` (heuristic TBD per spec).
  - `host_vcpus` / `host_cpu_model`: parsed from `/proc/cpuinfo` in a single pass.
  - `host_memory_mib`: `MemTotal` KiB from `/proc/meminfo` divided by 1024.
  - GPU fields derived from the GPU Vec passed in (avoids re-querying the driver).
  - `host_storage_gb`: sums 512-byte sectors from `/sys/block/*/size` for all
    non-loop, non-ram block devices.

#### Cloud discovery (`src/collector/host.rs`)
- `spawn_cloud_discovery()` spawns a background thread calling `probe_cloud()`.
- `probe_cloud()` launches three parallel sub-threads (AWS, GCP, Azure), each
  with a ≤ 2-second `timeout_global` configured via `ureq::config::Config`.
- AWS probe: GET `169.254.169.254/latest/meta-data/`; if successful, fetches
  `region`, `availability-zone`, `instance-type`, and `AccountId` from the
  identity credentials endpoint.
- GCP probe: GET `metadata.google.internal/computeMetadata/v1/` with
  `Metadata-Flavor: Google` header.
- Azure probe: GET `169.254.169.254/metadata/instance?api-version=2021-02-01`
  with `Metadata: true` header.
- On a non-cloud host all probes fail fast (no route to host) and return
  `CloudInfo::default()`; satisfies T-CLD-01 (no startup hang > 5s).

#### Startup integration (`src/main.rs`)
- GPU info collected once before warm-up so GPU-derived host fields are populated.
- `collect_host_info` called synchronously (fast, no network).
- `spawn_cloud_discovery()` called before the warm-up sleep; joined after the
  sleep so cloud probes run concurrently with the first sampling interval.
- `host_info` and `cloud_info` are bound and available for the Sentinel API
  registration (Priority 4); currently a no-op `let _ = (&host_info, &cloud_info)`.

#### Compare test fixes (`tests/compare.rs`)
- Added `py_scale: f64` to `ColSpec` to handle Python-KiB vs Rust-MiB unit
  difference for all memory columns (`KIB_TO_MIB = 1.0/1024.0`).
- Changed I/O byte columns to `use_median: true` to suppress single-interval
  burst spikes that inflate percentage error on near-zero readings.
- Increased `disk_write_bytes` tolerance from 10% to 20% (kernel write-back
  timing is a legitimate source of divergence between simultaneous collectors).

---

### Priority 1 -- Spec deviations fixed (2026-04-01)

#### `--interval 0` now rejected (`config.rs`)
- `Config::load()` checks the resolved interval after merging CLI/TOML/defaults.
- If the value is 0, the binary prints an error to stderr and exits with code 1.
- Satisfies test T-CFG-03.

#### `utilization_pct` changed to fractional cores, clamp removed (`collector/cpu.rs`, `metrics/cpu.rs`)
- Renamed internal helper `utilization_pct()` to `core_util_pct()` (used for per-core entries, still 0.0-100.0 with clamp).
- Added `aggregate_util_cores()` which computes `(delta_total - delta_idle) / delta_total * n_cores` with no clamp.
- `CpuMetrics.utilization_pct` now expresses fractional cores in use (0.0..N_cores), not a percentage.
- Matches daroczig's review: "the number of vCPUs fully utilized" is more useful than a percentage clamped to 100.

#### `total_cores` removed from `CpuMetrics` (`metrics/cpu.rs`, `collector/cpu.rs`)
- `total_cores` is a static host property; moved to host discovery (Section 8.1, `host_vcpus`), not yet implemented.
- Per-core array length still implicitly carries the core count via `per_core_pct.len()`.
- `CpuMetrics` gained `#[derive(Default)]`.

#### Memory fields renamed from KiB to MiB (`metrics/memory.rs`, `collector/memory.rs`, `output/csv.rs`)
- All `*_kib` field names renamed to `*_mib` (e.g. `free_kib` -> `free_mib`).
- Division factor changed from `/ 1024` to `/ 1_048_576` in the collector.
- CSV row builder updated to reference the new `_mib` fields.
- Standardized to match Python resource-tracker PR #9 which also adopted MiB.
- `MemoryMetrics` gained `#[derive(Default)]`.

#### `cpu_usage` CSV formula updated (`output/csv.rs`)
- Was: `utilization_pct / 100.0 * total_cores`
- Now: `utilization_pct` directly (field is already in fractional cores).

#### `.expect()` panics replaced with graceful fallbacks (`main.rs`)
- All five collector calls (`cpu`, `memory`, `network`, `disk`, `gpu`) now use `.unwrap_or_default()`.
- JSON serialization failure is caught with a `match` and logged to stderr instead of panicking.
- Satisfies the spec requirement: the binary MUST NEVER panic in production.

---

### Tests for Priority 1 and 2 + version bump to 0.1.1 (2026-04-01)

#### Version bump (`Cargo.toml`)
- Bumped version from `0.1.0` to `0.1.1`.

#### Unit tests added (`src/collector/cpu.rs`)
- Extracted `util_pct_from_ticks(prev_total, prev_idle, curr_total, curr_idle)` -- a pure
  function with no `CpuTime` dependency -- so tick-math is testable without constructing
  procfs types that have private fields.
- Six unit tests covering: all-idle, fully-busy, half-busy, no-delta, no-clamp on aggregate,
  and clamping behavior for per-core values.

#### Integration tests (`tests/smoke.rs`)
- Fixed broken tests that referenced removed/renamed fields (`total_cores`, `*_kib`).
- `T-CFG-03`: `interval_zero_exits_nonzero` -- verifies `--interval 0` exits non-zero.
- `T-CPU-01`: `json_utilization_pct_is_fractional_cores_not_percentage` -- value is in
  `[0, N_cores * 1.05]`, not clamped to 100.
- `T-CPU-02`: `json_total_cores_field_absent` -- `cpu.total_cores` must not appear in JSON.
- `json_memory_fields_are_mib` -- all `*_mib` fields present with sane values (128..10M MiB).
- `json_memory_kib_fields_absent` -- old `*_kib` fields must be absent.
- `csv_cpu_usage_is_fractional_cores` -- `cpu_usage` in CSV is in `[0, N_cores]`, uses
  `num_cpus` dev-dependency to get the real core count for the bound check.
- `csv_values_parse_and_are_sane` -- updated memory column assertions to reflect MiB scale.
- `shell_wrapper_propagates_exit_zero` / `_exit_nonzero` -- wrapper mode exit codes.
- `shell_wrapper_emits_json_samples` -- emits valid JSON while monitoring a child.
- `all_metadata_flags_accepted` -- all Section 9.3 flags accepted without error.
- `tracker_env_vars_accepted` -- all `TRACKER_*` env vars accepted without error.
- `tag_flag_repeatable` -- `--tag` accepted multiple times.

#### Updated (`tests/compare.rs`)
- Corrected `ColSpec` description strings from "KiB" to "MiB" for all memory columns.

#### `as` casts replaced with `try_from` where `From` is applicable (`src/collector/cpu.rs`, `src/output/csv.rs`)
- `count() as u32` and `.len() as u32` replaced with `u32::try_from(...).unwrap_or(0)`.
- Remaining `as f64` casts on `u64`/`usize` are kept: `From<u64> for f64` and
  `From<usize> for f64` are not in std (both conversions are lossy).

#### Dev dependency added (`Cargo.toml`)
- `num_cpus = "1"` added under `[dev-dependencies]` for use in smoke tests.

---

### Priority 2 -- Missing CLI flags and shell-wrapper mode (2026-04-01)

#### Section 9.3 metadata flags added (`config.rs`, `Cargo.toml`)
- Added `env` feature to clap to enable `TRACKER_*` environment variable support.
- Added all metadata fields from Section 9.3 of the spec as CLI flags with `env` attributes:
  `--project-name` / `TRACKER_PROJECT_NAME`, `--stage-name` / `TRACKER_STAGE_NAME`,
  `--task-name` / `TRACKER_TASK_NAME`, `--team` / `TRACKER_TEAM`,
  `--env` / `TRACKER_ENV`, `--language` / `TRACKER_LANGUAGE`,
  `--orchestrator` / `TRACKER_ORCHESTRATOR`, `--executor` / `TRACKER_EXECUTOR`,
  `--external-run-id` / `TRACKER_EXTERNAL_RUN_ID`,
  `--container-image` / `TRACKER_CONTAINER_IMAGE`.
- Added repeatable `--tag KEY=VALUE` flag for arbitrary key-value tags (stored as `Vec<String>`).
- `--job-name` / `TRACKER_JOB_NAME` already existed; moved into the new `JobMetadata` struct.
- New `JobMetadata` struct on `Config` holds all Section 9.3 fields; ready for Sentinel API (Priority 4).

#### Shell-wrapper mode (`main.rs`, `config.rs`)
- Added `command: Vec<String>` trailing positional arg to `Cli` (`trailing_var_arg = true`).
- When a command is present, `main.rs` spawns it via `std::process::Command`, sets `config.pid`
  to the child's PID (overriding any explicit `--pid`), and polls with `child.try_wait()` after
  each interval.
- When the child exits, the tracker emits one final sample then exits with the child's exit code.
- Spawn failure prints an error to stderr and exits with code 1.
- Note: explicit SIGTERM forwarding is a future enhancement; Ctrl-C (SIGINT) naturally reaches
  both processes via the shared process group.
