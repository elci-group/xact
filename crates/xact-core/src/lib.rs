//! Session state that ties the deterministic pipeline together:
//! lex -> parse -> semantic/policy validation -> reference/identity/policy update.
//!
//! `xact-core` deliberately stops at *validation*, not execution. Turning a
//! [`xact_ast::Command`] into an [`ExecutionPlan`](https://) and running it
//! is the job of `xact-planner`/`xact-executor` in a later phase (spec
//! sections 20-21); those crates are still stubs. `SessionOutcome::Accepted`
//! therefore means "grammatically and semantically valid, ready to plan",
//! not "ran".

use xact_ast::{Command, Line, PolicyStatement};
use xact_diagnostics::Diagnostic;
use xact_parser::{parse_line, ParseOutcome};
use xact_policy::PolicyContext;
use xact_reference::ReferenceContext;
use xact_semantic::{validate, IdentityContext};

#[derive(Debug, Clone)]
pub enum SessionOutcome {
    /// A grammatically and semantically valid command — ready for the (not
    /// yet implemented) planner/executor.
    Accepted(Command),
    /// A grammatically valid, internally consistent policy statement — now
    /// part of this session's active policy context. Not yet enforced by
    /// any executor.
    PolicyAccepted(PolicyStatement),
    /// A valid prefix that needs more input before it can be evaluated.
    Incomplete(Diagnostic),
    /// Cannot be accepted as typed.
    Rejected(Vec<Diagnostic>),
}

#[derive(Debug, Default)]
pub struct Session {
    identity: IdentityContext,
    references: ReferenceContext,
    policy: PolicyContext,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn submit(&mut self, input: &str) -> SessionOutcome {
        match parse_line(input) {
            ParseOutcome::Complete(Line::Command(command)) => match validate(command, &self.identity, &self.references) {
                Ok(validated) => {
                    if let Command::Identity(decl) = &validated.command {
                        self.identity.set(decl.members.clone());
                    }
                    if let Some(object) = validated.establishes.clone() {
                        self.references.record(object);
                    }
                    SessionOutcome::Accepted(validated.command)
                }
                Err(diagnostics) => SessionOutcome::Rejected(diagnostics),
            },
            ParseOutcome::Complete(Line::Policy(stmt)) => match xact_policy::apply(stmt, &mut self.policy) {
                Ok(applied) => SessionOutcome::PolicyAccepted(applied),
                Err(diagnostic) => SessionOutcome::Rejected(vec![diagnostic]),
            },
            ParseOutcome::Incomplete(diag) => SessionOutcome::Incomplete(diag),
            ParseOutcome::Invalid(diag) => SessionOutcome::Rejected(vec![diag]),
        }
    }

    /// Deterministic valid-next-token suggestions for the given draft
    /// input (spec section 15), filtered for policy operators already
    /// singleton-established in this session (spec section 16).
    pub fn complete(&self, input: &str) -> Vec<String> {
        self.policy.filter_suggestions(xact_completion::complete(input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn their_rejected_then_accepted_after_identity() {
        let mut session = Session::new();
        assert!(matches!(
            session.submit("£ SEE THEIR ~/shared"),
            SessionOutcome::Rejected(_)
        ));
        assert!(matches!(
            session.submit("£ THEY are \"alice,bob\""),
            SessionOutcome::Accepted(_)
        ));
        assert!(matches!(
            session.submit("£ SEE THEIR ~/shared"),
            SessionOutcome::Accepted(_)
        ));
    }

    #[test]
    fn that_resolves_to_previously_established_object() {
        let mut session = Session::new();
        assert!(matches!(
            session.submit("£ SEE MY ~/project/README.md"),
            SessionOutcome::Accepted(_)
        ));
        assert!(matches!(session.submit("£ EDIT THAT"), SessionOutcome::Accepted(_)));
    }

    #[test]
    fn that_rejected_with_no_prior_result() {
        let mut session = Session::new();
        assert!(matches!(session.submit("£ EDIT THAT"), SessionOutcome::Rejected(_)));
    }

    #[test]
    fn incomplete_draft_is_not_accepted() {
        let mut session = Session::new();
        assert!(matches!(session.submit("£ COPY MY"), SessionOutcome::Incomplete(_)));
    }

    #[test]
    fn policy_statement_accepted_and_tracked_across_session() {
        let mut session = Session::new();
        assert!(matches!(
            session.submit("! SAVE 20%RAM 30%CPU"),
            SessionOutcome::PolicyAccepted(_)
        ));
        assert!(matches!(
            session.submit("£ RUN 'chrome'"),
            SessionOutcome::Accepted(_)
        ));
    }

    #[test]
    fn conflicting_policy_statements_rejected() {
        let mut session = Session::new();
        assert!(matches!(
            session.submit("! CONCURRENTLY"),
            SessionOutcome::PolicyAccepted(_)
        ));
        assert!(matches!(
            session.submit("! CONSECUTIVELY"),
            SessionOutcome::Rejected(_)
        ));
    }

    #[test]
    fn completion_drops_schedule_operators_once_established() {
        let mut session = Session::new();
        session.submit("! CONCURRENTLY");
        let suggestions = session.complete("!");
        assert!(!suggestions.contains(&"CONCURRENTLY".to_string()));
        assert!(!suggestions.contains(&"CONSECUTIVELY".to_string()));
        assert!(suggestions.contains(&"WITH".to_string()));
    }
}
