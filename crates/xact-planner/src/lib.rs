//! Execution planner (spec section 20): `Command -> ExecutionPlan`, choosing
//! native vs ELci-tool-backed execution (section 21).
//!
//! This is where `Operand`s get resolved into concrete values — expanding
//! `THIS`/`THAT` against the [`ReferenceContext`] and `~` against `$HOME` —
//! completing the `ObjectRef -> ResolvedObject -> TypedResource` pipeline
//! spec section 11 describes (a resolved path is as far as "typed" goes so
//! far; there is no filesystem-vs-process distinction yet).
//!
//! Scope so far: only `CREATE` has a real plan, mapping to the `bank` tool
//! per section 21's table (`CREATE` is the language-level intent; `bank`
//! is the execution backend the planner happens to choose for it — spec
//! section 6/13's Xact-owns-intent, tool-owns-implementation split).
//! Every other verb, and identity declarations, are reported as
//! [`PlanOutcome::Unsupported`] — not silently dropped. Scheduling
//! (section 23) and resource policy (section 22) are not consulted yet;
//! that needs a real multi-command block model.

use std::path::PathBuf;

use xact_ast::{Command, Operand};
use xact_reference::{ReferenceContext, ResolvedObject};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionPlan {
    /// `CREATE` -> the `bank` tool (spec section 21).
    Bank { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOutcome {
    Plan(ExecutionPlan),
    /// Not yet implemented — the reason names what's missing, not "no-op".
    Unsupported(String),
}

pub fn plan(command: &Command, references: &ReferenceContext) -> PlanOutcome {
    let Command::Imperative(cmd) = command else {
        return PlanOutcome::Unsupported("THEY declarations establish identity but have no execution plan.".into());
    };

    match cmd.verb {
        xact_ast::Verb::Create => {
            let Some(operand) = &cmd.operand else {
                return PlanOutcome::Unsupported("CREATE requires a target.".into());
            };
            match resolve_path(operand, references) {
                Some(path) => PlanOutcome::Plan(ExecutionPlan::Bank {
                    path: expand_tilde(&path),
                }),
                None => PlanOutcome::Unsupported("CREATE's target did not resolve to a path.".into()),
            }
        }
        other => PlanOutcome::Unsupported(format!("{} has no execution plan yet.", other.as_str())),
    }
}

fn resolve_path(operand: &Operand, references: &ReferenceContext) -> Option<String> {
    match operand {
        Operand::Owned { path, .. } => Some(path.clone()),
        Operand::Reference { kind, .. } => references.resolve(*kind).map(|r| match r {
            ResolvedObject::Path(p) => p.clone(),
            ResolvedObject::Text(t) => t.clone(),
        }),
        Operand::StringArg { value, .. } => Some(value.clone()),
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xact_ast::{ImperativeCommand, OwnershipKind, ReferenceKind, Span, Verb};

    fn create_command(operand: Operand) -> Command {
        Command::Imperative(ImperativeCommand {
            verb: Verb::Create,
            verb_span: Span::default(),
            operand: Some(operand),
            destination: None,
        })
    }

    #[test]
    fn create_with_owned_path_expands_tilde() {
        std::env::set_var("HOME", "/home/testuser");
        let references = ReferenceContext::new();
        let cmd = create_command(Operand::Owned {
            kind: OwnershipKind::My,
            path: "~/project/src/main.rs".into(),
            span: Span::default(),
        });
        match plan(&cmd, &references) {
            PlanOutcome::Plan(ExecutionPlan::Bank { path }) => {
                assert_eq!(path, PathBuf::from("/home/testuser/project/src/main.rs"));
            }
            other => panic!("expected a bank plan, got {other:?}"),
        }
    }

    #[test]
    fn create_with_absolute_path_is_unchanged() {
        let references = ReferenceContext::new();
        let cmd = create_command(Operand::Owned {
            kind: OwnershipKind::My,
            path: "/tmp/project/".into(),
            span: Span::default(),
        });
        match plan(&cmd, &references) {
            PlanOutcome::Plan(ExecutionPlan::Bank { path }) => {
                assert_eq!(path, PathBuf::from("/tmp/project/"));
            }
            other => panic!("expected a bank plan, got {other:?}"),
        }
    }

    #[test]
    fn create_with_reference_resolves_from_context() {
        let mut references = ReferenceContext::new();
        references.record(ResolvedObject::Path("/tmp/from-reference".into()));
        let cmd = create_command(Operand::Reference {
            kind: ReferenceKind::That,
            span: Span::default(),
        });
        match plan(&cmd, &references) {
            PlanOutcome::Plan(ExecutionPlan::Bank { path }) => {
                assert_eq!(path, PathBuf::from("/tmp/from-reference"));
            }
            other => panic!("expected a bank plan, got {other:?}"),
        }
    }

    #[test]
    fn other_verbs_are_unsupported_not_silently_dropped() {
        let references = ReferenceContext::new();
        let cmd = Command::Imperative(ImperativeCommand {
            verb: Verb::Run,
            verb_span: Span::default(),
            operand: Some(Operand::StringArg {
                value: "chrome".into(),
                span: Span::default(),
            }),
            destination: None,
        });
        match plan(&cmd, &references) {
            PlanOutcome::Unsupported(reason) => assert!(reason.contains("RUN")),
            other => panic!("expected unsupported, got {other:?}"),
        }
    }
}
