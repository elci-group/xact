//! `@ TELL <model>` over the real Anthropic API — active only when
//! `ANTHROPIC_API_KEY` is set, same boundary as every other provider in
//! this crate. Request/response shapes verified this session against the
//! bundled `claude-api` skill's `curl/examples.md` (not recalled from
//! training): `GET /v1/models` -> `{"data": [{"id": ...}, ...]}`, `POST
//! /v1/messages` -> `{"content": [{"type": "text", "text": ...}, ...]}`.
//! Every request needs both `x-api-key` and `anthropic-version` — the
//! API rejects a request missing either.

use std::time::Duration;

use crate::TellError;

const BASE_URL: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// A real conversational reply comfortably fits well under this; it's a
/// ceiling against runaway cost, not a target length.
const MAX_TOKENS: u32 = 4096;

/// Every model this API key can see, or an empty list if
/// `ANTHROPIC_API_KEY` isn't set or the call fails for any reason — same
/// best-effort contract as [`crate::openai::models`].
pub fn models() -> Vec<String> {
    let Ok(key) = std::env::var("ANTHROPIC_API_KEY") else {
        return Vec::new();
    };
    let Ok(response) = ureq::get(&format!("{BASE_URL}/models"))
        .set("x-api-key", &key)
        .set("anthropic-version", ANTHROPIC_VERSION)
        .timeout(REQUEST_TIMEOUT)
        .call()
    else {
        return Vec::new();
    };
    let Ok(body) = response.into_string() else {
        return Vec::new();
    };
    parse_models_response(&body)
}

/// Extracted from [`models`] so it's unit-testable against hand-written
/// fixture JSON without a network call.
fn parse_models_response(body: &str) -> Vec<String> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    json.get("data")
        .and_then(|data| data.as_array())
        .map(|entries| entries.iter().filter_map(|entry| entry.get("id")?.as_str()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Runs `prompt` through `model` via `POST /v1/messages`, returning
/// Claude's reply text.
pub fn run(model: &str, prompt: &str) -> Result<String, TellError> {
    let key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| TellError("ANTHROPIC_API_KEY is not set; can't call the Anthropic API".to_string()))?;
    let body = serde_json::json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "messages": [{"role": "user", "content": prompt}],
    });
    let response = ureq::post(&format!("{BASE_URL}/messages"))
        .set("x-api-key", &key)
        .set("anthropic-version", ANTHROPIC_VERSION)
        .set("content-type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send_string(&body.to_string())
        .map_err(|err| TellError(format!("Anthropic request failed: {}", describe_ureq_error(err))))?;
    let text = response.into_string().map_err(|err| TellError(format!("failed to read Anthropic response: {err}")))?;
    parse_messages_response(&text)
}

/// Extracted from [`run`] so it's unit-testable against hand-written
/// fixture JSON without a network call.
fn parse_messages_response(body: &str) -> Result<String, TellError> {
    let json = serde_json::from_str::<serde_json::Value>(body)
        .map_err(|err| TellError(format!("could not parse Anthropic response: {err} — body: {body}")))?;
    json.get("content")
        .and_then(|content| content.as_array())
        .and_then(|blocks| blocks.iter().find(|block| block.get("type").and_then(|t| t.as_str()) == Some("text")))
        .and_then(|block| block.get("text"))
        .and_then(|text| text.as_str())
        .map(str::to_string)
        .ok_or_else(|| TellError(format!("Anthropic response had no reply text — body: {body}")))
}

/// `ureq::Error::Status` already carries the response body (often the
/// most useful part of an API error) — surface it instead of just the
/// status code.
fn describe_ureq_error(err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, response) => {
            format!("HTTP {code}: {}", response.into_string().unwrap_or_default())
        }
        ureq::Error::Transport(transport) => transport.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_ids_from_a_real_shaped_response() {
        let body = r#"{"data":[
            {"id":"claude-opus-5","display_name":"Claude Opus 5"},
            {"id":"claude-sonnet-5","display_name":"Claude Sonnet 5"}
        ],"has_more":false}"#;
        assert_eq!(parse_models_response(body), vec!["claude-opus-5".to_string(), "claude-sonnet-5".to_string()]);
    }

    #[test]
    fn models_response_with_no_data_field_is_empty_not_a_panic() {
        assert!(parse_models_response("{}").is_empty());
        assert!(parse_models_response("not json").is_empty());
    }

    #[test]
    fn parses_reply_text_from_a_real_shaped_messages_response() {
        let body = r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"hello there"}],"stop_reason":"end_turn"}"#;
        assert_eq!(parse_messages_response(body).unwrap(), "hello there");
    }

    #[test]
    fn messages_response_with_no_text_block_is_a_clear_error_not_a_panic() {
        assert!(parse_messages_response("{}").is_err());
        assert!(parse_messages_response("not json").is_err());
    }
}
