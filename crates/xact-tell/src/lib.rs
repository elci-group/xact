//! Real adapter for `@ TELL` (spec section 9; Xact–Mesut Integration
//! Phase 7 — "Agent operations SHALL also pass through the execution
//! architecture"). Started as `ollama`-only; now dispatches across
//! several real backends, but keeps the same boundary that always
//! applied to `ollama` (spec section 29, "No Semantic Leakage"):
//! provider/harness specifics stay entirely inside this crate — nothing
//! above it (not `xact-planner`, not `xact-mesut`, not `xact-cli`) knows
//! `ollama`, OpenAI, Anthropic, or `claude` exist; they only know "TELL
//! has a real execution path."
//!
//! [`dispatch`] tries each backend in this fixed order, matching how
//! specific each one's namespace is:
//! 1. [`harness`] — a small, hand-verified registry of real CLI coding
//!    agents whose non-interactive mode behaves like a stateless "answer
//!    this" call (`claude -p`, `kimi -p`, `gemini -p`, `cursor-agent -p`
//!    without `--force`) — deliberately excludes autonomous
//!    coding-agent invocations (`codex exec`, Meta's `muse exec`, `kilo
//!    run --auto`, `aider --message ... --yes-always`) that can edit
//!    files or run shell commands as a side effect; see `harness`'s own
//!    doc comment. Checked first since a harness binary name could never
//!    collide with a real model tag.
//! 2. Every API-key-provisioned provider — [`openai`], [`anthropic`],
//!    [`groq`], [`xai`], [`openrouter`], [`moonshot`] (Kimi's API, not
//!    to be confused with the `kimi` *harness* above — same underlying
//!    company, two different real integrations), [`zhipu`] (z.ai/GLM) —
//!    each active only when its own API key env var is set, and only
//!    for a name that's actually in that key's live model list.
//! 4. [`ollama`] — the original default: anything not resolved above is
//!    assumed to be a local Ollama model tag, and fails fast (not a raw
//!    `ollama` pull-manifest error) if it isn't actually installed.
//!
//! Not yet integrated: HuggingFace. Researched and deliberately left out
//! — HuggingFace has no per-token "models available to me" endpoint
//! matching every other provider's `{"data":[{"id":...}]}` shape; its
//! Hub catalog API lists its entire public catalog (hundreds of
//! thousands of repos) rather than an account-scoped list. Forcing it
//! into this module's existing pattern would mean either querying that
//! whole catalog (wrong scope) or hardcoding a curated shortlist blind
//! to what the user's own token can actually reach — a real product
//! decision, not a research gap this crate should quietly guess at.
//!
//! [`catalog::known_targets`] (re-exported as [`known_targets`])
//! aggregates real names from every backend above for live completion —
//! `xact-completion-graph` calls it while `@ TELL`'s target is being
//! typed. It's advisory only: `dispatch` re-resolves independently at
//! execution time and never trusts the completion list as authorization
//! to run something.
//!
//! `READING` a directory is aggregated through the real `bound` tool
//! first (spec section 4: "Xact shall therefore not reimplement
//! source-bundling semantics inside... `TELL` merely for convenience...
//! Xact shall construct and execute an appropriate `bound` operation"),
//! not walked by hand; `READING` a file is read directly. `POPULATING`,
//! if given, gets the response written to it via `bank` (establishing
//! the path first, exactly as `£ CREATE` would) then `std::fs::write`;
//! with no `POPULATING`, the response is only returned. Every backend
//! shares the exact same prompt text (persona/context/think folded into
//! one string by [`build_prompt`]) — no per-provider prompt-formatting
//! differences yet.
//!
//! `budget` (spec section 22) is applied the same way
//! `xact-process`/`xact-bound` apply it, for backends that spawn a real
//! local process (`ollama`, a harness): directive section 14 lists
//! "resource constraints" as one of the lifecycle semantics agent
//! execution must receive. A pure-API backend (every provider module
//! except `ollama`) has no local process to cgroup-constrain, so a
//! stated budget against one of those fails fast rather than being
//! silently ignored — the same "SHALL fail before the workload begins"
//! rule `ResourceBudget`'s own `unenforceable` list already enforces
//! elsewhere.
//!
//! Cancellation mid-generation is real for subprocess backends (Xact–
//! Mesut Integration Phase 8, continued): the spawned process is
//! registered with `xact-cancel` between `spawn` and `wait_with_output`,
//! so a real `CTRL-C` can stop generation the same way it stops `£ RUN`/
//! `£ BOUND`. An in-flight HTTP call to any provider is not abortable
//! this way yet — a bounded request timeout is the only ceiling on those
//! for now (documented limitation, not silently dropped).

mod anthropic;
mod catalog;
mod groq;
mod harness;
mod moonshot;
mod ollama;
mod openai;
mod openai_compatible;
mod openrouter;
mod xai;
mod zhipu;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use xact_ast::ResourceBudget;

pub use catalog::{known_targets, selectable_models};

#[derive(Debug)]
pub struct TellError(String);

impl fmt::Display for TellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TellError {}

/// A `TELL` block's inputs, already resolved to real paths by
/// `xact-planner` — this crate only knows how to actually run one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TellRequest {
    pub model: String,
    pub persona: Option<String>,
    pub reading: Option<PathBuf>,
    pub instruction: String,
    pub think: Option<u32>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Runs `request` through whichever real backend [`dispatch`] resolves
/// its `model` field to, constrained by `budget` where that backend has
/// a real process to constrain, writing the response to `populating` if
/// given (via `bank`, same as `£ CREATE`) in addition to returning it.
pub fn tell(request: &TellRequest, populating: Option<&Path>, budget: &ResourceBudget) -> Result<String, TellError> {
    let context = match &request.reading {
        Some(path) if path.is_dir() => Some(aggregate_directory(path)?),
        Some(path) => Some(
            std::fs::read_to_string(path)
                .map_err(|err| TellError(format!("failed to read {}: {err}", path.display())))?,
        ),
        None => None,
    };

    let prompt = build_prompt(request.persona.as_deref(), context.as_deref(), &request.instruction, request.think);

    let response = dispatch(&request.model, &prompt, request.think.is_some(), budget)?;

    if let Some(destination) = populating {
        xact_bank::establish(destination)
            .map_err(|err| TellError(format!("failed to establish {}: {err}", destination.display())))?;
        std::fs::write(destination, &response)
            .map_err(|err| TellError(format!("failed to write {}: {err}", destination.display())))?;
    }

    Ok(response)
}

/// Every pure-API provider this build knows, as `(display name, its
/// cached model list, its real `run`)` — one entry per provider module,
/// so adding a provider means adding a row here, not another copy of
/// the same three-line check. All share one function-pointer shape,
/// which every listed `models`/`run` already matches exactly (plain top-
/// level `fn`s, no captures), so this is a plain `const` table, not
/// something built at runtime.
type ModelsFn = fn() -> Vec<String>;
type RunFn = fn(&str, &str) -> Result<String, TellError>;
const API_PROVIDERS: &[(&str, ModelsFn, RunFn)] = &[
    ("OpenAI", catalog::openai_models, openai::run),
    ("Anthropic", catalog::anthropic_models, anthropic::run),
    ("Groq", catalog::groq_models, groq::run),
    ("xAI", catalog::xai_models, xai::run),
    ("OpenRouter", catalog::openrouter_models, openrouter::run),
    ("Moonshot (Kimi)", catalog::moonshot_models, moonshot::run),
    ("Zhipu (z.ai)", catalog::zhipu_models, zhipu::run),
];

/// Resolves `model` to a real backend and runs `prompt` through it — see
/// this module's doc comment for the fixed try-order and why it's
/// ordered that way. Falls all the way through to [`ollama::run`] (which
/// does its own real "is this actually installed" check) when nothing
/// more specific claims the name.
fn dispatch(model: &str, prompt: &str, think: bool, budget: &ResourceBudget) -> Result<String, TellError> {
    if let Some(flag) = harness::resolve(model) {
        return harness::run(model, flag, prompt, budget);
    }
    for (name, models, run) in API_PROVIDERS {
        if models().iter().any(|candidate| candidate == model) {
            require_no_resource_budget(budget, name)?;
            return run(model, prompt);
        }
    }
    ollama::run(model, prompt, think, budget)
}

/// A pure-API backend has no local process for `xact_resource::apply` to
/// constrain — rather than silently running unconstrained while a
/// `SPEND`/`SAVE` policy is active, fail the same way an already-
/// unenforceable resource does elsewhere (spec section 10: "execution
/// SHALL fail before the workload begins").
fn require_no_resource_budget(budget: &ResourceBudget, backend: &str) -> Result<(), TellError> {
    if budget.is_empty() {
        Ok(())
    } else {
        Err(TellError(format!(
            "cannot honour the active resource policy: {backend} is a remote API with no local process to constrain"
        )))
    }
}

fn aggregate_directory(path: &Path) -> Result<String, TellError> {
    let temp = std::env::temp_dir().join(format!(
        "xact-tell-context-{}-{}.txt",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    xact_bound::aggregate(path, Some(&temp), &ResourceBudget::default())
        .map_err(|err| TellError(format!("failed to aggregate {}: {err}", path.display())))?;
    let content = std::fs::read_to_string(&temp)
        .map_err(|err| TellError(format!("failed to read aggregated context: {err}")))?;
    let _ = std::fs::remove_file(&temp);
    Ok(content)
}

fn build_prompt(persona: Option<&str>, context: Option<&str>, instruction: &str, think: Option<u32>) -> String {
    let mut prompt = String::new();
    if let Some(persona) = persona {
        prompt.push_str("You are: ");
        prompt.push_str(persona);
        prompt.push_str("\n\n");
    }
    if let Some(context) = context {
        prompt.push_str("Context:\n");
        prompt.push_str(context);
        prompt.push_str("\n\n");
    }
    if let Some(think) = think {
        prompt.push_str(&format!("(Think budget: {think}.)\n\n"));
    }
    prompt.push_str(instruction);
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_includes_every_stated_part_in_order() {
        let prompt = build_prompt(Some("a careful reviewer"), Some("fn main() {}"), "Review this.", Some(80));
        let persona_at = prompt.find("You are: a careful reviewer").unwrap();
        let context_at = prompt.find("fn main() {}").unwrap();
        let think_at = prompt.find("Think budget: 80").unwrap();
        let instruction_at = prompt.find("Review this.").unwrap();
        assert!(persona_at < context_at && context_at < think_at && think_at < instruction_at);
    }

    #[test]
    fn build_prompt_with_nothing_stated_is_just_the_instruction() {
        assert_eq!(build_prompt(None, None, "Review this.", None), "Review this.");
    }

    /// Exercises the real `ollama` binary end to end — including a real
    /// `READING` directory aggregated through the real `bound` tool first
    /// — and is slow (tens of seconds: local model load plus generation).
    /// Kept to a single small model and a short deterministic instruction
    /// to bound that cost; still a genuine network-free local inference
    /// call, not a stub.
    fn tell_runs_the_real_ollama_model_and_populates_a_real_file() {
        let dir = std::env::temp_dir().join(format!("xact-tell-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let out = dir.join("response.txt");

        let request = TellRequest {
            model: "gemma3".into(),
            persona: None,
            reading: Some(dir.clone()),
            instruction: "Reply with exactly one word: hello".into(),
            think: None,
        };

        let result = tell(&request, Some(&out), &ResourceBudget::default());

        assert!(result.is_ok(), "ollama should succeed: {result:?}");
        let response = result.unwrap();
        assert!(!response.is_empty(), "ollama should return a non-empty response");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), response, "POPULATING should hold the same response");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A model that plain doesn't exist anywhere (not a harness, not a
    /// live provider model, not an installed Ollama tag) must fail fast,
    /// with a message naming the problem — not minutes later with
    /// `ollama`'s own raw pull-manifest error. Real (talks to the
    /// actually-installed `ollama list` on this machine), but fast: no
    /// model load or generation ever starts.
    #[test]
    fn unknown_model_fails_fast_with_a_clear_message() {
        let request = TellRequest {
            model: "definitely-not-a-real-model-xact-test".into(),
            persona: None,
            reading: None,
            instruction: "hi".into(),
            think: None,
        };
        let result = tell(&request, None, &ResourceBudget::default());
        let err = result.expect_err("an uninstalled model must be rejected, not passed to ollama run");
        assert!(err.to_string().contains("definitely-not-a-real-model-xact-test"), "{err}");
    }

    #[test]
    fn an_unenforceable_resource_fails_before_launching_ollama() {
        let request = TellRequest {
            model: "gemma3".into(),
            persona: None,
            reading: None,
            instruction: "hi".into(),
            think: None,
        };
        let budget = ResourceBudget { unenforceable: vec!["GPU".into()], ..Default::default() };
        assert!(tell(&request, None, &budget).is_err());
    }

    /// Proves cancellation reaches all the way through `tell` to the real
    /// `ollama` process, not just `xact-cancel`'s own unit tests. A real
    /// model call normally takes tens of seconds (model load plus
    /// generation, per `tell_runs_the_real_ollama_model_and_populates_a_
    /// real_file`); if `cancel_all` genuinely kills it, `tell` returns in
    /// a few seconds instead — the win a real `CTRL-C` gives a user is
    /// exactly this.
    fn cancellation_actually_kills_a_running_tell_call() {
        xact_cancel::reset();
        let start = std::time::Instant::now();

        let handle = std::thread::spawn(|| {
            let request = TellRequest {
                model: "gemma3".into(),
                persona: None,
                reading: None,
                instruction: "Write a very long, detailed essay about the history of computing.".into(),
                think: None,
            };
            tell(&request, None, &ResourceBudget::default())
        });
        std::thread::sleep(std::time::Duration::from_millis(500));
        xact_cancel::cancel_all();

        let result = handle.join().expect("tell should not panic");
        let elapsed = start.elapsed();

        assert!(result.is_err(), "a cancelled ollama call should be reported as a failure, not a fabricated success");
        assert!(
            elapsed < std::time::Duration::from_secs(30),
            "cancellation should stop ollama well before it would normally finish (took {elapsed:?})"
        );

        xact_cancel::reset();
    }

    /// Both real, slow, `ollama`-invoking cases run sequentially in one
    /// test: `xact-cancel`'s registry is process-global, and cargo runs
    /// tests within a crate in parallel by default — running them as
    /// separate `#[test]`s let a `cancel_all` from one kill the other's
    /// still-running `ollama` process (observed directly: the normal-
    /// completion case failed with "ollama exited with signal: 15" when
    /// run alongside the cancellation case).
    #[test]
    fn real_ollama_end_to_end_and_cancellation() {
        tell_runs_the_real_ollama_model_and_populates_a_real_file();
        cancellation_actually_kills_a_running_tell_call();
    }
}
