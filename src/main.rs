#![doc = include_str!("../README.md")]

mod collector;
mod config;
mod metrics;
mod output;

use collector::{CpuCollector, DiskCollector, GpuCollector, MemoryCollector, NetworkCollector};
use config::{Config, OutputFormat};
use metrics::Sample;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn main() {
    let config = Config::load();

    let interval = Duration::from_secs(config.interval_secs);

    let mut cpu = CpuCollector::new(config.pid);
    let memory = MemoryCollector::new();
    let mut network = NetworkCollector::new();
    let mut disk = DiskCollector::new();
    let gpu = GpuCollector::new();

    // Warm-up: prime delta state in all stateful collectors, then sleep one
    // full interval so the first real sample has meaningful rates.
    let _ = cpu.collect();
    let _ = network.collect();
    let _ = disk.collect();
    std::thread::sleep(interval);

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
            job_name: config.job_name.clone(),
            cpu: cpu.collect().expect("cpu collect failed"),
            memory: memory.collect().expect("memory collect failed"),
            network: network.collect().expect("network collect failed"),
            disk: disk.collect().expect("disk collect failed"),
            gpu: gpu.collect().expect("gpu collect failed"),
        };

        match config.format {
            OutputFormat::Json => {
                let mut v = serde_json::to_value(&sample).expect("json serialize failed");
                v[format!("{}-version", env!("CARGO_PKG_NAME"))] =
                    serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string());
                println!("{}", v);
            }
            OutputFormat::Csv => {
                println!(
                    "{}",
                    output::csv::sample_to_csv_row(&sample, config.interval_secs)
                );
            }
        }

        std::thread::sleep(interval);
    }
}
