//! Execution planner (spec section 20): `Command -> ExecutionPlan`, choosing
//! native vs ELci-tool-backed execution (section 21).
//!
//! This is where `Operand`s get resolved into concrete values — expanding
//! `THIS`/`THAT` against the [`ReferenceContext`] and `~` against `$HOME` —
//! completing the `ObjectRef -> ResolvedObject -> TypedResource` pipeline
//! spec section 11 describes (a resolved path is as far as "typed" goes so
//! far; there is no filesystem-vs-process distinction yet).
//!
//! Scope so far: `CREATE` maps to the `bank` tool, `SEE` maps to `gls`
//! (directories) or `bat` (files), and `RUN` maps to native process
//! execution — per section 21's table (the verb is the language-level
//! intent; the tool, or "no tool, do it natively", is the execution
//! backend the planner happens to choose for it — spec section 6/13's
//! Xact-owns-intent, tool-owns-implementation split). Unlike `CREATE`,
//! where one tool handles both files and directories internally, `SEE`
//! has no single tool covering both — so the planner itself inspects the
//! resolved path on disk to route to one or the other. Every other verb,
//! and identity declarations, are reported as [`PlanOutcome::Unsupported`]
//! — not silently dropped. Scheduling (section 23) and resource policy
//! (section 22) are not consulted yet; that needs a real multi-command
//! block model.

use std::path::PathBuf;

use xact_ast::{Command, Operand};
use xact_reference::{ReferenceContext, ResolvedObject};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionPlan {
    /// `CREATE` -> the `bank` tool (spec section 21).
    Bank { path: PathBuf },
    /// `SEE` on a directory -> the `gls` tool.
    ViewDirectory { path: PathBuf },
    /// `SEE` on a file -> the `bat` tool.
    ViewFile { path: PathBuf },
    /// `RUN` -> native process execution (spec section 21: no ELci tool
    /// owns this).
    Run { command_line: String },
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
        xact_ast::Verb::See => {
            let Some(operand) = &cmd.operand else {
                return PlanOutcome::Unsupported("SEE requires a target.".into());
            };
            match resolve_path(operand, references) {
                Some(raw) => {
                    let path = expand_tilde(&raw);
                    if path.is_dir() {
                        PlanOutcome::Plan(ExecutionPlan::ViewDirectory { path })
                    } else if path.is_file() {
                        PlanOutcome::Plan(ExecutionPlan::ViewFile { path })
                    } else {
                        PlanOutcome::Unsupported(format!(
                            "'{}' does not exist or is neither a file nor a directory.",
                            path.display()
                        ))
                    }
                }
                None => PlanOutcome::Unsupported("SEE's target did not resolve to a path.".into()),
            }
        }
        xact_ast::Verb::Run => {
            let Some(operand) = &cmd.operand else {
                return PlanOutcome::Unsupported("RUN requires a program to run.".into());
            };
            match resolve_path(operand, references) {
                Some(command_line) => PlanOutcome::Plan(ExecutionPlan::Run { command_line }),
                None => PlanOutcome::Unsupported("RUN's target did not resolve to a command.".into()),
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
            verb: Verb::Delete,
            verb_span: Span::default(),
            operand: Some(Operand::Owned {
                kind: OwnershipKind::My,
                path: "~/x".into(),
                span: Span::default(),
            }),
            destination: None,
        });
        match plan(&cmd, &references) {
            PlanOutcome::Unsupported(reason) => assert!(reason.contains("DELETE")),
            other => panic!("expected unsupported, got {other:?}"),
        }
    }

    #[test]
    fn run_resolves_string_operand_to_a_command_line() {
        let references = ReferenceContext::new();
        let cmd = Command::Imperative(ImperativeCommand {
            verb: Verb::Run,
            verb_span: Span::default(),
            operand: Some(Operand::StringArg {
                value: "true".into(),
                span: Span::default(),
            }),
            destination: None,
        });
        match plan(&cmd, &references) {
            PlanOutcome::Plan(ExecutionPlan::Run { command_line }) => assert_eq!(command_line, "true"),
            other => panic!("expected a run plan, got {other:?}"),
        }
    }

    #[test]
    fn run_resolves_reference_operand_to_a_command_line() {
        let mut references = ReferenceContext::new();
        references.record(ResolvedObject::Text("echo hi".into()));
        let cmd = Command::Imperative(ImperativeCommand {
            verb: Verb::Run,
            verb_span: Span::default(),
            operand: Some(Operand::Reference {
                kind: ReferenceKind::That,
                span: Span::default(),
            }),
            destination: None,
        });
        match plan(&cmd, &references) {
            PlanOutcome::Plan(ExecutionPlan::Run { command_line }) => assert_eq!(command_line, "echo hi"),
            other => panic!("expected a run plan, got {other:?}"),
        }
    }

    fn see_command(operand: Operand) -> Command {
        Command::Imperative(ImperativeCommand {
            verb: Verb::See,
            verb_span: Span::default(),
            operand: Some(operand),
            destination: None,
        })
    }

    #[test]
    fn see_on_a_directory_routes_to_gls() {
        let references = ReferenceContext::new();
        let cmd = see_command(Operand::Owned {
            kind: OwnershipKind::My,
            path: std::env::temp_dir().display().to_string(),
            span: Span::default(),
        });
        match plan(&cmd, &references) {
            PlanOutcome::Plan(ExecutionPlan::ViewDirectory { path }) => {
                assert_eq!(path, std::env::temp_dir());
            }
            other => panic!("expected a view-directory plan, got {other:?}"),
        }
    }

    #[test]
    fn see_on_a_file_routes_to_bat() {
        let file = std::env::temp_dir().join(format!("xact-planner-see-test-{}", std::process::id()));
        std::fs::write(&file, "hello").unwrap();

        let references = ReferenceContext::new();
        let cmd = see_command(Operand::Owned {
            kind: OwnershipKind::My,
            path: file.display().to_string(),
            span: Span::default(),
        });
        let result = plan(&cmd, &references);

        let _ = std::fs::remove_file(&file);
        match result {
            PlanOutcome::Plan(ExecutionPlan::ViewFile { path }) => assert_eq!(path, file),
            other => panic!("expected a view-file plan, got {other:?}"),
        }
    }

    #[test]
    fn see_on_a_nonexistent_path_is_unsupported() {
        let references = ReferenceContext::new();
        let cmd = see_command(Operand::Owned {
            kind: OwnershipKind::My,
            path: "/definitely/does/not/exist/xact-test".into(),
            span: Span::default(),
        });
        match plan(&cmd, &references) {
            PlanOutcome::Unsupported(reason) => assert!(reason.contains("does not exist")),
            other => panic!("expected unsupported, got {other:?}"),
        }
    }
}
