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
    ) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let mut uploaded_uris: Vec<String> = Vec::new();
            let mut region_cache = RegionCache::new();
            let mut seq: u32 = 0;
            let mut consecutive_failures: u32 = 0;

            let sleep_duration = Duration::from_secs(self.upload_interval_secs);

            loop {
                let shutting_down = self.shutdown.load(Ordering::Relaxed);

                if !shutting_down {
                    std::thread::sleep(sleep_duration);
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
                utilization_pct: 1.0,
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
}
