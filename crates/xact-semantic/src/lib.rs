//! Semantic validation: ownership and reference resolution (spec sections
//! 10, 11, 19). A grammatically complete [`xact_ast::Command`] can still be
//! semantically invalid — e.g. `THEIR` used before any identity is
//! established, `OUR` used with no shared root configured, a `THEY` member
//! who is not a real user, or `THAT` used before anything has been
//! established to refer to.
//!
//! Owned filesystem paths are resolved here, through the same
//! `xact_resolve::resolve_owned_path` the planner executes with, so an
//! ownership expression that cannot resolve is rejected before it is
//! accepted rather than failing at execution time. The objects recorded for
//! `THIS`/`THAT` are those resolved absolute paths, so a later reference
//! means exactly the location the original command used.
//!
//! Enforces two of the spec's testing invariants (section 27):
//! - **Ownership invariant**: no unresolved ownership expression reaches execution.
//! - **Reference invariant**: no unresolved `THIS`/`THAT` reference reaches execution.

use std::path::PathBuf;

use xact_ast::{Command, IdentityDeclaration, ImperativeCommand, Operand, OwnershipKind, Verb};
use xact_diagnostics::Diagnostic;
use xact_reference::{ReferenceContext, ResolvedObject};
use xact_resolve::OwnerRoots;

/// The ownership context owned paths resolve against: the users `THEIR`
/// refers to, established by `£ THEY are "alice"` (spec section 10), and
/// the shared root `OUR` refers to (`XACT_OUR_ROOT`).
#[derive(Debug, Clone, Default)]
pub struct IdentityContext {
    roots: OwnerRoots,
}

impl IdentityContext {
    /// No identity declared and no `OUR` root.
    pub fn new() -> Self {
        Self::default()
    }

    /// No identity declared; `OUR` rooted at `XACT_OUR_ROOT` if it is set.
    pub fn from_env() -> Self {
        Self::new().with_our_root(xact_resolve::our_root_from_env())
    }

    pub fn with_our_root(mut self, our: Option<PathBuf>) -> Self {
        self.roots.our = our;
        self
    }

    pub fn established(&self) -> bool {
        !self.roots.their.is_empty()
    }

    /// Replaces the declared `THEY` members with already-resolved
    /// `(user, home)` pairs (see [`ValidatedCommand::identity`]).
    pub fn set(&mut self, members: Vec<(String, PathBuf)>) {
        self.roots.their = members;
    }

    pub fn members(&self) -> Vec<String> {
        self.roots.their.iter().map(|(name, _)| name.clone()).collect()
    }

    pub fn roots(&self) -> &OwnerRoots {
        &self.roots
    }
}

/// What a command consumed and produced, for `THIS`/`THAT` respectively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Established {
    pub input: ResolvedObject,
    pub result: ResolvedObject,
}

/// A [`Command`] that has passed ownership and reference validation, paired
/// with the object it establishes (if any) for `xact-reference` to record.
#[derive(Debug, Clone)]
pub struct ValidatedCommand {
    pub command: Command,
    pub establishes: Option<Established>,
    /// For `£ THEY are "..."`: every member with their real home directory.
    pub identity: Option<Vec<(String, PathBuf)>>,
}

pub fn validate(
    command: Command,
    identity: &IdentityContext,
    references: &ReferenceContext,
) -> Result<ValidatedCommand, Vec<Diagnostic>> {
    match &command {
        Command::Identity(decl) => {
            let members = resolve_members(decl)?;
            Ok(ValidatedCommand { command, establishes: None, identity: Some(members) })
        }
        Command::Imperative(cmd) => {
            let mut diagnostics = Vec::new();
            check_destination(cmd, &mut diagnostics);
            let filesystem = cmd.verb != Verb::Run;
            check_operand(cmd.operand.as_ref(), filesystem, identity, references, &mut diagnostics);
            check_operand(cmd.destination.as_ref(), true, identity, references, &mut diagnostics);
            if let Some(dependency) = &cmd.dependency {
                if references.resolve(dependency.reference).is_none() {
                    diagnostics.push(Diagnostic::invalid(
                        format!(
                            "{} does not refer to anything yet, so WHEN {} {} has nothing to depend on.",
                            dependency.reference.as_str(),
                            dependency.reference.as_str(),
                            dependency.condition.as_str()
                        ),
                        dependency.reference_span,
                        vec![],
                    ));
                }
            }
            if !diagnostics.is_empty() {
                return Err(diagnostics);
            }
            let establishes = resolve_established(cmd, identity, references);
            Ok(ValidatedCommand { command, establishes, identity: None })
        }
    }
}

/// `COPY`/`MOVE` cannot mean anything without a destination, and a verb
/// that takes none must not silently ignore one (it would also make `THAT`
/// name an object the command never touched).
fn check_destination(cmd: &ImperativeCommand, diagnostics: &mut Vec<Diagnostic>) {
    let verb = cmd.verb.as_str();
    match (&cmd.destination, cmd.verb) {
        (None, Verb::Copy | Verb::Move) => diagnostics.push(Diagnostic::invalid(
            format!("{verb} requires a destination introduced by 'to'."),
            cmd.verb_span,
            vec!["to".into()],
        )),
        (Some(destination), _) if !cmd.verb.accepts_destination() => diagnostics.push(Diagnostic::invalid(
            format!("{verb} does not take a destination."),
            destination.span(),
            vec![],
        )),
        _ => {}
    }
}

/// Every `THEY` member must be a real user on this system; `THEIR` paths
/// resolve under that user's actual home directory.
fn resolve_members(decl: &IdentityDeclaration) -> Result<Vec<(String, PathBuf)>, Vec<Diagnostic>> {
    let mut members = Vec::new();
    let mut diagnostics = Vec::new();
    for name in &decl.members {
        match xact_resolve::user_home(name) {
            Some(home) => members.push((name.clone(), home)),
            None => diagnostics.push(Diagnostic::invalid(
                format!("'{name}' is not a user on this system, so THEIR cannot resolve to their files."),
                decl.span,
                vec![],
            )),
        }
    }
    if diagnostics.is_empty() {
        Ok(members)
    } else {
        Err(diagnostics)
    }
}

/// Checks a single operand's ownership and reference validity. `filesystem`
/// says whether the operand names a location (every verb but `RUN`, whose
/// operand names a program); only then is an owned path resolved. Exposed
/// for reuse by other language surfaces built on the same `Operand` type —
/// e.g. `xact-agent`'s `READING`/`POPULATING` clauses (spec section 9).
pub fn check_operand(
    operand: Option<&Operand>,
    filesystem: bool,
    identity: &IdentityContext,
    references: &ReferenceContext,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(operand) = operand else { return };
    match operand {
        Operand::Owned { kind, path, span } => {
            if filesystem {
                if let Err(reason) = xact_resolve::resolve_owned_path(path, *kind, identity.roots()) {
                    diagnostics.push(Diagnostic::invalid(reason, *span, vec![]));
                }
            } else if *kind == OwnershipKind::Their && !identity.established() {
                diagnostics.push(Diagnostic::invalid(
                    "THEIR cannot be used before an identity is established (e.g. £ THEY are \"alice\").",
                    *span,
                    vec![],
                ));
            }
        }
        Operand::Reference { kind, span } => {
            if references.resolve(*kind).is_none() {
                diagnostics.push(Diagnostic::invalid(
                    format!("{} does not refer to anything yet.", kind.as_str()),
                    *span,
                    vec![],
                ));
            }
        }
        Operand::StringArg { .. } => {}
    }
}

/// The object an operand denotes once resolved: an owned filesystem path
/// becomes its real absolute location.
fn resolve_object(
    operand: &Operand,
    filesystem: bool,
    identity: &IdentityContext,
    references: &ReferenceContext,
) -> Option<ResolvedObject> {
    match operand {
        Operand::Owned { kind, path, .. } if filesystem => {
            xact_resolve::resolve_owned_path(path, *kind, identity.roots())
                .ok()
                .map(|resolved| ResolvedObject::Path(resolved.to_string_lossy().into_owned()))
        }
        Operand::Owned { path, .. } => Some(ResolvedObject::Path(path.clone())),
        Operand::Reference { kind, .. } => references.resolve(*kind).cloned(),
        Operand::StringArg { value, .. } => Some(ResolvedObject::Text(value.clone())),
    }
}

/// What this command establishes for a later `THIS`/`THAT`: its operand is
/// the input, its destination (or, with none, the operand again) the result.
fn resolve_established(
    cmd: &ImperativeCommand,
    identity: &IdentityContext,
    references: &ReferenceContext,
) -> Option<Established> {
    let filesystem = cmd.verb != Verb::Run;
    let input = cmd.operand.as_ref().and_then(|op| resolve_object(op, filesystem, identity, references));
    let result = cmd.destination.as_ref().and_then(|op| resolve_object(op, true, identity, references));
    match (input, result) {
        (Some(input), Some(result)) => Some(Established { input, result }),
        (Some(only), None) | (None, Some(only)) => Some(Established { input: only.clone(), result: only }),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xact_ast::{Span, Verb};

    fn imperative(operand: Option<Operand>) -> Command {
        Command::Imperative(ImperativeCommand {
            verb: Verb::See,
            verb_span: Span::default(),
            operand,
            destination: None,
            dependency: None,
        })
    }

    #[test]
    fn their_rejected_without_identity() {
        let identity = IdentityContext::new();
        let references = ReferenceContext::new();
        let cmd =
            imperative(Some(Operand::Owned { kind: OwnershipKind::Their, path: "~/x".into(), span: Span::default() }));
        let result = validate(cmd, &identity, &references);
        assert!(result.is_err());
    }

    #[test]
    fn their_accepted_after_identity_established() {
        let mut identity = IdentityContext::new();
        identity.set(vec![("alice".into(), PathBuf::from("/home/alice"))]);
        let references = ReferenceContext::new();
        let cmd =
            imperative(Some(Operand::Owned { kind: OwnershipKind::Their, path: "~/x".into(), span: Span::default() }));
        assert!(validate(cmd, &identity, &references).is_ok());
    }

    #[test]
    fn unresolved_that_rejected() {
        let identity = IdentityContext::new();
        let references = ReferenceContext::new();
        let cmd = imperative(Some(Operand::Reference { kind: xact_ast::ReferenceKind::That, span: Span::default() }));
        assert!(validate(cmd, &identity, &references).is_err());
    }

    #[test]
    fn resolved_that_accepted_and_records_establishment() {
        let identity = IdentityContext::new();
        let mut references = ReferenceContext::new();
        let readme = ResolvedObject::Path("/home/u/project/README.md".into());
        references.record(readme.clone(), readme.clone());
        let cmd = imperative(Some(Operand::Reference { kind: xact_ast::ReferenceKind::That, span: Span::default() }));
        let validated = validate(cmd, &identity, &references).expect("should validate");
        assert_eq!(validated.establishes, Some(Established { input: readme.clone(), result: readme }));
    }

    fn dependency_command(reference: xact_ast::ReferenceKind, condition: xact_ast::SuccessCondition) -> Command {
        Command::Imperative(ImperativeCommand {
            verb: Verb::Run,
            verb_span: Span::default(),
            operand: Some(Operand::StringArg { value: "test".into(), span: Span::default() }),
            destination: None,
            dependency: Some(xact_ast::DependencyClause {
                reference,
                reference_span: Span::default(),
                condition,
                condition_span: Span::default(),
            }),
        })
    }

    #[test]
    fn when_that_rejected_with_no_prior_result() {
        let identity = IdentityContext::new();
        let references = ReferenceContext::new();
        let cmd = dependency_command(xact_ast::ReferenceKind::That, xact_ast::SuccessCondition::Succeeds);
        assert!(validate(cmd, &identity, &references).is_err());
    }

    #[test]
    fn when_that_accepted_once_something_is_established() {
        let identity = IdentityContext::new();
        let mut references = ReferenceContext::new();
        references.record(ResolvedObject::Text("compile".into()), ResolvedObject::Text("compile".into()));
        let cmd = dependency_command(xact_ast::ReferenceKind::That, xact_ast::SuccessCondition::Fails);
        assert!(validate(cmd, &identity, &references).is_ok());
    }

    fn owned(kind: OwnershipKind, path: &str) -> Operand {
        Operand::Owned { kind, path: path.into(), span: Span::default() }
    }

    #[test]
    fn copy_records_its_source_as_this_and_its_destination_as_that() {
        let identity = IdentityContext::new().with_our_root(Some(PathBuf::from("/srv/team")));
        let cmd = Command::Imperative(ImperativeCommand {
            verb: Verb::Copy,
            verb_span: Span::default(),
            operand: Some(owned(OwnershipKind::My, "/tmp/a")),
            destination: Some(owned(OwnershipKind::Our, "~/backup")),
            dependency: None,
        });
        let validated = validate(cmd, &identity, &ReferenceContext::new()).unwrap();
        assert_eq!(
            validated.establishes,
            Some(Established {
                input: ResolvedObject::Path("/tmp/a".into()),
                result: ResolvedObject::Path("/srv/team/backup".into()),
            })
        );
    }

    #[test]
    fn our_path_without_a_shared_root_is_rejected() {
        let cmd = imperative(Some(owned(OwnershipKind::Our, "~/backup")));
        let diagnostics = validate(cmd, &IdentityContext::new(), &ReferenceContext::new()).unwrap_err();
        assert!(diagnostics[0].message.contains("XACT_OUR_ROOT"));
    }

    #[test]
    fn their_path_with_several_users_is_rejected() {
        let mut identity = IdentityContext::new();
        identity.set(vec![("alice".into(), PathBuf::from("/home/alice")), ("bob".into(), PathBuf::from("/home/bob"))]);
        let cmd = imperative(Some(owned(OwnershipKind::Their, "~/x")));
        assert!(validate(cmd, &identity, &ReferenceContext::new()).is_err());
    }

    #[test]
    fn they_members_must_be_real_users() {
        let decl = |members: &[&str]| {
            Command::Identity(IdentityDeclaration {
                members: members.iter().map(|m| m.to_string()).collect(),
                span: Span::default(),
            })
        };
        let validated = validate(decl(&["root"]), &IdentityContext::new(), &ReferenceContext::new()).unwrap();
        assert_eq!(validated.identity, Some(vec![("root".to_string(), PathBuf::from("/root"))]));
        assert!(validate(decl(&["xact-no-such-user-for-tests"]), &IdentityContext::new(), &ReferenceContext::new())
            .is_err());
    }

    #[test]
    fn copy_without_a_destination_is_rejected_and_see_with_one_too() {
        let copy = Command::Imperative(ImperativeCommand {
            verb: Verb::Copy,
            verb_span: Span::default(),
            operand: Some(owned(OwnershipKind::My, "/tmp/a")),
            destination: None,
            dependency: None,
        });
        assert!(validate(copy, &IdentityContext::new(), &ReferenceContext::new()).is_err());
        let see = Command::Imperative(ImperativeCommand {
            verb: Verb::See,
            verb_span: Span::default(),
            operand: Some(owned(OwnershipKind::My, "/tmp/a")),
            destination: Some(owned(OwnershipKind::My, "/tmp/b")),
            dependency: None,
        });
        assert!(validate(see, &IdentityContext::new(), &ReferenceContext::new()).is_err());
    }
}
