#![doc = include_str!("../README.md")]

mod collector;
mod config;
mod metrics;
mod output;

use collector::{
    collect_host_info, spawn_cloud_discovery, CpuCollector, DiskCollector, GpuCollector,
    MemoryCollector, NetworkCollector,
};
use config::{Config, OutputFormat};
use metrics::Sample;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn main() {
    let mut config = Config::load();

    // -----------------------------------------------------------------------
    // Shell-wrapper mode: spawn the command and track its PID automatically.
    // -----------------------------------------------------------------------
    let mut child = if !config.command.is_empty() {
        let (program, args) = config.command.split_first().expect("command is non-empty");
        match std::process::Command::new(program).args(args).spawn() {
            Ok(c) => {
                // Child PID overrides any explicit --pid flag.
                config.pid = Some(i32::try_from(c.id()).unwrap_or(i32::MAX));
                Some(c)
            }
            Err(e) => {
                eprintln!("error: failed to spawn {:?}: {e}", program);
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    let interval = Duration::from_secs(config.interval_secs);

    let mut cpu = CpuCollector::new(config.pid);
    let memory = MemoryCollector::new();
    let mut network = NetworkCollector::new();
    let mut disk = DiskCollector::new();
    let gpu = GpuCollector::new();

    // Collect static GPU info now so host discovery can derive GPU host fields.
    let initial_gpus = gpu.collect().unwrap_or_default();

    // Host discovery: fast, local, no I/O — run synchronously before the loop.
    let host_info = collect_host_info(&initial_gpus);

    // Cloud discovery: network I/O with up to 2s per probe.  Spawn in the
    // background so it runs concurrently with the warm-up sleep and does not
    // delay the first sample.
    let cloud_handle = spawn_cloud_discovery();

    // Warm-up: prime delta state in stateful collectors, then sleep one full
    // interval so the first real sample has meaningful rates.
    let _ = cpu.collect();
    let _ = network.collect();
    let _ = disk.collect();
    std::thread::sleep(interval);

    // Cloud discovery should be done by now (bounded by the IMDS timeout ≤ 2s
    // per provider, and we slept for at least one interval).  Join it here.
    let cloud_info = cloud_handle.join().unwrap_or_default();

    // `host_info` and `cloud_info` are available for the Sentinel API
    // registration payload (Priority 4). Log to stderr in debug mode only.
    let _ = (&host_info, &cloud_info);

    // Print CSV header once before the loop.
    if config.format == OutputFormat::Csv {
        println!("{}", output::csv::csv_header());
    }

    loop {
        let timestamp_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let sample = Sample {
            timestamp_secs,
            job_name: config.metadata.job_name.clone(),
            // Collector failures surface as zero/empty values rather than panics.
            cpu:     cpu.collect().unwrap_or_default(),
            memory:  memory.collect().unwrap_or_default(),
            network: network.collect().unwrap_or_default(),
            disk:    disk.collect().unwrap_or_default(),
            gpu:     gpu.collect().unwrap_or_default(),
        };

        match config.format {
            OutputFormat::Json => {
                match serde_json::to_value(&sample) {
                    Ok(mut v) => {
                        v[format!("{}-version", env!("CARGO_PKG_NAME"))] =
                            serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string());
                        println!("{}", v);
                    }
                    Err(e) => eprintln!("warn: json serialize error: {e}"),
                }
            }
            OutputFormat::Csv => {
                println!(
                    "{}",
                    output::csv::sample_to_csv_row(&sample, config.interval_secs)
                );
            }
        }

        // In shell-wrapper mode: check if the child has exited. If so, emit
        // one final sample (already done above), then exit with its code.
        if let Some(ref mut c) = child {
            match c.try_wait() {
                Ok(Some(status)) => {
                    std::process::exit(status.code().unwrap_or(1));
                }
                Ok(None) => {} // still running
                Err(e) => eprintln!("warn: error checking child status: {e}"),
            }
        }

        std::thread::sleep(interval);
    }
}
