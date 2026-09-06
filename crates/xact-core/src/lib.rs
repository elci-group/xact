//! Session state that ties the deterministic pipeline together:
//! lex -> parse -> semantic/policy validation -> reference/identity/policy update.
//!
//! `xact-core` deliberately stops at *validation*, not execution.
//! `SessionOutcome::Accepted` means "grammatically and semantically valid,
//! ready to plan" — turning that into an `ExecutionPlan` and running it is
//! `xact-planner`/`xact-executor`'s job (spec sections 20-21), which a
//! caller invokes separately using [`Session::references`] (see
//! `xact-cli` for the reference wiring). Only `CREATE` has a real plan so
//! far; every other verb reports itself unsupported rather than executing.

use xact_agent::ValidatedAgentBlock;
use xact_ast::{AgentBlock, Command, Line, PolicyOperator, PolicyStatement};
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
    /// A grammatically and semantically valid agent block — a semantic
    /// intent, not a dispatch to any actual agent provider (spec section 9;
    /// `xact-agent`/`xact-planner` do not run agents yet).
    AgentAccepted(AgentBlock),
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
            ParseOutcome::Complete(Line::Agent(block)) => {
                match xact_agent::validate(block, &self.identity, &self.references) {
                    Ok(ValidatedAgentBlock { block }) => SessionOutcome::AgentAccepted(block),
                    Err(diagnostics) => SessionOutcome::Rejected(diagnostics),
                }
            }
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

    /// This session's resolved-reference state, for a caller that wants to
    /// plan and execute an [`SessionOutcome::Accepted`] command (spec
    /// sections 20-21) — `xact-core` itself never plans or executes.
    pub fn references(&self) -> &ReferenceContext {
        &self.references
    }

    /// This session's established scheduling policy (spec section 23),
    /// for a caller deciding whether to run an accepted command's plan
    /// synchronously (`CONSECUTIVELY`, or no policy stated — today's
    /// default sequential behavior already satisfies that) or as an
    /// independent concurrent branch (`CONCURRENTLY` — see `xact-cli`,
    /// which is the only crate that actually branches on this).
    pub fn schedule(&self) -> Option<PolicyOperator> {
        self.policy.schedule()
    }

    /// This session's established resource budget (spec section 22,
    /// Xact–Mesut Integration Phase 5), resolved from accumulated
    /// `SPEND`/`SAVE` statements — a caller passes this to
    /// `xact-executor::execute`/`execute_concurrent` so a `£ RUN` plan is
    /// actually constrained, not just described.
    pub fn resource_budget(&self) -> xact_ast::ResourceBudget {
        self.policy.resource_budget()
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

    #[test]
    fn agent_block_accepted() {
        let mut session = Session::new();
        assert!(matches!(
            session.submit("@ TELL 'GPT-5.6-luna' BE \"a meticulous senior Rust engineer\" READING MY ~/project/ POPULATING MY ~/project/review/ THINK 80 \"Review this project.\""),
            SessionOutcome::AgentAccepted(_)
        ));
    }

    #[test]
    fn agent_reading_their_rejected_without_identity() {
        let mut session = Session::new();
        assert!(matches!(
            session.submit("@ TELL 'x' READING THEIR ~/shared \"go\""),
            SessionOutcome::Rejected(_)
        ));
    }

    #[test]
    fn agent_populating_that_resolves_from_prior_command() {
        let mut session = Session::new();
        assert!(matches!(
            session.submit("£ SEE MY ~/project/review/"),
            SessionOutcome::Accepted(_)
        ));
        assert!(matches!(
            session.submit("@ TELL 'x' POPULATING THAT \"go\""),
            SessionOutcome::AgentAccepted(_)
        ));
    }
}
