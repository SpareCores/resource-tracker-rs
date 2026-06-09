#![doc = include_str!("../README.md")]

#[cfg(not(target_os = "linux"))]
compile_error!(
    "resource-tracker only supports Linux; /proc and cgroup interfaces are Linux-specific."
);

mod collector;
mod config;
mod metrics;
mod output;
mod sentinel;
mod thread_util;

extern crate libc;

use collector::{
    CpuCollector, DiskCollector, GpuCollector, MemoryCollector, NetworkCollector,
    collect_host_info, spawn_cloud_discovery,
};
use config::{Config, OutputFormat};
use metrics::CloudInfo;
use metrics::Sample;
use sentinel::{BatchUploader, RunContext, SentinelClient, close_run, samples_to_csv, start_run};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    exit_code: i32,
    sentinel: Option<&SentinelClient>,
    run_ctx: Option<Arc<Mutex<RunContext>>>,
    shutdown_flag: Option<Arc<AtomicBool>>,
    upload_handle: Option<std::thread::JoinHandle<Vec<String>>>,
    remaining: Vec<Sample>,
    interval_secs: u64,
) -> ! {
    if let (Some(client), Some(ctx_arc), Some(flag), Some(handle)) =
        (sentinel, run_ctx, shutdown_flag, upload_handle)
    {
        // Signal the upload thread to flush its buffer to S3, then wait for it.
        // The thread performs one final S3 upload of any remaining buffered samples
        // before it exits, and returns the list of all successfully uploaded URIs.
        flag.store(true, Ordering::Relaxed);
        let uploaded_uris = handle.join().unwrap_or_default();

        // Route selection:
        //   S3 route   -- at least one batch was uploaded; uploaded_uris is non-empty.
        //                 The final flush is already included in uploaded_uris.
        //   Inline route -- no S3 uploads (short run or all S3 failures); send all
        //                   collected samples as a raw CSV string.
        let remaining_csv = if uploaded_uris.is_empty() && !remaining.is_empty() {
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
            remaining_csv,
            &uploaded_uris,
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
    // Install SIGTERM and SIGINT handlers so the binary can flush before exiting.
    // Both signals set the same flag and trigger the same graceful shutdown path.
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_sigterm as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            handle_sigterm as *const () as libc::sighandler_t,
        );
    }

    let mut config = Config::load();

    // -----------------------------------------------------------------------
    // Output sink: stdout (default), file (--output), or suppressed (--quiet).
    // Warnings and errors always go to stderr via eprintln! regardless.
    // -----------------------------------------------------------------------
    let mut out_file: Option<std::io::BufWriter<std::fs::File>> = if config.quiet {
        None
    } else {
        config.output_file.as_deref().map(|path| {
            std::io::BufWriter::new(std::fs::File::create(path).unwrap_or_else(|e| {
                eprintln!("error: cannot open output file {path}: {e}");
                std::process::exit(1);
            }))
        })
    };

    // Emit one line of metric output to the selected sink.
    // quiet=true  -> no-op
    // output_file -> write to file and flush (so `tail -f` works)
    // default     -> eprintln! to stderr (keeps stdout clean for the tracked app)
    macro_rules! emit {
        ($($arg:tt)*) => {
            if !config.quiet {
                if let Some(ref mut f) = out_file {
                    let _ = writeln!(f, $($arg)*);
                    let _ = f.flush();
                } else {
                    eprintln!($($arg)*);
                }
            }
        }
    }

    let interval = Duration::from_secs(config.interval_secs);

    // Shell-wrapper child is spawned after warm-up so cloud IMDS probes (ureq may
    // use helper threads) do not race with fork-heavy stressors under PID limits.
    let mut child: Option<std::process::Child> = None;

    let mut cpu = CpuCollector::new(config.pid);
    let memory = MemoryCollector::new();
    let mut network = NetworkCollector::new();
    let mut disk = DiskCollector::new(interval);
    let mut gpu = GpuCollector::new();

    // Collect static GPU info now so host discovery can derive GPU host fields.
    let initial_gpus = gpu.collect().unwrap_or_default();

    // Host discovery: fast, local, no I/O.
    let host_info = collect_host_info(&initial_gpus);

    // Warm-up: prime delta state in stateful collectors while cloud probes run
    // in the background. spawn_cloud_discovery returns a channel Receiver so
    // the caller never blocks on probe completion -- try_recv() picks up the
    // result if probes finished during the sleep, or leaves cloud_info as None
    // to be resolved later (per-tick poll in the main loop, or recv_timeout
    // before start_run for Sentinel runs).
    let cloud_rx = spawn_cloud_discovery();
    let _ = cpu.collect();
    let _ = network.collect();
    let _ = disk.collect();
    std::thread::sleep(interval);
    // Non-blocking: on most non-cloud machines all probes fail fast
    // (EHOSTUNREACH); on cloud machines the matching probe returns in < 100 ms.
    // Either way the result is typically waiting by the time we reach here.
    let mut cloud_info: Option<CloudInfo> = cloud_rx.as_ref().and_then(|rx| rx.try_recv().ok());

    // -----------------------------------------------------------------------
    // Shell-wrapper mode: spawn the tracked command after warm-up / cloud probe.
    // -----------------------------------------------------------------------
    if !config.command.is_empty() {
        let (program, args) = config.command.split_first().expect("command is non-empty");
        match std::process::Command::new(program).args(args).spawn() {
            Ok(c) => {
                config.pid = Some(i32::try_from(c.id()).unwrap_or(i32::MAX));
                cpu.set_tracked_pid(config.pid);
                child = Some(c);
            }
            Err(e) => {
                eprintln!("error: failed to spawn {:?}: {e}", program);
                std::process::exit(1);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Sentinel API setup (gated on SENTINEL_API_TOKEN being set).
    // -----------------------------------------------------------------------
    let sentinel = SentinelClient::from_env();

    let (run_ctx_arc, sample_buffer, upload_shutdown_flag, upload_handle) = match &sentinel {
        None => (None, None, None, None),
        Some(client) => {
            // Bounded wait: give cloud discovery a chance to complete before
            // start_run so the run record carries cloud metadata. IMDS probes
            // run in parallel and finish within IMDS_TIMEOUT (1 s); 3 s is a
            // generous ceiling for unusual network paths. Pure metric runs
            // (no Sentinel token) skip this entirely.
            if cloud_info.is_none() {
                if let Some(ref rx) = cloud_rx {
                    cloud_info = rx.recv_timeout(Duration::from_secs(3)).ok();
                }
            }
            let default_cloud = CloudInfo::default();
            match start_run(
                &client.agent,
                &client.api_base,
                &client.token,
                &config.metadata,
                config.pid,
                &host_info,
                cloud_info.as_ref().unwrap_or(&default_cloud),
            ) {
                Err(e) => {
                    eprintln!("warn: sentinel start_run failed: {e}; streaming disabled");
                    (None, None, None, None)
                }
                Ok(ctx) => {
                    let ctx_arc = Arc::new(Mutex::new(ctx));
                    let upload_interval = std::env::var("TRACKER_UPLOAD_INTERVAL")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(60u64);
                    let (uploader, buf) = BatchUploader::new(upload_interval, config.interval_secs);
                    let flag = uploader.shutdown_flag();
                    let upload_handle = uploader.spawn(
                        Arc::clone(&ctx_arc),
                        SentinelClient::new_upload_agent(),
                        client.api_base.clone(),
                        client.token.clone(),
                    );
                    if upload_handle.is_none() {
                        eprintln!(
                            "warn: sentinel background upload disabled; samples will be flushed inline on exit"
                        );
                    }
                    (Some(ctx_arc), Some(buf), Some(flag), upload_handle)
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

    // Tracks the Instant at the start of each loop iteration so we can
    // compute the actual elapsed interval between samples and sleep only
    // for the remainder of the nominal interval (deadline-based scheduling).
    let mut prev_loop_start: Option<Instant> = None;

    // -----------------------------------------------------------------------
    // Main sampling loop
    // -----------------------------------------------------------------------
    loop {
        // Poll for cloud discovery result if not yet received. Typically a
        // no-op because probes complete within IMDS_TIMEOUT and the warm-up
        // sleep covers that window. Ensures the channel is drained and
        // cloud_info is populated for any future use.
        if cloud_info.is_none()
            && let Some(ref rx) = cloud_rx
            && let Ok(info) = rx.try_recv()
        {
            cloud_info = Some(info);
        }

        let loop_start = Instant::now();

        // Actual elapsed since the previous iteration started.  None on the
        // first real sample (no prior loop start to compare against).
        let actual_interval_ms: Option<u64> = prev_loop_start
            .map(|p| u64::try_from((loop_start - p).as_millis()).unwrap_or(u64::MAX));

        let timestamp_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut sample = Sample {
            timestamp_secs,
            actual_interval_ms,
            job_name: config.metadata.job_name.clone(),
            tracked_pid: config.pid,
            cpu: cpu.collect().unwrap_or_default(),
            memory: memory.collect().unwrap_or_default(),
            network: network.collect().unwrap_or_default(),
            disk: disk.collect().unwrap_or_default(),
            gpu: gpu.collect().unwrap_or_default(),
        };

        // Augment with per-process GPU stats.
        // With --pid: filter to the tracked process tree.
        // Without --pid: report system-wide GPU allocation (all processes).
        let (vram_mib, gpu_usage, gpu_utilized) =
            if config.pid.is_some() && !sample.cpu.process_tree_pids.is_empty() {
                let pids_u32: Vec<u32> = sample
                    .cpu
                    .process_tree_pids
                    .iter()
                    .filter_map(|&p| u32::try_from(p).ok())
                    .collect();
                gpu.process_gpu_info(&pids_u32, interval)
            } else {
                gpu.all_gpu_process_info(interval)
            };
        sample.cpu.process_gpu_vram_mib = vram_mib;
        sample.cpu.process_gpu_usage = gpu_usage;
        sample.cpu.process_gpu_utilized = gpu_utilized;

        // Emit to selected output sink.
        match config.format {
            OutputFormat::Json => match serde_json::to_value(&sample) {
                Ok(mut v) => {
                    v[format!("{}-version", env!("CARGO_PKG_NAME"))] =
                        serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string());
                    emit!("{}", v);
                }
                Err(e) => eprintln!("warn: json serialize error: {e}"),
            },
            OutputFormat::Csv => {
                emit!(
                    "{}",
                    output::csv::sample_to_csv_row(&sample, config.interval_secs)
                );
            }
        }

        // Push to sentinel buffer (if streaming is active).
        if let Some(ref buf) = sample_buffer {
            buf.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(sample.clone());
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

        prev_loop_start = Some(loop_start);

        // Deadline-based sleep: sleep only for the time remaining in the
        // nominal interval.  If collection itself took longer than the
        // interval, skip sleeping entirely and start the next sample right
        // away.  This prevents drift accumulation and matches the Python
        // resource-tracker's timer approach.
        let elapsed = loop_start.elapsed();
        if let Some(remaining) = interval.checked_sub(elapsed) {
            std::thread::sleep(remaining);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that SIGINT sets SIGTERM_RECEIVED, triggering the same graceful
    /// shutdown path as SIGTERM.  The test installs the handler, resets the
    /// flag, raises SIGINT, then asserts the flag is true.
    #[test]
    fn test_sigint_sets_shutdown_flag() {
        // Reset in case a previous test left the flag set.
        SIGTERM_RECEIVED.store(false, Ordering::SeqCst);

        // Install the handler for SIGINT (mirrors what main() does).
        unsafe {
            libc::signal(
                libc::SIGINT,
                handle_sigterm as *const () as libc::sighandler_t,
            );
        }

        // Raise SIGINT on the current process.
        unsafe {
            libc::raise(libc::SIGINT);
        }

        assert!(
            SIGTERM_RECEIVED.load(Ordering::SeqCst),
            "SIGTERM_RECEIVED flag must be true after SIGINT"
        );

        // Clean up: reset the flag and restore the default SIGINT disposition
        // so this does not interfere with other tests.
        SIGTERM_RECEIVED.store(false, Ordering::SeqCst);
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
        }
    }
}
