//! Typed AST for the Xact `£` imperative language (spec section 7),
//! ownership vocabulary (section 10), reference vocabulary (section 11),
//! `!` policy language (spec section 8), and `@` agent language (section 9).

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
    /// `£ CREATE MY ~/project/` — establish a file or directory. This is a
    /// language-level intent, distinct from `bank`, the tool that happens
    /// to execute it (spec section 6/13's Xact-owns-intent,
    /// tool-owns-implementation boundary — the same distinction section 12
    /// draws between `BOUND` and the `bound` tool).
    Create,
    /// `£ BOUND MY ~/project/ to MY ~/bundle.txt` — aggregate a deliberately
    /// established source set (spec section 12), distinct from `bound`, the
    /// tool that happens to execute it. The `to` destination (already
    /// generic grammar, spec section 7) is `bound`'s own `--out` file; with
    /// none given, `bound`'s own clipboard default applies.
    Bound,
}

impl Verb {
    pub const ALL: [Verb; 10] = [
        Verb::See,
        Verb::Edit,
        Verb::Move,
        Verb::Copy,
        Verb::Paste,
        Verb::Cut,
        Verb::Delete,
        Verb::Run,
        Verb::Create,
        Verb::Bound,
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
            Verb::Create => "CREATE",
            Verb::Bound => "BOUND",
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
            "CREATE" => Verb::Create,
            "BOUND" => Verb::Bound,
            _ => return None,
        })
    }

    /// Whether this verb accepts a `to <ownership> <path>` destination clause.
    pub fn accepts_destination(&self) -> bool {
        matches!(self, Verb::Move | Verb::Copy | Verb::Paste | Verb::Cut | Verb::Bound)
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

/// `£ RUN 'test' WHEN THAT SUCCEEDS` — a dependency clause (spec section 9;
/// Xact–Mesut Integration Phase 8): this command only actually runs once
/// `reference`'s most recent real execution outcome satisfies `condition`.
/// Xact represents the dependency; the executor is what checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuccessCondition {
    Succeeds,
    Fails,
}

impl SuccessCondition {
    pub fn as_str(&self) -> &'static str {
        match self {
            SuccessCondition::Succeeds => "SUCCEEDS",
            SuccessCondition::Fails => "FAILS",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "SUCCEEDS" => SuccessCondition::Succeeds,
            "FAILS" => SuccessCondition::Fails,
            _ => return None,
        })
    }

    /// Whether a real execution outcome of `success` satisfies this
    /// condition.
    pub fn is_satisfied_by(&self, success: bool) -> bool {
        match self {
            SuccessCondition::Succeeds => success,
            SuccessCondition::Fails => !success,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DependencyClause {
    pub reference: ReferenceKind,
    pub reference_span: Span,
    pub condition: SuccessCondition,
    pub condition_span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImperativeCommand {
    pub verb: Verb,
    pub verb_span: Span,
    pub operand: Option<Operand>,
    /// `COPY THAT to OUR ~/backup` — the `to` clause destination.
    pub destination: Option<Operand>,
    /// `RUN 'test' WHEN THAT SUCCEEDS` — the `WHEN` clause dependency.
    pub dependency: Option<DependencyClause>,
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

/// The effective resource constraint a session's accumulated `SPEND`/
/// `SAVE` statements resolve to (spec section 22; Xact–Mesut Integration
/// Phase 5): `SPEND N%X` caps the workload at `N%` of `X`; `SAVE N%X`
/// reserves `N%` of `X` for the rest of the system, i.e. caps the workload
/// at `(100 - N)%`. `xact-policy` computes this; `xact-resource` is the
/// only crate that enforces it. A default `ResourceBudget` (`None`/`None`/
/// empty) means no constraint is in effect — execution is unconstrained,
/// exactly today's pre-Phase-5 behavior.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceBudget {
    /// Cap as a percentage of one CPU core (cgroup v2's own native unit —
    /// `100` means one full core, `400` would mean four, though nothing
    /// here produces a value above `100` since a single `SPEND`/`SAVE`
    /// percent is grammatically capped at that range).
    pub cpu_percent: Option<u32>,
    /// Cap as a percentage of total system RAM.
    pub ram_percent: Option<u32>,
    /// Resource names present in the session's policy that Xact has no
    /// enforcement mechanism for yet (anything but `CPU`/`RAM`). Never
    /// silently dropped: a non-empty list here means Xact–Mesut
    /// Integration section 10's rule applies — "execution SHALL fail
    /// before the workload begins" — rather than running unconstrained
    /// while pretending the stated policy was honoured.
    pub unenforceable: Vec<String>,
}

impl ResourceBudget {
    /// No constraint at all — nothing stated, or everything named was
    /// already accounted for as CPU/RAM.
    pub fn is_empty(&self) -> bool {
        self.cpu_percent.is_none() && self.ram_percent.is_none() && self.unenforceable.is_empty()
    }
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

/// The `@` agent primitives that head a block (spec section 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentVerb {
    Tell,
    Team,
}

impl AgentVerb {
    pub const ALL: [AgentVerb; 2] = [AgentVerb::Tell, AgentVerb::Team];

    pub fn as_str(&self) -> &'static str {
        match self {
            AgentVerb::Tell => "TELL",
            AgentVerb::Team => "TEAM",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "TELL" => AgentVerb::Tell,
            "TEAM" => AgentVerb::Team,
            _ => return None,
        })
    }
}

/// The clause keywords that configure an agent block (spec section 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentClauseKind {
    Be,
    Reading,
    Populating,
    Think,
}

impl AgentClauseKind {
    pub const ALL: [AgentClauseKind; 4] = [
        AgentClauseKind::Be,
        AgentClauseKind::Reading,
        AgentClauseKind::Populating,
        AgentClauseKind::Think,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            AgentClauseKind::Be => "BE",
            AgentClauseKind::Reading => "READING",
            AgentClauseKind::Populating => "POPULATING",
            AgentClauseKind::Think => "THINK",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "BE" => AgentClauseKind::Be,
            "READING" => AgentClauseKind::Reading,
            "POPULATING" => AgentClauseKind::Populating,
            "THINK" => AgentClauseKind::Think,
            _ => return None,
        })
    }
}

/// One configured clause of an `@` agent block. Each kind is a singleton —
/// it may appear at most once per block (spec section 16's operator
/// metadata, applied here to agent clauses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentClause {
    Be { persona: String, span: Span },
    Reading { operand: Operand, span: Span },
    Populating { operand: Operand, span: Span },
    Think { budget: u32, span: Span },
}

impl AgentClause {
    pub fn kind(&self) -> AgentClauseKind {
        match self {
            AgentClause::Be { .. } => AgentClauseKind::Be,
            AgentClause::Reading { .. } => AgentClauseKind::Reading,
            AgentClause::Populating { .. } => AgentClauseKind::Populating,
            AgentClause::Think { .. } => AgentClauseKind::Think,
        }
    }
}

/// `@ TELL 'GPT-5.6-luna' BE "..." READING MY ~/project/ POPULATING MY
/// ~/project/review/ THINK 80 "Review this project."` (spec section 9).
///
/// Agent blocks produce semantic intents, not unrestricted shell access —
/// the policy engine remains authoritative over whatever a planner later
/// does with one (spec section 9, section 21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentBlock {
    pub verb: AgentVerb,
    pub verb_span: Span,
    pub target: String,
    pub target_span: Span,
    pub clauses: Vec<AgentClause>,
    pub instruction: String,
    pub instruction_span: Span,
}

impl AgentBlock {
    pub fn clause(&self, kind: AgentClauseKind) -> Option<&AgentClause> {
        self.clauses.iter().find(|c| c.kind() == kind)
    }
}

/// One parsed input line: a policy statement, an imperative/identity
/// command, or an agent block (spec section 17: a block is made of these
/// kinds of line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Policy(PolicyStatement),
    Command(Command),
    Agent(AgentBlock),
}
