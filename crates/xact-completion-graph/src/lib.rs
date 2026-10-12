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
//!
//! `@ TELL <target>` is a second, structurally different gap this crate
//! also fills: unlike a `£`-command's trailing operand, `TELL`'s target
//! never makes the parser report `Complete` while it's being typed —
//! `TELL` always expects a clause or the closing instruction *after* the
//! target (`xact-parser`'s `parse_agent_clauses`), so the outcome stays
//! `Incomplete` the whole time. [`agent_tell_target`] detects that case
//! directly off the token stream instead (the parser's own tokens, not a
//! second lexer), and [`xact_tell::known_targets`] stands in for
//! `xact_resolve` here — same relationship, different real-world
//! question ("what models/harnesses can `TELL` actually reach" instead
//! of "what binaries/paths exist").
//!
//! [`agent_model_selection`] is the forced-picker variant of that: the
//! instant `@ TELL` is entered (its target slot reached, cursor at the
//! end of the line), it returns the full deterministic model list
//! ([`xact_tell::selectable_models`] — installed Ollama plus available
//! provider models), narrowing as a partial target is typed. `xact-cli`
//! uses it as the dedicated model menu's candidate source.

use xact_ast::{AgentVerb, Command, ImperativeCommand, Line, Operand};
use xact_lexer::{tokenize, TokenKind};
use xact_parser::{parse_line, ParseOutcome};
use xact_resolve::ResolverKind;

/// Real-world dynamic completions for whatever's being typed right now, or
/// an empty list when nothing applies. Complements
/// [`xact_completion::complete`]: that function already covers every case
/// where the input is still grammatically incomplete; this one only ever
/// has something to say once the input is already a complete command whose
/// trailing operand is a live path or command name, or once `@ TELL`'s
/// target is being typed (see the module doc).
pub fn dynamic_complete(input: &str) -> Vec<String> {
    dynamic_complete_with(input, &xact_resolve::OwnerRoots::default())
}

/// [`dynamic_complete`] with the session's ownership roots, so a path typed
/// after `OUR`/`THEIR` lists entries under that owner's real root — the
/// same roots `xact-semantic` and `xact-planner` resolve it against.
pub fn dynamic_complete_with(input: &str, roots: &xact_resolve::OwnerRoots) -> Vec<String> {
    if input.is_empty() || input.ends_with(char::is_whitespace) {
        return Vec::new();
    }

    if let Some(partial) = agent_tell_target(input) {
        return rank_targets(xact_tell::known_targets(), &partial);
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
                // the owner's root applies here exactly as it does at
                // execution time.
                xact_resolve::list_owned_path_entries(path, *kind, roots)
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

/// If `input` is exactly `@ TELL <target>` or `@ TEAM <roster>` — the `@`
/// sigil, an agent verb, then a single still-being-typed word or (possibly
/// still-open) quoted string, nothing after it yet — returns the target text
/// typed so far. `TEAM` uses the same installed/provider target catalog for
/// every roster member: after a comma and optional space, this returns only
/// the member fragment being typed. The CLI's ordinary trailing-word span
/// then replaces that fragment while preserving the earlier roster entries.
/// Detected off the raw token stream rather than a parsed `AgentBlock`
/// because `parse_line` never reports `Complete` at this position —
/// `TELL` always expects a clause or the closing instruction after its
/// target — so there's no `Complete` outcome to pattern-match the way
/// [`dynamic_complete`]'s other branch does.
fn agent_tell_target(input: &str) -> Option<String> {
    let tokens = tokenize(input);
    let [at, verb, target, eof] = tokens.as_slice() else {
        return None;
    };
    if !matches!(at.kind, TokenKind::At) {
        return None;
    }
    let TokenKind::Word(verb_word) = &verb.kind else {
        return None;
    };
    let agent_verb = AgentVerb::parse(&verb_word.to_uppercase());
    if !matches!(agent_verb, Some(AgentVerb::Tell | AgentVerb::Team)) {
        return None;
    }
    // `Word`'s stored text is exactly the typed word, but a `StringLit`
    // token's `span` covers the surrounding quote characters too (see
    // `xact-lexer`) while its stored text is already unwrapped — take
    // the text from `kind`, not a slice of `input` by `span`, so both
    // cases return the same thing: what's actually been typed as the
    // target, quotes aside.
    let target_text = match &target.kind {
        TokenKind::Word(w) => w.clone(),
        TokenKind::StringLit(s) => s.clone(),
        _ => return None,
    };
    if target.span.end != input.len() || !matches!(eof.kind, TokenKind::Eof) {
        return None;
    }
    match agent_verb {
        Some(AgentVerb::Team) => target_text.rsplit(',').next().map(|member| member.trim().to_owned()),
        Some(AgentVerb::Tell) => Some(target_text),
        None => None,
    }
}

/// The forced model-selection list for the exact moment `@ TELL`'s target
/// slot is entered: [`xact_tell::selectable_models`] (every installed
/// Ollama model plus every available provider model, deterministically
/// ordered — see its own doc) ranked against whatever partial target is
/// already on the line, which is nothing right after the space following
/// `TELL`, so the full list is what the picker forces. Non-empty at
/// exactly one grammar position ([`tell_model_partial`]) and empty
/// everywhere else — which is what lets `xact-cli` fire the model menu
/// on every space keystroke and have it simply vanish wherever the
/// cursor isn't in `TELL`'s target slot.
pub fn agent_model_selection(input: &str) -> Vec<String> {
    match tell_model_partial(input) {
        Some(partial) => rank_targets(xact_tell::selectable_models(), &partial),
        None => Vec::new(),
    }
}

/// The byte range of `@ TELL`'s target token within `input`, if the line
/// has one — the slot the forced model menu fills, and the token
/// `xact-cli`'s highlighter paints blue once it's there (picked from the
/// menu or typed by hand: the target *is* the selected model either
/// way). Covers a bare-word or quoted target wherever it sits, clauses
/// after it included; `TEAM` rosters are deliberately not model targets.
pub fn tell_target_span(input: &str) -> Option<std::ops::Range<usize>> {
    let tokens = tokenize(input);
    let [at, verb, target, _rest @ ..] = tokens.as_slice() else {
        return None;
    };
    if !matches!(at.kind, TokenKind::At) {
        return None;
    }
    let TokenKind::Word(word) = &verb.kind else {
        return None;
    };
    if !word.eq_ignore_ascii_case("TELL") {
        return None;
    }
    match &target.kind {
        TokenKind::Word(_) | TokenKind::StringLit(_) => Some(target.span.start..target.span.end),
        _ => None,
    }
}

/// If `input` has reached `@ TELL`'s target slot with nothing after it —
/// `@ TELL ` with the cursor at the end (the moment of TELL entry; empty
/// partial), or a single still-being-typed target word or (possibly
/// still-open) quoted string at the very end of the line — returns the
/// target text typed so far. Deliberately unlike [`agent_tell_target`]
/// in two ways: it never fires for `TEAM` (a comma roster is not one
/// model to force-select) and it never fires before `TELL` is delimited
/// (`@ TELL` with no trailing whitespace is still the verb being typed,
/// not the target slot reached — a distinction the token stream alone
/// can't see, since both tokenize to `[At, Word, Eof]`, so the raw input
/// is what carries it).
fn tell_model_partial(input: &str) -> Option<String> {
    let tokens = tokenize(input);
    let [at, verb, rest @ ..] = tokens.as_slice() else {
        return None;
    };
    if !matches!(at.kind, TokenKind::At) {
        return None;
    }
    let TokenKind::Word(word) = &verb.kind else {
        return None;
    };
    if !word.eq_ignore_ascii_case("TELL") {
        return None;
    }
    match rest {
        // The target slot, entered but empty. The trailing whitespace is
        // checked on the raw input because the token stream is identical
        // without it (whitespace never produces a token).
        [eof] if matches!(eof.kind, TokenKind::Eof) && input.ends_with(char::is_whitespace) => Some(String::new()),
        // A single target token still under the cursor — same end-of-line
        // rule as `agent_tell_target`, and the text comes from the token
        // kind (a `StringLit`'s stored text is already unquoted) rather
        // than a slice of `input` by span.
        [target, eof] if matches!(eof.kind, TokenKind::Eof) && target.span.end == input.len() => match &target.kind {
            TokenKind::Word(w) => Some(w.clone()),
            TokenKind::StringLit(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Narrows and ranks `candidates` against `typed`, per the requested
/// ordering: a candidate whose own spelling starts with `typed` exactly
/// (case-sensitive — "prefers correct spelling + capitalisation") beats
/// one that only matches case-insensitively, which in turn beats one
/// that merely *contains* `typed` somewhere in the middle. Case is never
/// enforced on what the user types — only used to break ties in what's
/// shown first. Anything matching none of the three ways is dropped,
/// same "narrows to nothing" rule the rest of completion follows.
/// Case-insensitive alphabetical order within each tier keeps the list
/// stable and predictable rather than reflecting whatever order the
/// underlying sources happened to return.
fn rank_targets(candidates: Vec<String>, typed: &str) -> Vec<String> {
    let typed_lower = typed.to_lowercase();
    let mut tiered: Vec<(u8, String)> = candidates
        .into_iter()
        .filter_map(|candidate| {
            let candidate_lower = candidate.to_lowercase();
            let tier = if candidate.starts_with(typed) {
                0
            } else if candidate_lower.starts_with(&typed_lower) {
                1
            } else if candidate_lower.contains(&typed_lower) {
                2
            } else {
                return None;
            };
            Some((tier, candidate))
        })
        .collect();
    tiered.sort_by(|(tier_a, a), (tier_b, b)| tier_a.cmp(tier_b).then_with(|| a.to_lowercase().cmp(&b.to_lowercase())));
    tiered.into_iter().map(|(_, candidate)| candidate).collect()
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
    fn agent_tell_target_detects_a_bare_word_target_being_typed() {
        assert_eq!(agent_tell_target("@ TELL gpt").as_deref(), Some("gpt"));
        assert_eq!(
            agent_tell_target("@ tell gpt").as_deref(),
            Some("gpt"),
            "verb match is case-insensitive, like the parser's own"
        );
    }

    #[test]
    fn agent_tell_target_detects_a_still_open_quoted_target_being_typed() {
        // An unterminated quote's span still runs to the end of input
        // (xact-lexer), but the target text itself (unlike the span)
        // never includes the quote character — same as a finished one.
        assert_eq!(agent_tell_target("@ TELL 'gp").as_deref(), Some("gp"));
    }

    #[test]
    fn agent_tell_target_is_none_once_a_clause_or_instruction_follows() {
        // The target is no longer "being typed" — this is a different
        // position (clause/instruction), not this function's job.
        assert_eq!(agent_tell_target("@ TELL gpt-4o BE"), None);
        assert_eq!(agent_tell_target("@ TELL gpt-4o \"hi\""), None);
    }

    #[test]
    fn agent_tell_target_supports_team_and_rejects_non_agent_input() {
        assert_eq!(agent_tell_target("@ TEAM gp"), Some("gp".to_string()));
        assert_eq!(agent_tell_target("@ TEAM 'first, gp"), Some("gp".to_string()));
        assert_eq!(agent_tell_target("£ SEE MY gp"), None);
        assert_eq!(agent_tell_target("@ TELL"), None, "no target typed yet at all");
    }

    /// The behavior requested directly, on the forced-picker side: the
    /// instant `@ TELL ` is entered (target slot reached, nothing typed),
    /// the full deterministic model list is offered — the same list
    /// `xact_tell::selectable_models` computes, in the same order.
    #[test]
    fn model_selection_forces_the_full_list_the_moment_tell_is_entered() {
        assert_eq!(agent_model_selection("@ TELL "), xact_tell::selectable_models());
        assert_eq!(
            agent_model_selection("@ tell  "),
            xact_tell::selectable_models(),
            "verb match is case-insensitive and any trailing whitespace delimits it, like the parser's own"
        );
    }

    #[test]
    fn model_selection_narrows_the_forced_list_while_the_target_is_typed() {
        let selectable = xact_tell::selectable_models();
        assert_eq!(agent_model_selection("@ TELL g"), rank_targets(selectable.clone(), "g"));
        assert_eq!(
            agent_model_selection("@ TELL 'gp"),
            rank_targets(selectable, "gp"),
            "an open quote narrows the same way"
        );
    }

    #[test]
    fn model_selection_fires_nowhere_else() {
        assert!(agent_model_selection("@ TELL").is_empty(), "no delimiter yet — the verb is still being typed");
        assert!(agent_model_selection("@ TEAM ").is_empty(), "TEAM's roster slot is not a forced model selection");
        assert!(agent_model_selection("@ TEAM gp").is_empty());
        assert!(
            agent_model_selection("@ TELL gemma3 ").is_empty(),
            "target established — this is the clause position now"
        );
        assert!(agent_model_selection("@ TELL gemma3 BE").is_empty());
        assert!(agent_model_selection("£ SEE ").is_empty());
        assert!(agent_model_selection("@ ").is_empty());
        assert!(agent_model_selection("").is_empty());
    }

    #[test]
    fn model_selection_is_deterministic_across_calls() {
        assert_eq!(agent_model_selection("@ TELL "), agent_model_selection("@ TELL "));
    }

    #[test]
    fn rank_targets_prefers_exact_case_prefix_over_case_insensitive_prefix_over_substring() {
        let candidates = vec![
            "mid-GPT-thing".to_string(),
            "gpt-4o".to_string(),
            "GPT-4o-mini".to_string(),
            "claude-opus-5".to_string(),
        ];
        let ranked = rank_targets(candidates, "GPT");
        assert_eq!(ranked, vec!["GPT-4o-mini".to_string(), "gpt-4o".to_string(), "mid-GPT-thing".to_string()]);
    }

    #[test]
    fn rank_targets_drops_non_matches_and_is_case_insensitive_on_the_typed_text() {
        let candidates = vec!["gemma3:latest".to_string(), "claude-opus-5".to_string()];
        assert_eq!(rank_targets(candidates.clone(), "GEM"), vec!["gemma3:latest".to_string()]);
        assert!(rank_targets(candidates, "zzz").is_empty());
    }

    #[test]
    #[ignore = "live: requires gls"]
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
    #[ignore = "live: requires gls"]
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
    #[ignore = "live: requires gls"]
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
