//! Contextual, actionable diagnostics (spec section 24). The same
//! `expected` continuation list a diagnostic carries is also what
//! `xact-completion` offers proactively — there is no separate
//! "autocomplete grammar" (spec section 14).

use std::fmt;
use xact_ast::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// The input so far is a valid prefix of some program but is not yet
    /// complete (spec section 18: an incomplete draft, not an executable
    /// program).
    Incomplete,
    /// The input can never be completed into a valid, executable program
    /// (e.g. an unknown verb, or a reference that cannot resolve).
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
    pub span: Span,
    /// Deterministic valid next tokens (spec section 15: `CompletionSet`).
    pub expected: Vec<String>,
}

impl Diagnostic {
    pub fn incomplete(message: impl Into<String>, span: Span, expected: Vec<String>) -> Self {
        Self {
            kind: DiagnosticKind::Incomplete,
            message: message.into(),
            span,
            expected,
        }
    }

    pub fn invalid(message: impl Into<String>, span: Span, expected: Vec<String>) -> Self {
        Self {
            kind: DiagnosticKind::Invalid,
            message: message.into(),
            span,
            expected,
        }
    }

    /// Whether this diagnostic blocks execution. Both kinds do — the only
    /// difference is whether more input could still resolve it (spec
    /// section 19: every validation level must be capable of preventing
    /// execution).
    pub fn blocks_execution(&self) -> bool {
        true
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.message)?;
        if !self.expected.is_empty() {
            writeln!(f)?;
            writeln!(f, "Valid continuations:")?;
            for e in &self.expected {
                writeln!(f, "  {e}")?;
            }
        }
        Ok(())
    }
}
