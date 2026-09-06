//! Session state that ties the deterministic pipeline together:
//! lex -> parse -> semantic validation -> reference/identity update.
//!
//! `xact-core` deliberately stops at *validation*, not execution. Turning a
//! [`xact_ast::Command`] into an [`ExecutionPlan`](https://) and running it
//! is the job of `xact-planner`/`xact-executor` in a later phase (spec
//! sections 20-21); those crates are still stubs. `SessionOutcome::Accepted`
//! therefore means "grammatically and semantically valid, ready to plan",
//! not "ran".

use xact_ast::Command;
use xact_diagnostics::Diagnostic;
use xact_parser::{parse_line, ParseOutcome};
use xact_reference::ReferenceContext;
use xact_semantic::{validate, IdentityContext};

#[derive(Debug, Clone)]
pub enum SessionOutcome {
    /// Grammatically and semantically valid — ready for the (not yet
    /// implemented) planner/executor.
    Accepted(Command),
    /// A valid prefix that needs more input before it can be evaluated.
    Incomplete(Diagnostic),
    /// Cannot be accepted as typed.
    Rejected(Vec<Diagnostic>),
}

#[derive(Debug, Default)]
pub struct Session {
    identity: IdentityContext,
    references: ReferenceContext,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn submit(&mut self, input: &str) -> SessionOutcome {
        match parse_line(input) {
            ParseOutcome::Complete(command) => match validate(command, &self.identity, &self.references) {
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
            ParseOutcome::Incomplete(diag) => SessionOutcome::Incomplete(diag),
            ParseOutcome::Invalid(diag) => SessionOutcome::Rejected(vec![diag]),
        }
    }

    /// Deterministic valid-next-token suggestions for the given draft
    /// input (spec section 15), independent of `submit`.
    pub fn complete(&self, input: &str) -> Vec<String> {
        xact_completion::complete(input)
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
}
