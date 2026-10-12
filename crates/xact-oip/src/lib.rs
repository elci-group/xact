//! Real adapter for `£ EDIT` (SPEC §7, §11; ADR 0001): replaces a file's
//! content with the current clipboard text by invoking the ELci tool `oip`
//! (Overwrite In Place), which first copies the old content to
//! `<file>.oip.backup`.
//!
//! Everything `oip`-specific lives in this crate (MESUT §29, no semantic
//! leakage); callers only see [`edit`], [`check_target`], and [`Edit`].
//!
//! The invocation is always exactly `oip -- <absolute path>`: never `-i`
//! (which guesses the target) and never `--revert` (which restores every
//! backup in a directory, not one file). [`check_target`] enforces ADR
//! 0001's preconditions before anything is spawned:
//!
//! - the target exists and is a regular file: `oip` would otherwise create
//!   it, and creating files belongs to `CREATE`/`bank` (SPEC §6);
//! - it is not a symlink, matching `COPY`'s rule, so an edit cannot land
//!   outside the owner's domain through a link;
//! - its path contains no `{` or `}`, because `oip` brace-expands its
//!   target (`a{1,2}.rs` would edit two files) and has no way to turn that
//!   off.
//!
//! `oip` runs under the caller's `ResourceBudget` via `xact-resource`
//! (cgroup limits, and a private network namespace under `! WITHOUT
//! network`) and is registered with `xact-cancel` so CTRL-C stops it.
//! Its stdout (a short before/after preview) is captured and returned; a
//! non-zero exit (empty or non-text clipboard, no desktop session, a write
//! error) becomes an [`OipError`] carrying `oip`'s own message, and `oip`
//! leaves the file untouched in those cases.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use xact_ast::ResourceBudget;

/// `oip`'s backup naming: the target's full file name plus this suffix.
const BACKUP_SUFFIX: &str = ".oip.backup";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OipError(String);

impl fmt::Display for OipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OipError {}

/// A completed edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub path: PathBuf,
    /// Where `oip` saved the previous content. `None` only if `oip`
    /// reported success without leaving its backup, which callers should
    /// surface rather than hide.
    pub backup: Option<PathBuf>,
    /// `oip`'s own before/after summary, one line per entry.
    pub preview: Vec<String>,
}

/// Where `oip` keeps the previous content of `path`.
pub fn backup_path_for(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// ADR 0001's preconditions for `EDIT`, checked without spawning anything.
/// The planner calls this too, so a target that can never be edited is
/// refused before it is scheduled. `Err` names the problem and the fix.
pub fn check_target(path: &Path) -> Result<(), OipError> {
    let shown = path.display();
    if path.as_os_str().to_string_lossy().contains(['{', '}']) {
        return Err(OipError(format!(
            "EDIT cannot target '{shown}': `oip` would brace-expand the path into several files. \
             Rename the file or edit it another way."
        )));
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(OipError(format!(
                "EDIT needs an existing file, and '{shown}' does not exist. Create it first with £ CREATE."
            )))
        }
        Err(err) => return Err(OipError(format!("EDIT cannot inspect '{shown}': {err}"))),
    };
    if metadata.file_type().is_symlink() {
        return Err(OipError(format!("EDIT refuses '{shown}' because it is a symlink; edit the file it points to.")));
    }
    if metadata.is_dir() {
        return Err(OipError(format!("EDIT needs a file, and '{shown}' is a directory.")));
    }
    if !metadata.is_file() {
        return Err(OipError(format!("EDIT needs a regular file, and '{shown}' is not one.")));
    }
    Ok(())
}

/// Replaces `path`'s content with the clipboard text through the real
/// `oip` binary on `$PATH`, under `budget`.
pub fn edit(path: &Path, budget: &ResourceBudget) -> Result<Edit, OipError> {
    edit_with(Path::new("oip"), path, budget)
}

/// [`edit`] with an explicit `oip` binary, so tests can substitute a stub.
fn edit_with(oip: &Path, path: &Path, budget: &ResourceBudget) -> Result<Edit, OipError> {
    check_target(path)?;

    let mut command = Command::new(oip);
    // `--` so a path can never be read as an option.
    command.arg("--").arg(path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let _resource_guard = xact_resource::apply(&mut command, budget)
        .map_err(|err| OipError(format!("cannot honour the active resource policy: {err}")))?;

    let child = command.spawn().map_err(|err| OipError(format!("failed to run oip: {err}")))?;
    let _cancel_guard = xact_cancel::register(child.id());
    let output = child.wait_with_output().map_err(|err| OipError(format!("failed to wait for oip: {err}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        return Err(OipError(if message.is_empty() {
            format!("oip exited with {} and left '{}' untouched", output.status, path.display())
        } else {
            format!("oip: {message}")
        }));
    }

    let backup = backup_path_for(path);
    Ok(Edit {
        path: path.to_path_buf(),
        backup: backup.is_file().then_some(backup),
        preview: stdout.lines().map(str::trim_end).filter(|line| !line.is_empty()).map(str::to_string).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use xact_ast::Capability;

    fn scratch() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "xact-oip-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes an executable stub `oip` through a child shell, so this
    /// multi-threaded test process never holds a writable fd to it (a
    /// concurrent fork inheriting one would make exec fail with ETXTBSY).
    /// The stub records its arguments, then behaves like `oip`: back up,
    /// overwrite with `$STUB_TEXT`, print a preview, or fail with
    /// `$STUB_FAIL` on stderr.
    fn stub(dir: &Path) -> PathBuf {
        let script = dir.join("oip-stub");
        let body = r#"#!/bin/sh
printf '%s\n' "$@" > "$0.args"
if [ -n "$STUB_FAIL" ]; then echo "$STUB_FAIL" >&2; exit 1; fi
target="$2"
cp "$target" "$target.oip.backup"
printf '%s' "$STUB_TEXT" > "$target"
echo "\"$target\": 1 line(s) -> 1 line(s)"
echo "Applied clipboard to \"$target\""
"#;
        let status = Command::new("sh")
            .args(["-c", "printf '%s' \"$1\" > \"$2\" && chmod 755 \"$2\"", "sh", body])
            .arg(&script)
            .status()
            .unwrap();
        assert!(status.success());
        script
    }

    fn with_env<T>(vars: &[(&str, &str)], body: impl FnOnce() -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (key, value) in vars {
            std::env::set_var(key, value);
        }
        let result = body();
        for (key, _) in vars {
            std::env::remove_var(key);
        }
        result
    }

    #[test]
    fn edit_invokes_oip_with_a_separator_and_reports_backup_and_preview() {
        let dir = scratch();
        let oip = stub(&dir);
        let target = dir.join("notes.md");
        std::fs::write(&target, "old").unwrap();

        let edit = with_env(&[("STUB_TEXT", "new")], || edit_with(&oip, &target, &ResourceBudget::default())).unwrap();

        let args = std::fs::read_to_string(dir.join("oip-stub.args")).unwrap();
        assert_eq!(args.lines().collect::<Vec<_>>(), vec!["--", target.to_str().unwrap()]);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(edit.backup, Some(backup_path_for(&target)));
        assert_eq!(std::fs::read_to_string(backup_path_for(&target)).unwrap(), "old");
        assert_eq!(edit.preview.len(), 2);
    }

    #[test]
    fn oip_failure_carries_its_own_message() {
        let dir = scratch();
        let oip = stub(&dir);
        let target = dir.join("notes.md");
        std::fs::write(&target, "old").unwrap();

        let err = with_env(&[("STUB_FAIL", "Clipboard is empty; leaving it untouched")], || {
            edit_with(&oip, &target, &ResourceBudget::default())
        })
        .unwrap_err();
        assert!(err.to_string().contains("Clipboard is empty"), "{err}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
    }

    /// Each refused target must be refused before `oip` is spawned: the
    /// stub never records an invocation.
    #[test]
    fn invalid_targets_are_refused_without_spawning_oip() {
        let dir = scratch();
        let oip = stub(&dir);
        let real = dir.join("real.txt");
        std::fs::write(&real, "x").unwrap();
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let braces = dir.join("a{1,2}.txt");
        std::fs::write(&braces, "x").unwrap();

        for (target, expected) in [
            (dir.join("missing.txt"), "does not exist"),
            (dir.clone(), "is a directory"),
            (link, "symlink"),
            (braces, "brace-expand"),
        ] {
            let err = edit_with(&oip, &target, &ResourceBudget::default()).unwrap_err();
            assert!(err.to_string().contains(expected), "{target:?}: {err}");
        }
        assert!(!dir.join("oip-stub.args").exists(), "oip must never have been spawned");
    }

    #[test]
    fn an_unenforceable_budget_fails_before_spawning() {
        let dir = scratch();
        let oip = stub(&dir);
        let target = dir.join("notes.md");
        std::fs::write(&target, "old").unwrap();
        let budget = ResourceBudget { unenforceable: vec!["GPU".into()], ..Default::default() };

        assert!(edit_with(&oip, &target, &budget).is_err());
        assert!(!dir.join("oip-stub.args").exists());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
    }

    /// The real tool against the real desktop clipboard: puts known text
    /// on the clipboard, edits a scratch file through `oip`, and checks the
    /// content and backup. Restores the previous clipboard text afterwards.
    fn live_round_trip(budget: ResourceBudget) {
        let mut clipboard = arboard::Clipboard::new().expect("a desktop clipboard is required");
        let previous = clipboard.get_text().ok();
        let text = format!("xact live edit {}\n", std::process::id());
        clipboard.set_text(text.clone()).unwrap();

        let dir = scratch();
        let target = dir.join("live.txt");
        std::fs::write(&target, "before\n").unwrap();
        let result = edit(&target, &budget);

        if let Some(previous) = previous {
            let _ = clipboard.set_text(previous);
        }
        let edit = result.expect("oip should apply the clipboard");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), text);
        assert_eq!(edit.backup.as_deref().map(std::fs::read_to_string).unwrap().unwrap(), "before\n");
    }

    #[test]
    #[ignore = "live: requires oip and a desktop clipboard"]
    fn live_edit_applies_the_real_clipboard() {
        live_round_trip(ResourceBudget::default());
    }

    #[test]
    #[ignore = "live: requires oip, a desktop clipboard, and unprivileged user namespaces"]
    fn live_edit_works_inside_the_network_isolated_namespace() {
        live_round_trip(ResourceBudget { denied: vec![Capability::Network], ..Default::default() });
    }
}
