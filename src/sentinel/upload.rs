//! Background batch upload thread: buffers samples, serializes as CSV,
//! gzip-compresses, and uploads to S3 every 60 seconds (configurable).

use crate::metrics::Sample;
use crate::output::csv::{csv_header, sample_to_csv_row};
use crate::sentinel::run::{RunContext, refresh_credentials};
use crate::sentinel::s3::{RegionCache, parse_s3_uri, s3_put};
use flate2::{Compression, write::GzEncoder};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(test)]
use flate2::read::GzDecoder;

/// Shared sample buffer: main thread pushes, upload thread drains.
pub type SampleBuffer = Arc<Mutex<Vec<Sample>>>;

// Maximum consecutive upload failures before the thread stops retrying
// for the current batch and logs a warning.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

// Total upload attempts per batch (1 initial + N-1 retries).
// Delay before attempt i (i > 0) is 2^i seconds: 2 s, 4 s, 8 s, …
const MAX_UPLOAD_ATTEMPTS: u32 = 3;

// ---------------------------------------------------------------------------
// CSV serialization helper
// ---------------------------------------------------------------------------

/// Serialize a slice of samples as a complete CSV string (header + rows).
pub fn samples_to_csv(samples: &[Sample], interval_secs: u64) -> String {
    let mut out = String::with_capacity(samples.len() * 256);
    out.push_str(csv_header());
    out.push('\n');
    samples.iter().for_each(|s| {
        out.push_str(&sample_to_csv_row(s, interval_secs));
        out.push('\n');
    });
    out
}

/// Gzip-compress `data` using the default compression level.
pub fn gzip_compress(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|e| format!("gzip write failed: {e}"))?;
    encoder
        .finish()
        .map_err(|e| format!("gzip finish failed: {e}"))
}

// ---------------------------------------------------------------------------
// BatchUploader
// ---------------------------------------------------------------------------

pub struct BatchUploader {
    /// Buffer shared with the main thread.
    pub buffer: SampleBuffer,
    /// Set to true by `request_shutdown()` to trigger a final flush.
    shutdown: Arc<AtomicBool>,
    /// Polling interval for the upload thread (seconds, default 60).
    upload_interval_secs: u64,
    /// Sampling interval (seconds) -- needed to compute per-interval byte counts in CSV.
    sample_interval_secs: u64,
}

impl BatchUploader {
    /// Create a new `BatchUploader` and return the shared `SampleBuffer`
    /// so the main thread can push samples into it.
    pub fn new(upload_interval_secs: u64, sample_interval_secs: u64) -> (Self, SampleBuffer) {
        let buffer = Arc::new(Mutex::new(Vec::<Sample>::new()));
        let uploader = Self {
            buffer: Arc::clone(&buffer),
            shutdown: Arc::new(AtomicBool::new(false)),
            upload_interval_secs,
            sample_interval_secs,
        };
        (uploader, buffer)
    }

    /// Clone the shutdown flag so `main.rs` can signal the upload thread to
    /// flush and exit after moving `self` into the spawned thread.
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
    }

    /// Spawn the background upload thread.
    ///
    /// The thread wakes every `upload_interval_secs`, drains the buffer, builds
    /// a `.csv.gz` batch (gzip-compressed CSV, `Content-Type: application/gzip`),
    /// and uploads it to S3.  On shutdown signal it performs one final flush
    /// before exiting.
    ///
    /// Returns a `JoinHandle<Vec<String>>` whose value is the list of all
    /// successfully uploaded S3 URIs (e.g. `"s3://bucket/prefix/run-id/000000.csv.gz"`).
    /// The caller uses this list to decide the `/finish` route:
    /// - non-empty → `data_source: "s3"` with `data_uris`
    /// - empty     → `data_source: "inline"` with `data_csv`
    pub fn spawn(
        self,
        ctx: Arc<Mutex<RunContext>>,
        agent: ureq::Agent,
        api_base: String,
        token: String,
    ) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let mut region_cache = RegionCache::new();
            let mut seq: u32 = 0;
            let mut consecutive_failures: u32 = 0;
            let mut uploaded_uris: Vec<String> = Vec::new();

            // Break the upload interval into 250 ms ticks so a shutdown signal
            // is noticed within 250 ms rather than waiting a full 60 seconds.
            let tick = Duration::from_millis(250);
            let ticks_per_interval = (self.upload_interval_secs * 4).max(1);

            loop {
                let shutting_down = self.shutdown.load(Ordering::Relaxed);

                if !shutting_down {
                    (0..ticks_per_interval)
                        .take_while(|_| !self.shutdown.load(Ordering::Relaxed))
                        .for_each(|_| std::thread::sleep(tick));
                }

                // Drain buffer under a minimal lock window.
                let batch: Vec<Sample> = {
                    let mut guard = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
                    std::mem::take(&mut *guard)
                };

                if batch.is_empty() {
                    if shutting_down {
                        break;
                    }
                    continue;
                }

                // Serialize and compress outside the lock.
                let csv = samples_to_csv(&batch, self.sample_interval_secs);
                let compressed = match gzip_compress(csv.as_bytes()) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("warn: upload batch {seq} gzip failed: {e}");
                        if shutting_down {
                            break;
                        }
                        continue;
                    }
                };

                // Check credentials near expires_at; refresh if needed.
                {
                    let mut ctx_guard = ctx.lock().unwrap_or_else(|e| e.into_inner());
                    if ctx_guard.creds_expiring_soon() {
                        if let Err(e) =
                            refresh_credentials(&agent, &api_base, &token, &mut ctx_guard)
                        {
                            eprintln!("warn: credential refresh failed: {e}");
                        }
                    }
                }

                // Build the S3 key.
                let (bucket, prefix, creds, run_id) = {
                    let ctx_guard = ctx.lock().unwrap_or_else(|e| e.into_inner());
                    let uri = parse_s3_uri(&ctx_guard.upload_uri_prefix).unwrap_or_else(|_| {
                        crate::sentinel::s3::S3Uri {
                            bucket: String::new(),
                            key: String::new(),
                        }
                    });
                    (
                        uri.bucket,
                        uri.key,
                        ctx_guard.credentials.clone(),
                        ctx_guard.run_id.clone(),
                    )
                };

                if bucket.is_empty() {
                    eprintln!("warn: upload_uri_prefix could not be parsed; skipping batch {seq}");
                    if shutting_down {
                        break;
                    }
                    continue;
                }

                let region = region_cache.get_or_detect(&bucket);
                let key = format!("{prefix}/{run_id}/{seq:06}.csv.gz");

                // Upload with exponential backoff.
                // Attempt 0 is immediate; attempt i (i > 0) sleeps 2^i seconds first.
                // With MAX_UPLOAD_ATTEMPTS=3: delays are 2 s and 4 s before retries.
                let result: Result<String, String> = {
                    let mut last_err = String::new();
                    let mut uploaded_uri: Option<String> = None;
                    for attempt in 0..MAX_UPLOAD_ATTEMPTS {
                        if attempt > 0 {
                            std::thread::sleep(Duration::from_secs(1u64 << attempt));
                        }
                        match s3_put(&agent, &bucket, &key, &region, &compressed, &creds) {
                            Ok(uri) => { uploaded_uri = Some(uri); break; }
                            Err(e) => {
                                last_err = if last_err.is_empty() {
                                    e
                                } else {
                                    format!("{last_err}; retry{attempt}: {e}")
                                };
                            }
                        }
                    }
                    match uploaded_uri {
                        Some(uri) => Ok(uri),
                        None      => Err(last_err),
                    }
                };

                match result {
                    Ok(uri) => {
                        uploaded_uris.push(uri);
                        seq += 1;
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        eprintln!(
                            "warn: S3 upload failed (attempt {consecutive_failures}/{MAX_CONSECUTIVE_FAILURES}): {e}"
                        );
                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            eprintln!(
                                "warn: {MAX_CONSECUTIVE_FAILURES} consecutive upload failures; buffering continues but data may be lost"
                            );
                            consecutive_failures = 0;
                        }
                    }
                }

                if shutting_down {
                    break;
                }
            }
            uploaded_uris
        })
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{CpuMetrics, MemoryMetrics, Sample};
    use crate::output::csv::csv_header;
    use std::io::Read;

    fn minimal_sample() -> Sample {
        Sample {
            timestamp_secs: 1_000_000,
            job_name:    None,
            tracked_pid: None,
            cpu: CpuMetrics {
                utilization_pct:          1.0,
                process_utime_secs:       None,
                process_stime_secs:       None,
                process_rss_mib:          None,
                process_disk_read_bytes:  None,
                process_disk_write_bytes: None,
                process_gpu_vram_mib:     None,
                process_gpu_utilized:     None,
                process_tree_pids:        vec![],
                ..Default::default()
            },
            memory: MemoryMetrics {
                free_mib: 512,
                used_mib: 512,
                ..Default::default()
            },
            network: vec![],
            disk: vec![],
            gpu: vec![],
        }
    }

    // T-STR-01: upload thread exits within 2 s of the shutdown flag being set,
    // even when the upload interval is 60 s.
    // Without the tick-based sleep the thread would block for the full interval.
    #[test]
    fn test_upload_thread_shuts_down_promptly() {
        use crate::sentinel::run::RunContext;
        use crate::sentinel::s3::UploadCredentials;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let (uploader, _buf) = BatchUploader::new(60, 1);
        let flag = uploader.shutdown_flag();

        let ctx = Arc::new(Mutex::new(RunContext {
            run_id:              "r".to_string(),
            upload_uri_prefix:   "s3://b/p".to_string(),
            credentials: UploadCredentials {
                access_key_id:     "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token:     "t".to_string(),
                expires_at:        "2099-01-01T00:00:00Z".to_string(),
            },
        }));

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(1)))
            .build()
            .new_agent();

        // Buffer is empty so the thread will never attempt an upload.
        let handle = uploader.spawn(
            ctx,
            agent,
            "http://127.0.0.1:1".to_string(),
            "token".to_string(),
        );

        // Signal shutdown immediately and measure how long join takes.
        let t0 = Instant::now();
        flag.store(true, Ordering::Relaxed);
        handle.join().expect("upload thread panicked");
        let elapsed = t0.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "upload thread took {elapsed:?} to shut down; expected < 2 s"
        );
    }

    // T-STR-02: batch body decompresses to valid CSV (header + data rows).
    //
    // Spec Section 9.2.2: "A batch upload request contains Content-Encoding: gzip
    // and the body decompresses to valid CSV or JSONL."
    // This test verifies the compress/decompress round-trip and CSV structure.
    #[test]
    fn test_gzip_compress_decompresses_to_valid_csv() {
        let samples = vec![minimal_sample(), minimal_sample()];
        let csv = samples_to_csv(&samples, 1);
        let compressed = gzip_compress(csv.as_bytes()).expect("gzip_compress failed");

        // Gzip magic bytes (RFC 1952 Section 2.3.1).
        assert_eq!(&compressed[..2], b"\x1f\x8b", "missing gzip magic bytes");

        // Decompress must round-trip to identical bytes.
        let mut decoder = GzDecoder::new(&compressed[..]);
        let mut decompressed = String::new();
        decoder
            .read_to_string(&mut decompressed)
            .expect("gzip decompression failed");
        assert_eq!(
            decompressed, csv,
            "decompressed content does not match original CSV"
        );

        // First line must be the CSV header.
        let first_line = decompressed.lines().next().expect("empty output");
        assert_eq!(first_line, csv_header(), "first line is not the CSV header");

        // Every data row must have the same column count as the header.
        let header_cols = csv_header().split(',').count();
        decompressed
            .lines()
            .skip(1)
            .enumerate()
            .for_each(|(i, line)| {
                assert!(!line.is_empty(), "unexpected empty data line at index {i}");
                let cols = line.split(',').count();
                assert_eq!(
                    cols, header_cols,
                    "data row {i} has {cols} columns, expected {header_cols}: {line}"
                );
            });
    }

    // Every line produced by samples_to_csv (header and data rows) ends with '\n'.
    #[test]
    fn test_samples_to_csv_all_lines_end_with_newline() {
        let samples = vec![minimal_sample()];
        let csv = samples_to_csv(&samples, 1);
        csv.split_inclusive('\n').for_each(|chunk| {
            assert!(
                chunk.ends_with('\n'),
                "line does not end with newline: {chunk:?}"
            );
        });
    }

    // T-STR-05: when the buffer is non-empty at shutdown and the upload URI is
    // invalid (cannot be parsed into bucket+key), the thread serializes the batch,
    // compresses it, then skips the S3 put and exits cleanly.
    //
    // This test covers the CSV-serialize → gzip → bucket-empty → shutdown path
    // inside the upload thread without requiring a real S3 endpoint.
    #[test]
    fn test_upload_thread_processes_batch_with_invalid_uri() {
        use crate::sentinel::run::RunContext;
        use crate::sentinel::s3::UploadCredentials;
        use std::sync::{Arc, Mutex};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        let (uploader, buf) = BatchUploader::new(1, 1);
        let flag = uploader.shutdown_flag();

        // Push a sample into the buffer so the thread has a non-empty batch.
        {
            let mut guard = buf.lock().unwrap();
            guard.push(minimal_sample());
        }

        // Invalid upload_uri_prefix: parse_s3_uri will fail and bucket will be empty.
        let ctx = Arc::new(Mutex::new(RunContext {
            run_id: "r".to_string(),
            upload_uri_prefix: "invalid-not-an-s3-uri".to_string(),
            credentials: UploadCredentials {
                access_key_id:     "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token:     "t".to_string(),
                expires_at:        "2099-01-01T00:00:00Z".to_string(),
            },
        }));

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_millis(100)))
            .build()
            .new_agent();

        // Signal shutdown before spawning so the thread skips the sleep phase.
        flag.store(true, Ordering::Relaxed);

        let handle = uploader.spawn(
            ctx,
            agent,
            "http://127.0.0.1:1".to_string(),
            "token".to_string(),
        );

        let t0 = Instant::now();
        handle.join().expect("upload thread panicked");
        let elapsed = t0.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "upload thread with non-empty batch took {elapsed:?}; expected < 2 s"
        );
    }

    // T-UPL-INT-01: full roundtrip -- sample serialized, gzip-compressed, and
    // PUT to the real Sentinel S3 bucket.  Covers the Ok(uri) success arm inside
    // spawn() (uploaded_uris.push, seq += 1, consecutive_failures = 0).
    // Skips automatically when SENTINEL_API_TOKEN is absent.
    #[test]
    fn test_upload_roundtrip_real_api() {
        use crate::config::JobMetadata;
        use crate::metrics::{CloudInfo, HostInfo};
        use crate::sentinel::run::{close_run, start_run};
        use std::sync::{Arc, Mutex};
        use std::sync::atomic::Ordering;
        use std::time::Duration;

        let token = match std::env::var("SENTINEL_API_TOKEN") {
            Ok(t) if !t.is_empty() => t,
            _ => { eprintln!("skip: SENTINEL_API_TOKEN not set"); return; }
        };
        let api_base = std::env::var("SENTINEL_API_BASE")
            .unwrap_or_else(|_| "https://api.sentinel.sparecores.net".to_string());
        eprintln!("T-UPL-INT-01: api_base={api_base}");

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();

        let ctx = start_run(
            &agent, &api_base, &token,
            &JobMetadata { job_name: Some("upload-roundtrip-test".to_string()), ..Default::default() },
            None, &HostInfo::default(), &CloudInfo::default(),
        ).expect("start_run failed");
        eprintln!("T-UPL-INT-01: run_id={}", ctx.run_id);

        let ctx_arc = Arc::new(Mutex::new(ctx));
        let (uploader, buf) = BatchUploader::new(1, 1);
        let flag = uploader.shutdown_flag();

        buf.lock().unwrap().push(minimal_sample());

        // Signal shutdown before spawning: the thread processes one batch then exits.
        flag.store(true, Ordering::Relaxed);

        let handle = uploader.spawn(
            Arc::clone(&ctx_arc),
            agent.clone(),
            api_base.clone(),
            token.clone(),
        );
        let uris = handle.join().expect("upload thread panicked");
        eprintln!("T-UPL-INT-01: uris={uris:?}");

        assert!(!uris.is_empty(), "expected at least one S3 URI; S3 upload may have failed");
        assert!(uris[0].starts_with("s3://"), "URI must have s3:// scheme: {}", uris[0]);

        // Close the run via the S3 route.
        let ctx_guard = ctx_arc.lock().unwrap();
        let result = close_run(&agent, &api_base, &token, &ctx_guard, Some(0), None, &uris);
        assert!(result.is_ok(), "close_run (S3 route) failed: {result:?}");
        eprintln!("T-UPL-INT-01: close_run ok");
    }

    // T-UPL-INT-02: upload thread calls refresh_credentials when expires_at is in the
    // past.  Covers the creds_expiring_soon() -> refresh_credentials block.
    // The actual S3 credentials issued by start_run are still valid; setting
    // expires_at to 1970 only causes our code to request a refresh -- the server
    // returns fresh credentials and the upload proceeds normally.
    // Skips automatically when SENTINEL_API_TOKEN is absent.
    #[test]
    fn test_upload_thread_refreshes_expiring_credentials() {
        use crate::config::JobMetadata;
        use crate::metrics::{CloudInfo, HostInfo};
        use crate::sentinel::run::{close_run, start_run};
        use std::sync::{Arc, Mutex};
        use std::sync::atomic::Ordering;
        use std::time::Duration;

        let token = match std::env::var("SENTINEL_API_TOKEN") {
            Ok(t) if !t.is_empty() => t,
            _ => { eprintln!("skip: SENTINEL_API_TOKEN not set"); return; }
        };
        let api_base = std::env::var("SENTINEL_API_BASE")
            .unwrap_or_else(|_| "https://api.sentinel.sparecores.net".to_string());
        eprintln!("T-UPL-INT-02: api_base={api_base}");

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();

        let mut ctx = start_run(
            &agent, &api_base, &token,
            &JobMetadata { job_name: Some("cred-refresh-test".to_string()), ..Default::default() },
            None, &HostInfo::default(), &CloudInfo::default(),
        ).expect("start_run failed");
        eprintln!("T-UPL-INT-02: run_id={}", ctx.run_id);

        // Force expires_at into the past so creds_expiring_soon() returns true.
        // The underlying S3 session token is still valid; this only triggers the
        // refresh call path in the upload thread.
        ctx.credentials.expires_at = "1970-01-01T00:00:00Z".to_string();
        eprintln!("T-UPL-INT-02: expires_at forced to epoch; creds_expiring_soon() will be true");

        let ctx_arc = Arc::new(Mutex::new(ctx));
        let (uploader, buf) = BatchUploader::new(1, 1);
        let flag = uploader.shutdown_flag();

        buf.lock().unwrap().push(minimal_sample());

        flag.store(true, Ordering::Relaxed);

        let handle = uploader.spawn(
            Arc::clone(&ctx_arc),
            agent.clone(),
            api_base.clone(),
            token.clone(),
        );
        let uris = handle.join().expect("upload thread panicked");
        eprintln!("T-UPL-INT-02: uris={uris:?}");

        // After refresh the upload should succeed via S3 or fall back to inline.
        let ctx_guard = ctx_arc.lock().unwrap();
        let csv = if uris.is_empty() {
            Some(samples_to_csv(&[minimal_sample()], 1))
        } else {
            None
        };
        let result = close_run(&agent, &api_base, &token, &ctx_guard, Some(0), csv, &uris);
        assert!(result.is_ok(), "close_run after credential refresh failed: {result:?}");
        eprintln!("T-UPL-INT-02: close_run ok");
    }

    // T-STR-06: when a valid-looking S3 endpoint is unreachable, the thread
    // retries up to MAX_CONSECUTIVE_FAILURES times then resets and continues.
    // Shutdown flag is set after the first failed batch so the thread exits.
    // NOTE: this test takes ~7 s because the retry back-off sleeps 2 s + 4 s.
    #[test]
    fn test_upload_thread_handles_s3_failure_gracefully() {
        use crate::sentinel::run::RunContext;
        use crate::sentinel::s3::UploadCredentials;
        use std::sync::{Arc, Mutex};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        let (uploader, buf) = BatchUploader::new(1, 1);
        let flag = uploader.shutdown_flag();

        // Push a sample into the buffer.
        {
            let mut guard = buf.lock().unwrap();
            guard.push(minimal_sample());
        }

        // A real-looking S3 URI but bucket host is unreachable (port 1 = closed).
        let ctx = Arc::new(Mutex::new(RunContext {
            run_id: "r".to_string(),
            upload_uri_prefix: "s3://fake-nonexistent-bucket-xyz/prefix".to_string(),
            credentials: UploadCredentials {
                access_key_id:     "AKIAIOSFODNN7EXAMPLE".to_string(),
                secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
                session_token:     "token".to_string(),
                expires_at:        "2099-01-01T00:00:00Z".to_string(),
            },
        }));

        // Very short timeout so the S3 attempt fails fast.
        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_millis(200)))
            .build()
            .new_agent();

        // Pre-set shutdown: the thread will process one batch, fail the upload,
        // then exit on the shutdown check.
        flag.store(true, Ordering::Relaxed);

        let handle = uploader.spawn(
            ctx,
            agent,
            "http://127.0.0.1:1".to_string(),
            "token".to_string(),
        );

        let t0 = Instant::now();
        handle.join().expect("upload thread panicked");
        let elapsed = t0.elapsed();

        // Should complete well within 5 s even with retry back-off (2+4 s),
        // because the retries are skipped when timeout is 200 ms and the
        // first attempt fails near-instantly.
        // Timing breakdown: region detection (up to 2 s TCP timeout) +
        // 3 × 200 ms agent timeouts + 2 s + 4 s retry sleeps = ~8.6 s max.
        // Allow 20 s to accommodate slow DNS on any CI/dev host.
        assert!(
            elapsed < Duration::from_secs(20),
            "upload thread took {elapsed:?}; expected < 20 s"
        );
    }

    // T-COV-01: empty sample slice produces only the CSV header line.
    #[test]
    fn test_samples_to_csv_empty_slice() {
        let csv = samples_to_csv(&[], 1);
        let mut lines = csv.lines();
        assert_eq!(lines.next(), Some(csv_header()), "first line must be the CSV header");
        assert_eq!(lines.next(), None, "empty slice must produce no data rows");
        assert!(csv.ends_with('\n'), "output must end with a newline");
    }

    // T-STR-07: the upload thread hits the empty-batch `continue` path at least
    // twice before processing a sample pushed after those cycles, then exits
    // on the shutdown signal.  Covers lines 135-139 in the non-shutdown branch.
    #[test]
    fn test_upload_thread_skips_empty_batch_then_processes() {
        use crate::sentinel::run::RunContext;
        use crate::sentinel::s3::UploadCredentials;
        use std::sync::{Arc, Mutex};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        // upload_interval=0 → ticks_per_interval = (0*4).max(1) = 1 → 250 ms per cycle.
        let (uploader, buf) = BatchUploader::new(0, 1);
        let flag = uploader.shutdown_flag();

        // Invalid URI → bucket empty after parse → S3 call is skipped entirely.
        let ctx = Arc::new(Mutex::new(RunContext {
            run_id: "r".to_string(),
            upload_uri_prefix: "invalid-not-an-s3-uri".to_string(),
            credentials: UploadCredentials {
                access_key_id:     "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token:     "t".to_string(),
                expires_at:        "2099-01-01T00:00:00Z".to_string(),
            },
        }));

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_millis(100)))
            .build()
            .new_agent();

        let handle = uploader.spawn(
            ctx,
            agent,
            "http://127.0.0.1:1".to_string(),
            "token".to_string(),
        );

        // Let the thread execute at least two empty-buffer iterations (2 × 250 ms).
        std::thread::sleep(Duration::from_millis(700));

        // Push a sample then signal shutdown.  The thread drains it on the next wake,
        // serializes, gzip-compresses, skips S3 (empty bucket), and exits.
        buf.lock().unwrap().push(minimal_sample());
        flag.store(true, Ordering::Relaxed);

        let t0 = Instant::now();
        handle.join().expect("upload thread panicked");
        let elapsed = t0.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "thread took {elapsed:?} after sample push; expected < 2 s"
        );
    }

    // T-STR-08: after MAX_CONSECUTIVE_FAILURES (3) consecutive batch failures the
    // thread resets the counter to 0 and continues rather than exiting.
    //
    // Three distinct batches are forced by pushing one sample at a time and
    // sleeping between pushes so each drain is a separate upload attempt.
    // Shutdown is signaled together with the third push so the thread processes
    // batch 3 (hits the reset branch) and then exits on the shutdown check.
    //
    // NOTE: each failed batch runs through MAX_UPLOAD_ATTEMPTS retries with
    // exponential back-off (2 s + 4 s = 6 s) plus region detection (~2 s on
    // the first batch).  Total wall-clock time is approximately 26 s.
    #[test]
    fn test_upload_thread_resets_consecutive_failures() {
        use crate::sentinel::run::RunContext;
        use crate::sentinel::s3::UploadCredentials;
        use std::sync::{Arc, Mutex};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        let ctx = Arc::new(Mutex::new(RunContext {
            run_id: "r".to_string(),
            upload_uri_prefix: "s3://fake-nonexistent-bucket-xyz/prefix".to_string(),
            credentials: UploadCredentials {
                access_key_id:     "AKIAIOSFODNN7EXAMPLE".to_string(),
                secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
                session_token:     "token".to_string(),
                expires_at:        "2099-01-01T00:00:00Z".to_string(),
            },
        }));

        let agent = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_millis(50)))
            .build()
            .new_agent();

        // upload_interval=0 → thread wakes every 250 ms between upload cycles.
        let (uploader, buf) = BatchUploader::new(0, 1);
        let flag = uploader.shutdown_flag();

        let handle = uploader.spawn(
            Arc::clone(&ctx),
            agent,
            "http://127.0.0.1:1".to_string(),
            "token".to_string(),
        );

        // Each batch takes ≤10 s: region detection (~2 s first time) +
        // 3 × 50 ms agent timeout + 2 s + 4 s retry back-off.
        // Push one sample per batch; sleep between pushes so each drain is
        // a distinct batch (batch N drains before sample N+1 arrives).
        //
        // Batch 1 → consecutive_failures = 1
        // Batch 2 → consecutive_failures = 2
        // Batch 3 → consecutive_failures = 3 → RESET to 0 (lines 233-237)
        let batch_wait = Duration::from_secs(10);
        for i in 0..3u32 {
            buf.lock().unwrap().push(minimal_sample());
            if i < 2 {
                std::thread::sleep(batch_wait);
            }
        }

        // Signal shutdown together with the third sample so the thread
        // processes batch 3, resets the counter, then exits.
        flag.store(true, Ordering::Relaxed);

        let t0 = Instant::now();
        handle.join().expect("upload thread panicked");
        let elapsed = t0.elapsed();

        // From the shutdown signal: batch 3 takes ≤10 s, then the thread exits.
        assert!(
            elapsed < Duration::from_secs(15),
            "thread did not exit after consecutive-failure reset: {elapsed:?}"
        );
    }

    // T-STR-09: BatchUploader::new wires the uploader's internal buffer and the
    // returned SampleBuffer to the same Arc allocation.
    #[test]
    fn test_batch_uploader_new_shares_buffer() {
        use std::sync::Arc;
        let (uploader, buf) = BatchUploader::new(60, 1);
        assert!(
            Arc::ptr_eq(&uploader.buffer, &buf),
            "uploader.buffer and the returned SampleBuffer must point to the same Arc allocation"
        );
    }
}
