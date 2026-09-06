//! Typed AST for the Xact `£` imperative language (spec section 7),
//! ownership vocabulary (section 10) and reference vocabulary (section 11).
//!
//! Phase 1 scope only: the policy (`!`) and agent (`@`) languages are not
//! modelled yet.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    See,
    Edit,
    Move,
    Copy,
    Paste,
    Cut,
    Delete,
    Run,
    Bank,
}

impl Verb {
    pub const ALL: [Verb; 9] = [
        Verb::See,
        Verb::Edit,
        Verb::Move,
        Verb::Copy,
        Verb::Paste,
        Verb::Cut,
        Verb::Delete,
        Verb::Run,
        Verb::Bank,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Verb::See => "SEE",
            Verb::Edit => "EDIT",
            Verb::Move => "MOVE",
            Verb::Copy => "COPY",
            Verb::Paste => "PASTE",
            Verb::Cut => "CUT",
            Verb::Delete => "DELETE",
            Verb::Run => "RUN",
            Verb::Bank => "BANK",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "SEE" => Verb::See,
            "EDIT" => Verb::Edit,
            "MOVE" => Verb::Move,
            "COPY" => Verb::Copy,
            "PASTE" => Verb::Paste,
            "CUT" => Verb::Cut,
            "DELETE" => Verb::Delete,
            "RUN" => Verb::Run,
            "BANK" => Verb::Bank,
            _ => return None,
        })
    }

    /// Whether this verb accepts a `to <ownership> <path>` destination clause.
    pub fn accepts_destination(&self) -> bool {
        matches!(self, Verb::Move | Verb::Copy | Verb::Paste | Verb::Cut)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnershipKind {
    My,
    Our,
    Their,
}

impl OwnershipKind {
    pub const ALL: [OwnershipKind; 3] = [OwnershipKind::My, OwnershipKind::Our, OwnershipKind::Their];

    pub fn as_str(&self) -> &'static str {
        match self {
            OwnershipKind::My => "MY",
            OwnershipKind::Our => "OUR",
            OwnershipKind::Their => "THEIR",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "MY" => OwnershipKind::My,
            "OUR" => OwnershipKind::Our,
            "THEIR" => OwnershipKind::Their,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    This,
    That,
}

impl ReferenceKind {
    pub const ALL: [ReferenceKind; 2] = [ReferenceKind::This, ReferenceKind::That];

    pub fn as_str(&self) -> &'static str {
        match self {
            ReferenceKind::This => "THIS",
            ReferenceKind::That => "THAT",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "THIS" => ReferenceKind::This,
            "THAT" => ReferenceKind::That,
            _ => return None,
        })
    }
}

/// A target for an imperative command. `Reference` variants are unresolved
/// until semantic analysis resolves them against a `ReferenceContext`
/// (spec section 11: an unresolved reference is a semantic error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    Owned {
        kind: OwnershipKind,
        path: String,
        span: Span,
    },
    Reference {
        kind: ReferenceKind,
        span: Span,
    },
    StringArg {
        value: String,
        span: Span,
    },
}

impl Operand {
    pub fn span(&self) -> Span {
        match self {
            Operand::Owned { span, .. } => *span,
            Operand::Reference { span, .. } => *span,
            Operand::StringArg { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImperativeCommand {
    pub verb: Verb,
    pub verb_span: Span,
    pub operand: Option<Operand>,
    /// `COPY THAT to OUR ~/backup` — the `to` clause destination.
    pub destination: Option<Operand>,
}

/// `£ THEY are "alice,bob"` — establishes the identity context that `THEIR`
/// resolves against (spec section 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityDeclaration {
    pub members: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Imperative(ImperativeCommand),
    Identity(IdentityDeclaration),
}
