# Rust Crate-Level Competitive Landscape: Resource Monitoring

This document surveys Rust crates relevant to resource monitoring — tracking CPU, memory, GPU, network, and disk utilization — with particular focus on use cases analogous to the Python [`resource-tracker`](https://github.com/SpareCores/resource-tracker) package (batch job wrapping, structured output, low overhead).

It also covers `dial9-tokio-telemetry`, a notable 2026 Rust telemetry crate that is *not* a resource monitor but is included here to explain why it falls outside this landscape.

---

## Section 1: Core System Information Libraries
*(Foundational libraries; highest relevance as building blocks)*

| Crate                                                 | Notes                                                                                                                                                                                                        | Details                                                               |
|-------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------|
| [sysinfo](https://crates.io/crates/sysinfo)           | The dominant Rust system-info library. Cross-platform (Linux, macOS, Windows, FreeBSD). Covers everything resource-tracker needs except GPU. Used internally by most other crates here. ~2,700 GitHub stars. | Linux; no CLI; CPU/Mem/Net/Disk; process-level; active; 123M downloads |
| [procfs](https://crates.io/crates/procfs)             | Direct interface to Linux `/proc`. Most granular per-process data available (CPU time, RSS, VMS, I/O counters, smaps). Authoritative source for Linux-first tools.                                           | Linux only; no CLI; CPU/Mem/Net/Disk; process-level; active; 51M downloads |
| [psutil](https://crates.io/crates/psutil)             | Rust port of Python's psutil. Modular feature flags. Linux + macOS. README self-describes as "not well maintained" despite a July 2025 update.                                                               | Linux; no CLI; CPU/Mem/Net/Disk; process-level; active\*; 3.1M downloads |
| [systemstat](https://crates.io/crates/systemstat)     | Pure Rust (no C bindings). Cross-platform. System-wide only — no per-process metrics.                                                                                                                        | Linux; no CLI; CPU/Mem/Net/Disk; system-wide only; active; 3.6M downloads |
| [libproc](https://crates.io/crates/libproc)           | Per-process data on Linux + macOS. Useful complement to `procfs` for cross-platform support.                                                                                                                 | Linux; no CLI; CPU/Mem/Net/Disk; process-level; active; 5M downloads  |
| [memory-stats](https://crates.io/crates/memory-stats) | Cross-platform. Reports the *current process's own* RSS and virtual memory only. Narrow scope but zero-dependency and reliable.                                                                              | Linux; no CLI; Mem only; self-process only; active; 10.3M downloads   |
| [perf_monitor](https://crates.io/crates/perf_monitor) | Larksuite (Lark/Feishu). Designed explicitly as a monitoring foundation: per-process CPU, memory, FDs, disk I/O. Cross-platform. Archived January 2026 — do not adopt for new projects.                      | Linux; no CLI; CPU/Mem/Disk; process-level; **archived**; 36K downloads |
| [heim](https://crates.io/crates/heim)                 | Async-first psutil/gopsutil equivalent. Conceptually ideal but last released 2020; 74 open issues. Not safe to adopt.                                                                                        | Linux; no CLI; CPU/Mem/Net/Disk; process-level; **abandoned**; 490K downloads |

\*psutil: stated as "not well maintained" in README despite recent activity.

---

## Section 2: GPU Monitoring Libraries

| Crate                                                 | Notes                                                                                                                                                                                        | Details                                                                     |
|-------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------|
| [nvml-wrapper](https://crates.io/crates/nvml-wrapper) | Safe, ergonomic Rust wrapper for NVIDIA NVML. Covers GPU utilization, memory, temperature, power, fan speed, running compute processes. The standard library for NVIDIA GPU metrics in Rust. | Linux; no CLI; NVIDIA GPU; active; 3.5M downloads                           |
| [all-smi](https://crates.io/crates/all-smi)           | Most comprehensive multi-vendor GPU CLI in Rust. Prometheus metrics integration. Display-oriented but scriptable.                                                                            | Linux; CLI + Prometheus; NVIDIA/AMD/Intel/Apple/TPU/NPU GPU; active; 8.3K downloads |
| [nviwatch](https://crates.io/crates/nviwatch)         | Interactive TUI + InfluxDB integration. NVIDIA-only.                                                                                                                                         | Linux; TUI; NVIDIA GPU; active; 4.9K downloads                              |
| [gpuinfo](https://crates.io/crates/gpuinfo)           | Minimal CLI for GPU status with `--watch` and `--format` flags. Scriptable. NVIDIA-only.                                                                                                     | Linux; CLI; NVIDIA GPU; active; 5.9K downloads                              |

---

## Section 3: CLI Tools for Batch Job / Process Resource Tracking
*(Most directly comparable to `resource-tracker`'s execution model)*

| Crate                                                                       | Notes                                                                                                                                                                                                                                                         | Details                                                           |
|-----------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------|
| [denet](https://crates.io/crates/denet)                                     | **Closest Rust analogue to resource-tracker.** `denet run <cmd>` wraps a command and streams CPU%, memory (RSS+VMS), and I/O metrics. JSON/JSONL/CSV output. Adaptive sampling. Child process aggregation. Python API bindings. No GPU or network monitoring. | Linux; CLI; CPU/Mem/Disk; active; 2.6K downloads                  |
| [session-process-monitor](https://crates.io/crates/session-process-monitor) | Kubernetes-focused but `spm run` pattern directly wraps a batch job with monitoring + OOM protection + headless JSON logging. Tracks USS/PSS/RSS memory and disk I/O rate. Very new (March 2026). No GPU or network.                                          | Linux only; CLI (spm run); CPU/Mem/Disk; active; 173 downloads    |
| [stop-cli](https://crates.io/crates/stop-cli)                               | Modern process viewer with JSON/CSV structured output designed for piping to `jq`. Per-process CPU%, memory, disk I/O, FDs. Very early stage (v0.0.1, November 2025).                                                                                         | Linux; CLI; CPU/Mem/Disk; active; 72 downloads                    |
| [procrec](https://crates.io/crates/procrec)                                 | Records and plots CPU + memory for a process. Conceptually aligned but last updated 2021.                                                                                                                                                                     | Linux; CLI; CPU/Mem; **abandoned**; 1.7K downloads                |
| [radvisor](https://crates.io/crates/radvisor)                               | Container/Kubernetes batch monitoring at 50ms granularity via cgroups. CSVY output. CPU (including throttling), memory, block I/O. Dormant since 2022.                                                                                                        | Linux only; CLI; CPU/Mem/Disk; dormant; 1.7K downloads            |
| [pidtree_mon](https://crates.io/crates/pidtree_mon)                         | CLI monitor for CPU load across entire process trees (parent + all descendants). CPU-only; no memory/disk/network/GPU.                                                                                                                                        | Linux only; CLI; CPU only; active; 6.2K downloads                 |
| [gotta-watch-em-all](https://crates.io/crates/gotta-watch-em-all)           | CLI memory monitor for process trees. Memory-only. Dormant since 2022.                                                                                                                                                                                        | Linux; CLI; Mem only; dormant; 6.5K downloads                     |
| [procweb-rust](https://crates.io/crates/procweb-rust)                       | Web interface for per-process Linux resource usage. No structured data output. Stale since 2023.                                                                                                                                                              | Linux only; web UI; CPU/Mem; stale; 5.5K downloads                |
| [systrack](https://crates.io/crates/systrack)                               | Library for tracking CPU and memory usage over configurable time intervals (rolling windows) — the exact pattern resource-tracker uses. Single release in 2023; dormant since.                                                                                | Linux; no CLI; CPU/Mem; dormant; 1.4K downloads                   |

---

## Section 4: Interactive TUI System Monitors
*(Visual monitors; not designed for non-interactive batch job instrumentation)*

| Crate                                             | Notes                                                                                                                                            | Details                                               |
|---------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------|
| [bottom](https://crates.io/crates/bottom) (`btm`) | Most popular Rust TUI monitor. Cross-platform. No GPU. Uses `sysinfo` internally. Interactive only — not suitable for batch job instrumentation. | Linux; TUI; CPU/Mem/Net/Disk; active; 13,100 stars    |
| [mltop](https://crates.io/crates/mltop)           | ML-focused TUI combining CPU + NVIDIA GPU (via NVML). Directly targets the ML engineer use case. Interactive only.                               | Linux; TUI; CPU/Mem/NVIDIA GPU; active; 14 stars      |
| [rtop](https://crates.io/crates/rtop)             | TUI with optional NVIDIA GPU support. Covers all five resource types in a single tool. Interactive only.                                         | Linux; TUI; CPU/Mem/NVIDIA GPU/Net/Disk; active; 36 stars |
| [ttop](https://crates.io/crates/ttop)             | TUI with multi-vendor GPU (NVIDIA, AMD, Apple Silicon). Very new (March 2026). Interactive only.                                                 | Linux; TUI; CPU/Mem/multi-vendor GPU; active          |
| [hegemon](https://crates.io/crates/hegemon)       | Modular safe-Rust TUI. Last release 2018. Historical reference only.                                                                             | Linux only; TUI; CPU/Mem; **abandoned**; 336 stars    |

---

## Section 5: Comprehensive Hardware Monitoring

| Crate                                                       | Notes                                                                                                                                                                                                                                                                                                                                                                               | Details                                                       |
|-------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------|
| [silicon-monitor](https://crates.io/crates/silicon-monitor) | Most comprehensive hardware monitoring scope of any crate here. NVIDIA (NVML) + AMD (ROCm/sysfs) + Intel (i915) GPU. Also covers temperatures, SMART disk data, USB, audio, per-process GPU attribution. Provides CLI (JSON output), TUI, GUI, library (`simonlib`), and MCP/AI agent server. Very new (133 downloads, 1 star as of March 2026); unclear stability. Worth watching. | Linux; CLI (JSON); CPU/Mem/multi-vendor GPU/Net/Disk; active  |

---

## Section 6: Kernel / Low-Level Profiling Crates
*(Measure hardware counters, not high-level resource utilization)*

| Crate                                             | Notes                                                                                                                                                                                                                                | Details                                      |
|---------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|----------------------------------------------|
| [perf-event](https://crates.io/crates/perf-event) | Safe Rust interface to `perf_event_open`. Exposes hardware counters: CPU cycles, instructions, cache hits/misses, branch predictions, page faults, context switches. Deep profiling of batch jobs; not high-level resource tracking. | Linux only; no CLI; active; 4.2M downloads   |
| [pprof](https://crates.io/crates/pprof)           | CPU profiler for Rust programs (stack sampling → flamegraph/pprof output). Profiler, not a resource monitor.                                                                                                                         | Linux; no CLI; active; 34M downloads         |
| [metrics](https://crates.io/crates/metrics)       | Application metrics facade (counters, gauges, histograms). Used to *emit* measurements; not a collector of system resources.                                                                                                         | Linux; no CLI; active; 74M downloads         |

---

## Section 7: `dial9-tokio-telemetry` — Async Runtime Telemetry (Out of Scope)

[dial9-tokio-telemetry](https://github.com/dial9-rs/dial9-tokio-telemetry) is a runtime telemetry "flight recorder" for the **Tokio** async runtime in Rust, announced on the Tokio blog on March 18, 2026 (authored by Russell Cohen, with AWS contributions). It is included here to explain why it is **not** a resource monitor and does not belong in this landscape.

### What it does

dial9 hooks into Tokio's internal instrumentation to capture a microsecond-resolution event log of every:
- Task poll (timing per poll)
- Worker park / unpark event
- Task wake event and lifecycle (creation, worker migration)
- Queue depth change
- Lock contention event (with stack traces on Linux)
- Linux kernel scheduling delay (gap between "ready to run" and "actually scheduled")
- CPU profile samples (Linux perf/eBPF-style)
- Application-level `tracing` spans and logs

Traces are written to compact rotating binary files (or directly to S3) with <5% overhead, enabling continuous production deployment. A web-based trace viewer renders the results.

### Why it is not a resource monitor

| Dimension         | `resource-tracker`                   | `dial9-tokio-telemetry`                                |
|-------------------|--------------------------------------|--------------------------------------------------------|
| Target workload   | Batch jobs (ML, HPC, pipelines)      | Long-running async Rust services                       |
| Metrics tracked   | CPU%, RAM, GPU, network, disk        | Tokio task polls, scheduling delays, lock contention   |
| Integration       | Decorator / subprocess wrap          | Must be compiled into the Rust binary                  |
| Output            | Time-series resource usage / plots   | Binary event traces for async runtime debugging        |
| Question answered | "How much CPU/RAM did this job use?" | "Why did this async request take 18ms instead of 1ms?" |
| Platform          | Cross-platform                       | Linux-primary                                          |

dial9 is an **async runtime debugger**. It tracks none of the metrics — CPU utilization %, memory, GPU, network bandwidth, disk I/O — that define the resource-tracker use case. It is relevant to Rust async service reliability engineering, not to batch job resource instrumentation.

---

## Summary: Key Findings

### No single Rust crate fully replicates `resource-tracker`

No existing Rust crate combines: subprocess/batch-job wrapping + CPU% + memory + GPU + network + disk + structured JSON/CSV output + low overhead. The gap is real.

### Closest existing tools

| Crate                     | Why it is close                                                     | What is missing                   |
|---------------------------|---------------------------------------------------------------------|-----------------------------------|
| `denet`                   | `denet run <cmd>` wraps a command; JSON/CSV output; Python bindings | GPU, network                      |
| `session-process-monitor` | `spm run` pattern; OOM protection; headless JSON logging            | GPU, network                      |
| `stop-cli`                | Structured JSON/CSV; scripting-friendly                             | Not a job wrapper; no GPU/network |

### Recommended building blocks for a Rust resource-tracker port

| Purpose                                       | Crate          |
|-----------------------------------------------|----------------|
| CPU, memory, disk, network (system + process) | `sysinfo`      |
| Fine-grained Linux per-process I/O and memory | `procfs`       |
| NVIDIA GPU metrics                            | `nvml-wrapper` |
| Multi-vendor GPU CLI                          | `all-smi`      |

### The GPU gap

No Rust library cleanly integrates CPU + memory + multi-vendor GPU + network + disk in a single programmatic API suitable for batch job wrapping. `silicon-monitor` attempts this scope but is brand new and unproven. `nvml-wrapper` covers NVIDIA programmatically; multi-vendor GPU support requires either `all-smi` (CLI) or direct vendor SDK bindings.
