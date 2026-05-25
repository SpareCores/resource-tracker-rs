/// Demonstrates how tracked-process memory relates to system `used_mib`.
///
/// Two effects are shown when comparing process tree memory to system used RAM:
///
/// ## Effect 1 — file-backed pages vs system used_mib
///
/// PSS attributes file-backed mappings proportionally, and system `used_mib` is
/// `MemTotal − MemAvailable` (Python/psutil). A large mmap can still move the
/// two counters differently, but less than the old VmRSS sum did.
///
/// ## Effect 2 — shared mappings across the tree
///
/// With PSS via `/proc/pid/smaps_rollup`, identical read-only `MAP_PRIVATE`
/// mappings share physical pages and each process counts only its proportional
/// share. Summing VmRSS across workers used to multiply RAM by N; PSS largely
/// avoids that.
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
///     jq '{process_pss_mib: .cpu.process_pss_mib,
///           process_rss_mib: .cpu.process_rss_mib,
///           system_used_mib: .memory.used_mib}'
/// ```
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
/// in the tracked tree.
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

    // Hold the mapping so the tracker records tree PSS.
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

    // Parent also maps the file.
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
    let exe = std::env::current_exe().expect("cannot resolve executable");
    let mut children: Vec<_> = (0..N_WORKERS)
        .map(|_| {
            std::process::Command::new(&exe)
                .arg("--worker")
                .spawn()
                .expect("failed to spawn worker")
        })
        .collect();

    std::thread::sleep(Duration::from_secs(15));

    for c in &mut children {
        let _ = c.kill();
        let _ = c.wait();
    }
    unsafe { libc::munmap(parent_ptr, MAPPING_SIZE) };
    fs::remove_file(TEMP_PATH).ok();
}
