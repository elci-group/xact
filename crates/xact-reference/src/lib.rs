//! Reference resolution for `THIS`/`THAT` (spec section 11).
//!
//! `THIS`/`THAT` are semantic object references, not textual substitutions.
//! An unresolved reference is a semantic error and must never reach
//! execution (the reference invariant, spec section 27).
//!
//! `THIS` and `THAT` are distinct (decision D-2 in
//! `XACT_COMPLETION_DIRECTIVE.md`): after any command, `THAT` is what it
//! produced — its destination, or its target when it has none — and `THIS`
//! is what it consumed — its source operand. `£ COPY MY ~/a to MY ~/b`
//! leaves `THIS` = `~/a` and `THAT` = `~/b`; for a single-operand command
//! such as `£ SEE MY ~/x` both name the same object. Every spec example
//! (`£ SEE ...` then `£ EDIT THAT`, `WHEN THAT SUCCEEDS`) reads as before.
//!
//! The further step to a `TypedResource` (spec section 11's diagram) —
//! knowing a resolved object is specifically a file, directory, or process
//! — is still future work (directive M2.2).

use xact_ast::ReferenceKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedObject {
    Path(String),
    Text(String),
}

/// The input and result of the most recent command that established an
/// object (see the module doc).
#[derive(Debug, Clone, Default)]
pub struct ReferenceContext {
    input: Option<ResolvedObject>,
    result: Option<ResolvedObject>,
}

impl ReferenceContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// `THIS` resolves to the last command's input, `THAT` to its result.
    pub fn resolve(&self, kind: ReferenceKind) -> Option<&ResolvedObject> {
        match kind {
            ReferenceKind::This => self.input.as_ref(),
            ReferenceKind::That => self.result.as_ref(),
        }
    }

    /// Records a command that consumed `input` and produced `result`.
    pub fn record(&mut self, input: ResolvedObject, result: ResolvedObject) {
        self.input = Some(input);
        self.result = Some(result);
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
        let readme = ResolvedObject::Path("/home/u/project/README.md".into());
        ctx.record(readme.clone(), readme.clone());
        assert_eq!(ctx.resolve(ReferenceKind::That), Some(&readme));
        assert_eq!(ctx.resolve(ReferenceKind::This), Some(&readme));
    }

    #[test]
    fn this_is_the_input_and_that_is_the_result() {
        let mut ctx = ReferenceContext::new();
        let source = ResolvedObject::Path("/home/u/a".into());
        let destination = ResolvedObject::Path("/home/u/b".into());
        ctx.record(source.clone(), destination.clone());
        assert_eq!(ctx.resolve(ReferenceKind::This), Some(&source));
        assert_eq!(ctx.resolve(ReferenceKind::That), Some(&destination));
    }
}
