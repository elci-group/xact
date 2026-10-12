//! `@ TELL <model>` over the real OpenAI API — active only when
//! `OPENAI_API_KEY` is set, exactly like every other provider in this
//! crate (spec's "compose, don't reimplement" boundary, same as
//! `ollama`/`claude`). Request/response shapes and endpoints live in
//! [`crate::openai_compatible`], the client OpenAI itself defined and
//! that several other real providers (Groq, xAI, OpenRouter, Moonshot)
//! use verbatim — this module just names OpenAI's own base URL and key.

use crate::{openai_compatible, TellError};

const BASE_URL: &str = "https://api.openai.com/v1";

/// Every model this API key can see, or an empty list if
/// `OPENAI_API_KEY` isn't set or the call fails for any reason.
pub fn models() -> Vec<String> {
    openai_compatible::models(BASE_URL, "OPENAI_API_KEY")
}

/// Runs `prompt` through `model` via `POST /v1/chat/completions`,
/// returning the assistant's reply text.
pub fn run(model: &str, prompt: &str) -> Result<String, TellError> {
    openai_compatible::run("OpenAI", BASE_URL, "OPENAI_API_KEY", model, prompt)
}
