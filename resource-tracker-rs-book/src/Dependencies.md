# Project Dependencies

This is a [Rust programming language](https://rust-lang.org/) project requiring the [Rust toolchain](https://rust-lang.org/tools/install/), including the Rust build system and package manager, named `cargo`.

In addition to the base toolchain, this project also makes use of the following:

| Tool                                          | Description                                           | Rationale                                                 |
|-----------------------------------------------|-------------------------------------------------------|-----------------------------------------------------------|
| [uv](https://docs.astral.sh/uv/)              | An extremely fast Python package and project manager  | Solely for benchmarking against the Python implementation |
| [just](https://just.systems/man/en/)          | A handy way to save and run project-specific commands | Convenience                                               |
| [jq](https://jqlang.org/)                     | A handy way to slice and filter JSON output           | Convenience tool for JSON and JSONL.                      |
| [mdbook](https://rust-lang.github.io/mdBook/) | A tool to create books with Markdown.                 | This project is documented via mdbook.                    |

## Rust Crate Dependencies

Dependencies are declared in `Cargo.toml` and managed by `cargo`.

### Runtime dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `nvml-wrapper` | 0.12 | NVIDIA GPU monitoring via NVML; loaded at runtime with `libloading` -- no build-time system deps; returns empty on non-NVIDIA hosts |
| `clap` | 4 | CLI argument parsing; stripped to `derive`, `std`, `help`, `usage`, `error-context`, `env` features only |
| `procfs` | 0.18 | Linux `/proc` parsing for CPU, memory, disk, and network metrics |
| `ureq` | 3 | Lightweight synchronous HTTP client for Sentinel API and S3 PUT; avoids tokio runtime overhead |
| `serde` | 1 | Serialization/deserialization framework with `derive` macros |
| `serde_json` | 1 | JSON serialization for metric output and API payloads |
| `toml` | 1.0 | TOML config file parsing; `parse` + `serde` features only, no `display` overhead |
| `hmac` | 0.13.0-rc.6 | HMAC-SHA256 for manual AWS Signature Version 4 signing of S3 PUT requests |
| `sha2` | 0.11.0 | SHA-256 hashing required by AWS Sig V4; paired with `hmac` |
| `hex` | 0.4 | Hex encoding of HMAC digests for Sig V4 canonical request construction |
| `libc` | 0.2 | FFI bindings for `statvfs` (filesystem space), `gethostname`, and `SIGTERM` signal handling |
| `flate2` | 1.1.9 (pinned) | Gzip compression for `.csv.gz` S3 batch uploads; `rust_backend` feature uses pure Rust (no `zlib-sys` C dep) |
| `libamdgpu_top` | 0.11.2 | AMD GPU monitoring via `libdrm`; `libdrm_dynamic_loading` feature loads the library at runtime -- gracefully skipped on non-AMD hosts |

### Dev dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `num_cpus` | 1 | Smoke tests: verifies `cpu.utilization_pct` is expressed as fractional cores (bounded by logical CPU count), not a percentage |



