//! Plan execution (spec sections 2, 20): runs an [`ExecutionPlan`] and
//! reports what actually happened. This is where real side effects occur —
//! everything upstream (parsing, semantic validation, planning) is pure.
//!
//! Scope so far: every variant dispatches through `xact-mesut`'s execution
//! adapter (Xact–Mesut Integration Phase 2 — see `MESUT_INTEGRATION.md`),
//! which submits the real external-tool call (`bank`, `gls`/`bat`, or a
//! plain process launch) as blocking work to a Mesut runtime instead of
//! running it inline. `xact-bank`/`xact-see`/`xact-process` still own how
//! each tool is actually invoked; this crate no longer calls them
//! directly. Resource control (spec section 22) is not implemented — a
//! run is not yet constrained by any `SPEND`/`SAVE` policy in effect.
//!
//! [`execute`] runs a plan and blocks for its real result — the right
//! choice for `! CONSECUTIVELY` (spec section 23's `A → B`: the next
//! command must not start until this one has genuinely finished) and for
//! the default, no-policy-stated behavior, which is the same thing.
//! [`execute_concurrent`] instead admits the plan onto Mesut and returns a
//! [`Pending`] handle immediately — the adapter for `! CONCURRENTLY`'s
//! "independent execution branches" (Xact–Mesut Integration Phase 3).
//! `xact-cli` is the only caller that chooses between them, based on the
//! session's established schedule policy.
//!
//! [`Pending::drain_events`] additionally surfaces Mesut's real execution
//! lifecycle telemetry for a branch (Xact–Mesut Integration Phase 4) —
//! non-blocking and best-effort, separate from the authoritative outcome
//! `join`/`try_join` report (spec section 19).
//!
//! [`execute`]/[`execute_concurrent`] both take a `ResourceBudget` (spec
//! section 22, Xact–Mesut Integration Phase 5) and apply it to
//! `ExecutionPlan::Run` and `ExecutionPlan::Bound` — `xact-process`/
//! `xact-bound` (via `xact-resource`'s real cgroup v2 enforcement) are
//! what actually constrain them. `Bank`/`ViewDirectory`/`ViewFile` ignore
//! the budget for now: `bank`/`gls`/`bat` are typically short-lived, and
//! every spec/directive example of `SPEND`/`SAVE` pairs it with `RUN` —
//! widening enforcement to those is a scope decision to make later, not
//! an oversight here.
//!
//! `ExecutionPlan::Bound` (Xact–Mesut Integration Phase 6 — "route
//! appropriate BANK/BOUND operations through the unified execution
//! path") goes through the exact same `execute`/`execute_concurrent`
//! dispatch, the same `ResourceBudget` plumbing, and the same
//! `Pending`/lifecycle-event machinery as every other verb — no
//! special-casing, which is the actual meaning of "unified."
//!
//! `ExecutionPlan::Agent` (Xact–Mesut Integration Phase 7 — "route agent
//! workloads through Mesut") gets the same treatment again: `execute`/
//! `execute_concurrent` build an `xact_tell::TellRequest` from the plan
//! and hand it to `xact-mesut::tell`/`tell_async`, which submit it exactly
//! like any other verb. `xact-executor` never imports anything
//! provider-specific (no "ollama" anywhere here) — it only knows `@ TELL`
//! has an `xact-tell` adapter, the same way it knows `RUN` has
//! `xact-process`.

use std::path::PathBuf;

use xact_ast::ResourceBudget;
use xact_planner::ExecutionPlan;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionOutcome {
    BankEstablished { path: PathBuf },
    /// A directory or file was shown via `gls`/`bat`, whose own inherited
    /// stdio already rendered the content — `tool` names which one ran.
    Viewed { path: PathBuf, tool: &'static str },
    /// A process ran to completion. `success`/`code` report how it
    /// exited — a nonzero exit is a normal outcome (e.g. `grep` finding no
    /// matches), not an execution failure; [`ExecutionOutcome::Failed`] is
    /// reserved for Xact itself being unable to launch the plan at all.
    /// `signal` is `Some` when a real signal (e.g. a `CTRL-C`-triggered
    /// `SIGTERM`, Xact–Mesut Integration Phase 8's cancellation) is what
    /// actually ended it, rather than a normal exit.
    RunCompleted {
        command_line: String,
        success: bool,
        code: Option<i32>,
        signal: Option<i32>,
    },
    /// A source set was aggregated via `bound` — `destination` names
    /// where, or `None` for `bound`'s own clipboard default.
    Bounded { source: PathBuf, destination: Option<PathBuf> },
    /// A `@ TELL` block ran against a real agent provider. `populated`
    /// names where the response was also written, if `POPULATING` was
    /// stated.
    Told { model: String, response: String, populated: Option<PathBuf> },
    Failed { message: String },
}

pub fn execute(plan: ExecutionPlan, budget: ResourceBudget) -> ExecutionOutcome {
    match plan {
        ExecutionPlan::Bank { path } => bank_outcome(xact_mesut::establish_path(path)),
        ExecutionPlan::ViewDirectory { path } => {
            view_outcome(path.clone(), "gls", xact_mesut::view_directory(path))
        }
        ExecutionPlan::ViewFile { path } => view_outcome(path.clone(), "bat", xact_mesut::view_file(path)),
        ExecutionPlan::Run { command_line } => {
            run_outcome(command_line.clone(), xact_mesut::run_process(command_line, budget))
        }
        ExecutionPlan::Bound { source, destination } => bound_outcome(
            source.clone(),
            destination.clone(),
            xact_mesut::bound_aggregate(source, destination, budget),
        ),
        ExecutionPlan::Agent { model, persona, reading, populating, think, instruction } => {
            let request = xact_tell::TellRequest { model: model.clone(), persona, reading, instruction, think };
            told_outcome(model, populating.clone(), xact_mesut::tell(request, populating, budget))
        }
    }
}

/// Admits `plan` onto Mesut as an independent branch and returns
/// immediately (`! CONCURRENTLY`'s execution path — see the module
/// docs). `Err` means Mesut rejected the submission itself (e.g. no
/// executor available, or `budget` names a resource Xact can't enforce);
/// a genuinely running branch is always `Ok`, and its eventual
/// success/failure is only known once [`Pending::join`] or
/// [`Pending::try_join`] reports it.
pub fn execute_concurrent(plan: ExecutionPlan, budget: ResourceBudget) -> Result<Pending, ExecutionOutcome> {
    let submission_failed = |err: xact_mesut::AdapterError| ExecutionOutcome::Failed { message: err.to_string() };

    match plan {
        ExecutionPlan::Bank { path } => xact_mesut::establish_path_async(path)
            .map(|task| Pending(PendingKind::Bank { task }))
            .map_err(submission_failed),
        ExecutionPlan::ViewDirectory { path } => xact_mesut::view_directory_async(path.clone())
            .map(|task| Pending(PendingKind::View { path, tool: "gls", task }))
            .map_err(submission_failed),
        ExecutionPlan::ViewFile { path } => xact_mesut::view_file_async(path.clone())
            .map(|task| Pending(PendingKind::View { path, tool: "bat", task }))
            .map_err(submission_failed),
        ExecutionPlan::Run { command_line } => xact_mesut::run_process_async(command_line.clone(), budget)
            .map(|task| Pending(PendingKind::Run { command_line, task }))
            .map_err(submission_failed),
        ExecutionPlan::Bound { source, destination } => {
            xact_mesut::bound_aggregate_async(source.clone(), destination.clone(), budget)
                .map(|task| Pending(PendingKind::Bound { source, destination, task }))
                .map_err(submission_failed)
        }
        ExecutionPlan::Agent { model, persona, reading, populating, think, instruction } => {
            let request = xact_tell::TellRequest { model: model.clone(), persona, reading, instruction, think };
            xact_mesut::tell_async(request, populating.clone(), budget)
                .map(|task| Pending(PendingKind::Agent { model, populated: populating, task }))
                .map_err(submission_failed)
        }
    }
}

/// A concurrent branch admitted by [`execute_concurrent`], not yet joined.
pub struct Pending(PendingKind);

enum PendingKind {
    Bank {
        task: xact_mesut::PendingTask<PathBuf>,
    },
    View {
        path: PathBuf,
        tool: &'static str,
        task: xact_mesut::PendingTask<()>,
    },
    Run {
        command_line: String,
        task: xact_mesut::PendingTask<xact_mesut::ProcessOutcome>,
    },
    Bound {
        source: PathBuf,
        destination: Option<PathBuf>,
        task: xact_mesut::PendingTask<()>,
    },
    Agent {
        model: String,
        populated: Option<PathBuf>,
        task: xact_mesut::PendingTask<String>,
    },
}

impl Pending {
    /// Blocks until this branch's real result is in.
    pub fn join(self) -> ExecutionOutcome {
        match self.0 {
            PendingKind::Bank { task } => bank_outcome(task.join()),
            PendingKind::View { path, tool, task } => view_outcome(path, tool, task.join()),
            PendingKind::Run { command_line, task } => run_outcome(command_line, task.join()),
            PendingKind::Bound { source, destination, task } => bound_outcome(source, destination, task.join()),
            PendingKind::Agent { model, populated, task } => told_outcome(model, populated, task.join()),
        }
    }

    /// Non-blocking poll: `None` means the branch is still running.
    pub fn try_join(&mut self) -> Option<ExecutionOutcome> {
        match &mut self.0 {
            PendingKind::Bank { task } => task.try_join().map(bank_outcome),
            PendingKind::View { path, tool, task } => {
                task.try_join().map(|result| view_outcome(path.clone(), tool, result))
            }
            PendingKind::Run { command_line, task } => {
                task.try_join().map(|result| run_outcome(command_line.clone(), result))
            }
            PendingKind::Bound { source, destination, task } => {
                task.try_join().map(|result| bound_outcome(source.clone(), destination.clone(), result))
            }
            PendingKind::Agent { model, populated, task } => {
                task.try_join().map(|result| told_outcome(model.clone(), populated.clone(), result))
            }
        }
    }

    /// Mesut's execution-oriented telemetry for this branch since the
    /// last call, non-blocking (spec section 19 — supplementary to, never
    /// a substitute for, the outcome `join`/`try_join` reports).
    pub fn drain_events(&mut self) -> Vec<xact_mesut::LifecycleEvent> {
        match &mut self.0 {
            PendingKind::Bank { task } => task.drain_events(),
            PendingKind::View { task, .. } => task.drain_events(),
            PendingKind::Run { task, .. } => task.drain_events(),
            PendingKind::Bound { task, .. } => task.drain_events(),
            PendingKind::Agent { task, .. } => task.drain_events(),
        }
    }
}

fn bank_outcome(result: Result<PathBuf, xact_mesut::AdapterError>) -> ExecutionOutcome {
    match result {
        Ok(path) => ExecutionOutcome::BankEstablished { path },
        Err(err) => ExecutionOutcome::Failed { message: err.to_string() },
    }
}

fn bound_outcome(
    source: PathBuf,
    destination: Option<PathBuf>,
    result: Result<(), xact_mesut::AdapterError>,
) -> ExecutionOutcome {
    match result {
        Ok(()) => ExecutionOutcome::Bounded { source, destination },
        Err(err) => ExecutionOutcome::Failed { message: err.to_string() },
    }
}

fn told_outcome(
    model: String,
    populated: Option<PathBuf>,
    result: Result<String, xact_mesut::AdapterError>,
) -> ExecutionOutcome {
    match result {
        Ok(response) => ExecutionOutcome::Told { model, response, populated },
        Err(err) => ExecutionOutcome::Failed { message: err.to_string() },
    }
}

fn view_outcome(
    path: PathBuf,
    tool: &'static str,
    result: Result<(), xact_mesut::AdapterError>,
) -> ExecutionOutcome {
    match result {
        Ok(()) => ExecutionOutcome::Viewed { path, tool },
        Err(err) => ExecutionOutcome::Failed { message: err.to_string() },
    }
}

fn run_outcome(
    command_line: String,
    result: Result<xact_mesut::ProcessOutcome, xact_mesut::AdapterError>,
) -> ExecutionOutcome {
    match result {
        Ok(outcome) => ExecutionOutcome::RunCompleted {
            command_line,
            success: outcome.success,
            code: outcome.code,
            signal: outcome.signal,
        },
        Err(err) => ExecutionOutcome::Failed { message: err.to_string() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_plan_actually_creates_the_path() {
        let dir = std::env::temp_dir().join(format!("xact-executor-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = dir.join("nested/dir/");

        let outcome = execute(ExecutionPlan::Bank { path: target.clone() }, ResourceBudget::default());

        assert_eq!(outcome, ExecutionOutcome::BankEstablished { path: target.clone() });
        assert!(dir.join("nested/dir").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn view_directory_plan_runs_gls() {
        let outcome = execute(
            ExecutionPlan::ViewDirectory { path: std::env::temp_dir() },
            ResourceBudget::default(),
        );
        assert_eq!(
            outcome,
            ExecutionOutcome::Viewed {
                path: std::env::temp_dir(),
                tool: "gls"
            }
        );
    }

    #[test]
    fn run_plan_reports_success() {
        let outcome = execute(
            ExecutionPlan::Run { command_line: "true".into() },
            ResourceBudget::default(),
        );
        assert_eq!(
            outcome,
            ExecutionOutcome::RunCompleted {
                command_line: "true".into(),
                success: true,
                code: Some(0),
                signal: None,
            }
        );
    }

    #[test]
    fn run_plan_reports_nonzero_exit_as_completed_not_failed() {
        let outcome = execute(
            ExecutionPlan::Run { command_line: "false".into() },
            ResourceBudget::default(),
        );
        assert_eq!(
            outcome,
            ExecutionOutcome::RunCompleted {
                command_line: "false".into(),
                success: false,
                code: Some(1),
                signal: None,
            }
        );
    }

    #[test]
    fn run_plan_reports_missing_binary_as_failed() {
        let outcome = execute(
            ExecutionPlan::Run { command_line: "xact-definitely-not-a-real-binary".into() },
            ResourceBudget::default(),
        );
        assert!(matches!(outcome, ExecutionOutcome::Failed { .. }));
    }

    #[test]
    fn run_plan_under_a_resource_budget_is_still_constrained_for_real() {
        let budget = ResourceBudget { cpu_percent: Some(50), ..Default::default() };
        let outcome = execute(ExecutionPlan::Run { command_line: "true".into() }, budget);
        assert_eq!(
            outcome,
            ExecutionOutcome::RunCompleted { command_line: "true".into(), success: true, code: Some(0), signal: None }
        );
    }

    #[test]
    fn run_plan_fails_before_launching_for_an_unenforceable_resource() {
        let budget = ResourceBudget { unenforceable: vec!["GPU".into()], ..Default::default() };
        let outcome = execute(ExecutionPlan::Run { command_line: "true".into() }, budget);
        assert!(matches!(outcome, ExecutionOutcome::Failed { .. }));
    }

    #[test]
    fn bound_plan_actually_aggregates_a_real_directory() {
        let dir = std::env::temp_dir().join(format!("xact-executor-bound-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let out = dir.join("bundle.txt");

        let outcome = execute(
            ExecutionPlan::Bound { source: dir.clone(), destination: Some(out.clone()) },
            ResourceBudget::default(),
        );

        assert_eq!(outcome, ExecutionOutcome::Bounded { source: dir.clone(), destination: Some(out.clone()) });
        assert!(out.is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_bound_plan_reports_via_try_join() {
        let dir = std::env::temp_dir().join(format!("xact-executor-bound-concurrent-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let out = dir.join("bundle.txt");

        let mut branch = execute_concurrent(
            ExecutionPlan::Bound { source: dir.clone(), destination: Some(out.clone()) },
            ResourceBudget::default(),
        )
        .unwrap_or_else(|outcome| panic!("submission should be admitted: {outcome:?}"));

        let outcome = loop {
            if let Some(outcome) = branch.try_join() {
                break outcome;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };

        assert_eq!(outcome, ExecutionOutcome::Bounded { source: dir.clone(), destination: Some(out.clone()) });
        assert!(out.is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_run_plans_actually_run_in_parallel() {
        let start = std::time::Instant::now();

        let branches: Vec<Pending> = (0..3)
            .map(|_| {
                execute_concurrent(ExecutionPlan::Run { command_line: "sleep 1".into() }, ResourceBudget::default())
                    .unwrap_or_else(|outcome| panic!("submission should be admitted: {outcome:?}"))
            })
            .collect();

        for branch in branches {
            assert_eq!(
                branch.join(),
                ExecutionOutcome::RunCompleted {
                    command_line: "sleep 1".into(),
                    success: true,
                    code: Some(0),
                    signal: None,
                }
            );
        }

        assert!(
            start.elapsed() < std::time::Duration::from_millis(2500),
            "three concurrent branches took {:?} — looks sequential, not concurrent",
            start.elapsed()
        );
    }

    #[test]
    fn concurrent_bank_plan_reports_via_try_join() {
        let dir = std::env::temp_dir().join(format!("xact-executor-concurrent-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = dir.join("nested/dir/");

        let mut branch = execute_concurrent(ExecutionPlan::Bank { path: target.clone() }, ResourceBudget::default())
            .unwrap_or_else(|outcome| panic!("submission should be admitted: {outcome:?}"));

        let outcome = loop {
            if let Some(outcome) = branch.try_join() {
                break outcome;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };

        assert_eq!(outcome, ExecutionOutcome::BankEstablished { path: target.clone() });
        assert!(dir.join("nested/dir").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn agent_plan(instruction: &str, populating: Option<PathBuf>) -> ExecutionPlan {
        ExecutionPlan::Agent {
            model: "gemma3".into(),
            persona: None,
            reading: None,
            populating,
            think: None,
            instruction: instruction.into(),
        }
    }

    #[test]
    fn agent_plan_fails_before_launching_for_an_unenforceable_resource() {
        let budget = ResourceBudget { unenforceable: vec!["GPU".into()], ..Default::default() };
        let outcome = execute(agent_plan("hi", None), budget);
        assert!(matches!(outcome, ExecutionOutcome::Failed { .. }));
    }

    /// Real, not stubbed: runs the actual local `ollama` model through
    /// the full executor dispatch. Slow (model load + generation).
    #[test]
    fn agent_plan_runs_the_real_provider_and_populates_a_real_file() {
        let dir = std::env::temp_dir().join(format!("xact-executor-agent-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("response.txt");

        let outcome = execute(
            agent_plan("Reply with exactly one word: hello", Some(out.clone())),
            ResourceBudget::default(),
        );

        match outcome {
            ExecutionOutcome::Told { model, response, populated } => {
                assert_eq!(model, "gemma3");
                assert!(!response.is_empty());
                assert_eq!(populated, Some(out.clone()));
                assert_eq!(std::fs::read_to_string(&out).unwrap(), response);
            }
            other => panic!("expected a Told outcome, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
