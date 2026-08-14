//! Library crate for the `secrets` plugin.
//!
//! The `ConcurrentHandler` impl lives here (not in `main`) because of the
//! orphan rule: the trait comes from `veyron-sdk`, the type from this crate.

pub mod handler;
pub mod request;
pub mod vault;

use std::sync::Arc;

use veyron_sdk::concurrent::{response_envelope, ConcurrentHandler};
use veyron_sdk::proto::{ActionRequest, Envelope, PluginManifest};
use veyron_sdk::VeyronError;

pub const PLUGIN_ID: &str = "secrets";
pub const PLUGIN_VERSION: &str = "0.1.0";

impl ConcurrentHandler for handler::Handler {
    fn id(&self) -> &str {
        PLUGIN_ID
    }

    fn version(&self) -> &str {
        PLUGIN_VERSION
    }

    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            permissions: vec!["PERMISSION_SECRETS".into()],
            actions: vec![
                "secret_set".into(),
                "secret_get".into(),
                "secret_delete".into(),
                "secret_list".into(),
            ],
            ..Default::default()
        }
    }

    async fn on_action(&self, req: ActionRequest) -> Vec<Envelope> {
        let result = self
            .handle(&req.caller_plugin_id, &req.action, &req.params_json)
            .await;
        vec![response_envelope(req.action_id, result)]
    }

    async fn on_shutdown(&self) -> Result<(), VeyronError> {
        Ok(())
    }
}

/// Build the plugin handler from environment configuration. Panics on
/// missing/invalid required config (same convention as `database`).
pub fn handler_from_env() -> Arc<handler::Handler> {
    let data_dir = std::env::var("SECRETS_PLUGIN_DATA_DIR")
        .unwrap_or_else(|_| panic!("SECRETS_PLUGIN_DATA_DIR must be set (see config.example.yaml's data_dir)"));

    let master_key_raw = std::env::var("SECRETS_PLUGIN_MASTER_KEY")
        .unwrap_or_else(|_| panic!(
            "SECRETS_PLUGIN_MASTER_KEY must be set: 32 bytes as 64 hex chars or 44 base64 chars \
             (generate with: openssl rand -hex 32)"
        ));
    let master_key = vault::parse_master_key(&master_key_raw)
        .unwrap_or_else(|e| panic!("SECRETS_PLUGIN_MASTER_KEY invalid: {e}"));

    let max_name_bytes = std::env::var("SECRETS_PLUGIN_MAX_NAME_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(request::DEFAULT_MAX_NAME_BYTES);
    let max_value_bytes = std::env::var("SECRETS_PLUGIN_MAX_VALUE_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(request::DEFAULT_MAX_VALUE_BYTES);

    Arc::new(handler::Handler::new(
        data_dir.into(),
        master_key,
        max_name_bytes,
        max_value_bytes,
    ))
}
