//! Operator-supplied configuration for the `ai` plugin, read from env vars
//! set in the kernel's `plugins.d/ai.yaml` (the plugin reads no config file
//! itself — same mechanism as `AI_PLUGIN_ALLOWED_KEY_ENVS`). Three optional
//! JSON vars:
//!
//! - `AI_PLUGIN_MODELS` — models declared by hand (required for providers
//!   without a discovery API, e.g. Anthropic).
//! - `AI_PLUGIN_AGENTS` — named agent profiles (model + framing).
//! - `AI_PLUGIN_DISCOVERY` — providers whose model lists are pulled
//!   automatically (`refresh_models` action / on startup).

use crate::db::{Agent, Model};

pub const MODELS_ENV: &str = "AI_PLUGIN_MODELS";
pub const AGENTS_ENV: &str = "AI_PLUGIN_AGENTS";
pub const DISCOVERY_ENV: &str = "AI_PLUGIN_DISCOVERY";

/// A source to pull a model list from. `provider` is `"ollama"` or `"openai"`;
/// `ollama` hits `GET {base_url}/api/tags`, `openai` hits
/// `GET {base_url}/models` with `Authorization: Bearer <key>`.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Default)]
pub struct DiscoverySource {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: String,
}

/// Parsed plugin config. Env vars are optional JSON; absent/malformed vars
/// degrade to empty config (the plugin still serves configured models).
#[derive(Debug, Clone, Default)]
pub struct AiConfig {
    pub models: Vec<Model>,
    pub agents: Vec<Agent>,
    pub discovery: Vec<DiscoverySource>,
}

fn parse_json<T: serde::de::DeserializeOwned>(name: &str, raw: &str) -> Result<Vec<T>, String> {
    serde_json::from_str::<Vec<T>>(raw).map_err(|e| format!("{name} is not valid JSON: {e}"))
}

/// Read the three `AI_PLUGIN_*` env vars and parse them into [`AiConfig`].
pub fn from_env() -> AiConfig {
    let mut cfg = AiConfig::default();

    if let Some(raw) = std::env::var(MODELS_ENV).ok().filter(|s| !s.is_empty()) {
        match parse_json::<Model>(MODELS_ENV, &raw) {
            Ok(models) => cfg.models = models,
            Err(e) => eprintln!("[ai] config: {e} (ignoring {MODELS_ENV})"),
        }
    }
    if let Some(raw) = std::env::var(AGENTS_ENV).ok().filter(|s| !s.is_empty()) {
        match parse_json::<Agent>(AGENTS_ENV, &raw) {
            Ok(agents) => cfg.agents = agents,
            Err(e) => eprintln!("[ai] config: {e} (ignoring {AGENTS_ENV})"),
        }
    }
    if let Some(raw) = std::env::var(DISCOVERY_ENV).ok().filter(|s| !s.is_empty()) {
        match parse_json::<DiscoverySource>(DISCOVERY_ENV, &raw) {
            Ok(sources) => cfg.discovery = sources,
            Err(e) => eprintln!("[ai] config: {e} (ignoring {DISCOVERY_ENV})"),
        }
    }
    cfg
}

/// Seed the database with declared models/agents, then enforce a single
/// default per table: a config-declared `is_default` wins, otherwise the
/// first stored row.
pub fn seed(db: &crate::db::AiDb, cfg: &AiConfig) -> anyhow::Result<()> {
    let mut default_model: Option<String> = None;
    for m in &cfg.models {
        db.upsert_model(m)?;
        if m.is_default {
            default_model = Some(m.id.clone());
        }
    }
    let mut default_agent: Option<String> = None;
    for a in &cfg.agents {
        db.upsert_agent(a)?;
        if a.is_default {
            default_agent = Some(a.id.clone());
        }
    }

    let model_default = default_model.or_else(|| {
        db.list_models()
            .ok()
            .and_then(|ms| ms.into_iter().find(|m| m.is_default).map(|m| m.id))
            .or_else(|| {
                db.list_models()
                    .ok()
                    .and_then(|ms| ms.first().map(|m| m.id.clone()))
            })
    });
    if let Some(id) = model_default {
        db.set_model_default(&id)?;
    }

    let agent_default = default_agent.or_else(|| {
        db.list_agents()
            .ok()
            .and_then(|as_| as_.into_iter().find(|a| a.is_default).map(|a| a.id))
            .or_else(|| {
                db.list_agents()
                    .ok()
                    .and_then(|as_| as_.first().map(|a| a.id.clone()))
            })
    });
    if let Some(id) = agent_default {
        db.set_agent_default(&id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_models_json() {
        let raw = r#"[
            {"id":"claude-sonnet-5","provider":"anthropic","api_key_env":"ANTHROPIC_API_KEY","is_default":true},
            {"id":"llama3.2","provider":"openai","base_url":"http://localhost:11434/v1","api_key_env":"OLLAMA_API_KEY"}
        ]"#;
        let models = parse_json::<Model>(MODELS_ENV, raw).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].provider, "anthropic");
        assert!(models[0].is_default);
        assert_eq!(models[1].base_url, "http://localhost:11434/v1");
    }

    #[test]
    fn parses_agents_json() {
        let raw = r#"[
            {"id":"code","name":"Coder","model_id":"qwen2.5-coder","system_prompt":"code only","goal":"help","description":"dev","is_default":true}
        ]"#;
        let agents = parse_json::<Agent>(AGENTS_ENV, raw).unwrap();
        assert_eq!(agents[0].id, "code");
        assert_eq!(agents[0].model_id, "qwen2.5-coder");
        assert_eq!(agents[0].system_prompt, "code only");
    }

    #[test]
    fn parses_discovery_json() {
        let raw = r#"[
            {"provider":"ollama","base_url":"http://localhost:11434"},
            {"provider":"openai","base_url":"https://api.openai.com/v1","api_key_env":"OPENAI_API_KEY"}
        ]"#;
        let sources = parse_json::<DiscoverySource>(DISCOVERY_ENV, raw).unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].provider, "ollama");
        assert_eq!(sources[1].api_key_env, "OPENAI_API_KEY");
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_json::<Model>(MODELS_ENV, "not json").is_err());
    }

    #[test]
    fn empty_env_is_default_config() {
        // No env vars set in test process → from_env must not panic.
        let cfg = from_env();
        assert!(cfg.models.is_empty() && cfg.agents.is_empty() && cfg.discovery.is_empty());
    }
}
