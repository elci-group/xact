//! Xact/Mesut integration boundary (Xact–Mesut Technical Directive,
//! sections 1–34). This crate is the *only* place Xact code may depend on
//! `mesut`'s crate topology (directive section 28: "An adapter layer
//! SHOULD isolate Xact from Mesut's internal crate topology"; section 29:
//! the dependency direction is `Xact -> Mesut`, never the reverse, and
//! Mesut must never acquire dependencies on Xact syntax, pronouns, or
//! policy keywords).
//!
//! # Phase 1 (done)
//!
//! Directive section 32 scoped Phase 1 as: "Add Mesut as an Xact workspace
//! dependency. Establish `Xact -> Mesut` with no behavioural changes to
//! the language." That shipped as a dependency edge only — [`runtime`]
//! proved it was live without anything calling it.
//!
//! # Phase 2 (this crate, now)
//!
//! Directive section 32's Phase 2: "Implement the Xact-to-Mesut execution
//! adapter. Move ordinary external-process execution behind the adapter."
//!
//! Phase 1's module docs recorded a blocking finding: Mesut's executors
//! were simulation stubs (`// Simulate work execution`, sleep-and-discard)
//! and `Work` had no field to carry real executable content. Mesut has
//! since been extended (commit `2e94d38`, "Add real task execution,
//! coordination scheduling, adaptive scheduling, and observability") —
//! `Work::with_job` now carries a real closure that runs on a compute or
//! blocking worker and returns a real `TaskResult<Vec<u8>>`, and
//! `mesut-blocking`/`mesut-rayon`/`mesut-tokio` actually execute it instead
//! of discarding it. That unblocks this phase honestly.
//!
//! [`run_process`] is the adapter for `£ RUN`: it wraps `xact-process`'s
//! real process launch in a [`mesut::prelude::Work`] of
//! [`mesut::prelude::WorkKind::Blocking`] (an inherited-stdio, wait-for-exit
//! subprocess is exactly the "may stall a thread" work that kind exists
//! for), submits it through a shared [`mesut::MesuT`] runtime, and reports
//! back the same success/exit-code shape `xact-executor` already expects.
//! `xact-process` still owns *how* to run a process (naive whitespace
//! splitting, inherited stdio, no shell semantics); this crate only owns
//! *handing that work to Mesut and getting the result back* — composition,
//! not reimplementation (spec sections 3/12/13).
//!
//! `£ CREATE` (`xact-bank`) and `£ SEE` (`xact-see`) are not moved behind
//! the adapter yet — they shell out via `.output()`/`.status()` today and
//! are direct candidates for the same treatment, but Phase 2 as directed
//! calls out "ordinary external-process execution" (i.e. `RUN`) first.
//! Widening the adapter to cover them is natural follow-up work, not a
//! blocked decision like Phase 1's finding was.

use std::fmt;
use std::sync::OnceLock;

use mesut::prelude::*;

/// The outcome of a process run through the Mesut adapter — the same
/// success/exit-code shape `std::process::ExitStatus` reports, but owned
/// data so it can travel out of a `Work` closure and across the channel
/// that carries the real result back to the synchronous caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessOutcome {
    pub success: bool,
    pub code: Option<i32>,
}

#[derive(Debug)]
pub struct AdapterError(String);

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AdapterError {}

/// Constructs a default Mesut runtime with all three executors wired up.
/// Exists standalone (in addition to the shared instance behind
/// [`run_process`]) so tests — and future callers — can prove the
/// dependency edge without reaching into adapter internals.
pub fn runtime() -> MesuT {
    MesuT::new(RuntimeConfig::new().with_animations(false))
        .with_async_executor(std::sync::Arc::new(TokioExecutor::new(Default::default())))
        .with_compute_executor(std::sync::Arc::new(
            RayonExecutor::new(Default::default()).expect("rayon executor should build"),
        ))
        .with_blocking_executor(std::sync::Arc::new(BlockingExecutor::new(Default::default())))
}

/// `MesuT::submit` requires a live Tokio context (it calls
/// `tokio::runtime::Handle::try_current()` internally even for blocking
/// work), so the adapter keeps one small multi-thread runtime alive for
/// the process's lifetime rather than spinning one up per call.
fn tokio_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .build()
            .expect("failed to start the Mesut adapter's Tokio runtime")
    })
}

fn shared_mesut() -> &'static MesuT {
    static MESUT: OnceLock<MesuT> = OnceLock::new();
    MESUT.get_or_init(runtime)
}

/// Runs `command_line` behind the Mesut adapter (`£ RUN`'s execution
/// path): submits it as blocking [`Work`] to the shared Mesut runtime and
/// waits for the real result. `xact-process` still does the actual
/// launching; this function is the seam that hands that job to Mesut
/// instead of running it inline.
pub fn run_process(command_line: String) -> Result<ProcessOutcome, AdapterError> {
    let (tx, rx) = std::sync::mpsc::channel();

    let work = Work::new(WorkKind::Blocking)
        .with_label("xact.run")
        .with_job(move |_cancellation| {
            let outcome = xact_process::run(&command_line)
                .map(|status| ProcessOutcome {
                    success: status.success(),
                    code: status.code(),
                })
                .map_err(|err| err.to_string());
            let _ = tx.send(outcome);
            Ok(Vec::new())
        });

    tokio_runtime()
        .block_on(shared_mesut().submit(work))
        .map_err(|err| AdapterError(err.to_string()))?;

    rx.recv()
        .map_err(|_| AdapterError("Mesut completed the task without a result".into()))?
        .map_err(AdapterError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_constructs() {
        let runtime = runtime();
        assert_eq!(runtime.active_tasks(), 0);
    }

    #[tokio::test]
    async fn submits_work_through_the_real_mesut_runtime() {
        let runtime = runtime();

        let result = runtime.submit(Work::new(WorkKind::Io)).await;

        assert!(result.is_ok(), "expected Mesut to accept and route the work: {result:?}");
    }

    #[test]
    fn run_process_reports_success() {
        let outcome = run_process("true".into()).expect("true should launch");
        assert_eq!(outcome, ProcessOutcome { success: true, code: Some(0) });
    }

    #[test]
    fn run_process_reports_nonzero_exit_not_an_error() {
        let outcome = run_process("false".into()).expect("false should launch");
        assert_eq!(outcome, ProcessOutcome { success: false, code: Some(1) });
    }

    #[test]
    fn run_process_reports_missing_binary_as_an_error() {
        let result = run_process("xact-definitely-not-a-real-binary".into());
        assert!(result.is_err());
    }
}
