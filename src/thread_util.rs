//! Best-effort OS thread spawning — never panics when the kernel returns EAGAIN.

use std::thread::{self, JoinHandle};

/// Spawn `f` on a named thread. On failure (e.g. PID/thread limit), log a warning
/// and return `None` so callers can fall back to inline work or skip the feature.
pub fn spawn_named<T, F>(name: &str, f: F) -> Option<JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match thread::Builder::new().name(name.to_owned()).spawn(f) {
        Ok(handle) => Some(handle),
        Err(e) => {
            eprintln!("warn: failed to spawn thread '{name}': {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // T-NPROC-01: spawn_named must return None instead of panicking when the OS
    // rejects thread creation with EAGAIN (the root cause of the exit-139 crash
    // under tight cgroup pids.max).  RLIMIT_NPROC is set to the current thread
    // count so the very next spawn attempt is rejected by the kernel.
    #[test]
    fn test_spawn_named_returns_none_on_nproc_limit() {
        let threads: libc::rlim_t = match std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("Threads:"))
                    .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))
            }) {
            Some(n) => n,
            None => return, // /proc unavailable; skip
        };

        let mut old = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, std::ptr::null(), &mut old) } != 0 {
            return; // prlimit unavailable; skip
        }
        let tight = libc::rlimit {
            rlim_cur: threads,
            rlim_max: old.rlim_max,
        };
        if unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, &tight, std::ptr::null_mut()) } != 0 {
            return; // cannot tighten limit; skip
        }

        let result = spawn_named("test-eagain", || ());

        // Restore before asserting so a test failure does not leave the process
        // permanently thread-starved.
        unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, &old, std::ptr::null_mut()) };

        assert!(
            result.is_none(),
            "spawn_named panicked instead of returning None under tight RLIMIT_NPROC"
        );
    }
}
