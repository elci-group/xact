//! Grammatical parsing of Xact input lines into a typed AST.
//!
//! The parser is the single source of truth for "what can legally come
//! next" (spec section 15): both diagnostics (section 24) and completion
//! (`xact-completion`) read the `expected` list off a [`ParseOutcome`]
//! rather than maintaining a separate grammar.
//!
//! Scope so far: the `£` imperative language (verbs, ownership, references),
//! the `THEY are "..."` identity declaration, and the `!` policy language
//! (spec section 8). `@` agent blocks are recognised only far enough to
//! report "not supported yet".

use xact_ast::{
    Command, IdentityDeclaration, ImperativeCommand, Line, Operand, OwnershipKind, PolicyArgKind, PolicyArgs,
    PolicyOperator, PolicyStatement, ReferenceKind, ResourceQuota, Span, Verb,
};
use xact_diagnostics::Diagnostic;
use xact_lexer::{tokenize, Token, TokenKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    /// A grammatically complete, executable line (still subject to
    /// semantic/ownership/reference/policy validation downstream).
    Complete(Line),
    /// A valid prefix that needs more input (spec section 18: an
    /// incomplete draft, not yet an executable program).
    Incomplete(Diagnostic),
    /// Cannot be completed into a valid program without changing what has
    /// already been typed.
    Invalid(Diagnostic),
}

pub fn parse_line(input: &str) -> ParseOutcome {
    parse_tokens(&tokenize(input))
}

fn top_level_expected() -> Vec<String> {
    std::iter::once("!".to_string())
        .chain(Verb::ALL.iter().map(|v| v.as_str().to_string()))
        .chain(std::iter::once("THEY".to_string()))
        .collect()
}

fn operand_expected() -> Vec<String> {
    OwnershipKind::ALL
        .iter()
        .map(|k| k.as_str().to_string())
        .chain(ReferenceKind::ALL.iter().map(|k| k.as_str().to_string()))
        .chain(std::iter::once("'...'".to_string()))
        .collect()
}

fn policy_operator_expected() -> Vec<String> {
    PolicyOperator::ALL.iter().map(|op| op.as_str().to_string()).collect()
}

pub fn parse_tokens(tokens: &[Token]) -> ParseOutcome {
    let first = &tokens[0];
    match &first.kind {
        TokenKind::Eof => Diagnostic::incomplete("Expected a command.", first.span, vec!["£".into()]).into_incomplete(),
        TokenKind::Pound => parse_after_pound(tokens, 1),
        TokenKind::Bang => parse_policy(tokens, 1),
        TokenKind::At => {
            Diagnostic::invalid("Agent blocks (@) are not supported yet.", first.span, vec!["£".into()]).into_invalid()
        }
        TokenKind::Word(_) | TokenKind::StringLit(_) => {
            Diagnostic::invalid("Commands must begin with £ or !.", first.span, vec!["£".into(), "!".into()])
                .into_invalid()
        }
    }
}

fn parse_after_pound(tokens: &[Token], idx: usize) -> ParseOutcome {
    let tok = &tokens[idx];
    match &tok.kind {
        TokenKind::Eof => Diagnostic::incomplete("£ requires a command.", tok.span, top_level_expected()).into_incomplete(),
        TokenKind::Word(w) if w.eq_ignore_ascii_case("THEY") => parse_identity(tokens, idx, tok.span),
        TokenKind::Word(w) => match Verb::from_str(&w.to_uppercase()) {
            Some(verb) => parse_imperative(tokens, idx + 1, verb, tok.span),
            None => Diagnostic::invalid(format!("Unknown command '{w}'."), tok.span, top_level_expected()).into_invalid(),
        },
        TokenKind::StringLit(_) => {
            Diagnostic::invalid("Expected a command after £.", tok.span, top_level_expected()).into_invalid()
        }
        TokenKind::Bang | TokenKind::At | TokenKind::Pound => {
            Diagnostic::invalid("Expected a command after £.", tok.span, top_level_expected()).into_invalid()
        }
    }
}

fn parse_identity(tokens: &[Token], they_idx: usize, they_span: Span) -> ParseOutcome {
    let are_idx = they_idx + 1;
    let are_tok = &tokens[are_idx];
    match &are_tok.kind {
        TokenKind::Eof => {
            Diagnostic::incomplete("THEY requires 'are' and a member list.", are_tok.span, vec!["are".into()])
                .into_incomplete()
        }
        TokenKind::Word(w) if w.eq_ignore_ascii_case("are") => {
            let list_idx = are_idx + 1;
            let list_tok = &tokens[list_idx];
            match &list_tok.kind {
                TokenKind::Eof => Diagnostic::incomplete(
                    "Expected a quoted member list after 'are'.",
                    list_tok.span,
                    vec!["'...'".into()],
                )
                .into_incomplete(),
                TokenKind::StringLit(s) => {
                    let after = &tokens[list_idx + 1];
                    if !matches!(after.kind, TokenKind::Eof) {
                        return Diagnostic::invalid("Unexpected input after identity declaration.", after.span, vec![])
                            .into_invalid();
                    }
                    let members: Vec<String> = s
                        .split(',')
                        .map(|m| m.trim().to_string())
                        .filter(|m| !m.is_empty())
                        .collect();
                    if members.is_empty() {
                        return Diagnostic::invalid("Identity declaration requires at least one member.", list_tok.span, vec![])
                            .into_invalid();
                    }
                    ParseOutcome::Complete(Line::Command(Command::Identity(IdentityDeclaration {
                        members,
                        span: Span::new(they_span.start, list_tok.span.end),
                    })))
                }
                _ => Diagnostic::invalid(
                    "Expected a quoted member list (e.g. \"alice,bob\") after 'are'.",
                    list_tok.span,
                    vec!["'...'".into()],
                )
                .into_invalid(),
            }
        }
        _ => Diagnostic::invalid("Expected 'are' after THEY.", are_tok.span, vec!["are".into()]).into_invalid(),
    }
}

fn parse_imperative(tokens: &[Token], idx: usize, verb: Verb, verb_span: Span) -> ParseOutcome {
    let (operand, next_idx) = match parse_operand(tokens, idx, verb.as_str()) {
        Ok(pair) => pair,
        Err(outcome) => return outcome,
    };

    let after = &tokens[next_idx];
    match &after.kind {
        TokenKind::Eof => ParseOutcome::Complete(Line::Command(Command::Imperative(ImperativeCommand {
            verb,
            verb_span,
            operand: Some(operand),
            destination: None,
        }))),
        TokenKind::Word(w) if w.eq_ignore_ascii_case("to") => {
            let dest_idx = next_idx + 1;
            let (destination, end_idx) = match parse_operand(tokens, dest_idx, "'to'") {
                Ok(pair) => pair,
                Err(outcome) => return outcome,
            };
            let trailing = &tokens[end_idx];
            if !matches!(trailing.kind, TokenKind::Eof) {
                return Diagnostic::invalid("Unexpected input after destination.", trailing.span, vec![]).into_invalid();
            }
            ParseOutcome::Complete(Line::Command(Command::Imperative(ImperativeCommand {
                verb,
                verb_span,
                operand: Some(operand),
                destination: Some(destination),
            })))
        }
        _ => Diagnostic::invalid(
            "Unexpected input; expected end of command or 'to'.",
            after.span,
            vec!["to".into()],
        )
        .into_invalid(),
    }
}

/// Parses one operand (an ownership+path pair, a `THIS`/`THAT` reference, or
/// a string literal) starting at `idx`. `context` names what is requiring
/// the operand, for diagnostic messages.
fn parse_operand(tokens: &[Token], idx: usize, context: &str) -> Result<(Operand, usize), ParseOutcome> {
    let tok = &tokens[idx];
    match &tok.kind {
        TokenKind::Eof => Err(Diagnostic::incomplete(
            format!("{context} requires a target."),
            tok.span,
            operand_expected(),
        )
        .into_incomplete()),
        TokenKind::Word(w) => {
            let upper = w.to_uppercase();
            if let Some(kind) = OwnershipKind::from_str(&upper) {
                let path_idx = idx + 1;
                let path_tok = &tokens[path_idx];
                match &path_tok.kind {
                    TokenKind::Eof => Err(Diagnostic::incomplete(
                        format!("{upper} requires a path."),
                        path_tok.span,
                        vec!["<path>".into()],
                    )
                    .into_incomplete()),
                    TokenKind::Word(p) => Ok((
                        Operand::Owned {
                            kind,
                            path: p.clone(),
                            span: Span::new(tok.span.start, path_tok.span.end),
                        },
                        path_idx + 1,
                    )),
                    TokenKind::StringLit(p) => Ok((
                        Operand::Owned {
                            kind,
                            path: p.clone(),
                            span: Span::new(tok.span.start, path_tok.span.end),
                        },
                        path_idx + 1,
                    )),
                    _ => Err(Diagnostic::invalid(
                        format!("Expected a path after '{upper}'."),
                        path_tok.span,
                        vec!["<path>".into()],
                    )
                    .into_invalid()),
                }
            } else if let Some(kind) = ReferenceKind::from_str(&upper) {
                Ok((Operand::Reference { kind, span: tok.span }, idx + 1))
            } else {
                Err(Diagnostic::invalid(
                    format!("'{w}' is not a valid target for {context}."),
                    tok.span,
                    operand_expected(),
                )
                .into_invalid())
            }
        }
        TokenKind::StringLit(s) => Ok((
            Operand::StringArg {
                value: s.clone(),
                span: tok.span,
            },
            idx + 1,
        )),
        TokenKind::Bang | TokenKind::At | TokenKind::Pound => Err(Diagnostic::invalid(
            format!("{context} requires a target."),
            tok.span,
            operand_expected(),
        )
        .into_invalid()),
    }
}

fn parse_policy(tokens: &[Token], idx: usize) -> ParseOutcome {
    let tok = &tokens[idx];
    match &tok.kind {
        TokenKind::Eof => {
            Diagnostic::incomplete("! requires a policy operator.", tok.span, policy_operator_expected()).into_incomplete()
        }
        TokenKind::Word(w) => match PolicyOperator::from_str(&w.to_uppercase()) {
            Some(op) => parse_policy_args(tokens, idx + 1, op, tok.span),
            None => {
                Diagnostic::invalid(format!("Unknown policy operator '{w}'."), tok.span, policy_operator_expected())
                    .into_invalid()
            }
        },
        _ => Diagnostic::invalid("Expected a policy operator after !.", tok.span, policy_operator_expected())
            .into_invalid(),
    }
}

fn parse_policy_args(tokens: &[Token], idx: usize, op: PolicyOperator, op_span: Span) -> ParseOutcome {
    match op.arg_kind() {
        PolicyArgKind::None => {
            let tok = &tokens[idx];
            if !matches!(tok.kind, TokenKind::Eof) {
                return Diagnostic::invalid(format!("{} takes no arguments.", op.as_str()), tok.span, vec![])
                    .into_invalid();
            }
            ParseOutcome::Complete(Line::Policy(PolicyStatement {
                operator: op,
                operator_span: op_span,
                args: PolicyArgs::None,
            }))
        }
        PolicyArgKind::Capability => {
            let tok = &tokens[idx];
            let (name, name_span) = match &tok.kind {
                TokenKind::Eof => {
                    return Diagnostic::incomplete(
                        format!("{} requires a capability.", op.as_str()),
                        tok.span,
                        vec!["<capability>".into()],
                    )
                    .into_incomplete()
                }
                TokenKind::Word(w) => (w.clone(), tok.span),
                TokenKind::StringLit(s) => (s.clone(), tok.span),
                _ => {
                    return Diagnostic::invalid(
                        format!("{} requires a capability.", op.as_str()),
                        tok.span,
                        vec!["<capability>".into()],
                    )
                    .into_invalid()
                }
            };
            let after = &tokens[idx + 1];
            if !matches!(after.kind, TokenKind::Eof) {
                return Diagnostic::invalid("Unexpected input after policy capability.", after.span, vec![]).into_invalid();
            }
            ParseOutcome::Complete(Line::Policy(PolicyStatement {
                operator: op,
                operator_span: op_span,
                args: PolicyArgs::Capability { name, span: name_span },
            }))
        }
        PolicyArgKind::Condition => {
            let tok = &tokens[idx];
            let (text, text_span) = match &tok.kind {
                TokenKind::Eof => {
                    return Diagnostic::incomplete(
                        format!("{} requires a condition.", op.as_str()),
                        tok.span,
                        vec!["<condition>".into()],
                    )
                    .into_incomplete()
                }
                TokenKind::Word(w) => (w.clone(), tok.span),
                TokenKind::StringLit(s) => (s.clone(), tok.span),
                _ => {
                    return Diagnostic::invalid(
                        format!("{} requires a condition.", op.as_str()),
                        tok.span,
                        vec!["<condition>".into()],
                    )
                    .into_invalid()
                }
            };
            let after = &tokens[idx + 1];
            if !matches!(after.kind, TokenKind::Eof) {
                return Diagnostic::invalid("Unexpected input after policy condition.", after.span, vec![]).into_invalid();
            }
            ParseOutcome::Complete(Line::Policy(PolicyStatement {
                operator: op,
                operator_span: op_span,
                args: PolicyArgs::Condition { text, span: text_span },
            }))
        }
        PolicyArgKind::Quotas => parse_quotas(tokens, idx, op, op_span),
    }
}

fn parse_quotas(tokens: &[Token], mut idx: usize, op: PolicyOperator, op_span: Span) -> ParseOutcome {
    let mut quotas = Vec::new();
    loop {
        let tok = &tokens[idx];
        match &tok.kind {
            TokenKind::Eof => {
                if quotas.is_empty() {
                    return Diagnostic::incomplete(
                        format!("{} requires at least one resource quota, e.g. 20%RAM.", op.as_str()),
                        tok.span,
                        vec!["<percent>%<resource>".into()],
                    )
                    .into_incomplete();
                }
                return ParseOutcome::Complete(Line::Policy(PolicyStatement {
                    operator: op,
                    operator_span: op_span,
                    args: PolicyArgs::Quotas(quotas),
                }));
            }
            TokenKind::Word(w) => match parse_quota(w, tok.span) {
                Ok(quota) => {
                    quotas.push(quota);
                    idx += 1;
                }
                Err(message) => {
                    return Diagnostic::invalid(message, tok.span, vec!["<percent>%<resource>".into()]).into_invalid();
                }
            },
            _ => {
                return Diagnostic::invalid(
                    "Expected a resource quota, e.g. 20%RAM.",
                    tok.span,
                    vec!["<percent>%<resource>".into()],
                )
                .into_invalid()
            }
        }
    }
}

fn parse_quota(word: &str, span: Span) -> Result<ResourceQuota, String> {
    let Some((percent_str, resource)) = word.split_once('%') else {
        return Err(format!("'{word}' is not a resource quota (expected e.g. 20%RAM)."));
    };
    let Ok(percent) = percent_str.parse::<u32>() else {
        return Err(format!("'{percent_str}' is not a valid percentage in '{word}'."));
    };
    if resource.is_empty() {
        return Err(format!("'{word}' is missing a resource name (expected e.g. 20%RAM)."));
    }
    Ok(ResourceQuota {
        percent,
        resource: resource.to_uppercase(),
        span,
    })
}

trait IntoOutcome {
    fn into_incomplete(self) -> ParseOutcome;
    fn into_invalid(self) -> ParseOutcome;
}

impl IntoOutcome for Diagnostic {
    fn into_incomplete(self) -> ParseOutcome {
        ParseOutcome::Incomplete(self)
    }
    fn into_invalid(self) -> ParseOutcome {
        ParseOutcome::Invalid(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xact_ast::{OwnershipKind as Own, ReferenceKind as Ref};

    fn command(outcome: ParseOutcome) -> Command {
        match outcome {
            ParseOutcome::Complete(Line::Command(cmd)) => cmd,
            other => panic!("expected complete command, got {other:?}"),
        }
    }

    fn policy(outcome: ParseOutcome) -> PolicyStatement {
        match outcome {
            ParseOutcome::Complete(Line::Policy(stmt)) => stmt,
            other => panic!("expected complete policy, got {other:?}"),
        }
    }

    #[test]
    fn complete_see_with_owned_path() {
        let cmd = match command(parse_line("£ SEE MY ~/Documents")) {
            Command::Imperative(cmd) => cmd,
            other => panic!("expected imperative, got {other:?}"),
        };
        assert_eq!(cmd.verb, Verb::See);
        match cmd.operand {
            Some(Operand::Owned { kind, path, .. }) => {
                assert_eq!(kind, Own::My);
                assert_eq!(path, "~/Documents");
            }
            other => panic!("expected owned operand, got {other:?}"),
        }
        assert_eq!(cmd.destination, None);
    }

    #[test]
    fn incomplete_copy_missing_target() {
        match parse_line("£ COPY") {
            ParseOutcome::Incomplete(diag) => {
                assert!(diag.expected.contains(&"MY".to_string()));
                assert!(diag.expected.contains(&"THIS".to_string()));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn incomplete_ownership_missing_path() {
        match parse_line("£ COPY MY") {
            ParseOutcome::Incomplete(diag) => {
                assert_eq!(diag.expected, vec!["<path>".to_string()]);
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn invalid_unknown_verb() {
        match parse_line("£ FROBNICATE MY ~/x") {
            ParseOutcome::Invalid(diag) => {
                assert!(diag.message.contains("Unknown command"));
            }
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn grammatically_complete_reference_operand_is_accepted() {
        // Whether THAT actually resolves is a semantic concern (xact-semantic),
        // not a grammar concern — spec section 19's layered pipeline.
        let cmd = match command(parse_line("£ COPY THAT")) {
            Command::Imperative(cmd) => cmd,
            other => panic!("expected imperative, got {other:?}"),
        };
        match cmd.operand {
            Some(Operand::Reference { kind, .. }) => assert_eq!(kind, Ref::That),
            other => panic!("expected reference operand, got {other:?}"),
        }
    }

    #[test]
    fn complete_copy_with_destination() {
        let cmd = match command(parse_line("£ COPY THAT to OUR ~/backup")) {
            Command::Imperative(cmd) => cmd,
            other => panic!("expected imperative, got {other:?}"),
        };
        assert_eq!(cmd.verb, Verb::Copy);
        match cmd.destination {
            Some(Operand::Owned { kind, path, .. }) => {
                assert_eq!(kind, Own::Our);
                assert_eq!(path, "~/backup");
            }
            other => panic!("expected owned destination, got {other:?}"),
        }
    }

    #[test]
    fn complete_identity_declaration() {
        match command(parse_line("£ THEY are \"alice,bob\"")) {
            Command::Identity(decl) => {
                assert_eq!(decl.members, vec!["alice".to_string(), "bob".to_string()]);
            }
            other => panic!("expected identity, got {other:?}"),
        }
    }

    #[test]
    fn run_takes_string_operand() {
        let cmd = match command(parse_line("£ RUN 'chrome'")) {
            Command::Imperative(cmd) => cmd,
            other => panic!("expected imperative, got {other:?}"),
        };
        assert_eq!(cmd.verb, Verb::Run);
        match cmd.operand {
            Some(Operand::StringArg { value, .. }) => assert_eq!(value, "chrome"),
            other => panic!("expected string operand, got {other:?}"),
        }
    }

    #[test]
    fn missing_pound_rejected() {
        match parse_line("SEE MY ~/Documents") {
            ParseOutcome::Invalid(diag) => assert!(diag.message.contains("must begin with")),
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn policy_save_with_quotas() {
        let stmt = policy(parse_line("! SAVE 20%RAM 30%CPU"));
        assert_eq!(stmt.operator, PolicyOperator::Save);
        match stmt.args {
            PolicyArgs::Quotas(quotas) => {
                assert_eq!(quotas.len(), 2);
                assert_eq!(quotas[0].percent, 20);
                assert_eq!(quotas[0].resource, "RAM");
                assert_eq!(quotas[1].percent, 30);
                assert_eq!(quotas[1].resource, "CPU");
            }
            other => panic!("expected quotas, got {other:?}"),
        }
    }

    #[test]
    fn policy_save_incomplete_without_quota() {
        match parse_line("! SAVE") {
            ParseOutcome::Incomplete(diag) => assert!(diag.message.contains("requires at least one resource quota")),
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn policy_with_capability() {
        let stmt = policy(parse_line("! WITH 'network'"));
        assert_eq!(stmt.operator, PolicyOperator::With);
        match stmt.args {
            PolicyArgs::Capability { name, .. } => assert_eq!(name, "network"),
            other => panic!("expected capability, got {other:?}"),
        }
    }

    #[test]
    fn policy_concurrently_takes_no_args() {
        let stmt = policy(parse_line("! CONCURRENTLY"));
        assert_eq!(stmt.operator, PolicyOperator::Concurrently);
        assert_eq!(stmt.args, PolicyArgs::None);
    }

    #[test]
    fn policy_concurrently_rejects_trailing_input() {
        match parse_line("! CONCURRENTLY now") {
            ParseOutcome::Invalid(diag) => assert!(diag.message.contains("takes no arguments")),
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn policy_unknown_operator_rejected() {
        match parse_line("! FROBNICATE") {
            ParseOutcome::Invalid(diag) => assert!(diag.message.contains("Unknown policy operator")),
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn policy_bad_quota_shape_rejected() {
        match parse_line("! SAVE RAM") {
            ParseOutcome::Invalid(diag) => assert!(diag.message.contains("not a resource quota")),
            other => panic!("expected invalid, got {other:?}"),
        }
    }
}
