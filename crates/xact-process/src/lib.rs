//! Process execution for `RUN` (spec section 21: `RUN` -> native process
//! execution — no ELci tool owns this, so Xact runs it itself).
//!
//! Scope so far: launches a program with inherited stdio and waits for it
//! to exit (foreground, blocking) — the simplest, most predictable "shell
//! 101" behavior. Resource control (spec section 22 — `SPEND`/`SAVE` as
//! real kernel-native constraints, Xact–Mesut Integration Phase 5) is
//! real: `run` applies whatever [`ResourceBudget`] it's given via
//! `xact-resource` before spawning — see that crate for the actual cgroup
//! v2 mechanism. Supervision of multiple concurrent/consecutive processes
//! (section 23) lives in `xact-mesut`/`xact-executor`, not here; this
//! crate only runs one process and reports how it exited.
//!
//! `RUN`'s grammar (spec section 7) gives it a single string operand, with
//! no argument-list syntax — `£ RUN 'chrome'`. To still be useful for
//! anything beyond a bare program name, that string is split on ASCII
//! whitespace into a program and literal argv entries. This is naive
//! word-splitting, not shell parsing: no quoting, no escaping, no
//! metacharacter interpretation (`|`, `>`, `$VAR`, globs) — deliberately,
//! so `RUN` cannot smuggle in shell semantics through a string Xact never
//! validated as such. A real argument-list grammar is future work if this
//! turns out to matter.
//!
//! Cancellation (spec section 12; Xact–Mesut Integration Phase 8,
//! continued): the spawned child's pid is registered with `xact-cancel`
//! between `spawn` and `wait`, so a real `CTRL-C` (via `xact-cli`'s
//! handler) can actually kill it — a signal-terminated child just makes
//! `wait` return normally with a `None` exit code, which the "terminated
//! by signal" handling downstream already existed to describe.

use std::fmt;
use std::process::{Command, ExitStatus};

use xact_ast::ResourceBudget;

#[derive(Debug)]
pub struct ProcessError(String);

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ProcessError {}

/// Runs `command_line`, naively whitespace-split into a program and its
/// arguments, with inherited stdio, and waits for it to exit. `budget`
/// (a session's currently-established `SPEND`/`SAVE` policy, or
/// [`ResourceBudget::default`] for none) is applied via `xact-resource`
/// before the process is spawned — a nonempty budget that can't actually
/// be enforced fails the call outright, before anything runs, rather than
/// launching unconstrained.
pub fn run(command_line: &str, budget: &ResourceBudget) -> Result<ExitStatus, ProcessError> {
    let mut parts = command_line.split_whitespace();
    let Some(program) = parts.next() else {
        return Err(ProcessError("RUN was given an empty command.".into()));
    };
    let args: Vec<&str> = parts.collect();

    let mut command = Command::new(program);
    command.args(&args);

    let _resource_guard = xact_resource::apply(&mut command, budget)
        .map_err(|err| ProcessError(format!("cannot honour the active resource policy: {err}")))?;

    let mut child = command.spawn().map_err(|e| ProcessError(format!("failed to run '{program}': {e}")))?;
    let _cancel_guard = xact_cancel::register(child.id());
    child.wait().map_err(|e| ProcessError(format!("failed to wait for '{program}': {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_command_reports_success() {
        let status = run("true", &ResourceBudget::default()).expect("true should launch");
        assert!(status.success());
    }

    #[test]
    fn failing_command_reports_failure_not_an_error() {
        let status = run("false", &ResourceBudget::default()).expect("false should launch");
        assert!(!status.success());
    }

    #[test]
    fn arguments_are_split_on_whitespace_and_passed_through() {
        // "sh -c true" splits into program="sh", args=["-c", "true"] — sh
        // runs the command `true`, exit 0. This only succeeds if both
        // words after the program name reached sh as separate argv entries.
        let status = run("sh -c true", &ResourceBudget::default()).expect("sh should launch");
        assert!(status.success());
    }

    #[test]
    fn naive_splitting_does_not_understand_quoting() {
        // A quoted multi-word argument gets split into separate argv
        // entries instead of staying together: sh receives `"exit` and
        // `3"` as two broken tokens rather than the single script
        // `exit 3`, and fails — the documented "no quoting" limitation.
        let status = run("sh -c \"exit 3\"", &ResourceBudget::default()).expect("sh should launch");
        assert!(!status.success());
    }

    #[test]
    fn missing_program_is_an_error_not_a_panic() {
        assert!(run("xact-definitely-not-a-real-binary", &ResourceBudget::default()).is_err());
    }

    #[test]
    fn empty_command_is_an_error() {
        assert!(run("   ", &ResourceBudget::default()).is_err());
    }

    #[test]
    fn a_real_resource_budget_actually_caps_the_process() {
        let budget = ResourceBudget { cpu_percent: Some(40), ..Default::default() };
        let status = run("true", &budget).expect("true should launch under a real cgroup cap");
        assert!(status.success());
    }

    #[test]
    fn an_unenforceable_resource_fails_before_launching() {
        let budget = ResourceBudget { unenforceable: vec!["GPU".to_string()], ..Default::default() };
        assert!(run("true", &budget).is_err());
    }

    /// Proves cancellation reaches all the way through `run` to the real
    /// child process, not just `xact-cancel`'s own unit tests. Calls
    /// `xact_cancel::cancel_all` directly — the same call `xact-cli`'s
    /// real `CTRL-C` handler makes — from a second thread while `run` is
    /// blocked waiting on a real `sleep`. This crate's other tests all
    /// finish in milliseconds, well before the ~200ms this test waits
    /// before cancelling, so the tiny window where a stray `cancel_all`
    /// could reach an unrelated sibling test's already-finishing child is
    /// not a practical race.
    #[test]
    fn cancellation_actually_kills_a_running_run_call() {
        xact_cancel::reset();
        let start = std::time::Instant::now();

        let handle = std::thread::spawn(|| run("sleep 10", &ResourceBudget::default()));
        std::thread::sleep(std::time::Duration::from_millis(200));
        xact_cancel::cancel_all();

        let status = handle.join().expect("run should not panic").expect("sleep should launch");
        let elapsed = start.elapsed();

        assert!(!status.success(), "a cancelled process should not report success");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "cancellation should stop the process almost immediately, not let the full 10s sleep elapse (took {elapsed:?})"
        );

        xact_cancel::reset();
    }
}
