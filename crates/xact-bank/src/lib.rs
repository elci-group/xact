//! Typed adapter over the `bank` tool for resource establishment —
//! directories and empty files (spec sections 6, 13). Xact does not
//! reimplement `mkdir`+`touch` semantics; it shells out to the real `bank`
//! binary and lets bank's own file-vs-directory disambiguation (trailing
//! slash, extension heuristics, `-i` prompting) do its job.
//!
//! Always passed `-p`: spec section 6 lists "creates missing parent paths"
//! as one of `bank`'s inherent capabilities, and the shell "should not care
//! whether the underlying implementation requires... parent creation" —
//! that responsibility belongs to `bank`, so Xact always asks for it.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use xact_ast::ResourceBudget;

#[derive(Debug)]
pub struct BankError(String);

impl fmt::Display for BankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for BankError {}

/// Runs `bank -p <path>`, establishing the filesystem object at `path`
/// (a file or a directory — `bank` decides which from the path shape),
/// under `budget`'s quotas and capability denials like every other process
/// an executed command spawns (see `xact-resource`).
pub fn establish(path: &Path, budget: &ResourceBudget) -> Result<PathBuf, BankError> {
    let mut command = Command::new("bank");
    command.arg("-p").arg(path);
    let _resource_guard = xact_resource::apply(&mut command, budget)
        .map_err(|err| BankError(format!("cannot honour the active resource policy: {err}")))?;
    let output = command.output().map_err(|e| BankError(format!("failed to run bank: {e}")))?;

    if output.status.success() {
        Ok(path.to_path_buf())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(BankError(format!("bank exited with {}: {}", output.status, stderr.trim())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "live: requires bank"]
    fn establishes_a_file_with_missing_parents() {
        let dir = std::env::temp_dir().join(format!("xact-bank-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = dir.join("a/b/c/file.txt");

        let result = establish(&target, &ResourceBudget::default());

        assert!(result.is_ok(), "bank should succeed: {result:?}");
        assert!(target.is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "live: requires bank"]
    fn establishes_a_directory_from_trailing_slash() {
        let dir = std::env::temp_dir().join(format!("xact-bank-test-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = dir.join("project/");

        let result = establish(&target, &ResourceBudget::default());

        assert!(result.is_ok(), "bank should succeed: {result:?}");
        assert!(dir.join("project").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
