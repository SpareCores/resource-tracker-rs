#![doc = include_str!("../README.md")]

mod collector;
mod config;
mod metrics;
mod output;
mod sentinel;

extern crate libc;

use collector::{
    collect_host_info, spawn_cloud_discovery, CpuCollector, DiskCollector, GpuCollector,
    MemoryCollector, NetworkCollector,
};
use config::{Config, OutputFormat};
use metrics::Sample;
use sentinel::{
    close_run, samples_to_csv, start_run, BatchUploader, RunContext, SentinelClient,
};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// SIGTERM handler
// ---------------------------------------------------------------------------

static SIGTERM_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigterm(_: libc::c_int) {
    SIGTERM_RECEIVED.store(true, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Graceful shutdown
// ---------------------------------------------------------------------------

/// Flush remaining samples, close the Sentinel run, then exit.
///
/// Called on both shell-wrapper child exit and SIGTERM.  Replaces the former
/// bare `std::process::exit()` calls so the upload thread always gets a chance
/// to flush.
fn shutdown(
    exit_code:     i32,
    sentinel:      Option<&SentinelClient>,
    run_ctx:       Option<Arc<Mutex<RunContext>>>,
    shutdown_flag: Option<Arc<AtomicBool>>,
    upload_handle: Option<std::thread::JoinHandle<Vec<String>>>,
    remaining:     Vec<Sample>,
    interval_secs: u64,
) -> ! {
    if let (Some(client), Some(ctx_arc), Some(flag), Some(handle)) =
        (sentinel, run_ctx, shutdown_flag, upload_handle)
    {
        // Signal the upload thread to flush and stop, then wait for it.
        flag.store(true, Ordering::Relaxed);
        let uploaded_uris = handle.join().unwrap_or_default();

        // Any samples collected after the last batch upload go as local CSV.
        let remaining_csv = if !remaining.is_empty() {
            Some(samples_to_csv(&remaining, interval_secs))
        } else {
            None
        };

        let ctx = ctx_arc.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = close_run(
            &client.agent,
            &client.api_base,
            &client.token,
            &ctx,
            Some(exit_code),
            &uploaded_uris,
            remaining_csv,
        ) {
            eprintln!("warn: sentinel close_run failed: {e}");
        }
    }

    std::process::exit(exit_code);
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    // Install SIGTERM handler so the binary can flush before exiting.
    unsafe { libc::signal(libc::SIGTERM, handle_sigterm as *const () as libc::sighandler_t); }

    let mut config = Config::load();

    // -----------------------------------------------------------------------
    // Output sink: stdout (default), file (--output), or suppressed (--quiet).
    // Warnings and errors always go to stderr via eprintln! regardless.
    // -----------------------------------------------------------------------
    let mut out_file: Option<std::io::BufWriter<std::fs::File>> =
        if config.quiet {
            None
        } else {
            config.output_file.as_deref().map(|path| {
                std::io::BufWriter::new(
                    std::fs::File::create(path).unwrap_or_else(|e| {
                        eprintln!("error: cannot open output file {path}: {e}");
                        std::process::exit(1);
                    })
                )
            })
        };

    // Emit one line of metric output to the selected sink.
    // quiet=true  -> no-op
    // output_file -> write to file and flush (so `tail -f` works)
    // default     -> println! to stdout
    macro_rules! emit {
        ($($arg:tt)*) => {
            if !config.quiet {
                if let Some(ref mut f) = out_file {
                    let _ = writeln!(f, $($arg)*);
                    let _ = f.flush();
                } else {
                    println!($($arg)*);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Shell-wrapper mode: spawn the command and track its PID automatically.
    // -----------------------------------------------------------------------
    let mut child = if !config.command.is_empty() {
        let (program, args) = config.command.split_first().expect("command is non-empty");
        match std::process::Command::new(program).args(args).spawn() {
            Ok(c) => {
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

    let mut cpu     = CpuCollector::new(config.pid);
    let memory      = MemoryCollector::new();
    let mut network = NetworkCollector::new();
    let mut disk    = DiskCollector::new();
    let gpu         = GpuCollector::new();

    // Collect static GPU info now so host discovery can derive GPU host fields.
    let initial_gpus = gpu.collect().unwrap_or_default();

    // Host discovery: fast, local, no I/O.
    let host_info = collect_host_info(&initial_gpus);

    // Cloud discovery: spawn before warm-up so probes run concurrently.
    let cloud_handle = spawn_cloud_discovery();

    // Warm-up: prime delta state in stateful collectors, then sleep one full
    // interval so the first real sample has meaningful rates.
    let _ = cpu.collect();
    let _ = network.collect();
    let _ = disk.collect();
    std::thread::sleep(interval);

    // Cloud probes are bounded by 2s each; they are done by now.
    let cloud_info = cloud_handle.join().unwrap_or_default();

    // -----------------------------------------------------------------------
    // Sentinel API setup (gated on SENTINEL_API_TOKEN being set).
    // -----------------------------------------------------------------------
    let sentinel = SentinelClient::from_env();

    let (run_ctx_arc, sample_buffer, upload_shutdown_flag, upload_handle) =
        match &sentinel {
            None => (None, None, None, None),
            Some(client) => {
                match start_run(
                    &client.agent,
                    &client.api_base,
                    &client.token,
                    &config.metadata,
                    config.pid,
                    &host_info,
                    &cloud_info,
                ) {
                    Err(e) => {
                        eprintln!("warn: sentinel start_run failed: {e}; streaming disabled");
                        (None, None, None, None)
                    }
                    Ok(ctx) => {
                        let ctx_arc = Arc::new(Mutex::new(ctx));
                        let upload_interval =
                            std::env::var("TRACKER_UPLOAD_INTERVAL")
                                .ok()
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(60u64);
                        let (uploader, buf) =
                            BatchUploader::new(upload_interval, config.interval_secs);
                        let flag   = uploader.shutdown_flag();
                        let handle = uploader.spawn(
                            Arc::clone(&ctx_arc),
                            client.agent.clone(),
                            client.api_base.clone(),
                            client.token.clone(),
                        );
                        (Some(ctx_arc), Some(buf), Some(flag), Some(handle))
                    }
                }
            }
        };

    // Emit CSV header once before the loop.
    if config.format == OutputFormat::Csv {
        emit!("{}", output::csv::csv_header());
    }

    // Samples collected since the last S3 batch upload (for local fallback).
    let mut unflushed: Vec<Sample> = Vec::new();

    // -----------------------------------------------------------------------
    // Main sampling loop
    // -----------------------------------------------------------------------
    loop {
        let timestamp_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let sample = Sample {
            timestamp_secs,
            job_name:    config.metadata.job_name.clone(),
            tracked_pid: config.pid,
            cpu:         cpu.collect().unwrap_or_default(),
            memory:      memory.collect().unwrap_or_default(),
            network:     network.collect().unwrap_or_default(),
            disk:        disk.collect().unwrap_or_default(),
            gpu:         gpu.collect().unwrap_or_default(),
        };

        // Emit to selected output sink.
        match config.format {
            OutputFormat::Json => {
                match serde_json::to_value(&sample) {
                    Ok(mut v) => {
                        v[format!("{}-version", env!("CARGO_PKG_NAME"))] =
                            serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string());
                        emit!("{}", v);
                    }
                    Err(e) => eprintln!("warn: json serialize error: {e}"),
                }
            }
            OutputFormat::Csv => {
                emit!("{}", output::csv::sample_to_csv_row(&sample, config.interval_secs));
            }
        }

        // Push to sentinel buffer (if streaming is active).
        if let Some(ref buf) = sample_buffer {
            buf.lock().unwrap_or_else(|e| e.into_inner()).push(sample.clone());
        }
        unflushed.push(sample);

        // -----------------------------------------------------------------------
        // Shell-wrapper exit check
        // -----------------------------------------------------------------------
        if let Some(ref mut c) = child {
            match c.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code().unwrap_or(1);
                    shutdown(
                        code,
                        sentinel.as_ref(),
                        run_ctx_arc,
                        upload_shutdown_flag,
                        upload_handle,
                        unflushed,
                        config.interval_secs,
                    );
                }
                Ok(None) => {}
                Err(e) => eprintln!("warn: error checking child status: {e}"),
            }
        }

        // SIGTERM received: flush and exit cleanly.
        if SIGTERM_RECEIVED.load(Ordering::Relaxed) {
            shutdown(
                0,
                sentinel.as_ref(),
                run_ctx_arc,
                upload_shutdown_flag,
                upload_handle,
                unflushed,
                config.interval_secs,
            );
        }

        std::thread::sleep(interval);
    }
}
