//! Plan execution (spec sections 2, 20): runs an [`ExecutionPlan`] and
//! reports what actually happened. This is where real side effects occur —
//! everything upstream (parsing, semantic validation, planning) is pure.
//!
//! Scope so far: [`ExecutionPlan::Bank`] dispatches to `xact-bank`,
//! [`ExecutionPlan::ViewDirectory`]/[`ExecutionPlan::ViewFile`] dispatch to
//! `xact-see`, and [`ExecutionPlan::Run`] dispatches to `xact-mesut`'s
//! execution adapter (Xact–Mesut Integration Phase 2 — see
//! `MESUT_INTEGRATION.md`), which submits the real process launch as
//! blocking work to a Mesut runtime instead of running it inline.
//! Resource control (spec section 22) is not implemented — a run is not
//! yet constrained by any `SPEND`/`SAVE` policy in effect.

use std::path::PathBuf;

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
    RunCompleted {
        command_line: String,
        success: bool,
        code: Option<i32>,
    },
    Failed { message: String },
}

pub fn execute(plan: ExecutionPlan) -> ExecutionOutcome {
    match plan {
        ExecutionPlan::Bank { path } => match xact_bank::establish(&path) {
            Ok(path) => ExecutionOutcome::BankEstablished { path },
            Err(err) => ExecutionOutcome::Failed { message: err.to_string() },
        },
        ExecutionPlan::ViewDirectory { path } => match xact_see::view_directory(&path) {
            Ok(()) => ExecutionOutcome::Viewed { path, tool: "gls" },
            Err(err) => ExecutionOutcome::Failed { message: err.to_string() },
        },
        ExecutionPlan::ViewFile { path } => match xact_see::view_file(&path) {
            Ok(()) => ExecutionOutcome::Viewed { path, tool: "bat" },
            Err(err) => ExecutionOutcome::Failed { message: err.to_string() },
        },
        ExecutionPlan::Run { command_line } => {
            match xact_mesut::run_process(command_line.clone()) {
                Ok(outcome) => ExecutionOutcome::RunCompleted {
                    command_line,
                    success: outcome.success,
                    code: outcome.code,
                },
                Err(err) => ExecutionOutcome::Failed { message: err.to_string() },
            }
        }
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

        let outcome = execute(ExecutionPlan::Bank { path: target.clone() });

        assert_eq!(outcome, ExecutionOutcome::BankEstablished { path: target.clone() });
        assert!(dir.join("nested/dir").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn view_directory_plan_runs_gls() {
        let outcome = execute(ExecutionPlan::ViewDirectory {
            path: std::env::temp_dir(),
        });
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
        let outcome = execute(ExecutionPlan::Run {
            command_line: "true".into(),
        });
        assert_eq!(
            outcome,
            ExecutionOutcome::RunCompleted {
                command_line: "true".into(),
                success: true,
                code: Some(0),
            }
        );
    }

    #[test]
    fn run_plan_reports_nonzero_exit_as_completed_not_failed() {
        let outcome = execute(ExecutionPlan::Run {
            command_line: "false".into(),
        });
        assert_eq!(
            outcome,
            ExecutionOutcome::RunCompleted {
                command_line: "false".into(),
                success: false,
                code: Some(1),
            }
        );
    }

    #[test]
    fn run_plan_reports_missing_binary_as_failed() {
        let outcome = execute(ExecutionPlan::Run {
            command_line: "xact-definitely-not-a-real-binary".into(),
        });
        assert!(matches!(outcome, ExecutionOutcome::Failed { .. }));
    }
}
