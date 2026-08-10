//! Glue: validate a request, dispatch to the right provider, and map the
//! result back to `tts`'s normalized shape.
//!
//!   - `sherpa` (local): synthesize in-process via sherpa-onnx — no HTTP,
//!     no `network` hop.
//!   - `openai` / `elevenlabs` (cloud): build the provider HTTP request,
//!     send it through `network`'s `http_request` action, parse the audio
//!     body — same flow as `ai`'s `chat_completion` handler.

use veyron_sdk::VeyronClient;

use crate::provider::{elevenlabs::ElevenLabsProvider, openai::OpenAiProvider, AudioResult, Provider, VoiceInfo};
use crate::request::{self, AudioFormat, Provider as ProviderKind, SynthesizeParams, OPENAI_VOICES};

/// `network`'s `http_request` response shape (see
/// `plugins/network/src/handler.rs::HttpResponseJson`) — only the fields
/// `tts` needs to decode.
#[derive(serde::Deserialize)]
struct NetworkHttpResponse {
    status: u16,
    body: String,
    body_encoding: String,
}

/// Handle one `tts_synthesize` action end to end. Returns the JSON to
/// place in `ActionResponse.data_json` on success, or a human-readable
/// error (never containing a resolved API key) on failure.
pub async fn handle_tts_synthesize(
    client: &mut VeyronClient,
    params_json: &[u8],
) -> Result<Vec<u8>, String> {
    let params = request::parse_request(params_json)?;

    let result = match params.provider {
        ProviderKind::Sherpa => crate::provider::sherpa::synthesize(&params)?,
        ProviderKind::OpenAi | ProviderKind::ElevenLabs => {
            synthesize_cloud(client, &params).await?
        }
    };

    serde_json::to_vec(&result).map_err(|e| format!("failed to encode response: {e}"))
}

/// Handle one `tts_voices` action: list the voices the provider exposes.
pub async fn handle_tts_voices(
    _client: &mut VeyronClient,
    params_json: &[u8],
) -> Result<Vec<u8>, String> {
    let provider = request::parse_voices_request(params_json)?;
    let voices: Vec<VoiceInfo> = match provider {
        ProviderKind::Sherpa => crate::provider::sherpa::voices()?,
        ProviderKind::OpenAi => OPENAI_VOICES
            .iter()
            .map(|v| VoiceInfo {
                id: v.to_string(),
                name: v.to_string(),
            })
            .collect(),
        ProviderKind::ElevenLabs => {
            return Err("elevenlabs voices are per-account; list them via the ElevenLabs \
                         dashboard or GET /v1/voices with an account key"
                .to_string())
        }
    };
    serde_json::to_vec(&voices).map_err(|e| format!("failed to encode response: {e}"))
}

async fn synthesize_cloud(
    client: &mut VeyronClient,
    params: &SynthesizeParams,
) -> Result<AudioResult, String> {
    let allowed = request::parse_allowed_key_envs(
        &std::env::var(request::ALLOWED_KEY_ENVS_ENV).unwrap_or_default(),
    );
    if !request::is_allowed_key_env(&params.api_key_env, &allowed) {
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
        ProviderKind::OpenAi => &OpenAiProvider,
        ProviderKind::ElevenLabs => &ElevenLabsProvider,
        ProviderKind::Sherpa => unreachable!("sherpa handled separately"),
    };

    let http_req = provider.build_http_request(params, &api_key);
    let http_req_json = serde_json::to_vec(&http_req)
        .map_err(|e| format!("failed to encode outbound http request: {e}"))?;

    let action_timeout = request::NETWORK_MAX_TIMEOUT_MS.min(params.timeout_ms) as u32;
    let action_resp = client
        .send_action("http_request", &http_req_json, action_timeout)
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

    provider.parse_response(&body_bytes, params.format)
}

/// Convenience for tests: normalize a cloud provider's audio body without
/// a live `network` hop. Not part of the plugin's public interface.
pub fn parse_cloud_body(provider: ProviderKind, body: &[u8], format: AudioFormat) -> Result<AudioResult, String> {
    let provider: &dyn Provider = match provider {
        ProviderKind::OpenAi => &OpenAiProvider,
        ProviderKind::ElevenLabs => &ElevenLabsProvider,
        ProviderKind::Sherpa => return Err("sherpa is not a cloud provider".to_string()),
    };
    provider.parse_response(body, format)
}
