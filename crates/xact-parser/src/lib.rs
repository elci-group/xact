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
    AgentBlock, AgentClause, AgentClauseKind, AgentVerb, Command, IdentityDeclaration, ImperativeCommand, Line, Operand,
    OwnershipKind, PolicyArgKind, PolicyArgs, PolicyOperator, PolicyStatement, ReferenceKind, ResourceQuota, Span, Verb,
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

fn agent_verb_expected() -> Vec<String> {
    AgentVerb::ALL.iter().map(|v| v.as_str().to_string()).collect()
}

/// What can legally come next inside an agent block: any clause keyword not
/// already used, plus the instruction string that closes the block (spec
/// section 16: a singleton clause must not be suggested again).
fn agent_continuation_expected(used: &[AgentClause]) -> Vec<String> {
    AgentClauseKind::ALL
        .iter()
        .filter(|kind| !used.iter().any(|c| c.kind() == **kind))
        .map(|kind| kind.as_str().to_string())
        .chain(std::iter::once("'...'".to_string()))
        .collect()
}

pub fn parse_tokens(tokens: &[Token]) -> ParseOutcome {
    let first = &tokens[0];
    match &first.kind {
        TokenKind::Eof => Diagnostic::incomplete("Expected a command.", first.span, vec!["£".into()]).into_incomplete(),
        TokenKind::Pound => parse_after_pound(tokens, 1),
        TokenKind::Bang => parse_policy(tokens, 1),
        TokenKind::At => parse_agent(tokens, 1),
        TokenKind::Word(_) | TokenKind::StringLit(_) => Diagnostic::invalid(
            "Commands must begin with £, !, or @.",
            first.span,
            vec!["£".into(), "!".into(), "@".into()],
        )
        .into_invalid(),
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

fn dependency_reference_expected() -> Vec<String> {
    ReferenceKind::ALL.iter().map(|k| k.as_str().to_string()).collect()
}

fn dependency_condition_expected() -> Vec<String> {
    vec!["SUCCEEDS".to_string(), "FAILS".to_string()]
}

fn parse_imperative(tokens: &[Token], idx: usize, verb: Verb, verb_span: Span) -> ParseOutcome {
    let (operand, next_idx) = match parse_operand(tokens, idx, verb.as_str()) {
        Ok(pair) => pair,
        Err(outcome) => return outcome,
    };
    parse_after_operand(tokens, next_idx, verb, verb_span, operand)
}

fn parse_after_operand(
    tokens: &[Token],
    idx: usize,
    verb: Verb,
    verb_span: Span,
    operand: Operand,
) -> ParseOutcome {
    let tok = &tokens[idx];
    match &tok.kind {
        TokenKind::Eof => ParseOutcome::Complete(Line::Command(Command::Imperative(ImperativeCommand {
            verb,
            verb_span,
            operand: Some(operand),
            destination: None,
            dependency: None,
        }))),
        TokenKind::Word(w) if w.eq_ignore_ascii_case("to") => {
            let dest_idx = idx + 1;
            let (destination, end_idx) = match parse_operand(tokens, dest_idx, "'to'") {
                Ok(pair) => pair,
                Err(outcome) => return outcome,
            };
            parse_after_destination(tokens, end_idx, verb, verb_span, operand, destination)
        }
        TokenKind::Word(w) if w.eq_ignore_ascii_case("when") => {
            finish_with_dependency(tokens, idx + 1, verb, verb_span, operand, None)
        }
        _ => Diagnostic::invalid(
            "Unexpected input; expected end of command, 'to', or 'WHEN'.",
            tok.span,
            vec!["to".into(), "WHEN".into()],
        )
        .into_invalid(),
    }
}

fn parse_after_destination(
    tokens: &[Token],
    idx: usize,
    verb: Verb,
    verb_span: Span,
    operand: Operand,
    destination: Operand,
) -> ParseOutcome {
    let tok = &tokens[idx];
    match &tok.kind {
        TokenKind::Eof => ParseOutcome::Complete(Line::Command(Command::Imperative(ImperativeCommand {
            verb,
            verb_span,
            operand: Some(operand),
            destination: Some(destination),
            dependency: None,
        }))),
        TokenKind::Word(w) if w.eq_ignore_ascii_case("when") => {
            finish_with_dependency(tokens, idx + 1, verb, verb_span, operand, Some(destination))
        }
        _ => Diagnostic::invalid(
            "Unexpected input after destination; expected end of command or 'WHEN'.",
            tok.span,
            vec!["WHEN".into()],
        )
        .into_invalid(),
    }
}

/// Parses `THIS`/`THAT SUCCEEDS`/`FAILS` starting right after `WHEN` and, on
/// success, assembles the whole command (spec section 9's dependency
/// clause — Xact–Mesut Integration Phase 8).
fn finish_with_dependency(
    tokens: &[Token],
    idx: usize,
    verb: Verb,
    verb_span: Span,
    operand: Operand,
    destination: Option<Operand>,
) -> ParseOutcome {
    let ref_tok = &tokens[idx];
    let (reference, reference_span) = match &ref_tok.kind {
        TokenKind::Eof => {
            return Diagnostic::incomplete("WHEN requires THIS or THAT.", ref_tok.span, dependency_reference_expected())
                .into_incomplete()
        }
        TokenKind::Word(w) => match ReferenceKind::from_str(&w.to_uppercase()) {
            Some(kind) => (kind, ref_tok.span),
            None => {
                return Diagnostic::invalid(
                    format!("Unknown reference '{w}' after WHEN."),
                    ref_tok.span,
                    dependency_reference_expected(),
                )
                .into_invalid()
            }
        },
        _ => {
            return Diagnostic::invalid("WHEN requires THIS or THAT.", ref_tok.span, dependency_reference_expected())
                .into_invalid()
        }
    };

    let cond_idx = idx + 1;
    let cond_tok = &tokens[cond_idx];
    let (condition, condition_span) = match &cond_tok.kind {
        TokenKind::Eof => {
            return Diagnostic::incomplete(
                format!("WHEN {} requires SUCCEEDS or FAILS.", reference.as_str()),
                cond_tok.span,
                dependency_condition_expected(),
            )
            .into_incomplete()
        }
        TokenKind::Word(w) => match xact_ast::SuccessCondition::from_str(&w.to_uppercase()) {
            Some(condition) => (condition, cond_tok.span),
            None => {
                return Diagnostic::invalid(
                    format!("Unknown condition '{w}' after WHEN {}.", reference.as_str()),
                    cond_tok.span,
                    dependency_condition_expected(),
                )
                .into_invalid()
            }
        },
        _ => {
            return Diagnostic::invalid(
                format!("WHEN {} requires SUCCEEDS or FAILS.", reference.as_str()),
                cond_tok.span,
                dependency_condition_expected(),
            )
            .into_invalid()
        }
    };

    let trailing = &tokens[cond_idx + 1];
    if !matches!(trailing.kind, TokenKind::Eof) {
        return Diagnostic::invalid("Unexpected input after WHEN clause.", trailing.span, vec![]).into_invalid();
    }

    ParseOutcome::Complete(Line::Command(Command::Imperative(ImperativeCommand {
        verb,
        verb_span,
        operand: Some(operand),
        destination,
        dependency: Some(xact_ast::DependencyClause { reference, reference_span, condition, condition_span }),
    })))
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

fn parse_agent(tokens: &[Token], idx: usize) -> ParseOutcome {
    let tok = &tokens[idx];
    match &tok.kind {
        TokenKind::Eof => {
            Diagnostic::incomplete("@ requires TELL or TEAM.", tok.span, agent_verb_expected()).into_incomplete()
        }
        TokenKind::Word(w) => match AgentVerb::from_str(&w.to_uppercase()) {
            Some(verb) => parse_agent_target(tokens, idx + 1, verb, tok.span),
            None => {
                Diagnostic::invalid(format!("Unknown agent verb '{w}'."), tok.span, agent_verb_expected()).into_invalid()
            }
        },
        _ => Diagnostic::invalid("Expected TELL or TEAM after @.", tok.span, agent_verb_expected()).into_invalid(),
    }
}

fn parse_agent_target(tokens: &[Token], idx: usize, verb: AgentVerb, verb_span: Span) -> ParseOutcome {
    let tok = &tokens[idx];
    let (target, target_span) = match &tok.kind {
        TokenKind::Eof => {
            return Diagnostic::incomplete(
                format!("{} requires a target agent name.", verb.as_str()),
                tok.span,
                vec!["'...'".into()],
            )
            .into_incomplete()
        }
        TokenKind::Word(w) => (w.clone(), tok.span),
        TokenKind::StringLit(s) => (s.clone(), tok.span),
        _ => {
            return Diagnostic::invalid(
                format!("{} requires a target agent name.", verb.as_str()),
                tok.span,
                vec!["'...'".into()],
            )
            .into_invalid()
        }
    };
    parse_agent_clauses(tokens, idx + 1, verb, verb_span, target, target_span, Vec::new())
}

fn parse_agent_clauses(
    tokens: &[Token],
    mut idx: usize,
    verb: AgentVerb,
    verb_span: Span,
    target: String,
    target_span: Span,
    mut clauses: Vec<AgentClause>,
) -> ParseOutcome {
    loop {
        let tok = &tokens[idx];
        match &tok.kind {
            TokenKind::Eof => {
                return Diagnostic::incomplete(
                    "Expected a clause or the instruction to close this @ block.",
                    tok.span,
                    agent_continuation_expected(&clauses),
                )
                .into_incomplete()
            }
            TokenKind::StringLit(s) => {
                let after = &tokens[idx + 1];
                if !matches!(after.kind, TokenKind::Eof) {
                    return Diagnostic::invalid("Unexpected input after the @ block's instruction.", after.span, vec![])
                        .into_invalid();
                }
                return ParseOutcome::Complete(Line::Agent(AgentBlock {
                    verb,
                    verb_span,
                    target,
                    target_span,
                    clauses,
                    instruction: s.clone(),
                    instruction_span: tok.span,
                }));
            }
            TokenKind::Word(w) => {
                let Some(kind) = AgentClauseKind::from_str(&w.to_uppercase()) else {
                    return Diagnostic::invalid(
                        format!("Unknown clause '{w}'."),
                        tok.span,
                        agent_continuation_expected(&clauses),
                    )
                    .into_invalid();
                };
                if clauses.iter().any(|c| c.kind() == kind) {
                    return Diagnostic::invalid(
                        format!("{} was already given for this @ block.", kind.as_str()),
                        tok.span,
                        vec![],
                    )
                    .into_invalid();
                }
                let (clause, next_idx) = match parse_agent_clause_arg(tokens, idx + 1, kind, tok.span) {
                    Ok(pair) => pair,
                    Err(outcome) => return outcome,
                };
                clauses.push(clause);
                idx = next_idx;
            }
            TokenKind::Bang | TokenKind::At | TokenKind::Pound => {
                return Diagnostic::invalid(
                    "Expected a clause or the instruction to close this @ block.",
                    tok.span,
                    agent_continuation_expected(&clauses),
                )
                .into_invalid()
            }
        }
    }
}

fn parse_agent_clause_arg(
    tokens: &[Token],
    idx: usize,
    kind: AgentClauseKind,
    kind_span: Span,
) -> Result<(AgentClause, usize), ParseOutcome> {
    match kind {
        AgentClauseKind::Be => {
            let tok = &tokens[idx];
            match &tok.kind {
                TokenKind::Eof => Err(Diagnostic::incomplete(
                    "BE requires a persona description.",
                    tok.span,
                    vec!["'...'".into()],
                )
                .into_incomplete()),
                TokenKind::Word(w) => Ok((
                    AgentClause::Be {
                        persona: w.clone(),
                        span: Span::new(kind_span.start, tok.span.end),
                    },
                    idx + 1,
                )),
                TokenKind::StringLit(s) => Ok((
                    AgentClause::Be {
                        persona: s.clone(),
                        span: Span::new(kind_span.start, tok.span.end),
                    },
                    idx + 1,
                )),
                _ => Err(
                    Diagnostic::invalid("BE requires a persona description.", tok.span, vec!["'...'".into()])
                        .into_invalid(),
                ),
            }
        }
        AgentClauseKind::Reading | AgentClauseKind::Populating => {
            let (operand, next_idx) = parse_operand(tokens, idx, kind.as_str())?;
            let span = Span::new(kind_span.start, operand.span().end);
            let clause = if kind == AgentClauseKind::Reading {
                AgentClause::Reading { operand, span }
            } else {
                AgentClause::Populating { operand, span }
            };
            Ok((clause, next_idx))
        }
        AgentClauseKind::Think => {
            let tok = &tokens[idx];
            match &tok.kind {
                TokenKind::Eof => Err(Diagnostic::incomplete(
                    "THINK requires a numeric budget, e.g. THINK 80.",
                    tok.span,
                    vec!["<number>".into()],
                )
                .into_incomplete()),
                TokenKind::Word(w) => match w.parse::<u32>() {
                    Ok(budget) => Ok((
                        AgentClause::Think {
                            budget,
                            span: Span::new(kind_span.start, tok.span.end),
                        },
                        idx + 1,
                    )),
                    Err(_) => Err(Diagnostic::invalid(
                        format!("'{w}' is not a valid THINK budget (expected a number)."),
                        tok.span,
                        vec!["<number>".into()],
                    )
                    .into_invalid()),
                },
                _ => Err(Diagnostic::invalid(
                    "THINK requires a numeric budget, e.g. THINK 80.",
                    tok.span,
                    vec!["<number>".into()],
                )
                .into_invalid()),
            }
        }
    }
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

    fn agent(outcome: ParseOutcome) -> AgentBlock {
        match outcome {
            ParseOutcome::Complete(Line::Agent(block)) => block,
            other => panic!("expected complete agent block, got {other:?}"),
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
    fn run_with_when_clause_after_operand() {
        let cmd = match command(parse_line("£ RUN 'test' WHEN THAT SUCCEEDS")) {
            Command::Imperative(cmd) => cmd,
            other => panic!("expected imperative, got {other:?}"),
        };
        match cmd.dependency {
            Some(dep) => {
                assert_eq!(dep.reference, Ref::That);
                assert_eq!(dep.condition, xact_ast::SuccessCondition::Succeeds);
            }
            None => panic!("expected a dependency clause"),
        }
    }

    #[test]
    fn run_with_when_fails_clause() {
        let cmd = match command(parse_line("£ RUN 'cleanup' WHEN THIS FAILS")) {
            Command::Imperative(cmd) => cmd,
            other => panic!("expected imperative, got {other:?}"),
        };
        match cmd.dependency {
            Some(dep) => {
                assert_eq!(dep.reference, Ref::This);
                assert_eq!(dep.condition, xact_ast::SuccessCondition::Fails);
            }
            None => panic!("expected a dependency clause"),
        }
    }

    #[test]
    fn when_clause_after_destination() {
        let cmd = match command(parse_line("£ COPY THAT to OUR ~/backup WHEN THAT SUCCEEDS")) {
            Command::Imperative(cmd) => cmd,
            other => panic!("expected imperative, got {other:?}"),
        };
        assert!(cmd.destination.is_some());
        assert!(cmd.dependency.is_some());
    }

    #[test]
    fn when_clause_incomplete_without_reference() {
        match parse_line("£ RUN 'test' WHEN") {
            ParseOutcome::Incomplete(diag) => {
                assert!(diag.expected.contains(&"THIS".to_string()));
                assert!(diag.expected.contains(&"THAT".to_string()));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn when_clause_incomplete_without_condition() {
        match parse_line("£ RUN 'test' WHEN THAT") {
            ParseOutcome::Incomplete(diag) => {
                assert!(diag.expected.contains(&"SUCCEEDS".to_string()));
                assert!(diag.expected.contains(&"FAILS".to_string()));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn when_clause_rejects_unknown_condition() {
        match parse_line("£ RUN 'test' WHEN THAT MAYBE") {
            ParseOutcome::Invalid(_) => {}
            other => panic!("expected invalid, got {other:?}"),
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

    #[test]
    fn agent_block_minimal() {
        let block = agent(parse_line("@ TELL 'GPT-5.6-luna' \"Review this project.\""));
        assert_eq!(block.verb, AgentVerb::Tell);
        assert_eq!(block.target, "GPT-5.6-luna");
        assert!(block.clauses.is_empty());
        assert_eq!(block.instruction, "Review this project.");
    }

    #[test]
    fn agent_block_full_spec_example() {
        let block = agent(parse_line(
            "@ TELL 'GPT-5.6-luna' BE \"a meticulous senior Rust engineer\" READING MY ~/project/ POPULATING MY ~/project/review/ THINK 80 \"Review this project.\"",
        ));
        assert_eq!(block.clauses.len(), 4);
        match block.clause(AgentClauseKind::Be) {
            Some(AgentClause::Be { persona, .. }) => assert_eq!(persona, "a meticulous senior Rust engineer"),
            other => panic!("expected BE clause, got {other:?}"),
        }
        match block.clause(AgentClauseKind::Reading) {
            Some(AgentClause::Reading { operand: Operand::Owned { kind, path, .. }, .. }) => {
                assert_eq!(*kind, Own::My);
                assert_eq!(path, "~/project/");
            }
            other => panic!("expected READING clause, got {other:?}"),
        }
        match block.clause(AgentClauseKind::Think) {
            Some(AgentClause::Think { budget, .. }) => assert_eq!(*budget, 80),
            other => panic!("expected THINK clause, got {other:?}"),
        }
        assert_eq!(block.instruction, "Review this project.");
    }

    #[test]
    fn agent_block_incomplete_without_instruction() {
        match parse_line("@ TELL 'x' BE \"engineer\"") {
            ParseOutcome::Incomplete(diag) => {
                assert!(diag.expected.contains(&"READING".to_string()));
                assert!(!diag.expected.contains(&"BE".to_string()));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn agent_block_rejects_duplicate_clause() {
        match parse_line("@ TELL 'x' BE \"a\" BE \"b\" \"go\"") {
            ParseOutcome::Invalid(diag) => assert!(diag.message.contains("already given")),
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn agent_block_rejects_unknown_agent_verb() {
        match parse_line("@ ASK 'x' \"go\"") {
            ParseOutcome::Invalid(diag) => assert!(diag.message.contains("Unknown agent verb")),
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn agent_block_think_requires_number() {
        match parse_line("@ TELL 'x' THINK deep \"go\"") {
            ParseOutcome::Invalid(diag) => assert!(diag.message.contains("not a valid THINK budget")),
            other => panic!("expected invalid, got {other:?}"),
        }
    }
}
