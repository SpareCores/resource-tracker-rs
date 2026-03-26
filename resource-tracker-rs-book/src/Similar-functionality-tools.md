# Open-Source Tools with Similar Functionality to `resource-tracker`

[`resource-tracker`](https://github.com/SpareCores/resource-tracker) is a lightweight, zero-dependency Python package for monitoring CPU, memory, GPU, network, and disk utilization across processes and at the system level, designed for batch jobs (Python/R scripts, Metaflow steps), with decorator-based workflow integration and per-job visualization reports.

The tools below are organized into meaningful categories. No single open-source tool matches all of resource-tracker's characteristics simultaneously — most are either too narrow (single metric), too heavy (infrastructure daemons), or not batch-job oriented.

---

## Category 1: Python Libraries for Process/System Resource Monitoring
*(Closest functional analogues)*

| Tool                                                                   | Notes                                                                                                       | Details                                                                     |
|------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------|
| [psutil](https://github.com/giampaolo/psutil)                          | The foundational building block used by resource-tracker itself. Raw API only, no tracking loop or reports. | Linux; no CLI; CPU/Mem/Disk/Net/Process; no batch wrap; no report           |
| [memory\_profiler](https://github.com/pythonprofilers/memory_profiler) | Line-by-line memory, `@profile` decorator, `mprof plot`. No CPU/GPU/disk/network.                           | Linux; CLI (mprof); Memory; batch wrap (mprof CLI); report (plot)           |
| [Scalene](https://github.com/plasma-umass/scalene)                     | High-precision line-level profiler with AI optimization suggestions. No disk/network. Developer profiler.   | Linux; CLI; CPU/GPU/Mem; batch wrap (CLI); report (web UI)                  |
| [Memray](https://github.com/bloomberg/memray)                          | Bloomberg. Tracks every allocation including C/C++. No CPU/GPU/disk/network.                                | Linux; CLI; Memory; batch wrap (CLI); report (flame graphs)                 |
| [Fil](https://github.com/pythonspeed/filprofiler)                      | Peak memory focus for data scientists (NumPy/Pandas). Written in Rust+Python. Linux/macOS only.             | Linux; CLI; Memory (peak); batch wrap (CLI); report (flame graph)           |
| [pyinstrument](https://github.com/joerick/pyinstrument)                | Context manager + decorator. 1ms sampling. No memory/GPU/disk/network.                                      | Linux; CLI; CPU; batch wrap; report                                         |
| [py-spy](https://github.com/benfred/py-spy)                            | Written in Rust. Attaches to a running process. No memory/GPU/disk/network.                                 | Linux; CLI; CPU; batch wrap (attach); report (flame graph)                  |
| [Austin](https://github.com/P403n1x87/austin)                          | Pure C, extremely low overhead CPython frame stack sampler.                                                 | Linux; CLI; CPU; batch wrap; no report                                      |
| [Glances](https://github.com/nicolargo/glances)                        | Full system monitor with REST API, web UI, and exporters. Long-running daemon, not a batch-job wrapper.     | Linux; CLI; CPU/Mem/Disk/Net/GPU; no batch wrap; no report                  |
| [nvitop](https://github.com/XuehaiPan/nvitop)                          | Best GPU process viewer. Has programmatic `ResourceMetricCollector` API. No CPU/mem/disk/net.               | Linux; CLI; NVIDIA GPU; no batch wrap; no report                            |
| [gpustat](https://github.com/wookayin/gpustat)                         | Simple NVIDIA GPU status CLI. No time-series logging.                                                       | Linux; CLI; NVIDIA GPU; no batch wrap; no report                            |
| [pynvml / nvidia-ml-py](https://github.com/gpuopenanalytics/pynvml)    | Python NVML bindings. Building block only.                                                                  | Linux; no CLI; GPU (raw API); no batch wrap; no report                      |
| [CodeCarbon](https://github.com/mlco2/codecarbon)                      | `@track_emissions` decorator. CO2/energy focus, not utilization %. No disk/network.                         | Linux; partial CLI; CPU/Mem/GPU energy; batch wrap (decorator); report (CSV + dashboard) |
| [CarbonTracker](https://github.com/lfwa/carbontracker)                 | Predicts carbon footprint, can halt training. ML training specific.                                         | Linux; no CLI; CPU/GPU energy; batch wrap; report                           |
| [pyRAPL](https://github.com/powerapi-ng/pyRAPL)                        | Intel RAPL via `/sys/class/powercap`. Intel CPUs only. Energy joules, not utilization %.                    | Linux only; no CLI; CPU/DRAM energy; batch wrap (decorator); no report      |
| [pyJoules](https://github.com/powerapi-ng/pyJoules)                    | Multi-device energy (Intel RAPL + NVML). Context manager and decorator.                                     | Linux only; no CLI; CPU/DRAM/GPU energy; batch wrap (decorator); no report  |
| [PowerAPI](https://github.com/powerapi-ng/powerapi)                    | Framework for software-defined power meters. Process/container/VM granularity. Complex setup.               | Linux only; partial CLI; CPU/Mem power; no batch wrap; no report            |
| [eco2AI](https://github.com/sb-ai-lab/eco2AI)                          | ML training focused CO2 tracking.                                                                           | Linux; no CLI; CPU/GPU/RAM energy; batch wrap (decorator); report (CSV)     |
| [pyperf](https://github.com/psf/pyperf)                                | PSF benchmarking toolkit. `--track-memory` and `--tracemalloc` options. Not an operational monitor.         | Linux; CLI; Memory (benchmarks); batch wrap; report                         |
| [ClearML](https://github.com/clearml/clearml)                          | Full MLOps platform. Auto-logs system metrics. Requires ClearML server.                                     | Linux; CLI; CPU/Mem/GPU/Net; auto batch wrap; report (web UI)               |
| [python-resmon](https://github.com/xybu/python-resmon)                 | Lightweight script outputting CSV. System-level only, no per-process or GPU tracking.                       | Linux; CLI; CPU/Mem/Disk/Net; no batch wrap; report (CSV)                   |
| [yappi](https://github.com/sumerc/yappi)                               | CPU + wall time profiler with multi-thread and async support.                                               | Linux; no CLI; CPU; batch wrap; report                                      |
| [line\_profiler](https://github.com/pyutils/line_profiler)             | Line-by-line CPU time. No memory/GPU/disk/network.                                                          | Linux; CLI (kernprof); CPU; batch wrap (@profile); report                   |

---

## Category 2: Interactive Terminal System Monitors
*(Real-time visual monitoring; do not produce per-job reports or integrate with batch workflows)*

| Tool                                                        | Notes                                                                           | Details                                   |
|-------------------------------------------------------------|---------------------------------------------------------------------------------|-------------------------------------------|
| [htop](https://github.com/htop-dev/htop)                    | Interactive process viewer; no data capture                                     | C; Linux; CLI; CPU/Mem/Proc               |
| [btop++](https://github.com/aristocratos/btop)              | Most modern TUI monitor; GPU via plugins                                        | C++; Linux; CLI; CPU/Mem/Disk/Net/GPU     |
| [bpytop](https://github.com/aristocratos/bpytop)            | Predecessor to btop++                                                           | Python; Linux; CLI; CPU/Mem/Disk/Net      |
| [bashtop](https://github.com/aristocratos/bashtop)          | Predecessor to bpytop                                                           | Bash; Linux; CLI; CPU/Mem/Disk/Net        |
| [atop](https://github.com/Atoptool/atop)                    | Writes persistent binary logs; replay mode; strong process-level detail         | C; Linux only; CLI; CPU/Mem/Disk/Net/Proc |
| [nmon](http://nmon.sourceforge.net/)                        | CSV capture mode for offline analysis; primarily Linux/AIX                      | C; Linux; CLI; CPU/Mem/Disk/Net           |
| [collectl](http://collectl.sourceforge.net/)                | Wide metric coverage; daemon or one-shot mode                                   | Perl; Linux only; CLI; CPU/Mem/Disk/Net   |
| [sysstat (sar/pidstat)](https://github.com/sysstat/sysstat) | `pidstat` for per-process; `sadf` for JSON/CSV/XML export; schedulable via cron | C; Linux only; CLI; CPU/Mem/Disk/Net/Proc |
| [nvtop](https://github.com/Syllo/nvtop)                     | AMD, Apple, Intel, NVIDIA, Qualcomm support; interactive GPU monitor            | C; Linux; CLI; GPU (multi-vendor)         |
| [vtop](https://github.com/MrRio/vtop)                       | Node.js, Unicode charts                                                         | JS; Linux; CLI; CPU/Mem/Proc              |
| [Netdata](https://github.com/netdata/netdata)               | 76k+ GitHub stars. Per-second metrics, web UI, ML anomaly detection             | C; Linux; CLI; all (800+ plugins)         |

---

## Category 3: eBPF / Kernel Tracing Tools
*(Zero-overhead kernel-level observability; require root + Linux kernel 4.1+)*

| Tool                                                        | Notes                                                                               | Details                    |
|-------------------------------------------------------------|-------------------------------------------------------------------------------------|----------------------------|
| [BCC](https://github.com/iovisor/bcc)                       | Toolkit for writing eBPF programs; 70+ ready-made tools                             | C/Python/Lua; Linux only; CLI |
| [bpftrace](https://github.com/bpftrace/bpftrace)            | DTrace-like one-liners for eBPF; ad-hoc analysis                                    | C++ DSL; Linux only; CLI   |
| [Parca + Parca Agent](https://github.com/parca-dev/parca)   | Continuous eBPF-based CPU profiling; pprof format; <1% overhead                     | Go; Linux only; CLI        |
| [Pyroscope (Grafana)](https://github.com/grafana/pyroscope) | Continuous profiling database + eBPF agent; multi-language SDK; Grafana integration | Go; Linux only; CLI        |

---

## Category 4: Native C/C++ Profiling Tools

| Tool                                                               | Notes                                                                | Details                                           |
|--------------------------------------------------------------------|----------------------------------------------------------------------|---------------------------------------------------|
| [perf (Linux perf\_events)](https://perfwiki.github.io/main/)      | Foundation for many other tools; hardware counter sampling           | C (kernel); Linux only; CLI; CPU/kernel events    |
| [FlameGraph](https://github.com/brendangregg/FlameGraph)           | Visualizes perf/DTrace output as SVG flame graphs                    | Perl; Linux; CLI; visualization                   |
| [gperftools](https://github.com/gperftools/gperftools)             | Google Performance Tools: CPU profiler, heap profiler, TCMalloc      | C++; Linux; partial CLI (pprof); CPU/Memory       |
| [Valgrind / Massif](https://valgrind.org/)                         | High-overhead instrumentation; Massif=heap profiler; 10–50× slowdown | C; Linux; CLI; CPU/Memory                         |
| [Heaptrack](https://github.com/KDE/heaptrack)                      | KDE; faster alternative to Valgrind/Massif for heap profiling        | C++; Linux only; CLI; Memory                      |
| [Perfetto](https://github.com/google/perfetto)                     | Google; default Android profiler; SQL-queryable traces; browser UI   | C++; Linux; CLI; CPU/Mem/GPU/Disk/Sched           |
| [async-profiler](https://github.com/async-profiler/async-profiler) | Low-overhead JVM profiler; flame graphs; JVM only                    | C (JVM agent); Linux; CLI (asprof); CPU/Heap      |
| [TAU](https://www.cs.uoregon.edu/research/tau/)                    | HPC parallel profiling suite; complex setup                          | C++; Linux; CLI; CPU/GPU/MPI                      |
| [HPCToolkit](https://hpctoolkit.org/)                              | HPC sampling profiler; 1–5% overhead; supercomputer use              | C/C++; Linux; CLI; CPU/GPU                        |

---

## Category 5: Rust Tools

| Tool                                                | Notes                                                                                                                                                                              | Details              |
|-----------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|----------------------|
| [below](https://github.com/facebookincubator/below) | Facebook/Meta. Time-traveling system monitor with cgroup/PSI support; record+replay mode. System-wide daemon, not a batch-job wrapper. Architecturally most relevant Rust project. | Linux only; CLI      |
| [samply](https://github.com/mstange/samply)         | Sampling CPU profiler; wraps a subprocess (`samply record ./program`); uses Linux perf events; Firefox Profiler UI. CPU only.                                                      | Linux; CLI           |
| [Bytehound](https://github.com/koute/bytehound)     | Heap memory profiler; LD_PRELOAD-based; multi-arch (AMD64, ARM, AArch64, MIPS64); web-based GUI. Memory only.                                                                      | Linux only; CLI      |
| [pprof-rs](https://github.com/tikv/pprof-rs)        | CPU profiler for Rust programs using backtrace-rs; pprof output format. Library only.                                                                                              | Linux; no CLI        |

---

## Category 6: Infrastructure Metrics Collection (Daemons & Exporters)
*(Not batch-job wrappers; relevant for pipeline integration and metric output targets)*

| Tool                                                                     | Notes                                                                                                                  | Details                      |
|--------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------|------------------------------|
| [Prometheus node\_exporter](https://github.com/prometheus/node_exporter) | System-level Prometheus exporter; `/proc`-based                                                                        | Go; Linux; CLI               |
| [Prometheus Pushgateway](https://github.com/prometheus/pushgateway)      | Allows batch jobs to push metrics to Prometheus; standard solution for short-lived jobs                                | Go; Linux; CLI               |
| [process-exporter](https://github.com/ncabatoff/process-exporter)        | Per-process-group Prometheus metrics from `/proc`                                                                      | Go; Linux only; CLI          |
| [cAdvisor](https://github.com/google/cadvisor)                           | Container resource usage and performance; Prometheus exporter                                                          | Go; Linux only; CLI          |
| [Telegraf](https://github.com/influxdata/telegraf)                       | Plugin-driven metrics agent; 300+ inputs; InfluxDB backend                                                             | Go; Linux; CLI               |
| [OpenTelemetry](https://opentelemetry.io/)                               | CNCF standard for traces/metrics/logs; structured output for jobs                                                      | Multi-lang; Linux; CLI (otelcol) |
| [NVIDIA DCGM + dcgm-exporter](https://github.com/NVIDIA/DCGM)            | GPU telemetry for Kubernetes/data center; Prometheus exporter                                                          | C/Go; Linux only; CLI        |
| [kube-state-metrics](https://github.com/kubernetes/kube-state-metrics)   | Kubernetes object state metrics for Prometheus                                                                         | Go; Linux; CLI               |
| [Jobstats (HPC)](https://github.com/PrincetonUniversity/jobstats)        | Slurm-compatible per-job efficiency reports (CPU+GPU). Conceptually very close to resource-tracker but Slurm-specific. | Python; Linux only; CLI      |

---

## Category 7: Per-Process Network and Disk I/O Monitors

| Tool                                          | Notes                                                         | Details              |
|-----------------------------------------------|---------------------------------------------------------------|----------------------|
| [nethogs](https://github.com/raboof/nethogs)  | Per-process network bandwidth using `/proc/net/tcp` + libpcap | C++; Linux only; CLI |
| [iftop](https://www.ex-parrot.com/pdw/iftop/) | Per-connection (not per-process) bandwidth monitor            | C; Linux; CLI        |
| [iotop](https://github.com/Tomas-M/iotop)     | Per-process disk I/O using kernel I/O accounting              | C; Linux only; CLI   |
| [dstat](https://github.com/dagwieers/dstat)   | System-wide CPU+disk+network+memory with CSV output           | Python; Linux only; CLI |

---

## Category 8: ML Experiment Tracking with Resource Monitoring

| Tool                                               | Notes                                                                                  | Details              |
|----------------------------------------------------|----------------------------------------------------------------------------------------|----------------------|
| [Weights & Biases](https://github.com/wandb/wandb) | Auto-logs GPU, CPU, memory, network during training runs; cloud-first; rich dashboards | Linux; CLI (wandb)   |
| [ClearML](https://github.com/clearml/clearml)      | Open-source MLOps platform; auto-logs GPU+CPU+memory+network; requires ClearML server  | Linux; CLI           |
| [MLflow](https://github.com/mlflow/mlflow)         | Experiment tracking but no native system resource monitoring                           | Linux; CLI (mlflow)  |

---

## Category 9: R Language Profiling

| Tool                                                             | Notes                                                                               | Details              |
|------------------------------------------------------------------|-------------------------------------------------------------------------------------|----------------------|
| [profvis](https://github.com/rstudio/profvis)                    | Interactive R profiling visualization; CPU + memory timeline; used within R session | Linux; R session only |
| [bench](https://github.com/r-lib/bench)                          | Benchmarking with memory tracking; used within R session                            | Linux; R session only |
| [microbenchmark](https://github.com/joshuaulrich/microbenchmark) | Micro-benchmarking tool; used within R session                                      | Linux; R session only |
| [profmem](https://github.com/HenrikBengtsson/profmem)            | Memory allocation tracing for R expressions; used within R session                  | Linux; R session only |

---

## Category 10: Python Standard Library Profiling Tools

| Tool                                                                 | Notes                                                                                       | Details                            |
|----------------------------------------------------------------------|---------------------------------------------------------------------------------------------|------------------------------------|
| [cProfile / profile](https://docs.python.org/3/library/profile.html) | Function-level CPU time; stdlib                                                             | Linux; CLI (python -m cProfile)    |
| [tracemalloc](https://docs.python.org/3/library/tracemalloc.html)    | Python memory allocation tracing with tracebacks; stdlib since Python 3.4; used within code | Linux; no CLI (used within code)   |

---

## Summary: Key Differentiators of `resource-tracker`

The table below highlights what makes resource-tracker stand out relative to the landscape:

| Feature                         | resource-tracker | Most profilers | System monitors | ML trackers |
|---------------------------------|------------------|----------------|-----------------|-------------|
| CPU + Memory + GPU + Disk + Net | All 5            | Usually 1–2    | All 5           | CPU+Mem+GPU |
| Batch-job / script wrapper      | Yes              | Yes            | No (daemons)    | Yes         |
| Zero runtime dependencies       | Yes              | Varies         | No              | No          |
| Per-job visual report / card    | Yes              | Often          | No              | Yes (cloud) |
| Workflow integration (Metaflow) | Yes              | No             | No              | Varies      |
| Cloud instance recommendations  | Yes              | No             | No              | No          |
| Lightweight process footprint   | Yes              | Yes            | No              | No          |
| Process-level granularity       | Yes              | Yes            | Partial         | No          |
| Runs on Linux                   | Yes              | Yes            | Yes             | Yes         |
| CLI invocation                  | Yes              | Yes (most)     | Yes             | Yes         |
