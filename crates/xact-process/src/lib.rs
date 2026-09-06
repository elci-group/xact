//! Process execution for `RUN` (spec section 21: `RUN` -> native process
//! execution — no ELci tool owns this, so Xact runs it itself).
//!
//! Scope so far: launches a program with inherited stdio and waits for it
//! to exit (foreground, blocking) — the simplest, most predictable "shell
//! 101" behavior, and the natural default to pair with `! SAVE`/`! WITH`
//! policy on a single command before there is a real scheduler to honor
//! `CONCURRENTLY`/backgrounding. Resource control (spec section 22 —
//! `SPEND`/`SAVE` as real kernel-native constraints) and supervision of
//! multiple concurrent/consecutive processes (section 23) are not
//! implemented yet; this crate only runs one process and reports how it
//! exited.
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

use std::fmt;
use std::process::{Command, ExitStatus};

#[derive(Debug)]
pub struct ProcessError(String);

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ProcessError {}

/// Runs `command_line`, naively whitespace-split into a program and its
/// arguments, with inherited stdio, and waits for it to exit.
pub fn run(command_line: &str) -> Result<ExitStatus, ProcessError> {
    let mut parts = command_line.split_whitespace();
    let Some(program) = parts.next() else {
        return Err(ProcessError("RUN was given an empty command.".into()));
    };
    let args: Vec<&str> = parts.collect();

    Command::new(program)
        .args(&args)
        .status()
        .map_err(|e| ProcessError(format!("failed to run '{program}': {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_command_reports_success() {
        let status = run("true").expect("true should launch");
        assert!(status.success());
    }

    #[test]
    fn failing_command_reports_failure_not_an_error() {
        let status = run("false").expect("false should launch");
        assert!(!status.success());
    }

    #[test]
    fn arguments_are_split_on_whitespace_and_passed_through() {
        // "sh -c true" splits into program="sh", args=["-c", "true"] — sh
        // runs the command `true`, exit 0. This only succeeds if both
        // words after the program name reached sh as separate argv entries.
        let status = run("sh -c true").expect("sh should launch");
        assert!(status.success());
    }

    #[test]
    fn naive_splitting_does_not_understand_quoting() {
        // A quoted multi-word argument gets split into separate argv
        // entries instead of staying together: sh receives `"exit` and
        // `3"` as two broken tokens rather than the single script
        // `exit 3`, and fails — the documented "no quoting" limitation.
        let status = run("sh -c \"exit 3\"").expect("sh should launch");
        assert!(!status.success());
    }

    #[test]
    fn missing_program_is_an_error_not_a_panic() {
        assert!(run("xact-definitely-not-a-real-binary").is_err());
    }

    #[test]
    fn empty_command_is_an_error() {
        assert!(run("   ").is_err());
    }
}
