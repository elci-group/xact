//! Minimal REPL: type a `£` command, a `!` policy statement, or an `@`
//! agent block, get grammatical + semantic validation and a diagnostic if
//! it fails.
//!
//! Once a command is accepted, the CLI plans it (`xact-planner`) and, if a
//! plan exists, actually runs it (`xact-executor`): `£ CREATE ...` really
//! creates the file or directory via the real `bank` binary, `£ SEE ...`
//! really shows it via `gls` (directories) or `bat` (files), `£ RUN ...`
//! really spawns the process and waits for it to exit, and
//! `£ BOUND ... to ...` really aggregates a source directory via the real
//! `bound` binary (Xact–Mesut Integration Phase 6). Every other verb
//! reports itself unsupported rather than silently doing nothing.
//!
//! `@ TELL ...` blocks are planned and executed the same way (Xact–Mesut
//! Integration Phase 7) — `xact-planner::plan_agent` then
//! `xact-executor`, exactly parallel to a `£` command's
//! `xact-planner::plan` then `xact-executor`. `@ TEAM ...` is accepted as
//! a validated intent but has no execution plan yet, same as any other
//! unimplemented verb. `dispatch` is the one place both paths funnel
//! through, since planning and execution are identical from here on.
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
//! Terminal state (spec section 20, Xact–Mesut Integration Phase 4): only
//! `! CONCURRENTLY` branches have anything worth showing between the
//! semantic "accepted"/outcome lines this loop already prints — a
//! `CONSECUTIVELY` command blocks until it's done, so there's no gap to
//! narrate. `drain_ready` prints each branch's real Mesut lifecycle
//! events (`[description] started on ...`, `completed in Nms`, ...) as
//! they arrive, ahead of the next prompt; this is Mesut's
//! execution-oriented telemetry, distinct from — and printed separately
//! from — Xact's own semantic outcome line for that same branch.
//!
//! Resource policy (spec section 22, Xact–Mesut Integration Phase 5):
//! `session.resource_budget()` resolves the session's accumulated
//! `SPEND`/`SAVE` statements and is passed to every `execute`/
//! `execute_concurrent` call — `£ RUN`, `£ BOUND`, and `@ TELL` are
//! actually constrained by it (real Linux cgroup v2 enforcement, via
//! `xact-resource`); a run under an active budget that names an
//! unenforceable resource fails outright rather than running unconstrained.
//!
//! Dependencies (spec section 9, Xact–Mesut Integration Phase 8):
//! `£ RUN 'test' WHEN THAT SUCCEEDS` only actually plans/executes once
//! the real outcome of whatever most recently ran satisfies the stated
//! condition — `session.dependency_satisfied` checks this,
//! `session.record_outcome` is how it learns each real result, and this
//! loop is the only place that decides skip-vs-run. Under
//! `! CONCURRENTLY`, the "most recent" thing might still be an in-flight
//! branch: `LastResult::Pending(id)` tracks that, and `resolve_branch_now`
//! blocks on that *specific* branch (not the others, which keep running
//! independently) the moment a `WHEN` clause needs to know its outcome —
//! a real wait-then-check gate, not a fabricated instant answer.
//! Multi-branch fan-in dependencies and recovery constructs beyond a
//! plain `WHEN ... FAILS` fallback are not implemented — see
//! `MESUT_INTEGRATION.md`'s Phase 8 status for the honest boundary.
//!
//! Cancellation (spec section 12, Xact–Mesut Integration Phase 8,
//! continued): `main` installs a real `CTRL-C` handler
//! (`xact_cancel::cancel_all`) so an interrupt kills whatever's actually
//! running instead of the default "kill the whole Xact process"
//! disposition. See `xact-cancel`'s module docs for the mechanism.
//!
//! `@` blocks may be typed across several lines for readability (matching
//! spec section 9's example layout): once a line starts with `@`, the REPL
//! keeps reading continuation lines until a blank line, then submits the
//! whole thing as one input. This is a REPL-level convenience only — the
//! lexer treats newlines as ordinary whitespace, so the grammar itself
//! doesn't care whether a block is typed on one line or several.

use std::borrow::Cow;
use std::cell::Cell;
use std::sync::{Arc, Mutex};

use reedline::{
    default_emacs_keybindings, Completer, DescriptionMode, EditCommand, Emacs, IdeMenu, KeyCode, KeyModifiers,
    Keybindings, MenuBuilder, Prompt, PromptEditMode, PromptHistorySearch, Reedline, ReedlineEvent, ReedlineMenu,
    Signal, Span, Suggestion,
};
use xact_ast::{AgentBlock, AgentClause, Command, Operand, PolicyArgs, PolicyOperator, PolicyStatement};
use xact_core::{Session, SessionOutcome};
use xact_executor::{ExecutionOutcome, Pending};
use xact_planner::PlanOutcome;

/// The completion menu's name, shared between the keybindings that open it
/// and the `ReedlineMenu` that's registered under it.
const COMPLETION_MENU: &str = "completion_menu";

/// Bridges `reedline`'s [`Completer`] trait to [`Session::complete`] —
/// exactly the same deterministic, parser-reflecting + Padagonia-backed
/// dynamic completion this crate has been building all along, just now
/// wired into a real line editor instead of only being reachable from
/// tests. `Arc<Mutex<_>>` (not `Rc<RefCell<_>>`: `reedline::Completer:
/// Send`) because the menu needs read access to whatever
/// `session`'s current policy state is (spec section 16 — singleton
/// operators already established get filtered out) while the REPL loop
/// below needs mutable access to the same `Session` between lines.
struct XactCompleter {
    session: Arc<Mutex<Session>>,
}

impl Completer for XactCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let typed = &line[..pos];
        let start = trailing_word_start(typed);
        let session = self.session.lock().unwrap();
        session
            .complete(typed)
            .into_iter()
            .map(|value| {
                // Placeholders (`<path>`, `'...'`) and a directory entry
                // that already ends in `/` invite more typing right where
                // the cursor lands; every other candidate (a keyword, a
                // resolved binary name, a resolved file) is a finished
                // token, so a trailing space after accepting it is the
                // useful default.
                let append_whitespace =
                    !value.starts_with('<') && !value.starts_with('\'') && !value.ends_with('/');
                Suggestion {
                    value,
                    description: None,
                    style: None,
                    extra: None,
                    span: Span { start, end: pos },
                    append_whitespace,
                }
            })
            .collect()
    }
}

/// The byte offset, within `typed` (everything up to the cursor), where
/// the word currently being typed starts — i.e. the span reedline should
/// replace when a suggestion is accepted. Mirrors
/// `xact-completion`'s private `trailing_partial_word`: whitespace-ended
/// input has nothing live to replace, so the span collapses to a pure
/// insertion point at the cursor.
fn trailing_word_start(typed: &str) -> usize {
    if typed.is_empty() || typed.ends_with(char::is_whitespace) {
        return typed.len();
    }
    typed.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0)
}

/// A minimal two-state prompt: `xact> ` normally, `  ...> ` while reading
/// a `@` block's continuation lines — the exact same two strings the
/// previous bare `stdin.read_line` REPL printed, so switching to a real
/// line editor doesn't change what the session looks like at rest.
struct XactPrompt {
    continuation: Cell<bool>,
}

impl XactPrompt {
    fn new() -> Self {
        Self { continuation: Cell::new(false) }
    }

    fn set_continuation(&self, continuation: bool) {
        self.continuation.set(continuation);
    }
}

impl Prompt for XactPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        if self.continuation.get() {
            Cow::Borrowed("  ...> ")
        } else {
            Cow::Borrowed("xact> ")
        }
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _edit_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed(":::: ")
    }

    fn render_prompt_history_search_indicator(&self, search: PromptHistorySearch) -> Cow<'_, str> {
        Cow::Owned(format!("(search: {}) ", search.term))
    }
}

/// Every printable character this grammar actually uses (letters, digits,
/// and the literal symbols `£`/`!`/`@`/`'`/`"`/`~`/`/`/`.`/`-`/`_`/`%`, plus
/// space) is bound to insert itself *and* open/refresh the completion
/// menu — not just `Tab` — so the candidate list narrows live on every
/// keystroke, per the original request: options should visibly collapse
/// as the user types, without a manual trigger first. `Backspace` gets the
/// same treatment so deleting a character re-widens the list correctly.
/// `Tab` still works too, both to open the menu from a stopped state and
/// to step through candidates once it's open.
fn add_completion_keybindings(kb: &mut Keybindings) {
    let mut chars: Vec<char> = ('a'..='z').chain('A'..='Z').chain('0'..='9').collect();
    chars.extend(['£', '!', '@', '\'', '"', '~', '/', '.', '-', '_', '%', ' ']);

    for c in chars {
        kb.add_binding(
            KeyModifiers::NONE,
            KeyCode::Char(c),
            ReedlineEvent::Multiple(vec![
                ReedlineEvent::Edit(vec![EditCommand::InsertChar(c)]),
                ReedlineEvent::Menu(COMPLETION_MENU.to_string()),
            ]),
        );
        // crossterm/terminal-dependent: a physically shifted letter can be
        // reported either as its own char under KeyModifiers::NONE (bound
        // just above) or as the lowercase char under KeyModifiers::SHIFT —
        // Emacs::parse_event looks up the lowercased char for any non-NONE
        // modifier, so both real reporting conventions are covered.
        if c.is_ascii_lowercase() {
            let upper = c.to_ascii_uppercase();
            kb.add_binding(
                KeyModifiers::SHIFT,
                KeyCode::Char(c),
                ReedlineEvent::Multiple(vec![
                    ReedlineEvent::Edit(vec![EditCommand::InsertChar(upper)]),
                    ReedlineEvent::Menu(COMPLETION_MENU.to_string()),
                ]),
            );
        }
    }

    kb.add_binding(
        KeyModifiers::NONE,
        KeyCode::Backspace,
        ReedlineEvent::Multiple(vec![
            ReedlineEvent::Edit(vec![EditCommand::Backspace]),
            ReedlineEvent::Menu(COMPLETION_MENU.to_string()),
        ]),
    );

    kb.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![ReedlineEvent::Menu(COMPLETION_MENU.to_string()), ReedlineEvent::MenuNext]),
    );
    kb.add_binding(KeyModifiers::SHIFT, KeyCode::BackTab, ReedlineEvent::MenuPrevious);

    // Overrides default_emacs_keybindings' plain `Enter -> Enter`: because
    // the menu above re-opens/refreshes on essentially every keystroke
    // (that's what makes it live), it's still "active" by the time a
    // fully-typed command is ready to run -- and reedline's own Enter
    // handling, whenever *any* menu is active, unconditionally inserts
    // the highlighted candidate and closes the menu INSTEAD of
    // submitting (confirmed directly in reedline's source: the
    // `Enter | Submit | SubmitOrNewline if menus.any(is_active)` arm in
    // `Reedline::handle_editor_event`). Left alone, that means a real,
    // fully-typed command needs Enter pressed *twice* to actually run --
    // an unacceptable regression for the REPL's core job. `Esc` (which
    // only calls `deactivate_menus()`, no buffer mutation) run first
    // closes it cleanly, so this Enter always reaches plain submission in
    // one press, exactly like before live completion existed.
    kb.add_binding(
        KeyModifiers::NONE,
        KeyCode::Enter,
        ReedlineEvent::Multiple(vec![ReedlineEvent::Esc, ReedlineEvent::Enter]),
    );
}

/// Builds the live line editor: `XactCompleter` (backed by `session`) as
/// the candidate source, an `IdeMenu` dropdown wired to open/refresh on
/// every keystroke (see [`add_completion_keybindings`]) rather than only
/// on an explicit `Tab`.
fn build_line_editor(session: Arc<Mutex<Session>>) -> Reedline {
    let completer = Box::new(XactCompleter { session });

    let menu = Box::new(
        IdeMenu::default()
            .with_name(COMPLETION_MENU)
            .with_min_completion_width(0)
            .with_max_completion_width(60)
            .with_max_completion_height(u16::MAX)
            .with_padding(1)
            .with_cursor_offset(0)
            .with_description_mode(DescriptionMode::PreferRight)
            .with_correct_cursor_pos(false)
            .with_default_border(),
    );

    let mut keybindings = default_emacs_keybindings();
    add_completion_keybindings(&mut keybindings);

    Reedline::create()
        .with_completer(completer)
        .with_menu(ReedlineMenu::EngineCompleter(menu))
        .with_edit_mode(Box::new(Emacs::new(keybindings)))
}

/// A concurrent branch admitted under `! CONCURRENTLY` but not yet
/// joined, tagged with a stable id (so a later `WHEN` clause can block on
/// *this specific* branch — see [`LastResult`]) and the description
/// printed for `£ ...`/`@ ...` when it was queued.
type PendingBranches = Vec<(u64, String, Pending)>;

/// What `THIS`/`THAT`'s outcome currently refers to, for `WHEN` clauses
/// (Xact–Mesut Integration Phase 8). Mirrors the language's existing
/// single-slot `THIS`/`THAT` model (spec section 11) — one "most recent"
/// tracked thing, not a per-object history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastResult {
    /// The most recent real outcome is already recorded in `Session` —
    /// either nothing has run yet, or the last thing that ran already
    /// finished (every synchronous command finishes before the next line
    /// is even read).
    Resolved,
    /// The most recent thing accepted is still running as a concurrent
    /// branch, identified by its id in `PendingBranches`.
    Pending(u64),
}

/// Prints any lifecycle telemetry and finished outcomes for branches
/// since the last check, without blocking on the ones still running.
/// Drains events both before *and* right after checking a branch for
/// completion: the terminal `Completed`/`Failed` event and the branch's
/// real result travel over independent channels and can arrive in either
/// order, so a drain only before `try_join` could have a just-arrived
/// terminal event silently discarded along with the branch once it's
/// removed from `pending`. Also updates `session`'s recorded outcome
/// whenever the branch that resolves is the one `last_result` is
/// currently tracking.
fn drain_ready(pending: &mut PendingBranches, session: &mut Session, last_result: &mut LastResult) {
    pending.retain_mut(|(id, description, branch)| {
        for event in branch.drain_events() {
            print_lifecycle_event(description, &event);
        }
        match branch.try_join() {
            Some(outcome) => {
                for event in branch.drain_events() {
                    print_lifecycle_event(description, &event);
                }
                print_outcome(Some(description), &outcome);
                if *last_result == LastResult::Pending(*id) {
                    session.record_outcome(outcome_succeeded(&outcome));
                    *last_result = LastResult::Resolved;
                }
                false
            }
            None => true,
        }
    });
}

/// Blocks on one specific branch (identified by `id`) and records its
/// real outcome — the `WHEN` clause gate: a dependent command must not
/// even be planned until the branch it depends on has genuinely finished.
/// Branches other than `id` are left running untouched. A missing `id`
/// (should not happen — `last_result` only ever names a branch that was
/// actually queued) is a no-op rather than a panic.
fn resolve_branch_now(id: u64, pending: &mut PendingBranches, session: &mut Session) {
    let Some(pos) = pending.iter().position(|(pid, _, _)| *pid == id) else {
        return;
    };
    let (_, description, mut branch) = pending.remove(pos);
    let outcome = block_until_resolved(&description, &mut branch);
    print_outcome(Some(&description), &outcome);
    session.record_outcome(outcome_succeeded(&outcome));
}

/// Blocks until every remaining branch has finished — used at session end
/// so nothing started under `! CONCURRENTLY` is left unreported.
fn join_all(pending: PendingBranches) {
    for (_, description, mut branch) in pending {
        let outcome = block_until_resolved(&description, &mut branch);
        print_outcome(Some(&description), &outcome);
    }
}

/// Polls a branch to completion, printing lifecycle events as they
/// arrive, and returns its real outcome. Shared by [`join_all`] (every
/// remaining branch) and [`resolve_branch_now`] (one specific branch).
fn block_until_resolved(description: &str, branch: &mut Pending) -> ExecutionOutcome {
    let outcome = loop {
        for event in branch.drain_events() {
            print_lifecycle_event(description, &event);
        }
        if let Some(outcome) = branch.try_join() {
            break outcome;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    for event in branch.drain_events() {
        print_lifecycle_event(description, &event);
    }
    outcome
}

/// Whether `outcome` counts as a success for `WHEN ... SUCCEEDS`/`FAILS`
/// purposes: a `RunCompleted` uses its own real exit status (a nonzero
/// exit is a real failure here, even though `xact-executor` itself never
/// conflates it with `Failed`); every other non-`Failed` outcome only
/// exists on an `Ok` path already, so it counts as success.
fn outcome_succeeded(outcome: &ExecutionOutcome) -> bool {
    match outcome {
        ExecutionOutcome::RunCompleted { success, .. } => *success,
        ExecutionOutcome::Failed { .. } => false,
        ExecutionOutcome::BankEstablished { .. }
        | ExecutionOutcome::Viewed { .. }
        | ExecutionOutcome::Bounded { .. }
        | ExecutionOutcome::Told { .. } => true,
    }
}

fn print_lifecycle_event(description: &str, event: &xact_mesut::LifecycleEvent) {
    use xact_mesut::LifecycleEvent;
    let detail = match event {
        LifecycleEvent::Submitted => "submitted".to_string(),
        LifecycleEvent::Routed { route } => format!("routed to {route}"),
        LifecycleEvent::Queued { queue_depth } => format!("queued (depth {queue_depth})"),
        LifecycleEvent::Started { executor } => format!("started on {executor}"),
        LifecycleEvent::Completed { duration_ms } => format!("completed in {duration_ms}ms"),
        LifecycleEvent::Failed { error } => format!("failed: {error}"),
        LifecycleEvent::Cancelled { reason } => format!("cancelled: {reason}"),
    };
    println!("  [{description}] {detail}");
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
        ExecutionOutcome::RunCompleted { command_line, success, code, signal } => {
            let status = match (success, code, signal) {
                (true, _, _) => "exited 0".to_string(),
                (false, Some(code), _) => format!("exited {code}"),
                (false, None, Some(signal)) => format!("terminated by signal {signal}"),
                (false, None, None) => "terminated by signal".to_string(),
            };
            println!("{prefix}ran '{command_line}' — {status}");
        }
        ExecutionOutcome::Bounded { source, destination } => match destination {
            Some(destination) => {
                println!("{prefix}bound: aggregated {} to {}", source.display(), destination.display());
            }
            None => println!("{prefix}bound: aggregated {} to the clipboard", source.display()),
        },
        ExecutionOutcome::Told { model, response, populated } => {
            println!("{prefix}{model}: {response}");
            if let Some(populated) = populated {
                println!("{prefix}(also written to {})", populated.display());
            }
        }
        ExecutionOutcome::Failed { message } => {
            println!("{prefix}execution failed: {message}");
        }
    }
}

/// Plans and dispatches `plan_outcome` — shared by `£` commands and
/// `@ TELL` blocks, which differ only in how they got a `PlanOutcome`.
/// Updates `session`'s recorded outcome and `last_result` for anything
/// that actually ran (or failed to even submit); a plan that turned out
/// `Unsupported` touches neither, since nothing executed at all.
#[allow(clippy::too_many_arguments)]
fn dispatch(
    plan_outcome: PlanOutcome,
    budget: xact_ast::ResourceBudget,
    description: String,
    concurrent: bool,
    session: &mut Session,
    pending: &mut PendingBranches,
    next_branch_id: &mut u64,
    last_result: &mut LastResult,
) {
    let plan = match plan_outcome {
        PlanOutcome::Plan(plan) => plan,
        PlanOutcome::Unsupported(reason) => {
            println!("  {reason}");
            return;
        }
    };

    if concurrent {
        match xact_executor::execute_concurrent(plan, budget) {
            Ok(branch) => {
                let id = *next_branch_id;
                *next_branch_id += 1;
                println!("  queued as an independent branch ({} in flight)", pending.len() + 1);
                pending.push((id, description, branch));
                *last_result = LastResult::Pending(id);
            }
            Err(outcome) => {
                print_outcome(None, &outcome);
                session.record_outcome(outcome_succeeded(&outcome));
                *last_result = LastResult::Resolved;
            }
        }
    } else {
        let outcome = xact_executor::execute(plan, budget);
        print_outcome(None, &outcome);
        session.record_outcome(outcome_succeeded(&outcome));
        *last_result = LastResult::Resolved;
    }
}

fn main() {
    // Real CTRL-C cancellation (spec section 12; Xact–Mesut Integration
    // Phase 8, continued): without this handler, CTRL-C's default
    // disposition just kills the whole Xact process — the directive
    // explicitly rules that out except for an irrecoverably unresponsive
    // workload. `xact_cancel::cancel_all` sends a real SIGTERM to every
    // process `xact-process`/`xact-bound`/`xact-tell` currently has
    // registered; a killed child's `wait()` simply returns normally with
    // a signal-terminated status, so the REPL loop below needs no special
    // handling for this at all — it already treats that as an ordinary
    // (if unsuccessful) outcome.
    ctrlc::set_handler(xact_cancel::cancel_all).expect("failed to install CTRL-C handler");

    println!("xact 0.1.0 — grammar, ownership, reference, policy, and agent validation; CREATE, SEE, and RUN actually run");
    println!("Type a £ command, a ! policy statement, an @ agent block, or 'exit'.");
    println!("Suggestions narrow live as you type; Tab cycles them, Enter accepts the line.");

    // `Session` is shared (not owned outright) by this loop: `XactCompleter`
    // (inside `line_editor`) needs read access to the same live policy
    // state (spec section 16 — already-established singleton operators
    // drop out of the suggestion list) that this loop mutates between
    // lines. See `XactCompleter`'s doc comment.
    let session = Arc::new(Mutex::new(Session::new()));
    let mut line_editor = build_line_editor(Arc::clone(&session));
    let prompt = XactPrompt::new();

    let mut pending: PendingBranches = Vec::new();
    let mut next_branch_id: u64 = 0;
    let mut last_result = LastResult::Resolved;

    loop {
        drain_ready(&mut pending, &mut session.lock().unwrap(), &mut last_result);

        prompt.set_continuation(false);
        let line = match line_editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => buffer.trim_end().to_string(),
            Ok(Signal::CtrlC) => continue,
            Ok(Signal::CtrlD) | Err(_) => {
                join_all(pending);
                break;
            }
        };
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
                prompt.set_continuation(true);
                let cont = match line_editor.read_line(&prompt) {
                    Ok(Signal::Success(buffer)) => buffer.trim().to_string(),
                    _ => break,
                };
                if cont.is_empty() {
                    break;
                }
                input.push(' ');
                input.push_str(&cont);
            }
        }

        // Bound to a plain `let` rather than matched inline: `match`
        // extends a scrutinee's temporaries across every arm, so an
        // inline `match session.lock().unwrap().submit(&input) { ... }`
        // would hold the lock for the whole match — including the arms
        // below that lock `session` again, which would deadlock (a
        // `Mutex` isn't reentrant). Binding first drops the guard the
        // instant `submit` returns its owned `SessionOutcome`.
        let outcome = session.lock().unwrap().submit(&input);
        match outcome {
            SessionOutcome::Accepted(command) => {
                let description = describe(&command);
                println!("accepted: {description}");

                let dependency = match &command {
                    Command::Imperative(cmd) => cmd.dependency,
                    Command::Identity(_) => None,
                };

                if let Some(dependency) = dependency {
                    if let LastResult::Pending(id) = last_result {
                        resolve_branch_now(id, &mut pending, &mut session.lock().unwrap());
                        last_result = LastResult::Resolved;
                    }
                    if !session.lock().unwrap().dependency_satisfied(&dependency) {
                        println!(
                            "  skipped: WHEN {} {} was not satisfied.",
                            dependency.reference.as_str(),
                            dependency.condition.as_str()
                        );
                        continue;
                    }
                }

                let plan_outcome = xact_planner::plan(&command, session.lock().unwrap().references());
                let budget = session.lock().unwrap().resource_budget();
                let concurrent = session.lock().unwrap().schedule() == Some(PolicyOperator::Concurrently);
                dispatch(
                    plan_outcome,
                    budget,
                    description,
                    concurrent,
                    &mut session.lock().unwrap(),
                    &mut pending,
                    &mut next_branch_id,
                    &mut last_result,
                );
            }
            SessionOutcome::PolicyAccepted(stmt) => {
                println!("policy set: {}", describe_policy(&stmt));
                let already_enforced = matches!(
                    stmt.operator,
                    PolicyOperator::Concurrently | PolicyOperator::Consecutively | PolicyOperator::Spend | PolicyOperator::Save
                );
                if !already_enforced {
                    println!("  (not yet enforced — the planner/executor are not implemented yet.)");
                }
            }
            SessionOutcome::AgentAccepted(block) => {
                let description = describe_agent(&block);
                println!("agent intent accepted: {description}");
                let plan_outcome = xact_planner::plan_agent(&block, session.lock().unwrap().references());
                let budget = session.lock().unwrap().resource_budget();
                let concurrent = session.lock().unwrap().schedule() == Some(PolicyOperator::Concurrently);
                dispatch(
                    plan_outcome,
                    budget,
                    description,
                    concurrent,
                    &mut session.lock().unwrap(),
                    &mut pending,
                    &mut next_branch_id,
                    &mut last_result,
                );
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
            if let Some(dep) = &cmd.dependency {
                s.push_str(&format!(" WHEN {} {}", dep.reference.as_str(), dep.condition.as_str()));
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
