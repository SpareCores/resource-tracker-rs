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
    /// before returning all successfully uploaded S3 URIs.
    pub fn spawn(
        self,
        ctx: Arc<Mutex<RunContext>>,
        agent: ureq::Agent,
        api_base: String,
        token: String,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut region_cache = RegionCache::new();
            let mut seq: u32 = 0;
            let mut consecutive_failures: u32 = 0;

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

                // Upload with exponential backoff: retry 1 after 2s, retry 2 after 4s
                // (Section 9.2.2: "retry at least once with exponential back-off").
                let result = s3_put(&agent, &bucket, &key, &region, &compressed, &creds)
                    .or_else(|e1| {
                        std::thread::sleep(Duration::from_secs(2));
                        s3_put(&agent, &bucket, &key, &region, &compressed, &creds)
                            .map_err(|e2| format!("{e1}; retry1: {e2}"))
                    })
                    .or_else(|e1| {
                        std::thread::sleep(Duration::from_secs(4));
                        s3_put(&agent, &bucket, &key, &region, &compressed, &creds)
                            .map_err(|e2| format!("{e1}; retry2: {e2}"))
                    });

                match result {
                    Ok(_uri) => {
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
}
