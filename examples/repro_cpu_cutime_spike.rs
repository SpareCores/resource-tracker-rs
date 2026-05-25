/// Minimal reproduction of the `process_cores_used` > system CPU bug.
///
/// # Root cause
///
/// `process_tree_ticks()` stores each process's tick count as `utime + cutime`
/// (plus `stime + cstime`).  `cutime`/`cstime` are *cumulative* — they sum the
/// total CPU time of every child ever `wait()`-ed since process start.
///
/// When a child that has been running for N seconds is reaped in a single
/// sampling interval, the *delta* of the parent's `cutime` for that one
/// interval equals the child's **entire lifetime** tick count, not just the
/// ticks it accumulated since the previous sample.  `process_cores_used` then
/// spikes to roughly `child_run_secs / interval_secs`, which can far exceed
/// the simultaneously measured system `utilization_pct`.
///
/// # How to run
///
/// ```text
/// cargo build --examples
/// ./target/debug/resource-tracker --interval 1 -- \
///     ./target/debug/examples/repro_cpu_cutime_spike 2>&1 | \
///     jq '{cores_process: .cpu.process_cores_used,
///           cores_system:  .cpu.utilization_pct}'
/// ```
///
/// # Expected output
///
/// During the ~5 s the child is alive both metrics hover near 1.0.
/// In the single interval where the child exits and is reaped,
/// `cores_process` spikes to ≈ 5.0 while `cores_system` stays near 1.0.
use std::time::{Duration, Instant};

const CHILD_RUN_SECS: u64 = 5;
const WARMUP_SECS: u64 = 5;
const TAIL_SECS: u64 = 3;

fn main() {
    // When re-invoked with --burn, saturate one core and exit.
    if std::env::args().nth(1).as_deref() == Some("--burn") {
        let end = Instant::now() + Duration::from_secs(CHILD_RUN_SECS);
        let mut x = 0u64;
        while Instant::now() < end {
            x = x.wrapping_add(1);
        }
        let _ = x;
        return;
    }

    std::thread::sleep(Duration::from_secs(WARMUP_SECS));

    let exe = std::env::current_exe().expect("cannot resolve own executable");
    let mut child = std::process::Command::new(&exe)
        .arg("--burn")
        .spawn()
        .expect("failed to spawn child");

    child.wait().expect("wait() failed");
    // Child is now reaped: its full lifetime ticks roll into parent cutime.
    // The next tracker sample sees a cutime delta = child's entire lifetime,
    // spiking process_cores_used to ≈ CHILD_RUN_SECS.

    std::thread::sleep(Duration::from_secs(TAIL_SECS));
}
