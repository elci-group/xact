//! `@ TELL <model>` over the real xAI (Grok) API — active only when
//! `XAI_API_KEY` is set. OpenAI-compatible, verified against xAI's own
//! docs this session.

use crate::{openai_compatible, TellError};

const BASE_URL: &str = "https://api.x.ai/v1";

/// Every model this API key can see, or an empty list if `XAI_API_KEY`
/// isn't set or the call fails for any reason.
pub fn models() -> Vec<String> {
    openai_compatible::models(BASE_URL, "XAI_API_KEY")
}

/// Runs `prompt` through `model` via `POST /v1/chat/completions`,
/// returning the assistant's reply text.
pub fn run(model: &str, prompt: &str) -> Result<String, TellError> {
    openai_compatible::run("xAI", BASE_URL, "XAI_API_KEY", model, prompt)
}
