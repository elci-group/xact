//! `@ TELL <harness>` — shelling out to a real CLI coding-agent harness
//! that supports a `-p`/print (non-interactive) prompt flag, exactly the
//! same "compose, don't reimplement" boundary the rest of `xact-tell`
//! draws with `ollama`. Deliberately a small, fixed, hand-verified
//! registry rather than probing arbitrary `$PATH` binaries for `-p`
//! support (`£ RUN` doesn't need that here either) — a wrong guess at an
//! unfamiliar CLI's flags would silently misinvoke it.
//!
//! Detection intentionally does *not* reuse `xact_resolve::system_bin_dirs`
//! — that's a curated, Padagonia-backed fixed list built for `£ RUN`'s own
//! sandboxed execution model, not a stand-in for "what's really on this
//! machine's `$PATH`". Harness detection is closer to a real `which`: it
//! reads the actual `$PATH` environment variable.

use std::path::PathBuf;
use std::process::Command;

use xact_ast::ResourceBudget;

use crate::TellError;

/// One real, hand-verified CLI harness this build knows how to prompt
/// non-interactively, and the flag that puts it in that mode.
struct Harness {
    binary: &'static str,
    flag: &'static str,
}

/// Verified, text-in/text-out harnesses: each one's non-interactive mode
/// behaves like a stateless "answer this" call, matching `TELL`'s
/// existing semantics (a response is captured and optionally written to
/// a file — nothing here reads or edits the rest of the filesystem on
/// its own). Deliberately excludes several other real, researched
/// harnesses — `codex exec`, Meta's `muse exec`, `kilo run --auto`,
/// `aider --message ... --yes-always` — because those are autonomous
/// coding agents: invoked this way they can read/edit files and run
/// shell commands in `xact`'s working directory as a side effect of
/// answering the prompt, a materially different blast radius than a
/// text completion. Confirmed with the user to hold off on those until
/// there's a real design for that (this file's registry format — one
/// flag before the prompt — doesn't even fit their invocation shapes
/// yet: `codex`/`muse` take a subcommand instead of a flag, `kilo` needs
/// both a subcommand and a flag, and `aider` needs a flag *after* the
/// prompt too).
const HARNESSES: &[Harness] = &[
    Harness { binary: "claude", flag: "-p" },
    Harness { binary: "kimi", flag: "-p" },
    Harness { binary: "gemini", flag: "-p" },
    // Deliberately no `--force`: without it, `cursor-agent` only
    // proposes changes rather than applying them, keeping this harness
    // in the same "text response, no side effects" category as the
    // others here.
    Harness { binary: "cursor-agent", flag: "-p" },
];

/// The real directories named in `$PATH`, in order — Unix `:`-separated,
/// same convention every shell uses. Empty if `$PATH` is unset.
fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default()
}

/// Whether `binary` is a real, present file somewhere in `dirs` — the
/// same "does a plain file with this name exist" check
/// `xact_resolve::find_binary_in_dirs` already uses, just against a
/// caller-supplied directory list instead of the curated ontology list.
/// Takes `dirs` as a parameter rather than reading `$PATH` itself for
/// exactly the reason `find_binary_in_dirs` does: `$PATH` is process-
/// global, and mutating it in a test would race every *other*
/// concurrently-running test that spawns a real subprocess (`ollama`,
/// `bound`, ...) and needs the real `$PATH` intact to find it — Rust's
/// default test harness runs `#[test]`s as threads in one process, not
/// separate processes, so that's a real, not theoretical, hazard.
fn is_on_path(binary: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| dir.join(binary).is_file())
}

fn available_in(dirs: &[PathBuf]) -> Vec<String> {
    HARNESSES.iter().filter(|h| is_on_path(h.binary, dirs)).map(|h| h.binary.to_string()).collect()
}

fn resolve_in(name: &str, dirs: &[PathBuf]) -> Option<&'static str> {
    HARNESSES.iter().find(|h| h.binary == name && is_on_path(h.binary, dirs)).map(|h| h.flag)
}

/// Every registered harness that's actually installed on this machine
/// right now — real, not assumed.
pub fn available() -> Vec<String> {
    available_in(&path_dirs())
}

/// Whether `name` names a registered, currently-installed harness.
pub fn resolve(name: &str) -> Option<&'static str> {
    resolve_in(name, &path_dirs())
}

/// Runs `prompt` through the named harness binary, constrained by
/// `budget` — the exact same spawn/resource/cancel shape `tell`'s Ollama
/// path already uses, just aimed at a different binary and a `-p`-style
/// flag instead of `run <model>`.
pub fn run(binary: &str, flag: &str, prompt: &str, budget: &ResourceBudget) -> Result<String, TellError> {
    let mut command = Command::new(binary);
    command.arg(flag).arg(prompt);

    let _resource_guard = xact_resource::apply(&mut command, budget)
        .map_err(|err| TellError(format!("cannot honour the active resource policy: {err}")))?;

    command.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let child = command.spawn().map_err(|err| TellError(format!("failed to run {binary}: {err}")))?;
    let _cancel_guard = xact_cancel::register(child.id());
    let output = child.wait_with_output().map_err(|err| TellError(format!("failed to wait for {binary}: {err}")))?;
    if !output.status.success() {
        return Err(TellError(format!(
            "{binary} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_finds_a_real_claude_binary_on_a_fake_directory() {
        let dir = std::env::temp_dir().join(format!("xact-tell-harness-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("claude"), b"#!/bin/sh\necho hi\n").unwrap();

        let dirs = [dir.clone()];
        assert_eq!(available_in(&dirs), vec!["claude".to_string()]);
        assert_eq!(resolve_in("claude", &dirs), Some("-p"));
        assert_eq!(resolve_in("not-a-real-harness", &dirs), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn available_only_lists_the_registered_harnesses_actually_present() {
        let dir = std::env::temp_dir().join(format!("xact-tell-harness-test-partial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kimi"), b"#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(dir.join("cursor-agent"), b"#!/bin/sh\necho hi\n").unwrap();
        // Not every registered binary needs to be present for the ones
        // that are to be found — `claude` and `gemini` are deliberately
        // absent from this fake directory.

        let dirs = [dir.clone()];
        let mut found = available_in(&dirs);
        found.sort();
        assert_eq!(found, vec!["cursor-agent".to_string(), "kimi".to_string()]);
        assert_eq!(resolve_in("kimi", &dirs), Some("-p"));
        assert_eq!(resolve_in("gemini", &dirs), None, "registered but not present in this directory");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_is_none_when_the_binary_is_missing_from_the_given_directories() {
        let dir = std::env::temp_dir().join(format!("xact-tell-harness-test-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let dirs = [dir.clone()];
        assert!(available_in(&dirs).is_empty());
        assert_eq!(resolve_in("claude", &dirs), None);

        std::fs::remove_dir_all(&dir).ok();
    }
}
