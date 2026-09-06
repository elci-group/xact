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
//! submits it through a shared [`mesut::MesuT`] runtime, and reports back
//! the real result. Those three crates still own *how* to run their
//! respective tools (argv splitting, flag conventions, stdio inheritance);
//! this crate only owns *handing that work to Mesut and getting the
//! result back* — composition, not reimplementation (spec sections
//! 3/12/13).
//!
//! # Phase 3 (this crate, now)
//!
//! Directive section 32's Phase 3: "Translate `CONCURRENTLY`/
//! `CONSECUTIVELY` into Mesut execution constraints." Spec section 23:
//! `CONCURRENTLY` "creates independent execution branches"; `CONSECUTIVELY`
//! "creates an explicit dependency" (`A → B`).
//!
//! `CONSECUTIVELY` needs no new mechanism here: it's already what
//! `run_process`/`establish_path`/`view_directory`/`view_file` do — submit,
//! then block until the result is in, so the next command can't start
//! until the previous one has genuinely finished. That *is* `A → B`.
//!
//! `CONCURRENTLY` needs a real independent branch: work that starts now
//! and is joined later, so N submissions can be in flight on Mesut's
//! blocking-executor thread pool at once (a real OS thread pool —
//! `mesut-blocking`'s `num_cpus::get().max(2)` workers — not simulated
//! parallelism). [`PendingTask`] is that handle, and `*_async` variants of
//! each adapter function return one instead of blocking. `xact-executor`
//! wraps these into its own `ExecutionOutcome`-typed handle; `xact-cli` is
//! the only place that decides, from the session's established schedule
//! policy, whether to call the blocking or the `_async` adapter function.

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

/// A handle to blocking [`Work`] admitted onto the shared Mesut runtime.
/// Submission (the call that produces this) has already happened — the
/// job is genuinely running, or queued to run, on a Mesut worker thread —
/// this only controls when *this caller* waits for the result.
pub struct PendingTask<T> {
    rx: std::sync::mpsc::Receiver<Result<T, String>>,
}

impl<T> PendingTask<T> {
    /// Blocks until the real result is in.
    pub fn join(self) -> Result<T, AdapterError> {
        self.rx
            .recv()
            .map_err(|_| AdapterError("Mesut completed the task without a result".into()))?
            .map_err(AdapterError)
    }

    /// Non-blocking poll: `None` means still running.
    pub fn try_join(&mut self) -> Option<Result<T, AdapterError>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result.map_err(AdapterError)),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err(AdapterError("Mesut completed the task without a result".into())))
            }
        }
    }
}

/// Admits `job` as blocking [`Work`] on the shared Mesut runtime and
/// returns immediately with a handle to its real result. `job` runs on a
/// Mesut blocking-executor worker thread, not inline — stdio inheritance
/// and captured output both work the same regardless of which OS thread
/// spawns the child process, so this is transparent to callers that shell
/// out. This is the one place every adapter function funnels through;
/// the blocking (`run_process`, etc.) and `_async` variants differ only in
/// whether they call [`PendingTask::join`] before returning.
fn submit<T: Send + 'static>(
    label: &'static str,
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<PendingTask<T>, AdapterError> {
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

    Ok(PendingTask { rx })
}

/// Runs `command_line` behind the Mesut adapter (`£ RUN`'s execution
/// path). `xact-process` still does the actual launching.
pub fn run_process(command_line: String) -> Result<ProcessOutcome, AdapterError> {
    run_process_async(command_line)?.join()
}

/// Same as [`run_process`], but returns immediately as an independent
/// branch (`£ RUN` under `! CONCURRENTLY`) instead of waiting for the
/// process to exit.
pub fn run_process_async(command_line: String) -> Result<PendingTask<ProcessOutcome>, AdapterError> {
    submit("xact.run", move || {
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
    establish_path_async(path)?.join()
}

/// Same as [`establish_path`], but returns immediately as an independent
/// branch.
pub fn establish_path_async(path: PathBuf) -> Result<PendingTask<PathBuf>, AdapterError> {
    submit("xact.create", move || {
        xact_bank::establish(&path).map_err(|err| err.to_string())
    })
}

/// Shows a directory with `gls` behind the Mesut adapter (`£ SEE`'s
/// directory routing).
pub fn view_directory(path: PathBuf) -> Result<(), AdapterError> {
    view_directory_async(path)?.join()
}

/// Same as [`view_directory`], but returns immediately as an independent
/// branch.
pub fn view_directory_async(path: PathBuf) -> Result<PendingTask<()>, AdapterError> {
    submit("xact.see.directory", move || {
        xact_see::view_directory(&path).map_err(|err| err.to_string())
    })
}

/// Shows a file with `bat` behind the Mesut adapter (`£ SEE`'s file
/// routing).
pub fn view_file(path: PathBuf) -> Result<(), AdapterError> {
    view_file_async(path)?.join()
}

/// Same as [`view_file`], but returns immediately as an independent
/// branch.
pub fn view_file_async(path: PathBuf) -> Result<PendingTask<()>, AdapterError> {
    submit("xact.see.file", move || {
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

    /// Proves `_async` submissions are genuinely concurrent branches, not
    /// just a deferred API: three `sleep 1` processes started back-to-back
    /// without waiting between them must all be done well under 3x a
    /// single sleep's duration once joined, because they actually ran in
    /// parallel on Mesut's blocking thread pool.
    #[test]
    fn async_submissions_run_concurrently_not_sequentially() {
        let start = std::time::Instant::now();

        let branches: Vec<_> = (0..3)
            .map(|_| run_process_async("sleep 1".into()).expect("submission should be admitted"))
            .collect();

        for branch in branches {
            let outcome = branch.join().expect("sleep should launch");
            assert!(outcome.success);
        }

        assert!(
            start.elapsed() < std::time::Duration::from_millis(2500),
            "three concurrent 1s sleeps took {:?} — looks sequential, not concurrent",
            start.elapsed()
        );
    }

    #[test]
    fn try_join_reports_none_while_still_running_then_some_once_done() {
        let mut branch = run_process_async("sleep 1".into()).expect("submission should be admitted");

        assert!(branch.try_join().is_none(), "should still be running immediately after submission");

        let outcome = branch.join().expect("sleep should launch");
        assert!(outcome.success);
    }
}
