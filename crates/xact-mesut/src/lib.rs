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
//!
//! # Phase 4 (this crate, now)
//!
//! Directive section 32's Phase 4: "Connect Mesut execution events to
//! Xact's terminal state model." Section 19 draws the line this crate
//! keeps: "Xact's user-facing diagnostic model SHALL remain semantic.
//! Mesut's telemetry SHALL remain execution-oriented... Xact may expose
//! selected Mesut telemetry through its dynamic terminal interface." The
//! result each `PendingTask`/`join`/`try_join` already reports *is* that
//! semantic diagnostic (`ran '...' — exited 0`); it does not depend on
//! anything below.
//!
//! [`LifecycleEvent`] is Mesut's execution-oriented telemetry, translated
//! out of `mesut_observe::TaskEventType` so nothing outside this crate
//! needs a `mesut`/`mesut-observe` dependency (section 28's isolation
//! invariant). A [`BranchObserver`] registered on the shared runtime via
//! `MesuT::with_observer` (replacing whichever observer `RuntimeConfig`
//! picked — this crate already disables Mesut's own animation observer,
//! so nothing is duplicated per section 20's "Mesut's existing
//! lifecycle-driven terminal animation behaviour SHALL remain
//! Mesut-owned") captures every real `Submitted`/`Routed`/`Queued`/
//! `Started`/`Completed`/`Failed`/`Cancelled` event Mesut fires and routes
//! it to whichever `PendingTask` subscribed for that task's ID —
//! `submit` now does that subscription before admitting the work, so no
//! event can be missed. `PendingTask::drain_events` hands them out,
//! non-blocking, best-effort: a caller that never polls loses nothing
//! that matters, because the authoritative outcome still comes from
//! `join`/`try_join`. `xact-cli` is the only caller that actually prints
//! these, and only for `! CONCURRENTLY` branches — see its module docs.
//!
//! # Phase 5 (this crate, now)
//!
//! Directive section 32's Phase 5: "Translate Xact resource policies into
//! Mesut execution constraints." [`run_process`]/[`run_process_async`]
//! now take a `xact_ast::ResourceBudget` and pass it straight to
//! `xact_process::run`, which enforces it via `xact-resource`'s real
//! cgroup v2 mechanism before spawning — see that crate for how. This
//! deliberately does *not* route the budget through `mesut::ResourceHint`:
//! that field is Mesut's own pre-execution size *estimate*, used for
//! scheduling heuristics, not a cap any Mesut executor enforces (Mesut has
//! no resource-enforcement mechanism at all, confirmed by inspection —
//! forcing a hard percentage into an "estimated cycles" field would be a
//! fabricated translation, not a real one). Enforcement is a property of
//! how the OS process is spawned, owned end-to-end by
//! `xact-process`/`xact-resource`, orthogonal to which Mesut executor
//! thread happens to call `Command::spawn`.
//!
//! # Phase 6 (this crate, now)
//!
//! Directive section 32's Phase 6: "Route appropriate BANK/BOUND
//! operations through the unified execution path." `£ CREATE` (`bank`)
//! has gone through this adapter since Phase 2; [`bound_aggregate`]/
//! [`bound_aggregate_async`] give `£ BOUND` the same full treatment —
//! `WorkKind::Blocking` submission, lifecycle events, and a
//! `ResourceBudget` — rather than a special-cased path, which is the
//! actual meaning of "unified": every real external-tool call goes
//! through the same seam, whichever tool it happens to be.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use mesut::prelude::*;
use mesut::{EventObserver, TaskError, TaskEvent};
use mesut_observe::TaskEventType;

/// The outcome of a process run through the Mesut adapter — the same
/// success/exit-code shape `std::process::ExitStatus` reports, but owned
/// data so it can travel out of a `Work` closure and across the channel
/// that carries the real result back to the synchronous caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessOutcome {
    pub success: bool,
    pub code: Option<i32>,
}

/// Mesut's execution-oriented telemetry for one task (spec section 19),
/// translated out of `mesut_observe::TaskEventType` so callers outside
/// this crate never need a `mesut`/`mesut-observe` dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleEvent {
    Submitted,
    Routed { route: String },
    Queued { queue_depth: usize },
    Started { executor: String },
    Completed { duration_ms: u128 },
    Failed { error: String },
    Cancelled { reason: String },
}

impl LifecycleEvent {
    /// Real Mesut lifecycle events terminate in exactly one of these three
    /// — once one arrives for a task, no further event will.
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled { .. })
    }
}

impl From<&TaskEventType> for LifecycleEvent {
    fn from(event_type: &TaskEventType) -> Self {
        match event_type {
            TaskEventType::Submitted { .. } => Self::Submitted,
            TaskEventType::Routed { route } => Self::Routed { route: route.clone() },
            TaskEventType::Queued { queue_depth } => Self::Queued { queue_depth: *queue_depth },
            TaskEventType::Started { executor_id } => Self::Started { executor: executor_id.clone() },
            TaskEventType::Completed { duration_ms } => Self::Completed { duration_ms: *duration_ms },
            TaskEventType::Failed { error } => Self::Failed { error: error.clone() },
            TaskEventType::Cancelled { reason } => Self::Cancelled { reason: reason.clone() },
        }
    }
}

/// Routes each real Mesut [`TaskEvent`] to whichever [`PendingTask`]
/// subscribed for that event's task ID. Self-cleaning: a subscriber entry
/// is removed the moment a terminal event is delivered to it, regardless
/// of whether anything ever polled for it, so a `PendingTask` that's
/// dropped without calling `drain_events` cannot leak an entry here.
#[derive(Default)]
struct BranchObserver {
    subscribers: Mutex<HashMap<TaskId, std::sync::mpsc::Sender<LifecycleEvent>>>,
}

impl BranchObserver {
    fn subscribe(&self, task_id: TaskId) -> std::sync::mpsc::Receiver<LifecycleEvent> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.subscribers.lock().unwrap().insert(task_id, tx);
        rx
    }
}

impl EventObserver for BranchObserver {
    fn observe(&self, event: &TaskEvent) {
        let lifecycle = LifecycleEvent::from(&event.event_type);
        let mut subscribers = self.subscribers.lock().unwrap();
        let Some(sender) = subscribers.get(&event.task_id) else {
            return;
        };
        let _ = sender.send(lifecycle.clone());
        if lifecycle.is_terminal() {
            subscribers.remove(&event.task_id);
        }
    }
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

fn shared_observer() -> &'static Arc<BranchObserver> {
    static OBSERVER: OnceLock<Arc<BranchObserver>> = OnceLock::new();
    OBSERVER.get_or_init(|| Arc::new(BranchObserver::default()))
}

fn shared_mesut() -> &'static MesuT {
    static MESUT: OnceLock<MesuT> = OnceLock::new();
    MESUT.get_or_init(|| runtime().with_observer(shared_observer().clone()))
}

/// A handle to blocking [`Work`] admitted onto the shared Mesut runtime.
/// Submission (the call that produces this) has already happened — the
/// job is genuinely running, or queued to run, on a Mesut worker thread —
/// this only controls when *this caller* waits for the result.
pub struct PendingTask<T> {
    rx: std::sync::mpsc::Receiver<Result<T, String>>,
    events: std::sync::mpsc::Receiver<LifecycleEvent>,
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

    /// Drains every [`LifecycleEvent`] observed for this task since the
    /// last call, without blocking. Best-effort telemetry (spec section
    /// 19) — nothing about correctness depends on a caller ever polling
    /// this.
    pub fn drain_events(&mut self) -> Vec<LifecycleEvent> {
        std::iter::from_fn(|| self.events.try_recv().ok()).collect()
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
            let result = job();
            // Mirrored into Mesut's own Work result so its Completed/Failed
            // telemetry reflects the real domain outcome (a genuine adapter
            // failure, e.g. a missing binary) rather than only whether this
            // wrapper closure panicked. A normal nonzero exit status is
            // still `Ok` here — `job()` only returns `Err` for a failure to
            // launch at all, never for the process's own exit code.
            let mesut_result = match &result {
                Ok(_) => Ok(Vec::new()),
                Err(message) => Err(TaskError::ExecutionFailed(message.clone())),
            };
            let _ = tx.send(result);
            mesut_result
        });

    // Subscribed before submission is admitted, so no event — not even
    // `Submitted` itself — can fire before there is a receiver for it.
    let events = shared_observer().subscribe(work.id);

    tokio_runtime()
        .block_on(shared_mesut().submit(work))
        .map_err(|err| AdapterError(err.to_string()))?;

    Ok(PendingTask { rx, events })
}

/// Runs `command_line` behind the Mesut adapter (`£ RUN`'s execution
/// path), constrained by `budget` (spec section 22, Xact–Mesut
/// Integration Phase 5). `xact-process` still does the actual launching
/// and, via `xact-resource`, the actual enforcement.
pub fn run_process(command_line: String, budget: xact_ast::ResourceBudget) -> Result<ProcessOutcome, AdapterError> {
    run_process_async(command_line, budget)?.join()
}

/// Same as [`run_process`], but returns immediately as an independent
/// branch (`£ RUN` under `! CONCURRENTLY`) instead of waiting for the
/// process to exit.
pub fn run_process_async(
    command_line: String,
    budget: xact_ast::ResourceBudget,
) -> Result<PendingTask<ProcessOutcome>, AdapterError> {
    submit("xact.run", move || {
        xact_process::run(&command_line, &budget)
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

/// Aggregates `source` behind the Mesut adapter (`£ BOUND`'s execution
/// path), constrained by `budget`. `xact-bound` still owns the actual
/// `bound` invocation and, via `xact-resource`, the actual enforcement.
pub fn bound_aggregate(
    source: PathBuf,
    destination: Option<PathBuf>,
    budget: xact_ast::ResourceBudget,
) -> Result<(), AdapterError> {
    bound_aggregate_async(source, destination, budget)?.join()
}

/// Same as [`bound_aggregate`], but returns immediately as an independent
/// branch.
pub fn bound_aggregate_async(
    source: PathBuf,
    destination: Option<PathBuf>,
    budget: xact_ast::ResourceBudget,
) -> Result<PendingTask<()>, AdapterError> {
    submit("xact.bound", move || {
        xact_bound::aggregate(&source, destination.as_deref(), &budget).map_err(|err| err.to_string())
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
        let outcome = run_process("true".into(), xact_ast::ResourceBudget::default()).expect("true should launch");
        assert_eq!(outcome, ProcessOutcome { success: true, code: Some(0) });
    }

    #[test]
    fn run_process_reports_nonzero_exit_not_an_error() {
        let outcome = run_process("false".into(), xact_ast::ResourceBudget::default()).expect("false should launch");
        assert_eq!(outcome, ProcessOutcome { success: false, code: Some(1) });
    }

    #[test]
    fn run_process_reports_missing_binary_as_an_error() {
        let result = run_process("xact-definitely-not-a-real-binary".into(), xact_ast::ResourceBudget::default());
        assert!(result.is_err());
    }

    #[test]
    fn run_process_applies_a_real_resource_budget() {
        let budget = xact_ast::ResourceBudget { cpu_percent: Some(50), ..Default::default() };
        let outcome = run_process("true".into(), budget).expect("true should launch under a real cgroup cap");
        assert_eq!(outcome, ProcessOutcome { success: true, code: Some(0) });
    }

    #[test]
    fn run_process_fails_before_launching_for_an_unenforceable_resource() {
        let budget = xact_ast::ResourceBudget { unenforceable: vec!["GPU".into()], ..Default::default() };
        assert!(run_process("true".into(), budget).is_err());
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

    #[test]
    fn bound_aggregate_runs_the_real_bound_tool() {
        let dir = std::env::temp_dir().join(format!("xact-mesut-bound-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let out = dir.join("bundle.txt");

        let result = bound_aggregate(dir.clone(), Some(out.clone()), xact_ast::ResourceBudget::default());

        assert!(result.is_ok(), "bound should succeed: {result:?}");
        assert!(out.is_file(), "bound should have written the output file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bound_aggregate_fails_before_launching_for_an_unenforceable_resource() {
        let dir = std::env::temp_dir().join(format!("xact-mesut-bound-budget-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let budget = xact_ast::ResourceBudget { unenforceable: vec!["GPU".into()], ..Default::default() };
        let result = bound_aggregate(dir.clone(), None, budget);

        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
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
            .map(|_| run_process_async("sleep 1".into(), xact_ast::ResourceBudget::default()).expect("submission should be admitted"))
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
        let mut branch = run_process_async("sleep 1".into(), xact_ast::ResourceBudget::default()).expect("submission should be admitted");

        assert!(branch.try_join().is_none(), "should still be running immediately after submission");

        let outcome = branch.join().expect("sleep should launch");
        assert!(outcome.success);
    }

    /// Proves lifecycle telemetry is real Mesut events, not fabricated:
    /// a submitted task must eventually report `Submitted` and exactly
    /// one terminal event (`Completed` here, since `true` succeeds).
    #[test]
    fn drain_events_reports_real_lifecycle_including_a_terminal_event() {
        let mut branch = run_process_async("true".into(), xact_ast::ResourceBudget::default()).expect("submission should be admitted");

        // The result (via try_join) and the terminal lifecycle event are
        // delivered through independent channels and can arrive in either
        // order, so wait for the event itself rather than for try_join.
        let mut events = Vec::new();
        let mut result = None;
        for _ in 0..100 {
            events.extend(branch.drain_events());
            if result.is_none() {
                result = branch.try_join();
            }
            if events.iter().any(LifecycleEvent::is_terminal) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(result.is_some(), "the process should have completed within 1s: {events:?}");
        assert!(events.contains(&LifecycleEvent::Submitted), "expected a Submitted event: {events:?}");
        let terminal: Vec<_> = events.iter().filter(|e| e.is_terminal()).collect();
        assert_eq!(terminal.len(), 1, "expected exactly one terminal event: {events:?}");
        assert!(matches!(terminal[0], LifecycleEvent::Completed { .. }), "expected Completed: {events:?}");
    }
}
