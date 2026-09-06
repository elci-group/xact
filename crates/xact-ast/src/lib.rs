//! Typed AST for the Xact `£` imperative language (spec section 7),
//! ownership vocabulary (section 10), reference vocabulary (section 11),
//! and `!` policy language (spec section 8).
//!
//! The agent (`@`) language is not modelled yet.

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

/// The `!` policy operators (spec section 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyOperator {
    With,
    Without,
    Prefer,
    Dodge,
    Spend,
    Save,
    Concurrently,
    Consecutively,
    When,
}

/// What shape of argument a policy operator takes — grammar metadata the
/// parser uses to decide how to consume the rest of the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyArgKind {
    /// `CONCURRENTLY` / `CONSECUTIVELY` — a bare scheduling mode, no args.
    None,
    /// `WITH`/`WITHOUT`/`PREFER`/`DODGE` — a single named capability.
    Capability,
    /// `SPEND`/`SAVE` — one or more resource quotas, e.g. `20%RAM 30%CPU`.
    Quotas,
    /// `WHEN` — a condition. Phase 2 stores this as opaque text; a real
    /// condition language is future work.
    Condition,
}

impl PolicyOperator {
    pub const ALL: [PolicyOperator; 9] = [
        PolicyOperator::With,
        PolicyOperator::Without,
        PolicyOperator::Prefer,
        PolicyOperator::Dodge,
        PolicyOperator::Spend,
        PolicyOperator::Save,
        PolicyOperator::Concurrently,
        PolicyOperator::Consecutively,
        PolicyOperator::When,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            PolicyOperator::With => "WITH",
            PolicyOperator::Without => "WITHOUT",
            PolicyOperator::Prefer => "PREFER",
            PolicyOperator::Dodge => "DODGE",
            PolicyOperator::Spend => "SPEND",
            PolicyOperator::Save => "SAVE",
            PolicyOperator::Concurrently => "CONCURRENTLY",
            PolicyOperator::Consecutively => "CONSECUTIVELY",
            PolicyOperator::When => "WHEN",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "WITH" => PolicyOperator::With,
            "WITHOUT" => PolicyOperator::Without,
            "PREFER" => PolicyOperator::Prefer,
            "DODGE" => PolicyOperator::Dodge,
            "SPEND" => PolicyOperator::Spend,
            "SAVE" => PolicyOperator::Save,
            "CONCURRENTLY" => PolicyOperator::Concurrently,
            "CONSECUTIVELY" => PolicyOperator::Consecutively,
            "WHEN" => PolicyOperator::When,
            _ => return None,
        })
    }

    pub fn arg_kind(&self) -> PolicyArgKind {
        match self {
            PolicyOperator::Concurrently | PolicyOperator::Consecutively => PolicyArgKind::None,
            PolicyOperator::With | PolicyOperator::Without | PolicyOperator::Prefer | PolicyOperator::Dodge => {
                PolicyArgKind::Capability
            }
            PolicyOperator::Spend | PolicyOperator::Save => PolicyArgKind::Quotas,
            PolicyOperator::When => PolicyArgKind::Condition,
        }
    }

    /// Hard constraints are evaluated before preferences (spec section 8)
    /// and a soft preference may never override one (section 27's policy
    /// invariant).
    pub fn is_hard_constraint(&self) -> bool {
        matches!(
            self,
            PolicyOperator::With | PolicyOperator::Without | PolicyOperator::Spend | PolicyOperator::Save
        )
    }

    /// Scheduling mode is chosen once per block (spec section 16: a
    /// singleton operator must not be suggested again once established).
    pub fn is_singleton(&self) -> bool {
        matches!(self, PolicyOperator::Concurrently | PolicyOperator::Consecutively)
    }

    /// The operator this one mutually excludes from the same block, if any
    /// (spec section 16).
    pub fn excludes(&self) -> Option<PolicyOperator> {
        match self {
            PolicyOperator::Concurrently => Some(PolicyOperator::Consecutively),
            PolicyOperator::Consecutively => Some(PolicyOperator::Concurrently),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceQuota {
    pub percent: u32,
    pub resource: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyArgs {
    None,
    Capability { name: String, span: Span },
    Quotas(Vec<ResourceQuota>),
    Condition { text: String, span: Span },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyStatement {
    pub operator: PolicyOperator,
    pub operator_span: Span,
    pub args: PolicyArgs,
}

/// One parsed input line: either a policy statement or an imperative/identity
/// command (spec section 17: a block is made of both kinds of line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Policy(PolicyStatement),
    Command(Command),
}
