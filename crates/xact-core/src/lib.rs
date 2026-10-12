//! Session state that ties the deterministic pipeline together:
//! lex -> parse -> semantic/capability/policy validation ->
//! reference/identity/policy update.
//!
//! `xact-core` deliberately stops at *validation*, not execution.
//! `SessionOutcome::Accepted` means "grammatically, semantically, and
//! policy-valid, ready to plan" — turning that into an `ExecutionPlan` and
//! running it is `xact-planner`/`xact-executor`'s job (spec sections
//! 20-21), which a caller (`xact-cli`) invokes separately using
//! [`Session::references`] and [`Session::owner_roots`].
//!
//! Capability policy is part of validation (spec section 19): a command
//! whose operation needs a capability forbidden by `! WITHOUT` is
//! rejected here, before it is accepted, so it can never run partway
//! through a block. `! WITH` is accepted only when the capability is
//! actually available on this host (see [`probe_capability`]).
//!
//! [`Session`] is `Clone` so a caller can validate a whole block against a
//! throwaway copy before executing any of it (spec section 18; see
//! `xact-cli`'s batch mode).

use xact_agent::ValidatedAgentBlock;
use xact_ast::{AgentBlock, Capability, Command, Line, PolicyOperator, PolicyStatement};
use xact_diagnostics::Diagnostic;
use xact_parser::{parse_line, ParseOutcome};
use xact_policy::PolicyContext;
use xact_reference::ReferenceContext;
use xact_resolve::OwnerRoots;
use xact_semantic::{validate, IdentityContext};

#[derive(Debug, Clone)]
pub enum SessionOutcome {
    /// A grammatically, semantically, and policy-valid command, ready for
    /// `xact-planner`/`xact-executor`. The [`ReferenceContext`] is the one
    /// the command was validated in — what *its* `THIS`/`THAT` mean — and is
    /// what it must be planned with: by the time `submit` returns, the
    /// session's own context already reflects this command's result, so
    /// `£ COPY THAT to MY ~/b` planned against [`Session::references`]
    /// would read `THAT` as `~/b` itself.
    Accepted(Command, ReferenceContext),
    /// A grammatically valid, internally consistent policy statement — now
    /// part of this session's active policy context.
    PolicyAccepted(PolicyStatement),
    /// A valid agent block — a semantic intent (spec section 9), ready for
    /// `xact-planner::plan_agent`/`xact-executor`.
    AgentAccepted(AgentBlock),
    /// A valid prefix that needs more input before it can be evaluated.
    Incomplete(Diagnostic),
    /// Cannot be accepted as typed.
    Rejected(Vec<Diagnostic>),
}

#[derive(Debug, Default, Clone)]
pub struct Session {
    identity: IdentityContext,
    references: ReferenceContext,
    policy: PolicyContext,
    last_outcome: Option<bool>,
}

/// `WITH`'s availability check: whether `capability` can actually be
/// provided on this host right now. Each answer comes from the crate that
/// owns the capability.
pub fn probe_capability(capability: Capability) -> Result<(), String> {
    let available = |ok: bool, reason: &str| if ok { Ok(()) } else { Err(reason.to_string()) };
    match capability {
        Capability::Network => available(xact_resource::has_default_route(), "this host has no default network route"),
        Capability::ProcessSpawn | Capability::FilesystemWrite => Ok(()),
        Capability::Clipboard => available(
            std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some(),
            "there is no desktop session (neither WAYLAND_DISPLAY nor DISPLAY is set)",
        ),
        Capability::AgentLocal => available(xact_tell::local_provider_available(), "`ollama` is not installed"),
        Capability::AgentRemote => {
            available(xact_tell::remote_provider_configured(), "no provider API key is set in the environment")
        }
        Capability::AgentTools => {
            available(xact_tell::harness_available(), "no supported CLI agent harness is installed")
        }
    }
}

/// Starts background refreshes of the agent-model catalog completion reads
/// (see `xact_tell::prewarm`); returns immediately.
pub fn prewarm_completion() {
    xact_tell::prewarm();
}

impl Session {
    /// A fresh session whose `OUR` root comes from `XACT_OUR_ROOT`.
    pub fn new() -> Self {
        Self { identity: IdentityContext::from_env(), ..Self::default() }
    }

    /// The roots owned paths resolve against (`OUR`'s shared root and the
    /// declared `THEY` users' homes), for planning and completion.
    pub fn owner_roots(&self) -> &OwnerRoots {
        self.identity.roots()
    }

    /// Spec section 19's capability level for an accepted operation.
    fn authorize(&self, operation: &str, required: &[Capability]) -> Result<(), Vec<Diagnostic>> {
        self.policy
            .authorize(operation, required)
            .map_err(|reason| vec![Diagnostic::invalid(reason, xact_ast::Span::default(), vec![])])
    }

    pub fn submit(&mut self, input: &str) -> SessionOutcome {
        match parse_line(input) {
            ParseOutcome::Complete(Line::Command(command)) => {
                let validated = match validate(command, &self.identity, &self.references) {
                    Ok(validated) => validated,
                    Err(diagnostics) => return SessionOutcome::Rejected(diagnostics),
                };
                if let Command::Imperative(cmd) = &validated.command {
                    let required = xact_planner::required_capabilities(&validated.command);
                    if let Err(diagnostics) = self.authorize(cmd.verb.as_str(), &required) {
                        return SessionOutcome::Rejected(diagnostics);
                    }
                }
                if let Some(members) = validated.identity.clone() {
                    self.identity.set(members);
                }
                let references_before = self.references.clone();
                if let Some(established) = validated.establishes.clone() {
                    self.references.record(established.input, established.result);
                }
                SessionOutcome::Accepted(validated.command, references_before)
            }
            ParseOutcome::Complete(Line::Policy(stmt)) => {
                match xact_policy::apply(stmt, &mut self.policy, &probe_capability) {
                    Ok(applied) => {
                        let denied = self.policy.denied();
                        let remote_allowed =
                            !denied.contains(&Capability::Network) && !denied.contains(&Capability::AgentRemote);
                        xact_tell::set_remote_lookups_allowed(remote_allowed);
                        SessionOutcome::PolicyAccepted(applied)
                    }
                    Err(diagnostic) => SessionOutcome::Rejected(vec![diagnostic]),
                }
            }
            ParseOutcome::Complete(Line::Agent(block)) => {
                match xact_agent::validate(block, &self.identity, &self.references) {
                    Ok(ValidatedAgentBlock { block }) => {
                        let required = xact_planner::required_agent_capabilities(&block);
                        if let Err(diagnostics) = self.authorize(block.verb.as_str(), &required) {
                            return SessionOutcome::Rejected(diagnostics);
                        }
                        SessionOutcome::AgentAccepted(block)
                    }
                    Err(diagnostics) => SessionOutcome::Rejected(diagnostics),
                }
            }
            ParseOutcome::Incomplete(diag) => SessionOutcome::Incomplete(diag),
            ParseOutcome::Invalid(diag) => SessionOutcome::Rejected(vec![diag]),
        }
    }

    /// Deterministic valid-next-token suggestions for the given draft
    /// input (spec section 15), filtered for policy operators already
    /// singleton-established in this session (spec section 16). Tries
    /// real-world dynamic candidates first — installed binaries after
    /// `RUN`, real filesystem entries after an ownership keyword, real
    /// model/harness names while `@ TELL`'s target is being typed — and
    /// falls back to the static grammar list (keywords, operators,
    /// placeholders) only once dynamic has nothing to say. Safe to order
    /// this way for every case that predates `@ TELL`'s target
    /// completion: `xact_completion_graph::dynamic_complete` only ever
    /// returns non-empty once `xact_completion::complete` would already
    /// return empty (see that crate's module doc — the two are
    /// complementary, not overlapping) — except for `@ TELL`'s target
    /// itself, where the static list is *wrong* the instant any word
    /// follows `TELL` (it already treats the target as consumed and
    /// jumps to suggesting clause keywords), which is exactly why this
    /// order had to change: static no longer gets a chance to shadow the
    /// real target list there.
    pub fn complete(&self, input: &str) -> Vec<String> {
        let dynamic_suggestions = xact_completion_graph::dynamic_complete_with(input, self.owner_roots());
        if !dynamic_suggestions.is_empty() {
            return dynamic_suggestions;
        }
        self.policy.filter_suggestions(xact_completion::complete(input))
    }

    /// This session's current reference state, including the most recently
    /// accepted command's result. To plan that command itself, use the
    /// context carried by [`SessionOutcome::Accepted`]; this one is for
    /// `@` blocks (which establish nothing) and inspection.
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

    /// Records whether the most recently completed real execution
    /// succeeded (spec section 9; Xact–Mesut Integration Phase 8),
    /// consulted by [`Session::dependency_satisfied`] for `WHEN
    /// THIS/THAT SUCCEEDS`/`FAILS`. `xact-core` never executes anything
    /// itself — a caller (`xact-cli`) reports the real result back here
    /// once it has one.
    ///
    /// One session-wide slot: both `WHEN THIS` and `WHEN THAT` ask about
    /// the outcome of whatever most recently *actually ran*. (`THIS`/`THAT`
    /// differ as objects — input vs result, see `xact-reference` — but a
    /// command has a single outcome.)
    pub fn record_outcome(&mut self, success: bool) {
        self.last_outcome = Some(success);
    }

    /// Whether `dependency`'s stated condition is satisfied by the most
    /// recently recorded real outcome (see [`Session::record_outcome`]).
    /// `false` — not satisfied, meaning the dependent command should be
    /// skipped rather than run — if nothing has actually run yet.
    pub fn dependency_satisfied(&self, dependency: &xact_ast::DependencyClause) -> bool {
        self.last_outcome.is_some_and(|success| dependency.condition.is_satisfied_by(success))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn their_rejected_then_accepted_after_identity() {
        let mut session = Session::new();
        assert!(matches!(session.submit("£ SEE THEIR ~/shared"), SessionOutcome::Rejected(_)));
        assert!(matches!(session.submit("£ THEY are \"root\""), SessionOutcome::Accepted(..)));
        assert!(matches!(session.submit("£ SEE THEIR ~/shared"), SessionOutcome::Accepted(..)));
    }

    #[test]
    fn that_resolves_to_previously_established_object() {
        let mut session = Session::new();
        assert!(matches!(session.submit("£ SEE MY ~/project/README.md"), SessionOutcome::Accepted(..)));
        assert!(matches!(session.submit("£ EDIT THAT"), SessionOutcome::Accepted(..)));
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
        assert!(matches!(session.submit("! SAVE 20%RAM 30%CPU"), SessionOutcome::PolicyAccepted(_)));
        assert!(matches!(session.submit("£ RUN 'chrome'"), SessionOutcome::Accepted(..)));
    }

    #[test]
    fn conflicting_policy_statements_rejected() {
        let mut session = Session::new();
        assert!(matches!(session.submit("! CONCURRENTLY"), SessionOutcome::PolicyAccepted(_)));
        assert!(matches!(session.submit("! CONSECUTIVELY"), SessionOutcome::Rejected(_)));
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
        assert!(matches!(session.submit("@ TELL 'x' READING THEIR ~/shared \"go\""), SessionOutcome::Rejected(_)));
    }

    #[test]
    fn agent_populating_that_resolves_from_prior_command() {
        let mut session = Session::new();
        assert!(matches!(session.submit("£ SEE MY ~/project/review/"), SessionOutcome::Accepted(..)));
        assert!(matches!(session.submit("@ TELL 'x' POPULATING THAT \"go\""), SessionOutcome::AgentAccepted(_)));
    }

    fn when_that(condition: &str) -> String {
        format!("£ RUN 'x' WHEN THAT {condition}")
    }

    #[test]
    fn dependency_not_satisfied_before_anything_has_run() {
        let session = Session::new();
        let dependency = xact_ast::DependencyClause {
            reference: xact_ast::ReferenceKind::That,
            reference_span: Default::default(),
            condition: xact_ast::SuccessCondition::Succeeds,
            condition_span: Default::default(),
        };
        assert!(!session.dependency_satisfied(&dependency));
    }

    #[test]
    fn dependency_satisfied_after_a_matching_recorded_outcome() {
        let mut session = Session::new();
        session.record_outcome(true);
        let succeeds = xact_ast::DependencyClause {
            reference: xact_ast::ReferenceKind::That,
            reference_span: Default::default(),
            condition: xact_ast::SuccessCondition::Succeeds,
            condition_span: Default::default(),
        };
        let fails = xact_ast::DependencyClause { condition: xact_ast::SuccessCondition::Fails, ..succeeds };
        assert!(session.dependency_satisfied(&succeeds));
        assert!(!session.dependency_satisfied(&fails));

        session.record_outcome(false);
        assert!(!session.dependency_satisfied(&succeeds));
        assert!(session.dependency_satisfied(&fails));
    }

    #[test]
    fn when_that_parses_and_validates_once_something_is_established() {
        let mut session = Session::new();
        assert!(matches!(session.submit("£ RUN 'compile'"), SessionOutcome::Accepted(..)));
        assert!(matches!(session.submit(&when_that("SUCCEEDS")), SessionOutcome::Accepted(..)));
    }

    #[test]
    fn they_with_an_unknown_user_is_rejected() {
        let mut session = Session::new();
        assert!(matches!(session.submit("£ THEY are \"xact-no-such-user-for-tests\""), SessionOutcome::Rejected(_)));
    }

    #[test]
    fn without_rejects_an_operation_that_needs_the_capability() {
        let mut session = Session::new();
        assert!(matches!(session.submit("! WITHOUT filesystem-write"), SessionOutcome::PolicyAccepted(_)));
        match session.submit("£ CREATE MY /tmp/xact-never-created") {
            SessionOutcome::Rejected(diagnostics) => {
                assert!(diagnostics[0].message.contains("WITHOUT filesystem-write"), "{diagnostics:?}")
            }
            other => panic!("expected rejection, got {other:?}"),
        }
        // Operations that don't need it are unaffected.
        assert!(matches!(session.submit("£ SEE MY /tmp"), SessionOutcome::Accepted(..)));
    }

    #[test]
    fn rejected_commands_establish_no_reference() {
        let mut session = Session::new();
        session.submit("! WITHOUT filesystem-write");
        session.submit("£ CREATE MY /tmp/xact-never-created");
        assert!(matches!(session.submit("£ SEE THAT"), SessionOutcome::Rejected(_)));
    }

    #[test]
    fn this_and_that_name_a_copys_source_and_destination() {
        let mut session = Session::new();
        session.submit("£ COPY MY /tmp/a to MY /tmp/b");
        assert_eq!(
            session.references().resolve(xact_ast::ReferenceKind::This),
            Some(&xact_reference::ResolvedObject::Path("/tmp/a".into()))
        );
        assert_eq!(
            session.references().resolve(xact_ast::ReferenceKind::That),
            Some(&xact_reference::ResolvedObject::Path("/tmp/b".into()))
        );
    }

    #[test]
    fn a_cloned_session_validates_without_touching_the_original() {
        let session = Session::new();
        let mut dry_run = session.clone();
        assert!(matches!(dry_run.submit("£ SEE MY /tmp"), SessionOutcome::Accepted(..)));
        assert!(session.references().resolve(xact_ast::ReferenceKind::That).is_none());
    }

    /// The bug this guards against: `submit` records a command's own
    /// result before returning, so planning `£ COPY THAT to ...` against the
    /// session's *current* references would make THAT the destination.
    #[test]
    fn an_accepted_command_carries_the_references_it_was_validated_in() {
        let mut session = Session::new();
        session.submit("£ CREATE MY /tmp/xact-made.txt");
        match session.submit("£ COPY THAT to MY /tmp/xact-copy.txt") {
            SessionOutcome::Accepted(_, references) => assert_eq!(
                references.resolve(xact_ast::ReferenceKind::That),
                Some(&xact_reference::ResolvedObject::Path("/tmp/xact-made.txt".into()))
            ),
            other => panic!("expected acceptance, got {other:?}"),
        }
    }

    #[test]
    fn without_clipboard_rejects_edit_before_it_is_accepted() {
        let mut session = Session::new();
        assert!(matches!(session.submit("! WITHOUT clipboard"), SessionOutcome::PolicyAccepted(_)));
        match session.submit("£ EDIT MY /tmp/xact-never-edited.txt") {
            SessionOutcome::Rejected(diagnostics) => {
                assert!(diagnostics[0].message.contains("WITHOUT clipboard"), "{diagnostics:?}")
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }
}
