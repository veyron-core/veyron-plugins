//! Anthropic Messages API adapter (`POST {base_url}/v1/messages`).

use std::collections::HashMap;

use super::{ChatResult, HttpRequestJson, Provider, Usage};
use crate::request::ChatCompletionParams;

/// Anthropic API version pinned in the `anthropic-version` header — see
/// https://docs.anthropic.com/en/api/versioning.
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider;

impl Provider for AnthropicProvider {
    fn build_http_request(&self, params: &ChatCompletionParams, api_key: &str) -> HttpRequestJson {
        let url = format!("{}/v1/messages", params.base_url.trim_end_matches('/'));

        let messages: Vec<serde_json::Value> = params
            .messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
            .collect();

        let body = serde_json::json!({
            "model": params.model,
            "max_tokens": params.max_tokens,
            "messages": messages,
        })
        .to_string();

        let mut headers = HashMap::new();
        headers.insert("x-api-key".to_string(), api_key.to_string());
        headers.insert("anthropic-version".to_string(), ANTHROPIC_VERSION.to_string());
        headers.insert("content-type".to_string(), "application/json".to_string());

        HttpRequestJson {
            method: "POST",
            url,
            headers,
            body,
            timeout_ms: params.timeout_ms,
        }
    }

    fn parse_response(&self, body: &[u8]) -> Result<ChatResult, String> {
        #[derive(serde::Deserialize)]
        struct ContentBlock {
            #[serde(default)]
            text: String,
        }
        #[derive(serde::Deserialize)]
        struct AnthropicUsage {
            #[serde(default)]
            input_tokens: u64,
            #[serde(default)]
            output_tokens: u64,
        }
        #[derive(serde::Deserialize)]
        struct Response {
            content: Vec<ContentBlock>,
            #[serde(default)]
            stop_reason: Option<String>,
            usage: AnthropicUsage,
        }

        let resp: Response = serde_json::from_slice(body)
            .map_err(|e| format!("malformed anthropic response: {e}"))?;
        let content = resp
            .content
            .first()
            .map(|b| b.text.clone())
            .ok_or("anthropic response has no content blocks")?;

        Ok(ChatResult {
            content,
            stop_reason: resp.stop_reason.unwrap_or_default(),
            usage: Usage {
                input_tokens: resp.usage.input_tokens,
                output_tokens: resp.usage.output_tokens,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{Message, Provider as ReqProvider};

    fn params() -> ChatCompletionParams {
        ChatCompletionParams {
            provider: ReqProvider::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            model: "claude-sonnet-5".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "hi".to_string(),
            }],
            max_tokens: 1024,
            timeout_ms: 30_000,
        }
    }

    #[test]
    fn builds_request_with_auth_header_and_no_leaked_key_in_url() {
        let req = AnthropicProvider.build_http_request(&params(), "sk-secret");
        assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(req.headers.get("x-api-key").unwrap(), "sk-secret");
        assert!(!req.url.contains("sk-secret"));
        assert!(req.body.contains("claude-sonnet-5"));
    }

    #[test]
    fn parses_valid_response() {
        let body = serde_json::json!({
            "content": [{"type": "text", "text": "hello there"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 3}
        })
        .to_string();
        let result = AnthropicProvider.parse_response(body.as_bytes()).unwrap();
        assert_eq!(result.content, "hello there");
        assert_eq!(result.stop_reason, "end_turn");
        assert_eq!(result.usage.input_tokens, 5);
        assert_eq!(result.usage.output_tokens, 3);
    }

    #[test]
    fn rejects_response_with_no_content_blocks() {
        let body = serde_json::json!({
            "content": [],
            "usage": {"input_tokens": 1, "output_tokens": 0}
        })
        .to_string();
        let err = AnthropicProvider.parse_response(body.as_bytes()).unwrap_err();
        assert!(err.contains("no content blocks"), "error was: {err}");
    }

    #[test]
    fn rejects_malformed_json() {
        let err = AnthropicProvider.parse_response(b"not json").unwrap_err();
        assert!(err.contains("malformed"), "error was: {err}");
    }
}
