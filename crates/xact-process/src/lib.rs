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
//! Ownership domains give `RUN` real behavioral meaning (spec section 10)
//! rather than being purely grammatical: `MY` (the default when the
//! operand states none — `£ RUN 'chrome'` and `£ RUN MY chrome` behave
//! identically) resolves the program the same way a shell would — via the
//! process's own `$PATH`, letting `Command::new` do the actual search so
//! `argv[0]` stays exactly what was typed when that succeeds. If `$PATH`
//! search genuinely fails (`ErrorKind::NotFound`) and the program name is
//! bare (no `/`), `MY` falls back to [`SYSTEM_BIN_DIRS`] — Xact's `OUR`
//! domain, the standard POSIX system binary directories (the same list
//! most Linux `sudo` configurations use as `secure_path`) — before giving
//! up for real. An explicit `£ RUN OUR chrome` searches that list
//! directly, skipping `$PATH` entirely. `THEIR` is unchanged: a plain
//! `$PATH` search, no fallback (this domain's real meaning is the
//! identity-gated semantics `xact-semantic` already enforces, not binary
//! resolution).
//!
//! Cancellation (spec section 12; Xact–Mesut Integration Phase 8,
//! continued): the spawned child's pid is registered with `xact-cancel`
//! between `spawn` and `wait`, so a real `CTRL-C` (via `xact-cli`'s
//! handler) can actually kill it — a signal-terminated child just makes
//! `wait` return normally with a `None` exit code, which the "terminated
//! by signal" handling downstream already existed to describe.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use xact_ast::{OwnershipKind, ResourceBudget};

#[derive(Debug)]
pub struct ProcessError(String);

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ProcessError {}

/// The `OUR` ownership domain's search list: the standard POSIX/Linux
/// system binary directories, not user-configurable. If a shared binary
/// genuinely lives somewhere else, `£ RUN OUR ...` honestly won't find
/// it, rather than silently guessing at other locations.
pub const SYSTEM_BIN_DIRS: &[&str] = &["/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin"];

/// Runs `command_line`, naively whitespace-split into a program and its
/// arguments, with inherited stdio, and waits for it to exit. `domain`
/// governs how the program is resolved (see the module docs); `budget`
/// (a session's currently-established `SPEND`/`SAVE` policy, or
/// [`ResourceBudget::default`] for none) is applied via `xact-resource`
/// before the process is spawned — a nonempty budget that can't actually
/// be enforced fails the call outright, before anything runs, rather than
/// launching unconstrained.
pub fn run(command_line: &str, domain: OwnershipKind, budget: &ResourceBudget) -> Result<ExitStatus, ProcessError> {
    run_with_dirs(command_line, domain, budget, SYSTEM_BIN_DIRS)
}

fn run_with_dirs(
    command_line: &str,
    domain: OwnershipKind,
    budget: &ResourceBudget,
    our_dirs: &[&str],
) -> Result<ExitStatus, ProcessError> {
    let mut parts = command_line.split_whitespace();
    let Some(program) = parts.next() else {
        return Err(ProcessError("RUN was given an empty command.".into()));
    };
    let args: Vec<&str> = parts.collect();

    if domain == OwnershipKind::Our {
        return match find_in_dirs(program, our_dirs) {
            Some(resolved) => spawn_and_wait(&resolved, &args, budget).map_err(|f| f.describe(program)),
            None => Err(ProcessError(format!(
                "'{program}' was not found in Xact's shared system binary directories."
            ))),
        };
    }

    match spawn_and_wait(Path::new(program), &args, budget) {
        Ok(status) => Ok(status),
        Err(SpawnFailure::NotFound) if domain == OwnershipKind::My && !program.contains('/') => {
            match find_in_dirs(program, our_dirs) {
                Some(resolved) => spawn_and_wait(&resolved, &args, budget).map_err(|f| f.describe(program)),
                None => Err(ProcessError(format!(
                    "'{program}' was not found on $PATH or in Xact's shared system binary directories."
                ))),
            }
        }
        Err(failure) => Err(failure.describe(program)),
    }
}

fn find_in_dirs(program: &str, dirs: &[&str]) -> Option<PathBuf> {
    dirs.iter().map(|dir| Path::new(dir).join(program)).find(|candidate| candidate.is_file())
}

enum SpawnFailure {
    NotFound,
    Other(ProcessError),
}

impl SpawnFailure {
    fn describe(self, program: &str) -> ProcessError {
        match self {
            SpawnFailure::NotFound => ProcessError(format!("failed to run '{program}': No such file or directory")),
            SpawnFailure::Other(err) => err,
        }
    }
}

fn spawn_and_wait(program: &Path, args: &[&str], budget: &ResourceBudget) -> Result<ExitStatus, SpawnFailure> {
    let mut command = Command::new(program);
    command.args(args);

    let _resource_guard = xact_resource::apply(&mut command, budget).map_err(|err| {
        SpawnFailure::Other(ProcessError(format!("cannot honour the active resource policy: {err}")))
    })?;

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(SpawnFailure::NotFound),
        Err(e) => {
            return Err(SpawnFailure::Other(ProcessError(format!("failed to run '{}': {e}", program.display()))))
        }
    };
    let _cancel_guard = xact_cancel::register(child.id());
    child
        .wait()
        .map_err(|e| SpawnFailure::Other(ProcessError(format!("failed to wait for '{}': {e}", program.display()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_command_reports_success() {
        let status = run("true", OwnershipKind::My, &ResourceBudget::default()).expect("true should launch");
        assert!(status.success());
    }

    #[test]
    fn failing_command_reports_failure_not_an_error() {
        let status = run("false", OwnershipKind::My, &ResourceBudget::default()).expect("false should launch");
        assert!(!status.success());
    }

    #[test]
    fn arguments_are_split_on_whitespace_and_passed_through() {
        // "sh -c true" splits into program="sh", args=["-c", "true"] — sh
        // runs the command `true`, exit 0. This only succeeds if both
        // words after the program name reached sh as separate argv entries.
        let status = run("sh -c true", OwnershipKind::My, &ResourceBudget::default()).expect("sh should launch");
        assert!(status.success());
    }

    #[test]
    fn naive_splitting_does_not_understand_quoting() {
        // A quoted multi-word argument gets split into separate argv
        // entries instead of staying together: sh receives `"exit` and
        // `3"` as two broken tokens rather than the single script
        // `exit 3`, and fails — the documented "no quoting" limitation.
        let status = run("sh -c \"exit 3\"", OwnershipKind::My, &ResourceBudget::default()).expect("sh should launch");
        assert!(!status.success());
    }

    #[test]
    fn missing_program_is_an_error_not_a_panic() {
        assert!(run("xact-definitely-not-a-real-binary", OwnershipKind::My, &ResourceBudget::default()).is_err());
    }

    #[test]
    fn empty_command_is_an_error() {
        assert!(run("   ", OwnershipKind::My, &ResourceBudget::default()).is_err());
    }

    #[test]
    fn a_real_resource_budget_actually_caps_the_process() {
        let budget = ResourceBudget { cpu_percent: Some(40), ..Default::default() };
        let status = run("true", OwnershipKind::My, &budget).expect("true should launch under a real cgroup cap");
        assert!(status.success());
    }

    #[test]
    fn an_unenforceable_resource_fails_before_launching() {
        let budget = ResourceBudget { unenforceable: vec!["GPU".to_string()], ..Default::default() };
        assert!(run("true", OwnershipKind::My, &budget).is_err());
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

        let handle = std::thread::spawn(|| run("sleep 10", OwnershipKind::My, &ResourceBudget::default()));
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

    fn write_stub(dir: &std::path::Path, name: &str) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let script = dir.join(name);
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&script, perms).unwrap();
        dir.to_str().unwrap().to_string()
    }

    #[test]
    fn system_bin_dirs_contains_the_real_standard_locations() {
        assert!(SYSTEM_BIN_DIRS.contains(&"/usr/bin"));
        assert!(SYSTEM_BIN_DIRS.contains(&"/bin"));
    }

    #[test]
    fn our_domain_resolves_directly_from_the_shared_directories() {
        let dir = std::env::temp_dir().join(format!("xact-process-our-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_str = write_stub(&dir, "xact-our-stub");

        let status = run_with_dirs("xact-our-stub", OwnershipKind::Our, &ResourceBudget::default(), &[&dir_str])
            .expect("the stub should launch from the OUR directory");
        assert!(status.success());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn our_domain_never_consults_path() {
        // "true" is a real $PATH-findable program, but an explicit OUR
        // must resolve only against the given directories, never $PATH —
        // an empty OUR directory list must fail even for "true".
        let result = run_with_dirs("true", OwnershipKind::Our, &ResourceBudget::default(), &[]);
        assert!(result.is_err());
    }

    #[test]
    fn my_falls_back_to_our_directories_when_not_on_path() {
        let dir = std::env::temp_dir().join(format!("xact-process-fallback-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_str = write_stub(&dir, "xact-fallback-stub");

        // Not a real $PATH-findable name, so the primary attempt must
        // fail with NotFound and MY must fall back to the given OUR
        // directory to find it.
        let status =
            run_with_dirs("xact-fallback-stub", OwnershipKind::My, &ResourceBudget::default(), &[&dir_str])
                .expect("MY should fall back to the OUR directory and find the stub");
        assert!(status.success());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn their_does_not_fall_back_to_our_directories() {
        let dir = std::env::temp_dir().join(format!("xact-process-their-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_str = write_stub(&dir, "xact-their-stub");

        let result =
            run_with_dirs("xact-their-stub", OwnershipKind::Their, &ResourceBudget::default(), &[&dir_str]);
        assert!(result.is_err(), "THEIR should behave like a plain $PATH search, with no OUR fallback");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_like_program_never_falls_back() {
        // Contains '/', so PATH-search semantics (and therefore the MY ->
        // OUR fallback) never apply — a missing file at that exact
        // relative path is a real, honest failure, not a search miss.
        let result = run_with_dirs("./definitely/not/a/real/path", OwnershipKind::My, &ResourceBudget::default(), &[
            "/usr/bin",
        ]);
        assert!(result.is_err());
    }
}
