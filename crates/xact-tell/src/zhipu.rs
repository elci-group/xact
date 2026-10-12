//! `@ TELL <model>` over the real Zhipu AI / z.ai API (GLM models) —
//! active only when `ZAI_API_KEY` is set. `run` is OpenAI-compatible,
//! verified against z.ai's own docs this session — but unlike every
//! other provider in this crate, z.ai has **no confirmed live `/models`
//! endpoint**: its docs present available models as a static table, not
//! a queryable per-key list. Rather than guess at an endpoint that may
//! not exist, [`models`] returns a small hardcoded list of current GLM
//! model IDs (kept current only as of this writing) whenever the key is
//! set, instead of reusing [`crate::openai_compatible::models`] the way
//! every other provider here does. If z.ai later documents a real
//! `/models` endpoint, this should switch to the shared client like the
//! others.

use crate::{openai_compatible, TellError};

const BASE_URL: &str = "https://api.z.ai/api/paas/v4";

/// z.ai's current GLM model lineup per its own docs, as of this
/// writing — a hardcoded stand-in for a live listing endpoint that
/// doesn't exist yet (see this module's doc comment).
const KNOWN_MODELS: &[&str] = &["glm-5", "glm-4.7", "glm-4.6", "glm-4.5"];

/// [`KNOWN_MODELS`] if `ZAI_API_KEY` is set, otherwise empty — matching
/// every other provider's "nothing to offer without a key" contract,
/// even though this list isn't a live call.
pub fn models() -> Vec<String> {
    models_for(std::env::var("ZAI_API_KEY").is_ok())
}

/// Split out from [`models`] so the gate is unit-testable without
/// touching the real `ZAI_API_KEY` env var — process-global state that a
/// concurrently-running test elsewhere must not have mutated out from
/// under it (the exact hazard `harness.rs`'s tests hit with `$PATH`).
fn models_for(key_is_set: bool) -> Vec<String> {
    if key_is_set {
        KNOWN_MODELS.iter().map(|m| m.to_string()).collect()
    } else {
        Vec::new()
    }
}

/// Runs `prompt` through `model` via `POST /api/paas/v4/chat/completions`
/// (OpenAI-compatible), returning GLM's reply text.
pub fn run(model: &str, prompt: &str) -> Result<String, TellError> {
    openai_compatible::run("Zhipu (z.ai)", BASE_URL, "ZAI_API_KEY", model, prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_for_lists_the_known_models_only_when_the_key_is_set() {
        assert_eq!(models_for(true), vec!["glm-5", "glm-4.7", "glm-4.6", "glm-4.5"]);
        assert!(models_for(false).is_empty());
    }
}
