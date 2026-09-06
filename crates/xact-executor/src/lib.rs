//! Plan execution (spec sections 2, 20): runs an [`ExecutionPlan`] and
//! reports what actually happened. This is where real side effects occur —
//! everything upstream (parsing, semantic validation, planning) is pure.
//!
//! Scope so far: [`ExecutionPlan::Bank`] dispatches to `xact-bank`, and
//! [`ExecutionPlan::ViewDirectory`]/[`ExecutionPlan::ViewFile`] dispatch to
//! `xact-see`. Process supervision and resource control (spec section 22)
//! are not implemented — there is nothing to supervise yet.

use std::path::PathBuf;

use xact_planner::ExecutionPlan;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionOutcome {
    BankEstablished { path: PathBuf },
    /// A directory or file was shown via `gls`/`bat`, whose own inherited
    /// stdio already rendered the content — `tool` names which one ran.
    Viewed { path: PathBuf, tool: &'static str },
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
}
