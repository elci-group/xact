//! Policy evaluation (spec section 8): accumulates the `!` statements
//! established so far in the current block and enforces the invariants the
//! spec calls out explicitly:
//!
//! - hard constraints (`WITH`/`WITHOUT`/`SPEND`/`SAVE`) are evaluated
//!   before preferences, and no soft preference may override a hard
//!   constraint (section 8, section 27's policy invariant);
//! - `CONCURRENTLY`/`CONSECUTIVELY` are singleton and mutually exclusive
//!   for a block (section 16).
//!
//! Turning an accepted [`PolicyContext`] into enforcement:
//!
//! - `SPEND`/`SAVE` resolve to a [`ResourceBudget`] that `xact-resource`
//!   enforces with cgroup v2; `CONCURRENTLY`/`CONSECUTIVELY` drive the
//!   CLI's scheduling (Xact–Mesut Integration Phases 3/5).
//! - `WITHOUT <capability>` is a hard prohibition, enforced at two points:
//!   [`PolicyContext::authorize`] rejects any command whose operation needs
//!   a forbidden capability before it is accepted (spec section 19's
//!   capability validation level), and [`ResourceBudget::denied`] carries
//!   the prohibition down to the adapters that enforce it at the kernel or
//!   provider boundary (a private network namespace for every spawned
//!   process under `WITHOUT network`; refused agent backends).
//! - `WITH <capability>` is a hard requirement: it is only accepted if the
//!   capability is actually available on this host right now, checked
//!   through the caller-supplied probe (see [`apply`]).
//! - `PREFER`/`DODGE` are soft and advisory. They are validated against the
//!   same vocabulary and can never contradict a hard constraint, but no
//!   operation Xact plans today has an alternative for them to choose
//!   between, and the CLI says so when one is accepted.
//!
//! A standalone `! WHEN <condition>` is rejected by the parser (there is no
//! condition language to enforce it with); the enforced dependency form is
//! the command-level `WHEN THIS/THAT SUCCEEDS|FAILS` clause, handled by
//! `xact-core`/`xact-cli`.

use std::collections::{HashMap, HashSet};

use xact_ast::{Capability, PolicyArgs, PolicyOperator, PolicyStatement, ResourceBudget, ResourceQuota};
use xact_diagnostics::Diagnostic;

#[derive(Debug, Clone, Default)]
pub struct PolicyContext {
    schedule: Option<PolicyOperator>,
    hard_with: HashSet<String>,
    hard_without: HashSet<String>,
    soft_prefer: HashSet<String>,
    soft_dodge: HashSet<String>,
    /// Which of SPEND/SAVE last claimed a given resource, to catch a
    /// contradictory `SPEND 10%RAM` followed by `SAVE 10%RAM`.
    quota_owner: HashMap<String, PolicyOperator>,
    quotas: Vec<(PolicyOperator, ResourceQuota)>,
}

impl PolicyContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schedule(&self) -> Option<PolicyOperator> {
        self.schedule
    }

    pub fn quotas(&self) -> &[(PolicyOperator, ResourceQuota)] {
        &self.quotas
    }

    /// Resolves the session's accumulated `SPEND`/`SAVE` statements into
    /// the actual cap `xact-resource` should enforce (spec section 22,
    /// Xact–Mesut Integration Phase 5 — "the Xact policy engine SHALL
    /// determine the semantic meaning of these policies"): `SPEND N%X`
    /// caps the workload at `N%`; `SAVE N%X` reserves `N%` for the system,
    /// capping the workload at `100 - N`%. Percentages are already known
    /// to be in `1..=100` (validated in `apply`), so `100 - N` never
    /// underflows. Later statements for the same resource win, matching
    /// `CONCURRENTLY`/`CONSECUTIVELY`'s "restating is fine" precedent —
    /// `apply` already rejects a same-resource SPEND/SAVE *conflict*, so
    /// this only ever resolves repeated agreement, never a contradiction.
    pub fn resource_budget(&self) -> ResourceBudget {
        let mut budget = ResourceBudget { denied: self.denied(), ..ResourceBudget::default() };
        for (operator, quota) in &self.quotas {
            let cap = match operator {
                PolicyOperator::Spend => quota.percent,
                PolicyOperator::Save => 100 - quota.percent,
                _ => unreachable!("only SPEND/SAVE quotas are ever pushed into `quotas`"),
            };
            match quota.resource.to_uppercase().as_str() {
                "CPU" => budget.cpu_percent = Some(cap),
                "RAM" => budget.ram_percent = Some(cap),
                other => {
                    let name = other.to_string();
                    if !budget.unenforceable.contains(&name) {
                        budget.unenforceable.push(name);
                    }
                }
            }
        }
        budget
    }

    /// Every capability `WITHOUT` currently forbids, in vocabulary order.
    pub fn denied(&self) -> Vec<Capability> {
        Capability::ALL.into_iter().filter(|c| self.hard_without.contains(c.as_str())).collect()
    }

    /// Spec section 19's capability validation level: `Err` names the first
    /// capability in `required` that a `WITHOUT` statement forbids.
    /// `operation` is only used to word the message.
    pub fn authorize(&self, operation: &str, required: &[Capability]) -> Result<(), String> {
        match required.iter().find(|c| self.hard_without.contains(c.as_str())) {
            Some(capability) => Err(format!(
                "{operation} needs '{cap}', which the active policy forbids (! WITHOUT {cap}).",
                cap = capability.as_str()
            )),
            None => Ok(()),
        }
    }

    /// Removes already-established singleton operators from a completion
    /// suggestion list (spec section 16). Stateless `xact-completion`
    /// cannot know this on its own — it has no session history.
    pub fn filter_suggestions(&self, suggestions: Vec<String>) -> Vec<String> {
        if self.schedule.is_none() {
            return suggestions;
        }
        suggestions
            .into_iter()
            .filter(|s| s != PolicyOperator::Concurrently.as_str() && s != PolicyOperator::Consecutively.as_str())
            .collect()
    }
}

/// Answers "is this capability actually available on this host right
/// now?" for `WITH`. Supplied by the caller (`xact-core`), because the
/// answer lives in the crates that own each capability (`xact-resource`
/// for network isolation, `xact-tell` for agent backends) and this crate
/// must not depend on them. `Err` carries the reason it isn't.
pub type CapabilityProbe<'a> = &'a dyn Fn(Capability) -> Result<(), String>;

pub fn apply(
    stmt: PolicyStatement,
    context: &mut PolicyContext,
    probe: CapabilityProbe<'_>,
) -> Result<PolicyStatement, Diagnostic> {
    match stmt.operator {
        PolicyOperator::Concurrently | PolicyOperator::Consecutively => {
            if let Some(existing) = context.schedule {
                if existing != stmt.operator {
                    return Err(Diagnostic::invalid(
                        format!(
                            "{} conflicts with {}, already established for this block.",
                            stmt.operator.as_str(),
                            existing.as_str()
                        ),
                        stmt.operator_span,
                        vec![],
                    ));
                }
            }
            context.schedule = Some(stmt.operator);
            Ok(stmt)
        }
        PolicyOperator::With => {
            let key = capability_key(&stmt.args);
            if context.hard_without.contains(&key) {
                return Err(same_capability_conflict(&stmt, PolicyOperator::Without));
            }
            if context.soft_dodge.contains(&key) {
                return Err(overrides_hard(&stmt, PolicyOperator::Dodge));
            }
            if let Some(capability) = Capability::parse(&key) {
                if let Err(reason) = probe(capability) {
                    return Err(Diagnostic::invalid(
                        format!("WITH {key} cannot be honoured on this host: {reason}"),
                        stmt.operator_span,
                        vec![],
                    ));
                }
            }
            context.hard_with.insert(key);
            Ok(stmt)
        }
        PolicyOperator::Without => {
            let key = capability_key(&stmt.args);
            if context.hard_with.contains(&key) {
                return Err(same_capability_conflict(&stmt, PolicyOperator::With));
            }
            if context.soft_prefer.contains(&key) {
                return Err(overrides_hard(&stmt, PolicyOperator::Prefer));
            }
            context.hard_without.insert(key);
            Ok(stmt)
        }
        PolicyOperator::Prefer => {
            let key = capability_key(&stmt.args);
            if context.hard_without.contains(&key) {
                return Err(overridden_by_hard(&stmt, PolicyOperator::Without));
            }
            if context.soft_dodge.contains(&key) {
                return Err(same_capability_conflict(&stmt, PolicyOperator::Dodge));
            }
            context.soft_prefer.insert(key);
            Ok(stmt)
        }
        PolicyOperator::Dodge => {
            let key = capability_key(&stmt.args);
            if context.hard_with.contains(&key) {
                return Err(overridden_by_hard(&stmt, PolicyOperator::With));
            }
            if context.soft_prefer.contains(&key) {
                return Err(same_capability_conflict(&stmt, PolicyOperator::Prefer));
            }
            context.soft_dodge.insert(key);
            Ok(stmt)
        }
        PolicyOperator::Spend | PolicyOperator::Save => {
            let PolicyArgs::Quotas(quotas) = &stmt.args else {
                unreachable!("parser guarantees SPEND/SAVE carries Quotas args")
            };
            for quota in quotas {
                if !(1..=100).contains(&quota.percent) {
                    return Err(Diagnostic::invalid(
                        format!(
                            "{} {}%{} is not a valid percentage — must be between 1 and 100.",
                            stmt.operator.as_str(),
                            quota.percent,
                            quota.resource
                        ),
                        quota.span,
                        vec![],
                    ));
                }
                if let Some(&owner) = context.quota_owner.get(&quota.resource) {
                    if owner != stmt.operator {
                        return Err(Diagnostic::invalid(
                            format!(
                                "{} {}% conflicts with {} already set for {}.",
                                stmt.operator.as_str(),
                                quota.percent,
                                owner.as_str(),
                                quota.resource
                            ),
                            quota.span,
                            vec![],
                        ));
                    }
                }
            }
            for quota in quotas {
                context.quota_owner.insert(quota.resource.clone(), stmt.operator);
                context.quotas.push((stmt.operator, quota.clone()));
            }
            Ok(stmt)
        }
        // The parser already rejects `! WHEN`; this is the same refusal for
        // a statement constructed some other way, so it can never be
        // recorded and then ignored.
        PolicyOperator::When => Err(Diagnostic::invalid(
            "! WHEN is not supported; attach WHEN THIS/THAT SUCCEEDS|FAILS to a command instead.",
            stmt.operator_span,
            vec![],
        )),
    }
}

fn capability_key(args: &PolicyArgs) -> String {
    let PolicyArgs::Capability { name, .. } = args else {
        unreachable!("parser guarantees WITH/WITHOUT/PREFER/DODGE carries Capability args")
    };
    name.to_lowercase()
}

fn same_capability_conflict(stmt: &PolicyStatement, other: PolicyOperator) -> Diagnostic {
    Diagnostic::invalid(
        format!(
            "{} contradicts {}, already established for the same capability in this block.",
            stmt.operator.as_str(),
            other.as_str()
        ),
        stmt.operator_span,
        vec![],
    )
}

/// A soft preference conflicting with an already-established hard
/// constraint on the opposite side (e.g. `PREFER 'x'` after `WITHOUT 'x'`).
fn overridden_by_hard(stmt: &PolicyStatement, hard: PolicyOperator) -> Diagnostic {
    Diagnostic::invalid(
        format!(
            "{} cannot override the hard constraint {} already established for this capability.",
            stmt.operator.as_str(),
            hard.as_str()
        ),
        stmt.operator_span,
        vec![],
    )
}

/// A new hard constraint that would retroactively invalidate an
/// already-accepted opposite soft preference.
fn overrides_hard(stmt: &PolicyStatement, soft: PolicyOperator) -> Diagnostic {
    Diagnostic::invalid(
        format!(
            "{} contradicts the already-established {} for this capability.",
            stmt.operator.as_str(),
            soft.as_str()
        ),
        stmt.operator_span,
        vec![],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use xact_ast::Span;

    fn apply_ok(stmt: PolicyStatement, ctx: &mut PolicyContext) -> Result<PolicyStatement, Diagnostic> {
        apply(stmt, ctx, &|_| Ok(()))
    }

    fn capability(op: PolicyOperator, name: &str) -> PolicyStatement {
        PolicyStatement {
            operator: op,
            operator_span: Span::default(),
            args: PolicyArgs::Capability { name: name.to_string(), span: Span::default() },
        }
    }

    fn schedule(op: PolicyOperator) -> PolicyStatement {
        PolicyStatement { operator: op, operator_span: Span::default(), args: PolicyArgs::None }
    }

    fn quotas(op: PolicyOperator, entries: &[(u32, &str)]) -> PolicyStatement {
        PolicyStatement {
            operator: op,
            operator_span: Span::default(),
            args: PolicyArgs::Quotas(
                entries
                    .iter()
                    .map(|(percent, resource)| ResourceQuota {
                        percent: *percent,
                        resource: resource.to_string(),
                        span: Span::default(),
                    })
                    .collect(),
            ),
        }
    }

    #[test]
    fn concurrently_then_consecutively_conflicts() {
        let mut ctx = PolicyContext::new();
        assert!(apply_ok(schedule(PolicyOperator::Concurrently), &mut ctx).is_ok());
        assert!(apply_ok(schedule(PolicyOperator::Consecutively), &mut ctx).is_err());
    }

    #[test]
    fn restating_same_schedule_is_fine() {
        let mut ctx = PolicyContext::new();
        assert!(apply_ok(schedule(PolicyOperator::Concurrently), &mut ctx).is_ok());
        assert!(apply_ok(schedule(PolicyOperator::Concurrently), &mut ctx).is_ok());
    }

    #[test]
    fn with_and_without_same_capability_conflicts() {
        let mut ctx = PolicyContext::new();
        assert!(apply_ok(capability(PolicyOperator::With, "network"), &mut ctx).is_ok());
        assert!(apply_ok(capability(PolicyOperator::Without, "network"), &mut ctx).is_err());
    }

    #[test]
    fn prefer_cannot_override_existing_without() {
        let mut ctx = PolicyContext::new();
        assert!(apply_ok(capability(PolicyOperator::Without, "agent-remote"), &mut ctx).is_ok());
        assert!(apply_ok(capability(PolicyOperator::Prefer, "agent-remote"), &mut ctx).is_err());
    }

    #[test]
    fn without_cannot_be_added_after_prefer_for_same_capability() {
        let mut ctx = PolicyContext::new();
        assert!(apply_ok(capability(PolicyOperator::Prefer, "agent-remote"), &mut ctx).is_ok());
        assert!(apply_ok(capability(PolicyOperator::Without, "agent-remote"), &mut ctx).is_err());
    }

    #[test]
    fn prefer_and_dodge_same_capability_conflicts() {
        let mut ctx = PolicyContext::new();
        assert!(apply_ok(capability(PolicyOperator::Prefer, "agent-remote"), &mut ctx).is_ok());
        assert!(apply_ok(capability(PolicyOperator::Dodge, "agent-remote"), &mut ctx).is_err());
    }

    #[test]
    fn independent_capabilities_do_not_conflict() {
        let mut ctx = PolicyContext::new();
        assert!(apply_ok(capability(PolicyOperator::With, "network"), &mut ctx).is_ok());
        assert!(apply_ok(capability(PolicyOperator::Without, "agent-remote"), &mut ctx).is_ok());
    }

    #[test]
    fn spend_then_save_same_resource_conflicts() {
        let mut ctx = PolicyContext::new();
        assert!(apply_ok(quotas(PolicyOperator::Spend, &[(10, "RAM")]), &mut ctx).is_ok());
        assert!(apply_ok(quotas(PolicyOperator::Save, &[(10, "RAM")]), &mut ctx).is_err());
    }

    #[test]
    fn filter_suggestions_drops_schedule_after_established() {
        let mut ctx = PolicyContext::new();
        apply_ok(schedule(PolicyOperator::Concurrently), &mut ctx).unwrap();
        let filtered = ctx.filter_suggestions(vec!["CONCURRENTLY".into(), "CONSECUTIVELY".into(), "WITH".into()]);
        assert_eq!(filtered, vec!["WITH".to_string()]);
    }

    #[test]
    fn spend_out_of_range_percent_rejected() {
        let mut ctx = PolicyContext::new();
        assert!(apply_ok(quotas(PolicyOperator::Spend, &[(0, "CPU")]), &mut ctx).is_err());
        assert!(apply_ok(quotas(PolicyOperator::Spend, &[(101, "CPU")]), &mut ctx).is_err());
        assert!(apply_ok(quotas(PolicyOperator::Spend, &[(150, "CPU")]), &mut ctx).is_err());
    }

    #[test]
    fn save_out_of_range_percent_rejected() {
        let mut ctx = PolicyContext::new();
        assert!(apply_ok(quotas(PolicyOperator::Save, &[(0, "RAM")]), &mut ctx).is_err());
        assert!(apply_ok(quotas(PolicyOperator::Save, &[(200, "RAM")]), &mut ctx).is_err());
    }

    #[test]
    fn resource_budget_translates_spend_directly_and_save_as_the_complement() {
        let mut ctx = PolicyContext::new();
        apply_ok(quotas(PolicyOperator::Spend, &[(40, "CPU")]), &mut ctx).unwrap();
        apply_ok(quotas(PolicyOperator::Save, &[(10, "RAM")]), &mut ctx).unwrap();

        let budget = ctx.resource_budget();

        assert_eq!(budget.cpu_percent, Some(40));
        assert_eq!(budget.ram_percent, Some(90));
        assert!(budget.unenforceable.is_empty());
    }

    #[test]
    fn resource_budget_reports_unrecognized_resources_instead_of_dropping_them() {
        let mut ctx = PolicyContext::new();
        apply_ok(quotas(PolicyOperator::Spend, &[(50, "GPU")]), &mut ctx).unwrap();

        let budget = ctx.resource_budget();

        assert_eq!(budget.cpu_percent, None);
        assert_eq!(budget.unenforceable, vec!["GPU".to_string()]);
    }

    #[test]
    fn resource_budget_is_empty_with_no_quotas_stated() {
        let ctx = PolicyContext::new();
        assert!(ctx.resource_budget().is_empty());
    }

    #[test]
    fn without_forbids_exactly_the_named_capability() {
        let mut ctx = PolicyContext::new();
        apply_ok(capability(PolicyOperator::Without, "network"), &mut ctx).unwrap();
        assert_eq!(ctx.denied(), vec![Capability::Network]);
        assert_eq!(ctx.resource_budget().denied, vec![Capability::Network]);
        assert!(ctx.authorize("RUN", &[Capability::ProcessSpawn]).is_ok());
        let err = ctx.authorize("TELL", &[Capability::ProcessSpawn, Capability::Network]).unwrap_err();
        assert!(err.contains("WITHOUT network"), "{err}");
    }

    #[test]
    fn with_is_refused_when_the_probe_says_unavailable() {
        let mut ctx = PolicyContext::new();
        let unavailable = |_: Capability| Err("no default route".to_string());
        let err = apply(capability(PolicyOperator::With, "network"), &mut ctx, &unavailable).unwrap_err();
        assert!(err.message.contains("no default route"));
        // Nothing was recorded: a refused WITH does not linger.
        assert!(apply_ok(capability(PolicyOperator::Without, "network"), &mut ctx).is_ok());
    }

    #[test]
    fn standalone_when_is_never_recorded() {
        let mut ctx = PolicyContext::new();
        let stmt = PolicyStatement {
            operator: PolicyOperator::When,
            operator_span: Span::default(),
            args: PolicyArgs::Condition { text: "it rains".into(), span: Span::default() },
        };
        assert!(apply_ok(stmt, &mut ctx).is_err());
    }
}
