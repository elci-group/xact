//! Agent language (spec section 9): validates an `@ TELL`/`@ TEAM` block
//! into a semantic intent. Agents "shall produce semantic intents rather
//! than directly receiving unrestricted shell access" — this crate is
//! exactly that boundary. The Xact policy engine remains authoritative
//! over whatever a planner later does with an accepted intent (still a
//! stub in `xact-planner`); this crate does not dispatch to any actual
//! agent provider.
//!
//! `READING`/`POPULATING` clauses carry an [`xact_ast::Operand`], the same
//! type imperative commands use, so they get the same ownership/reference
//! validation for free by reusing `xact_semantic::check_operand` rather
//! than reimplementing it (spec section 3's composition principle, applied
//! within the workspace rather than across ELci tools).

use xact_ast::{AgentBlock, AgentClause};
use xact_diagnostics::Diagnostic;
use xact_reference::ReferenceContext;
use xact_semantic::{check_operand, IdentityContext};

/// An [`AgentBlock`] that has passed ownership/reference validation on its
/// `READING`/`POPULATING` clauses.
#[derive(Debug, Clone)]
pub struct ValidatedAgentBlock {
    pub block: AgentBlock,
}

pub fn validate(
    block: AgentBlock,
    identity: &IdentityContext,
    references: &ReferenceContext,
) -> Result<ValidatedAgentBlock, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    for clause in &block.clauses {
        match clause {
            AgentClause::Reading { operand, .. } | AgentClause::Populating { operand, .. } => {
                check_operand(Some(operand), identity, references, &mut diagnostics);
            }
            AgentClause::Be { .. } | AgentClause::Think { .. } => {}
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    Ok(ValidatedAgentBlock { block })
}

#[cfg(test)]
mod tests {
    use super::*;
    use xact_ast::{AgentVerb, Operand, OwnershipKind, ReferenceKind, Span};

    fn block_with_clauses(clauses: Vec<AgentClause>) -> AgentBlock {
        AgentBlock {
            verb: AgentVerb::Tell,
            verb_span: Span::default(),
            target: "GPT-5.6-luna".into(),
            target_span: Span::default(),
            clauses,
            instruction: "Review this project.".into(),
            instruction_span: Span::default(),
        }
    }

    #[test]
    fn block_with_no_clauses_is_valid() {
        let identity = IdentityContext::new();
        let references = ReferenceContext::new();
        assert!(validate(block_with_clauses(vec![]), &identity, &references).is_ok());
    }

    #[test]
    fn reading_their_rejected_without_identity() {
        let identity = IdentityContext::new();
        let references = ReferenceContext::new();
        let block = block_with_clauses(vec![AgentClause::Reading {
            operand: Operand::Owned {
                kind: OwnershipKind::Their,
                path: "~/shared".into(),
                span: Span::default(),
            },
            span: Span::default(),
        }]);
        assert!(validate(block, &identity, &references).is_err());
    }

    #[test]
    fn populating_that_resolves_against_reference_context() {
        let identity = IdentityContext::new();
        let mut references = ReferenceContext::new();
        references.record(xact_reference::ResolvedObject::Path("~/project/review".into()));
        let block = block_with_clauses(vec![AgentClause::Populating {
            operand: Operand::Reference {
                kind: ReferenceKind::That,
                span: Span::default(),
            },
            span: Span::default(),
        }]);
        assert!(validate(block, &identity, &references).is_ok());
    }

    #[test]
    fn populating_that_rejected_with_no_prior_result() {
        let identity = IdentityContext::new();
        let references = ReferenceContext::new();
        let block = block_with_clauses(vec![AgentClause::Populating {
            operand: Operand::Reference {
                kind: ReferenceKind::That,
                span: Span::default(),
            },
            span: Span::default(),
        }]);
        assert!(validate(block, &identity, &references).is_err());
    }
}
