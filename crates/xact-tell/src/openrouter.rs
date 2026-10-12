//! `@ TELL <model>` over the real OpenRouter API — active only when
//! `OPENROUTER_API_KEY` is set. OpenAI-compatible, verified against
//! OpenRouter's own docs this session — with one real difference from
//! every other provider here: OpenRouter's model IDs are `provider/model`
//! slugs (e.g. `openai/gpt-4o`, `anthropic/claude-sonnet-4`), not bare
//! names, and its `/models` listing is actually public/unauthenticated —
//! it returns OpenRouter's entire catalog regardless of API key, not a
//! per-key subset. Both facts are handled correctly by reusing
//! [`openai_compatible::models`] unmodified: gating on the API key being
//! set (even though the endpoint itself wouldn't require it) keeps
//! OpenRouter's full catalog from cluttering every `xact` user's `TELL`
//! completion menu whether or not they actually have an account — it
//! only shows up for someone who's set the key, same as every other
//! provider here.

use crate::{openai_compatible, TellError};

const BASE_URL: &str = "https://openrouter.ai/api/v1";

/// OpenRouter's full model catalog (its `/models` listing is public, not
/// per-key), or an empty list if `OPENROUTER_API_KEY` isn't set — see
/// this module's doc comment for why that gate stays even though the
/// endpoint itself doesn't require it.
pub fn models() -> Vec<String> {
    openai_compatible::models(BASE_URL, "OPENROUTER_API_KEY")
}

/// Runs `prompt` through `model` (a `provider/model` slug) via `POST
/// /api/v1/chat/completions`, returning the assistant's reply text.
pub fn run(model: &str, prompt: &str) -> Result<String, TellError> {
    openai_compatible::run("OpenRouter", BASE_URL, "OPENROUTER_API_KEY", model, prompt)
}
