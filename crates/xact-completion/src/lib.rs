//! Deterministic completion (spec sections 14, 15).
//!
//! There is no independent "autocomplete grammar": completion reads the
//! same `expected` continuation list that the parser attaches to a
//! diagnostic when a draft is incomplete or invalid. This directly
//! satisfies the completion invariant (spec section 27): every suggested
//! continuation is, by construction, a continuation the parser itself
//! considers valid.
//!
//! [`complete`] narrows that list live, per character, against whatever
//! is being typed right now: `£ C` suggests `CREATE`/`COPY`/`CUT` — every
//! verb starting with `C` — not the full verb list, and `RUN`/`SEE`/etc.
//! are excluded. This works without a second, separate "autocomplete
//! grammar" to keep in sync with the real one: the parser's `expected`
//! list already names every keyword valid *at this position*; the only
//! new piece is deciding whether the input's trailing word is a
//! finished token (in which case `expected` already describes what comes
//! *after* it, and nothing should be filtered) or a partial one still
//! being typed (in which case `expected` describes what it's a prefix
//! *of*, and filtering by it is exactly right). That's decided by
//! re-parsing with the trailing word removed and comparing: if the
//! candidate list doesn't change, the word wasn't consumed — it's a
//! partial. If it does change, the parser consumed it as a real token
//! and moved on, so nothing here should be filtered a second time.
//! Operator metadata (`repeatable`/`singleton`/`mutually-exclusive`, spec
//! section 16) still doesn't apply — Phase 1's grammar has no repeatable
//! operators to filter.

use xact_parser::{parse_line, ParseOutcome};

/// The valid next tokens for the given (possibly incomplete) input,
/// narrowed by whatever partial word is currently being typed at the
/// end. Returns an empty list once the input is already a complete,
/// executable command — there is nothing left to suggest.
pub fn complete(input: &str) -> Vec<String> {
    let Some(suggestions) = expected_after(input) else {
        return Vec::new();
    };

    let Some(partial) = trailing_partial_word(input) else {
        return suggestions;
    };

    // Re-parse with the partial word removed. An unchanged candidate list
    // means the parser never consumed it — it's genuinely partial, so
    // narrow by it. A changed (or now-complete) list means the parser
    // already consumed it as a real token; `suggestions` already
    // describes what comes next, not a filter target.
    let stripped = &input[..input.len() - partial.len()];
    if expected_after(stripped).as_deref() == Some(suggestions.as_slice()) {
        filter_by_prefix(suggestions, partial)
    } else {
        suggestions
    }
}

/// The parser's own next-token candidates for `input`, or `None` if
/// `input` already parses as a complete command.
fn expected_after(input: &str) -> Option<Vec<String>> {
    match parse_line(input) {
        ParseOutcome::Complete(_) => None,
        ParseOutcome::Incomplete(diag) | ParseOutcome::Invalid(diag) => Some(diag.expected),
    }
}

/// The word at the very end of `input`, if `input` doesn't end in
/// whitespace (and isn't empty) — the thing currently being typed.
fn trailing_partial_word(input: &str) -> Option<&str> {
    if input.is_empty() || input.ends_with(char::is_whitespace) {
        return None;
    }
    input.rsplit(char::is_whitespace).next().filter(|w| !w.is_empty())
}

/// Narrows `suggestions` to those starting with `prefix`, case-
/// insensitively. Placeholders (`<path>`, `'...'`, `<percent>%<resource>`)
/// are never filtered out this way — matching real user input against a
/// placeholder's literal text would be meaningless (typing the start of
/// a quoted string, for instance, should never make `'...'` disappear
/// from the list just because it doesn't start with the same letters).
fn filter_by_prefix(suggestions: Vec<String>, prefix: &str) -> Vec<String> {
    suggestions
        .into_iter()
        .filter(|s| is_placeholder(s) || s.to_uppercase().starts_with(&prefix.to_uppercase()))
        .collect()
}

/// Whether `s` is a placeholder (stands for free-form user input) rather
/// than a literal keyword or symbol that can be prefix-matched. Every
/// placeholder this grammar produces is written `<...>` or `'...'`;
/// everything else — keywords like `SEE`/`MY`/`WHEN`, and the bare
/// top-level symbols `!`/`£`/`@` — is a real candidate to filter.
fn is_placeholder(s: &str) -> bool {
    s.starts_with('<') || s.starts_with('\'')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_verbs_and_they_at_start() {
        let suggestions = complete("£ ");
        assert!(suggestions.contains(&"SEE".to_string()));
        assert!(suggestions.contains(&"CREATE".to_string()));
        assert!(suggestions.contains(&"THEY".to_string()));
    }

    #[test]
    fn suggests_ownership_and_reference_after_verb() {
        let suggestions = complete("£ COPY");
        assert!(suggestions.contains(&"MY".to_string()));
        assert!(suggestions.contains(&"THIS".to_string()));
    }

    #[test]
    fn suggests_path_after_ownership_keyword() {
        assert_eq!(complete("£ COPY MY"), vec!["<path>".to_string()]);
    }

    #[test]
    fn no_suggestions_for_a_complete_command() {
        assert!(complete("£ SEE MY ~/Documents").is_empty());
    }

    #[test]
    fn suggests_policy_operators_after_bang() {
        let suggestions = complete("!");
        assert!(suggestions.contains(&"WITH".to_string()));
        assert!(suggestions.contains(&"SAVE".to_string()));
        assert!(suggestions.contains(&"CONCURRENTLY".to_string()));
    }

    #[test]
    fn suggests_quota_placeholder_after_save() {
        assert_eq!(complete("! SAVE"), vec!["<percent>%<resource>".to_string()]);
    }

    #[test]
    fn suggests_agent_verbs_after_at() {
        let suggestions = complete("@");
        assert_eq!(suggestions, vec!["TELL".to_string(), "TEAM".to_string()]);
    }

    #[test]
    fn suggests_remaining_clauses_and_instruction_placeholder() {
        let suggestions = complete("@ TELL 'x' BE \"engineer\"");
        assert!(!suggestions.contains(&"BE".to_string()));
        assert!(suggestions.contains(&"READING".to_string()));
        assert!(suggestions.contains(&"'...'".to_string()));
    }

    #[test]
    fn does_not_suggest_operators_after_an_operator() {
        // Spec section 15: SEE/EDIT/MOVE/COPY must not appear as suggestions
        // once a verb slot has already been filled by another verb.
        let suggestions = complete("£ COPY");
        assert!(!suggestions.contains(&"SEE".to_string()));
        assert!(!suggestions.contains(&"EDIT".to_string()));
    }

    #[test]
    fn narrows_verbs_by_partial_prefix() {
        // The user's own example: "£ C" keeps every verb starting with
        // C (CREATE, COPY, CUT) and drops everything else, including RUN.
        let suggestions = complete("£ C");
        assert!(suggestions.contains(&"CREATE".to_string()));
        assert!(suggestions.contains(&"COPY".to_string()));
        assert!(suggestions.contains(&"CUT".to_string()));
        assert!(!suggestions.contains(&"RUN".to_string()));
        assert!(!suggestions.contains(&"SEE".to_string()));
        assert!(!suggestions.contains(&"EDIT".to_string()));
        assert!(!suggestions.contains(&"MOVE".to_string()));
        assert!(!suggestions.contains(&"PASTE".to_string()));
        assert!(!suggestions.contains(&"DELETE".to_string()));
        assert!(!suggestions.contains(&"BOUND".to_string()));
        assert!(!suggestions.contains(&"THEY".to_string()));
    }

    #[test]
    fn narrows_further_as_more_characters_are_typed() {
        assert_eq!(complete("£ CR"), vec!["CREATE".to_string()]);
    }

    #[test]
    fn narrowing_is_case_insensitive() {
        assert_eq!(complete("£ c"), complete("£ C"));
    }

    #[test]
    fn a_fully_typed_verb_with_no_trailing_space_is_not_re_filtered() {
        // "£ RUN" (no trailing space yet) already parses RUN as a
        // complete, consumed token — the real next-step suggestions
        // (MY/OUR/THIS/THAT/'...') must be returned as-is, not filtered
        // by "RUN" as if it were still a partial word (which would wipe
        // out every suggestion, since none of them start with "RUN").
        let suggestions = complete("£ RUN");
        assert!(suggestions.contains(&"MY".to_string()));
        assert!(suggestions.contains(&"'...'".to_string()));
    }

    #[test]
    fn placeholders_are_never_filtered_out_by_a_partial_prefix() {
        // At this position the candidates are THIS/THAT/'...' (spec
        // section 11's reference operand parsing) — typing "T" narrows
        // THIS/THAT but must not hide '...' too: a real string argument
        // isn't ruled out just because the user is currently leaning
        // toward THIS/THAT.
        let suggestions = complete("@ TELL 'x' READING T");
        assert!(suggestions.contains(&"THIS".to_string()));
        assert!(suggestions.contains(&"THAT".to_string()));
        assert!(suggestions.contains(&"'...'".to_string()));
    }

    #[test]
    fn narrows_policy_operators_by_partial_prefix() {
        let suggestions = complete("! S");
        assert!(suggestions.contains(&"SPEND".to_string()));
        assert!(suggestions.contains(&"SAVE".to_string()));
        assert!(!suggestions.contains(&"WITH".to_string()));
        assert!(!suggestions.contains(&"CONCURRENTLY".to_string()));
    }

    #[test]
    fn narrows_agent_verbs_by_partial_prefix() {
        // "TEAM" itself starts with "TE" too, so both survive a "TE"
        // prefix — "TEL" is what actually disambiguates down to TELL.
        assert_eq!(complete("@ TE"), vec!["TELL".to_string(), "TEAM".to_string()]);
        assert_eq!(complete("@ TEL"), vec!["TELL".to_string()]);
    }

    #[test]
    fn unmatched_prefix_narrows_to_nothing() {
        assert!(complete("£ Z").is_empty());
    }
}
