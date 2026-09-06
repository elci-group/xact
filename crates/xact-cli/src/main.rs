//! Minimal Phase 1 REPL: type a `£ ...` line, get grammatical + semantic
//! validation and a diagnostic if it fails. There is no execution yet —
//! that needs `xact-planner`/`xact-executor`, which are still stubs.

use std::io::{self, Write};

use xact_ast::{Command, Operand, PolicyArgs, PolicyStatement};
use xact_core::{Session, SessionOutcome};

fn main() {
    println!("xact 0.1.0 — grammar, ownership, reference, and policy validation; no execution yet");
    println!("Type a £ command, a ! policy statement, or 'exit'.");

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
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if line == "exit" || line == "quit" {
            break;
        }

        match session.submit(line) {
            SessionOutcome::Accepted(command) => {
                println!("accepted: {}", describe(&command));
                println!("  (planning/execution are not implemented yet.)");
            }
            SessionOutcome::PolicyAccepted(stmt) => {
                println!("policy set: {}", describe_policy(&stmt));
                println!("  (not yet enforced — the planner/executor are not implemented yet.)");
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
