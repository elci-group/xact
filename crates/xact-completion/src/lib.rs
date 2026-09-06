//! Deterministic completion (spec sections 14, 15).
//!
//! There is no independent "autocomplete grammar": completion reads the
//! same `expected` continuation list that the parser attaches to a
//! diagnostic when a draft is incomplete or invalid. This directly
//! satisfies the completion invariant (spec section 27): every suggested
//! continuation is, by construction, a continuation the parser itself
//! considers valid.
//!
//! Phase 1 limitation: completion operates on whole-token prefixes (e.g.
//! after a trailing space), not on a partially-typed word. Operator
//! metadata (`repeatable` / `singleton` / `mutually-exclusive`, spec
//! section 16) does not apply yet — Phase 1's grammar has no repeatable
//! operators to filter.

use xact_parser::{parse_line, ParseOutcome};

/// The valid next tokens for the given (possibly incomplete) input.
/// Returns an empty list once the input is already a complete, executable
/// command — there is nothing left to suggest.
pub fn complete(input: &str) -> Vec<String> {
    match parse_line(input) {
        ParseOutcome::Complete(_) => Vec::new(),
        ParseOutcome::Incomplete(diag) | ParseOutcome::Invalid(diag) => diag.expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_verbs_and_they_at_start() {
        let suggestions = complete("£ ");
        assert!(suggestions.contains(&"SEE".to_string()));
        assert!(suggestions.contains(&"BANK".to_string()));
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
}
