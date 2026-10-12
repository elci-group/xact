//! `@ TELL <model>` over the real Moonshot AI (Kimi) API — active only
//! when `MOONSHOT_API_KEY` is set. OpenAI-compatible; uses the
//! international/global endpoint (`api.moonshot.ai`), not the China
//! region's `api.moonshot.cn` — verified against Moonshot's own docs
//! this session.

use crate::{openai_compatible, TellError};

const BASE_URL: &str = "https://api.moonshot.ai/v1";

/// Every model this API key can see, or an empty list if
/// `MOONSHOT_API_KEY` isn't set or the call fails for any reason.
pub fn models() -> Vec<String> {
    openai_compatible::models(BASE_URL, "MOONSHOT_API_KEY")
}

/// Runs `prompt` through `model` via `POST /v1/chat/completions`,
/// returning Kimi's reply text.
pub fn run(model: &str, prompt: &str) -> Result<String, TellError> {
    openai_compatible::run("Moonshot (Kimi)", BASE_URL, "MOONSHOT_API_KEY", model, prompt)
}
