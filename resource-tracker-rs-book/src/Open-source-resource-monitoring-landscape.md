# Open-Source Resource Monitoring Landscape
## Competitive Analysis for `resource-tracker` (SpareCores)

**Prepared:** March 25, 2026
**Context:** Phase 1 feasibility assessment for a Rust/Linux CLI implementation of ResourceTracker
**Reference tool:** https://github.com/SpareCores/resource-tracker

---

## Executive Summary

`resource-tracker` occupies a specific and underserved niche: a **lightweight, zero-dependency, batch-job-oriented process + system resource monitor** with workflow framework integration (Metaflow), visualization via cards, and cloud server recommendations. The open-source landscape has many partial overlaps but no single tool matches all its characteristics simultaneously.

The tools below are organized into meaningful categories. Most tools are either:
- **Too low-level** (profilers that require code instrumentation or produce flame graphs rather than time-series resource logs)
- **Too heavy** (system daemons, full observability stacks)
- **Too narrow** (single-resource: CPU only, or memory only, or GPU only)
- **Not batch-job oriented** (designed for long-running services, not scripts that run and exit)

---

## Category 1: Python Libraries for Process/System Resource Monitoring

These are the closest functional analogues to `resource-tracker` in the Python ecosystem.

---

### 1.1 psutil
- **URL:** https://github.com/giampaolo/psutil
- **Language:** Python (C extension)
- **Description:** The foundational library for cross-platform system/process information in Python. `resource-tracker` itself uses psutil as an optional backend on non-Linux systems. psutil retrieves CPU, memory, disk, network, and process-level data programmatically but provides no time-series tracking, no decorator/wrapper API, no visualization, and no batch job reporting.
- **Key features:** CPU %, memory (RSS/PSS/USS/VMS), per-process I/O, network I/O, disk usage, process tree traversal. Cross-platform (Linux, macOS, Windows).
- **Difference:** Raw data API only. No tracking loop, no reports, no workflow integration. It is a building block, not a solution.

---

### 1.2 memory_profiler
- **URL:** https://github.com/pythonprofilers/memory_profiler
- **Language:** Python
- **Description:** Line-by-line memory usage profiler for Python scripts. Uses `@profile` decorator and `mprof` CLI to record memory usage over time and plot it. Built on psutil.
- **Key features:** Line-level memory profiling, time-series memory plot via `mprof`, `@profile` decorator, `memory_usage()` API.
- **Difference:** Memory only (no CPU, GPU, disk, network). Requires code instrumentation for line-level profiling. Targeted at developers finding memory leaks, not at batch job operators seeking resource utilization logs.

---

### 1.3 Scalene
- **URL:** https://github.com/plasma-umass/scalene
- **Language:** Python + C++
- **Description:** High-performance, high-precision CPU, GPU, and memory profiler for Python. Uniquely profiles CPU time, GPU time, and memory at the line level simultaneously. Includes AI-powered optimization suggestions and an interactive web UI.
- **Key features:** Line-level CPU + GPU + memory profiling, separates Python vs native time, web-based interactive report, minimal overhead (~10-20%).
- **Difference:** A developer profiler (find bottlenecks in code), not a resource utilization logger for batch jobs. Does not track network or disk I/O, does not integrate with workflow tools, does not produce time-series utilization logs for operational use.

---

### 1.4 Memray
- **URL:** https://github.com/bloomberg/memray
- **Language:** Python + C++
- **Description:** Bloomberg's memory profiler for Python. Tracks every allocation in Python, native extensions, and the interpreter itself. Produces flame graphs, heap charts, and other visualizations.
- **Key features:** Full allocation tracking (Python + C/C++), flame graphs, live mode, Jupyter integration, reporter API.
- **Difference:** Memory only, developer-oriented (find leaks/hotspots in code). Does not track CPU, GPU, disk, or network. Not designed for batch job monitoring.

---

### 1.5 Fil (filprofiler)
- **URL:** https://github.com/pythonspeed/filprofiler
- **Language:** Python + Rust
- **Description:** Memory profiler from pythonspeed targeting data scientists and scientific computing. Finds peak memory usage and identifies what code caused the peak. Produces flame graphs.
- **Key features:** Peak memory tracking (captures C and Python allocations), flame graphs, designed for NumPy/Pandas workloads, CLI usage.
- **Difference:** Memory only, developer-oriented. No CPU, GPU, disk, network. Produces offline profiling reports, not operational time-series logs.

---

### 1.6 pyinstrument
- **URL:** https://github.com/joerick/pyinstrument
- **Language:** Python
- **Description:** Sampling call-stack profiler for Python. Samples the call stack every 1ms and shows a readable summary of where time is spent. Supports context manager and decorator API.
- **Key features:** Low-overhead sampling, context manager (`with Profiler()`), decorator, CLI, HTML/text/JSON output, async support.
- **Difference:** CPU time only (call stack), no memory/GPU/disk/network. Developer-oriented (why is code slow?), not a resource utilization monitor.

---

### 1.7 py-spy
- **URL:** https://github.com/benfred/py-spy
- **Language:** Rust
- **Description:** Sampling profiler for Python programs written in Rust. Attaches to a running Python process without modifying it. Can generate flame graphs or a top-like display.
- **Key features:** Attaches to running process (no code changes), flame graphs, top-like live view, very low overhead, works across OS.
- **Difference:** CPU only (call stack). No memory, GPU, disk, or network tracking. Attach-to-process model differs from `resource-tracker`'s wrap-a-job model.

---

### 1.8 Austin
- **URL:** https://github.com/P403n1x87/austin
- **Language:** C
- **Description:** Python frame stack sampler for CPython. Samples the Python interpreter's memory space directly to retrieve running thread stacks. Extremely low overhead.
- **Key features:** Zero-instrumentation, pure C, very low overhead, multi-thread and multi-process support, output compatible with flame graph tools.
- **Difference:** CPU/call stack profiling only. No resource utilization metrics (memory, GPU, disk, network).

---

### 1.9 Glances
- **URL:** https://github.com/nicolargo/glances
- **Language:** Python
- **Description:** Cross-platform system monitoring tool with a rich curses/web UI. Shows CPU, memory, disk, network, process list, temperatures, GPU (via plugin), Docker containers, and more. Can export data to InfluxDB, CSV, Prometheus, etc.
- **Key features:** Real-time monitoring, web UI, REST API, exporters (InfluxDB, Prometheus, CSV, JSON), Docker/container awareness, GPU plugin, cross-platform (Linux, macOS, Windows, BSD).
- **Difference:** A long-running system monitor daemon/interactive tool, not designed to wrap a batch job, produce a per-job report, or integrate with workflow frameworks. No job-level summary reports.

---

### 1.10 nvitop
- **URL:** https://github.com/XuehaiPan/nvitop
- **Language:** Python
- **Description:** Interactive NVIDIA GPU process viewer with a rich terminal UI. Goes beyond `nvidia-smi` by showing per-process GPU/VRAM usage in real time, supports programmatic API access.
- **Key features:** Per-process GPU utilization and VRAM, process tree, interactive kill/signal, rich terminal UI, Python API (`ResourceMetricCollector`).
- **Difference:** GPU-only (NVIDIA). Covers system + process level GPU metrics well. Its `ResourceMetricCollector` API is a meaningful overlap with `resource-tracker` for GPU tracking. No CPU/memory/disk/network integration.

---

### 1.11 gpustat
- **URL:** https://github.com/wookayin/gpustat
- **Language:** Python
- **Description:** Simple command-line utility for querying and monitoring NVIDIA GPU status. Aggregates `nvidia-smi` output with color-coded display. Supports `--watch` mode.
- **Key features:** GPU utilization, VRAM usage, temperature, power draw, per-process GPU use, JSON output, watch mode.
- **Difference:** NVIDIA GPU only, read-only display tool, no time-series logging, no CPU/memory/disk/network.

---

### 1.12 pynvml / nvidia-ml-py
- **URL:** https://github.com/gpuopenanalytics/pynvml
- **Language:** Python (NVML binding)
- **Description:** Python bindings for NVIDIA's NVML C library, enabling programmatic GPU diagnostics. Used as a building block by gpustat, nvitop, and resource-tracker itself.
- **Key features:** Full NVML API access: GPU utilization, VRAM, temperature, power, clock speed, process-level GPU usage, fan speed.
- **Difference:** Raw API, no tracking loop, no reporting. A building block.

---

### 1.13 CodeCarbon
- **URL:** https://github.com/mlco2/codecarbon
- **Language:** Python
- **Description:** Tracks CPU, GPU, and RAM energy consumption and converts it to estimated CO2 emissions. Designed for ML training runs. Provides decorator and context manager APIs.
- **Key features:** `@track_emissions` decorator, context manager, estimates CO2 equivalent, per-run reporting, dashboard, supports Intel RAPL and NVML.
- **Difference:** Focused on energy/carbon footprint rather than raw resource utilization metrics. Does not track disk I/O or network. Closest in UX philosophy (decorator for batch scripts) but different output goal.

---

### 1.14 CarbonTracker
- **URL:** https://github.com/lfwa/carbontracker
- **Language:** Python
- **Description:** Tracks and predicts energy consumption and carbon footprint of deep learning model training. Can stop training when predicted impact exceeds a threshold.
- **Key features:** Predictive carbon footprint, supports GPU and CPU energy, training-run oriented, can send alerts.
- **Difference:** Energy/carbon focused, ML training specific, no disk/network tracking.

---

### 1.15 pyRAPL
- **URL:** https://github.com/powerapi-ng/pyRAPL
- **Language:** Python
- **Description:** Measures energy consumption of Python code using Intel RAPL (Running Average Power Limit) hardware counters. Provides decorator and context manager APIs.
- **Key features:** CPU socket, DRAM, and integrated GPU energy measurement, decorator and `with` block APIs, per-domain granularity.
- **Difference:** Intel RAPL only (Intel CPUs since Sandy Bridge), energy not utilization percentage, no GPU computation metrics, no disk/network.

---

### 1.16 pyJoules
- **URL:** https://github.com/powerapi-ng/pyJoules
- **Language:** Python
- **Description:** Captures energy consumption of code snippets using Intel RAPL and NVIDIA NVML. Provides decorator and context manager APIs with breakpoints.
- **Key features:** Multi-device energy capture (CPU, DRAM, NVIDIA GPU), decorator API, MongoDB and Pandas export handlers.
- **Difference:** Energy measurement, not utilization tracking. Requires Intel RAPL-capable hardware.

---

### 1.17 PowerAPI
- **URL:** https://github.com/powerapi-ng/powerapi
- **Language:** Python
- **Description:** Middleware framework for building software-defined power meters. Estimates power at process, container, VM, or application level. Can use hardware counters or performance counters.
- **Key features:** Pluggable sensors and estimators, multiple granularity levels (process, container, VM), real-time power estimation.
- **Difference:** Power/energy framework requiring configuration and sensor setup. Not a drop-in decorator for batch jobs.

---

### 1.18 eco2AI
- **URL:** https://github.com/sb-ai-lab/eco2AI
- **Language:** Python
- **Description:** Tracks carbon emissions while training/inferring Python ML models. Accounts for CPU, GPU, and RAM energy consumption.
- **Key features:** `@track_emissions` decorator, real-time emission monitoring, CSV reporting.
- **Difference:** Carbon/energy focus, similar decorator pattern to `resource-tracker`, no disk/network.

---

### 1.19 pyperf
- **URL:** https://github.com/psf/pyperf
- **Language:** Python
- **Description:** Python Software Foundation toolkit for writing and running benchmarks. Includes memory tracking (`--track-memory`, `--tracemalloc`) as part of benchmark metadata collection.
- **Key features:** Benchmark calibration, worker process management, memory peak tracking, JSON results, statistical analysis.
- **Difference:** Benchmarking framework, not a general resource monitor. Memory tracking is incidental to benchmarking.

---

### 1.20 ClearML
- **URL:** https://github.com/clearml/clearml
- **Language:** Python
- **Description:** Open-source MLOps platform. Automatically tracks GPU, CPU, memory, and network metrics during ML experiment runs. Provides an experiment tracker, data manager, orchestrator, and more.
- **Key features:** Automatic system metric logging (GPU, CPU, memory, network), experiment tracking, model registry, pipeline orchestration, web UI.
- **Difference:** Full MLOps platform (not a lightweight library). Requires a ClearML server. Targets ML experiments rather than general batch jobs.

---

### 1.21 python-resmon
- **URL:** https://github.com/xybu/python-resmon
- **Language:** Python
- **Description:** Lightweight resource monitor that records CPU usage, RAM usage, disk I/O, and NIC speed, outputting data in CSV format for post-processing.
- **Key features:** CSV output, configurable polling interval, system-level metrics, easy post-processing.
- **Difference:** System-level only (no per-process tracking), no GPU, no visualization, no workflow integration. Small utility script rather than a library.

---

## Category 2: Interactive Terminal Monitors (System-Level)

These tools provide real-time visual monitoring of system resources. They do not produce per-job reports or integrate with batch workflows, but they are widely used for manual resource observation.

---

### 2.1 htop
- **URL:** https://github.com/htop-dev/htop
- **Language:** C
- **Description:** Interactive process viewer and system monitor. The modern replacement for `top`. Shows per-CPU usage, memory, swap, and a process list with tree view.
- **Key features:** Interactive (kill, renice, filter), color-coded per-CPU bars, tree view, mouse support, cross-platform.
- **Difference:** Interactive visual tool only. No data capture, no time-series, no batch job integration.

---

### 2.2 btop / btop++
- **URL:** https://github.com/aristocratos/btop
- **Language:** C++
- **Description:** Advanced terminal resource monitor. Third generation of bashtop->bpytop->btop++. Shows CPU, memory, disk I/O, network, and process list with rich ASCII art graphs.
- **Key features:** Responsive UI, mouse support, GPU support (Nvidia/AMD/Intel via plugins), disk I/O, network I/O, process filtering, themes.
- **Difference:** Interactive visual tool only. No data export, no batch job tracking.

---

### 2.3 bpytop
- **URL:** https://github.com/aristocratos/bpytop
- **Language:** Python
- **Description:** Python predecessor to btop++. Linux/macOS/FreeBSD resource monitor with animated ASCII graphs.
- **Key features:** CPU, memory, disk, network, process list, ASCII graphs.
- **Difference:** Interactive visual tool. Superseded by btop++.

---

### 2.4 bashtop
- **URL:** https://github.com/aristocratos/bashtop
- **Language:** Bash
- **Description:** Original Bash-based resource monitor from the same developer. Ancestor of bpytop and btop++.
- **Key features:** CPU, memory, disk, network, process monitoring in pure Bash.
- **Difference:** Superseded by btop++. Interactive visual only.

---

### 2.5 glances (see 1.9 above)
- Interactive + exportable, see Category 1 entry.

---

### 2.6 atop
- **URL:** https://github.com/Atoptool/atop
- **Language:** C
- **Description:** Advanced interactive system and process monitor for Linux. Records all system activity and writes to binary log files for later replay/analysis. Integrates with `atopsar` for historical reporting.
- **Key features:** Full system activity logging (CPU, memory, disk, network, process), persistent binary logs, replay mode, atopsar for reporting.
- **Difference:** Long-running daemon for system-wide logging. Not designed to wrap a specific job; tracks the whole system. Closest among CLI tools to providing historical per-process data.

---

### 2.7 nmon (Nigel's Monitor)
- **URL:** http://nmon.sourceforge.net/
- **Language:** C
- **Description:** Performance monitoring tool for AIX and Linux. Provides real-time view and can capture data to CSV for later analysis with nmon Analyser.
- **Key features:** CPU, memory, disk I/O, network, filesystem, processes; CSV capture mode, lightweight.
- **Difference:** System-wide monitor. No batch job integration or workflow decorator. The CSV output mode is useful for offline analysis.

---

### 2.8 collectl
- **URL:** http://collectl.sourceforge.net/
- **Language:** Perl
- **Description:** Collects a broad set of Linux system statistics (CPU, memory, network, disk, inodes, processes, NFS, TCP, sockets) and can write to files, print to stdout, or feed to Graphite/ganglia.
- **Key features:** Wide metric coverage, multiple output formats (CSV, plot, etc.), daemon or one-shot mode.
- **Difference:** System-wide collection daemon. No batch job wrapping, no workflow integration.

---

### 2.9 sysstat (sar/sadc/sadf/iostat/pidstat/mpstat)
- **URL:** https://github.com/sysstat/sysstat
- **Language:** C
- **Description:** Collection of Linux performance monitoring utilities. `sar` collects and reports system activity historically. `pidstat` reports per-process CPU, memory, and I/O. `iostat` reports disk I/O. `sadc` is the backend data collector.
- **Key features:** Historical data collection, per-process stats via `pidstat`, JSON/CSV/XML output via `sadf`, schedulable via cron/systemd, very low overhead.
- **Difference:** System and process monitoring utilities, not designed for batch job wrapping. `pidstat` is the closest to per-job process monitoring but requires manual invocation.

---

### 2.10 nvtop
- **URL:** https://github.com/Syllo/nvtop
- **Language:** C
- **Description:** (h)top-like task monitor for GPUs and accelerators. Supports AMD, Apple M1/M2 (limited), Huawei Ascend, Intel, NVIDIA, Qualcomm, Broadcom, Rockchip.
- **Key features:** Multi-GPU and multi-vendor support, real-time GPU/VRAM utilization, per-process GPU use, interactive UI.
- **Difference:** GPU-focused interactive monitor. No data export, no CPU/memory/disk/network integration.

---

### 2.11 vtop
- **URL:** https://github.com/MrRio/vtop
- **Language:** JavaScript (Node.js)
- **Description:** Graphical terminal activity monitor with Unicode braille charts. Groups processes sharing the same name (e.g., NGINX master + workers).
- **Key features:** ASCII charts, process grouping, extensible via plugins.
- **Difference:** Interactive visual only, no data capture. Note: project appears unmaintained.

---

### 2.12 Netdata
- **URL:** https://github.com/netdata/netdata
- **Language:** C (agent core)
- **Description:** Real-time performance monitoring with per-second metrics and a powerful web UI. 800+ integrations. Most-starred monitoring project on GitHub (76k+ stars).
- **Key features:** Per-second metrics, web dashboard, alerts, ML anomaly detection, 800+ integrations (Docker, Kubernetes, StatsD, OpenMetrics), process-level metrics, GPU plugins.
- **Difference:** Full-stack observability daemon. Requires installation as a service. Not designed for wrapping a batch job.

---

## Category 3: eBPF / Kernel-Level Tracing Tools

These tools use Linux eBPF (extended Berkeley Packet Filter) for highly efficient, zero-instrumentation tracing deep in the kernel. Most relevant for system-level visibility with very low overhead.

---

### 3.1 BCC (BPF Compiler Collection)
- **URL:** https://github.com/iovisor/bcc
- **Language:** C + Python/Lua frontends
- **Description:** Toolkit for creating efficient kernel tracing and manipulation programs using eBPF. Includes ready-made tools (execsnoop, biolatency, tcplife, memleak, etc.) and a framework for writing custom eBPF programs with Python frontends.
- **Key features:** Kernel + userspace tracing, network/disk/memory/CPU tools, Python API for custom programs, very low overhead.
- **Difference:** Requires kernel support (Linux 4.1+), root privileges, and knowledge of eBPF to build custom tools. Not a drop-in batch job monitor.

---

### 3.2 bpftrace
- **URL:** https://github.com/bpftrace/bpftrace
- **Language:** C++ (awk/DTrace-like scripting language)
- **Description:** High-level tracing language for Linux eBPF. Write concise one-liners or short scripts for ad-hoc analysis.
- **Key features:** High-level scripting, LLVM backend, supports tracepoints, kprobes, uprobes, usdt. One-liner analysis.
- **Difference:** Ad-hoc kernel tracing tool. Requires root and kernel support. Not designed for operational batch job monitoring.

---

### 3.3 Parca / Parca Agent
- **URL:** https://github.com/parca-dev/parca
- **Language:** Go
- **Description:** Continuous profiling for CPU and memory usage, down to the line number and throughout time. Parca Agent is an eBPF-based always-on profiler with Kubernetes auto-discovery. Uses pprof format.
- **Key features:** Zero-instrumentation eBPF profiling, <1% overhead, continuous collection, icicle graph UI, SQL-queryable profile storage, multi-language support.
- **Difference:** Continuous profiling infrastructure (runs as a DaemonSet on Kubernetes nodes). Not a per-job wrapper. Heavy infrastructure requirement.

---

### 3.4 Pyroscope (Grafana)
- **URL:** https://github.com/grafana/pyroscope
- **Language:** Go
- **Description:** Continuous profiling database and platform (formed from merger of Phlare + Pyroscope). Stores profiling data from applications instrumented with Pyroscope SDKs or from eBPF agents. Integrates with Grafana.
- **Key features:** SDK-based push profiling (Python, Go, Java, Ruby, .NET, Rust, PHP, Node.js), eBPF pull mode, flame graphs, Grafana integration, scalable storage.
- **Difference:** Continuous profiling infrastructure. Requires a server and SDK integration. Not a lightweight batch job wrapper.

---

## Category 4: Linux Performance Profiling Tools (C/C++/Native)

These tools profile native code at a low level. Most are developer-focused profilers rather than operational monitors.

---

### 4.1 perf (Linux perf_events)
- **URL:** https://perfwiki.github.io/main/
- **Language:** C (Linux kernel subsystem)
- **Description:** The primary Linux performance tool. Samples CPU events using hardware performance counters, traces system calls, and instruments kernel/userspace functions. Foundation for many other tools.
- **Key features:** Hardware counter sampling, call graph recording, per-process and system-wide, flame graph generation (via FlameGraph scripts), supports all architectures.
- **Difference:** Low-level developer profiler. Requires root for many features. No time-series resource logging, no workflow integration.

---

### 4.2 FlameGraph
- **URL:** https://github.com/brendangregg/FlameGraph
- **Language:** Perl
- **Description:** Stack trace visualization toolkit by Brendan Gregg. Generates SVG flame graphs from perf, DTrace, SystemTap, and other profiler output.
- **Key features:** CPU, memory, and off-CPU flame graphs, works with many backends.
- **Difference:** Visualization tool for profiler output, not a monitoring tool itself.

---

### 4.3 gperftools (Google Performance Tools)
- **URL:** https://github.com/gperftools/gperftools
- **Language:** C++
- **Description:** Collection from Google: fast malloc (TCMalloc), CPU profiler, heap profiler, and heap checker. Used via `LD_PRELOAD` or explicit linking.
- **Key features:** CPU profiling (sampling), heap profiling, heap leak detection, pprof visualization, multi-threaded support.
- **Difference:** Developer profiler requiring code linking or LD_PRELOAD. No time-series operational monitoring, no disk/network/GPU.

---

### 4.4 Valgrind / Massif / Callgrind
- **URL:** https://valgrind.org/
- **Language:** C
- **Description:** Instrumentation framework for building dynamic analysis tools. Massif is its heap profiler; Callgrind is its call graph profiler; Memcheck is its memory error detector.
- **Key features:** Complete heap tracking, memory leak detection, call graph analysis, massif-visualizer GUI.
- **Difference:** High-overhead instrumentation (10-50x slowdown). Developer tool, not operational monitor. No GPU, disk, or network metrics.

---

### 4.5 Heaptrack
- **URL:** https://github.com/KDE/heaptrack
- **Language:** C++ + Python
- **Description:** Fast heap memory profiler for Linux, designed as a faster, lower-overhead alternative to Valgrind/Massif. Traces all allocations and annotates with stack traces.
- **Key features:** Lower overhead than Valgrind, flame graph output, heaptrack_gui for visualization, finds memory leaks and allocation hotspots.
- **Difference:** Memory only, developer profiler. No GPU, CPU utilization, disk, or network.

---

### 4.6 Perfetto
- **URL:** https://github.com/google/perfetto
- **Language:** C++
- **Description:** Google's open-source production-grade system profiling and tracing tool. Default tracing system for Android and used in Chromium. Can capture CPU scheduling, memory, I/O, GPU events, and custom trace points.
- **Key features:** Multi-process system trace, SQL-based analysis, browser-based UI, heap profiling (heapprofd), CPU frequency and scheduling, Android + Linux support.
- **Difference:** Complex tracing infrastructure primarily targeting Android/embedded and browser use cases. Not a lightweight batch job wrapper.

---

### 4.7 async-profiler
- **URL:** https://github.com/async-profiler/async-profiler
- **Language:** C (JVM agent)
- **Description:** Low-overhead sampling CPU and heap profiler for JVM (Java/Kotlin/Scala/Clojure). Uses AsyncGetCallTrace + perf_events to avoid safepoint bias.
- **Key features:** CPU + heap sampling, flame graphs, JFR files, tracks native + JVM code, suitable for production.
- **Difference:** JVM-specific. No Python/R/general process monitoring. No disk, network, or GPU.

---

### 4.8 TAU (Tuning and Analysis Utilities)
- **URL:** https://www.cs.uoregon.edu/research/tau/home.php
- **Language:** C++ (with Python, Fortran, Java support)
- **Description:** Comprehensive profiling and tracing toolkit for HPC parallel programs (MPI, OpenMP, CUDA). Supports hardware counters, GPU profiling, and generates call graphs.
- **Key features:** Parallel program profiling (MPI, OpenMP), hardware counters, GPU support, ParaProf visualization, call graph.
- **Difference:** HPC research tool for parallel program performance analysis. Complex setup, not a lightweight batch job wrapper.

---

### 4.9 HPCToolkit
- **URL:** https://hpctoolkit.org/
- **Language:** C/C++
- **Description:** Sampling-based measurement and analysis suite for HPC programs on CPUs and GPUs. Supports supercomputers.
- **Key features:** 1-5% overhead sampling, full calling context, hpcviewer GUI, GPU support.
- **Difference:** HPC research tool, complex setup, not designed for general batch jobs or Python/R scripts.

---

## Category 5: Rust Tools

---

### 5.1 below (Facebook/Meta)
- **URL:** https://github.com/facebookincubator/below
- **Language:** Rust
- **Description:** Time-traveling resource monitor for modern Linux systems. Records system activity to disk and allows replay of historical data. Cgroup-aware with PSI (Pressure Stall Information) support.
- **Key features:** Record + replay mode, cgroup hierarchy view, PSI metrics, process-level stats, live mode, persistent storage. Built on cgroupv2.
- **Difference:** System-wide monitoring daemon. Designed for Linux infrastructure monitoring, not for wrapping individual batch jobs. No workflow integration. Very strong on cgroup/container awareness.

---

### 5.2 samply
- **URL:** https://github.com/mstange/samply
- **Language:** Rust
- **Description:** Command-line sampling CPU profiler for macOS, Linux, and Windows. Uses Linux perf events. Spawns the target process as a subprocess and profiles it, then opens Firefox Profiler UI.
- **Key features:** Subprocess wrapping (`samply record ./your_program`), Firefox Profiler UI, local symbol resolution, flame graphs.
- **Difference:** CPU profiling only (call stack). No memory, GPU, disk, or network tracking. Developer profiler.

---

### 5.3 Bytehound
- **URL:** https://github.com/koute/bytehound
- **Language:** Rust
- **Description:** Memory profiler for Linux. Intercepts all heap allocations via `LD_PRELOAD`. Produces detailed allocation timelines with stack traces.
- **Key features:** Full allocation tracking, web-based GUI, Rhai scripting for analysis, multi-architecture (AMD64, ARM, AArch64, MIPS64).
- **Difference:** Memory only. Developer profiler. Requires `LD_PRELOAD`, no GPU/disk/network.

---

### 5.4 pprof-rs
- **URL:** https://github.com/tikv/pprof-rs
- **Language:** Rust
- **Description:** Rust CPU profiler using backtrace-rs. Generates pprof-compatible output.
- **Key features:** CPU profiling for Rust applications, pprof output, flame graphs, low overhead.
- **Difference:** CPU profiler for Rust programs only.

---

## Category 6: System-Level Daemons and Metrics Collection Infrastructure

These tools are designed for long-running infrastructure monitoring, not individual batch jobs, but represent the broader ecosystem.

---

### 6.1 Prometheus + node_exporter
- **URL:** https://github.com/prometheus/node_exporter
- **Language:** Go
- **Description:** Prometheus exporter for hardware and OS metrics from `/proc` and `/sys`. Exposes CPU, memory, disk, network, filesystem, and more as Prometheus metrics.
- **Key features:** Pull-based metrics, scrape-able endpoint, very broad metric coverage, alerting via Prometheus + Alertmanager.
- **Difference:** Infrastructure monitoring daemon. Requires Prometheus server. No per-job tracking.

---

### 6.2 Prometheus Pushgateway
- **URL:** https://github.com/prometheus/pushgateway
- **Language:** Go
- **Description:** Push acceptor for ephemeral and batch jobs. Allows short-lived jobs to push metrics to Prometheus (which normally pulls). Stores last-received metrics until explicitly deleted.
- **Key features:** HTTP push endpoint, labels/grouping by job, integrates with Prometheus.
- **Difference:** Infrastructure component. Not a resource tracker itself; requires a separate process to collect and push metrics. **Most relevant for a Rust implementation that needs to output to Prometheus.**

---

### 6.3 Prometheus process-exporter
- **URL:** https://github.com/ncabatoff/process-exporter
- **Language:** Go
- **Description:** Prometheus exporter that reads `/proc` to report on selected processes. Groups processes by name or regex and exposes CPU, memory, file descriptors, I/O, and thread counts.
- **Key features:** Per-process-group CPU and memory metrics, `/proc`-based, configurable process selection, Prometheus compatible.
- **Difference:** Infrastructure daemon, not a batch job wrapper. Monitors selected processes continuously.

---

### 6.4 cAdvisor (Container Advisor)
- **URL:** https://github.com/google/cadvisor
- **Language:** Go
- **Description:** Google's container resource usage and performance analysis agent. Exposes Prometheus metrics for running containers.
- **Key features:** Container-level CPU, memory, disk, and network metrics, Prometheus endpoint, supports Docker and other runtimes.
- **Difference:** Container/cgroup focused daemon. Not for general process monitoring.

---

### 6.5 Telegraf
- **URL:** https://github.com/influxdata/telegraf
- **Language:** Go
- **Description:** Plugin-driven metrics collection agent from InfluxData. Single agent collecting system metrics (CPU, memory, disk, network, GPU, containers) and writing to InfluxDB or other backends.
- **Key features:** 300+ input plugins (system, Docker, SNMP, statsd, etc.), multiple output backends, flexible configuration.
- **Difference:** Infrastructure agent daemon. Not designed for per-job wrapping.

---

### 6.6 Netdata (see 2.12)

---

### 6.7 kube-state-metrics
- **URL:** https://github.com/kubernetes/kube-state-metrics
- **Language:** Go
- **Description:** Kubernetes add-on that generates metrics about Kubernetes object state (pod resource requests/limits, deployment status, etc.) for Prometheus.
- **Key features:** Pod/node resource quota metrics, deployment health, Prometheus format.
- **Difference:** Kubernetes-only, no process-level metrics.

---

### 6.8 OpenTelemetry (OTel)
- **URL:** https://opentelemetry.io/ / https://github.com/open-telemetry/opentelemetry-python
- **Language:** Multi-language (Go, Python, Java, .NET, etc.)
- **Description:** CNCF standard for collecting traces, metrics, and logs. Includes system metrics via the OTel Collector. Growing support for profiling via OTel.
- **Key features:** Traces + metrics + logs, vendor-neutral, collector, SDKs in all major languages, exporters to Prometheus, Jaeger, OTLP.
- **Difference:** General observability framework, not a resource tracker per se. Relevant for instrumenting a Rust CLI to expose metrics in a standard format.

---

### 6.9 NVIDIA DCGM + dcgm-exporter
- **URL:** https://github.com/NVIDIA/DCGM / https://github.com/NVIDIA/dcgm-exporter
- **Language:** C (DCGM) + Go (exporter)
- **Description:** NVIDIA Data Center GPU Manager for GPU telemetry in large Linux clusters. dcgm-exporter exposes GPU metrics for Prometheus.
- **Key features:** Per-GPU and per-process GPU metrics, health monitoring, diagnostics, Kubernetes integration, Prometheus exporter.
- **Difference:** NVIDIA GPU infrastructure daemon for data center clusters. Not a batch job wrapper.

---

## Category 7: Per-Process Network and Disk I/O Monitors

---

### 7.1 nethogs
- **URL:** https://github.com/raboof/nethogs
- **Language:** C++
- **Description:** Linux "net top" tool that groups network bandwidth by process using `/proc/net/tcp` and libpcap.
- **Key features:** Per-process network bandwidth (upload/download), real-time top-like display.
- **Difference:** Network only, interactive display, no data capture to file.

---

### 7.2 iftop
- **URL:** https://www.ex-parrot.com/pdw/iftop/
- **Language:** C
- **Description:** Shows network bandwidth grouped by source/destination host pairs. Does not show per-process breakdown.
- **Key features:** Per-connection bandwidth, host name resolution.
- **Difference:** Network only, host-pair level (not process level).

---

### 7.3 iotop
- **URL:** https://github.com/Tomas-M/iotop
- **Language:** C (rewrite of original Python version)
- **Description:** Top-like tool for disk I/O. Shows per-process disk read/write rates using kernel I/O accounting.
- **Key features:** Per-process disk I/O, real-time display, accumulated I/O counters.
- **Difference:** Disk I/O only, interactive display, no data capture.

---

### 7.4 dstat
- **URL:** https://github.com/dagwieers/dstat
- **Language:** Python
- **Description:** Versatile system statistics tool combining vmstat, iostat, netstat, and ifstat. Outputs columns of metrics to terminal, can write to CSV.
- **Key features:** CPU, disk, network, memory, system statistics; CSV output; pluggable.
- **Difference:** System-wide only (not per-process), no GPU. CSV output mode is useful for offline analysis.

---

## Category 8: ML Experiment Tracking Platforms with Resource Monitoring

These platforms include resource metric tracking as one feature among many.

---

### 8.1 Weights & Biases (W&B)
- **URL:** https://github.com/wandb/wandb
- **Language:** Python
- **Description:** ML experiment tracking platform with automatic system metric logging. Tracks GPU, CPU, memory, and network during training runs.
- **Key features:** Automatic system metric logging (GPU, CPU, RAM, network), experiment tracking, model registry, artifacts, collaborative dashboards.
- **Difference:** Primarily an ML experiment tracker. Resource monitoring is automatic and integrated but secondary to experiment logging. Requires W&B account (cloud-first, has open-source local server option).

---

### 8.2 MLflow
- **URL:** https://github.com/mlflow/mlflow
- **Language:** Python
- **Description:** Open-source ML lifecycle management. Does not natively log CPU/GPU metrics; requires external integration.
- **Key features:** Experiment tracking, model registry, deployment. No built-in system resource monitoring.
- **Difference:** No native resource tracking.

---

### 8.3 ClearML (see 1.20)

---

## Category 9: HPC Batch Job Monitoring

---

### 9.1 Jobstats
- **URL:** https://github.com/PrincetonUniversity/jobstats
- **Language:** Python + Prometheus stack
- **Description:** Slurm-compatible job monitoring platform for CPU and GPU clusters. Displays per-job CPU and GPU efficiency summaries using Prometheus, Grafana, and Slurm Prolog/Epilog hooks.
- **Key features:** Per-Slurm-job efficiency report (CPU utilization, memory, GPU utilization), compares requested vs. used resources, automatically stores data in Slurm AdminComment field.
- **Difference:** Slurm HPC specific. Requires full Prometheus + Grafana + Slurm infrastructure. Very close in concept to `resource-tracker` (per-job resource reports) but for HPC/Slurm, not general Python/R scripts.

---

### 9.2 Open XDMoD
- **URL:** https://open.xdmod.org/
- **Language:** PHP + Python
- **Description:** Open-source tool for analyzing HPC center usage and job efficiency. Tracks CPU, memory, GPU, and I/O for Slurm/PBS/SGE jobs.
- **Key features:** Job-level resource utilization reports, efficiency recommendations, web portal.
- **Difference:** HPC management tool. Requires full HPC stack. Not for general batch jobs.

---

## Category 10: R Language Profiling Tools

Resource-tracker explicitly supports R scripts. These are the closest R-ecosystem analogues.

---

### 10.1 profvis
- **URL:** https://github.com/rstudio/profvis
- **Language:** R
- **Description:** Interactive visualization of R code profiling data. Uses `Rprof()` to collect call stack samples and displays an interactive flame graph and memory timeline in a web browser.
- **Key features:** Interactive flame graph, memory timeline, line-level time attribution, RStudio integration.
- **Difference:** CPU + memory profiling for R code, developer-oriented. No disk, network, or GPU. No batch job wrapping or time-series operational logging.

---

### 10.2 bench
- **URL:** https://github.com/r-lib/bench
- **Language:** R
- **Description:** High-precision benchmarking for R with memory tracking.
- **Key features:** High-resolution timing, memory allocation tracking, comparison of multiple expressions.
- **Difference:** Benchmarking tool. No operational resource monitoring.

---

### 10.3 microbenchmark
- **URL:** https://github.com/joshuaulrich/microbenchmark
- **Language:** R
- **Description:** R package for sub-millisecond timing benchmarks.
- **Key features:** High-precision CPU timing.
- **Difference:** CPU timing only, micro-benchmarking specific.

---

### 10.4 profmem
- **URL:** https://github.com/HenrikBengtsson/profmem
- **Language:** R
- **Description:** Simple memory profiling for R expressions. Uses `tracemem`/R internals to log all memory allocations.
- **Key features:** Per-expression memory allocation log.
- **Difference:** Memory only, developer-oriented.

---

## Category 11: Python Standard Library / Built-in Profiling

---

### 11.1 cProfile / profile
- **URL:** https://docs.python.org/3/library/profile.html
- **Language:** Python (stdlib)
- **Description:** Python's built-in deterministic profiler. Records function call counts and cumulative time.
- **Key features:** Function-level timing, call count, cumulative/per-call time, pstats for analysis.
- **Difference:** CPU time only, function-level. No memory, GPU, disk, or network.

---

### 11.2 tracemalloc
- **URL:** https://docs.python.org/3/library/tracemalloc.html
- **Language:** Python (stdlib, since 3.4)
- **Description:** Traces Python memory allocations with tracebacks to allocation sites.
- **Key features:** Peak memory tracking, traceback to allocation sites, snapshot comparison.
- **Difference:** Python-managed memory only. No native/C allocations, no GPU/disk/network.

---

### 11.3 yappi
- **URL:** https://github.com/sumerc/yappi
- **Language:** Python + C
- **Description:** Yet Another Python Profiler. Supports both wall clock and CPU time, multi-threaded profiling, and async code.
- **Key features:** Wall + CPU time, multi-thread awareness, async support, pstats/callgrind output.
- **Difference:** CPU profiling only.

---

### 11.4 line_profiler
- **URL:** https://github.com/pyutils/line_profiler
- **Language:** Python + C
- **Description:** Line-by-line CPU time profiler for Python using `@profile` decorator.
- **Key features:** Line-level execution time, `@profile` decorator.
- **Difference:** CPU time only, requires decoration.

---

## Summary Comparison Table

| Tool                 | Lang   | CPU | Mem | GPU | Disk | Net | Batch-job wrap | Per-job report  | Workflow integration     | Output                       |
|----------------------|--------|-----|-----|-----|------|-----|----------------|-----------------|--------------------------|------------------------------|
| **resource-tracker** | Python | Y   | Y   | Y   | Y    | Y   | Y              | Y               | Metaflow, Flyte, Airflow | Metrics + card visualization |
| psutil               | Python | Y   | Y   | —   | Y    | Y   | —              | —               | —                        | Raw API                      |
| `memory_profiler`    | Python | —   | Y   | —   | —    | —   | Y (mprof)      | Y (plot)        | —                        | Plot + log                   |
| Scalene              | Python | Y   | Y   | Y   | —    | —   | Y (CLI)        | Y (web UI)      | —                        | Interactive web report       |
| Memray               | Python | —   | Y   | —   | —    | —   | Y (CLI)        | Y (flame graph) | —                        | Flame graphs                 |
| Fil                  | Python | —   | Y   | —   | —    | —   | Y (CLI)        | Y (flame graph) | —                        | Flame graph                  |
| pyinstrument         | Python | Y   | —   | —   | —    | —   | Y              | Y               | —                        | HTML/text                    |
| py-spy               | Rust   | Y   | —   | —   | —    | —   | Y (attach)     | Y (flame graph) | —                        | Flame graph                  |
| Austin               | C      | Y   | —   | —   | —    | —   | Y              | —               | —                        | Stack samples                |
| Glances              | Python | Y   | Y   | Y*  | Y    | Y   | —              | —               | —                        | TUI + web API                |
| nvitop               | Python | —   | —   | Y   | —    | —   | —              | —               | —                        | TUI + Python API             |
| gpustat              | Python | —   | —   | Y   | —    | —   | —              | —               | —                        | CLI display                  |
| CodeCarbon           | Python | Y*  | Y*  | Y*  | —    | —   | Y (decorator)  | Y (CSV)         | —                        | CO2 report                   |
| ClearML              | Python | Y   | Y   | Y   | —    | Y   | Y (auto)       | Y (web)         | ML frameworks            | Web dashboard                |
| below                | Rust   | Y   | Y   | —   | Y    | Y   | —              | —               | —                        | TUI + replay                 |
| samply               | Rust   | Y   | —   | —   | —    | —   | Y (subprocess) | Y (flame graph) | —                        | Firefox profiler             |
| Bytehound            | Rust   | —   | Y   | —   | —    | —   | Y (LD_PRELOAD) | Y (web GUI)     | —                        | Web GUI                      |
| atop                 | C      | Y   | Y   | —   | Y    | Y   | —              | —               | —                        | TUI + binary log             |
| sysstat/pidstat      | C      | Y   | Y   | —   | Y    | Y   | —              | —               | —                        | CLI + CSV                    |
| htop                 | C      | Y   | Y   | —   | Y    | Y   | —              | —               | —                        | TUI                          |
| btop++               | C++    | Y   | Y   | Y*  | Y    | Y   | —              | —               | —                        | TUI                          |
| Jobstats             | Python | Y   | Y   | Y   | —    | —   | Y* (Slurm)     | Y (Slurm)       | Slurm                    | CLI + DB                     |
| Pyroscope            | Go     | Y   | Y   | —   | —    | —   | Y (SDK)        | —               | —                        | Flame graphs                 |
| Parca                | Go     | Y   | Y   | —   | —    | —   | —              | —               | Kubernetes               | Icicle graphs                |
| perf                 | C      | Y   | —   | —   | Y    | —   | Y (subprocess) | —               | —                        | Raw perf data                |
| Valgrind             | C      | Y   | Y   | —   | —    | —   | Y (subprocess) | Y               | —                        | Text + GUI                   |
| nethogs              | C++    | —   | —   | —   | —    | Y   | —              | —               | —                        | TUI                          |
| iotop                | C      | —   | —   | —   | Y    | —   | —              | —               | —                        | TUI                          |
| PowerAPI             | Python | Y*  | Y*  | —   | —    | —   | —              | —               | —                        | Power estimates              |
| W&B                  | Python | Y   | Y   | Y   | —    | Y   | Y (auto)       | Y (web)         | ML frameworks            | Web dashboard                |
| Prometheus stack     | Go     | Y   | Y   | Y*  | Y    | Y   | —              | —               | Kubernetes               | Time-series DB               |

*Y* = partial/plugin-based support

---

## Key Findings for Rust CLI Implementation

Based on this landscape analysis, the following observations are most relevant to the planned Rust/Linux CLI implementation:

1. **No existing Rust tool covers the full feature set** of resource-tracker (CPU + memory + GPU + disk + network + batch job wrapping + per-job reporting). `below` (Rust) is the closest in scope but is a system-wide daemon, not a per-job wrapper.

2. **procfs is the right foundation for Linux.** The `/proc` filesystem is used by psutil, process-exporter, sysstat, and resource-tracker itself. A Rust implementation can use the `procfs` crate or read `/proc` directly with zero external dependencies.

3. **GPU support requires dynamic linking** (NVML via `libpynvml` or direct `libnvidia-ml.so`). This is a hard constraint noted in the SOW. The Rust NVML binding (nvidia-management-library crate or similar) will be needed.

4. **The Pushgateway integration** (Extra Component: S3 PUT) is unique to resource-tracker and not present in any comparable tool. This makes it particularly well-suited for cloud batch job environments.

5. **The decorator/wrapper pattern** (similar to `samply record ./program`) is present in py-spy, samply, Austin, and Fil — wrapping a subprocess is the right architectural pattern for a CLI tool.

6. **The closest functional analogues** (tools that wrap a job, collect multi-resource metrics, and produce a per-job report) are:
   - Scalene (Python, CPU+GPU+memory, developer-oriented)
   - memory_profiler (Python, memory only, has mprof)
   - Jobstats (HPC/Slurm specific)
   - resource-tracker itself (the reference implementation)

   None of these is in Rust, none covers all six resource dimensions (CPU, memory, GPU, VRAM, network, disk) in a single zero-dependency binary.

---

## Sources

- https://github.com/SpareCores/resource-tracker
- https://github.com/giampaolo/psutil
- https://github.com/pythonprofilers/memory_profiler
- https://github.com/plasma-umass/scalene
- https://github.com/bloomberg/memray
- https://github.com/pythonspeed/filprofiler
- https://github.com/joerick/pyinstrument
- https://github.com/benfred/py-spy
- https://github.com/P403n1x87/austin
- https://github.com/nicolargo/glances
- https://github.com/XuehaiPan/nvitop
- https://github.com/wookayin/gpustat
- https://github.com/gpuopenanalytics/pynvml
- https://github.com/mlco2/codecarbon
- https://github.com/lfwa/carbontracker
- https://github.com/powerapi-ng/pyRAPL
- https://github.com/powerapi-ng/pyJoules
- https://github.com/powerapi-ng/powerapi
- https://github.com/sb-ai-lab/eco2AI
- https://github.com/psf/pyperf
- https://github.com/clearml/clearml
- https://github.com/xybu/python-resmon
- https://github.com/htop-dev/htop
- https://github.com/aristocratos/btop
- https://github.com/aristocratos/bpytop
- https://github.com/aristocratos/bashtop
- https://github.com/Atoptool/atop
- https://github.com/sysstat/sysstat
- https://github.com/Syllo/nvtop
- https://github.com/MrRio/vtop
- https://github.com/netdata/netdata
- https://github.com/iovisor/bcc
- https://github.com/bpftrace/bpftrace
- https://github.com/parca-dev/parca
- https://github.com/grafana/pyroscope
- https://github.com/brendangregg/FlameGraph
- https://github.com/gperftools/gperftools
- https://valgrind.org/
- https://github.com/KDE/heaptrack
- https://github.com/google/perfetto
- https://github.com/async-profiler/async-profiler
- https://github.com/facebookincubator/below
- https://github.com/mstange/samply
- https://github.com/koute/bytehound
- https://github.com/tikv/pprof-rs
- https://github.com/prometheus/node_exporter
- https://github.com/prometheus/pushgateway
- https://github.com/ncabatoff/process-exporter
- https://github.com/google/cadvisor
- https://github.com/influxdata/telegraf
- https://github.com/kubernetes/kube-state-metrics
- https://opentelemetry.io/
- https://github.com/NVIDIA/DCGM
- https://github.com/NVIDIA/dcgm-exporter
- https://github.com/raboof/nethogs
- https://github.com/wandb/wandb
- https://github.com/mlflow/mlflow
- https://github.com/PrincetonUniversity/jobstats
- https://github.com/rstudio/profvis
- https://github.com/r-lib/bench
- https://github.com/sumerc/yappi
- https://github.com/pyutils/line_profiler
- https://github.com/msaroufim/awesome-profiling
- https://lambda.ai/blog/keeping-an-eye-on-your-gpus-2
- https://sparecores.com/article/metaflow-resource-tracker
- https://developers.facebook.com/blog/post/2021/09/21/below-time-travelling-resource-monitoring-tool/
