/// Minimal reproduction of the `process_rss_mib` > `system used_mib` bug.
///
/// Two distinct effects are demonstrated simultaneously:
///
/// ## Effect 1 — file-backed pages vs system used_mib (partially mitigated)
///
/// `process_rss_mib` sums `VmRSS`, which includes `RssFile` (file-backed
/// resident pages).  System `used_mib` is `MemTotal − MemAvailable` (same as
/// Python/psutil), which treats reclaimable cache as available — so a large
/// mmap still inflates process RSS more than system used, but less severely
/// than the old `total − free − buffers − cached` formula.
///
/// ## Effect 2 — shared pages counted once per process (double-counting)
///
/// When multiple processes in the tracked tree all map the same file with
/// `MAP_PRIVATE` (copy-on-write, read-only), the kernel stores one physical
/// copy in the page cache.  But each process's `VmRSS` includes the full
/// size, so the sum over the tree multiplies the actual RAM by N_WORKERS.
///
/// Disk: requires ~MAPPING_GIB GiB of free space in /tmp.  If /tmp is a
/// tmpfs the file itself consumes RAM — use MAPPING_GIB=1 in that case.
///
/// # How to run
///
/// ```text
/// cargo build --examples
/// ./target/debug/resource-tracker --interval 1 -- \
///     ./target/debug/examples/repro_memory_rss_vs_used 2>&1 | \
///     jq '{process_mib: .cpu.process_rss_mib,
///           system_used_mib: .memory.used_mib}'
/// ```
///
/// # Expected output
///
/// `process_mib` rises to ≈ (N_WORKERS + 1) × MAPPING_GIB × 1024 MiB
/// while `system_used_mib` barely changes.
use std::fs;
use std::io::Write as _;
use std::os::unix::io::AsRawFd;
use std::time::Duration;

/// GiB per mapping.  Keep at 1 on tmpfs /tmp to avoid OOM.
const MAPPING_GIB: usize = 1;
const MAPPING_SIZE: usize = MAPPING_GIB * 1024 * 1024 * 1024;
const N_WORKERS: usize = 4;
const TEMP_PATH: &str = "/tmp/repro_rss_mmap_data";

/// mmap the file at TEMP_PATH, touch every page, then sleep.
/// Called when re-invoked with --worker so each worker is a separate process
/// in the tracked tree (maximising the summed-VmRSS effect).
fn worker_main() {
    let file = fs::File::open(TEMP_PATH).expect("worker: cannot open temp file");
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            MAPPING_SIZE,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            file.as_raw_fd(),
            0,
        )
    };
    assert_ne!(ptr, libc::MAP_FAILED, "worker: mmap failed");

    let slice = unsafe { std::slice::from_raw_parts(ptr as *const u8, MAPPING_SIZE) };
    let mut checksum = 0u64;
    for offset in (0..MAPPING_SIZE).step_by(4096) {
        checksum = checksum.wrapping_add(slice[offset] as u64);
    }
    let _ = checksum;

    // Hold the mapping so the tracker records the inflated RSS.
    std::thread::sleep(Duration::from_secs(20));
    unsafe { libc::munmap(ptr, MAPPING_SIZE) };
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--worker") {
        worker_main();
        return;
    }

    // Idle baseline.
    std::thread::sleep(Duration::from_secs(5));

    // Write the file once; all workers map it read-only → one physical copy.
    {
        let mut f = fs::File::create(TEMP_PATH).expect("cannot create temp file");
        let chunk = vec![0xABu8; 1024 * 1024];
        for _ in 0..(MAPPING_SIZE / chunk.len()) {
            f.write_all(&chunk).expect("write failed");
        }
    }

    // Parent also maps the file (contributes one extra MAPPING_GIB to the sum).
    let file = fs::File::open(TEMP_PATH).expect("cannot open temp file");
    let parent_ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            MAPPING_SIZE,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            file.as_raw_fd(),
            0,
        )
    };
    assert_ne!(parent_ptr, libc::MAP_FAILED, "parent mmap failed");
    let parent_slice = unsafe { std::slice::from_raw_parts(parent_ptr as *const u8, MAPPING_SIZE) };
    let mut checksum = 0u64;
    for offset in (0..MAPPING_SIZE).step_by(4096) {
        checksum = checksum.wrapping_add(parent_slice[offset] as u64);
    }
    let _ = checksum;

    // Spawn N_WORKERS children, each mapping the same file independently.
    // Summed VmRSS = (N_WORKERS + 1) * MAPPING_GIB GiB.
    // Actual RAM    =               1 * MAPPING_GIB GiB  (shared page cache).
    let exe = std::env::current_exe().expect("cannot resolve executable");
    let mut children: Vec<_> = (0..N_WORKERS)
        .map(|_| {
            std::process::Command::new(&exe)
                .arg("--worker")
                .spawn()
                .expect("failed to spawn worker")
        })
        .collect();

    // Hold everything so the tracker observes the inflated RSS.
    std::thread::sleep(Duration::from_secs(15));

    for c in &mut children {
        let _ = c.kill();
        let _ = c.wait();
    }
    unsafe { libc::munmap(parent_ptr, MAPPING_SIZE) };
    fs::remove_file(TEMP_PATH).ok();
}
