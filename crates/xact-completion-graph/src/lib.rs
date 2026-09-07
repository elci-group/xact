//! Dynamic completion candidates for `xact-cli`'s live autocomplete.
//!
//! [`xact_completion::complete`] answers "what keyword comes next" by
//! reflecting the parser's own `expected` list — deliberately with no
//! second grammar to keep in sync (see that crate's module doc). That
//! covers every *static* continuation (keywords, operators, placeholders),
//! but it structurally can't help with the live content of a path or a
//! command name: the instant a path or a quoted string gets its first
//! character, the parser's outcome flips from `Incomplete` to `Complete`
//! (that's correct — `£ SEE MY ~/project` **is** a complete, executable
//! command) and `xact_completion::complete` returns nothing more to
//! suggest, by design.
//!
//! This crate fills exactly that gap, and *only* that gap: it figures out
//! *which* operand is still being live-typed and *which* verb it belongs
//! to, then asks [`xact_resolve`] for the real answer. It never resolves
//! anything itself — no directory scanning, no `$PATH` walking, no `~`
//! expansion lives here. `xact_resolve` is the same Padagonia-backed
//! resolution engine `xact-process` and `xact-planner` call to actually
//! *execute* `RUN`/`SEE`/etc — autocomplete asks it the same questions
//! ("what resolver applies to this verb", "what real binaries/paths match
//! this prefix") that execution does, so the two can never disagree about
//! where `OUR` looks or what `~` expands to.

use xact_ast::{Command, ImperativeCommand, Line, Operand};
use xact_parser::{parse_line, ParseOutcome};
use xact_resolve::ResolverKind;

/// Real-world dynamic completions for whatever's being typed right now, or
/// an empty list when nothing applies. Complements
/// [`xact_completion::complete`]: that function already covers every case
/// where the input is still grammatically incomplete; this one only ever
/// has something to say once the input is already a complete command whose
/// trailing operand is a live path or command name (see the module doc).
pub fn dynamic_complete(input: &str) -> Vec<String> {
    if input.is_empty() || input.ends_with(char::is_whitespace) {
        return Vec::new();
    }
    let ParseOutcome::Complete(Line::Command(Command::Imperative(cmd))) = parse_line(input) else {
        return Vec::new();
    };
    let Some(operand) = trailing_operand(&cmd, input.len()) else {
        return Vec::new();
    };

    let resolvers = xact_resolve::resolvers_for(cmd.verb);

    match operand {
        Operand::Owned { kind, path, .. } => {
            if resolvers.contains(&ResolverKind::InstalledBinary) {
                xact_resolve::list_binaries(path, *kind)
            } else if resolvers.contains(&ResolverKind::FilesystemPath) {
                // Every verb with a filesystem_path resolver is, by
                // construction, not RUN (see xact-resolve's ontology), so
                // MY's real default -- a bare relative path anchored at
                // ~/, same as xact-planner applies at execution time --
                // always applies here.
                let anchored = xact_resolve::anchor_my_default(path, *kind, true);
                xact_resolve::list_path_entries(&anchored)
            } else {
                Vec::new()
            }
        }
        Operand::StringArg { value, .. } if resolvers.contains(&ResolverKind::InstalledBinary) => {
            // Only the command name (the string's first word) names a real
            // binary; once a space appears we're into argv, which this
            // build makes no attempt to complete. A bare StringArg carries
            // no ownership prefix, so it defaults to MY (spec section 10 —
            // the same default `xact-planner::ownership_domain` applies at
            // execution time).
            if value.chars().any(char::is_whitespace) {
                Vec::new()
            } else {
                xact_resolve::list_binaries(value, xact_ast::OwnershipKind::My)
            }
        }
        _ => Vec::new(),
    }
}

/// Whichever of `cmd`'s operands (destination takes priority, since it's
/// written later — `to <ownership> <path>`) ends exactly at `input`'s end:
/// the thing still being typed right now.
fn trailing_operand(cmd: &ImperativeCommand, input_len: usize) -> Option<&Operand> {
    if let Some(dest) = &cmd.destination {
        if dest.span().end == input_len {
            return Some(dest);
        }
    }
    if let Some(op) = &cmd.operand {
        if op.span().end == input_len {
            return Some(op);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn no_dynamic_candidates_for_an_incomplete_command() {
        // Still Incomplete (no ownership keyword typed yet) — this is
        // xact_completion's job, not this crate's.
        assert!(dynamic_complete("£ SEE").is_empty());
    }

    #[test]
    fn no_dynamic_candidates_once_the_input_has_moved_on() {
        // Trailing whitespace means nothing is "currently being typed".
        assert!(dynamic_complete("£ SEE MY ~/ ").is_empty());
    }

    #[test]
    fn resolves_real_filesystem_entries_after_an_ownership_keyword() {
        let dir = std::env::temp_dir().join(format!("xact-completion-graph-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("project_alpha")).unwrap();
        fs::write(dir.join("project_beta.txt"), b"x").unwrap();
        fs::write(dir.join("other.txt"), b"x").unwrap();

        let input = format!("£ SEE MY {}/project", dir.display());
        let mut suggestions = dynamic_complete(&input);
        suggestions.sort();

        let expected_dir = format!("{}/project_alpha/", dir.display());
        let expected_file = format!("{}/project_beta.txt", dir.display());
        assert!(suggestions.contains(&expected_dir), "{suggestions:?}");
        assert!(suggestions.contains(&expected_file), "{suggestions:?}");
        assert!(!suggestions.iter().any(|s| s.ends_with("other.txt")));

        fs::remove_dir_all(&dir).ok();
    }

    /// `$HOME` is process-global mutable state, and cargo runs tests in
    /// parallel by default — any test that overrides it takes this lock
    /// first and restores the real value before returning (same pattern
    /// as `xact-planner`'s `HOME_SENSITIVE`).
    static HOME_SENSITIVE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The behavior requested directly, on the completion side: `MY` with
    /// a bare relative path (no `~/`, no leading `/`) defaults to `~/`,
    /// the same real default `xact-planner` applies at execution time —
    /// `£ SEE MY project` should dynamic-complete against `~/project*`,
    /// not whatever directory the REPL happens to be running from.
    #[test]
    fn my_with_a_bare_relative_path_defaults_to_home_completion() {
        let _guard = HOME_SENSITIVE.lock().unwrap_or_else(|e| e.into_inner());
        let home = std::env::temp_dir().join(format!("xact-completion-graph-home-test-{}", std::process::id()));
        fs::create_dir_all(home.join("myproject")).unwrap();
        let original_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);

        let suggestions = dynamic_complete("£ SEE MY myproj");

        match original_home {
            Some(original) => std::env::set_var("HOME", original),
            None => std::env::remove_var("HOME"),
        }
        fs::remove_dir_all(&home).ok();

        // The candidate text is built from the anchored ~/ prefix, not
        // the fully-expanded absolute directory -- the same
        // human-readable, portable form a literally-typed `~/` would
        // produce, and honest about what MY actually resolved against.
        assert_eq!(suggestions, vec!["~/myproject/".to_string()], "{suggestions:?}");
    }

    #[test]
    fn resolves_real_installed_binaries_after_run_with_a_quoted_string() {
        // "ls" is real and present on any system this test runs on.
        let suggestions = dynamic_complete("£ RUN 'ls");
        assert!(suggestions.iter().any(|s| s == "ls"), "{suggestions:?}");
    }

    #[test]
    fn resolves_real_installed_binaries_after_run_with_a_bare_ownership_word() {
        // `£ RUN MY chrome` (no quotes) is the documented primary form —
        // an Owned operand, not a StringArg — and must dynamic-complete
        // exactly the same way.
        let suggestions = dynamic_complete("£ RUN MY ls");
        assert!(suggestions.iter().any(|s| s == "ls"), "{suggestions:?}");
    }

    #[test]
    fn does_not_complete_binaries_once_argv_starts() {
        assert!(dynamic_complete("£ RUN 'ls -").is_empty());
    }

    #[test]
    fn does_not_resolve_binaries_for_a_path_taking_verb() {
        // SEE has only a filesystem_path resolver (confirmed structurally
        // by xact-resolve's own resolvers_for_see_is_filesystem_path_only
        // test) — a small, controlled fixture directory proves it
        // resolves real *paths*, not bare binary names, without risking a
        // real, potentially huge system directory (like /bin -> /usr/bin,
        // whose ~2600 real entries can legitimately exceed xact-resolve's
        // real gls timeout).
        let dir = std::env::temp_dir().join(format!("xact-completion-graph-see-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("listing.txt"), b"x").unwrap();

        let input = format!("£ SEE MY {}/lis", dir.display());
        let suggestions = dynamic_complete(&input);
        assert_eq!(suggestions, vec![format!("{}/listing.txt", dir.display())]);

        fs::remove_dir_all(&dir).ok();
    }
}
