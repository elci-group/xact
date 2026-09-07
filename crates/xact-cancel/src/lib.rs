//! Real process cancellation (spec section 12; Xact–Mesut Integration
//! Phase 8, continued): "Cancellation SHALL be first-class... Xact SHALL
//! propagate cancellation through the Mesut execution context...
//! Cancellation SHALL NOT require killing the entire Xact process unless
//! the workload has become irrecoverably unresponsive."
//!
//! This crate is the shared registry every real process-spawning adapter
//! (`xact-process`, `xact-bound`, `xact-tell`) registers its spawned
//! child into, immediately after `spawn` and before waiting on it, via
//! [`register`]. [`cancel_all`] is what `xact-cli`'s `CTRL-C` handler
//! calls to actually stop whatever's running, by sending a real
//! `SIGTERM` to every currently registered child.
//!
//! Deliberately independent of relying on the terminal's own default
//! `CTRL-C` behavior (which, via OS process groups, would often already
//! deliver `SIGINT` straight to children in the same group without any
//! help from Xact). That path exists but isn't something a test — or
//! Xact itself, when invoked non-interactively (piped, backgrounded) —
//! can rely on or observe. Explicitly registering every real child's pid
//! and signalling it directly is deterministic regardless of how Xact
//! was invoked, which is what this crate's own tests exercise: they call
//! [`cancel_all`] directly, the same call a real `CTRL-C` handler makes,
//! rather than sending the test process itself a real signal.
//!
//! A cancelled child's `wait()` simply returns with a signal-terminated
//! `ExitStatus` — no new outcome type is needed anywhere upstream:
//! `xact-process`/`xact-mesut`/`xact-executor`/`xact-cli` already treat a
//! `None` exit code as "terminated by signal" (this was already true
//! before any real signal could ever produce it).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static REGISTRY: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static CANCELLED: AtomicBool = AtomicBool::new(false);

/// Removes its pid from the registry when dropped — hold this for exactly
/// as long as the child might still be running (from just after `spawn`
/// until `wait`/`wait_with_output` returns).
pub struct ChildGuard(u32);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Ok(mut registry) = REGISTRY.lock() {
            registry.retain(|pid| *pid != self.0);
        }
    }
}

/// Registers a just-spawned child's pid so a later [`cancel_all`] can
/// reach it. Call this immediately after `spawn`, before waiting.
pub fn register(pid: u32) -> ChildGuard {
    if let Ok(mut registry) = REGISTRY.lock() {
        registry.push(pid);
    }
    ChildGuard(pid)
}

/// Marks cancellation requested for this session and sends a real
/// `SIGTERM` to every currently registered child — safe to call more
/// than once (e.g. from a `CTRL-C` handler that could fire again before
/// the first cancellation finishes propagating).
pub fn cancel_all() {
    CANCELLED.store(true, Ordering::SeqCst);
    let Ok(registry) = REGISTRY.lock() else { return };
    for &pid in registry.iter() {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
}

/// Whether cancellation has been requested at any point in this process's
/// lifetime. `xact-cli` can use this to acknowledge a `CTRL-C` to the
/// user even if, by the time it checks, nothing was actually running to
/// cancel.
pub fn is_cancelled() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}

/// Clears the cancellation flag once it's been acknowledged, so a later
/// unrelated command isn't mistakenly reported as cancelled too.
pub fn reset() {
    CANCELLED.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// One test, not several: `cancel_all` reaches every currently
    /// registered pid, and cargo runs tests within a crate in parallel by
    /// default — a `cancel_all` in one test could otherwise kill a
    /// still-running child a sibling test just registered.
    #[test]
    fn cancellation_registry_behaves_correctly() {
        reset();
        assert!(!is_cancelled());

        // A guard dropped before cancellation removes the pid — that
        // child is never touched by a later `cancel_all`.
        let mut finished = Command::new("true").spawn().expect("true should launch");
        {
            let _guard = register(finished.id());
        }
        let finished_status = finished.wait().expect("true should be waitable");
        assert!(finished_status.success(), "an already-unregistered process should be unaffected by cancel_all");

        // A still-registered child is really killed, promptly, by cancel_all.
        let mut running = Command::new("sleep").arg("10").spawn().expect("sleep should launch");
        let _guard = register(running.id());

        let start = Instant::now();
        cancel_all();
        let running_status = running.wait().expect("sleep should be waitable after being killed");
        let elapsed = start.elapsed();

        assert!(is_cancelled());
        assert!(!running_status.success(), "a killed process should not report success");
        assert!(
            elapsed < Duration::from_secs(5),
            "cancel_all should kill the process almost immediately, not let the full 10s sleep elapse (took {elapsed:?})"
        );

        reset();
    }
}
