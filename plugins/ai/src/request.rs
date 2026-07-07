//! Parse + validate the JSON body of a `chat_completion` `ActionRequest`.

/// Hard ceiling on `timeout_ms`; matches `network`'s own cap so a
/// `chat_completion` call can't outlive the HTTP request it wraps.
pub const MAX_TIMEOUT_MS: u64 = 30_000;

/// Default `max_tokens` when the caller omits it.
pub const DEFAULT_MAX_TOKENS: u32 = 1024;

/// Hard ceiling on `max_tokens`. Clamped, never rejected.
pub const MAX_MAX_TOKENS: u32 = 8192;

pub const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCompletionParams {
    pub provider: Provider,
    pub base_url: String,
    pub model: String,
    pub api_key_env: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub timeout_ms: u64,
}

/// Parse and validate `params_json` for the `chat_completion` action.
/// Returns a human-readable error message on any validation failure —
/// caller maps that straight into `ActionResponse.error`.
pub fn parse_request(params_json: &[u8]) -> Result<ChatCompletionParams, String> {
    #[derive(serde::Deserialize)]
    struct RawMessage {
        role: String,
        content: String,
    }

    #[derive(serde::Deserialize)]
    struct Raw {
        provider: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
        api_key_env: Option<String>,
        messages: Option<Vec<RawMessage>>,
        max_tokens: Option<u32>,
        timeout_ms: Option<u64>,
    }

    let raw: Raw =
        serde_json::from_slice(params_json).map_err(|e| format!("invalid JSON: {e}"))?;

    let provider_str = raw.provider.ok_or("missing required field: provider")?;
    let provider = match provider_str.as_str() {
        "anthropic" => Provider::Anthropic,
        "openai" => Provider::OpenAi,
        other => return Err(format!("unsupported provider: {other}")),
    };

    let base_url = match (raw.base_url, provider) {
        (Some(u), _) if !u.is_empty() => u,
        (_, Provider::Anthropic) => DEFAULT_ANTHROPIC_BASE_URL.to_string(),
        (_, Provider::OpenAi) => return Err("missing required field: base_url".to_string()),
    };

    let model = raw.model.ok_or("missing required field: model")?;
    if model.is_empty() {
        return Err("model must not be empty".to_string());
    }

    let api_key_env = raw
        .api_key_env
        .ok_or("missing required field: api_key_env")?;
    if api_key_env.is_empty() {
        return Err("api_key_env must not be empty".to_string());
    }

    let raw_messages = raw.messages.ok_or("missing required field: messages")?;
    if raw_messages.is_empty() {
        return Err("messages must not be empty".to_string());
    }
    let messages = raw_messages
        .into_iter()
        .map(|m| Message {
            role: m.role,
            content: m.content,
        })
        .collect();

    let max_tokens = raw
        .max_tokens
        .unwrap_or(DEFAULT_MAX_TOKENS)
        .min(MAX_MAX_TOKENS);
    let timeout_ms = raw.timeout_ms.unwrap_or(MAX_TIMEOUT_MS).min(MAX_TIMEOUT_MS);

    Ok(ChatCompletionParams {
        provider,
        base_url,
        model,
        api_key_env,
        messages,
        max_tokens,
        timeout_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_anthropic_json() -> serde_json::Value {
        serde_json::json!({
            "provider": "anthropic",
            "model": "claude-sonnet-5",
            "api_key_env": "ANTHROPIC_API_KEY",
            "messages": [{"role": "user", "content": "hi"}],
        })
    }

    #[test]
    fn accepts_minimal_anthropic_request() {
        let body = valid_anthropic_json().to_string();
        let params = parse_request(body.as_bytes()).unwrap();
        assert_eq!(params.provider, Provider::Anthropic);
        assert_eq!(params.base_url, DEFAULT_ANTHROPIC_BASE_URL);
        assert_eq!(params.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(params.messages.len(), 1);
    }

    #[test]
    fn rejects_missing_provider() {
        let mut body = valid_anthropic_json();
        body.as_object_mut().unwrap().remove("provider");
        let err = parse_request(body.to_string().as_bytes()).unwrap_err();
        assert!(err.contains("provider"), "error was: {err}");
    }

    #[test]
    fn rejects_unsupported_provider() {
        let mut body = valid_anthropic_json();
        body["provider"] = "gemini".into();
        let err = parse_request(body.to_string().as_bytes()).unwrap_err();
        assert!(err.contains("unsupported provider"), "error was: {err}");
    }

    #[test]
    fn openai_requires_base_url() {
        let mut body = valid_anthropic_json();
        body["provider"] = "openai".into();
        let err = parse_request(body.to_string().as_bytes()).unwrap_err();
        assert!(err.contains("base_url"), "error was: {err}");
    }

    #[test]
    fn openai_accepts_explicit_base_url() {
        let mut body = valid_anthropic_json();
        body["provider"] = "openai".into();
        body["base_url"] = "http://localhost:11434/v1".into();
        let params = parse_request(body.to_string().as_bytes()).unwrap();
        assert_eq!(params.provider, Provider::OpenAi);
        assert_eq!(params.base_url, "http://localhost:11434/v1");
    }

    #[test]
    fn rejects_missing_messages() {
        let mut body = valid_anthropic_json();
        body.as_object_mut().unwrap().remove("messages");
        let err = parse_request(body.to_string().as_bytes()).unwrap_err();
        assert!(err.contains("messages"), "error was: {err}");
    }

    #[test]
    fn rejects_empty_messages() {
        let mut body = valid_anthropic_json();
        body["messages"] = serde_json::json!([]);
        let err = parse_request(body.to_string().as_bytes()).unwrap_err();
        assert!(err.contains("messages"), "error was: {err}");
    }

    #[test]
    fn clamps_max_tokens_above_cap() {
        let mut body = valid_anthropic_json();
        body["max_tokens"] = 999_999.into();
        let params = parse_request(body.to_string().as_bytes()).unwrap();
        assert_eq!(params.max_tokens, MAX_MAX_TOKENS);
    }

    #[test]
    fn rejects_missing_api_key_env() {
        let mut body = valid_anthropic_json();
        body.as_object_mut().unwrap().remove("api_key_env");
        let err = parse_request(body.to_string().as_bytes()).unwrap_err();
        assert!(err.contains("api_key_env"), "error was: {err}");
    }
}
