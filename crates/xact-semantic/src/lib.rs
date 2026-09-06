//! Semantic validation: ownership and reference resolution (spec sections
//! 10, 11, 19). A grammatically complete [`xact_ast::Command`] can still be
//! semantically invalid — e.g. `THEIR` used before any identity is
//! established, or `THAT` used before anything has been established to
//! refer to.
//!
//! Enforces two of the spec's testing invariants (section 27):
//! - **Ownership invariant**: no unresolved ownership expression reaches execution.
//! - **Reference invariant**: no unresolved `THIS`/`THAT` reference reaches execution.

use xact_ast::{Command, ImperativeCommand, Operand, OwnershipKind};
use xact_diagnostics::Diagnostic;
use xact_reference::{ReferenceContext, ResolvedObject};

/// The identity context `THEIR` resolves against, established by
/// `£ THEY are "alice,bob"` (spec section 10).
#[derive(Debug, Clone, Default)]
pub struct IdentityContext {
    members: Vec<String>,
}

impl IdentityContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn established(&self) -> bool {
        !self.members.is_empty()
    }

    pub fn set(&mut self, members: Vec<String>) {
        self.members = members;
    }

    pub fn members(&self) -> &[String] {
        &self.members
    }
}

/// A [`Command`] that has passed ownership and reference validation, paired
/// with the object it establishes (if any) for `xact-reference` to record
/// once the command actually runs.
#[derive(Debug, Clone)]
pub struct ValidatedCommand {
    pub command: Command,
    pub establishes: Option<ResolvedObject>,
}

pub fn validate(
    command: Command,
    identity: &IdentityContext,
    references: &ReferenceContext,
) -> Result<ValidatedCommand, Vec<Diagnostic>> {
    match &command {
        Command::Identity(_) => Ok(ValidatedCommand {
            command,
            establishes: None,
        }),
        Command::Imperative(cmd) => {
            let mut diagnostics = Vec::new();
            check_operand(cmd.operand.as_ref(), identity, references, &mut diagnostics);
            check_operand(cmd.destination.as_ref(), identity, references, &mut diagnostics);
            if !diagnostics.is_empty() {
                return Err(diagnostics);
            }
            let establishes = resolve_established(cmd, references);
            Ok(ValidatedCommand { command, establishes })
        }
    }
}

fn check_operand(
    operand: Option<&Operand>,
    identity: &IdentityContext,
    references: &ReferenceContext,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(operand) = operand else { return };
    match operand {
        Operand::Owned {
            kind: OwnershipKind::Their,
            span,
            ..
        } => {
            if !identity.established() {
                diagnostics.push(Diagnostic::invalid(
                    "THEIR cannot be used before an identity is established (e.g. £ THEY are \"alice,bob\").",
                    *span,
                    vec![],
                ));
            }
        }
        Operand::Reference { kind, span } => {
            if references.resolve(*kind).is_none() {
                diagnostics.push(Diagnostic::invalid(
                    format!("{} does not refer to anything yet.", kind.as_str()),
                    *span,
                    vec![],
                ));
            }
        }
        Operand::Owned { .. } | Operand::StringArg { .. } => {}
    }
}

/// What this command establishes as the new resolvable object for a later
/// `THIS`/`THAT` — the destination if there is one, else the operand.
fn resolve_established(cmd: &ImperativeCommand, references: &ReferenceContext) -> Option<ResolvedObject> {
    let target = cmd.destination.as_ref().or(cmd.operand.as_ref())?;
    match target {
        Operand::Owned { path, .. } => Some(ResolvedObject::Path(path.clone())),
        Operand::Reference { kind, .. } => references.resolve(*kind).cloned(),
        Operand::StringArg { value, .. } => Some(ResolvedObject::Text(value.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xact_ast::{Span, Verb};

    fn imperative(operand: Option<Operand>) -> Command {
        Command::Imperative(ImperativeCommand {
            verb: Verb::See,
            verb_span: Span::default(),
            operand,
            destination: None,
        })
    }

    #[test]
    fn their_rejected_without_identity() {
        let identity = IdentityContext::new();
        let references = ReferenceContext::new();
        let cmd = imperative(Some(Operand::Owned {
            kind: OwnershipKind::Their,
            path: "~/x".into(),
            span: Span::default(),
        }));
        let result = validate(cmd, &identity, &references);
        assert!(result.is_err());
    }

    #[test]
    fn their_accepted_after_identity_established() {
        let mut identity = IdentityContext::new();
        identity.set(vec!["alice".into()]);
        let references = ReferenceContext::new();
        let cmd = imperative(Some(Operand::Owned {
            kind: OwnershipKind::Their,
            path: "~/x".into(),
            span: Span::default(),
        }));
        assert!(validate(cmd, &identity, &references).is_ok());
    }

    #[test]
    fn unresolved_that_rejected() {
        let identity = IdentityContext::new();
        let references = ReferenceContext::new();
        let cmd = imperative(Some(Operand::Reference {
            kind: xact_ast::ReferenceKind::That,
            span: Span::default(),
        }));
        assert!(validate(cmd, &identity, &references).is_err());
    }

    #[test]
    fn resolved_that_accepted_and_records_establishment() {
        let identity = IdentityContext::new();
        let mut references = ReferenceContext::new();
        references.record(ResolvedObject::Path("~/project/README.md".into()));
        let cmd = imperative(Some(Operand::Reference {
            kind: xact_ast::ReferenceKind::That,
            span: Span::default(),
        }));
        let validated = validate(cmd, &identity, &references).expect("should validate");
        assert_eq!(
            validated.establishes,
            Some(ResolvedObject::Path("~/project/README.md".into()))
        );
    }
}
