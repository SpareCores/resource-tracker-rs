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

    // RAII guard: restores RLIMIT_NPROC on drop, including during unwinding from
    // a failing assertion. Without this, a panicking test leaves the limit tight
    // and starves subsequent tests with EAGAIN.
    struct NprocGuard(libc::rlimit);

    impl Drop for NprocGuard {
        fn drop(&mut self) {
            let rc = unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, &self.0, std::ptr::null_mut()) };
            if rc != 0 {
                eprintln!(
                    "warn: RLIMIT_NPROC restore failed: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    fn tighten_nproc() -> Option<NprocGuard> {
        let threads: libc::rlim_t = std::fs::read_to_string("/proc/self/status")
            .ok()?
            .lines()
            .find(|l| l.starts_with("Threads:"))
            .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))?;
        let mut old = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, std::ptr::null(), &mut old) } != 0 {
            return None;
        }
        let tight = libc::rlimit {
            rlim_cur: threads,
            rlim_max: old.rlim_max,
        };
        if unsafe { libc::prlimit(0, libc::RLIMIT_NPROC, &tight, std::ptr::null_mut()) } != 0 {
            return None;
        }
        Some(NprocGuard(old))
    }

    // T-NPROC-01: spawn_named must return None instead of panicking when the OS
    // rejects thread creation with EAGAIN (the root cause of the exit-139 crash
    // under tight cgroup pids.max).  RLIMIT_NPROC is set to the current thread
    // count so the very next spawn attempt is rejected by the kernel.
    // NprocGuard restores the limit via Drop so subsequent tests are not starved.
    #[test]
    fn test_spawn_named_returns_none_on_nproc_limit() {
        let Some(_guard) = tighten_nproc() else {
            return; // /proc or prlimit unavailable; skip
        };

        let result = spawn_named("test-eagain", || ());

        assert!(
            result.is_none(),
            "spawn_named should return None under tight RLIMIT_NPROC, not panic"
        );
        // _guard drops here, restoring RLIMIT_NPROC
    }
}
