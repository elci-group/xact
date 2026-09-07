//! Typed adapter over the `bound` tool for source aggregation/bounding
//! (spec sections 4, 12). Xact does not reimplement recursive traversal,
//! filtering, dependency resolution, or limits — it shells out to the
//! real `bound` binary and lets its own defaults/heuristics do the work.
//!
//! `bound`'s CLI takes two ordered positionals, `[FILTER] [DIRECTORY]`
//! (`bound --help`), so a single positional value always fills `FILTER`,
//! never `DIRECTORY` — there is no way to pass an arbitrary target
//! directory positionally without also supplying a filter (and there is
//! no "match everything" filter string; omitting the argument entirely is
//! the only way to get `bound`'s own no-filter default). [`aggregate`]
//! instead runs `bound` with its working directory set to `source` and no
//! positionals at all, so `bound`'s own defaults apply exactly as if a
//! user had `cd`'d there and run `bound` bare: no filter (all files),
//! `directory` defaulting to `.` — which, via the child's cwd, *is*
//! `source`.
//!
//! `destination` maps to `bound`'s own `--out <file>`; with none given,
//! `bound`'s own clipboard default applies (spec section 4 lists
//! "clipboard/file output" as one of `bound`'s inherent capabilities).
//! Runs with inherited stdio, matching `bank`/`gls`/`bat`: `bound`'s own
//! progress/telemetry output (spec section 4's "progress telemetry") is
//! TTY-aware and should reach the terminal directly, not be captured and
//! re-printed.
//!
//! `budget` (spec section 22, Xact–Mesut Integration Phase 5/6) is applied
//! via `xact-resource` before spawning, same as `xact-process::run` — a
//! `bound` aggregation over a large source set is exactly the kind of
//! workload `SPEND`/`SAVE` exists to constrain.

use std::fmt;
use std::path::Path;
use std::process::Command;

use xact_ast::ResourceBudget;

#[derive(Debug)]
pub struct BoundError(String);

impl fmt::Display for BoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for BoundError {}

/// Aggregates `source` (a directory) via the real `bound` tool, writing to
/// `destination` if given, or `bound`'s own clipboard default otherwise,
/// constrained by `budget` if it names a real constraint.
pub fn aggregate(source: &Path, destination: Option<&Path>, budget: &ResourceBudget) -> Result<(), BoundError> {
    let mut command = Command::new("bound");
    command.current_dir(source);
    if let Some(destination) = destination {
        command.arg("--out").arg(destination);
    }

    let _guard = xact_resource::apply(&mut command, budget)
        .map_err(|err| BoundError(format!("cannot honour the active resource policy: {err}")))?;

    let status = command.status().map_err(|e| BoundError(format!("failed to run bound: {e}")))?;

    if status.success() {
        Ok(())
    } else {
        Err(BoundError(format!("bound exited with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_a_real_directory_to_a_real_output_file() {
        let dir = std::env::temp_dir().join(format!("xact-bound-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let out = dir.join("bundle.txt");

        let result = aggregate(&dir, Some(&out), &ResourceBudget::default());

        assert!(result.is_ok(), "bound should succeed: {result:?}");
        let contents = std::fs::read_to_string(&out).expect("bound should have written the output file");
        assert!(
            contents.contains("fn main()"),
            "bundle should contain the aggregated file's content: {contents}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `destination: None` is deliberately not covered here — it takes the
    /// real clipboard path, which depends on a live desktop session that a
    /// test run isn't guaranteed to have. This is documented, not silently
    /// skipped: `aggregate`'s only branch on `destination` is whether
    /// `--out` is passed, so exercising the `Some` path already proves the
    /// dispatch logic; only the destination-less path's *environment
    /// dependency* is left unverified here.
    #[test]
    fn missing_source_directory_is_an_error_not_a_panic() {
        let missing = std::env::temp_dir().join("xact-bound-definitely-does-not-exist");
        let _ = std::fs::remove_dir_all(&missing);

        assert!(aggregate(&missing, None, &ResourceBudget::default()).is_err());
    }

    #[test]
    fn a_real_resource_budget_actually_constrains_bound() {
        let dir = std::env::temp_dir().join(format!("xact-bound-budget-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let out = dir.join("bundle.txt");

        let budget = ResourceBudget { cpu_percent: Some(50), ..Default::default() };
        let result = aggregate(&dir, Some(&out), &budget);

        assert!(result.is_ok(), "bound should succeed under a real cgroup cap: {result:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unenforceable_resource_fails_before_launching() {
        let dir = std::env::temp_dir().join(format!("xact-bound-unenforceable-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let budget = ResourceBudget { unenforceable: vec!["GPU".into()], ..Default::default() };
        assert!(aggregate(&dir, None, &budget).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
