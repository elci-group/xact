//! The Xact shell: an interactive REPL and a non-interactive runner. Type
//! a `£` command, a `!` policy statement, or an `@` agent block; it is
//! validated grammatically, semantically, and against the active policy
//! (`xact-core`), planned (`xact-planner`), and actually run
//! (`xact-executor`): `£ CREATE` via `bank`, `£ SEE` via `gls`/`bat`,
//! `£ RUN` as a real process, `£ BOUND` via `bound`, `£ COPY`/`£ MOVE`
//! natively, `£ EDIT` via `oip` (ADR 0001), and `@ TELL`/`@ TEAM` against
//! real agent backends. `PASTE`, `CUT`, and `DELETE` have no execution plan
//! yet and report themselves unsupported rather than silently doing nothing.
//! [`dispatch`] is the one place `£` and `@` paths funnel through.
//!
//! Scheduling (spec section 23, Xact–Mesut Integration Phase 3): with no
//! `! CONCURRENTLY`/`! CONSECUTIVELY` stated, or under `! CONSECUTIVELY`,
//! each accepted command runs and is waited on before the next is read —
//! `A → B`. Under `! CONCURRENTLY`, subsequent commands are admitted onto
//! Mesut as independent branches; [`drain_ready`] prints each branch's real
//! Mesut lifecycle events and outcome as they arrive (Phase 4).
//!
//! Policy: `session.resource_budget()` carries `SPEND`/`SAVE` quotas
//! (real cgroup v2 limits, via `xact-resource`) and `WITHOUT`'s denied
//! capabilities (a private network namespace for spawned processes under
//! `WITHOUT network`; refused agent tiers) to every execution. Commands
//! needing a forbidden capability are already rejected by `xact-core`.
//! `PREFER`/`DODGE` are accepted as advisory, and the CLI says so.
//!
//! Dependencies (spec section 9, Phase 8): `£ RUN 'test' WHEN THAT
//! SUCCEEDS` only runs once the real outcome of whatever most recently ran
//! satisfies the condition. Under `! CONCURRENTLY` that may be an in-flight
//! branch: [`LastResult::Pending`] tracks it, and [`resolve_branch_now`]
//! blocks on that specific branch before deciding. Multi-branch fan-in and
//! recovery beyond a plain `WHEN ... FAILS` fallback are not implemented —
//! see `MESUT_INTEGRATION.md`'s Phase 8 status.
//!
//! Cancellation (Phase 8, continued): `main` installs a real `CTRL-C`
//! handler (`xact_cancel::cancel_all`) so an interrupt kills whatever's
//! running instead of the whole Xact process.
//!
//! Interactive input: [`XactValidator`] keeps Enter from submitting a draft
//! the parser considers incomplete (inserting a newline instead), so a
//! statement can span several lines; [`XactHighlighter`] paints input the
//! parser can never complete in red from the offending token onward.
//! History persists to `~/.local/share/xact/history`, driving `Ctrl-R` and
//! a `fish`-style inline hint; `--vi` switches to Vi edit mode (without the
//! live per-keystroke completion overlay built for Emacs mode).
//!
//! Non-interactive input — `xact -c '...'`, `xact script.xact`, or input
//! piped to `xact` — is one block (spec sections 17/18): [`run_batch`]
//! splits it into statements ([`split_statements`]), validates *every*
//! statement against a throwaway copy of the session first
//! ([`validate_block`]), and runs nothing at all if any is invalid. The
//! exit status is 0 when every command succeeded, 1 when one did not, and
//! 2 when the block was rejected before running.

use std::borrow::Cow;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use nu_ansi_term::{Color, Style};
use reedline::{
    default_emacs_keybindings, Completer, DefaultHinter, DescriptionMode, EditCommand, Editor, Emacs,
    FileBackedHistory, Highlighter, IdeMenu, KeyCode, KeyModifiers, Keybindings, Menu, MenuBuilder, MenuEvent, Painter,
    Prompt, PromptEditMode, PromptHistorySearch, Reedline, ReedlineEvent, ReedlineMenu, Signal, Span, StyledText,
    Suggestion, ValidationResult, Validator, Vi, HISTORY_SIZE,
};
use xact_ast::{AgentBlock, AgentClause, Command, Operand, PolicyArgs, PolicyOperator, PolicyStatement};
use xact_completion_graph::agent_model_selection;
use xact_core::{Session, SessionOutcome};
use xact_executor::{ExecutionOutcome, Pending};
use xact_parser::ParseOutcome;
use xact_planner::PlanOutcome;

/// `exit`/`quit` are handled by the REPL loop itself, not the grammar —
/// [`XactValidator`] and [`XactHighlighter`] both need to recognize them
/// as always-submittable so leaving the REPL isn't blocked behind syntax
/// gating meant for the `£`/`!`/`@` language.
fn is_exit_word(trimmed: &str) -> bool {
    trimmed == "exit" || trimmed == "quit"
}

/// The completion menu's name, shared between the keybindings that open it
/// and the `ReedlineMenu` that's registered under it.
const COMPLETION_MENU: &str = "completion_menu";

/// The forced model picker's name — the blue-background menu that opens
/// the moment `@ TELL`'s space delimiter lands (see [`char_binding`] and
/// [`build_line_editor`]).
const MODEL_MENU: &str = "model_menu";

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
        let word_so_far = &typed[start..];
        let session = self.session.lock().unwrap();
        session
            .complete(typed)
            .into_iter()
            // A candidate that's already exactly what's been typed (and
            // doesn't invite more typing, like a directory's trailing
            // `/`) has nothing left to offer — keeping it around just
            // means the menu stays active with a no-op entry once a term
            // is finished, which is exactly what leaves Enter accepting
            // that entry (see `char_binding`'s doc comment) instead of
            // falling through to submit. Static keyword candidates never
            // reach this (the parser already stops suggesting a verb
            // once it's fully typed and consumed — see `xact-completion`),
            // but a real, dynamically-resolved filesystem/binary name has
            // no such self-narrowing, so this is where it gets one.
            .filter(|value| value.ends_with('/') || value != word_so_far)
            .map(|value| {
                // Placeholders (`<path>`, `'...'`) and a directory entry
                // that already ends in `/` invite more typing right where
                // the cursor lands; every other candidate (a keyword, a
                // resolved binary name, a resolved file) is a finished
                // token, so a trailing space after accepting it is the
                // useful default.
                let append_whitespace = !value.starts_with('<') && !value.starts_with('\'') && !value.ends_with('/');
                let description = keyword_description(&value).map(str::to_string);
                Suggestion {
                    value,
                    description,
                    style: None,
                    extra: None,
                    span: Span { start, end: pos },
                    append_whitespace,
                }
            })
            .collect()
    }
}

/// Bridges `reedline`'s [`Completer`] trait to
/// [`xact_completion_graph::agent_model_selection`] — the candidate
/// source for the forced model picker ([`MODEL_MENU`]) and nothing else.
/// Where [`XactCompleter`] answers "what's a valid continuation of this
/// draft" for the general completion menu, this answers exactly one
/// question: if the cursor is in `@ TELL`'s target slot, which models can
/// it be? The answer is the full deterministic list — installed Ollama
/// models plus every key-provisioned provider's models
/// (`xact_tell::selectable_models`) — the instant the slot is entered,
/// narrowing as a partial target is typed, and empty at every other
/// grammar position. That emptiness is load-bearing: it's what lets the
/// space keybinding fire [`MODEL_MENU`] on *every* space keystroke and
/// have the picker simply not exist anywhere but right after `@ TELL`
/// (see [`SelfHidingMenu`]).
struct ModelMenuCompleter;

impl Completer for ModelMenuCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let typed = &line[..pos];
        let start = trailing_word_start(typed);
        agent_model_selection(typed)
            .into_iter()
            .map(|value| Suggestion {
                value,
                description: None,
                style: None,
                extra: None,
                span: Span { start, end: pos },
                // A model name is always a finished token; the trailing
                // space moves the draft on to `TELL`'s clause position.
                append_whitespace: true,
            })
            .collect()
    }
}

/// A menu that gets out of the way the moment it has nothing to show.
///
/// reedline's own menus stay "active" once opened even when their
/// completer returns zero values: an active-but-empty menu renders its
/// "no records" box, swallows the next Enter just to close itself, and
/// blocks any other menu from activating (the `Menu` event only fires
/// when no menu is active). This wrapper makes emptiness equivalent to
/// closed: every value refresh ([`Menu::update_values`],
/// [`Menu::update_working_details`], [`Menu::can_partially_complete`])
/// funnels through [`SelfHidingMenu::refresh`], and if the wrapped menu
/// comes back with no values, it is deactivated right there — inside the
/// same repaint cycle that refreshed it, so the engine's subsequent
/// "is any menu active?" checks (which gate both rendering and the
/// Enter-eats-a-menu-press branch) see a closed menu, never an empty
/// open one. `is_active` stays plain delegation, and activation still
/// triggers the refresh (the wrapped menu reports active until its
/// values have actually been pulled), so a menu dismissed by emptiness
/// at one cursor position reopens fine at the next `Menu` event.
///
/// This is what makes [`MODEL_MENU`]'s unconditional-on-space activation
/// safe: at every position that isn't `@ TELL`'s target slot the picker
/// activates, finds nothing, and vanishes — indistinguishable from never
/// having opened. Wrapping the general completion menu in the same thing
/// additionally retires the old "empty menu eats one Enter" hazard
/// documented on [`char_binding`] for good, rather than papering over it
/// with `Esc`-on-delimiter alone.
struct SelfHidingMenu {
    inner: Box<dyn Menu>,
}

impl SelfHidingMenu {
    fn new(inner: Box<dyn Menu>) -> Self {
        Self { inner }
    }

    /// Pulls fresh values into the wrapped menu, then shuts the menu off
    /// if there was nothing to pull.
    fn refresh(&mut self, editor: &mut Editor, completer: &mut dyn Completer) {
        self.inner.update_values(editor, completer);
        self.deactivate_if_empty();
    }

    fn deactivate_if_empty(&mut self) {
        if self.inner.get_values().is_empty() {
            self.inner.menu_event(MenuEvent::Deactivate);
        }
    }
}

impl Menu for SelfHidingMenu {
    // reedline 0.38 doesn't re-export `MenuSettings`, so `settings` itself
    // can't be delegated; `name`/`indicator` are the only trait methods
    // that read it, so delegating those two covers everything.
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn indicator(&self) -> &str {
        self.inner.indicator()
    }

    fn is_active(&self) -> bool {
        self.inner.is_active()
    }

    fn menu_event(&mut self, event: MenuEvent) {
        self.inner.menu_event(event);
    }

    fn can_quick_complete(&self) -> bool {
        self.inner.can_quick_complete()
    }

    fn can_partially_complete(
        &mut self,
        values_updated: bool,
        editor: &mut Editor,
        completer: &mut dyn Completer,
    ) -> bool {
        let completed = self.inner.can_partially_complete(values_updated, editor, completer);
        self.deactivate_if_empty();
        completed
    }

    fn update_values(&mut self, editor: &mut Editor, completer: &mut dyn Completer) {
        self.refresh(editor, completer);
    }

    fn update_working_details(&mut self, editor: &mut Editor, completer: &mut dyn Completer, painter: &Painter) {
        self.inner.update_working_details(editor, completer, painter);
        self.deactivate_if_empty();
    }

    fn replace_in_buffer(&self, editor: &mut Editor) {
        self.inner.replace_in_buffer(editor);
    }

    fn menu_required_lines(&self, terminal_columns: u16) -> u16 {
        self.inner.menu_required_lines(terminal_columns)
    }

    fn menu_string(&self, available_lines: u16, use_ansi_coloring: bool) -> String {
        self.inner.menu_string(available_lines, use_ansi_coloring)
    }

    fn min_rows(&self) -> u16 {
        self.inner.min_rows()
    }

    fn get_values(&self) -> &[Suggestion] {
        self.inner.get_values()
    }

    fn set_cursor_pos(&mut self, pos: (u16, u16)) {
        self.inner.set_cursor_pos(pos);
    }
}

/// Paints a draft red from the point it can never be completed into a
/// valid program without changing what's already typed — the same
/// [`ParseOutcome::Invalid`] a submitted line would be rejected with, just
/// surfaced live instead of only after Enter. A merely-incomplete prefix
/// (still typing a token, or mid `@` block) gets no red at all: it isn't
/// wrong, just unfinished, and [`XactValidator`] is what keeps Enter from
/// submitting it early. Stateless — this is pure syntax, the same
/// `xact-parser` grammar check `xact-core::Session::submit` runs, not the
/// ownership/reference/policy semantics layered on top of it.
///
/// One deliberate splash of color besides the error red: `@ TELL`'s
/// target — the model, whether picked from the forced blue menu or typed
/// by hand — renders inline in blue font for as long as it's on the line
/// (`xact_completion_graph::tell_target_span`), the same color family as
/// the menu it was selected from.
struct XactHighlighter;

impl Highlighter for XactHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();
        let trimmed = line.trim();
        if trimmed.is_empty() || is_exit_word(trimmed) {
            styled.push((Style::default(), line.to_string()));
            return styled;
        }
        styled.push((Style::default(), line.to_string()));
        if let Some(span) = xact_completion_graph::tell_target_span(line) {
            styled.style_range(span.start, span.end, Style::new().fg(Color::Blue));
        }
        if let ParseOutcome::Invalid(diag) = xact_parser::parse_line(line) {
            let start = diag.span.start.min(line.len());
            styled.style_range(start, line.len(), Style::new().fg(Color::Red));
        }
        styled
    }
}

/// Gates Enter behind runnability instead of grammaticality-on-the-spot:
/// reedline calls this on every Enter press and only actually submits the
/// buffer when it reports [`ValidationResult::Complete`] — anything else
/// inserts a newline instead (see `reedline::Reedline::handle_editor_event`'s
/// `ReedlineEvent::Enter` arm), which is exactly what lets a `£`/`!`/`@`
/// block span several lines under plain Enter, no separate continuation
/// prompt needed. `Incomplete` covers both
/// [`ParseOutcome::Incomplete`] (a valid prefix, not done yet) and
/// [`ParseOutcome::Invalid`] (wrong so far — [`XactHighlighter`] is what
/// tells the user that, not a refusal to keep editing): either way, Enter
/// must not submit, and the buffer stays intact for the user to fix or
/// finish in place rather than being discarded.
struct XactValidator;

impl Validator for XactValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_exit_word(trimmed) {
            return ValidationResult::Complete;
        }
        match xact_parser::parse_line(line) {
            ParseOutcome::Complete(_) => ValidationResult::Complete,
            ParseOutcome::Incomplete(_) | ParseOutcome::Invalid(_) => ValidationResult::Incomplete,
        }
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

/// A one-line description for a candidate, straight out of the language
/// spec (`AGENTS.md`) — real, not invented for the sake of a description
/// field. Scoped to the fixed keyword/placeholder vocabulary only: a
/// dynamic candidate (a real path or binary name from `xact-resolve`)
/// isn't in this table and gets no description here, since inventing one
/// would mean guessing at metadata this crate was never given (rather
/// than plumbing e.g. file kind through just for a completion hint).
fn keyword_description(value: &str) -> Option<&'static str> {
    Some(match value {
        "£" => "Start an imperative command",
        "!" => "Start a policy statement",
        "@" => "Start an agent block",
        "SEE" => "Show a file or directory (gls/bat)",
        "EDIT" => "Replace a file's content with the clipboard (backup kept)",
        "MOVE" => "Move a resource to a destination",
        "COPY" => "Copy a resource to a destination",
        "PASTE" => "Paste a resource to a destination",
        "CUT" => "Cut a resource to a destination",
        "DELETE" => "Delete a resource",
        "RUN" => "Execute a program or command line",
        "CREATE" => "Establish a file or directory (bank)",
        "BOUND" => "Aggregate a source set (bound)",
        "MY" => "Your own resource — a bare path defaults to ~/",
        "OUR" => "A shared resource, from Xact's system directories",
        "THEIR" => "A resource scoped to a declared identity",
        "THIS" | "THAT" => "The most recently established or viewed resource",
        "WITH" => "Require a capability",
        "WITHOUT" => "Forbid a capability",
        "PREFER" => "Softly prefer a capability",
        "DODGE" => "Softly avoid a capability",
        "SPEND" => "Cap real resource usage (cgroup-enforced)",
        "SAVE" => "Reserve resource headroom (cgroup-enforced)",
        "CONCURRENTLY" => "Run subsequent commands as parallel branches",
        "CONSECUTIVELY" => "Run subsequent commands one at a time (default)",
        "WHEN" => "Gate this command on a prior outcome",
        "SUCCEEDS" => "...if the referenced outcome succeeded",
        "FAILS" => "...if the referenced outcome failed",
        "TELL" => "Ask a local model to do one task",
        "TEAM" => "Run a comma-separated roster of agents concurrently",
        "BE" => "Set the agent's persona",
        "READING" => "Give the agent a resource to read",
        "POPULATING" => "Give the agent a destination to write",
        "THINK" => "Set the agent's reasoning budget",
        "<path>" => "A file or directory path",
        "'...'" => "A quoted string",
        "<percent>%<resource>" => "e.g. 40%CPU",
        _ => return None,
    })
}

/// `xact> ` on the buffer's first visual line; reedline paints
/// [`render_prompt_multiline_indicator`](Prompt::render_prompt_multiline_indicator)
/// on every line after that — which, now that [`XactValidator`] lets a
/// draft's Enter presses grow the buffer across real newlines instead of
/// submitting early, is every continuation line of a still-incomplete
/// `£`/`!`/`@` block. `  ...> ` matches the exact string the old
/// two-state prompt (and the bare `stdin.read_line` REPL before it) used
/// for that same purpose, so the session looks the same at rest even
/// though reedline, not this struct, now decides when it's shown.
struct XactPrompt;

impl XactPrompt {
    fn new() -> Self {
        Self
    }
}

impl Prompt for XactPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed("xact> ")
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _edit_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("  ...> ")
    }

    fn render_prompt_history_search_indicator(&self, search: PromptHistorySearch) -> Cow<'_, str> {
        Cow::Owned(format!("(search: {}) ", search.term))
    }
}

/// A delimiter that closes off whatever term precedes it: a space ends any
/// bare keyword/path/percent token, and a quote closes a `'...'`/`"..."`
/// string argument. Typing one of these always dismisses whatever
/// completion menu is currently open *before* the character lands (see
/// [`char_binding`]) — the term just finished is implicitly confirmed by
/// having been typed out and delimited, exactly like accepting it from the
/// menu would, just without needing Enter or Tab for it. This is what
/// keeps the menu from sitting open, stale, once a token's actually done.
fn is_confirming_delimiter(c: char) -> bool {
    c == ' ' || c == '\'' || c == '"'
}

/// The keybinding for inserting `c`. For most characters: insert it, then
/// open/refresh the completion menu for whatever comes next. An
/// [`is_confirming_delimiter`] character is different — see below.
fn char_binding(c: char) -> ReedlineEvent {
    if is_confirming_delimiter(c) {
        // Space is the one delimiter that can enter a slot something is
        // waiting to force: the instant the space after `@ TELL` lands,
        // the model picker ([`MODEL_MENU`]) must open with the full
        // deterministic list — installed Ollama models plus available
        // provider models — before the user has typed anything at all.
        // So space sends `Menu(MODEL_MENU)` right after the insert; at
        // every other position that menu's completer has nothing
        // (`agent_model_selection` is empty outside `TELL`'s target slot)
        // and it deactivates itself within the same paint cycle
        // ([`SelfHidingMenu`]), leaving the line exactly as menu-free as
        // the old `Esc`-only binding left it — including no active-but-
        // empty menu eating the next Enter. A closing quote never enters
        // a new slot anything could complete, so quotes stay plain
        // delimiters (insert only, menu closed).
        if c == ' ' {
            ReedlineEvent::Multiple(vec![
                ReedlineEvent::Esc,
                ReedlineEvent::Edit(vec![EditCommand::InsertChar(c)]),
                ReedlineEvent::Menu(MODEL_MENU.to_string()),
            ])
        } else {
            ReedlineEvent::Multiple(vec![ReedlineEvent::Esc, ReedlineEvent::Edit(vec![EditCommand::InsertChar(c)])])
        }
    } else {
        ReedlineEvent::Multiple(vec![
            ReedlineEvent::Edit(vec![EditCommand::InsertChar(c)]),
            ReedlineEvent::Menu(COMPLETION_MENU.to_string()),
        ])
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
        kb.add_binding(KeyModifiers::NONE, KeyCode::Char(c), char_binding(c));
        // crossterm/terminal-dependent: a physically shifted letter can be
        // reported either as its own char under KeyModifiers::NONE (bound
        // just above) or as the lowercase char under KeyModifiers::SHIFT —
        // Emacs::parse_event looks up the lowercased char for any non-NONE
        // modifier, so both real reporting conventions are covered.
        if c.is_ascii_lowercase() {
            kb.add_binding(KeyModifiers::SHIFT, KeyCode::Char(c), char_binding(c.to_ascii_uppercase()));
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

    // Plain Enter is deliberately left at `default_emacs_keybindings`'s own
    // `Enter -> Enter` binding — no override here. `Reedline` itself
    // branches on live menu state at the moment Enter is actually
    // pressed (something a static `Keybindings` entry can't do): with a
    // completion menu open, Enter inserts the currently-selected
    // candidate and closes the menu instead of submitting (confirmed in
    // reedline's source, the `Enter | Submit | SubmitOrNewline if
    // menus.any(is_active)` arm of `Reedline::handle_editor_event`) — that
    // *is* "confirm the option Enter has selected," not a bug to route
    // around. Once no menu is left active, that same key falls through to
    // plain `ReedlineEvent::Enter`, which runs through `XactValidator`
    // (installed on the editor in `build_line_editor`): submits once the
    // buffer already parses as a complete `£`/`!`/`@` program, inserts a
    // newline otherwise — see `XactValidator`'s and `XactHighlighter`'s
    // doc comments for why an invalid draft gets to stay in the buffer
    // (styled red) rather than being bounced the instant Enter is hit.
    // What keeps a *finished* term's menu from lingering active and
    // eating that first Enter is [`char_binding`]'s `Esc` on every
    // delimiter keystroke (space, `'`, `"`) — typing a term fully and
    // then its natural delimiter auto-confirms it exactly the way
    // pressing Enter on a highlighted match would, no extra keypress
    // needed. This doesn't cover every case (an unquoted, unterminated
    // final token — e.g. a bare path with no trailing space — may still
    // have a live, non-empty menu of real matches when Enter is pressed,
    // costing one extra press to dismiss it before a second Enter
    // submits); that's an accepted, minor edge relative to `Enter` doing
    // its actual job of confirming a selected completion.
    kb.add_binding(KeyModifiers::NONE, KeyCode::Enter, ReedlineEvent::Enter);

    // Shift+Enter always inserts a literal newline, bypassing
    // `XactValidator` entirely (it's a plain `Edit`, not an `Enter`/
    // `Submit` event) — the explicit "add a line break right here"
    // escape hatch even when the draft would already validate as
    // complete and plain Enter would submit it. Whether a terminal
    // actually reports Shift as a distinct modifier on Enter (rather
    // than an identical `\r`/`\n`) depends on that terminal's keyboard
    // protocol support, same caveat as the shifted-letter bindings above.
    kb.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::Enter,
        ReedlineEvent::Multiple(vec![ReedlineEvent::Esc, ReedlineEvent::Edit(vec![EditCommand::InsertNewline])]),
    );
}

/// `~/.local/share/xact/history` — the same `~/.local/share/<tool>/...`
/// convention this project's sibling `gls` already uses for its own
/// persisted state. `None` (no `$HOME`) just means this run's history
/// stays in-memory-only rather than failing to start.
fn history_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/xact/history"))
}

/// Builds the live line editor: `XactCompleter` (backed by `session`) as
/// the candidate source, an `IdeMenu` dropdown wired to open/refresh on
/// every keystroke (see [`add_completion_keybindings`]) rather than only
/// on an explicit `Tab`, `XactHighlighter` painting an unrecoverable draft
/// red instead of it just being bounced on Enter, and `XactValidator`
/// gating Enter itself behind the same syntax check so a `£`/`!`/`@` block
/// can only ever be submitted once it's actually runnable. History
/// persists to [`history_path`] (real `FileBackedHistory` — survives
/// across sessions, drives `Ctrl-R` and the inline `DefaultHinter`
/// suggestion) unless `$HOME` is unset, in which case the session still
/// runs with reedline's in-memory default rather than failing to start
/// over a missing history file.
///
/// A second, dedicated menu is registered alongside the general one: the
/// forced model picker ([`MODEL_MENU`]), with its own completer
/// ([`ModelMenuCompleter`]) and a blue background on every row (white
/// bold text marks the selection). It opens the moment `@ TELL`'s space
/// delimiter lands — see [`char_binding`] — offering the full
/// deterministic list of installed Ollama + available provider models,
/// and narrows live as a target is typed. Both menus sit inside a
/// [`SelfHidingMenu`], so whichever one activates with nothing to offer
/// simply isn't there.
fn build_line_editor(session: Arc<Mutex<Session>>, vi_mode: bool) -> Reedline {
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

    let model_menu = Box::new(
        IdeMenu::default()
            .with_name(MODEL_MENU)
            .with_min_completion_width(0)
            .with_max_completion_width(60)
            .with_max_completion_height(u16::MAX)
            .with_padding(1)
            .with_cursor_offset(0)
            .with_correct_cursor_pos(false)
            .with_default_border()
            .with_text_style(Style::new().on(Color::Blue))
            .with_selected_text_style(Style::new().on(Color::Blue).fg(Color::White).bold())
            .with_match_text_style(Style::new().on(Color::Blue).fg(Color::White).bold())
            .with_selected_match_text_style(Style::new().on(Color::Blue).fg(Color::White).bold()),
    );

    let mut editor = Reedline::create()
        .with_completer(completer)
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(SelfHidingMenu::new(menu))))
        .with_menu(ReedlineMenu::WithCompleter {
            menu: Box::new(SelfHidingMenu::new(model_menu)),
            completer: Box::new(ModelMenuCompleter),
        })
        .with_hinter(Box::new(DefaultHinter::default()))
        .with_highlighter(Box::new(XactHighlighter))
        .with_validator(Box::new(XactValidator));

    if let Some(history) = history_path().and_then(|path| FileBackedHistory::with_file(HISTORY_SIZE, path).ok()) {
        editor = editor.with_history(Box::new(history));
    }

    if vi_mode {
        editor.with_edit_mode(Box::new(Vi::default()))
    } else {
        let mut keybindings = default_emacs_keybindings();
        add_completion_keybindings(&mut keybindings);
        editor.with_edit_mode(Box::new(Emacs::new(keybindings)))
    }
}

/// A concurrent branch admitted under `! CONCURRENTLY` but not yet
/// joined, tagged with a stable id (so a later `WHEN` clause can block on
/// *this specific* branch — see [`LastResult`]) and the description
/// printed for `£ ...`/`@ ...` when it was queued.
type PendingBranches = Vec<(u64, String, Pending)>;

/// What the most recent command's outcome currently is, for `WHEN
/// THIS/THAT` clauses (Xact–Mesut Integration Phase 8): one session-wide
/// "most recent outcome" slot (a command has one outcome, even though
/// `THIS`/`THAT` name different objects — see `xact-reference`).
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

/// Everything one run of the REPL or a batch tracks between commands.
struct Runtime {
    pending: PendingBranches,
    next_branch_id: u64,
    last_result: LastResult,
    /// How many commands did not succeed (rejected, unsupported, failed,
    /// or a nonzero exit) — batch mode's exit status.
    failures: usize,
}

impl Runtime {
    fn new() -> Self {
        Self { pending: Vec::new(), next_branch_id: 0, last_result: LastResult::Resolved, failures: 0 }
    }

    fn record(&mut self, session: &mut Session, outcome: &ExecutionOutcome) {
        let succeeded = outcome_succeeded(outcome);
        session.record_outcome(succeeded);
        if !succeeded {
            self.failures += 1;
        }
    }
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
fn drain_ready(rt: &mut Runtime, session: &mut Session) {
    let mut finished = Vec::new();
    rt.pending.retain_mut(|(id, description, branch)| {
        for event in branch.drain_events() {
            print_lifecycle_event(description, &event);
        }
        match branch.try_join() {
            Some(outcome) => {
                for event in branch.drain_events() {
                    print_lifecycle_event(description, &event);
                }
                print_outcome(Some(description), &outcome);
                finished.push((*id, outcome));
                false
            }
            None => true,
        }
    });
    for (id, outcome) in finished {
        rt.record(session, &outcome);
        if rt.last_result == LastResult::Pending(id) {
            rt.last_result = LastResult::Resolved;
        }
    }
}

/// Blocks on one specific branch (identified by `id`) and records its
/// real outcome — the `WHEN` clause gate: a dependent command must not
/// even be planned until the branch it depends on has genuinely finished.
/// Branches other than `id` are left running untouched. A missing `id`
/// (should not happen — `last_result` only ever names a branch that was
/// actually queued) is a no-op rather than a panic.
fn resolve_branch_now(id: u64, rt: &mut Runtime, session: &mut Session) {
    let Some(pos) = rt.pending.iter().position(|(pid, _, _)| *pid == id) else {
        return;
    };
    let (_, description, mut branch) = rt.pending.remove(pos);
    let outcome = block_until_resolved(&description, &mut branch);
    print_outcome(Some(&description), &outcome);
    rt.record(session, &outcome);
}

/// Blocks until every remaining branch has finished — used at session end
/// so nothing started under `! CONCURRENTLY` is left unreported.
fn join_all(rt: &mut Runtime, session: &mut Session) {
    for (_, description, mut branch) in std::mem::take(&mut rt.pending) {
        let outcome = block_until_resolved(&description, &mut branch);
        print_outcome(Some(&description), &outcome);
        rt.record(session, &outcome);
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
        | ExecutionOutcome::Edited { .. }
        | ExecutionOutcome::Copied { .. }
        | ExecutionOutcome::Moved { .. }
        | ExecutionOutcome::Told { .. }
        | ExecutionOutcome::Teamed { .. } => true,
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
        ExecutionOutcome::Edited { path, backup, preview } => {
            for line in preview {
                println!("{prefix}oip: {line}");
            }
            match backup {
                Some(backup) => {
                    println!("{prefix}edited {} (previous content saved to {})", path.display(), backup.display())
                }
                None => println!("{prefix}edited {} — oip reported success but left no backup", path.display()),
            }
        }
        ExecutionOutcome::Copied { source, destination } => {
            println!("{prefix}copied {} to {}", source.display(), destination.display())
        }
        ExecutionOutcome::Moved { source, destination } => {
            println!("{prefix}moved {} to {}", source.display(), destination.display())
        }
        ExecutionOutcome::Told { model, response, populated } => {
            println!("{prefix}{model}: {response}");
            if let Some(populated) = populated {
                println!("{prefix}(also written to {})", populated.display());
            }
        }
        ExecutionOutcome::Teamed { members, report, populated } => {
            println!("{prefix}team [{}]:\n{report}", members.join(", "));
            if let Some(populated) = populated {
                println!("{prefix}(team report also written to {})", populated.display());
            }
        }
        ExecutionOutcome::Failed { message } => {
            println!("{prefix}execution failed: {message}");
        }
    }
}

/// Plans and dispatches `plan_outcome` — shared by `£` commands and
/// `@` blocks, which differ only in how they got a `PlanOutcome`.
/// Updates `session`'s recorded outcome and `rt.last_result` for anything
/// that actually ran (or failed to even submit); a plan that turned out
/// `Unsupported` ran nothing, but still counts as a failure for batch
/// mode's exit status.
fn dispatch(
    plan_outcome: PlanOutcome,
    budget: xact_ast::ResourceBudget,
    description: String,
    concurrent: bool,
    session: &mut Session,
    rt: &mut Runtime,
) {
    let plan = match plan_outcome {
        PlanOutcome::Plan(plan) => plan,
        PlanOutcome::Unsupported(reason) => {
            println!("  {reason}");
            rt.failures += 1;
            return;
        }
    };

    if concurrent {
        match xact_executor::execute_concurrent(plan, budget) {
            Ok(branch) => {
                let id = rt.next_branch_id;
                rt.next_branch_id += 1;
                println!("  queued as an independent branch ({} in flight)", rt.pending.len() + 1);
                rt.pending.push((id, description, branch));
                rt.last_result = LastResult::Pending(id);
            }
            Err(outcome) => {
                print_outcome(None, &outcome);
                rt.record(session, &outcome);
                rt.last_result = LastResult::Resolved;
            }
        }
    } else {
        let outcome = xact_executor::execute(plan, budget);
        print_outcome(None, &outcome);
        rt.record(session, &outcome);
        rt.last_result = LastResult::Resolved;
    }
}

/// Splits a non-interactive program into its statements. A statement
/// starts at a line whose first non-blank character is `£`, `!`, or `@`
/// (or at any non-blank line when no statement is open yet, so stray text
/// is reported rather than swallowed); every other line continues the
/// statement above it, so a `£`/`!`/`@` statement may span lines exactly as
/// it can at the interactive prompt. A line reading `exit` or `quit` ends
/// the program. Returns `(first line number, statement text)` pairs.
fn split_statements(text: &str) -> Vec<(usize, String)> {
    let mut statements: Vec<(usize, String)> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if is_exit_word(trimmed) {
            break;
        }
        let starts_statement = trimmed.starts_with(['£', '!', '@']);
        match statements.last_mut() {
            Some((_, current)) if !starts_statement => {
                current.push('\n');
                current.push_str(line);
            }
            _ if trimmed.is_empty() => {}
            _ => statements.push((index + 1, line.to_string())),
        }
    }
    for (_, statement) in &mut statements {
        let end = statement.trim_end().len();
        statement.truncate(end);
    }
    statements
}

/// Validates every statement of a block against a throwaway copy of
/// `session`, without planning or executing anything (spec section 18: an
/// invalid block never executes, not even its valid prefix). Each
/// statement is checked against the state the ones before it would
/// establish, exactly as the real run will see it. Returns every problem
/// found, each with the line it starts on.
fn validate_block(session: &Session, statements: &[(usize, String)]) -> Vec<(usize, String)> {
    let mut dry_run = session.clone();
    let mut problems = Vec::new();
    for (line, statement) in statements {
        match dry_run.submit(statement) {
            SessionOutcome::Accepted(..) | SessionOutcome::PolicyAccepted(_) | SessionOutcome::AgentAccepted(_) => {}
            SessionOutcome::Incomplete(diag) => problems.push((*line, format!("incomplete statement — {diag}"))),
            SessionOutcome::Rejected(diagnostics) => {
                problems.extend(diagnostics.into_iter().map(|diag| (*line, diag.to_string())));
            }
        }
    }
    problems
}

/// Submits one complete statement through the real validate → plan →
/// dispatch pipeline and prints the outcome lines. Bound to a plain `let`
/// rather than matched inline: `match` extends a scrutinee's temporaries
/// across every arm, so an inline `match session.lock().unwrap().submit(..)`
/// would hold the lock for the whole match — including the arms below that
/// lock `session` again, which would deadlock (a `Mutex` isn't reentrant).
fn process_submitted_input(input: &str, session: &Arc<Mutex<Session>>, rt: &mut Runtime) {
    let outcome = session.lock().unwrap().submit(input);
    match outcome {
        SessionOutcome::Accepted(command, references) => {
            let description = describe(&command);
            println!("accepted: {description}");

            let dependency = match &command {
                Command::Imperative(cmd) => cmd.dependency,
                Command::Identity(_) => None,
            };

            if let Some(dependency) = dependency {
                if let LastResult::Pending(id) = rt.last_result {
                    resolve_branch_now(id, rt, &mut session.lock().unwrap());
                    rt.last_result = LastResult::Resolved;
                }
                if !session.lock().unwrap().dependency_satisfied(&dependency) {
                    println!(
                        "  skipped: WHEN {} {} was not satisfied.",
                        dependency.reference.as_str(),
                        dependency.condition.as_str()
                    );
                    return;
                }
            }

            let (plan_outcome, budget, concurrent) = {
                let session = session.lock().unwrap();
                (
                    xact_planner::plan(&command, &references, session.owner_roots()),
                    session.resource_budget(),
                    session.schedule() == Some(PolicyOperator::Concurrently),
                )
            };
            dispatch(plan_outcome, budget, description, concurrent, &mut session.lock().unwrap(), rt);
        }
        SessionOutcome::PolicyAccepted(stmt) => {
            println!("policy set: {}", describe_policy(&stmt));
            match stmt.operator {
                PolicyOperator::With => println!("  (checked: available on this host now.)"),
                PolicyOperator::Prefer | PolicyOperator::Dodge => println!(
                    "  (advisory: no operation Xact plans today has an alternative for this preference to choose between.)"
                ),
                _ => {}
            }
        }
        SessionOutcome::AgentAccepted(block) => {
            let description = describe_agent(&block);
            println!("agent intent accepted: {description}");
            let (plan_outcome, budget, concurrent) = {
                let session = session.lock().unwrap();
                (
                    xact_planner::plan_agent(&block, session.references(), session.owner_roots()),
                    session.resource_budget(),
                    session.schedule() == Some(PolicyOperator::Concurrently),
                )
            };
            dispatch(plan_outcome, budget, description, concurrent, &mut session.lock().unwrap(), rt);
        }
        SessionOutcome::Incomplete(diag) => {
            print!("{diag}");
            rt.failures += 1;
        }
        SessionOutcome::Rejected(diagnostics) => {
            for diag in diagnostics {
                print!("{diag}");
            }
            rt.failures += 1;
        }
    }
}

/// The real interactive REPL: a live `reedline` editor with persistent
/// history, live per-keystroke completion, red-on-invalid highlighting,
/// and Enter gated on runnability (see [`build_line_editor`]). Because
/// `XactValidator` already keeps Enter from submitting anything short of a
/// complete `£`/`!`/`@` program — inserting a newline instead, however
/// many times that takes — `line` here is always either empty or already
/// whole: no separate continuation-reading step like [`run_batch`] needs.
fn run_interactive(vi_mode: bool) {
    println!(
        "xact {} — type a £ command, a ! policy statement, an @ agent block, or 'exit'.",
        env!("CARGO_PKG_VERSION")
    );
    println!("Suggestions narrow live as you type; invalid input is highlighted in red;");
    println!("Enter submits once the line is runnable, Shift+Enter always adds a line break.");

    // Starts the agent-model catalog refresh in the background, so the
    // first `@ TELL` menu is usually already populated without completion
    // ever waiting on a provider.
    xact_core::prewarm_completion();

    // `Session` is shared (not owned outright) by this loop: `XactCompleter`
    // (inside `line_editor`) needs read access to the same live policy
    // state (spec section 16 — already-established singleton operators
    // drop out of the suggestion list) that this loop mutates between
    // lines. See `XactCompleter`'s doc comment.
    let session = Arc::new(Mutex::new(Session::new()));
    let mut line_editor = build_line_editor(Arc::clone(&session), vi_mode);
    let prompt = XactPrompt::new();
    let mut rt = Runtime::new();

    loop {
        drain_ready(&mut rt, &mut session.lock().unwrap());

        let line = match line_editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => buffer.trim_end().to_string(),
            Ok(Signal::CtrlC) => continue,
            Ok(Signal::CtrlD) | Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }
        if is_exit_word(&line) {
            break;
        }

        process_submitted_input(&line, &session, &mut rt);
    }
    join_all(&mut rt, &mut session.lock().unwrap());
}

/// Exit status when a block was rejected before anything ran.
const EXIT_INVALID: i32 = 2;
/// Exit status when the block ran but some command did not succeed.
const EXIT_FAILED: i32 = 1;

/// Runs a whole non-interactive program (`-c`, a script file, or piped
/// stdin) as one block: every statement is validated first (see
/// [`validate_block`]) and nothing runs unless all of them pass. It then
/// runs through the same `Session::submit` → plan → dispatch pipeline the
/// REPL uses. Returns the process exit status: 0 when every command
/// succeeded, [`EXIT_FAILED`] when any did not (a `WHEN` skip is control
/// flow, not a failure), [`EXIT_INVALID`] when the block was rejected.
fn run_batch(text: &str) -> i32 {
    let statements = split_statements(text);
    let session = Arc::new(Mutex::new(Session::new()));

    let problems = validate_block(&session.lock().unwrap(), &statements);
    if !problems.is_empty() {
        eprintln!("xact: nothing was run — {} problem(s) in this block:", problems.len());
        for (line, problem) in problems {
            eprintln!("line {line}: {}", problem.trim_end());
        }
        return EXIT_INVALID;
    }

    let mut rt = Runtime::new();
    for (_, statement) in &statements {
        drain_ready(&mut rt, &mut session.lock().unwrap());
        process_submitted_input(statement, &session, &mut rt);
    }
    join_all(&mut rt, &mut session.lock().unwrap());

    if rt.failures == 0 {
        0
    } else {
        EXIT_FAILED
    }
}

/// What to run and how, from `argv`: `xact` alone starts the interactive
/// REPL; `xact -c '£ SEE MY ~/'` runs one (possibly multi-line, if the
/// argument itself contains real newlines) input non-interactively;
/// `xact path/to/script` reads and runs a file's lines the same way.
/// `--vi` only affects the interactive REPL's edit mode.
enum RunMode {
    Interactive { vi_mode: bool },
    Command(String),
    File(PathBuf),
}

fn parse_args() -> RunMode {
    let mut vi_mode = false;
    let mut run: Option<RunMode> = None;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--vi" => vi_mode = true,
            "-c" => {
                let command = args.next().unwrap_or_else(|| {
                    eprintln!("xact: -c requires a command string");
                    std::process::exit(2);
                });
                run = Some(RunMode::Command(command));
            }
            other => run = Some(RunMode::File(PathBuf::from(other))),
        }
    }

    match run {
        Some(mode) => mode,
        None => RunMode::Interactive { vi_mode },
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
    // a signal-terminated status, so neither run mode below needs any
    // special handling for this at all — both already treat that as an
    // ordinary (if unsuccessful) outcome.
    ctrlc::set_handler(xact_cancel::cancel_all).expect("failed to install CTRL-C handler");

    let status = match parse_args() {
        // No arguments but input piped in: run it as a script rather than
        // starting a line editor that would silently ignore it.
        RunMode::Interactive { .. } if !std::io::stdin().is_terminal() => {
            let mut text = String::new();
            if let Err(err) = std::io::stdin().read_to_string(&mut text) {
                eprintln!("xact: cannot read standard input: {err}");
                std::process::exit(EXIT_FAILED);
            }
            run_batch(&text)
        }
        RunMode::Interactive { vi_mode } => {
            run_interactive(vi_mode);
            0
        }
        RunMode::Command(command) => run_batch(&command),
        RunMode::File(path) => {
            let content = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                eprintln!("xact: cannot read {}: {e}", path.display());
                std::process::exit(EXIT_FAILED);
            });
            run_batch(&content)
        }
    };
    std::process::exit(status);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_start_at_a_sigil_and_continue_otherwise() {
        let text = "! SAVE 20%CPU\n\n£ COPY MY /tmp/a\n    to MY /tmp/b\n@ TELL 'm'\n  \"say hi\"\nexit\n£ RUN 'never'";
        let statements = split_statements(text);
        assert_eq!(
            statements,
            vec![
                (1, "! SAVE 20%CPU".to_string()),
                (3, "£ COPY MY /tmp/a\n    to MY /tmp/b".to_string()),
                (5, "@ TELL 'm'\n  \"say hi\"".to_string()),
            ]
        );
    }

    #[test]
    fn stray_text_before_any_statement_is_kept_so_it_gets_reported() {
        assert_eq!(split_statements("hello\n£ SEE MY /tmp"), vec![(1, "hello".into()), (2, "£ SEE MY /tmp".into())]);
    }

    #[test]
    fn an_invalid_later_line_blocks_the_whole_block() {
        let statements = split_statements("£ CREATE MY /tmp/xact-block-test-never\n£ COPY THAT\n£ BOGUS");
        let problems = validate_block(&Session::new(), &statements);
        let lines: Vec<usize> = problems.iter().map(|(line, _)| *line).collect();
        assert!(lines.contains(&2) && lines.contains(&3), "{problems:?}");
        assert!(!std::path::Path::new("/tmp/xact-block-test-never").exists());
    }

    /// The forced picker's candidate source, exactly as the live menu
    /// will call it: right after `@ TELL`'s space, the full deterministic
    /// list is offered with the span sitting at the pure insertion point
    /// the trailing space left behind.
    #[test]
    #[ignore = "live: requires ollama with at least one model pulled"]
    fn model_menu_completer_forces_the_full_list_right_after_tell() {
        // The catalog fills in the background (it never blocks the
        // editor), so wait for it to warm before comparing.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while agent_model_selection("@ TELL ").is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mut completer = ModelMenuCompleter;
        let suggestions = completer.complete("@ TELL ", "@ TELL ".len());
        let expected = agent_model_selection("@ TELL ");
        assert!(!expected.is_empty(), "this machine has Ollama models installed; the forced list must not be empty");
        assert_eq!(suggestions.iter().map(|s| s.value.clone()).collect::<Vec<_>>(), expected);
        for suggestion in &suggestions {
            assert_eq!(suggestion.span, Span { start: 7, end: 7 }, "an empty target offers a pure insertion point");
            assert!(suggestion.append_whitespace, "a picked model is a finished token");
        }
    }

    #[test]
    fn model_menu_completer_narrows_and_replaces_a_partial_target() {
        let mut completer = ModelMenuCompleter;
        let suggestions = completer.complete("@ TELL gem", "@ TELL gem".len());
        assert_eq!(
            suggestions.iter().map(|s| s.value.clone()).collect::<Vec<_>>(),
            agent_model_selection("@ TELL gem")
        );
        assert!(
            suggestions.iter().all(|s| s.span == Span { start: 7, end: 10 }),
            "the partial 'gem' is what gets replaced"
        );
    }

    #[test]
    fn model_menu_completer_has_nothing_to_say_anywhere_else() {
        let mut completer = ModelMenuCompleter;
        for line in ["£ SEE ", "@ TELL", "@ TEAM ", "@ TELL gemma3 ", "£ RUN 'ls"] {
            assert!(completer.complete(line, line.len()).is_empty(), "'{line}' is not TELL's target slot");
        }
    }

    /// Space is the one delimiter that must reach a menu activation —
    /// the forced picker — while quotes stay plain delimiters.
    #[test]
    fn space_opens_the_model_menu_quotes_do_not() {
        assert_eq!(
            char_binding(' '),
            ReedlineEvent::Multiple(vec![
                ReedlineEvent::Esc,
                ReedlineEvent::Edit(vec![EditCommand::InsertChar(' ')]),
                ReedlineEvent::Menu(MODEL_MENU.to_string()),
            ])
        );
        assert_eq!(
            char_binding('\''),
            ReedlineEvent::Multiple(
                vec![ReedlineEvent::Esc, ReedlineEvent::Edit(vec![EditCommand::InsertChar('\'')]),]
            )
        );
    }

    /// The menu wrapper's whole job: an empty menu must be
    /// indistinguishable from a closed one, a stocked one untouched.
    struct StubMenu {
        active: bool,
        values: Vec<Suggestion>,
    }

    impl Menu for StubMenu {
        fn name(&self) -> &str {
            "stub"
        }
        fn indicator(&self) -> &str {
            ""
        }
        fn is_active(&self) -> bool {
            self.active
        }
        fn menu_event(&mut self, event: MenuEvent) {
            match event {
                MenuEvent::Activate(_) => self.active = true,
                MenuEvent::Deactivate => self.active = false,
                _ => {}
            }
        }
        fn can_quick_complete(&self) -> bool {
            false
        }
        fn can_partially_complete(&mut self, _: bool, _: &mut Editor, _: &mut dyn Completer) -> bool {
            false
        }
        fn update_values(&mut self, _: &mut Editor, _: &mut dyn Completer) {}
        fn update_working_details(&mut self, _: &mut Editor, _: &mut dyn Completer, _: &Painter) {}
        fn replace_in_buffer(&self, _: &mut Editor) {}
        fn menu_required_lines(&self, _: u16) -> u16 {
            0
        }
        fn menu_string(&self, _: u16, _: bool) -> String {
            String::new()
        }
        fn min_rows(&self) -> u16 {
            0
        }
        fn get_values(&self) -> &[Suggestion] {
            &self.values
        }
    }

    #[test]
    fn self_hiding_menu_shuts_off_when_empty_and_stays_on_when_stocked() {
        let mut empty = SelfHidingMenu::new(Box::new(StubMenu { active: true, values: Vec::new() }));
        empty.deactivate_if_empty();
        assert!(!empty.is_active(), "an empty menu must be indistinguishable from a closed one");

        let mut stocked = SelfHidingMenu::new(Box::new(StubMenu {
            active: true,
            values: vec![Suggestion { value: "gemma3".into(), ..Suggestion::default() }],
        }));
        stocked.deactivate_if_empty();
        assert!(stocked.is_active(), "a menu with values must stay up");
    }

    /// The selected model, inline in blue font — the request, verbatim:
    /// once `@ TELL` has its target (picked or typed), that token and
    /// nothing else renders blue, quoted or bare.
    #[test]
    fn the_tell_target_renders_inline_in_blue() {
        let blue = Style::new().fg(Color::Blue);

        let styled = XactHighlighter.highlight("@ TELL gemma3 BE \"reviewer\"", 0);
        assert_eq!(
            styled.buffer,
            vec![
                (Style::default(), "@ TELL ".to_string()),
                (blue, "gemma3".to_string()),
                (Style::default(), " BE \"reviewer\"".to_string()),
            ]
        );

        let styled = XactHighlighter.highlight("@ TELL 'gemma3'", 0);
        assert_eq!(
            styled.buffer,
            vec![(Style::default(), "@ TELL ".to_string()), (blue, "'gemma3'".to_string())],
            "a quoted target's span covers its quotes, like every StringLit span"
        );
    }

    #[test]
    fn nothing_else_gains_color_and_invalid_still_goes_red() {
        let styled = XactHighlighter.highlight("£ SEE MY ~/docs", 0);
        assert_eq!(styled.buffer, vec![(Style::default(), "£ SEE MY ~/docs".to_string())]);

        let styled = XactHighlighter.highlight("hello", 0);
        assert_eq!(styled.buffer, vec![(Style::new().fg(Color::Red), "hello".to_string())]);

        // A draft that's blue at the target and unrecoverably wrong after
        // it keeps both: the model stays blue, the broken tail goes red.
        let styled = XactHighlighter.highlight("@ TELL gemma3 9", 0);
        assert_eq!(
            styled.buffer,
            vec![
                (Style::default(), "@ TELL ".to_string()),
                (Style::new().fg(Color::Blue), "gemma3".to_string()),
                (Style::default(), " ".to_string()),
                (Style::new().fg(Color::Red), "9".to_string()),
            ]
        );
    }
}
