//! Glue: validate a `chat_completion` request, dispatch to the right
//! provider adapter, send the resulting HTTP request through `network`'s
//! `http_request` action, and map the response back to `ai`'s normalized
//! shape.

use veyron_sdk::VeyronClient;

use crate::provider::{anthropic::AnthropicProvider, openai_compat::OpenAiCompatProvider, Provider};
use crate::request;

/// `network`'s `http_request` response shape (see
/// `plugins/network/src/handler.rs::HttpResponseJson`) — only the fields
/// `ai` needs to decode.
#[derive(serde::Deserialize)]
struct NetworkHttpResponse {
    status: u16,
    body: String,
    body_encoding: String,
}

/// Handle one `chat_completion` action end to end. `client` is the same
/// connection `ai` used to register with the kernel — see `main.rs` for why
/// a second connection isn't an option. Returns the JSON to place in
/// `ActionResponse.data_json` on success, or a human-readable error
/// (never containing the resolved API key) on failure.
pub async fn handle_chat_completion(
    client: &mut VeyronClient,
    params_json: &[u8],
) -> Result<Vec<u8>, String> {
    let params = request::parse_request(params_json)?;

    let allowed_key_envs = request::parse_allowed_key_envs(
        &std::env::var(request::ALLOWED_KEY_ENVS_ENV).unwrap_or_default(),
    );
    if !request::is_allowed_key_env(&params.api_key_env, &allowed_key_envs) {
        return Err(format!(
            "api_key_env '{}' is not in the operator's {} allowlist",
            params.api_key_env,
            request::ALLOWED_KEY_ENVS_ENV
        ));
    }

    let api_key = std::env::var(&params.api_key_env).unwrap_or_default();
    if api_key.is_empty() {
        return Err(format!(
            "environment variable {} is not set",
            params.api_key_env
        ));
    }

    let provider: &dyn Provider = match params.provider {
        request::Provider::Anthropic => &AnthropicProvider,
        request::Provider::OpenAi => &OpenAiCompatProvider,
    };

    let http_req = provider.build_http_request(&params, &api_key);
    let http_req_json = serde_json::to_vec(&http_req)
        .map_err(|e| format!("failed to encode outbound http request: {e}"))?;

    let action_resp = client
        .send_action("http_request", &http_req_json, params.timeout_ms as u32)
        .await
        .map_err(|e| format!("network plugin call failed: {e}"))?;

    if action_resp.status != veyron_sdk::proto::ActionStatus::ActionOk as i32 {
        return Err(format!("network plugin error: {}", action_resp.error));
    }

    let net_resp: NetworkHttpResponse = serde_json::from_slice(&action_resp.data_json)
        .map_err(|e| format!("malformed network response: {e}"))?;

    if !(200..300).contains(&net_resp.status) {
        return Err(format!(
            "provider returned HTTP {}: {}",
            net_resp.status, net_resp.body
        ));
    }

    let body_bytes: Vec<u8> = match net_resp.body_encoding.as_str() {
        "base64" => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(&net_resp.body)
                .map_err(|e| format!("malformed base64 response body: {e}"))?
        }
        _ => net_resp.body.into_bytes(),
    };

    let result = provider.parse_response(&body_bytes)?;
    serde_json::to_vec(&result).map_err(|e| format!("failed to encode response: {e}"))
}
