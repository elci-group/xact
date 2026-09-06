//! Reference resolution for `THIS`/`THAT` (spec section 11).
//!
//! `THIS`/`THAT` are semantic object references, not textual substitutions.
//! An unresolved reference is a semantic error and must never reach
//! execution (the reference invariant, spec section 27).
//!
//! Phase 1 models the conceptual pipeline `ObjectRef -> ResolvedObject`
//! only; the further step to a `TypedResource` (spec section 11's diagram)
//! — knowing a resolved object is specifically a file, directory, or
//! process — is deferred to the semantic/executor phases once there is a
//! real filesystem/process layer to type-check against.

use xact_ast::ReferenceKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedObject {
    Path(String),
    Text(String),
}

/// Tracks the most recently established object so that a later `THIS`/
/// `THAT` can resolve against it (spec section 11 example: `SEE MY
/// ~/project/README.md` followed by `EDIT THAT`).
///
/// Phase 1 does not yet distinguish `THIS` (the object just established in
/// the current command) from `THAT` (an earlier result) — both resolve to
/// the single most recent object. Disambiguating them needs a real
/// multi-object session history, which belongs to `xact-core` in a later
/// phase.
#[derive(Debug, Clone, Default)]
pub struct ReferenceContext {
    last: Option<ResolvedObject>,
}

impl ReferenceContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve(&self, _kind: ReferenceKind) -> Option<&ResolvedObject> {
        self.last.as_ref()
    }

    pub fn record(&mut self, object: ResolvedObject) {
        self.last = Some(object);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresolved_reference_is_none() {
        let ctx = ReferenceContext::new();
        assert_eq!(ctx.resolve(ReferenceKind::That), None);
    }

    #[test]
    fn resolves_after_recording() {
        let mut ctx = ReferenceContext::new();
        ctx.record(ResolvedObject::Path("~/project/README.md".into()));
        assert_eq!(
            ctx.resolve(ReferenceKind::That),
            Some(&ResolvedObject::Path("~/project/README.md".into()))
        );
    }
}
