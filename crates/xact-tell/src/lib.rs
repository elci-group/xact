//! Real adapter for `@ TELL` over `ollama`, a local LLM runtime installed
//! on this machine (spec section 9; Xact–Mesut Integration Phase 7 —
//! "Agent operations SHALL also pass through the execution architecture").
//!
//! Xact does not implement an LLM itself. `TELL '<model>'` names a model
//! tag verbatim, passed straight through to `ollama run <model>` — the
//! same "compose, don't reimplement" boundary `CREATE` draws with `bank`
//! (spec sections 3/12/13). Provider specifics stay entirely inside this
//! crate (spec section 29, "No Semantic Leakage"): nothing above this
//! layer — not `xact-planner`, not `xact-mesut`, not `xact-cli` — knows
//! `ollama` exists; they only know "TELL has a real execution path."
//!
//! `READING` a directory is aggregated through the real `bound` tool
//! first (spec section 4: "Xact shall therefore not reimplement
//! source-bundling semantics inside... `TELL` merely for convenience...
//! Xact shall construct and execute an appropriate `bound` operation"),
//! not walked by hand; `READING` a file is read directly. `POPULATING`,
//! if given, gets the model's response written to it via `bank`
//! (establishing the path first, exactly as `£ CREATE` would) then
//! `std::fs::write`; with no `POPULATING`, the response is only returned.
//!
//! `budget` (spec section 22) is applied to the `ollama` process the same
//! way `xact-process`/`xact-bound` apply it — directive section 14 lists
//! "resource constraints" as one of the lifecycle semantics agent
//! execution must receive, same as any other workload.
//!
//! `THINK`'s numeric budget has no numeric equivalent in `ollama` — its
//! real `--think` flag only accepts `true`/`false`/`low`/`medium`/`high`.
//! Rather than inventing a fabricated number-to-level mapping, any
//! `THINK` value enables the real thinking-mode flag (`--think true`) and
//! the stated number is folded into the prompt text itself as a plain
//! instruction, letting the model interpret it in its own words.
//!
//! Cancellation mid-generation is real (Xact–Mesut Integration Phase 8,
//! continued): the spawned `ollama` process is registered with
//! `xact-cancel` between `spawn` and `wait_with_output`, so a real
//! `CTRL-C` can stop generation the same way it stops `£ RUN`/`£ BOUND`.
//! `ollama`'s own real thinking-mode/streaming behavior is otherwise used
//! as-is, uninterpreted.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use xact_ast::ResourceBudget;

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

/// Runs `request` through the real `ollama` binary, constrained by
/// `budget`, writing the response to `populating` if given (via `bank`,
/// same as `£ CREATE`) in addition to returning it.
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

    let mut command = Command::new("ollama");
    command.arg("run").arg(&request.model).arg(&prompt);
    if request.think.is_some() {
        command.arg("--think").arg("true");
    }

    let _resource_guard = xact_resource::apply(&mut command, budget)
        .map_err(|err| TellError(format!("cannot honour the active resource policy: {err}")))?;

    command.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let child = command.spawn().map_err(|err| TellError(format!("failed to run ollama: {err}")))?;
    let _cancel_guard = xact_cancel::register(child.id());
    let output = child
        .wait_with_output()
        .map_err(|err| TellError(format!("failed to wait for ollama: {err}")))?;
    if !output.status.success() {
        return Err(TellError(format!(
            "ollama exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let response = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if let Some(destination) = populating {
        xact_bank::establish(destination)
            .map_err(|err| TellError(format!("failed to establish {}: {err}", destination.display())))?;
        std::fs::write(destination, &response)
            .map_err(|err| TellError(format!("failed to write {}: {err}", destination.display())))?;
    }

    Ok(response)
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
