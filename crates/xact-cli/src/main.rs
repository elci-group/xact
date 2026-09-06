//! Minimal REPL: type a `£` command, a `!` policy statement, or an `@`
//! agent block, get grammatical + semantic validation and a diagnostic if
//! it fails.
//!
//! Once a command is accepted, the CLI plans it (`xact-planner`) and, if a
//! plan exists, actually runs it (`xact-executor`): `£ CREATE ...` really
//! creates the file or directory via the real `bank` binary, `£ SEE ...`
//! really shows it via `gls` (directories) or `bat` (files), and
//! `£ RUN ...` really spawns the process and waits for it to exit. Every
//! other verb reports itself unsupported rather than silently doing
//! nothing.
//!
//! Scheduling (spec section 23, Xact–Mesut Integration Phase 3): with no
//! `! CONCURRENTLY`/`! CONSECUTIVELY` stated, or under `! CONSECUTIVELY`,
//! each accepted command runs and is waited on before the next line is
//! even read — `A → B`. Once `! CONCURRENTLY` is established for the
//! session, subsequent commands are instead admitted onto Mesut as
//! independent branches and not waited on immediately; this is the only
//! place that distinction is made — `xact-executor` just exposes both a
//! blocking and a non-blocking way to run a plan; `session.schedule()` is
//! what tells this loop which one to call.
//!
//! `@` blocks may be typed across several lines for readability (matching
//! spec section 9's example layout): once a line starts with `@`, the REPL
//! keeps reading continuation lines until a blank line, then submits the
//! whole thing as one input. This is a REPL-level convenience only — the
//! lexer treats newlines as ordinary whitespace, so the grammar itself
//! doesn't care whether a block is typed on one line or several.

use std::io::{self, Write};

use xact_ast::{AgentBlock, AgentClause, Command, Operand, PolicyArgs, PolicyOperator, PolicyStatement};
use xact_core::{Session, SessionOutcome};
use xact_executor::{ExecutionOutcome, Pending};
use xact_planner::PlanOutcome;

/// Concurrent branches admitted under `! CONCURRENTLY` but not yet joined,
/// paired with the description printed for `£ ...` when they were queued.
type PendingBranches = Vec<(String, Pending)>;

/// Prints any branches that have finished since the last check, without
/// blocking on the ones still running.
fn drain_ready(pending: &mut PendingBranches) {
    pending.retain_mut(|(description, branch)| match branch.try_join() {
        Some(outcome) => {
            print_outcome(Some(description), &outcome);
            false
        }
        None => true,
    });
}

/// Blocks until every remaining branch has finished — used at session end
/// so nothing started under `! CONCURRENTLY` is left unreported.
fn join_all(pending: PendingBranches) {
    for (description, branch) in pending {
        print_outcome(Some(&description), &branch.join());
    }
}

fn print_outcome(branch: Option<&str>, outcome: &ExecutionOutcome) {
    let prefix = match branch {
        Some(description) => format!("  [{description}] "),
        None => "  ".to_string(),
    };
    match outcome {
        ExecutionOutcome::BankEstablished { path } => {
            println!("{prefix}bank: established {}", path.display());
        }
        ExecutionOutcome::Viewed { path, tool } => {
            println!("{prefix}{tool}: displayed {}", path.display());
        }
        ExecutionOutcome::RunCompleted { command_line, success, code } => {
            let status = match (success, code) {
                (true, _) => "exited 0".to_string(),
                (false, Some(code)) => format!("exited {code}"),
                (false, None) => "terminated by signal".to_string(),
            };
            println!("{prefix}ran '{command_line}' — {status}");
        }
        ExecutionOutcome::Failed { message } => {
            println!("{prefix}execution failed: {message}");
        }
    }
}

fn main() {
    println!("xact 0.1.0 — grammar, ownership, reference, policy, and agent validation; CREATE, SEE, and RUN actually run");
    println!("Type a £ command, a ! policy statement, an @ agent block, or 'exit'.");

    let mut session = Session::new();
    let stdin = io::stdin();
    let mut pending: PendingBranches = Vec::new();

    loop {
        drain_ready(&mut pending);

        print!("xact> ");
        if io::stdout().flush().is_err() {
            join_all(pending);
            break;
        }

        let mut line = String::new();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            join_all(pending);
            break;
        }
        let line = line.trim_end().to_string();
        if line.is_empty() {
            continue;
        }
        if line == "exit" || line == "quit" {
            join_all(pending);
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
                let description = describe(&command);
                println!("accepted: {description}");
                match xact_planner::plan(&command, session.references()) {
                    PlanOutcome::Plan(plan) => {
                        if session.schedule() == Some(PolicyOperator::Concurrently) {
                            match xact_executor::execute_concurrent(plan) {
                                Ok(branch) => {
                                    println!("  queued as an independent branch ({} in flight)", pending.len() + 1);
                                    pending.push((description, branch));
                                }
                                Err(outcome) => print_outcome(None, &outcome),
                            }
                        } else {
                            print_outcome(None, &xact_executor::execute(plan));
                        }
                    }
                    PlanOutcome::Unsupported(reason) => println!("  {reason}"),
                }
            }
            SessionOutcome::PolicyAccepted(stmt) => {
                println!("policy set: {}", describe_policy(&stmt));
                if !matches!(stmt.operator, PolicyOperator::Concurrently | PolicyOperator::Consecutively) {
                    println!("  (not yet enforced — the planner/executor are not implemented yet.)");
                }
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
