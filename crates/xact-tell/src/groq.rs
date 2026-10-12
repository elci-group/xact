//! `@ TELL <model>` over the real Groq API — active only when
//! `GROQ_API_KEY` is set. Groq's API is OpenAI-compatible verbatim (its
//! own docs serve it at an `/openai/v1` path specifically to say so) —
//! verified against Groq's own docs this session, not assumed from
//! "OpenAI-compatible" marketing copy alone.

use crate::{openai_compatible, TellError};

const BASE_URL: &str = "https://api.groq.com/openai/v1";

/// Every model this API key can see, or an empty list if
/// `GROQ_API_KEY` isn't set or the call fails for any reason.
pub fn models() -> Vec<String> {
    openai_compatible::models(BASE_URL, "GROQ_API_KEY")
}

/// Runs `prompt` through `model` via `POST /openai/v1/chat/completions`,
/// returning the assistant's reply text.
pub fn run(model: &str, prompt: &str) -> Result<String, TellError> {
    openai_compatible::run("Groq", BASE_URL, "GROQ_API_KEY", model, prompt)
}
