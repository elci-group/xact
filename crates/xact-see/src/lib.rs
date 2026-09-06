//! Typed adapter for `SEE` (spec section 7): directories are shown with the
//! ELci `gls` tool, files with `bat`. Xact does not reimplement directory
//! listing or syntax-highlighted file viewing — it composes existing tools
//! (spec section 3), the same principle sections 4/6 apply to `bound`/`bank`.
//!
//! Which tool applies is a routing decision Xact itself must make — unlike
//! `CREATE` (one tool, `bank`, handles both files and directories via its
//! own heuristics), `SEE` has no single tool that covers both file and
//! directory targets, so `xact-planner` inspects the resolved path and
//! picks one of these two entry points.
//!
//! Both run with inherited stdio rather than captured output: `gls`'s grid/
//! animation and `bat`'s syntax highlighting are TTY-aware, and capturing
//! then re-printing would flatten that.

use std::fmt;
use std::path::Path;
use std::process::Command;

#[derive(Debug)]
pub struct SeeError(String);

impl fmt::Display for SeeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SeeError {}

pub fn view_directory(path: &Path) -> Result<(), SeeError> {
    run("gls", path)
}

pub fn view_file(path: &Path) -> Result<(), SeeError> {
    run("bat", path)
}

fn run(tool: &str, path: &Path) -> Result<(), SeeError> {
    let status = Command::new(tool)
        .arg(path)
        .status()
        .map_err(|e| SeeError(format!("failed to run {tool}: {e}")))?;

    if status.success() {
        Ok(())
    } else {
        Err(SeeError(format!("{tool} exited with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_stub(dir: &Path, name: &str, exit_code: i32) {
        std::fs::create_dir_all(dir).unwrap();
        let script = dir.join(name);
        std::fs::write(&script, format!("#!/bin/sh\nexit {exit_code}\n")).unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }

    /// One test, not several: `PATH` is process-global mutable state, and
    /// cargo runs tests in parallel by default, so a separate `PATH`-free
    /// test asserting against the real `gls` could race against another
    /// test's `PATH` mutation. Keeping every `PATH`-sensitive case
    /// sequential in one function sidesteps that without depending on how
    /// the harness is invoked. `bat`'s dispatch is verified against an
    /// executable stub rather than the real binary, so this test doesn't
    /// depend on `bat` being installed or on the exact exit-code behavior
    /// of whatever version is.
    #[test]
    fn see_adapter_dispatches_by_tool_name() {
        assert!(
            view_directory(&std::env::temp_dir()).is_ok(),
            "gls is expected to be installed and to succeed on a real directory"
        );

        let original_path = std::env::var("PATH").unwrap_or_default();

        let ok_dir = std::env::temp_dir().join(format!("xact-see-stub-ok-{}", std::process::id()));
        write_stub(&ok_dir, "bat", 0);
        let file = std::env::temp_dir().join(format!("xact-see-test-file-{}", std::process::id()));
        std::fs::write(&file, "hello").unwrap();

        std::env::set_var("PATH", format!("{}:{}", ok_dir.display(), original_path));
        assert!(view_file(&file).is_ok(), "a zero-exit stub should be reported as success");

        let fail_dir = std::env::temp_dir().join(format!("xact-see-stub-fail-{}", std::process::id()));
        write_stub(&fail_dir, "bat", 1);
        std::env::set_var("PATH", format!("{}:{}", fail_dir.display(), original_path));
        assert!(view_file(&file).is_err(), "a nonzero-exit stub should be reported as failure");

        let empty_dir = std::env::temp_dir().join(format!("xact-see-empty-path-{}", std::process::id()));
        std::fs::create_dir_all(&empty_dir).unwrap();
        std::env::set_var("PATH", &empty_dir);
        assert!(view_file(&file).is_err(), "a missing tool should be reported as failure, not panic");

        std::env::set_var("PATH", original_path);
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir_all(&ok_dir);
        let _ = std::fs::remove_dir_all(&fail_dir);
        let _ = std::fs::remove_dir_all(&empty_dir);
    }
}
