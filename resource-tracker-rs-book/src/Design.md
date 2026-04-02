# resource-tracker-rs — Design Notes

## Spec Summary

1. Linux resource tracker (x86 + ARM), using `procfs` where appropriate
2. Configurable polling interval for: CPU, memory, GPU, VRAM, network in/out, disk read/write
3. GPU support requires dynamic linking (no static link)
4. CLI tool with optional params (job name/metadata); TOML config file with sane defaults
5. Basic HTTP client: hit API endpoints at start, stop, and every X minutes (heartbeat)
6. Lightweight S3 PUT using AWS creds to stream resource utilization data

---

## Dependency Assessment

### Current `Cargo.toml` dependencies

| Crate             | Version                                                      | Purpose                                                      |
|-------------------|--------------------------------------------------------------|--------------------------------------------------------------|
| `nvml-wrapper`    | 0.12                                                         | NVIDIA GPU/VRAM monitoring via NVML; runtime dynamic loading |
| `libamdgpu_top`   | 0.11.2, no defaults, libdrm\_dynamic\_loading                | AMD GPU monitoring via libdrm; runtime dynamic loading       |
| `clap`            | 4, no defaults, derive+std+help+usage+error-context+env      | CLI argument parsing, minimal footprint                      |
| `procfs`          | 0.18, serde feature only                                     | Linux `/proc` -- CPU, memory, network, disk                  |
| `ureq`            | 3, json feature                                              | Lightweight sync HTTP -- no tokio, no async runtime          |
| `serde`           | 1, derive                                                    | Serialization/deserialization                                |
| `serde_json`      | 1                                                            | JSON payload encoding for API and S3                         |
| `toml`            | 1.0, no defaults, parse+serde features                       | TOML config file parsing                                     |
| `hmac`            | 0.13.0-rc.6                                                  | AWS Signature V4 HMAC signing                                |
| `sha2`            | 0.11.0                                                       | SHA-256 hashing for AWS Sig V4                               |
| `hex`             | 0.4                                                          | Hex encoding for AWS Sig V4 signature                        |
| `libc`            | 0.2                                                          | `statvfs` for filesystem space, `gethostname`, SIGTERM       |
| `flate2`          | =1.1.9 (pinned), no defaults, rust\_backend                  | Gzip compression for S3 batch uploads; pure Rust, no zlib-sys|

### Release profile

```toml
[profile.release]
opt-level = "z"      # optimize for size
lto = true           # link-time optimization
codegen-units = 1    # better dead-code elimination
strip = true         # strip symbols
panic = "abort"      # smaller panic handler
```

### Key decisions

- **`nvml-wrapper` + `libamdgpu_top` over `all-smi`**: `all-smi` required `protoc` at build time. Replaced with `nvml-wrapper` (NVIDIA, no build-time deps) and `libamdgpu_top` with `libdrm_dynamic_loading` (AMD, runtime-only). Both load their respective drivers at runtime and degrade gracefully when absent.
- **`ureq` over `reqwest`**: `reqwest` v0.13 pulls in `tokio` (full async runtime), `hyper`, and TLS stacks -- adds ~5-10 MB. `ureq` v3 is synchronous, no runtime, comparable API surface.
- **`procfs` features trimmed**: Dropped `chrono` (heavy date/time lib, `std::time` suffices) and `flate2` (only needed for gzip-compressed `/proc` files, which are rare).
- **`clap` defaults disabled**: Default clap features include terminal color, unicode width, etc. Stripped to the functional minimum; `env` feature added to support `TRACKER_*` environment variable overrides.
- **Manual AWS Sig V4** (`hmac` + `sha2` + `hex`): Avoids `aws-sdk-s3` (~50+ transitive deps, large binary). S3 PUT only needs ~100-150 lines of signing logic.
- **`toml` v1.0 defaults disabled**: `parse` + `serde` features; `serde` feature required for `toml::from_str` deserialization into config structs.
- **`flate2` pinned to `=1.1.9` with `rust_backend`**: Pure Rust gzip implementation; avoids a `zlib-sys` C build dependency. Version pinned to prevent unexpected breakage from pre-1.0 semver.
- **`libc` for sysfs/POSIX calls**: `statvfs` for filesystem space, `gethostname` for host identity, and `SIGTERM` signal handling -- pure FFI bindings with no additional binary size overhead.

---

## Implementation Approaches

### Option A — Single-file polling loop

All logic in `main.rs`. One tight loop: sleep → collect → diff deltas → buffer → flush.

```
main.rs
 ├── CLI parsing (clap)
 ├── Config loading (toml)
 ├── Polling loop
 │    ├── procfs → CPU/mem/net/disk snapshots + delta computation
 │    ├── all-smi → GPU/VRAM snapshots
 │    └── Vec<Sample> batch buffer
 ├── HTTP calls (ureq) — start / stop / heartbeat
 └── AWS Sig V4 signing + ureq PUT (inline)
```

**Pros:**
- Simplest to read and audit end-to-end
- Zero abstraction overhead
- Fastest to prototype

**Cons:**
- `main.rs` grows large and hard to navigate
- No isolation between collectors — hard to unit test
- Tight coupling makes it hard to disable/swap individual collectors

**Best for:** MVP / proof of concept.

---

### Option B — Module-per-resource + collector trait *(current)*

A `Collector` trait drives a scheduler. Each resource lives in its own module with its own delta state.

```
src/
 ├── main.rs            — CLI, config, scheduler loop
 ├── config.rs          — TOML config struct + CLI override merge
 ├── sample.rs          — Sample / Report structs (serde)
 ├── collector/
 │    ├── mod.rs        — Collector trait: fn collect(&mut self) -> Metric
 │    ├── cpu.rs        — procfs::CpuTime, delta between ticks
 │    ├── memory.rs     — procfs::Meminfo
 │    ├── network.rs    — procfs::Net, bytes delta
 │    ├── disk.rs       — procfs::DiskStats, read/write delta
 │    └── gpu.rs        — all-smi wrapper
 └── reporter/
      ├── mod.rs        — Reporter trait: fn report(&self, batch: &[Sample])
      ├── http.rs       — ureq: start/stop/heartbeat endpoints
      └── s3.rs         — AWS Sig V4 + ureq PUT (batch upload)
```

**Collector trait sketch:**
```rust
pub trait Collector {
    fn collect(&mut self) -> Metric;
}
```

**Reporter trait sketch:**
```rust
pub trait Reporter {
    fn on_start(&self, meta: &JobMeta);
    fn on_sample(&self, batch: &[Sample]);
    fn on_stop(&self, meta: &JobMeta);
}
```

**Pros:**
- Each collector independently testable with mock `/proc` data
- Clean ownership: delta state lives inside each collector struct
- Easy to add/remove resources without touching other collectors
- Reporter abstraction allows multiple outputs (HTTP + S3 simultaneously)

**Cons:**
- Slightly more upfront boilerplate (trait definitions, module layout)
- Minor indirection vs. inline code

**Best for:** Production implementation. Right level of structure for the spec.

---

### Option C — Config-driven pipeline with Cargo feature flags

Extends Option B with `#[cfg(feature = "...")]` gates. GPU collector is behind `feature = "gpu"` since it requires dynamic linking. This enables a statically-linked build for non-GPU targets.

```toml
[features]
default = ["gpu", "s3", "http"]
gpu     = ["dep:all-smi"]
s3      = []
http    = []
```

```
src/
 ├── main.rs
 ├── config.rs
 ├── sample.rs
 ├── collector/
 │    ├── cpu.rs
 │    ├── memory.rs
 │    ├── network.rs
 │    ├── disk.rs
 │    └── gpu.rs          — #[cfg(feature = "gpu")]
 └── reporter/
      ├── http.rs         — #[cfg(feature = "http")]
      └── s3.rs           — #[cfg(feature = "s3")]
```

**Build variants:**
```sh
# Full build (default)
cargo build --release

# No GPU — allows static linking (musl target)
cargo build --release --no-default-features --features http,s3
cargo build --release --target x86_64-unknown-linux-musl --no-default-features --features http,s3

# Minimal — metrics only, no reporting
cargo build --release --no-default-features
```

**Pros:**
- Truly minimal binary for constrained/embedded/container targets
- Static linking possible when GPU excluded
- Clean separation of optional functionality

**Cons:**
- `#[cfg(...)]` gates add noise throughout the code
- More complex CI/build matrix (multiple feature combinations to test)
- Premature if targets are homogeneous

**Best for:** Distributing to heterogeneous environments — e.g., some hosts have GPUs, some don't; or when a stripped container image is a requirement.

---

## Status

**Implement Option B first.** This provides the right structure for the spec without over-engineering. The `Collector` and `Reporter` traits give clean boundaries for testing and future extension.

Option C's feature-flag layer can be added on top of B later with minimal refactoring; the module boundaries are already in place.

### Implementation order (Option B)

1. `config.rs` — TOML struct + CLI merge (clap + toml)
2. `sample.rs` — data model (serde + serde_json)
3. `collector/cpu.rs`, `memory.rs`, `network.rs`, `disk.rs` — procfs collectors
4. `collector/gpu.rs` — all-smi wrapper
5. `reporter/http.rs` — ureq start/stop/heartbeat
6. `reporter/s3.rs` — AWS Sig V4 + ureq PUT
7. `main.rs` — wire scheduler loop
