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
//! [`run_process`], [`establish_path`], [`view_directory`], and
//! [`view_file`] are the adapters for `£ RUN`, `£ CREATE`, and `£ SEE`
//! respectively: each wraps a real external-tool call (`xact-process`,
//! `xact-bank`, `xact-see`) in a [`mesut::prelude::Work`] of
//! [`mesut::prelude::WorkKind::Blocking`] (a wait-for-exit subprocess —
//! whether it's a naive `RUN`, `bank -p`, or inherited-stdio `gls`/`bat` —
//! is exactly the "may stall a thread" work that kind exists for),
//! submits it through a shared [`mesut::MesuT`] runtime via
//! [`submit_blocking`], and reports back the real result. Those three
//! crates still own *how* to run their respective tools (argv splitting,
//! flag conventions, stdio inheritance); this crate only owns *handing
//! that work to Mesut and getting the result back* — composition, not
//! reimplementation (spec sections 3/12/13).

use std::fmt;
use std::path::PathBuf;
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

/// Runs `job` as blocking [`Work`] on the shared Mesut runtime and waits
/// for its real result. `job` runs on a Mesut blocking-executor worker
/// thread, not inline — stdio inheritance and captured output both work
/// the same regardless of which OS thread spawns the child process, so
/// this is transparent to callers that shell out.
fn submit_blocking<T: Send + 'static>(
    label: &'static str,
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, AdapterError> {
    let (tx, rx) = std::sync::mpsc::channel();

    let work = Work::new(WorkKind::Blocking)
        .with_label(label)
        .with_job(move |_cancellation| {
            let _ = tx.send(job());
            Ok(Vec::new())
        });

    tokio_runtime()
        .block_on(shared_mesut().submit(work))
        .map_err(|err| AdapterError(err.to_string()))?;

    rx.recv()
        .map_err(|_| AdapterError("Mesut completed the task without a result".into()))?
        .map_err(AdapterError)
}

/// Runs `command_line` behind the Mesut adapter (`£ RUN`'s execution
/// path). `xact-process` still does the actual launching.
pub fn run_process(command_line: String) -> Result<ProcessOutcome, AdapterError> {
    submit_blocking("xact.run", move || {
        xact_process::run(&command_line)
            .map(|status| ProcessOutcome {
                success: status.success(),
                code: status.code(),
            })
            .map_err(|err| err.to_string())
    })
}

/// Establishes `path` behind the Mesut adapter (`£ CREATE`'s execution
/// path). `xact-bank` still owns `bank -p <path>` and its file/directory
/// disambiguation.
pub fn establish_path(path: PathBuf) -> Result<PathBuf, AdapterError> {
    submit_blocking("xact.create", move || {
        xact_bank::establish(&path).map_err(|err| err.to_string())
    })
}

/// Shows a directory with `gls` behind the Mesut adapter (`£ SEE`'s
/// directory routing).
pub fn view_directory(path: PathBuf) -> Result<(), AdapterError> {
    submit_blocking("xact.see.directory", move || {
        xact_see::view_directory(&path).map_err(|err| err.to_string())
    })
}

/// Shows a file with `bat` behind the Mesut adapter (`£ SEE`'s file
/// routing).
pub fn view_file(path: PathBuf) -> Result<(), AdapterError> {
    submit_blocking("xact.see.file", move || {
        xact_see::view_file(&path).map_err(|err| err.to_string())
    })
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

    #[test]
    fn establish_path_actually_creates_the_path() {
        let dir = std::env::temp_dir().join(format!("xact-mesut-create-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = dir.join("nested/dir/");

        let result = establish_path(target.clone());

        assert_eq!(result.expect("bank should succeed"), target);
        assert!(dir.join("nested/dir").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn view_directory_runs_gls_through_the_adapter() {
        let result = view_directory(std::env::temp_dir());
        assert!(result.is_ok(), "gls is expected to be installed and to succeed: {result:?}");
    }
}
