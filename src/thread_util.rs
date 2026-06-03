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
