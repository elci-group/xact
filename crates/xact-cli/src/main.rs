//! Minimal REPL: type a `£` command, a `!` policy statement, or an `@`
//! agent block, get grammatical + semantic validation and a diagnostic if
//! it fails. There is no execution yet — that needs `xact-planner`/
//! `xact-executor`, which are still stubs.
//!
//! `@` blocks may be typed across several lines for readability (matching
//! spec section 9's example layout): once a line starts with `@`, the REPL
//! keeps reading continuation lines until a blank line, then submits the
//! whole thing as one input. This is a REPL-level convenience only — the
//! lexer treats newlines as ordinary whitespace, so the grammar itself
//! doesn't care whether a block is typed on one line or several.

use std::io::{self, Write};

use xact_ast::{AgentBlock, AgentClause, Command, Operand, PolicyArgs, PolicyStatement};
use xact_core::{Session, SessionOutcome};

fn main() {
    println!("xact 0.1.0 — grammar, ownership, reference, policy, and agent validation; no execution yet");
    println!("Type a £ command, a ! policy statement, an @ agent block, or 'exit'.");

    let mut session = Session::new();
    let stdin = io::stdin();

    loop {
        print!("xact> ");
        if io::stdout().flush().is_err() {
            break;
        }

        let mut line = String::new();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end().to_string();
        if line.is_empty() {
            continue;
        }
        if line == "exit" || line == "quit" {
            break;
        }

        let mut input = line.clone();
        if line.trim_start().starts_with('@') {
            loop {
                print!("  ...> ");
                if io::stdout().flush().is_err() {
                    break;
                }
                let mut cont = String::new();
                if stdin.read_line(&mut cont).unwrap_or(0) == 0 {
                    break;
                }
                if cont.trim().is_empty() {
                    break;
                }
                input.push(' ');
                input.push_str(cont.trim());
            }
        }

        match session.submit(&input) {
            SessionOutcome::Accepted(command) => {
                println!("accepted: {}", describe(&command));
                println!("  (planning/execution are not implemented yet.)");
            }
            SessionOutcome::PolicyAccepted(stmt) => {
                println!("policy set: {}", describe_policy(&stmt));
                println!("  (not yet enforced — the planner/executor are not implemented yet.)");
            }
            SessionOutcome::AgentAccepted(block) => {
                println!("agent intent accepted: {}", describe_agent(&block));
                println!("  (no agent provider is wired up yet — this is a validated intent only.)");
            }
            SessionOutcome::Incomplete(diag) => print!("{diag}"),
            SessionOutcome::Rejected(diagnostics) => {
                for diag in diagnostics {
                    print!("{diag}");
                }
            }
        }
    }
}

fn describe(command: &Command) -> String {
    match command {
        Command::Imperative(cmd) => {
            let mut s = cmd.verb.as_str().to_string();
            if let Some(op) = &cmd.operand {
                s.push(' ');
                s.push_str(&describe_operand(op));
            }
            if let Some(dest) = &cmd.destination {
                s.push_str(" to ");
                s.push_str(&describe_operand(dest));
            }
            s
        }
        Command::Identity(decl) => format!("THEY are {:?}", decl.members),
    }
}

fn describe_operand(operand: &Operand) -> String {
    match operand {
        Operand::Owned { kind, path, .. } => format!("{} {}", kind.as_str(), path),
        Operand::Reference { kind, .. } => kind.as_str().to_string(),
        Operand::StringArg { value, .. } => format!("'{value}'"),
    }
}

fn describe_policy(stmt: &PolicyStatement) -> String {
    let mut s = stmt.operator.as_str().to_string();
    match &stmt.args {
        PolicyArgs::None => {}
        PolicyArgs::Capability { name, .. } => {
            s.push(' ');
            s.push('\'');
            s.push_str(name);
            s.push('\'');
        }
        PolicyArgs::Quotas(quotas) => {
            for q in quotas {
                s.push(' ');
                s.push_str(&q.percent.to_string());
                s.push('%');
                s.push_str(&q.resource);
            }
        }
        PolicyArgs::Condition { text, .. } => {
            s.push(' ');
            s.push_str(text);
        }
    }
    s
}

fn describe_agent(block: &AgentBlock) -> String {
    let mut s = format!("{} '{}'", block.verb.as_str(), block.target);
    for clause in &block.clauses {
        s.push(' ');
        match clause {
            AgentClause::Be { persona, .. } => s.push_str(&format!("BE \"{persona}\"")),
            AgentClause::Reading { operand, .. } => s.push_str(&format!("READING {}", describe_operand(operand))),
            AgentClause::Populating { operand, .. } => s.push_str(&format!("POPULATING {}", describe_operand(operand))),
            AgentClause::Think { budget, .. } => s.push_str(&format!("THINK {budget}")),
        }
    }
    s.push_str(&format!(" \"{}\"", block.instruction));
    s
}
