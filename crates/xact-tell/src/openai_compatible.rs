//! Shared client for any provider that speaks the same wire format
//! OpenAI itself defined: `GET <base>/models` -> `{"data":[{"id":...}]}`
//! and `POST <base>/chat/completions` -> `{"choices":[{"message":
//! {"content":...}}]}`. Several real providers (Groq, xAI, OpenRouter,
//! Moonshot/Kimi, alongside OpenAI itself) use this verbatim — verified
//! against each provider's own docs this session, not assumed from
//! "OpenAI-compatible" marketing copy alone. Implemented once here
//! instead of once per provider file; each provider module (`openai.rs`,
//! `groq.rs`, `xai.rs`, `openrouter.rs`, `moonshot.rs`) is a thin
//! wrapper naming its own base URL and API-key env var.

use std::time::Duration;

use crate::TellError;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Every model `api_key_env`'s key can see on `base_url`, or an empty
/// list if that env var isn't set or the call fails for any reason —
/// the same best-effort "contribute nothing rather than fail the whole
/// catalog" contract every source in `catalog::known_targets` follows.
pub fn models(base_url: &str, api_key_env: &str) -> Vec<String> {
    let Ok(key) = std::env::var(api_key_env) else {
        return Vec::new();
    };
    let Ok(response) = ureq::get(&format!("{base_url}/models"))
        .set("Authorization", &format!("Bearer {key}"))
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

/// Runs `prompt` through `model` via `POST <base_url>/chat/completions`,
/// returning the assistant's reply text. `provider` names the caller
/// only for error messages ("Groq request failed: ..." vs "xAI request
/// failed: ...").
pub fn run(provider: &str, base_url: &str, api_key_env: &str, model: &str, prompt: &str) -> Result<String, TellError> {
    let key = std::env::var(api_key_env)
        .map_err(|_| TellError(format!("{api_key_env} is not set; can't call the {provider} API")))?;
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
    });
    let response = ureq::post(&format!("{base_url}/chat/completions"))
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send_string(&body.to_string())
        .map_err(|err| TellError(format!("{provider} request failed: {}", describe_ureq_error(err))))?;
    let text = response.into_string().map_err(|err| TellError(format!("failed to read {provider} response: {err}")))?;
    parse_chat_response(provider, &text)
}

/// Extracted from [`run`] so it's unit-testable against hand-written
/// fixture JSON without a network call.
fn parse_chat_response(provider: &str, body: &str) -> Result<String, TellError> {
    let json = serde_json::from_str::<serde_json::Value>(body)
        .map_err(|err| TellError(format!("could not parse {provider} response: {err} — body: {body}")))?;
    json.get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(str::to_string)
        .ok_or_else(|| TellError(format!("{provider} response had no reply text — body: {body}")))
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
        let body = r#"{"object":"list","data":[
            {"id":"llama-3.3-70b-versatile","object":"model"},
            {"id":"mixtral-8x7b-32768","object":"model"}
        ]}"#;
        assert_eq!(
            parse_models_response(body),
            vec!["llama-3.3-70b-versatile".to_string(), "mixtral-8x7b-32768".to_string()]
        );
    }

    #[test]
    fn models_response_with_no_data_field_is_empty_not_a_panic() {
        assert!(parse_models_response("{}").is_empty());
        assert!(parse_models_response("not json").is_empty());
    }

    #[test]
    fn parses_reply_text_from_a_real_shaped_chat_response() {
        let body = r#"{"choices":[{"index":0,"message":{"role":"assistant","content":"hello there"}}]}"#;
        assert_eq!(parse_chat_response("Test", body).unwrap(), "hello there");
    }

    #[test]
    fn chat_response_with_no_choices_is_a_clear_error_not_a_panic() {
        assert!(parse_chat_response("Test", "{}").is_err());
        assert!(parse_chat_response("Test", "not json").is_err());
    }
}
